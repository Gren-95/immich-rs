use std::fmt;

/// Terminates a command. The message (if any) is printed by `main`.
#[derive(Debug)]
pub enum Error {
    /// The user-facing message was already printed; exit with this code.
    Exit(u8),
    /// An unexpected failure. Printed to stderr, exit code 1.
    Fatal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Exit(code) => write!(f, "exit code {code}"),
            Error::Fatal(message) => f.write_str(message),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Formats an I/O error the way Node.js does (`ENOENT: no such file or directory, stat '/x'`),
/// so error lines read the same as the original CLI's.
pub fn node_io_error(error: &std::io::Error, syscall: &str, path: &str) -> String {
    let described = match error.raw_os_error() {
        Some(1) => Some("EPERM: operation not permitted"),
        Some(2) => Some("ENOENT: no such file or directory"),
        Some(5) => Some("EIO: i/o error"),
        Some(13) => Some("EACCES: permission denied"),
        Some(16) => Some("EBUSY: resource busy or locked"),
        Some(20) => Some("ENOTDIR: not a directory"),
        Some(21) => Some("EISDIR: illegal operation on a directory"),
        Some(24) => Some("EMFILE: too many open files"),
        Some(30) => Some("EROFS: read-only file system"),
        Some(36) => Some("ENAMETOOLONG: name too long"),
        Some(40) => Some("ELOOP: too many symbolic links encountered"),
        _ => None,
    };
    match described {
        Some(text) => format!("Error: {text}, {syscall} '{path}'"),
        None => format!("Error: {error}, {syscall} '{path}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_errno_like_node() {
        let error = std::io::Error::from_raw_os_error(2);
        assert_eq!(
            node_io_error(&error, "stat", "/tmp/x.jpg"),
            "Error: ENOENT: no such file or directory, stat '/tmp/x.jpg'"
        );
    }
}
