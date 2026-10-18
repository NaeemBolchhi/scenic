use serde::{ser::Serializer, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Kotlin side rejected the call. `message` is the reason it gave.
    #[error("{message}")]
    Android {
        code: Option<String>,
        message: String,
    },

    /// The bridge itself failed (JNI, serialization, unreachable webview).
    #[error("android bridge error: {0}")]
    Bridge(String),

    /// Called on a platform other than Android (desktop `tauri dev`, iOS).
    #[error("this feature is only available on Android")]
    Unsupported,
}

#[cfg(mobile)]
impl From<tauri::plugin::mobile::PluginInvokeError> for Error {
    fn from(err: tauri::plugin::mobile::PluginInvokeError) -> Self {
        use tauri::plugin::mobile::PluginInvokeError;
        match err {
            PluginInvokeError::InvokeRejected(resp) => Error::Android {
                code: resp.code,
                message: resp
                    .message
                    .unwrap_or_else(|| "the Android side rejected the request".into()),
            },
            other => Error::Bridge(other.to_string()),
        }
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
