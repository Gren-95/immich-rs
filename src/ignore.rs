//! The `--ignore` pattern with fast-glob semantics: matched case-insensitively against absolute
//! paths as `**/<pattern>`, with braces expanded first. Like fast-glob, alternatives that end in
//! `/**` or whose last segment has no wildcards also prune directories while crawling.

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

use crate::pathutil;

/// Expands `{a,b}` alternatives (nested groups included) like micromatch's brace expansion.
/// Groups without a top-level comma are left as literal text.
pub fn expand_braces(pattern: &str) -> Vec<String> {
    let bytes = pattern.as_bytes();
    let mut depth = 0usize;
    let mut open = None;
    let mut commas = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            b'{' => {
                if depth == 0 {
                    open = Some(index);
                    commas.clear();
                }
                depth += 1;
            }
            b',' if depth == 1 => commas.push(index),
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0
                    && let Some(start) = open
                {
                    if commas.is_empty() {
                        // `{x}` is not an expansion: keep it and expand what follows.
                        let prefix = &pattern[..=index];
                        return expand_braces(&pattern[index + 1..])
                            .into_iter()
                            .map(|rest| format!("{prefix}{rest}"))
                            .collect();
                    }
                    let prefix = &pattern[..start];
                    let suffixes = expand_braces(&pattern[index + 1..]);
                    let mut bounds = vec![start];
                    bounds.extend(&commas);
                    bounds.push(index);
                    let mut expanded = Vec::new();
                    for window in bounds.windows(2) {
                        for alternative in expand_braces(&pattern[window[0] + 1..window[1]]) {
                            for suffix in &suffixes {
                                expanded.push(format!("{prefix}{alternative}{suffix}"));
                            }
                        }
                    }
                    return expanded;
                }
            }
            _ => {}
        }
        index += 1;
    }
    vec![pattern.to_string()]
}

/// fast-glob's `isDynamicPattern` for a single path segment.
pub fn is_dynamic(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    if segment.contains(['*', '?', '\\']) || segment.starts_with('!') {
        return true;
    }
    let has_class = segment.find('[').is_some_and(|open| segment[open + 1..].contains(']'));
    let has_group = segment.find('(').is_some_and(|open| {
        let rest = &segment[open + 1..];
        rest.find(')').is_some_and(|close| rest[..close].contains('|'))
    });
    let has_extglob = ["!(", "*(", "+(", "?(", "@("]
        .iter()
        .any(|marker| segment.contains(marker));
    let has_braces = segment.find('{').is_some_and(|open| {
        segment[open + 1..].find('}').is_some_and(|close| {
            let inner = &segment[open + 1..open + 1 + close];
            inner.contains(',') || inner.contains("..")
        })
    });
    has_class || has_group || has_extglob || has_braces
}

/// Escapes characters globset would treat as syntax but picomatch takes literally.
pub fn escape_unexpanded_braces(pattern: &str) -> String {
    pattern.replace('{', "\\{").replace('}', "\\}")
}

pub fn build_glob(pattern: &str) -> Result<Glob, String> {
    GlobBuilder::new(pattern)
        .case_insensitive(true)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .map_err(|error| format!("Error: invalid ignore pattern: {error}"))
}

/// The `--ignore` pattern, compiled as fast-glob uses it.
pub struct IgnoreMatcher {
    /// Excludes matching files (`**/<pattern>`).
    entries: GlobSet,
    /// Prunes matching directories while walking.
    directories: GlobSet,
}

impl IgnoreMatcher {
    pub fn new(pattern: &str) -> Result<Self, String> {
        let mut entries = GlobSetBuilder::new();
        let mut directories = GlobSetBuilder::new();
        for alternative in expand_braces(&format!("**/{pattern}")) {
            let collapsed = collapse_slashes(&alternative);
            let glob_text = escape_unexpanded_braces(&collapsed);
            entries.add(build_glob(&glob_text)?);
            let last_segment = pathutil::basename(&collapsed);
            if collapsed.ends_with("/**") || !is_dynamic(&last_segment) {
                directories.add(build_glob(&glob_text)?);
                if let Some(parent) = glob_text.strip_suffix("/**")
                    && !parent.is_empty()
                {
                    // picomatch lets `dir/**` match `dir` itself.
                    directories.add(build_glob(parent)?);
                }
            }
        }
        let build = |builder: GlobSetBuilder| {
            builder
                .build()
                .map_err(|error| format!("Error: invalid ignore pattern: {error}"))
        };
        Ok(IgnoreMatcher {
            entries: build(entries)?,
            directories: build(directories)?,
        })
    }

    pub fn ignores_file(&self, path: &str) -> bool {
        self.entries.is_match(path)
    }

    pub fn prunes_directory(&self, path: &str) -> bool {
        self.directories.is_match(path)
    }

    /// Whether the path or any of its parent directories is ignored (used by `--watch`).
    pub fn ignores_path_or_parent(&self, path: &str) -> bool {
        if self.entries.is_match(path) || self.directories.is_match(path) {
            return true;
        }
        let mut current = path;
        while let Some(index) = current.rfind('/') {
            current = &current[..index];
            if current.is_empty() {
                break;
            }
            if self.entries.is_match(current) || self.directories.is_match(current) {
                return true;
            }
        }
        false
    }
}

/// Removes repeated slashes except at the very start, like fast-glob's `removeDuplicateSlashes`.
pub fn collapse_slashes(pattern: &str) -> String {
    let mut output = String::with_capacity(pattern.len());
    let mut previous_slash = false;
    for (index, c) in pattern.chars().enumerate() {
        if c == '/' && previous_slash && index > 1 {
            continue;
        }
        previous_slash = c == '/';
        output.push(c);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_braces() {
        assert_eq!(
            expand_braces("**/{ocr,Screenshots}/**"),
            vec!["**/ocr/**", "**/Screenshots/**"]
        );
        assert_eq!(expand_braces("a{b,c{d,e}}f"), vec!["abf", "acdf", "acef"]);
        assert_eq!(expand_braces("x{y}z{1,2}"), vec!["x{y}z1", "x{y}z2"]);
    }

    #[test]
    fn detects_dynamic_segments() {
        assert!(!is_dynamic("Screenshots"));
        assert!(is_dynamic("*.jpg"));
        assert!(is_dynamic("[ab]"));
        assert!(!is_dynamic("x{y}"));
        assert!(is_dynamic("{a,b}"));
    }

    #[test]
    fn matches_case_insensitively_and_prunes_like_fast_glob() {
        let matcher = IgnoreMatcher::new("**/{ocr,Screenshots}/**").expect("valid pattern");
        assert!(matcher.ignores_file("/home/u/Pictures/screenshots/a.jpg"));
        assert!(matcher.prunes_directory("/home/u/Pictures/Screenshots"));
        assert!(!matcher.ignores_file("/home/u/Pictures/ocr.jpg"));
        let static_name = IgnoreMatcher::new("Screenshots").expect("valid pattern");
        assert!(static_name.prunes_directory("/p/Screenshots"));
        assert!(!static_name.ignores_file("/p/Screenshots/a.jpg"));
        let wildcard = IgnoreMatcher::new("*.png").expect("valid pattern");
        assert!(wildcard.ignores_file("/p/x/A.PNG"));
        assert!(!wildcard.prunes_directory("/p/x.png"));
        assert!(wildcard.ignores_path_or_parent("/p/x.png"));
    }
}
