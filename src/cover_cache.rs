//! EPUB Library cover thumbnails: extraction, scaled decode, Floyd-Steinberg
//! dithering to 1bpp, and an SD-backed cache keyed by source size + mtime.
//!
//! # Architecture
//!
//! - **Extraction** (`epub::extract_cover`, in [`crate::epub`]): locates the
//!   manifest cover item and returns raw image bytes. Reused as-is from the
//!   existing EPUB parser rather than duplicated here.
//! - **Decode / resize / dither** (this module, private functions below
//!   [`decode_and_dither_cover`]): JPEG uses `jpeg-decoder`'s native scaled
//!   decoding (`Decoder::scale`, factors 1/8..1) to decode near the target
//!   size instead of at full resolution; PNG has no such native scaling and
//!   always decodes at full resolution, then both paths share one software
//!   box-filter resize down to the exact thumbnail size and one Floyd-Steinberg
//!   pass to 1bpp. Extraction and decode both run together on one dedicated
//!   worker stack (see [`generate_thumbnail`]) — JPEG/PNG decoding is real
//!   working-set-heavy code, not something to risk on the 16 KB firmware main
//!   task, same reasoning [`crate::epub`] already applies to EPUB parsing.
//! - **Cache management** ([`CoverCache`]): a tiny binary header (magic,
//!   dimensions, a size+mtime+format fingerprint) followed by the packed 1bpp
//!   bitmap, one file per book under the cache directory, ready to blit
//!   without re-decoding JPEG/PNG on a cache hit.
//! - **Scheduling** ([`CoverCache::pump_pending`]): no dedicated background
//!   thread. This firmware's main loop is single-threaded and polls buttons
//!   every iteration, so thumbnail generation is amortized instead: call
//!   `pump_pending` once per main-loop iteration while the Library screen is
//!   on-panel, passing only the entries visible on the current page. Each call
//!   builds at most one thumbnail (~20-40ms budget) and returns the book it
//!   refreshed, so the caller can issue a partial refresh of just that row —
//!   but the caller must throttle *that* refresh itself (a real e-paper panel
//!   update takes far longer than the thumbnail build and must not run once
//!   per generated thumbnail back-to-back). This also sidesteps introducing
//!   the project's first concurrent SD access: generation only ever runs
//!   synchronously from the main task's call, interleaved with (never
//!   concurrent with) any other SD read.
//!
//! Cover extraction or decode failures (missing cover, corrupt image,
//! unsupported format) fall back to a placeholder bitmap, which is itself
//! cached with a `placeholder` flag so a book without a usable cover is not
//! retried on every Library visit.

use std::{
    fs,
    io::{self},
    path::{Path, PathBuf},
};

use crate::reader::{BookFormat, ReaderBook};

/// Thumbnail width in pixels. A multiple of 8 so packed rows need no padding.
/// Sized to fill the two-column Library grid cell edge-to-edge (~210px wide
/// on the 480 x 800 portrait UI, minus a 1px pad each side so the cell
/// border doesn't overlap the image) — the Library screen shows cover art
/// only, no title text, so the cover itself needs to be legible at a glance
/// and the tile should have no dead space between the border and the art.
pub const THUMB_WIDTH: u16 = 208;
/// Thumbnail height in pixels. Fills the Library grid cell's fixed row
/// height (256px, minus a 2px pad top and bottom) the same way `THUMB_WIDTH`
/// fills its column width.
pub const THUMB_HEIGHT: u16 = 252;
const THUMB_ROW_BYTES: usize = THUMB_WIDTH as usize / 8;
const THUMB_BITMAP_BYTES: usize = THUMB_ROW_BYTES * THUMB_HEIGHT as usize;
/// Stack budget for the worker that extracts + decodes + resizes + dithers
/// one cover. Matches [`crate::epub::EPUB_PARSER_WORKER_STACK_BYTES`]: JPEG/
/// PNG decoding is comparable in stack depth to EPUB's own DEFLATE+XHTML
/// work, and both are kept off the 16 KB main task for the same reason.
const COVER_WORKER_STACK_BYTES: usize = 64 * 1024;

