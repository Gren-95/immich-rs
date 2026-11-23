//! Finding the files to upload. Mirrors the original `crawl()`, which ran fast-glob with
//! `<dir>[/**]/*.{extensions}`, `caseSensitiveMatch: false`, `dot: includeHidden` and
//! `ignore: ['**/<pattern>']`:
//!
//! * a path that is a file is taken as-is (no extension, hidden or ignore filtering);
//! * a directory is searched for files whose extension the server supports (case-insensitive),
//!   recursing only with `--recursive`;
//! * names starting with `.` (files and folders) are skipped unless `--include-hidden`;
//! * `--ignore` is matched against absolute paths as `**/<pattern>`; like fast-glob, ignore
//!   patterns that end in `/**` or whose last segment has no wildcards also prune directories;
//! * a path that does not exist is treated as a glob pattern;
//! * the result is sorted like JavaScript's default sort.

use std::collections::HashSet;
use std::fs;
use std::io;

use globset::GlobSetBuilder;

use crate::error::{Error, Result, node_io_error};
use crate::ignore::{IgnoreMatcher, build_glob, collapse_slashes, escape_unexpanded_braces, expand_braces, is_dynamic};
use crate::pathutil;

pub struct CrawlOptions<'a> {
    pub paths: &'a [String],
    pub recursive: bool,
    pub ignore: Option<&'a str>,
    pub include_hidden: bool,
    /// Supported extensions as the server reports them (`.jpg`, ...).
    pub extensions: &'a [String],
}

/// Case-insensitive `*.{ext,...}` matching on a file name.
pub struct ExtensionFilter {
    suffixes: Vec<String>,
}

impl ExtensionFilter {
    pub fn new(extensions: &[String]) -> Self {
        let suffixes = extensions
            .iter()
            .map(|extension| format!(".{}", extension.replacen('.', "", 1).to_ascii_lowercase()))
            .collect();
        ExtensionFilter { suffixes }
    }

    pub fn matches(&self, file_name: &str) -> bool {
        let lowered = file_name.to_ascii_lowercase();
        self.suffixes.iter().any(|suffix| lowered.ends_with(suffix.as_str()))
    }
}

/// Whether a name is hidden (and so skipped unless `--include-hidden`).
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

struct Walker<'a> {
    recursive: bool,
    include_hidden: bool,
    extensions: &'a ExtensionFilter,
    ignore: Option<&'a IgnoreMatcher>,
    found: Vec<String>,
    seen: HashSet<String>,
}

