//! Hashing local files and asking the server which ones it already has
//! (`POST /assets/bulk-upload-check`). Hashing and checking overlap: each batch of up to 5000
//! files is sent as soon as all of its files are hashed.

use std::collections::HashSet;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, MutexGuard};
use std::thread;

use indicatif::{MultiProgress, ProgressBar};

use crate::api::{BulkCheckItem, BulkCheckResult, Client};
use crate::cli::UploadOptions;
use crate::error::{Error, Result, node_io_error};
use crate::hash::sha1_file;
use crate::output::{self, Asset, byte_size, s};
use crate::queue::{self, ATTEMPTS};
use crate::signal;

/// Files are checked against the server in batches of this many checksums.
const BATCH_SIZE: usize = 5000;

type Checksum = std::result::Result<String, String>;

/// The server's verdict for one checked batch, in response order.
#[derive(Debug, Default)]
struct BatchOutcome {
    new_files: Vec<String>,
    duplicates: Vec<Asset>,
}

fn classify(requested: &[BulkCheckItem], results: Vec<BulkCheckResult>) -> BatchOutcome {
    let requested: HashSet<&str> = requested.iter().map(|item| item.id.as_str()).collect();
    let mut outcome = BatchOutcome::default();
    for result in results {
        if !requested.contains(result.id.as_str()) {
            eprintln!("Ignoring unexpected file in bulk upload check response: {}", result.id);
            continue;
        }
        if result.action == "accept" {
            outcome.new_files.push(result.id);
        } else {
            outcome.duplicates.push(Asset {
                id: result.asset_id,
                filepath: result.id,
            });
        }
    }
    outcome
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// State shared by the hashing and checking threads.
struct Pipeline<'a> {
    client: &'a Client,
    files: &'a [String],
    sizes: &'a [u64],
    /// The hashing and duplicate-check progress bars, when shown.
    bars: Option<(ProgressBar, ProgressBar)>,
    next_file: AtomicUsize,
    checksums: Vec<Mutex<Option<Checksum>>>,
    unhashed_per_batch: Vec<Mutex<usize>>,
    outcomes: Vec<Mutex<Option<BatchOutcome>>>,
}

impl<'a> Pipeline<'a> {
    fn new(
        client: &'a Client,
        files: &'a [String],
        sizes: &'a [u64],
        bars: Option<(ProgressBar, ProgressBar)>,
    ) -> Self {
        let batch_count = files.len().div_ceil(BATCH_SIZE);
        let pipeline = Pipeline {
            client,
            files,
            sizes,
            bars,
            next_file: AtomicUsize::new(0),
            checksums: files.iter().map(|_| Mutex::new(None)).collect(),
            unhashed_per_batch: Vec::new(),
            outcomes: (0..batch_count).map(|_| Mutex::new(None)).collect(),
        };
        let unhashed_per_batch = (0..batch_count)
            .map(|batch| Mutex::new(pipeline.batch_range(batch).len()))
            .collect();
        Pipeline {
            unhashed_per_batch,
            ..pipeline
        }
    }

    fn batch_range(&self, batch: usize) -> Range<usize> {
        batch * BATCH_SIZE..self.files.len().min((batch + 1) * BATCH_SIZE)
    }

    /// Hashes files until none are left, announcing each batch whose files are all hashed.
    fn hash_files(&self, completed_batches: Sender<usize>) {
        loop {
            let index = self.next_file.fetch_add(1, Ordering::SeqCst);
            let Some(path) = self.files.get(index) else { break };
            let checksum = queue::run_with_retries(path, ATTEMPTS, &|path: &String| {
                sha1_file(Path::new(path)).map_err(|error| node_io_error(&error, "open", path))
            });
            if checksum.is_ok()
                && let Some((hash_bar, _)) = &self.bars
            {
                hash_bar.inc(self.sizes[index]);
            }
            *lock(&self.checksums[index]) = Some(checksum);

            let batch = index / BATCH_SIZE;
            let mut unhashed = lock(&self.unhashed_per_batch[batch]);
            *unhashed -= 1;
            if *unhashed == 0 {
                let _ = completed_batches.send(batch);
            }
        }
    }

    /// Sends completed batches to the server until the hashers are done.
    fn check_batches(&self, completed_batches: &Mutex<Receiver<usize>>) {
        loop {
            let next = lock(completed_batches).recv();
            match next {
                Ok(batch) => self.check_batch(batch),
                Err(_) => break,
            }
        }
    }

    fn check_batch(&self, batch: usize) {
        let hashed: Vec<(usize, BulkCheckItem)> = self
            .batch_range(batch)
            .filter_map(|index| {
                let checksum = lock(&self.checksums[index]).as_ref()?.as_ref().ok()?.clone();
                Some((
                    index,
                    BulkCheckItem {
                        id: self.files[index].clone(),
                        checksum,
                    },
                ))
            })
            .collect();
        if hashed.is_empty() {
            return;
        }
        let items: Vec<BulkCheckItem> = hashed.iter().map(|(_, item)| item.clone()).collect();
        let response = queue::run_with_retries(&items, ATTEMPTS, &|items: &Vec<BulkCheckItem>| {
            self.client.check_bulk_upload(items).map_err(|error| error.to_string())
        });
        // Like the original, a batch that fails every attempt is silently left out.
        let Ok(results) = response else { return };
        *lock(&self.outcomes[batch]) = Some(classify(&items, results));
        if let Some((_, check_bar)) = &self.bars {
            check_bar.inc(hashed.iter().map(|(index, _)| self.sizes[*index]).sum());
        }
    }

