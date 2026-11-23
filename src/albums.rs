//! `--album` (one album per parent folder name) and `--album-name`.

use std::collections::HashMap;
use std::thread;

use crate::api::{Album, ApiError, Client};
use crate::auth::fatal_api_error;
use crate::cli::UploadOptions;
use crate::error::Result;
use crate::output::{self, Asset, s};
use crate::pathutil;

/// The album an asset belongs to: `--album-name`, or the name of its parent folder.
pub fn album_name(filepath: &str, album_name: Option<&str>) -> String {
    match album_name {
        Some(name) => name.to_string(),
        None => pathutil::basename(&pathutil::dirname(filepath)),
    }
}

/// Album names that do not exist yet, in first-seen order.
fn missing_albums(assets: &[Asset], existing: &HashMap<String, String>, fixed_name: Option<&str>) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for asset in assets {
        let name = album_name(&asset.filepath, fixed_name);
        if !name.is_empty() && !existing.contains_key(&name) && !missing.contains(&name) {
            missing.push(name);
        }
    }
    missing
}

/// Asset ids grouped by album id, albums in first-seen order.
fn group_by_album(
    assets: &[Asset],
    existing: &HashMap<String, String>,
    fixed_name: Option<&str>,
) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for asset in assets {
        let name = album_name(&asset.filepath, fixed_name);
        let (Some(album_id), Some(asset_id)) = (existing.get(&name), &asset.id) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        match groups.iter_mut().find(|(id, _)| id == album_id) {
            Some((_, ids)) => ids.push(asset_id.clone()),
            None => groups.push((album_id.clone(), vec![asset_id.clone()])),
        }
    }
    groups
}

pub fn update_albums(client: &Client, assets: &[Asset], options: &UploadOptions) -> Result<()> {
    if !options.album && options.album_name.is_none() {
        return Ok(());
    }
    let fixed_name = options.album_name.as_deref();
    let albums = client
        .get_all_albums()
        .map_err(|error| fatal_api_error(&error, "Failed to load albums"))?;
    let mut existing: HashMap<String, String> = albums.into_iter().map(|album| (album.album_name, album.id)).collect();
    let new_albums = missing_albums(assets, &existing, fixed_name);

    if options.dry_run {
        println!(
            "Would have created {} new album{}",
            new_albums.len(),
            s(new_albums.len())
        );
        println!("Would have updated albums of {} asset{}", assets.len(), s(assets.len()));
        return Ok(());
    }

    let concurrency = options.concurrency.max(1);
    let bar = output::count_bar("Creating albums", "albums", new_albums.len() as u64);
    for names in new_albums.chunks(concurrency) {
        let created: Vec<std::result::Result<Album, ApiError>> = thread::scope(|scope| {
            let handles: Vec<_> = names
                .iter()
                .map(|name| scope.spawn(|| client.create_album(name)))
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(ApiError::Transport("request thread panicked".to_string())))
                })
                .collect()
        });
        for album in created {
            let album = album.map_err(|error| {
                bar.abandon();
                fatal_api_error(&error, "Failed to create album")
            })?;
            existing.insert(album.album_name, album.id);
        }
        bar.inc(names.len() as u64);
    }
    bar.abandon();
    println!(
        "Successfully created {} new album{}",
        new_albums.len(),
        s(new_albums.len())
    );
    println!("Successfully updated {} asset{}", assets.len(), s(assets.len()));

    let bar = output::count_bar("Adding assets to albums", "assets", assets.len() as u64);
    let chunk_size = (1000 * concurrency).min(65_000);
    for (album_id, ids) in group_by_album(assets, &existing, fixed_name) {
        for chunk in ids.chunks(chunk_size) {
            client.add_assets_to_album(&album_id, chunk).map_err(|error| {
                bar.abandon();
                fatal_api_error(&error, "Failed to add assets to album")
            })?;
            bar.inc(chunk.len() as u64);
        }
    }
    bar.abandon();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, filepath: &str) -> Asset {
        Asset {
            id: Some(id.to_string()),
            filepath: filepath.to_string(),
        }
    }

    #[test]
    fn album_name_is_the_parent_folder() {
        assert_eq!(album_name("/photos/Holiday 2024/IMG_1.jpg", None), "Holiday 2024");
        assert_eq!(album_name("/IMG_1.jpg", None), "");
        assert_eq!(album_name("relative/dir/x.jpg", None), "dir");
        assert_eq!(album_name("/photos/a/x.jpg", Some("Fixed")), "Fixed");
    }

    #[test]
    fn finds_missing_albums_and_groups_assets() {
        let assets = vec![
            asset("1", "/p/A/x.jpg"),
            asset("2", "/p/B/y.jpg"),
            asset("3", "/p/A/z.jpg"),
        ];
        let mut existing = HashMap::from([("B".to_string(), "album-b".to_string())]);
        assert_eq!(missing_albums(&assets, &existing, None), vec!["A"]);
        existing.insert("A".to_string(), "album-a".to_string());
        assert_eq!(
            group_by_album(&assets, &existing, None),
            vec![
                ("album-a".to_string(), vec!["1".to_string(), "3".to_string()]),
                ("album-b".to_string(), vec!["2".to_string()])
            ]
        );
    }
}
