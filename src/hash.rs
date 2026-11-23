use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha1::{Digest, Sha1};

/// Same chunk size as Node's `fs.createReadStream`.
const READ_CHUNK_SIZE: usize = 64 * 1024;

/// The lowercase hex SHA-1 digest of everything `reader` yields.
pub fn sha1_reader(mut reader: impl Read) -> io::Result<String> {
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; READ_CHUNK_SIZE];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The lowercase hex SHA-1 digest of a file, streamed rather than loaded into memory.
pub fn sha1_file(path: &Path) -> io::Result<String> {
    sha1_reader(File::open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_vectors() {
        assert_eq!(
            sha1_reader(&b""[..]).ok(),
            Some("da39a3ee5e6b4b0d3255bfef95601890afd80709".to_string())
        );
        assert_eq!(
            sha1_reader(&b"abc"[..]).ok(),
            Some("a9993e364706816aba3e25717850c26c9cd0d89d".to_string())
        );
    }

    #[test]
    fn hashes_a_file_larger_than_one_chunk() {
        let dir = std::env::temp_dir().join(format!("immich-rs-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("known.bin");
        // One million 'a' characters: a classic SHA-1 test vector spanning many read chunks.
        std::fs::write(&path, vec![b'a'; 1_000_000]).expect("write test file");
        let digest = sha1_file(&path).expect("hash file");
        std::fs::remove_dir_all(&dir).expect("remove temp dir");
        assert_eq!(digest, "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }
}
