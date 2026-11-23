//! `--watch`: uploads new and changed files as they appear, like the original's chokidar setup:
//! events are only reported once a file's size has been stable for two seconds
//! (`awaitWriteFinish`), then batched (100 files, or 10 seconds after the last change).
//! Without `--recursive` the watcher still sees one level of subfolders (chokidar `depth: 1`).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode, CreateKind, MetadataKind, ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::api::Client;
use crate::auth::fatal_api_error;
use crate::cli::UploadOptions;
use crate::error::{Error, Result};
use crate::ignore::IgnoreMatcher;
use crate::pathutil;
use crate::signal;
use crate::upload::{unique, upload_batch};

pub const BATCH_SIZE: usize = 100;
pub const DEBOUNCE: Duration = Duration::from_secs(10);
const STABILITY_THRESHOLD: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A watched path: where it is on disk and how the user wrote it (events are reported that way).
#[derive(Clone)]
struct Root {
    absolute: String,
    display: String,
}

/// A running watcher. Dropping it stops watching.
pub struct Watch {
    _watcher: Arc<Mutex<RecommendedWatcher>>,
    uploader: JoinHandle<()>,
}

impl Watch {
    /// Blocks until the process is interrupted.
    pub fn wait(self) {
        let _ = self.uploader.join();
    }
}

/// The lowercased extension the way the original computes it: `"." + path.split(".").pop()`.
pub fn watched_extension(path: &str) -> String {
    format!(".{}", path.rsplit('.').next().unwrap_or_default().to_lowercase())
}

/// Collects paths and flushes them when `BATCH_SIZE` is reached or `DEBOUNCE` passes quietly.
pub fn run_batcher(input: Receiver<String>, output: Sender<Vec<String>>, batch_size: usize, debounce: Duration) {
    let mut items: Vec<String> = Vec::new();
    let mut deadline = Instant::now();
    loop {
        let received = if items.is_empty() {
            input.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            input.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        };
        match received {
            Ok(path) => {
                items.push(path);
                deadline = Instant::now() + debounce;
                if items.len() >= batch_size && output.send(std::mem::take(&mut items)).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if output.send(std::mem::take(&mut items)).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                if !items.is_empty() {
                    let _ = output.send(items);
                }
                return;
            }
        }
    }
}

struct EventFilter {
    roots: Vec<Root>,
    recursive: bool,
    ignore: Option<IgnoreMatcher>,
}

impl EventFilter {
    /// Maps an absolute event path to the user's spelling, or `None` when it is out of scope.
    fn display_path(&self, absolute: &str) -> Option<(String, usize)> {
        let root = self
            .roots
            .iter()
            .filter(|root| absolute == root.absolute || absolute.starts_with(&format!("{}/", root.absolute)))
            .max_by_key(|root| root.absolute.len())?;
        let relative = absolute[root.absolute.len()..].trim_start_matches('/');
        let depth = if relative.is_empty() {
            0
        } else {
            relative.split('/').count()
        };
        if !self.recursive && depth > 2 {
            return None;
        }
        let display = pathutil::join(&root.display, relative);
        if self
            .ignore
            .as_ref()
            .is_some_and(|ignore| ignore.ignores_path_or_parent(&display))
        {
            return None;
        }
        Some((display, depth))
    }
}

/// Paths in an event that may have new content.
fn changed_paths(event: Event) -> Vec<std::path::PathBuf> {
    match event.kind {
        EventKind::Create(_)
        | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any | ModifyKind::Other)
        | EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime | MetadataKind::Any))
        | EventKind::Access(AccessKind::Close(AccessMode::Write)) => event.paths,
        EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Any)) => event.paths,
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => event.paths.into_iter().skip(1).collect(),
        _ => Vec::new(),
    }
}

fn is_created_directory(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(CreateKind::Folder | CreateKind::Any)
            | EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Both | RenameMode::Any))
    )
}

/// Every file below `directory` (for folders that appear while watching, e.g. moved in).
fn files_below(directory: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = format!("{directory}/{}", entry.file_name().to_string_lossy());
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => files_below(&path, out),
            Ok(metadata) if metadata.is_file() => out.push(path),
            _ => {}
        }
    }
}

