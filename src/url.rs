//! A small subset of the WHATWG URL standard: enough to reproduce `new URL(input, base).href`
//! for the server URLs the CLI handles (API discovery and the `.well-known/immich` lookup).

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidUrl;

impl fmt::Display for InvalidUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TypeError: Invalid URL")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Url {
    scheme: String,
    /// `user[:password]@`, kept verbatim.
    userinfo: Option<String>,
    /// `None` for URLs without an authority (opaque or path-only).
    host: Option<String>,
    port: Option<u16>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
}

fn is_special(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp" | "file")
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

/// Strips leading/trailing C0 controls and spaces, and removes tabs and newlines.
fn clean_input(input: &str) -> String {
    input
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect()
}

/// Splits a leading `scheme:` off the input, if it has one.
fn split_scheme(input: &str) -> Option<(String, &str)> {
    let colon = input.find(':')?;
    let candidate = &input[..colon];
    let mut chars = candidate.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some((candidate.to_ascii_lowercase(), &input[colon + 1..]))
}

fn percent_encode(input: &str, should_encode: impl Fn(char) -> bool) -> String {
    let mut output = String::with_capacity(input.len());
    for c in input.chars() {
        if should_encode(c) || !c.is_ascii() {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                output.push_str(&format!("%{byte:02X}"));
            }
        } else {
            output.push(c);
        }
    }
    output
}

fn encode_path(path: &str) -> String {
    percent_encode(path, |c| {
        c <= ' ' || matches!(c, '"' | '#' | '<' | '>' | '?' | '`' | '{' | '}' | '\x7f')
    })
}

fn encode_query(query: &str, special: bool) -> String {
    percent_encode(query, |c| {
        c <= ' ' || matches!(c, '"' | '#' | '<' | '>' | '\x7f') || (special && c == '\'')
    })
}

fn encode_fragment(fragment: &str) -> String {
    percent_encode(fragment, |c| c <= ' ' || matches!(c, '"' | '<' | '>' | '`' | '\x7f'))
}

/// Applies dot-segment removal to an absolute path (one starting with `/`).
fn remove_dot_segments(path: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let last_index = segments.len().saturating_sub(1);
    for (index, segment) in segments.iter().enumerate() {
        let is_last = index == last_index;
        match *segment {
            "." | "%2e" | "%2E" => {
                if is_last {
                    output.push("");
                }
            }
            ".." | ".%2e" | ".%2E" | "%2e." | "%2E." | "%2e%2e" | "%2E%2E" | "%2e%2E" | "%2E%2e" => {
                output.pop();
                if is_last {
                    output.push("");
                }
            }
            other => output.push(other),
        }
    }
    format!("/{}", output.join("/"))
}

/// Splits `rest` into path, query and fragment parts.
fn split_path_query_fragment(rest: &str) -> (&str, Option<&str>, Option<&str>) {
    let (before_fragment, fragment) = match rest.find('#') {
        Some(index) => (&rest[..index], Some(&rest[index + 1..])),
        None => (rest, None),
    };
    let (path, query) = match before_fragment.find('?') {
        Some(index) => (&before_fragment[..index], Some(&before_fragment[index + 1..])),
        None => (before_fragment, None),
    };
    (path, query, fragment)
}

fn parse_authority(scheme: &str, authority: &str) -> Result<(Option<String>, String, Option<u16>), InvalidUrl> {
    let (userinfo, host_port) = match authority.rfind('@') {
        Some(index) => (Some(authority[..index].to_string()), &authority[index + 1..]),
        None => (None, authority),
    };
    let (host, port_text) = if host_port.starts_with('[') {
        let end = host_port.find(']').ok_or(InvalidUrl)?;
        let after = &host_port[end + 1..];
        let port = match after.strip_prefix(':') {
            Some(port) => Some(port),
            None if after.is_empty() => None,
            None => return Err(InvalidUrl),
        };
        (&host_port[..=end], port)
    } else {
        match host_port.rfind(':') {
            Some(index) => (&host_port[..index], Some(&host_port[index + 1..])),
            None => (host_port, None),
        }
    };
    if host.is_empty() && is_special(scheme) && scheme != "file" {
        return Err(InvalidUrl);
    }
    if host
        .chars()
        .any(|c| c <= ' ' || matches!(c, '#' | '/' | '<' | '>' | '?' | '@' | '\\' | '^' | '|'))
    {
        return Err(InvalidUrl);
    }
    let port = match port_text {
        None | Some("") => None,
        Some(text) => {
            if !text.chars().all(|c| c.is_ascii_digit()) {
                return Err(InvalidUrl);
            }
            let value: u32 = text.parse().map_err(|_| InvalidUrl)?;
            let value = u16::try_from(value).map_err(|_| InvalidUrl)?;
            if default_port(scheme) == Some(value) {
                None
            } else {
                Some(value)
            }
        }
    };
    Ok((userinfo, host.to_ascii_lowercase(), port))
}

