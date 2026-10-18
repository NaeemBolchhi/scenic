//! Stand-in for non-Android builds, so `tauri dev` on a desktop still runs and
//! you can work on the frontend. Both calls return `Error::Unsupported`.

use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use crate::models::*;

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> crate::Result<Scenic<R>> {
    Ok(Scenic(PhantomData))
}

pub struct Scenic<R: Runtime>(PhantomData<fn() -> R>);

impl<R: Runtime> Scenic<R> {
    pub fn save_to_gallery(&self, _req: SaveToGalleryRequest) -> crate::Result<SaveToGalleryResponse> {
        Err(crate::Error::Unsupported)
    }

    pub fn set_wallpaper(&self, _req: SetWallpaperRequest) -> crate::Result<SetWallpaperResponse> {
        Err(crate::Error::Unsupported)
    }
}
