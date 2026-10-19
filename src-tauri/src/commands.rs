//! The commands the frontend calls:
//!
//! ```js
//! const { invoke } = window.__TAURI__.core;
//! await invoke('download_wallpaper', { dataUri, fileName });   // fileName optional
//! await invoke('set_wallpaper', { dataUri, target: 'both' });  // 'home' | 'lock' | 'both'
//!
//! // Image cache ('thumbnail' | 'fullres', plus 'any' where noted):
//! await invoke('store_image_cache', { imageType, url, days });
//! await invoke('erase_image_cache', { imageType /* or 'any' */, olderThanDays });
//! await invoke('clear_expired_image_cache', { imageType /* or 'any' */ });
//! await invoke('load_image_cache', { imageType, url, days, forceRefresh });  // -> data URI
//! ```
//!
//! Each runs its decoding, file I/O and Android calls on a blocking thread so
//! none of it stalls the async runtime. Downloads are async.

use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_scenic::{ScenicExt, SaveToGalleryRequest, SetWallpaperRequest, WallpaperTarget};

use crate::{
    cache::{self, EntryInfo, Kind, SweepResult},
    error::{Error, Result},
    fetch, image,
    temp::TempImage,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    /// content:// URI of the saved image.
    pub uri: String,
    /// Name it was saved under (may differ from the request if the name was taken).
    pub file_name: String,
    /// Folder in shared storage, e.g. "Pictures/Scenic/".
    pub relative_path: String,
    pub mime_type: &'static str,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperResult {
    pub target: WallpaperTarget,
    pub mime_type: &'static str,
    pub bytes: usize,
}

/// Save an image to Pictures/<app name>/ so it shows up in the gallery.
#[tauri::command]
pub async fn download_wallpaper<R: Runtime>(
    app: AppHandle<R>,
    data_uri: String,
    file_name: Option<String>,
) -> Result<DownloadResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let img = image::decode(&data_uri)?;
        drop(data_uri);

        let album = album_name(&app);
        let name = image::file_name(file_name.as_deref(), &album, img.format);
        let tmp = TempImage::write(&app, &img.bytes, img.format.extension())?;

        let saved = app.scenic().save_to_gallery(SaveToGalleryRequest {
            path: tmp.path_string()?,
            file_name: name,
            mime_type: img.format.mime().to_owned(),
            album,
        })?;

        Ok(DownloadResult {
            uri: saved.uri,
            file_name: saved.file_name,
            relative_path: saved.relative_path,
            mime_type: img.format.mime(),
            bytes: img.bytes.len(),
        })
    })
    .await?
}

/// Apply an image as the wallpaper on the home screen, lock screen, or both.
/// `target` defaults to "both" when omitted.
#[tauri::command]
pub async fn set_wallpaper<R: Runtime>(
    app: AppHandle<R>,
    data_uri: String,
    target: Option<String>,
) -> Result<WallpaperResult> {
    let target = parse_target(target.as_deref())?;

    tauri::async_runtime::spawn_blocking(move || {
        let img = image::decode(&data_uri)?;
        drop(data_uri);

        let tmp = TempImage::write(&app, &img.bytes, img.format.extension())?;

        let applied = app.scenic().set_wallpaper(SetWallpaperRequest {
            path: tmp.path_string()?,
            target,
        })?;

        Ok(WallpaperResult {
            target: applied.target,
            mime_type: img.format.mime(),
            bytes: img.bytes.len(),
        })
    })
    .await?
}

// ---------------------------------------------------------------------------
// Image cache
// ---------------------------------------------------------------------------

/// Download an image and store it in the cache, replacing any earlier entry
/// for the same URL and type. The creation time is recorded automatically and
/// the entry expires `days` days later (fractions are allowed).
#[tauri::command]
pub async fn store_image_cache<R: Runtime>(
    app: AppHandle<R>,
    image_type: String,
    url: String,
    days: f64,
) -> Result<EntryInfo> {
    let kind = cache::parse_kind(&image_type)?;
    let url = url.trim().to_owned();
    let (info, _bytes) = download_and_store(cache_root(&app)?, kind, url, days).await?;
    Ok(info)
}

/// Delete cached images of `image_type` ("thumbnail", "fullres" or "any")
/// that were created `older_than_days` or more days ago. Judged by creation
/// date, not expiry. `0` deletes everything of that type.
#[tauri::command]
pub async fn erase_image_cache<R: Runtime>(
    app: AppHandle<R>,
    image_type: String,
    older_than_days: f64,
) -> Result<SweepResult> {
    let scope = cache::parse_scope(&image_type)?;
    let root = cache_root(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        cache::erase_older_than(&root, scope, older_than_days, cache::now_ms())
    })
    .await?
}