fn finish_path(path: &str, special: bool) -> String {
    let path = if special {
        path.replace('\\', "/")
    } else {
        path.to_string()
    };
    if path.is_empty() {
        return if special { "/".to_string() } else { String::new() };
    }
    let absolute = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    encode_path(&remove_dot_segments(&absolute))
}

/// Parses an absolute URL string.
fn parse_absolute(input: &str) -> Result<Url, InvalidUrl> {
    let cleaned = clean_input(input);
    let (scheme, rest) = split_scheme(&cleaned).ok_or(InvalidUrl)?;
    let special = is_special(&scheme);
    let rest_normalized = if special {
        rest.replace('\\', "/")
    } else {
        rest.to_string()
    };

    let has_authority = if special {
        true
    } else {
        rest_normalized.starts_with("//")
    };
    if has_authority {
        let after_slashes = if special {
            rest_normalized.trim_start_matches('/')
        } else {
            &rest_normalized[2..]
        };
        let authority_end = after_slashes.find(['/', '?', '#']).unwrap_or(after_slashes.len());
        let (userinfo, host, port) = parse_authority(&scheme, &after_slashes[..authority_end])?;
        let (path, query, fragment) = split_path_query_fragment(&after_slashes[authority_end..]);
        return Ok(Url {
            path: finish_path(path, special),
            query: query.map(|q| encode_query(q, special)),
            fragment: fragment.map(encode_fragment),
            scheme,
            userinfo,
            host: Some(host),
            port,
        });
    }

    let (path, query, fragment) = split_path_query_fragment(&rest_normalized);
    let path = if path.starts_with('/') {
        encode_path(&remove_dot_segments(path))
    } else {
        // An opaque path such as `mailto:x` or `localhost:2283`.
        percent_encode(path, |c| c < ' ' || c == '\x7f')
    };
    Ok(Url {
        scheme,
        userinfo: None,
        host: None,
        port: None,
        path,
        query: query.map(|q| encode_query(q, false)),
        fragment: fragment.map(encode_fragment),
    })
}

impl Url {
    fn is_opaque(&self) -> bool {
        self.host.is_none() && !self.path.starts_with('/')
    }

    fn href(&self) -> String {
        let mut output = format!("{}:", self.scheme);
        if let Some(host) = &self.host {
            output.push_str("//");
            if let Some(userinfo) = &self.userinfo {
                output.push_str(userinfo);
                output.push('@');
            }
            output.push_str(host);
            if let Some(port) = self.port {
                output.push_str(&format!(":{port}"));
            }
        }
        output.push_str(&self.path);
        if let Some(query) = &self.query {
            output.push('?');
            output.push_str(query);
        }
        if let Some(fragment) = &self.fragment {
            output.push('#');
            output.push_str(fragment);
        }
        output
    }
}

