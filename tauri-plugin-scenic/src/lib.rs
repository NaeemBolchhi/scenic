//! Rust side of the Scenic Android bridge.
//!
//! The app crate does all validation and decoding, writes the image to a
//! temporary file, and calls into this plugin with the file path. The Kotlin
//! half (`android/`) only talks to MediaStore and WallpaperManager.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

mod error;
mod models;

#[cfg(target_os = "android")]
mod android;
#[cfg(not(target_os = "android"))]
mod unsupported;

#[cfg(target_os = "android")]
pub use android::Scenic;
#[cfg(not(target_os = "android"))]
pub use unsupported::Scenic;

pub use error::{Error, Result};
pub use models::*;

/// Access the plugin from anything that implements [`Manager`]
/// (`AppHandle`, `App`, `WebviewWindow`, ...).
pub trait ScenicExt<R: Runtime> {
    fn scenic(&self) -> &Scenic<R>;
}

impl<R: Runtime, T: Manager<R>> ScenicExt<R> for T {
    fn scenic(&self) -> &Scenic<R> {
        self.state::<Scenic<R>>().inner()
    }
}

/// Register with `tauri::Builder::default().plugin(tauri_plugin_scenic::init())`.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("scenic")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let scenic = android::init(app, api)?;
            #[cfg(not(target_os = "android"))]
            let scenic = unsupported::init(app, api)?;
            app.manage(scenic);
            Ok(())
        })
        .build()
}