/// Default cache root. Callers that already own a per-book cache directory
/// (Reader's `.EPX`/`.EPP`/`.CCH` sidecars live under `<state_root>/CACHE`)
/// should pass that same directory to [`CoverCache::new`] instead, so
/// thumbnail files (`.THB`) sit alongside them.
pub const DEFAULT_COVER_CACHE_DIRECTORY: &str = "/sdcard/RUSTMIX/CACHE";

const CACHE_MAGIC: [u8; 4] = *b"RWTH";
const CACHE_VERSION: u8 = 1;
const CACHE_HEADER_BYTES: usize = 4 + 1 + 1 + 2 + 2 + 8; // magic+version+flags+w+h+fingerprint
const FLAG_PLACEHOLDER: u8 = 0x01;
const CACHE_FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const CACHE_FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
/// Bumped whenever the on-disk format, thumbnail size, or dithering changes
/// in a way that must invalidate every existing cache entry.
const COVER_CACHE_FORMAT_VERSION: &str = "1";

/// One decoded 1bpp thumbnail, packed MSB-first, bit `1` = ink (black) —
/// directly usable as the byte slice backing an
/// `embedded_graphics::image::ImageRaw<BinaryColor>` for blitting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedThumbnail {
    pub width: u16,
    pub height: u16,
    pub bits: Vec<u8>,
    /// `true` when the book had no usable cover (missing, corrupt, or an
    /// unsupported format) and this is the generic fallback glyph.
    pub placeholder: bool,
}

/// Read-only-by-default, write-through SD-backed thumbnail cache. Stateless
/// beyond the root directory: every method recomputes the current
/// size+mtime+format fingerprint from the `ReaderBook` passed in, so a
/// replaced or resized book on SD is detected without any explicit
/// invalidation call.
#[derive(Clone, Debug)]
pub struct CoverCache {
    root: PathBuf,
}

impl Default for CoverCache {
    fn default() -> Self {
        Self::new(DEFAULT_COVER_CACHE_DIRECTORY)
    }
}

impl CoverCache {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn cache_path(&self, book: &ReaderBook) -> PathBuf {
        self.root
            .join(format!("{:08X}.THB", cover_fingerprint(book) as u32))
    }

    /// Read a cached thumbnail iff it exists and matches the book's current
    /// size+mtime+format. Any I/O error, truncation, or fingerprint mismatch
    /// is treated as a plain cache miss (`None`) — never an error the caller
    /// has to handle, since a miss just means "(re)generate it".
    #[must_use]
    pub fn load_cached_thumbnail(&self, book: &ReaderBook) -> Option<CachedThumbnail> {
        let bytes = fs::read(self.cache_path(book)).ok()?;
        parse_cache_bytes(&bytes, book)
    }

    /// Convenience predicate: `true` when [`load_cached_thumbnail`] would
    /// return `None`, i.e. this book needs (re)generation.
    #[must_use]
    pub fn invalidate_if_stale(&self, book: &ReaderBook) -> bool {
        self.load_cached_thumbnail(book).is_none()
    }

    /// Build (or rebuild) one book's thumbnail and persist it to SD,
    /// overwriting any existing entry. Bounded to ~20-40ms per call on target
    /// hardware (scaled JPEG decode, or full-resolution PNG decode, plus one
    /// small box-filter resize, one Floyd-Steinberg pass over a
    /// `THUMB_WIDTH x THUMB_HEIGHT` buffer, and one SD write) — safe to call
    /// synchronously for a handful of books, and the unit callers of
    /// [`pump_pending`] rely on this bound to keep button polling reactive.
    pub fn generate_thumbnail(&self, book: &ReaderBook) -> CachedThumbnail {
        let cache_path = self.cache_path(book);
        let worker_book = book.clone();
        let result = crate::runtime_worker::run_named_worker(
            "cover-thumbnail",
            COVER_WORKER_STACK_BYTES,
            move || -> Result<CachedThumbnail, String> {
                let thumbnail = build_thumbnail(&worker_book);
                let bytes = write_cache_bytes(&worker_book, &thumbnail);
                if let Err(error) = atomic_write(&cache_path, &bytes) {
                    log::warn!(
                        "rustmix-wave=cover-cache status=write-failed path={} error={error}",
                        worker_book.path
                    );
                }
                Ok(thumbnail)
            },
        );
        result.unwrap_or_else(|error| {
            log::warn!(
                "rustmix-wave=cover-cache status=worker-failed path={} error={error}",
                book.path
            );
            placeholder_bitmap()
        })
    }

