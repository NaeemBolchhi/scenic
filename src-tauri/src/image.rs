//! Turning a `data:` URI (or bare base64) into checked image bytes.
//!
//! The declared MIME type in a data URI is often wrong or missing, so the
//! format is always taken from the file's magic bytes.

use base64::{
    alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};

use crate::error::{Error, Result};

/// Largest decoded image accepted. A 4K PNG is usually 5–25 MB; 64 MB leaves
/// room for 8K sources without letting a bad payload exhaust memory.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

const MAX_STEM_CHARS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
    Gif,
    Bmp,
    Heif,
    Heic,
    Avif,
}

impl ImageFormat {
    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Bmp => "image/bmp",
            ImageFormat::Heif => "image/heif",
            ImageFormat::Heic => "image/heic",
            ImageFormat::Avif => "image/avif",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::Webp => "webp",
            ImageFormat::Gif => "gif",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Heif => "heif",
            ImageFormat::Heic => "heic",
            ImageFormat::Avif => "avif",
        }
    }

    /// Identify the format from the first bytes of the file.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(ImageFormat::Jpeg);
        }
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(ImageFormat::Png);
        }
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Some(ImageFormat::Webp);
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(ImageFormat::Gif);
        }
        if bytes.len() >= 14 && bytes.starts_with(b"BM") {
            return Some(ImageFormat::Bmp);
        }
        // ISO-BMFF: [4-byte box size]["ftyp"][major brand]
        if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
            return match &bytes[8..12] {
                b"avif" | b"avis" => Some(ImageFormat::Avif),
                b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" => Some(ImageFormat::Heic),
                b"mif1" | b"msf1" => Some(ImageFormat::Heif),
                _ => None,
            };
        }
        None
    }
}

#[derive(Debug)]
pub struct DecodedImage {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
}

/// Accepts `data:image/png;base64,....` or a bare base64 string.
/// Whitespace and line breaks inside the base64 are ignored, padding is
/// optional, and URL-safe base64 is accepted too.
pub fn decode(input: &str) -> Result<DecodedImage> {
    let input = input.trim();
    if input.is_empty() {
        return Err(Error::InvalidImage("no image data was given".into()));
    }

    let payload = match strip_data_uri_header(input)? {
        Some(rest) => rest,
        None => input,
    };

    // Reject oversized payloads before allocating the decoded buffer.
    let estimated = payload.len() / 4 * 3;
    if estimated > MAX_IMAGE_BYTES + 3 {
        return Err(Error::TooLarge {
            size: estimated,
            limit: MAX_IMAGE_BYTES,
        });
    }

    let compact: Vec<u8> = payload
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if compact.is_empty() {
        return Err(Error::InvalidImage("the data URI has no image data after the comma".into()));
    }

    let bytes = base64_engine(&alphabet::STANDARD)
        .decode(&compact)
        .or_else(|_| base64_engine(&alphabet::URL_SAFE).decode(&compact))
        .map_err(|e| Error::InvalidImage(format!("the image data is not valid base64 ({e})")))?;

    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(Error::TooLarge {
            size: bytes.len(),
            limit: MAX_IMAGE_BYTES,
        });
    }

    let format = ImageFormat::sniff(&bytes).ok_or_else(|| {
        Error::InvalidImage(
            "the data is not a supported image (expected JPEG, PNG, WebP, GIF, BMP, HEIC or AVIF)".into(),
        )
    })?;

    Ok(DecodedImage { bytes, format })
}

/// Returns the part after the comma for a `data:` URI, `None` for bare base64.
fn strip_data_uri_header(input: &str) -> Result<Option<&str>> {
    let has_scheme = input
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("data:"));
    if !has_scheme {
        return Ok(None);
    }

    let comma = input
        .find(',')
        .ok_or_else(|| Error::InvalidImage("malformed data URI: missing ','".into()))?;
    let header = &input[5..comma];

    let mut parts = header.split(';').map(str::trim);
    let media_type = parts.next().unwrap_or("");
    let is_base64 = parts.any(|p| p.eq_ignore_ascii_case("base64"));

    if !is_base64 {
        return Err(Error::InvalidImage(
            "data URI must be base64-encoded (data:image/...;base64,...)".into(),
        ));
    }
    if !media_type.is_empty()
        && !media_type.to_ascii_lowercase().starts_with("image/")
        && !media_type.eq_ignore_ascii_case("application/octet-stream")
    {
        return Err(Error::InvalidImage(format!(
            "data URI declares \"{media_type}\", which is not an image type"
        )));
    }

    Ok(Some(&input[comma + 1..]))
}