/// Waits for files to stop growing, then forwards them (chokidar's `awaitWriteFinish`).
fn run_stability_checker(pending: Arc<Mutex<HashMap<String, (u64, Instant)>>>, ready: Sender<String>) {
    loop {
        thread::sleep(POLL_INTERVAL);
        let mut finished = Vec::new();
        if let Ok(mut pending) = pending.lock() {
            let now = Instant::now();
            pending.retain(|path, (size, changed_at)| match fs::metadata(path) {
                Err(_) => false,
                Ok(metadata) if metadata.len() != *size => {
                    *size = metadata.len();
                    *changed_at = now;
                    true
                }
                Ok(_) if now.duration_since(*changed_at) >= STABILITY_THRESHOLD => {
                    finished.push(path.clone());
                    false
                }
                Ok(_) => true,
            });
        }
        finished.sort();
        for path in finished {
            if ready.send(path).is_err() {
                return;
            }
        }
    }
}

fn watch_root(watcher: &mut RecommendedWatcher, root: &Root, recursive: bool, ignore: Option<&IgnoreMatcher>) {
    let path = Path::new(&root.absolute);
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    if let Err(error) = watcher.watch(path, mode) {
        eprintln!("Watcher error: {error}");
        return;
    }
    if recursive || !path.is_dir() {
        return;
    }
    // chokidar's depth 1: also watch the immediate subfolders.
    let Ok(entries) = fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        let child = entry.path();
        let display = pathutil::join(&root.display, &entry.file_name().to_string_lossy());
        if child.is_dir() && !ignore.is_some_and(|ignore| ignore.ignores_path_or_parent(&display)) {
            let _ = watcher.watch(&child, RecursiveMode::NonRecursive);
        }
    }
}

type Pending = Arc<Mutex<HashMap<String, (u64, Instant)>>>;
type SharedWatcher = Arc<Mutex<RecommendedWatcher>>;

/// The files an event brings into scope (all files inside a folder that just appeared).
fn files_from_event(event: Event, filter: &EventFilter, watcher: &SharedWatcher) -> Vec<String> {
    let created_directory = is_created_directory(&event.kind);
    let mut files = Vec::new();
    for path in changed_paths(event) {
        let absolute = path.to_string_lossy().into_owned();
        let Some((display, depth)) = filter.display_path(&absolute) else {
            continue;
        };
        let Ok(metadata) = fs::metadata(&path) else { continue };
        if !metadata.is_dir() {
            files.push(display);
            continue;
        }
        if !created_directory {
            continue;
        }
        if !filter.recursive
            && depth == 1
            && let Ok(mut guard) = watcher.lock()
        {
            let _ = guard.watch(&path, RecursiveMode::NonRecursive);
        }
        let mut found = Vec::new();
        files_below(&absolute, &mut found);
        files.extend(
            found
                .iter()
                .filter_map(|file| filter.display_path(file))
                .map(|(display, _)| display),
        );
    }
    files
}

/// Filesystem events -> files waiting for their writes to finish.
fn run_event_loop(
    events: Receiver<notify::Result<Event>>,
    filter: EventFilter,
    watcher: SharedWatcher,
    pending: Pending,
) {
    for event in events {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                eprintln!("Watcher error: {error}");
                continue;
            }
        };
        let files = files_from_event(event, &filter, &watcher);
        if let Ok(mut pending) = pending.lock() {
            for file in files {
                let size = fs::metadata(&file).map(|metadata| metadata.len()).unwrap_or(0);
                pending.entry(file).or_insert((size, Instant::now()));
            }
        }
    }
}

/// Finished files -> extension check -> `Change detected` -> batcher.
fn run_change_reporter(ready: Receiver<String>, extensions: HashSet<String>, progress: bool, paths: Sender<String>) {
    for path in ready {
        if fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            continue;
        }
        if !extensions.contains(&watched_extension(&path)) {
            continue;
        }
        if !progress {
            println!("Change detected: {path}");
        }
        if paths.send(path).is_err() {
            return;
        }
    }
}

/// Uploads each batch; like an unhandled rejection in the original, a failure ends the process.
fn run_uploader(batches: Receiver<Vec<String>>, client: Arc<Client>, options: Arc<UploadOptions>) {
    for batch in batches {
        if let Err(error) = upload_batch(&client, unique(batch), &options) {
            if let Error::Fatal(message) = &error {
                eprintln!("{message}");
            }
            std::process::exit(1);
        }
    }
}