    /// Amortized generation step: builds at most one missing/stale thumbnail
    /// among `visible` (already narrowed by the caller to the Library
    /// screen's current page — lazy generation never runs ahead of what is
    /// actually on-panel) and returns the book plus its freshly generated
    /// thumbnail, so the caller can insert it directly into its render-side
    /// thumbnail cache without a redundant SD read. Call once per main-loop
    /// iteration while the Library screen is active; `None` means every
    /// visible entry already has a fresh cache hit this tick.
    #[must_use]
    pub fn pump_pending(&self, visible: &[ReaderBook]) -> Option<(ReaderBook, CachedThumbnail)> {
        let book = visible.iter().find(|book| self.invalidate_if_stale(book))?;
        let thumbnail = self.generate_thumbnail(book);
        Some((book.clone(), thumbnail))
    }
}

/// Runs on the dedicated worker spawned by [`CoverCache::generate_thumbnail`]
/// — never call this directly from the main task, it does real ZIP/DEFLATE
/// extraction and JPEG/PNG decode work.
fn build_thumbnail(book: &ReaderBook) -> CachedThumbnail {
    if book.format != BookFormat::Epub {
        return placeholder_bitmap();
    }
    match crate::epub::extract_cover(&book.path) {
        Ok(Some(cover)) => decode_and_dither_cover(&cover.bytes, &cover.media_type)
            .unwrap_or_else(|error| {
                log::warn!(
                    "rustmix-wave=cover-cache status=decode-failed path={} error={error}",
                    book.path
                );
                placeholder_bitmap()
            }),
        Ok(None) => placeholder_bitmap(),
        Err(error) => {
            log::warn!(
                "rustmix-wave=cover-cache status=extract-failed path={} error={error}",
                book.path
            );
            placeholder_bitmap()
        }
    }
}

// --- decode / resize / dither -------------------------------------------

/// 8-bit grayscale image, row-major, no padding.
struct GrayImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn decode_and_dither_cover(bytes: &[u8], media_type: &str) -> Result<CachedThumbnail, String> {
    let gray = if is_jpeg(bytes) {
        decode_jpeg_scaled(bytes)?
    } else if is_png(bytes) {
        decode_png_full(bytes)?
    } else if media_type.eq_ignore_ascii_case("image/jpeg") {
        decode_jpeg_scaled(bytes)?
    } else if media_type.eq_ignore_ascii_case("image/png") {
        decode_png_full(bytes)?
    } else {
        return Err(format!("unsupported cover media type: {media_type}"));
    };
    let resized = resize_area_average(&gray, u32::from(THUMB_WIDTH), u32::from(THUMB_HEIGHT));
    let bits = floyd_steinberg_to_1bpp(resized);
    Ok(CachedThumbnail {
        width: THUMB_WIDTH,
        height: THUMB_HEIGHT,
        bits,
        placeholder: false,
    })
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
}

