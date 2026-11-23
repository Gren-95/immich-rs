//! A `multipart/form-data` body framed exactly like Node's `FormData` (undici), with file parts
//! streamed from disk so large videos are never held in memory.

use std::collections::VecDeque;
use std::fs::File;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{self, Cursor, Read};
use std::path::PathBuf;

const CRLF: &str = "\r\n";

enum Part {
    Text {
        name: String,
        value: String,
    },
    File {
        name: String,
        filename: String,
        path: PathBuf,
        size: u64,
    },
}

pub struct Multipart {
    boundary: String,
    parts: Vec<Part>,
}

/// undici escapes these characters in field names and file names.
fn escape_quoted(value: &str) -> String {
    value.replace('\n', "%0A").replace('\r', "%0D").replace('"', "%22")
}

/// Text values and names have their line breaks normalised to CRLF.
fn normalize_linefeeds(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n").replace('\n', CRLF)
}

/// A random boundary. `RandomState` is seeded from the OS, so no RNG crate is needed.
pub fn random_boundary() -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(u64::from(std::process::id()));
    format!("----formdata-immich-rs-{:016x}", hasher.finish())
}

impl Multipart {
    pub fn new(boundary: String) -> Self {
        Multipart {
            boundary,
            parts: Vec::new(),
        }
    }

    pub fn text(&mut self, name: &str, value: &str) {
        self.parts.push(Part::Text {
            name: name.to_string(),
            value: value.to_string(),
        });
    }

    /// Adds a file part. `size` is the number of bytes that will be streamed from `path`.
    pub fn file(&mut self, name: &str, filename: &str, path: PathBuf, size: u64) {
        self.parts.push(Part::File {
            name: name.to_string(),
            filename: filename.to_string(),
            path,
            size,
        });
    }

    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    fn part_header(&self, part: &Part) -> String {
        let prefix = format!("--{}{CRLF}Content-Disposition: form-data", self.boundary);
        match part {
            Part::Text { name, value } => format!(
                "{prefix}; name=\"{}\"{CRLF}{CRLF}{}{CRLF}",
                escape_quoted(&normalize_linefeeds(name)),
                normalize_linefeeds(value)
            ),
            Part::File { name, filename, .. } => format!(
                "{prefix}; name=\"{}\"; filename=\"{}\"{CRLF}Content-Type: application/octet-stream{CRLF}{CRLF}",
                escape_quoted(&normalize_linefeeds(name)),
                escape_quoted(filename)
            ),
        }
    }

    fn closing(&self) -> String {
        format!("--{}--{CRLF}", self.boundary)
    }

    /// The exact number of bytes `into_reader` will produce.
    pub fn content_length(&self) -> u64 {
        let parts: u64 = self
            .parts
            .iter()
            .map(|part| {
                let header = self.part_header(part).len() as u64;
                match part {
                    Part::Text { .. } => header,
                    Part::File { size, .. } => header + size + CRLF.len() as u64,
                }
            })
            .sum();
        parts + self.closing().len() as u64
    }

    /// Opens every file and returns a reader over the whole body.
    pub fn into_reader(self) -> io::Result<BodyReader> {
        let mut segments: VecDeque<Box<dyn Read + Send>> = VecDeque::new();
        for part in &self.parts {
            segments.push_back(Box::new(Cursor::new(self.part_header(part).into_bytes())));
            if let Part::File { path, size, .. } = part {
                let file = File::open(path)?;
                segments.push_back(Box::new(ExactReader {
                    inner: file.take(*size),
                    remaining: *size,
                }));
                segments.push_back(Box::new(Cursor::new(CRLF.as_bytes())));
            }
        }
        segments.push_back(Box::new(Cursor::new(self.closing().into_bytes())));
        Ok(BodyReader { segments })
    }
}

/// Reads exactly `remaining` bytes, failing if the file shrank since it was measured,
/// so the body can never disagree with the announced Content-Length.
struct ExactReader {
    inner: io::Take<File>,
    remaining: u64,
}

impl Read for ExactReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let read = self.inner.read(buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "file changed size while uploading",
            ));
        }
        self.remaining -= read as u64;
        Ok(read)
    }
}

/// The chained multipart body.
pub struct BodyReader {
    segments: VecDeque<Box<dyn Read + Send>>,
}

impl Read for BodyReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while let Some(segment) = self.segments.front_mut() {
            let read = segment.read(buffer)?;
            if read > 0 || buffer.is_empty() {
                return Ok(read);
            }
            self.segments.pop_front();
        }
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_text_and_file_parts_like_undici() {
        let dir = std::env::temp_dir().join(format!("immich-rs-multipart-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("photo.jpg");
        std::fs::write(&path, b"JPEGDATA").expect("write test file");

        let mut body = Multipart::new("XBOUNDARY".to_string());
        body.text("fileSize", "8");
        body.file("assetData", "pho\"to.jpg", path, 8);
        let expected = "--XBOUNDARY\r\nContent-Disposition: form-data; name=\"fileSize\"\r\n\r\n8\r\n\
             --XBOUNDARY\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"pho%22to.jpg\"\r\n\
             Content-Type: application/octet-stream\r\n\r\nJPEGDATA\r\n\
             --XBOUNDARY--\r\n";

        assert_eq!(body.content_type(), "multipart/form-data; boundary=XBOUNDARY");
        assert_eq!(body.content_length(), expected.len() as u64);
        let mut actual = String::new();
        body.into_reader()
            .expect("open body")
            .read_to_string(&mut actual)
            .expect("read body");
        std::fs::remove_dir_all(&dir).expect("remove temp dir");
        assert_eq!(actual, expected);
    }

    #[test]
    fn fails_when_a_file_is_shorter_than_announced() {
        let dir = std::env::temp_dir().join(format!("immich-rs-multipart-short-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("short.jpg");
        std::fs::write(&path, b"abc").expect("write test file");
        let mut body = Multipart::new("B".to_string());
        body.file("assetData", "short.jpg", path, 10);
        let mut sink = Vec::new();
        let result = body.into_reader().expect("open body").read_to_end(&mut sink);
        std::fs::remove_dir_all(&dir).expect("remove temp dir");
        assert!(result.is_err());
    }

    #[test]
    fn normalizes_linefeeds_and_escapes_names() {
        assert_eq!(normalize_linefeeds("a\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
        assert_eq!(escape_quoted("a\"b\nc"), "a%22b%0Ac");
        assert!(random_boundary().starts_with("----formdata-immich-rs-"));
    }
}
