//! The two commands the frontend calls:
//!
//! ```js
//! const { invoke } = window.__TAURI__.core;
//! await invoke('download_wallpaper', { dataUri, fileName });   // fileName optional
//! await invoke('set_wallpaper', { dataUri, target: 'both' });  // 'home' | 'lock' | 'both'
//! ```
//!
//! Each runs on a blocking thread: decoding, file I/O and the Android call
//! all block, and none of it should stall the async runtime.

use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_scenic::{ScenicExt, SaveToGalleryRequest, SetWallpaperRequest, WallpaperTarget};

use crate::{
    error::{Error, Result},
    image,
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