/// Decode via `jpeg-decoder`'s native scaled IDCT (factors 1/8, 1/4, 1/2, 1):
/// the decoder picks the smallest factor that still covers the requested
/// thumbnail size in at least one axis, so a large embedded cover is decoded
/// near the target resolution instead of at full size before being thrown
/// away by resize.
fn decode_jpeg_scaled(bytes: &[u8]) -> Result<GrayImage, String> {
    let mut decoder = jpeg_decoder::Decoder::new(io::Cursor::new(bytes));
    decoder
        .scale(THUMB_WIDTH, THUMB_HEIGHT)
        .map_err(|error| format!("JPEG scale failed: {error}"))?;
    let pixels = decoder
        .decode()
        .map_err(|error| format!("JPEG decode failed: {error}"))?;
    let info = decoder
        .info()
        .ok_or_else(|| "JPEG decode produced no image metadata".to_string())?;
    if info.width == 0 || info.height == 0 {
        return Err("JPEG cover has zero dimensions".into());
    }
    let pixels = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => pixels,
        jpeg_decoder::PixelFormat::L16 => pixels.chunks_exact(2).map(|pixel| pixel[0]).collect(),
        jpeg_decoder::PixelFormat::RGB24 => rgb_to_gray(&pixels),
        jpeg_decoder::PixelFormat::CMYK32 => cmyk_to_gray(&pixels),
    };
    Ok(GrayImage {
        width: u32::from(info.width),
        height: u32::from(info.height),
        pixels,
    })
}

/// Decode via the `png` crate. PNG has no native scaled decoding, so this
/// always decodes at full resolution; the caller's resize step then does the
/// same work the JPEG path's scaled decode avoided up front. Covers are
/// bounded by [`crate::epub::EPUB_COVER_BYTES_LIMIT`] to keep this bounded on
/// PSRAM-limited hardware.
fn decode_png_full(bytes: &[u8]) -> Result<GrayImage, String> {
    let mut decoder = png::Decoder::new(io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("PNG header decode failed: {error}"))?;
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| format!("PNG frame decode failed: {error}"))?;
    if info.width == 0 || info.height == 0 {
        return Err("PNG cover has zero dimensions".into());
    }
    let pixel_count = (info.width * info.height) as usize;
    let pixels = match info.color_type {
        png::ColorType::Grayscale => buffer[..pixel_count].to_vec(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .take(pixel_count)
            .map(|pixel| pixel[0])
            .collect(),
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .take(pixel_count)
            .map(|pixel| luma(pixel[0], pixel[1], pixel[2]))
            .collect(),
        png::ColorType::Rgba => buffer
            .chunks_exact(4)
            .take(pixel_count)
            .map(|pixel| luma(pixel[0], pixel[1], pixel[2]))
            .collect(),
        png::ColorType::Indexed => {
            return Err("PNG cover used indexed color after normalize_to_color8".into())
        }
    };
    Ok(GrayImage {
        width: info.width,
        height: info.height,
        pixels,
    })
}

fn rgb_to_gray(pixels: &[u8]) -> Vec<u8> {
    pixels
        .chunks_exact(3)
        .map(|pixel| luma(pixel[0], pixel[1], pixel[2]))
        .collect()
}

/// Approximate CMYK -> grayscale for the rare Adobe-CMYK JPEG cover: not
/// colorimetrically exact, but this only feeds a 1bpp thumbnail, so exact
/// color reproduction was never the goal.
fn cmyk_to_gray(pixels: &[u8]) -> Vec<u8> {
    pixels
        .chunks_exact(4)
        .map(|pixel| {
            let (c, m, y, k) = (
                u32::from(pixel[0]),
                u32::from(pixel[1]),
                u32::from(pixel[2]),
                u32::from(pixel[3]),
            );
            let ink = ((c + m + y) / 3 + k).min(255);
            (255 - ink) as u8
        })
        .collect()
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8
}

/// Box-filter downscale (or nearest-neighbor upscale for a rare tiny source
/// cover) to the exact target dimensions. Cheap enough at thumbnail size to
/// run unconditionally rather than special-casing "already the right size".
fn resize_area_average(src: &GrayImage, target_width: u32, target_height: u32) -> GrayImage {
    let mut pixels = vec![0u8; (target_width * target_height) as usize];
    for target_y in 0..target_height {
        let source_y0 = target_y * src.height / target_height;
        let source_y1 = ((target_y + 1) * src.height / target_height)
            .max(source_y0 + 1)
            .min(src.height);
        for target_x in 0..target_width {
            let source_x0 = target_x * src.width / target_width;
            let source_x1 = ((target_x + 1) * src.width / target_width)
                .max(source_x0 + 1)
                .min(src.width);
            let mut sum: u32 = 0;
            let mut count: u32 = 0;
            for source_y in source_y0..source_y1 {
                let row_start = (source_y * src.width) as usize;
                for source_x in source_x0..source_x1 {
                    sum += u32::from(src.pixels[row_start + source_x as usize]);
                    count += 1;
                }
            }
            let index = (target_y * target_width + target_x) as usize;
            pixels[index] = if count > 0 { (sum / count) as u8 } else { 255 };
        }
    }
    GrayImage {
        width: target_width,
        height: target_height,
        pixels,
    }
}

