//! Small artwork thumbnails for the control panel's live card preview.
use anyhow::{Context, Result};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, imageops::FilterType};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mask {
    Square,
    /// Corner radius in pixels.
    Rounded(u32),
    Circle,
}

/// Premultiplied, top-down BGRA pixels, ready for a 32-bit DIB and `AlphaBlend`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Decodes PNG, JPEG or WebP, scales it to cover `width`×`height`, centre-crops,
/// and applies `mask` with a softened edge.
pub fn decode(bytes: &[u8], width: u32, height: u32, mask: Mask) -> Result<Thumbnail> {
    anyhow::ensure!(
        width > 0 && height > 0 && width <= 1024 && height <= 1024,
        "Bad size"
    );
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Unknown image format")?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 << 20);
    reader.limits(limits);
    let image = reader.decode().context("Could not decode the image")?;
    let rgba = image
        .resize_to_fill(width, height, FilterType::Triangle)
        .to_rgba8();
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let [r, g, b, a] = pixel.0;
        let coverage = coverage(mask, x, y, width, height);
        let alpha = (a as f32 * coverage).round() as u32;
        let premultiply = |c: u8| ((c as u32 * alpha + 127) / 255) as u8;
        bgra.extend([premultiply(b), premultiply(g), premultiply(r), alpha as u8]);
    }
    Ok(Thumbnail {
        width,
        height,
        bgra,
    })
}

