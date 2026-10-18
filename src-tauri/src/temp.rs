//! Short-lived image files in the app's cache directory.
//!
//! Base64 data can be tens of megabytes. Rather than push that through the
//! JNI bridge as JSON, Rust writes the decoded bytes to a file and the
//! Kotlin side reads the file directly.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use tauri::{AppHandle, Manager, Runtime};

use crate::error::{Error, Result};

const DIR_NAME: &str = "scenic-tmp";

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    Ok(app.path().app_cache_dir()?.join(DIR_NAME))
}

/// Remove files left by an earlier run that was killed mid-operation.
pub fn sweep<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(dir) = temp_dir(app) {
        let _ = fs::remove_dir_all(dir);
    }
}

/// A file deleted when this value is dropped, whether the operation
/// succeeded or not.
pub struct TempImage {
    path: PathBuf,
}

impl TempImage {
    pub fn write<R: Runtime>(app: &AppHandle<R>, bytes: &[u8], extension: &str) -> Result<Self> {
        let dir = temp_dir(app)?;
        fs::create_dir_all(&dir)?;

        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("img-{millis}-{n}.{extension}"));

        fs::write(&path, bytes)?;
        Ok(Self { path })
    }

    pub fn path_string(&self) -> Result<String> {
        self.path
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Internal("cache path is not valid UTF-8".into()))
    }
}

impl Drop for TempImage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