/// Floyd-Steinberg error-diffusion dither straight to packed 1bpp. Runs
/// in-place over the already-tiny (`THUMB_WIDTH x THUMB_HEIGHT` grayscale,
/// one byte per pixel) resized buffer — no full-resolution buffer is ever
/// kept around this point, it was dropped when `resize_area_average`
/// returned.
fn floyd_steinberg_to_1bpp(mut gray: GrayImage) -> Vec<u8> {
    let width = gray.width as usize;
    let height = gray.height as usize;
    let row_bytes = width.div_ceil(8);
    let mut bits = vec![0u8; row_bytes * height];
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let old_value = i32::from(gray.pixels[index]);
            let ink = old_value < 128;
            if ink {
                bits[y * row_bytes + x / 8] |= 0x80 >> (x % 8);
            }
            let error = old_value - if ink { 0 } else { 255 };
            if x + 1 < width {
                diffuse(&mut gray.pixels, index + 1, error * 7 / 16);
            }
            if y + 1 < height {
                if x > 0 {
                    diffuse(&mut gray.pixels, index + width - 1, error * 3 / 16);
                }
                diffuse(&mut gray.pixels, index + width, error * 5 / 16);
                if x + 1 < width {
                    diffuse(&mut gray.pixels, index + width + 1, error * 1 / 16);
                }
            }
        }
    }
    bits
}

fn diffuse(pixels: &mut [u8], index: usize, error: i32) {
    pixels[index] = (i32::from(pixels[index]) + error).clamp(0, 255) as u8;
}

/// Generic placeholder shown for missing/corrupt covers and non-EPUB books:
/// a bordered "closed book" glyph (outer frame, a spine line, a few page
/// ridges), deterministic and allocation-cheap enough to build inline instead
/// of embedding a bitmap asset.
fn placeholder_bitmap() -> CachedThumbnail {
    let mut bits = vec![0u8; THUMB_BITMAP_BYTES];
    for y in 0..THUMB_HEIGHT {
        for x in 0..THUMB_WIDTH {
            if placeholder_pixel_is_ink(x, y) {
                bits[y as usize * THUMB_ROW_BYTES + x as usize / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    CachedThumbnail {
        width: THUMB_WIDTH,
        height: THUMB_HEIGHT,
        bits,
        placeholder: true,
    }
}

fn placeholder_pixel_is_ink(x: u16, y: u16) -> bool {
    let border = x == 0 || y == 0 || x == THUMB_WIDTH - 1 || y == THUMB_HEIGHT - 1;
    let spine = x == 6;
    let page_ridge = x > 6 && x < THUMB_WIDTH - 3 && y % 12 == 6;
    border || spine || page_ridge
}

// --- binary cache format --------------------------------------------------

fn cover_fingerprint(book: &ReaderBook) -> u64 {
    let mut hash = CACHE_FNV_OFFSET;
    fn feed(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(CACHE_FNV_PRIME);
        }
    }
    feed(&mut hash, book.path.as_bytes());
    feed(&mut hash, &book.size_bytes.to_le_bytes());
    feed(&mut hash, &book.modified_seconds.to_le_bytes());
    feed(&mut hash, book_format_marker(book.format).as_bytes());
    feed(&mut hash, &THUMB_WIDTH.to_le_bytes());
    feed(&mut hash, &THUMB_HEIGHT.to_le_bytes());
    feed(&mut hash, COVER_CACHE_FORMAT_VERSION.as_bytes());
    hash
}

/// Small local copy of `ReaderBook`'s format tag: `BookFormat::marker` exists
/// in [`crate::reader`] but is private to that module.
fn book_format_marker(format: BookFormat) -> &'static str {
    match format {
        BookFormat::Text => "txt",
        BookFormat::Epub => "epub",
    }
}

fn write_cache_bytes(book: &ReaderBook, thumbnail: &CachedThumbnail) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CACHE_HEADER_BYTES + thumbnail.bits.len());
    bytes.extend_from_slice(&CACHE_MAGIC);
    bytes.push(CACHE_VERSION);
    bytes.push(if thumbnail.placeholder {
        FLAG_PLACEHOLDER
    } else {
        0
    });
    bytes.extend_from_slice(&thumbnail.width.to_le_bytes());
    bytes.extend_from_slice(&thumbnail.height.to_le_bytes());
    bytes.extend_from_slice(&cover_fingerprint(book).to_le_bytes());
    bytes.extend_from_slice(&thumbnail.bits);
    bytes
}

