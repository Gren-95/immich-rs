//! POSIX path helpers with the semantics of Node's `path` module, so that printed paths,
//! album names and sidecar lookups match the original CLI exactly.

/// Resolves `.` and `..` segments lexically. `allow_above_root` keeps leading `..` segments.
fn normalize_segments(path: &str, allow_above_root: bool) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| *last != "..") {
                    segments.pop();
                } else if allow_above_root {
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

/// Node's `path.normalize`.
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let is_absolute = path.starts_with('/');
    let trailing_separator = path.ends_with('/');
    let mut result = normalize_segments(path, !is_absolute);
    if result.is_empty() {
        if is_absolute {
            return "/".to_string();
        }
        return if trailing_separator { "./" } else { "." }.to_string();
    }
    if trailing_separator {
        result.push('/');
    }
    if is_absolute { format!("/{result}") } else { result }
}

/// Node's `path.join` for two segments.
pub fn join(first: &str, second: &str) -> String {
    let joined = match (first.is_empty(), second.is_empty()) {
        (true, true) => return ".".to_string(),
        (true, false) => second.to_string(),
        (false, true) => first.to_string(),
        (false, false) => format!("{first}/{second}"),
    };
    normalize(&joined)
}

/// Node's `path.resolve(path)` relative to `cwd`.
pub fn resolve(cwd: &str, path: &str) -> String {
    let combined = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{cwd}/{path}")
    };
    format!("/{}", normalize_segments(&combined, false))
}

/// Node's `path.basename`.
pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(index) => trimmed[index + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

/// Node's `path.dirname`.
pub fn dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let has_root = path.starts_with('/');
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        None => ".".to_string(),
        Some(index) => {
            let parent = trimmed[..index].trim_end_matches('/');
            if parent.is_empty() {
                if has_root { "/".to_string() } else { ".".to_string() }
            } else {
                parent.to_string()
            }
        }
    }
}

/// The file name without its extension, like `path.parse(p).name`.
pub fn stem(path: &str) -> String {
    let base = basename(path);
    match base.rfind('.') {
        Some(index) if index > 0 => base[..index].to_string(),
        _ => base,
    }
}

/// The current working directory as a string.
pub fn current_dir() -> String {
    std::env::current_dir()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string())
}

/// Sorts strings like JavaScript's default `Array.prototype.sort` (UTF-16 code unit order).
pub fn js_sort(values: &mut [String]) {
    values.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_matches_node() {
        assert_eq!(
            join("/home/u/.config/immich/", "auth.yml"),
            "/home/u/.config/immich/auth.yml"
        );
        assert_eq!(join("conf//dir/../x", "auth.yml"), "conf/x/auth.yml");
        assert_eq!(join("", "auth.yml"), "auth.yml");
        assert_eq!(join("/a/b", ".."), "/a");
    }

    #[test]
    fn resolve_matches_node() {
        assert_eq!(resolve("/home/u", "Pictures/"), "/home/u/Pictures");
        assert_eq!(resolve("/home/u", "../x/./y"), "/home/x/y");
        assert_eq!(resolve("/home/u", "/abs//p/"), "/abs/p");
        assert_eq!(resolve("/", ".."), "/");
    }

    #[test]
    fn dirname_and_basename_match_node() {
        assert_eq!(dirname("/a/b/c.jpg"), "/a/b");
        assert_eq!(dirname("/c.jpg"), "/");
        assert_eq!(dirname("c.jpg"), ".");
        assert_eq!(dirname("a/b/"), "a");
        assert_eq!(basename("/a/b/"), "b");
        assert_eq!(basename("/"), "");
    }

    #[test]
    fn stem_strips_last_extension_only() {
        assert_eq!(stem("/a/IMG_1.tar.jpg"), "IMG_1.tar");
        assert_eq!(stem("/a/.hidden"), ".hidden");
        assert_eq!(stem("/a/.hidden.jpg"), ".hidden");
    }

    #[test]
    fn js_sort_uses_utf16_order() {
        let mut values = vec![
            "b".to_string(),
            "\u{1F600}".to_string(),
            "\u{FF5E}".to_string(),
            "a".to_string(),
        ];
        js_sort(&mut values);
        assert_eq!(values, vec!["a", "b", "\u{1F600}", "\u{FF5E}"]);
    }
}
