//! The `upload` command: crawl, hash, check for duplicates, upload, then albums and deletion.

use std::collections::HashSet;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::albums::update_albums;
use crate::api::Client;
use crate::auth::{authenticate, fatal_api_error, require_permissions};
use crate::cli::{GlobalOptions, UploadOptions};
use crate::crawl::{CrawlOptions, crawl};
use crate::duplicates::check_for_duplicates;
use crate::error::{Error, Result, node_io_error};
use crate::output::{self, Asset, byte_size, s};
use crate::queue::{self, ATTEMPTS};
use crate::watch;

pub fn upload(global: &GlobalOptions, options: UploadOptions) -> Result<()> {
    let connection = authenticate(global)?;
    let client = Arc::new(connection.client);
    require_permissions(&client, &["asset.upload"])?;

    let files = scan(&client, &options)?;
    if files.is_empty() {
        if options.watch {
            println!("No files found initially.");
        } else {
            println!("No files found, exiting");
            return Ok(());
        }
    }

    let options = Arc::new(options);
    let watcher = if options.watch {
        println!("Watching for changes...");
        Some(watch::start(Arc::clone(&client), Arc::clone(&options))?)
    } else {
        None
    };
    upload_batch(&client, files, &options)?;
    if let Some(watcher) = watcher {
        watcher.wait();
    }
    Ok(())
}

fn scan(client: &Client, options: &UploadOptions) -> Result<Vec<String>> {
    let media_types = client
        .get_supported_media_types()
        .map_err(|error| fatal_api_error(&error, "Failed to get supported media types"))?;
    println!("Crawling for assets...");
    let extensions: Vec<String> = media_types.image.iter().chain(&media_types.video).cloned().collect();
    crawl(&CrawlOptions {
        paths: &options.paths,
        recursive: options.recursive,
        ignore: options.ignore.as_deref(),
        include_hidden: options.include_hidden,
        extensions: &extensions,
    })
}

/// Hash, check, upload, report, then update albums and delete local files for one set of files.
pub fn upload_batch(client: &Client, files: Vec<String>, options: &UploadOptions) -> Result<()> {
    let (new_files, duplicates) = check_for_duplicates(client, files, options)?;
    let new_assets = upload_files(client, &new_files, options)?;
    if options.json_output {
        println!("{}", output::json_output(&new_files, &duplicates, &new_assets));
    }
    let mut album_assets = new_assets.clone();
    album_assets.extend(duplicates.iter().cloned());
    update_albums(client, &album_assets, options)?;
    delete_files(&new_assets, &duplicates, options)
}

/// Running totals of an upload. Like the original, the progress bar fills with newly created
/// bytes while its label counts created plus duplicate bytes (`shown_bytes`).
#[derive(Default)]
struct UploadTally {
    created: AtomicU64,
    created_bytes: AtomicU64,
    duplicates: AtomicU64,
    duplicate_bytes: AtomicU64,
    shown_bytes: Arc<AtomicU64>,
}

impl UploadTally {
    fn record(&self, duplicate: bool, size: u64) {
        let (count, bytes) = if duplicate {
            (&self.duplicates, &self.duplicate_bytes)
        } else {
            (&self.created, &self.created_bytes)
        };
        count.fetch_add(1, Ordering::SeqCst);
        bytes.fetch_add(size, Ordering::SeqCst);
        self.shown_bytes.fetch_add(size, Ordering::SeqCst);
    }

    fn print_summary(&self) {
        let created = self.created.load(Ordering::SeqCst) as usize;
        let created_bytes = byte_size(self.created_bytes.load(Ordering::SeqCst));
        println!(
            "Successfully uploaded {created} new asset{} ({created_bytes})",
            s(created)
        );
        let duplicates = self.duplicates.load(Ordering::SeqCst) as usize;
        if duplicates > 0 {
            let duplicate_bytes = byte_size(self.duplicate_bytes.load(Ordering::SeqCst));
            println!(
                "Skipped {duplicates} duplicate asset{} ({duplicate_bytes})",
                s(duplicates)
            );
        }
    }
}