/// Delete cached images of `image_type` ("thumbnail", "fullres" or "any")
/// whose creation date plus lifetime has passed.
#[tauri::command]
pub async fn clear_expired_image_cache<R: Runtime>(
    app: AppHandle<R>,
    image_type: String,
) -> Result<SweepResult> {
    let scope = cache::parse_scope(&image_type)?;
    let root = cache_root(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        cache::remove_expired(&root, scope, cache::now_ms())
    })
    .await?
}

/// Get an image as a `data:` URI usable as an `<img src>`.
///
/// Serves the cached copy when there is a live one. Otherwise (no entry, an
/// expired entry, or `force_refresh`) it downloads the image, stores it with
/// a lifetime of `days`, and returns that. `days` only matters when a new
/// entry is written; a cache hit keeps the lifetime it was stored with.
#[tauri::command]
pub async fn load_image_cache<R: Runtime>(
    app: AppHandle<R>,
    image_type: String,
    url: String,
    days: f64,
    force_refresh: Option<bool>,
) -> Result<String> {
    let kind = cache::parse_kind(&image_type)?;
    let url = url.trim().to_owned();
    cache::lifetime_ms(days)?;
    fetch::validate_url(&url)?;
    let root = cache_root(&app)?;

    if !force_refresh.unwrap_or(false) {
        let (root, url) = (root.clone(), url.clone());
        let hit = tauri::async_runtime::spawn_blocking(move || {
            cache::read(&root, kind, &url, cache::now_ms())
                .map(|img| to_data_uri(img.format.mime(), &img.bytes))
        })
        .await?;
        if let Some(data_uri) = hit {
            return Ok(data_uri);
        }
    }

    let (info, bytes) = download_and_store(root, kind, url, days).await?;
    let data_uri =
        tauri::async_runtime::spawn_blocking(move || to_data_uri(info.mime_type, &bytes)).await?;
    Ok(data_uri)
}

/// `<app cache dir>/image-cache`. In the cache dir on purpose: Android's
/// "Clear cache" button in the app's settings empties it.
fn cache_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    Ok(app.path().app_cache_dir()?.join(cache::DIR_NAME))
}

/// Download `url`, write it to the cache, and hand back the entry info and
/// the bytes. Arguments are checked before any network traffic happens.
async fn download_and_store(
    root: PathBuf,
    kind: Kind,
    url: String,
    days: f64,
) -> Result<(EntryInfo, Vec<u8>)> {
    cache::lifetime_ms(days)?;
    let parsed = fetch::validate_url(&url)?;
    let bytes = fetch::download(&parsed).await?;

    tauri::async_runtime::spawn_blocking(move || {
        let info = cache::store(&root, kind, &url, days, &bytes, cache::now_ms())?;
        Ok((info, bytes))
    })
    .await?
}

fn to_data_uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

/// Lenient so the frontend doesn't have to remember the exact spelling.
fn parse_target(raw: Option<&str>) -> Result<WallpaperTarget> {
    let Some(raw) = raw else {
        return Ok(WallpaperTarget::Both);
    };
    let key: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();

    match key.as_str() {
        "home" | "homescreen" | "system" => Ok(WallpaperTarget::Home),
        "lock" | "lockscreen" => Ok(WallpaperTarget::Lock),
        "both" | "all" | "" => Ok(WallpaperTarget::Both),
        _ => Err(Error::InvalidArgument(format!(
            "unknown wallpaper target \"{raw}\"; use \"home\", \"lock\" or \"both\""
        ))),
    }
}

/// Folder name under Pictures/: the productName from tauri.conf.json.
fn album_name<R: Runtime>(app: &AppHandle<R>) -> String {
    let name = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| app.package_info().name.clone());
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        "Scenic".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(parse_target(None).unwrap(), WallpaperTarget::Both);
        assert_eq!(parse_target(Some("Home")).unwrap(), WallpaperTarget::Home);
        assert_eq!(parse_target(Some("lock_screen")).unwrap(), WallpaperTarget::Lock);
        assert_eq!(parse_target(Some("BOTH")).unwrap(), WallpaperTarget::Both);
        assert!(parse_target(Some("desktop")).is_err());
    }
}