impl Walker<'_> {
    /// Records a file unless `--ignore` matches `matched_path` (fast-glob matches the path as
    /// the pattern was written: absolute for directories, relative for relative globs).
    fn accept(&mut self, path: String, matched_path: &str) {
        if self.ignore.is_some_and(|ignore| ignore.ignores_file(matched_path)) {
            return;
        }
        if self.seen.insert(path.clone()) {
            self.found.push(path);
        }
    }

    /// Lists a directory, following symlinks; unreadable entries (e.g. broken links) are skipped.
    fn read_directory(&self, directory: &str) -> Result<Vec<(String, fs::Metadata)>> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Error::Fatal(node_io_error(&error, "scandir", directory))),
        };
        let mut listed = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| Error::Fatal(node_io_error(&error, "scandir", directory)))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !self.include_hidden && is_hidden(&name) {
                continue;
            }
            let path = if directory == "/" {
                format!("/{name}")
            } else {
                format!("{directory}/{name}")
            };
            if let Ok(metadata) = fs::metadata(&path) {
                listed.push((path, metadata));
            }
        }
        Ok(listed)
    }

    /// Walks a directory given on the command line.
    fn walk(&mut self, directory: &str, ancestors: &mut Vec<(u64, u64)>) -> Result<()> {
        for (path, metadata) in self.read_directory(directory)? {
            if metadata.is_file() {
                if self.extensions.matches(&pathutil::basename(&path)) {
                    let matched_path = path.clone();
                    self.accept(path, &matched_path);
                }
            } else if metadata.is_dir() && self.recursive {
                if self.ignore.is_some_and(|ignore| ignore.prunes_directory(&path)) {
                    continue;
                }
                let identity = file_identity(&metadata);
                // Symlink loops would otherwise recurse forever.
                if identity.is_some_and(|id| ancestors.contains(&id)) {
                    continue;
                }
                if let Some(id) = identity {
                    ancestors.push(id);
                }
                self.walk(&path, ancestors)?;
                if identity.is_some() {
                    ancestors.pop();
                }
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// Handles a path argument that does not exist by treating it as a glob for directories,
/// as fast-glob does (e.g. `'photos/2023-*'`).
fn crawl_glob(walker: &mut Walker, pattern: &str, cwd: &str) -> Result<()> {
    let segments: Vec<&str> = pattern.split('/').collect();
    let static_count = segments.iter().take_while(|segment| !is_dynamic(segment)).count();
    if static_count == segments.len() {
        return Ok(());
    }
    let base = segments[..static_count].join("/");
    let base = match (base.is_empty(), pattern.starts_with('/')) {
        (true, true) => "/".to_string(),
        (true, false) => ".".to_string(),
        (false, _) => base,
    };
    let mut patterns = GlobSetBuilder::new();
    for alternative in expand_braces(&collapse_slashes(pattern)) {
        let alternative = escape_unexpanded_braces(alternative.trim_end_matches('/'));
        patterns.add(build_glob(&alternative).map_err(Error::Fatal)?);
        if walker.recursive {
            patterns.add(build_glob(&format!("{alternative}/**")).map_err(Error::Fatal)?);
        }
    }
    let directories = patterns
        .build()
        .map_err(|error| Error::Fatal(format!("Error: invalid pattern: {error}")))?;

    // Folders deeper than the pattern can reach are not visited.
    let max_depth = if walker.recursive || pattern.contains("**") {
        usize::MAX
    } else {
        segments.len() - static_count
    };

    // Walk below the static base, then keep files whose parent directory matches.
    let mut pending = vec![(base, 0usize)];
    let mut visited = HashSet::new();
    while let Some((directory, depth)) = pending.pop() {
        for (path, metadata) in walker.read_directory(&directory)? {
            let display = path.strip_prefix("./").unwrap_or(&path).to_string();
            if metadata.is_dir() {
                let unvisited = file_identity(&metadata).is_none_or(|id| visited.insert(id));
                let pruned = walker.ignore.is_some_and(|ignore| ignore.prunes_directory(&display));
                if unvisited && !pruned && depth < max_depth {
                    pending.push((path, depth + 1));
                }
            } else if metadata.is_file()
                && walker.extensions.matches(&pathutil::basename(&display))
                && directories.is_match(pathutil::dirname(&display))
            {
                walker.accept(pathutil::resolve(cwd, &display), &display);
            }
        }
    }
    Ok(())
}

pub fn crawl(options: &CrawlOptions) -> Result<Vec<String>> {
    if options.paths.is_empty() {
        return Ok(Vec::new());
    }
    let cwd = pathutil::current_dir();
    let mut files = Vec::new();
    let mut directories = Vec::new();
    let mut globs = Vec::new();
    for path in options.paths {
        let absolute = pathutil::resolve(&cwd, path);
        match fs::metadata(&absolute) {
            Ok(metadata) if metadata.is_file() => files.push(absolute),
            Ok(_) => directories.push(absolute),
            Err(error) if error.kind() == io::ErrorKind::NotFound => globs.push(path.clone()),
            Err(error) => return Err(Error::Fatal(node_io_error(&error, "stat", &absolute))),
        }
    }
    if directories.is_empty() && globs.is_empty() {
        return Ok(files);
    }

    let ignore = match options.ignore {
        Some(pattern) => Some(IgnoreMatcher::new(pattern).map_err(Error::Fatal)?),
        None => None,
    };
    let extensions = ExtensionFilter::new(options.extensions);
    let mut walker = Walker {
        recursive: options.recursive,
        include_hidden: options.include_hidden,
        extensions: &extensions,
        ignore: ignore.as_ref(),
        found: Vec::new(),
        seen: HashSet::new(),
    };
    for directory in &directories {
        let mut ancestors = fs::metadata(directory)
            .ok()
            .and_then(|m| file_identity(&m))
            .into_iter()
            .collect();
        walker.walk(directory, &mut ancestors)?;
    }
    for pattern in &globs {
        crawl_glob(&mut walker, pattern, &cwd)?;
    }

    let mut found = walker.found;
    found.extend(files);
    pathutil::js_sort(&mut found);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree {
        root: String,
    }

    impl Tree {
        fn new(name: &str, files: &[&str]) -> Self {
            let root = std::env::temp_dir().join(format!("immich-rs-crawl-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            for file in files {
                let path = root.join(file);
                fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
                fs::write(&path, b"x").expect("write file");
            }
            Tree {
                root: root.to_string_lossy().into_owned(),
            }
        }

        fn crawl(&self, recursive: bool, ignore: Option<&str>, include_hidden: bool) -> Vec<String> {
            let extensions: Vec<String> = [".jpg", ".jpeg", ".mp4", ".heic"]
                .iter()
                .map(|e| e.to_string())
                .collect();
            let paths = vec![self.root.clone()];
            let options = CrawlOptions {
                paths: &paths,
                recursive,
                ignore,
                include_hidden,
                extensions: &extensions,
            };
            let found = crawl(&options).expect("crawl");
            found.iter().map(|path| path.replacen(&self.root, "", 1)).collect()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const FILES: [&str; 12] = [
        "a.jpg",
        "B.JPG",
        "c.txt",
        ".hidden.jpg",
        "sub/d.mp4",
        "sub/deep/e.heic",
        ".secret/f.jpg",
        "Screenshots/g.jpg",
        "x/screenshots/h.jpg",
        "ocr/i.jpg",
        "Trip [2020]/j.jpg",
        "sub/k.png",
    ];

    #[test]
    fn filters_by_extension_case_insensitively() {
        let filter = ExtensionFilter::new(&[".jpg".to_string(), ".mp4".to_string()]);
        assert!(filter.matches("IMG_1.JPG"));
        assert!(filter.matches("clip.mp4"));
        assert!(!filter.matches("notes.txt"));
        assert!(!filter.matches("jpg"));
    }

    #[test]
    fn non_recursive_lists_only_top_level_visible_files() {
        let tree = Tree::new("flat", &FILES);
        assert_eq!(tree.crawl(false, None, false), vec!["/B.JPG", "/a.jpg"]);
    }

    #[test]
    fn recursive_skips_hidden_unless_requested() {
        let tree = Tree::new("recursive", &FILES);
        let found = tree.crawl(true, None, false);
        assert_eq!(
            found,
            vec![
                "/B.JPG",
                "/Screenshots/g.jpg",
                "/Trip [2020]/j.jpg",
                "/a.jpg",
                "/ocr/i.jpg",
                "/sub/d.mp4",
                "/sub/deep/e.heic",
                "/x/screenshots/h.jpg"
            ]
        );
        let with_hidden = tree.crawl(true, None, true);
        assert!(with_hidden.contains(&"/.hidden.jpg".to_string()));
        assert!(with_hidden.contains(&"/.secret/f.jpg".to_string()));
    }

    #[test]
    fn ignore_patterns_follow_fast_glob_semantics() {
        let tree = Tree::new("ignore", &FILES);
        let found = tree.crawl(true, Some("**/{ocr,Screenshots}/**"), false);
        assert!(
            !found
                .iter()
                .any(|path| path.to_lowercase().contains("screenshots") || path.contains("ocr"))
        );
        assert_eq!(found.len(), 5);
        // A static last segment prunes matching directories (case-insensitively).
        let found = tree.crawl(true, Some("Screenshots"), false);
        assert!(!found.iter().any(|path| path.to_lowercase().contains("screenshots")));
        assert!(found.contains(&"/ocr/i.jpg".to_string()));
        // A wildcard pattern only filters files.
        let found = tree.crawl(true, Some("*.mp4"), false);
        assert!(!found.contains(&"/sub/d.mp4".to_string()));
        assert!(found.contains(&"/sub/deep/e.heic".to_string()));
    }

    #[test]
    fn file_arguments_bypass_filters() {
        let tree = Tree::new("files", &FILES);
        let paths = vec![format!("{}/c.txt", tree.root), format!("{}/.hidden.jpg", tree.root)];
        let extensions = vec![".jpg".to_string()];
        let options = CrawlOptions {
            paths: &paths,
            recursive: false,
            ignore: None,
            include_hidden: false,
            extensions: &extensions,
        };
        assert_eq!(crawl(&options).expect("crawl"), paths);
    }
}