pub fn start(client: Arc<Client>, options: Arc<UploadOptions>) -> Result<Watch> {
    let media_types = client
        .get_supported_media_types()
        .map_err(|error| fatal_api_error(&error, "Failed to get supported media types"))?;
    let extensions: HashSet<String> = media_types
        .image
        .iter()
        .chain(&media_types.video)
        .map(|extension| extension.to_lowercase())
        .collect();
    let ignore = match &options.ignore {
        Some(pattern) => Some(IgnoreMatcher::new(pattern).map_err(Error::Fatal)?),
        None => None,
    };
    let cwd = pathutil::current_dir();
    let roots: Vec<Root> = options
        .paths
        .iter()
        .map(|path| Root {
            absolute: pathutil::resolve(&cwd, path),
            display: pathutil::normalize(path),
        })
        .collect();

    let (event_sender, event_receiver) = mpsc::channel::<notify::Result<Event>>();
    let watcher =
        notify::recommended_watcher(event_sender).map_err(|error| Error::Fatal(format!("Watcher error: {error}")))?;
    let watcher = Arc::new(Mutex::new(watcher));
    if let Ok(mut guard) = watcher.lock() {
        for root in &roots {
            watch_root(&mut guard, root, options.recursive, ignore.as_ref());
        }
    }
    signal::exit_on_interrupt(true);

    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let (ready_sender, ready_receiver) = mpsc::channel::<String>();
    let (path_sender, path_receiver) = mpsc::channel::<String>();
    let (batch_sender, batch_receiver) = mpsc::channel::<Vec<String>>();
    let filter = EventFilter {
        roots,
        recursive: options.recursive,
        ignore,
    };
    {
        let (watcher, pending) = (Arc::clone(&watcher), Arc::clone(&pending));
        thread::spawn(move || run_event_loop(event_receiver, filter, watcher, pending));
    }
    thread::spawn(move || run_stability_checker(pending, ready_sender));
    let progress = options.progress;
    thread::spawn(move || run_change_reporter(ready_receiver, extensions, progress, path_sender));
    thread::spawn(move || run_batcher(path_receiver, batch_sender, BATCH_SIZE, DEBOUNCE));
    let uploader = thread::spawn(move || run_uploader(batch_receiver, client, options));

    Ok(Watch {
        _watcher: watcher,
        uploader,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_is_taken_from_the_last_dot_of_the_path() {
        assert_eq!(watched_extension("/photos/IMG_1.JPG"), ".jpg");
        assert_eq!(watched_extension("/photos.d/README"), ".d/readme");
    }

    #[test]
    fn batches_by_size_and_by_quiet_period() {
        let (input, input_receiver) = mpsc::channel();
        let (output_sender, output) = mpsc::channel();
        let batcher = thread::spawn(move || run_batcher(input_receiver, output_sender, 3, Duration::from_millis(100)));
        for name in ["a", "b", "c", "d"] {
            input.send(name.to_string()).expect("send");
        }
        assert_eq!(
            output.recv_timeout(Duration::from_secs(2)).ok(),
            Some(vec!["a".into(), "b".into(), "c".into()])
        );
        // The fourth path is flushed once nothing arrives for the debounce period.
        assert_eq!(
            output.recv_timeout(Duration::from_secs(2)).ok(),
            Some(vec!["d".to_string()])
        );
        drop(input);
        batcher.join().expect("batcher thread");
    }

    #[test]
    fn filters_events_by_depth_and_ignore() {
        let filter = EventFilter {
            roots: vec![Root {
                absolute: "/abs/photos".to_string(),
                display: "photos".to_string(),
            }],
            recursive: false,
            ignore: IgnoreMatcher::new("**/tmp/**").ok(),
        };
        assert_eq!(
            filter.display_path("/abs/photos/a.jpg"),
            Some(("photos/a.jpg".to_string(), 1))
        );
        assert_eq!(
            filter.display_path("/abs/photos/sub/a.jpg"),
            Some(("photos/sub/a.jpg".to_string(), 2))
        );
        assert_eq!(filter.display_path("/abs/photos/sub/deeper/a.jpg"), None);
        assert_eq!(filter.display_path("/abs/photos/tmp/a.jpg"), None);
        assert_eq!(filter.display_path("/abs/other/a.jpg"), None);
    }
}