fn base64_engine(alphabet: &alphabet::Alphabet) -> GeneralPurpose {
    GeneralPurpose::new(
        alphabet,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    )
}

/// Build a safe gallery file name. Uses the caller's name when it has one,
/// otherwise `<prefix>_<unix millis>`. The extension always matches the
/// real format, so "sunset.png" holding JPEG bytes is saved as "sunset.jpg".
pub fn file_name(requested: Option<&str>, prefix: &str, format: ImageFormat) -> String {
    let stem = requested
        .map(clean_stem)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}_{}", clean_stem(prefix), unix_millis()));
    format!("{stem}.{}", format.extension())
}

fn clean_stem(raw: &str) -> String {
    // Keep only the last path component.
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();

    // Drop an existing image extension; we add the correct one back.
    let base = match base.rsplit_once('.') {
        Some((stem, ext)) if is_image_extension(ext) => stem,
        _ => base,
    };

    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .take(MAX_STEM_CHARS)
        .collect();

    // Leading dots would make the file hidden; trailing dots and spaces
    // are stripped by some file systems.
    cleaned
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string()
}

fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "heic" | "heif" | "avif"
    )
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1x1 transparent PNG.
    const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

    #[test]
    fn decodes_data_uri() {
        let img = decode(&format!("data:image/png;base64,{PNG_B64}")).unwrap();
        assert_eq!(img.format, ImageFormat::Png);
    }

    #[test]
    fn decodes_bare_base64_with_whitespace_and_no_padding() {
        let messy = format!("  {}\n{} ", &PNG_B64[..40], PNG_B64[40..].trim_end_matches('='));
        assert_eq!(decode(&messy).unwrap().format, ImageFormat::Png);
    }

    #[test]
    fn trusts_bytes_over_declared_type() {
        let img = decode(&format!("data:image/jpeg;base64,{PNG_B64}")).unwrap();
        assert_eq!(img.format, ImageFormat::Png);
    }

    #[test]
    fn rejects_non_image() {
        assert!(decode("data:text/plain;base64,aGVsbG8gd29ybGQ=").is_err());
        assert!(decode("aGVsbG8gd29ybGQgaGVsbG8gd29ybGQ=").is_err());
        assert!(decode("data:image/png,rawtext").is_err());
        assert!(decode("").is_err());
    }

    #[test]
    fn sniffs_formats() {
        assert_eq!(ImageFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::Webp));
        assert_eq!(ImageFormat::sniff(b"\0\0\0\x1cftypavif\0\0"), Some(ImageFormat::Avif));
        assert_eq!(ImageFormat::sniff(b"\0\0\0\x18ftypheic\0\0"), Some(ImageFormat::Heic));
    }

    #[test]
    fn builds_file_names() {
        assert_eq!(file_name(Some("Sunset.PNG"), "Scenic", ImageFormat::Jpeg), "Sunset.jpg");
        assert_eq!(file_name(Some("../../etc/passwd"), "Scenic", ImageFormat::Png), "passwd.png");
        assert_eq!(file_name(Some("a:b*c?"), "Scenic", ImageFormat::Png), "a_b_c_.png");
        assert_eq!(file_name(Some("my.photo.v2"), "Scenic", ImageFormat::Webp), "my.photo.v2.webp");
        let auto = file_name(Some("  ..  "), "Scenic", ImageFormat::Png);
        assert!(auto.starts_with("Scenic_") && auto.ends_with(".png"));
        assert!(file_name(None, "Scenic", ImageFormat::Gif).ends_with(".gif"));
    }
}
