//! The `auth.yml` credentials file. It is a tiny YAML mapping (`url` and `key`), so it is read
//! and written by hand: plain, single-quoted and double-quoted scalars are supported.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};

use crate::error::{Error, Result, node_io_error};
use crate::pathutil;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub url: String,
    pub key: String,
}

pub fn auth_file_path(config_dir: &str) -> String {
    pathutil::join(config_dir, "auth.yml")
}

/// The default `--config-directory`: `~/.config/immich/` (trailing slash included, as in the original).
pub fn default_config_directory() -> String {
    let home = std::env::home_dir()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default();
    pathutil::join(&home, ".config/immich/")
}

/// Reads the auth file, printing the original message and exiting when it does not exist.
pub fn read_auth_file(config_dir: &str) -> Result<AuthConfig> {
    let path = auth_file_path(config_dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(20) => {
            println!("No auth file exists. Please login first.");
            return Err(Error::Exit(1));
        }
        Err(error) => return Err(Error::Fatal(node_io_error(&error, "open", &path))),
    };
    parse_auth_yaml(&text).map_err(|message| Error::Fatal(format!("Error: invalid auth file {path}: {message}")))
}

/// Writes the auth file with mode 0600 (only applied when the file is created, like Node's `writeFile`).
pub fn write_auth_file(config_dir: &str, auth: &AuthConfig) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(auth_file_path(config_dir))?;
    file.write_all(format_auth_yaml(auth).as_bytes())
}

/// Parses `auth.yml`. Accepts the legacy `instanceUrl`/`apiKey` keys as the original does.
pub fn parse_auth_yaml(text: &str) -> std::result::Result<AuthConfig, String> {
    let mut values: HashMap<String, String> = HashMap::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" || trimmed == "..." {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            return Err(format!("unsupported nested value: {trimmed}"));
        }
        let Some((key, value)) = split_mapping_line(line) else {
            return Err(format!("expected `key: value`, found: {trimmed}"));
        };
        values.insert(parse_scalar(key)?, parse_scalar(value)?);
    }

    if let (Some(url), Some(key)) = (values.get("instanceUrl"), values.get("apiKey"))
        && !url.is_empty()
        && !key.is_empty()
    {
        return Ok(AuthConfig {
            url: url.clone(),
            key: key.clone(),
        });
    }
    match (values.remove("url"), values.remove("key")) {
        (Some(url), Some(key)) => Ok(AuthConfig { url, key }),
        _ => Err("missing `url` or `key`".to_string()),
    }
}

/// Splits `key: value` at the first `: ` (or a trailing `:`) that is outside quotes.
fn split_mapping_line(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    for (index, &byte) in bytes.iter().enumerate() {
        match quote {
            Some(q) if byte == q => quote = None,
            Some(_) => {}
            None if (byte == b'"' || byte == b'\'') && index == 0 => quote = Some(byte),
            None if byte == b':' => {
                let next = bytes.get(index + 1);
                if next.is_none() || next == Some(&b' ') || next == Some(&b'\t') {
                    return Some((line[..index].trim(), line[index + 1..].trim()));
                }
            }
            None => {}
        }
    }
    None
}

/// Parses a YAML flow scalar used as a string.
fn parse_scalar(raw: &str) -> std::result::Result<String, String> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('\'') {
        let end = find_single_quote_end(rest).ok_or("unterminated single-quoted string")?;
        return Ok(rest[..end].replace("''", "'"));
    }
    if let Some(rest) = raw.strip_prefix('"') {
        return parse_double_quoted(rest);
    }
    // A plain scalar ends at ` #` (a comment).
    let value = match raw.find(" #") {
        Some(index) => raw[..index].trim_end(),
        None => raw,
    };
    Ok(value.to_string())
}

fn find_single_quote_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\'' {
            if bytes.get(index + 1) == Some(&b'\'') {
                index += 2;
                continue;
            }
            return Some(index);
        }
        index += 1;
    }
    None
}

