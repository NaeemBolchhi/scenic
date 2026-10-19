//! On-disk image cache.
//!
//! Lives in the app's *cache* directory, so Android's
//! Settings > Apps > Scenic > Storage > "Clear cache" wipes it. Nothing here
//! assumes the folders still exist: they are created on write and a missing
//! folder just means "empty".
//!
//! ```text
//! <app cache dir>/image-cache/thumbnails/<sha256(url)>.img
//! <app cache dir>/image-cache/fullres/<sha256(url)>.img
//! ```
//!
//! Each entry is one file: a small header (URL, creation time, expiry time)
//! followed by the raw image bytes. Keeping both in one file means an entry
//! can never exist without its timestamps, and writing to a temp file and
//! renaming makes every store atomic: a reader sees the old entry or the new
//! one, never half of either. The creation time is stamped here, not passed
//! in by the caller.
//!
//! Everything in this module is synchronous and takes the cache root and the
//! current time as arguments, so it is easy to test. The Tauri commands that
//! wrap it live in `commands.rs`.

use std::{
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    error::{Error, Result},
    image::{ImageFormat, MAX_IMAGE_BYTES},
};

/// Folder under the app cache dir that holds everything in this module.
pub const DIR_NAME: &str = "image-cache";

const MAGIC: &[u8; 4] = b"SCIC";
const FORMAT_VERSION: u8 = 1;
const MAX_HEADER_BYTES: u32 = 16 * 1024;
const ENTRY_EXT: &str = "img";
const TEMP_EXT: &str = "tmp";

/// Upper bound on any "days" argument (10 years). Keeps the millisecond
/// arithmetic far away from overflow and catches typos like 1e9.
const MAX_DAYS: f64 = 3650.0;
const MS_PER_DAY: f64 = 86_400_000.0;

/// Temp files from a write that was killed midway are removed by sweeps once
/// they are older than this.
const STALE_TEMP: Duration = Duration::from_secs(60 * 60);

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Where an image is stored. Each kind has its own folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Thumbnail,
    Fullres,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Kind::Thumbnail => "thumbnails",
            Kind::Fullres => "fullres",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Thumbnail => "thumbnail",
            Kind::Fullres => "fullres",
        }
    }
}

/// Which kinds an erase/expire operation covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    One(Kind),
    Any,
}

impl Scope {
    fn kinds(self) -> &'static [Kind] {
        match self {
            Scope::One(Kind::Thumbnail) => &[Kind::Thumbnail],
            Scope::One(Kind::Fullres) => &[Kind::Fullres],
            Scope::Any => &[Kind::Thumbnail, Kind::Fullres],
        }
    }
}

/// Stored at the front of every entry file.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    url: String,
    /// Unix milliseconds. Stamped by `store`.
    created_at: u64,
    /// Unix milliseconds. `created_at` + the requested number of days.
    expires_at: u64,
}

/// What `store` reports back to JS.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryInfo {
    pub image_type: &'static str,
    pub url: String,
    /// Unix milliseconds (usable as `new Date(createdAt)`).
    pub created_at: u64,
    /// Unix milliseconds.
    pub expires_at: u64,
    pub bytes: usize,
    pub mime_type: &'static str,
}

/// A cache hit.
#[derive(Debug)]
pub struct CachedImage {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
}

/// What an erase/expire pass removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepResult {
    pub removed: usize,
    pub freed_bytes: u64,
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

/// "thumbnail" or "fullres", for operations that need one specific kind.
pub fn parse_kind(raw: &str) -> Result<Kind> {
    match normalize(raw).as_str() {
        "thumbnail" | "thumbnails" | "thumb" | "thumbs" => Ok(Kind::Thumbnail),
        "fullres" | "full" | "fullresolution" => Ok(Kind::Fullres),
        "any" | "all" => Err(Error::InvalidArgument(
            "\"any\" is not allowed here; the image type must be \"thumbnail\" or \"fullres\"".into(),
        )),
        _ => Err(unknown_type(raw)),
    }
}

/// "thumbnail", "fullres" or "any".
pub fn parse_scope(raw: &str) -> Result<Scope> {
    match normalize(raw).as_str() {
        "any" | "all" => Ok(Scope::Any),
        _ => parse_kind(raw).map(Scope::One),
    }
}