fn parse_cache_bytes(bytes: &[u8], book: &ReaderBook) -> Option<CachedThumbnail> {
    if bytes.len() < CACHE_HEADER_BYTES || bytes[0..4] != CACHE_MAGIC || bytes[4] != CACHE_VERSION
    {
        return None;
    }
    let flags = bytes[5];
    let width = u16::from_le_bytes([bytes[6], bytes[7]]);
    let height = u16::from_le_bytes([bytes[8], bytes[9]]);
    let fingerprint = u64::from_le_bytes(bytes[10..18].try_into().ok()?);
    if width != THUMB_WIDTH || height != THUMB_HEIGHT || fingerprint != cover_fingerprint(book) {
        return None;
    }
    let payload = &bytes[CACHE_HEADER_BYTES..];
    let expected_len = THUMB_ROW_BYTES * height as usize;
    if payload.len() != expected_len {
        return None;
    }
    Some(CachedThumbnail {
        width,
        height,
        bits: payload.to_vec(),
        placeholder: flags & FLAG_PLACEHOLDER != 0,
    })
}

/// Write-temp-then-rename so a crash or power loss mid-write never leaves a
/// truncated `.THB` file that `parse_cache_bytes` would need to detect.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("TMP");
    fs::write(&tmp_path, bytes)?;
    fs::rename(&tmp_path, path)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        decode_and_dither_cover, floyd_steinberg_to_1bpp, parse_cache_bytes,
        placeholder_bitmap, resize_area_average, write_cache_bytes, CoverCache, GrayImage,
        CACHE_HEADER_BYTES, THUMB_BITMAP_BYTES, THUMB_HEIGHT, THUMB_WIDTH,
    };
    use crate::reader::{BookFormat, ReaderBook};

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustmix-cover-cache-{name}-{nonce}"))
    }

    fn sample_book(path: &str) -> ReaderBook {
        ReaderBook {
            path: path.to_string(),
            title: "Sample".into(),
            format: BookFormat::Epub,
            size_bytes: 1234,
            modified_seconds: 5678,
        }
    }

    #[test]
    fn placeholder_bitmap_has_expected_size_and_border() {
        let thumbnail = placeholder_bitmap();
        assert_eq!(thumbnail.width, THUMB_WIDTH);
        assert_eq!(thumbnail.height, THUMB_HEIGHT);
        assert_eq!(thumbnail.bits.len(), THUMB_BITMAP_BYTES);
        assert!(thumbnail.placeholder);
        // Top-left corner pixel (border) must be ink.
        assert_eq!(thumbnail.bits[0] & 0x80, 0x80);
    }

    #[test]
    fn dither_pure_white_produces_no_ink() {
        let gray = GrayImage {
            width: 4,
            height: 4,
            pixels: vec![255; 16],
        };
        let bits = floyd_steinberg_to_1bpp(gray);
        assert!(bits.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn dither_pure_black_produces_full_ink_rows() {
        let gray = GrayImage {
            width: 8,
            height: 2,
            pixels: vec![0; 16],
        };
        let bits = floyd_steinberg_to_1bpp(gray);
        assert_eq!(bits, vec![0xFF, 0xFF]);
    }

    #[test]
    fn resize_area_average_averages_a_two_by_two_block() {
        let src = GrayImage {
            width: 2,
            height: 2,
            pixels: vec![0, 100, 100, 200],
        };
        let resized = resize_area_average(&src, 1, 1);
        assert_eq!(resized.pixels, vec![100]);
    }

    #[test]
    fn decode_and_dither_round_trips_a_generated_png() {
        let png_bytes = encode_test_png();
        let thumbnail = decode_and_dither_cover(&png_bytes, "image/png").unwrap();
        assert_eq!(thumbnail.width, THUMB_WIDTH);
        assert_eq!(thumbnail.height, THUMB_HEIGHT);
        assert_eq!(thumbnail.bits.len(), THUMB_BITMAP_BYTES);
        assert!(!thumbnail.placeholder);
    }

    #[test]
    fn cache_round_trips_through_binary_header() {
        let book = sample_book("/sdcard/BOOKS/sample.epub");
        let thumbnail = placeholder_bitmap();
        let bytes = write_cache_bytes(&book, &thumbnail);
        assert_eq!(bytes.len(), CACHE_HEADER_BYTES + THUMB_BITMAP_BYTES);
        let parsed = parse_cache_bytes(&bytes, &book).unwrap();
        assert_eq!(parsed, thumbnail);
    }

    #[test]
    fn cache_rejects_stale_fingerprint_after_book_changes() {
        let book = sample_book("/sdcard/BOOKS/sample.epub");
        let bytes = write_cache_bytes(&book, &placeholder_bitmap());
        let mut changed = book.clone();
        changed.size_bytes += 1;
        assert!(parse_cache_bytes(&bytes, &changed).is_none());
    }

    #[test]
    fn cover_cache_generates_loads_and_invalidates() {
        let root = temp_dir("basic");
        let cache = CoverCache::new(&root);
        // Text books never carry a cover, so this exercises the placeholder
        // path without needing a real EPUB fixture on disk.
        let book = ReaderBook {
            format: BookFormat::Text,
            ..sample_book("/sdcard/BOOKS/notes.txt")
        };

        assert!(cache.invalidate_if_stale(&book));
        let generated = cache.generate_thumbnail(&book);
        assert!(generated.placeholder);
        assert!(!cache.invalidate_if_stale(&book));
        assert_eq!(cache.load_cached_thumbnail(&book), Some(generated));

        let mut replaced = book.clone();
        replaced.modified_seconds += 1;
        assert!(cache.invalidate_if_stale(&replaced));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pump_pending_generates_at_most_one_entry_per_call() {
        let root = temp_dir("pump");
        let cache = CoverCache::new(&root);
        let books = vec![
            ReaderBook {
                format: BookFormat::Text,
                ..sample_book("/sdcard/BOOKS/one.txt")
            },
            ReaderBook {
                format: BookFormat::Text,
                ..sample_book("/sdcard/BOOKS/two.txt")
            },
        ];

        let (first_book, first_thumbnail) = cache.pump_pending(&books).unwrap();
        assert_eq!(first_book.path, books[0].path);
        assert!(first_thumbnail.placeholder);
        assert!(!cache.invalidate_if_stale(&books[0]));
        assert!(cache.invalidate_if_stale(&books[1]));

        let (second_book, _) = cache.pump_pending(&books).unwrap();
        assert_eq!(second_book.path, books[1].path);
        assert!(cache.pump_pending(&books).is_none());

        let _ = fs::remove_dir_all(&root);
    }

    /// Encode a tiny 4x4 RGB PNG in-memory for the decode round-trip test —
    /// avoids checking a binary fixture into the repo for one test.
    fn encode_test_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 4, 4);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let pixels = vec![0u8; 4 * 4 * 3];
            writer.write_image_data(&pixels).unwrap();
        }
        bytes
    }
}