/// How much of pixel (x, y) lies inside the mask, 0.0–1.0, with a 1 px soft edge.
fn coverage(mask: Mask, x: u32, y: u32, width: u32, height: u32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let edge = |distance_outside: f32| (0.5 - distance_outside).clamp(0.0, 1.0);
    match mask {
        Mask::Square => 1.0,
        Mask::Circle => {
            let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
            let radius = cx.min(cy);
            edge(((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius)
        }
        Mask::Rounded(radius) => {
            let r = (radius as f32)
                .min(width as f32 / 2.0)
                .min(height as f32 / 2.0);
            let nearest_x = px.clamp(r, width as f32 - r);
            let nearest_y = py.clamp(r, height as f32 - r);
            let distance = ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt();
            if distance == 0.0 {
                1.0
            } else {
                edge(distance - r)
            }
        }
    }
}

/// Decodes PNG, JPEG or WebP, applies any EXIF orientation, downscales to fit within 512×512,
/// and re-encodes as PNG (stripping any EXIF/GPS metadata while preserving transparency).
pub fn prepare_upload(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Unsupported image format (use PNG, JPEG, or WebP)")?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 << 20);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .context("Could not decode the image")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).context("Could not decode the image")?;
    image.apply_orientation(orientation);
    let resized = if image.width() > 512 || image.height() > 512 {
        image.resize(512, 512, FilterType::Triangle)
    } else {
        image
    };
    let mut out = Cursor::new(Vec::new());
    resized
        .write_to(&mut out, image::ImageFormat::Png)
        .context("Could not encode PNG")?;
    Ok(out.into_inner())
}

/// Downloads (any public HTTPS URL, up to 8 MB) and decodes a thumbnail.
#[cfg(windows)]
pub fn fetch(url: &str, width: u32, height: u32, mask: Mask) -> Result<Thumbnail> {
    anyhow::ensure!(url.starts_with("https://"), "Not an HTTPS image");
    decode(&crate::net::get(url, 8 << 20)?, width, height, mask)
}

// --- Persistent thumbnail cache -------------------------------------------------

/// Cached files stored below [`cache_root`].
const CACHE_MAGIC: &[u8; 7] = b"SCRPT1\0";
const CACHE_HEADER: usize = 15; // magic, width, height
const CACHE_MAX_FILES: usize = 512;
const CACHE_MAX_BYTES: u64 = 128 << 20;
const CACHE_ENV: &str = "SCRP_CACHE_DIR";

/// Where cached thumbnails live: `%LOCALAPPDATA%\StarCitizenRichPresence\thumbnails`,
/// or `SCRP_CACHE_DIR` when that environment variable is set (used by tests).
pub fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(CACHE_ENV).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let local = std::env::var_os("LOCALAPPDATA")?;
    if local.is_empty() {
        return None;
    }
    Some(
        PathBuf::from(local)
            .join("StarCitizenRichPresence")
            .join("thumbnails"),
    )
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// A stable FNV-1a key over everything that changes the pixels.
fn cache_key(url: &str, width: u32, height: u32, mask: Mask) -> u64 {
    let mut hash = fnv1a(FNV_OFFSET, url.as_bytes());
    hash = fnv1a(hash, &width.to_le_bytes());
    hash = fnv1a(hash, &height.to_le_bytes());
    match mask {
        Mask::Square => fnv1a(hash, &[0]),
        Mask::Rounded(radius) => fnv1a(fnv1a(hash, &[1]), &radius.to_le_bytes()),
        Mask::Circle => fnv1a(hash, &[2]),
    }
}

fn cache_file_name(url: &str, width: u32, height: u32, mask: Mask) -> String {
    format!("{:016x}.bin", cache_key(url, width, height, mask))
}

/// Removes every cached thumbnail file. Missing directories are not an error.
pub fn clear_cache() -> Result<()> {
    let root = cache_root().context("No cache directory is available")?;
    if !root.exists() {
        return Ok(());
    }
    let _guard = CACHE_WRITE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for entry in
        std::fs::read_dir(&root).with_context(|| format!("Could not read {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("Could not read {}", root.display()))?;
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            std::fs::remove_file(entry.path())
                .with_context(|| format!("Could not remove {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Returns the cached thumbnail for `url`, downloading and decoding it on a miss.
/// Cache failures never make a successful download fail.
pub fn fetch_cached(url: &str, width: u32, height: u32, mask: Mask) -> Result<Thumbnail> {
    #[cfg(windows)]
    let download = || fetch(url, width, height, mask);
    #[cfg(not(windows))]
    let download = || -> Result<Thumbnail> {
        anyhow::bail!("Artwork downloads are only supported on Windows")
    };
    fetch_cached_in(cache_root().as_deref(), url, width, height, mask, download)
}

fn fetch_cached_in(
    root: Option<&Path>,
    url: &str,
    width: u32,
    height: u32,
    mask: Mask,
    download: impl FnOnce() -> Result<Thumbnail>,
) -> Result<Thumbnail> {
    let path = root.map(|root| root.join(cache_file_name(url, width, height, mask)));
    if let Some(path) = path.as_deref()
        && let Some(cached) = read_cache_file(path, width, height)
    {
        return Ok(cached);
    }
    let thumbnail = with_limit(download)?;
    if let Some(path) = path.as_deref() {
        // A read-only or full cache directory must not discard a good thumbnail.
        let _ = write_cache_file(path, &thumbnail);
    }
    Ok(thumbnail)
}

fn read_cache_file(path: &Path, width: u32, height: u32) -> Option<Thumbnail> {
    parse_cache(&std::fs::read(path).ok()?, width, height)
}

/// Returns `None` for anything that is not a complete, matching cache file.
fn parse_cache(bytes: &[u8], width: u32, height: u32) -> Option<Thumbnail> {
    if bytes.len() < CACHE_HEADER || &bytes[..CACHE_MAGIC.len()] != CACHE_MAGIC {
        return None;
    }
    let stored_width = u32::from_le_bytes(bytes[7..11].try_into().ok()?);
    let stored_height = u32::from_le_bytes(bytes[11..15].try_into().ok()?);
    if stored_width != width || stored_height != height || width == 0 || height == 0 {
        return None;
    }
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    let bgra = &bytes[CACHE_HEADER..];
    if bgra.len() != expected {
        return None;
    }
    Some(Thumbnail {
        width,
        height,
        bgra: bgra.to_vec(),
    })
}

fn encode_cache(thumbnail: &Thumbnail) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CACHE_HEADER + thumbnail.bgra.len());
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&thumbnail.width.to_le_bytes());
    bytes.extend_from_slice(&thumbnail.height.to_le_bytes());
    bytes.extend_from_slice(&thumbnail.bgra);
    bytes
}

/// Writes via a sibling `.tmp` file and renames it into place. Serialised so
/// concurrent requests for the same URL cannot collide on the temporary name.
fn write_cache_file(path: &Path, thumbnail: &Thumbnail) -> std::io::Result<()> {
    let expected = (thumbnail.width as usize)
        .checked_mul(thumbnail.height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(thumbnail.bgra.len()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "thumbnail buffer size does not match its dimensions",
        ));
    }
    let directory = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cache file has no parent directory",
        )
    })?;
    let _guard = CACHE_WRITE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    std::fs::create_dir_all(directory)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = std::fs::write(&temporary, encode_cache(thumbnail))
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written?;
    prune_cache(directory, CACHE_MAX_FILES, CACHE_MAX_BYTES);
    Ok(())
}