fn unknown_type(raw: &str) -> Error {
    Error::InvalidArgument(format!(
        "unknown image type \"{raw}\"; use \"thumbnail\", \"fullres\" or \"any\""
    ))
}

/// Case, spaces, dashes and underscores don't matter: "Full-Res" == "fullres".
fn normalize(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// A cache lifetime: finite, greater than zero, at most ten years.
pub fn lifetime_ms(days: f64) -> Result<u64> {
    if !days.is_finite() || days <= 0.0 {
        return Err(Error::InvalidArgument(
            "days must be a number greater than 0".into(),
        ));
    }
    if days > MAX_DAYS {
        return Err(Error::InvalidArgument(format!(
            "days must be at most {MAX_DAYS}"
        )));
    }
    Ok(((days * MS_PER_DAY).round() as u64).max(1))
}

/// An age threshold: like `lifetime_ms` but 0 is allowed ("everything").
fn age_ms(days: f64) -> Result<u64> {
    if days == 0.0 {
        return Ok(0);
    }
    if !days.is_finite() || days < 0.0 {
        return Err(Error::InvalidArgument(
            "olderThanDays must be a number, 0 or greater".into(),
        ));
    }
    if days > MAX_DAYS {
        return Err(Error::InvalidArgument(format!(
            "olderThanDays must be at most {MAX_DAYS}"
        )));
    }
    Ok((days * MS_PER_DAY).round() as u64)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Store / read
// ---------------------------------------------------------------------------

fn entry_path(root: &Path, kind: Kind, url: &str) -> PathBuf {
    let digest = Sha256::digest(url.as_bytes());
    root.join(kind.dir()).join(format!("{digest:x}.{ENTRY_EXT}"))
}

/// Save `bytes` as the cache entry for `url`, replacing any earlier entry
/// for the same URL and kind. The entry's creation time is `now`, and it
/// expires `days` later.
pub fn store(
    root: &Path,
    kind: Kind,
    url: &str,
    days: f64,
    bytes: &[u8],
    now: u64,
) -> Result<EntryInfo> {
    let lifetime = lifetime_ms(days)?;

    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(Error::TooLarge {
            size: bytes.len(),
            limit: MAX_IMAGE_BYTES,
        });
    }
    let format = ImageFormat::sniff(bytes).ok_or_else(|| {
        Error::InvalidImage(
            "the downloaded data is not a supported image (expected JPEG, PNG, WebP, GIF, BMP, HEIC or AVIF)"
                .into(),
        )
    })?;

    let header = Header {
        url: url.to_owned(),
        created_at: now,
        expires_at: now.saturating_add(lifetime),
    };
    let header_json =
        serde_json::to_vec(&header).map_err(|e| Error::Internal(e.to_string()))?;

    let dir = root.join(kind.dir());
    fs::create_dir_all(&dir)?;
    let target = entry_path(root, kind, url);
    let temp = dir.join(format!(
        "w{}-{}-{}.{TEMP_EXT}",
        std::process::id(),
        now,
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let written = (|| -> io::Result<()> {
        let mut f = File::create(&temp)?;
        f.write_all(MAGIC)?;
        f.write_all(&[FORMAT_VERSION])?;
        f.write_all(&(header_json.len() as u32).to_le_bytes())?;
        f.write_all(&header_json)?;
        f.write_all(bytes)?;
        f.flush()?;
        drop(f);
        // Replaces an existing entry in one step (also on Windows).
        fs::rename(&temp, &target)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(e.into());
    }

    Ok(EntryInfo {
        image_type: kind.name(),
        url: header.url,
        created_at: header.created_at,
        expires_at: header.expires_at,
        bytes: bytes.len(),
        mime_type: format.mime(),
    })
}

/// The cached image for `url`, or `None` if there is no entry, the entry is
/// expired (`now >= expires_at`) or the file is damaged. Never an error: for
/// a cache, "can't use it" and "not there" mean the same thing.
pub fn read(root: &Path, kind: Kind, url: &str, now: u64) -> Option<CachedImage> {
    let file = File::open(entry_path(root, kind, url)).ok()?;
    let mut reader = BufReader::new(file);

    let header = parse_header(&mut reader)?;
    if header.url != url || now >= header.expires_at {
        return None;
    }

    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).ok()?;
    let format = ImageFormat::sniff(&bytes)?;
    Some(CachedImage { bytes, format })
}

fn parse_header(reader: &mut impl Read) -> Option<Header> {
    let mut fixed = [0u8; 9];
    reader.read_exact(&mut fixed).ok()?;
    if &fixed[..4] != MAGIC || fixed[4] != FORMAT_VERSION {
        return None;
    }
    let len = u32::from_le_bytes(fixed[5..9].try_into().ok()?);
    if len == 0 || len > MAX_HEADER_BYTES {
        return None;
    }
    let mut json = vec![0u8; len as usize];
    reader.read_exact(&mut json).ok()?;
    serde_json::from_slice(&json).ok()
}

// ---------------------------------------------------------------------------
// Erase / expire
// ---------------------------------------------------------------------------

/// Remove entries in `scope` that were created `older_than_days` or more
/// days ago. `0` removes everything in scope.
pub fn erase_older_than(
    root: &Path,
    scope: Scope,
    older_than_days: f64,
    now: u64,
) -> Result<SweepResult> {
    let cutoff = now.saturating_sub(age_ms(older_than_days)?);
    sweep(root, scope, |h| h.created_at <= cutoff)
}

/// Remove entries in `scope` whose expiry time has passed.
pub fn remove_expired(root: &Path, scope: Scope, now: u64) -> Result<SweepResult> {
    sweep(root, scope, |h| now >= h.expires_at)
}

/// Walks the folders for `scope`, removing entries `should_remove` accepts.
/// Damaged entries (unreadable header) are always removed: they can never be
/// served, so they only waste space. Folders that don't exist are skipped.
fn sweep(root: &Path, scope: Scope, should_remove: impl Fn(&Header) -> bool) -> Result<SweepResult> {
    let mut result = SweepResult::default();

    for kind in scope.kinds() {
        let dir = root.join(kind.dir());
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }

            match path.extension().and_then(|e| e.to_str()) {
                Some(ENTRY_EXT) => {
                    // Can't open it right now (e.g. locked): leave it for next time.
                    let Ok(file) = File::open(&path) else { continue };
                    let remove = match parse_header(&mut BufReader::new(file)) {
                        Some(header) => should_remove(&header),
                        None => true,
                    };
                    if remove && fs::remove_file(&path).is_ok() {
                        result.removed += 1;
                        result.freed_bytes += meta.len();
                    }
                }
                Some(TEMP_EXT) => {
                    let stale = meta
                        .modified()
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .is_some_and(|age| age > STALE_TEMP);
                    if stale {
                        let _ = fs::remove_file(&path);
                    }
                }
                _ => {}
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;
    const T0: u64 = 1_800_000_000_000;

    /// A scratch folder removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "scenic-cache-test-{}-{}-{n}",
                std::process::id(),
                now_ms()
            ));
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Bytes that sniff as PNG, `extra` bytes longer than the signature.
    fn png(extra: usize) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend(std::iter::repeat(7u8).take(extra));
        v
    }

    fn jpeg() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]
    }

    fn files_in(root: &Path, kind: Kind) -> usize {
        fs::read_dir(root.join(kind.dir()))
            .map(|d| d.flatten().count())
            .unwrap_or(0)
    }

    #[test]
    fn stores_and_reads_back() {
        let s = Scratch::new();
        let info = store(s.path(), Kind::Thumbnail, "https://x/a.png", 7.0, &png(10), T0).unwrap();
        assert_eq!(info.image_type, "thumbnail");
        assert_eq!(info.created_at, T0);
        assert_eq!(info.expires_at, T0 + 7 * DAY);
        assert_eq!(info.mime_type, "image/png");

        let hit = read(s.path(), Kind::Thumbnail, "https://x/a.png", T0 + DAY).unwrap();
        assert_eq!(hit.bytes, png(10));
        assert_eq!(hit.format, ImageFormat::Png);
    }

    #[test]
    fn kinds_are_stored_separately() {
        let s = Scratch::new();
        let url = "https://x/same";
        store(s.path(), Kind::Thumbnail, url, 1.0, &png(1), T0).unwrap();
        assert!(read(s.path(), Kind::Fullres, url, T0).is_none());

        store(s.path(), Kind::Fullres, url, 1.0, &jpeg(), T0).unwrap();
        assert_eq!(read(s.path(), Kind::Thumbnail, url, T0).unwrap().format, ImageFormat::Png);
        assert_eq!(read(s.path(), Kind::Fullres, url, T0).unwrap().format, ImageFormat::Jpeg);
    }

    #[test]
    fn storing_again_replaces_the_entry_and_restamps_it() {
        let s = Scratch::new();
        let url = "https://x/a";
        store(s.path(), Kind::Fullres, url, 1.0, &png(1), T0).unwrap();
        let second = store(s.path(), Kind::Fullres, url, 5.0, &jpeg(), T0 + 3 * DAY).unwrap();

        assert_eq!(files_in(s.path(), Kind::Fullres), 1);
        assert_eq!(second.created_at, T0 + 3 * DAY);
        assert_eq!(second.expires_at, T0 + 8 * DAY);
        let hit = read(s.path(), Kind::Fullres, url, T0 + 3 * DAY).unwrap();
        assert_eq!(hit.format, ImageFormat::Jpeg);
    }

    #[test]
    fn expired_entries_are_not_served() {
        let s = Scratch::new();
        store(s.path(), Kind::Thumbnail, "u", 2.0, &png(1), T0).unwrap();
        assert!(read(s.path(), Kind::Thumbnail, "u", T0 + 2 * DAY - 1).is_some());
        assert!(read(s.path(), Kind::Thumbnail, "u", T0 + 2 * DAY).is_none());
    }

    #[test]
    fn rejects_non_images_and_bad_days() {
        let s = Scratch::new();
        assert!(matches!(
            store(s.path(), Kind::Thumbnail, "u", 1.0, b"<html>not an image</html>", T0),
            Err(Error::InvalidImage(_))
        ));
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 4000.0] {
            assert!(
                matches!(store(s.path(), Kind::Thumbnail, "u", bad, &png(1), T0), Err(Error::InvalidArgument(_))),
                "days = {bad}"
            );
        }
        assert_eq!(files_in(s.path(), Kind::Thumbnail), 0);
    }

    #[test]
    fn erase_older_than_respects_type_and_age() {
        let s = Scratch::new();
        // Thumbnails created 5, 4 and 1 days before "now"; one fullres at 5 days.
        let now = T0 + 10 * DAY;
        store(s.path(), Kind::Thumbnail, "t5", 30.0, &png(1), now - 5 * DAY).unwrap();
        store(s.path(), Kind::Thumbnail, "t4", 30.0, &png(1), now - 4 * DAY).unwrap();
        store(s.path(), Kind::Thumbnail, "t1", 30.0, &png(1), now - DAY).unwrap();
        store(s.path(), Kind::Fullres, "f5", 30.0, &png(1), now - 5 * DAY).unwrap();

        let r = erase_older_than(s.path(), Scope::One(Kind::Thumbnail), 3.0, now).unwrap();
        assert_eq!(r.removed, 2);
        assert!(r.freed_bytes > 0);

        assert!(read(s.path(), Kind::Thumbnail, "t5", now).is_none());
        assert!(read(s.path(), Kind::Thumbnail, "t4", now).is_none());
        assert!(read(s.path(), Kind::Thumbnail, "t1", now).is_some());
        assert!(read(s.path(), Kind::Fullres, "f5", now).is_some(), "fullres must be untouched");
    }

    #[test]
    fn erase_uses_creation_date_not_expiry() {
        let s = Scratch::new();
        let now = T0 + 10 * DAY;
        // Old but long-lived: created 5 days ago, expires in 100 days.
        store(s.path(), Kind::Thumbnail, "old", 105.0, &png(1), now - 5 * DAY).unwrap();
        let r = erase_older_than(s.path(), Scope::Any, 3.0, now).unwrap();
        assert_eq!(r.removed, 1);
    }

    #[test]
    fn erase_any_and_zero_days_clears_everything() {
        let s = Scratch::new();
        store(s.path(), Kind::Thumbnail, "a", 1.0, &png(1), T0).unwrap();
        store(s.path(), Kind::Fullres, "b", 1.0, &png(1), T0).unwrap();
        let r = erase_older_than(s.path(), Scope::Any, 0.0, T0).unwrap();
        assert_eq!(r.removed, 2);
        assert_eq!(files_in(s.path(), Kind::Thumbnail) + files_in(s.path(), Kind::Fullres), 0);
    }

    #[test]
    fn erase_boundary_is_inclusive() {
        let s = Scratch::new();
        store(s.path(), Kind::Thumbnail, "a", 30.0, &png(1), T0).unwrap();
        // Exactly 3 days old counts as "older than 3 days" (>= 3 days).
        assert_eq!(erase_older_than(s.path(), Scope::Any, 3.0, T0 + 3 * DAY - 1).unwrap().removed, 0);
        assert_eq!(erase_older_than(s.path(), Scope::Any, 3.0, T0 + 3 * DAY).unwrap().removed, 1);
    }

    #[test]
    fn remove_expired_compares_creation_plus_lifetime_to_now() {
        let s = Scratch::new();
        store(s.path(), Kind::Thumbnail, "short", 1.0, &png(1), T0).unwrap();
        store(s.path(), Kind::Thumbnail, "long", 10.0, &png(1), T0).unwrap();
        store(s.path(), Kind::Fullres, "full-short", 1.0, &png(1), T0).unwrap();

        // Only thumbnails are in scope, so the expired fullres entry survives.
        let r = remove_expired(s.path(), Scope::One(Kind::Thumbnail), T0 + 2 * DAY).unwrap();
        assert_eq!(r.removed, 1);
        assert!(read(s.path(), Kind::Thumbnail, "long", T0 + 2 * DAY).is_some());
        assert_eq!(files_in(s.path(), Kind::Fullres), 1);

        let r = remove_expired(s.path(), Scope::Any, T0 + 2 * DAY).unwrap();
        assert_eq!(r.removed, 1);
        assert_eq!(files_in(s.path(), Kind::Fullres), 0);
    }

    #[test]
    fn sweeps_tolerate_missing_folders_and_clean_up_damaged_files() {
        let s = Scratch::new();
        // Android "Clear cache" can remove the folders entirely.
        assert_eq!(remove_expired(s.path(), Scope::Any, T0).unwrap(), SweepResult::default());
        assert_eq!(erase_older_than(s.path(), Scope::Any, 1.0, T0).unwrap(), SweepResult::default());
        assert!(read(s.path(), Kind::Thumbnail, "u", T0).is_none());

        // A truncated/garbage entry is removed by the next sweep.
        store(s.path(), Kind::Thumbnail, "u", 30.0, &png(1), T0).unwrap();
        let path = entry_path(s.path(), Kind::Thumbnail, "u");
        fs::write(&path, b"garbage").unwrap();
        assert!(read(s.path(), Kind::Thumbnail, "u", T0).is_none());
        assert_eq!(remove_expired(s.path(), Scope::Any, T0).unwrap().removed, 1);
        assert!(!path.exists());
    }

    #[test]
    fn storing_works_after_the_folders_are_wiped() {
        let s = Scratch::new();
        store(s.path(), Kind::Fullres, "u", 1.0, &png(1), T0).unwrap();
        fs::remove_dir_all(s.path()).unwrap();
        store(s.path(), Kind::Fullres, "u", 1.0, &png(1), T0).unwrap();
        assert!(read(s.path(), Kind::Fullres, "u", T0).is_some());
    }

    #[test]
    fn parses_types_leniently() {
        assert_eq!(parse_kind("Thumbnail").unwrap(), Kind::Thumbnail);
        assert_eq!(parse_kind("thumb").unwrap(), Kind::Thumbnail);
        assert_eq!(parse_kind("full-res").unwrap(), Kind::Fullres);
        assert_eq!(parse_kind("FULLRES").unwrap(), Kind::Fullres);
        assert!(parse_kind("any").is_err(), "storing needs a concrete type");
        assert!(parse_kind("banner").is_err());
        assert_eq!(parse_scope("any").unwrap(), Scope::Any);
        assert_eq!(parse_scope("fullres").unwrap(), Scope::One(Kind::Fullres));
        assert!(parse_scope("").is_err());
    }

    #[test]
    fn rejects_bad_ages() {
        let s = Scratch::new();
        for bad in [-1.0, f64::NAN, f64::INFINITY, 4000.0] {
            assert!(erase_older_than(s.path(), Scope::Any, bad, T0).is_err(), "age = {bad}");
        }
    }
}
