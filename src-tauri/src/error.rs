use serde::{ser::SerializeStruct, Serialize, Serializer};

/// Errors returned to JS. They arrive in the `catch` of `invoke()` as
/// `{ kind: "...", message: "..." }`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    InvalidImage(String),

    #[error("{0}")]
    InvalidArgument(String),

    #[error("image is too large ({size} bytes; limit is {limit} bytes)")]
    TooLarge { size: usize, limit: usize },

    #[error("could not prepare the image: {0}")]
    Io(#[from] std::io::Error),

    #[error("could not download the image: {0}")]
    Network(String),

    #[error("{0}")]
    Platform(#[from] tauri_plugin_scenic::Error),

    #[error("{0}")]
    Internal(String),
}

impl Error {
    fn kind(&self) -> &'static str {
        match self {
            Error::InvalidImage(_) => "invalidImage",
            Error::InvalidArgument(_) => "invalidArgument",
            Error::TooLarge { .. } => "tooLarge",
            Error::Io(_) => "io",
            Error::Network(_) => "network",
            Error::Platform(tauri_plugin_scenic::Error::Unsupported) => "unsupported",
            Error::Platform(_) => "android",
            Error::Internal(_) => "internal",
        }
    }
}

impl From<tauri::Error> for Error {
    fn from(err: tauri::Error) -> Self {
        Error::Internal(err.to_string())
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("Error", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

pub type Result<T> = std::result::Result<T, Error>;