fn parse_double_quoted(text: &str) -> std::result::Result<String, String> {
    let mut output = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Ok(output),
            '\\' => {
                let escaped = chars.next().ok_or("unterminated escape")?;
                match escaped {
                    'n' => output.push('\n'),
                    't' => output.push('\t'),
                    'r' => output.push('\r'),
                    '0' => output.push('\0'),
                    '\\' => output.push('\\'),
                    '"' => output.push('"'),
                    '/' => output.push('/'),
                    ' ' => output.push(' '),
                    'x' | 'u' | 'U' => {
                        let width = match escaped {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let hex: String = chars.by_ref().take(width).collect();
                        let code = u32::from_str_radix(&hex, 16).map_err(|_| "invalid escape")?;
                        output.push(char::from_u32(code).ok_or("invalid escape")?);
                    }
                    other => return Err(format!("unsupported escape \\{other}")),
                }
            }
            other => output.push(other),
        }
    }
    Err("unterminated double-quoted string".to_string())
}

/// Whether a string would read back as something other than the same string if written plain.
fn needs_quotes(value: &str) -> bool {
    let Some(first) = value.chars().next() else { return true };
    if value != value.trim() || value.contains(": ") || value.contains(" #") || value.ends_with(':') {
        return true;
    }
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
        return true;
    }
    if value.chars().any(|c| c.is_control()) {
        return true;
    }
    let lowered = value.to_ascii_lowercase();
    if matches!(
        lowered.as_str(),
        "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off" | ".nan" | ".inf"
    ) {
        return true;
    }
    // Anything that YAML would parse as a number.
    let numeric = value.trim_start_matches(['+', '-']);
    numeric.parse::<f64>().is_ok() || numeric.starts_with("0x") || numeric.starts_with("0o")
}

fn quote(value: &str) -> String {
    let mut output = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            '\r' => output.push_str("\\r"),
            c if c.is_control() => output.push_str(&format!("\\x{:02X}", u32::from(c))),
            c => output.push(c),
        }
    }
    output.push('"');
    output
}

fn scalar(value: &str) -> String {
    if needs_quotes(value) {
        quote(value)
    } else {
        value.to_string()
    }
}

/// Serialises the credentials like the `yaml` package's `stringify({ url, key })`.
pub fn format_auth_yaml(auth: &AuthConfig) -> String {
    format!("url: {}\nkey: {}\n", scalar(&auth.url), scalar(&auth.key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(url: &str, key: &str) -> AuthConfig {
        AuthConfig {
            url: url.to_string(),
            key: key.to_string(),
        }
    }

    #[test]
    fn writes_the_same_format_as_the_original() {
        let text = format_auth_yaml(&auth("http://192.0.2.10:2283/api", "abcDEF123_xyz"));
        assert_eq!(text, "url: http://192.0.2.10:2283/api\nkey: abcDEF123_xyz\n");
    }

    #[test]
    fn quotes_values_that_would_not_read_back_as_strings() {
        assert_eq!(scalar("-leading-dash"), "\"-leading-dash\"");
        assert_eq!(scalar("12345"), "\"12345\"");
        assert_eq!(scalar("true"), "\"true\"");
        assert_eq!(scalar("a: b"), "\"a: b\"");
        assert_eq!(scalar(""), "\"\"");
    }

    #[test]
    fn round_trips_through_parse() {
        for (url, key) in [
            ("https://photos.example.com/api", "k3y"),
            ("http://h:2283/api", "-dash"),
            ("http://h/api", "has \"quotes\" and \\ backslash"),
            ("http://h/api", "0123"),
        ] {
            let original = auth(url, key);
            assert_eq!(parse_auth_yaml(&format_auth_yaml(&original)), Ok(original));
        }
    }

    #[test]
    fn reads_quoted_commented_and_legacy_files() {
        let text = "# credentials\nurl: 'https://h/api'   \nkey: \"k\\x41y\" # comment\n";
        assert_eq!(parse_auth_yaml(text), Ok(auth("https://h/api", "kAy")));
        let legacy = "instanceUrl: https://old/api\napiKey: legacy\n";
        assert_eq!(parse_auth_yaml(legacy), Ok(auth("https://old/api", "legacy")));
        assert!(parse_auth_yaml("url: https://h/api\n").is_err());
    }

    #[test]
    fn writes_and_reads_a_file_with_private_permissions() {
        let dir = std::env::temp_dir().join(format!("immich-rs-config-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        let dir_text = dir.to_string_lossy().into_owned();
        let original = auth("http://h:2283/api", "secret");
        write_auth_file(&dir_text, &original).expect("write auth file");
        let read_back = read_auth_file(&dir_text).expect("read auth file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(auth_file_path(&dir_text))
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(&dir).expect("remove temp dir");
        assert_eq!(read_back, original);
    }
}
