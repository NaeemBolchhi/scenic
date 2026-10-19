use serde::{Deserialize, Serialize};

/// Which screen(s) a wallpaper goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WallpaperTarget {
    Home,
    Lock,
    #[default]
    Both,
}

impl WallpaperTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            WallpaperTarget::Home => "home",
            WallpaperTarget::Lock => "lock",
            WallpaperTarget::Both => "both",
        }
    }
}

/// Copy an image file into the shared Pictures collection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveToGalleryRequest {
    /// Absolute path of a file the app can read (normally in the cache dir).
    pub path: String,
    /// Display name in the gallery, extension included.
    pub file_name: String,
    pub mime_type: String,
    /// Subfolder under Pictures/.
    pub album: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveToGalleryResponse {
    /// content:// URI of the new MediaStore entry.
    pub uri: String,
    /// Final display name. MediaStore may append " (1)" etc. on collisions.
    pub file_name: String,
    /// Relative path inside shared storage, e.g. "Pictures/Scenic/".
    pub relative_path: String,
}

/// Apply an image file as the wallpaper.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetWallpaperRequest {
    pub path: String,
    pub target: WallpaperTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetWallpaperResponse {
    pub target: WallpaperTarget,
    /// Wallpaper IDs WallpaperManager returned, for debugging.
    #[serde(default)]
    pub wallpaper_id: i64,
}

/// Match the status and navigation bars to the app's theme.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSystemBarsRequest {
    /// The app itself is dark, so the bar icons should be light.
    pub dark: bool,
    /// Optional "#RRGGBB" background for Android 14 and older; Android 15+
    /// ignores it and shows the page behind the bars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetSystemBarsResponse {}
