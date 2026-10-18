use serde::de::DeserializeOwned;
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

use crate::models::*;

/// Kotlin package + class registered in android/src/main/java/nb/scenic/plugin.
const PLUGIN_IDENTIFIER: &str = "nb.scenic.plugin";
const PLUGIN_CLASS: &str = "ScenicPlugin";

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<Scenic<R>> {
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, PLUGIN_CLASS)?;
    Ok(Scenic(handle))
}

/// Handle to the Kotlin plugin. Every method blocks until Android answers,
/// so call these from a blocking thread, not from the async runtime directly.
pub struct Scenic<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Scenic<R> {
    pub fn save_to_gallery(&self, req: SaveToGalleryRequest) -> crate::Result<SaveToGalleryResponse> {
        self.0
            .run_mobile_plugin("saveToGallery", req)
            .map_err(Into::into)
    }

    pub fn set_wallpaper(&self, req: SetWallpaperRequest) -> crate::Result<SetWallpaperResponse> {
        self.0
            .run_mobile_plugin("setWallpaper", req)
            .map_err(Into::into)
    }
}