fn upload_files(client: &Client, files: &[String], options: &UploadOptions) -> Result<Vec<Asset>> {
    if files.is_empty() {
        println!("All assets were already uploaded, nothing to do.");
        return Ok(Vec::new());
    }
    let metadata: Vec<fs::Metadata> = files
        .iter()
        .map(|path| fs::metadata(path).map_err(|error| Error::Fatal(node_io_error(&error, "stat", path))))
        .collect::<Result<_>>()?;
    let total_size: u64 = metadata.iter().map(fs::Metadata::len).sum();
    let summary = format!("{} asset{} ({})", files.len(), s(files.len()), byte_size(total_size));

    if options.dry_run {
        println!("Would have uploaded {summary}");
        return Ok(files
            .iter()
            .map(|path| Asset {
                id: Some(String::new()),
                filepath: path.clone(),
            })
            .collect());
    }

    let tally = UploadTally::default();
    let bar = options
        .progress
        .then(|| output::upload_bar(total_size, Arc::clone(&tally.shown_bytes)));
    if bar.is_none() {
        println!("Uploading {summary}");
    }
    let indices: Vec<usize> = (0..files.len()).collect();
    let results = queue::run(&indices, options.concurrency, ATTEMPTS, |&index| {
        let response = client.upload_asset(&files[index], &metadata[index], options.visibility.as_deref())?;
        tally.record(response.status == "duplicate", metadata[index].len());
        if let Some(bar) = &bar {
            bar.set_position(tally.created_bytes.load(Ordering::SeqCst));
        }
        Ok(response.id)
    });
    if let Some(bar) = &bar {
        bar.abandon();
    }
    tally.print_summary();

    let mut new_assets = Vec::new();
    let mut failures = Vec::new();
    for (path, result) in files.iter().zip(results) {
        match result {
            Ok(id) => new_assets.push(Asset {
                id: Some(id),
                filepath: path.clone(),
            }),
            Err(error) => failures.push((path.clone(), error)),
        }
    }
    output::print_failures("Failed to upload", "asset", &failures);
    Ok(new_assets)
}

fn delete_files(uploaded: &[Asset], duplicates: &[Asset], options: &UploadOptions) -> Result<()> {
    let mut file_count = 0;
    if options.delete {
        file_count += uploaded.len();
    }
    if options.delete_duplicates {
        file_count += duplicates.len();
    }
    if options.dry_run {
        println!("Would have deleted {} local asset{}", file_count, s(file_count));
        return Ok(());
    }
    if file_count == 0 {
        return Ok(());
    }

    println!("Deleting assets that have been uploaded...");
    let bar = output::count_bar("Deleting local assets", "assets", file_count as u64);
    let delete_all = |assets: &[Asset]| -> Result<()> {
        for chunk in assets.chunks(options.concurrency.max(1)) {
            for asset in chunk {
                delete_with_sidecar(&asset.filepath)?;
            }
            bar.inc(chunk.len() as u64);
        }
        Ok(())
    };
    let mut result = Ok(());
    if options.delete {
        result = delete_all(uploaded);
    }
    if result.is_ok() && options.delete_duplicates {
        result = delete_all(duplicates);
    }
    bar.abandon();
    result
}

fn delete_with_sidecar(filepath: &str) -> Result<()> {
    fs::remove_file(filepath).map_err(|error| Error::Fatal(node_io_error(&error, "unlink", filepath)))?;
    if let Some(sidecar) = crate::api::find_sidecar(filepath) {
        fs::remove_file(&sidecar).map_err(|error| Error::Fatal(node_io_error(&error, "unlink", &sidecar)))?;
    }
    Ok(())
}

/// Keeps the first occurrence of each path, like `[...new Set(paths)]`.
pub fn unique(paths: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    paths.into_iter().filter(|path| seen.insert(path.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_keeps_first_occurrences_in_order() {
        let paths = vec!["b".to_string(), "a".to_string(), "b".to_string()];
        assert_eq!(unique(paths), vec!["b", "a"]);
    }
}