/// `new URL(input, base).href`.
pub fn resolve(base: &str, input: &str) -> Result<String, InvalidUrl> {
    if let Ok(absolute) = parse_absolute(input) {
        return Ok(absolute.href());
    }
    let base = parse_absolute(base)?;
    let special = is_special(&base.scheme);
    let cleaned = clean_input(input);
    let reference = if special { cleaned.replace('\\', "/") } else { cleaned };

    if base.is_opaque() {
        return match reference.strip_prefix('#') {
            Some(fragment) => Ok(Url {
                fragment: Some(encode_fragment(fragment)),
                ..base
            }
            .href()),
            None => Err(InvalidUrl),
        };
    }

    if reference.starts_with("//") {
        return parse_absolute(&format!("{}:{}", base.scheme, reference)).map(|url| url.href());
    }

    let (path, query, fragment) = split_path_query_fragment(&reference);
    let mut url = base.clone();
    url.fragment = fragment.map(encode_fragment);
    if path.is_empty() {
        if let Some(query) = query {
            url.query = Some(encode_query(query, special));
        } else if !reference.starts_with('#') && !reference.is_empty() {
            url.query = None;
        }
        if reference.is_empty() {
            url.fragment = None;
        }
        return Ok(url.href());
    }

    url.query = query.map(|q| encode_query(q, special));
    let merged = if path.starts_with('/') {
        path.to_string()
    } else {
        let directory = match base.path.rfind('/') {
            Some(index) => &base.path[..=index],
            None => "/",
        };
        format!("{directory}{path}")
    };
    url.path = encode_path(&remove_dot_segments(&merged));
    Ok(url.href())
}

/// Joins an API base URL and an endpoint path the way the generated SDK does:
/// trailing slashes of the base and leading slashes of the path collapse into one.
pub fn join_api(base: &str, path: &str) -> String {
    if base.is_empty() {
        return path.to_string();
    }
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_url_is_resolved_against_the_server_root() {
        let base = "http://192.0.2.10:2283/api";
        assert_eq!(
            resolve(base, ".well-known/immich").as_deref(),
            Ok("http://192.0.2.10:2283/.well-known/immich")
        );
        assert_eq!(
            resolve("https://photos.example.com/api/", ".well-known/immich").as_deref(),
            Ok("https://photos.example.com/api/.well-known/immich")
        );
        assert_eq!(
            resolve("https://photos.example.com", ".well-known/immich").as_deref(),
            Ok("https://photos.example.com/.well-known/immich")
        );
    }

    #[test]
    fn endpoint_discovery_normalises_like_whatwg() {
        assert_eq!(
            resolve("http://192.0.2.10:2283/api", "/api").as_deref(),
            Ok("http://192.0.2.10:2283/api")
        );
        assert_eq!(
            resolve("HTTP://Photos.Example.com:80", "/api").as_deref(),
            Ok("http://photos.example.com/api")
        );
        assert_eq!(
            resolve("https://h.example:443/x/y", "../api/").as_deref(),
            Ok("https://h.example/api/")
        );
        assert_eq!(
            resolve("https://h.example/sub/", "api").as_deref(),
            Ok("https://h.example/sub/api")
        );
        assert_eq!(
            resolve("https://a.example/api", "https://b.example/api").as_deref(),
            Ok("https://b.example/api")
        );
        assert_eq!(
            resolve("https://a.example/api", "//b.example/api").as_deref(),
            Ok("https://b.example/api")
        );
        assert_eq!(
            resolve("https://a.example/api?x=1", "?y=2").as_deref(),
            Ok("https://a.example/api?y=2")
        );
        assert_eq!(
            resolve("http://[::1]:2283/api", "/api").as_deref(),
            Ok("http://[::1]:2283/api")
        );
    }

    #[test]
    fn invalid_bases_are_rejected() {
        assert_eq!(resolve("192.0.2.10:2283/api", ".well-known/immich"), Err(InvalidUrl));
        assert_eq!(resolve("localhost:2283", ".well-known/immich"), Err(InvalidUrl));
        assert_eq!(resolve("http://", "/api"), Err(InvalidUrl));
        assert_eq!(resolve("http://host:99999", "/api"), Err(InvalidUrl));
        assert_eq!(resolve("", "/api"), Err(InvalidUrl));
    }

    #[test]
    fn joins_api_paths_like_the_sdk() {
        assert_eq!(join_api("http://h:2283/api", "/users/me"), "http://h:2283/api/users/me");
        assert_eq!(
            join_api("http://h:2283/api/", "/users/me"),
            "http://h:2283/api/users/me"
        );
        assert_eq!(join_api("http://h:2283/api", "assets"), "http://h:2283/api/assets");
    }
}
