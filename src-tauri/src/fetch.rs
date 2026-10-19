//! Downloading an image from an http(s) URL, with a size cap.
//!
//! This runs in Rust rather than JS `fetch()` so the bytes go straight to the
//! cache file and the webview never holds the full download twice, and so
//! cross-origin rules don't apply to image hosts that lack CORS headers.

use std::{sync::OnceLock, time::Duration};

use crate::{
    error::{Error, Result},
    image::MAX_IMAGE_BYTES,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Whole request, including the body. Generous because full-res images are big.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn client() -> Result<&'static reqwest::Client> {
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let built = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .user_agent(concat!("Scenic/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Internal(format!("could not start the HTTP client: {e}")))?;
    Ok(CLIENT.get_or_init(|| built))
}

/// Check that `raw` is an absolute http(s) URL.
pub fn validate_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|e| Error::InvalidArgument(format!("\"{raw}\" is not a valid URL ({e})")))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(Error::InvalidArgument(format!(
            "only http and https URLs can be fetched, not \"{other}:\""
        ))),
    }
}

/// Download the body of `url`. Fails on non-2xx responses and on bodies
/// larger than [`MAX_IMAGE_BYTES`] (checked while streaming, so an oversized
/// or endless response is cut off early).
pub async fn download(url: &reqwest::Url) -> Result<Vec<u8>> {
    let mut response = client()?
        .get(url.clone())
        .send()
        .await
        .map_err(|e| Error::Network(e.without_url().to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(Error::Network(format!("the server answered {status}")));
    }

    if let Some(len) = response.content_length() {
        if len > MAX_IMAGE_BYTES as u64 {
            return Err(Error::TooLarge {
                size: len as usize,
                limit: MAX_IMAGE_BYTES,
            });
        }
    }

    let mut body = Vec::with_capacity(response.content_length().unwrap_or(0).min(8 * 1024 * 1024) as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::Network(e.without_url().to_string()))?
    {
        if body.len() + chunk.len() > MAX_IMAGE_BYTES {
            return Err(Error::TooLarge {
                size: body.len() + chunk.len(),
                limit: MAX_IMAGE_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_http_and_https() {
        assert!(validate_url("https://example.com/a.jpg").is_ok());
        assert!(validate_url(" http://example.com/a.jpg ").is_ok());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("data:image/png;base64,AAAA").is_err());
        assert!(validate_url("content://media/1").is_err());
        assert!(validate_url("/relative/path.jpg").is_err());
        assert!(validate_url("").is_err());
    }
}