/// Deletes the least recently modified cache files until both caps hold.
fn prune_cache(root: &Path, max_files: usize, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut files = Vec::new();
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(extension) = path.extension() else {
            continue;
        };
        if extension != std::ffi::OsStr::new("bin") && extension != std::ffi::OsStr::new("tmp") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        total = total.saturating_add(metadata.len());
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((path, metadata.len(), modified));
    }
    if files.len() <= max_files && total <= max_bytes {
        return;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut remaining = files.len();
    for (path, size, _) in files {
        if remaining <= max_files && total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            remaining -= 1;
            total = total.saturating_sub(size);
        }
    }
}

// --- Bounded downloads ----------------------------------------------------------

/// Process-wide cap on simultaneous downloads (the Images page can ask for 27).
const DOWNLOAD_LIMIT: usize = 3;

struct Semaphore {
    used: Mutex<usize>,
    released: Condvar,
    limit: usize,
}

impl Semaphore {
    const fn new(limit: usize) -> Self {
        Self {
            used: Mutex::new(0),
            released: Condvar::new(),
            limit,
        }
    }

    fn acquire(&self) -> Permit<'_> {
        let mut used = self.used.lock().unwrap_or_else(|error| error.into_inner());
        while *used >= self.limit {
            used = self
                .released
                .wait(used)
                .unwrap_or_else(|error| error.into_inner());
        }
        *used += 1;
        Permit { semaphore: self }
    }

    fn release(&self) {
        let mut used = self.used.lock().unwrap_or_else(|error| error.into_inner());
        *used = used.saturating_sub(1);
        self.released.notify_one();
    }
}

/// Releases its slot even when `f` panics.
struct Permit<'a> {
    semaphore: &'a Semaphore,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.semaphore.release();
    }
}

static DOWNLOADS: Semaphore = Semaphore::new(DOWNLOAD_LIMIT);
static CACHE_WRITE: Mutex<()> = Mutex::new(());