    /// Runs the hashing and checking threads to completion.
    fn run(&self, concurrency: usize) {
        let (sender, receiver) = mpsc::channel::<usize>();
        let receiver = Mutex::new(receiver);
        thread::scope(|scope| {
            for _ in 0..concurrency.min(self.outcomes.len()) {
                scope.spawn(|| self.check_batches(&receiver));
            }
            for _ in 0..concurrency.min(self.files.len()) {
                let sender = sender.clone();
                scope.spawn(move || self.hash_files(sender));
            }
            drop(sender);
        });
    }

    /// New files and duplicates in batch order, plus the files that could not be hashed.
    fn into_results(self) -> (Vec<String>, Vec<Asset>, Vec<(String, String)>) {
        let mut new_files = Vec::new();
        let mut duplicates = Vec::new();
        for outcome in self
            .outcomes
            .into_iter()
            .filter_map(|slot| slot.into_inner().ok().flatten())
        {
            new_files.extend(outcome.new_files);
            duplicates.extend(outcome.duplicates);
        }
        let failures = self
            .files
            .iter()
            .zip(self.checksums)
            .filter_map(|(path, slot)| match slot.into_inner().ok().flatten() {
                Some(Err(error)) => Some((path.clone(), error)),
                _ => None,
            })
            .collect();
        (new_files, duplicates, failures)
    }
}

/// Splits `files` into new files and duplicates, printing the original's summary lines.
pub fn check_for_duplicates(
    client: &Client,
    files: Vec<String>,
    options: &UploadOptions,
) -> Result<(Vec<String>, Vec<Asset>)> {
    if options.skip_hash {
        println!("Skipping hash check, assuming all files are new");
        return Ok((files, Vec::new()));
    }

    let sizes: Vec<u64> = files.iter().map(|path| fs_size(path)).collect::<Result<_>>()?;
    let total_size: u64 = sizes.iter().sum();
    let multi = MultiProgress::new();
    let bars = if options.progress {
        signal::exit_on_interrupt(false);
        let hash_bar = multi.add(output::bytes_bar("Hashing files          ", total_size));
        let check_bar = multi.add(output::bytes_bar("Checking for duplicates", total_size));
        Some((hash_bar, check_bar))
    } else {
        println!("Received {} files ({}), hashing...", files.len(), byte_size(total_size));
        None
    };

    let pipeline = Pipeline::new(client, &files, &sizes, bars);
    pipeline.run(options.concurrency.max(1));
    if let Some((hash_bar, check_bar)) = &pipeline.bars {
        hash_bar.abandon();
        check_bar.abandon();
    }
    let (new_files, duplicates, failures) = pipeline.into_results();

    println!(
        "Found {} new files and {} duplicate{}",
        new_files.len(),
        duplicates.len(),
        s(duplicates.len())
    );
    output::print_failures("Failed to verify", "file", &failures);
    Ok((new_files, duplicates))
}

fn fs_size(path: &str) -> Result<u64> {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| Error::Fatal(node_io_error(&error, "stat", path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_bulk_check_results() {
        let requested = vec![
            BulkCheckItem {
                id: "/a.jpg".to_string(),
                checksum: "1".to_string(),
            },
            BulkCheckItem {
                id: "/b.jpg".to_string(),
                checksum: "2".to_string(),
            },
        ];
        let results = vec![
            BulkCheckResult {
                id: "/a.jpg".to_string(),
                action: "accept".to_string(),
                asset_id: None,
            },
            BulkCheckResult {
                id: "/b.jpg".to_string(),
                action: "reject".to_string(),
                asset_id: Some("x".to_string()),
            },
            BulkCheckResult {
                id: "/other.jpg".to_string(),
                action: "accept".to_string(),
                asset_id: None,
            },
        ];
        let outcome = classify(&requested, results);
        assert_eq!(outcome.new_files, vec!["/a.jpg"]);
        assert_eq!(
            outcome.duplicates,
            vec![Asset {
                id: Some("x".to_string()),
                filepath: "/b.jpg".to_string()
            }]
        );
    }

    #[test]
    fn batches_cover_every_file_once() {
        let client = Client::new("http://127.0.0.1:9/api", "unused");
        let files: Vec<String> = (0..12_001).map(|index| format!("/f{index}.jpg")).collect();
        let sizes = vec![0; files.len()];
        let pipeline = Pipeline::new(&client, &files, &sizes, None);
        assert_eq!(pipeline.outcomes.len(), 3);
        assert_eq!(pipeline.batch_range(0), 0..5000);
        assert_eq!(pipeline.batch_range(2), 10_000..12_001);
        assert_eq!(*lock(&pipeline.unhashed_per_batch[2]), 2001);
    }
}