/// Runs `f` while holding one of the three process-wide download slots.
pub fn with_limit<T>(f: impl FnOnce() -> T) -> T {
    let _permit = DOWNLOADS.acquire();
    f()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, MutexGuard};
    use std::time::Duration;

    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn prepare_upload_downscales_large_images_to_png() {
        let big = png(1024, 512, [10, 20, 30, 128]);
        let prepared = prepare_upload(&big).unwrap();
        let decoded = ImageReader::new(Cursor::new(&prepared))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 256));
        let small = png(64, 32, [10, 20, 30, 255]);
        let kept = prepare_upload(&small).unwrap();
        let decoded_small = ImageReader::new(Cursor::new(&kept))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded_small.width(), decoded_small.height()), (64, 32));
        assert!(prepare_upload(b"not an image").is_err());
    }

    #[test]
    fn decodes_scales_and_premultiplies() {
        let thumb = decode(&png(40, 20, [255, 0, 0, 255]), 10, 10, Mask::Square).unwrap();
        assert_eq!((thumb.width, thumb.height, thumb.bgra.len()), (10, 10, 400));
        assert_eq!(&thumb.bgra[..4], &[0, 0, 255, 255]);
        let half = decode(&png(4, 4, [200, 100, 50, 128]), 2, 2, Mask::Square).unwrap();
        assert_eq!(&half.bgra[..4], &[25, 50, 100, 128]);
    }

    #[test]
    fn masks_clear_corners_but_keep_centres() {
        let source = png(32, 32, [255, 255, 255, 255]);
        let alpha = |t: &Thumbnail, x: u32, y: u32| t.bgra[((y * t.width + x) * 4 + 3) as usize];
        let circle = decode(&source, 32, 32, Mask::Circle).unwrap();
        assert_eq!((alpha(&circle, 0, 0), alpha(&circle, 16, 16)), (0, 255));
        let rounded = decode(&source, 32, 32, Mask::Rounded(8)).unwrap();
        assert_eq!((alpha(&rounded, 0, 0), alpha(&rounded, 16, 0)), (0, 255));
    }

    #[test]
    fn rejects_junk_and_silly_sizes() {
        assert!(decode(b"not an image", 10, 10, Mask::Square).is_err());
        assert!(decode(&png(2, 2, [0; 4]), 0, 10, Mask::Square).is_err());
    }

    /// Talks to the wiki: `cargo test --lib live_wiki -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_wiki_webp_and_png_decode() {
        let ship = fetch(
            "https://media.starcitizen.tools/thumb/6/6c/Perseus_angled_combat.jpg/480px-Perseus_angled_combat.jpg.webp",
            96,
            96,
            Mask::Rounded(10),
        )
        .unwrap();
        assert_eq!(ship.bgra.len(), 96 * 96 * 4);
        let badge = fetch(
            "https://media.starcitizen.tools/a/ad/Mercenary_Guild_logo_RepUI.png",
            28,
            28,
            Mask::Circle,
        )
        .unwrap();
        assert_eq!(badge.width, 28);
    }

    fn sample(width: u32, height: u32, seed: u8) -> Thumbnail {
        Thumbnail {
            width,
            height,
            bgra: (0..width as usize * height as usize * 4)
                .map(|index| (index as u8).wrapping_add(seed))
                .collect(),
        }
    }

    /// Serialises tests that mutate `SCRP_CACHE_DIR` and restores it on unwind.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct CacheEnv {
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl CacheEnv {
        fn set(directory: &Path) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
            let previous = std::env::var_os(CACHE_ENV);
            // SAFETY: ENV_LOCK is held for this guard's whole lifetime, so no
            // other test reads or writes the process environment concurrently.
            unsafe { std::env::set_var(CACHE_ENV, directory) };
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for CacheEnv {
        fn drop(&mut self) {
            // SAFETY: `_lock` is still held while Drop runs.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(CACHE_ENV, value),
                    None => std::env::remove_var(CACHE_ENV),
                }
            }
        }
    }

    #[test]
    fn cache_file_round_trips_pixels_and_header() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("round-trip.bin");
        let thumbnail = sample(5, 3, 7);
        write_cache_file(&path, &thumbnail).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..7], b"SCRPT1\0");
        assert_eq!(u32::from_le_bytes(bytes[7..11].try_into().unwrap()), 5);
        assert_eq!(u32::from_le_bytes(bytes[11..15].try_into().unwrap()), 3);
        assert_eq!(bytes.len(), CACHE_HEADER + thumbnail.bgra.len());
        assert_eq!(read_cache_file(&path, 5, 3), Some(thumbnail));
        assert_eq!(read_cache_file(&path, 4, 3), None);
        assert_eq!(read_cache_file(&path, 5, 4), None);
    }

    #[test]
    fn corrupt_cache_is_ignored_and_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let url = "https://example.invalid/art.png";
        let path = root.join(cache_file_name(url, 8, 6, Mask::Rounded(3)));
        std::fs::create_dir_all(root).unwrap();

        let truncated = [&b"SCRPT1\0\x08\0\0\0\x06\0\0\0"[..], b"oops"].concat();
        std::fs::write(&path, &truncated).unwrap();
        assert_eq!(read_cache_file(&path, 8, 6), None);

        let calls = Cell::new(0);
        let fresh = sample(8, 6, 9);
        let fetched = fetch_cached_in(Some(root), url, 8, 6, Mask::Rounded(3), || {
            calls.set(calls.get() + 1);
            Ok(fresh.clone())
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(fetched, fresh);
        assert_eq!(read_cache_file(&path, 8, 6), Some(fresh.clone()));

        // A valid file is now used without touching the downloader.
        let again = fetch_cached_in(Some(root), url, 8, 6, Mask::Rounded(3), || {
            calls.set(calls.get() + 1);
            Ok(sample(8, 6, 1))
        })
        .unwrap();
        assert_eq!(again, fresh);
        assert_eq!(calls.get(), 1);

        // Bad magic is just as unusable as a truncated payload.
        std::fs::write(&path, b"NOTMAGIC").unwrap();
        let repaired = fetch_cached_in(Some(root), url, 8, 6, Mask::Rounded(3), || {
            calls.set(calls.get() + 1);
            Ok(sample(8, 6, 2))
        })
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(read_cache_file(&path, 8, 6), Some(repaired));
    }

    #[test]
    fn cache_keys_depend_on_size_and_mask() {
        let base = cache_key("https://example.invalid/art.png", 96, 96, Mask::Square);
        assert_ne!(
            base,
            cache_key("https://example.invalid/art.png", 78, 78, Mask::Square)
        );
        assert_ne!(
            base,
            cache_key("https://example.invalid/art.png", 96, 78, Mask::Square)
        );
        assert_ne!(
            base,
            cache_key("https://example.invalid/art.png", 78, 96, Mask::Square)
        );
        assert_ne!(
            base,
            cache_key("https://example.invalid/art.png", 96, 96, Mask::Circle)
        );
        assert_ne!(
            base,
            cache_key("https://example.invalid/art.png", 96, 96, Mask::Rounded(6))
        );
        assert_ne!(
            cache_key("https://example.invalid/art.png", 96, 96, Mask::Rounded(6)),
            cache_key("https://example.invalid/art.png", 96, 96, Mask::Rounded(7))
        );
        assert_ne!(
            base,
            cache_key("https://example.invalid/other.png", 96, 96, Mask::Square)
        );
        assert_eq!(
            cache_file_name("https://example.invalid/art.png", 96, 96, Mask::Square),
            format!("{base:016x}.bin")
        );
    }

    #[test]
    fn clear_cache_empties_the_directory() {
        let directory = tempfile::tempdir().unwrap();
        let _env = CacheEnv::set(directory.path());
        std::fs::write(directory.path().join("one.bin"), b"x").unwrap();
        std::fs::write(directory.path().join("two.bin"), b"y").unwrap();
        std::fs::write(directory.path().join("leftover.tmp"), b"z").unwrap();
        clear_cache().unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        clear_cache().unwrap();
    }

    #[test]
    fn cache_root_honours_the_env_override() {
        let directory = tempfile::tempdir().unwrap();
        let _env = CacheEnv::set(directory.path());
        assert_eq!(cache_root().as_deref(), Some(directory.path()));
    }

    #[test]
    fn pruning_keeps_the_cache_under_both_caps() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..5 {
            std::fs::write(
                directory.path().join(format!("{index:016x}.bin")),
                vec![0u8; 10],
            )
            .unwrap();
        }
        prune_cache(directory.path(), 2, u64::MAX);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);

        for index in 0..5 {
            std::fs::write(
                directory.path().join(format!("{index:016x}.bin")),
                vec![0u8; 10],
            )
            .unwrap();
        }
        prune_cache(directory.path(), 100, 25);
        let entries: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .collect();
        assert!(entries.iter().sum::<u64>() <= 25);
    }

    #[test]
    fn download_limit_never_exceeds_three() {
        const THREADS: usize = 12;
        static CURRENT: AtomicUsize = AtomicUsize::new(0);
        static HIGH_WATER: AtomicUsize = AtomicUsize::new(0);

        let barrier = Arc::new(Barrier::new(THREADS));
        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                with_limit(|| {
                    let now = CURRENT.fetch_add(1, Ordering::SeqCst) + 1;
                    HIGH_WATER.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    CURRENT.fetch_sub(1, Ordering::SeqCst);
                });
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            HIGH_WATER.load(Ordering::SeqCst),
            DOWNLOAD_LIMIT,
            "the limiter should saturate without ever exceeding its cap"
        );
    }
}
