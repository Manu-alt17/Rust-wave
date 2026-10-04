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
/// Most a PNG decode may allocate for its pixels, checked from the header
/// before allocating: the image already reduced for a plain PNG
/// ([`decode_png_reduced`]), the whole 8-bit frame for an interlaced one
/// ([`decode_png_full`]). Matched to the 8 MB of PSRAM this device works
/// within (`crate::runtime_worker::run_named_worker_in_psram`), on the
/// assumption that one image decode never runs alongside a book's full
/// flattened text (`crate::epub::EPUB_REFLOW_TEXT_LIMIT`).
const MAX_PNG_DECODED_BUFFER_BYTES: u64 = 8 * 1024 * 1024;

/// Widest PNG either decode accepts. The `png` crate keeps a few source rows
/// in memory, allocated where a failure aborts the firmware, before any
/// budget of ours is checked. No book illustration comes near this.
const MAX_PNG_WIDTH: u32 = 16_384;

/// Cap on [`png_reduction_factor`], so a block's sum stays within `u32`
/// (255 x 256 x 256).
const MAX_PNG_REDUCTION_FACTOR: u32 = 256;

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
/// in a way that must invalidate every existing cache entry. `"2"`: covers
/// centre-cropped instead of stretched. `"3"`: progressive JPEGs decoded
/// (by `jpeg_luma`) instead of left as placeholders. `"4"`: PNGs reduced
/// while read, transparency on white; the covers the full-frame decode
/// refused, or crashed on, get another try. `"5"`: Library and Home covers
/// stretched to the cell again instead of centre-cropped, so the whole
/// cover shows. The Wi-Fi portal computes the same fingerprint in the
/// browser (`coverFingerprint`), so the two must match.
const COVER_CACHE_FORMAT_VERSION: &str = "5";

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

/// Names, inside the shared cache directory, of the cover files kept for
/// `book`: its Library thumbnail and its full-screen sleep cover. For
/// whoever deletes the book, so they do not stay behind.
#[must_use]
pub fn cover_cache_file_names(book: &ReaderBook) -> [String; 2] {
    [
        format!("{:08X}.THB", cover_fingerprint(book) as u32),
        format!(
            "{:08X}.SLC",
            fullscreen_cover_fingerprint(
                book,
                crate::sleep_cover::SLEEP_COVER_WIDTH,
                crate::sleep_cover::SLEEP_COVER_HEIGHT
            ) as u32
        ),
    ]
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
        let _span = crate::boot_profile::span("cover-load-cached-thumbnail");
        let bytes = crate::sd_io::read_file(self.cache_path(book)).ok()?;
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
        let _span = crate::boot_profile::span("cover-generate-thumbnail");
        let cache_path = self.cache_path(book);
        let Some(_attempt) = DecodeAttempt::begin(&cache_path) else {
            log::warn!(
                "rustmix-wave=cover-cache status=skipped-after-crash path={}",
                book.path
            );
            let placeholder = placeholder_bitmap();
            let _ = atomic_write(&cache_path, &write_cache_bytes(book, &placeholder));
            return placeholder;
        };
        let worker_book = book.clone();
        let result = crate::runtime_worker::run_named_worker_in_psram(
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

    fn fullscreen_cache_path(&self, book: &ReaderBook, width: u16, height: u16) -> PathBuf {
        self.root.join(format!(
            "{:08X}.SLC",
            fullscreen_cover_fingerprint(book, width, height) as u32
        ))
    }

    /// Book cover decoded to exactly `width x height`, center-cropped to that
    /// aspect ratio rather than stretched, for the sleep screen. Served from
    /// an SD `.SLC` entry when one matches, else extracted and decoded on a
    /// PSRAM worker and cached. `None` for a non-EPUB book or one with no
    /// usable cover; that outcome is cached too so it is not retried on every
    /// sleep.
    #[must_use]
    pub fn load_or_generate_fullscreen_cover(
        &self,
        book: &ReaderBook,
        width: u16,
        height: u16,
    ) -> Option<CachedThumbnail> {
        let fingerprint = fullscreen_cover_fingerprint(book, width, height);
        let cache_path = self.fullscreen_cache_path(book, width, height);
        let cached = crate::sd_io::read_file(&cache_path)
            .ok()
            .and_then(|bytes| parse_inline_image_cache_bytes(&bytes, fingerprint))
            .filter(|cover| cover.width == width && cover.height == height);
        let cover = match cached {
            Some(cover) => cover,
            None => {
                let worker_book = book.clone();
                let attempt = DecodeAttempt::begin(&cache_path);
                let result = attempt.as_ref().map_or_else(
                    || Err(String::from("an earlier attempt crashed the device")),
                    |_| {
                        crate::runtime_worker::run_named_worker_in_psram(
                            "sleep-cover",
                            COVER_WORKER_STACK_BYTES,
                            move || -> Result<CachedThumbnail, String> {
                                if worker_book.format != BookFormat::Epub {
                                    return Err("not an EPUB".into());
                                }
                                let cover = crate::epub::extract_cover(&worker_book.path)?
                                    .ok_or_else(|| String::from("no cover in manifest"))?;
                                decode_and_dither_fill(
                                    &cover.bytes,
                                    &cover.media_type,
                                    u32::from(width),
                                    u32::from(height),
                                )
                            },
                        )
                        .map_err(|error| error.to_string())
                    },
                );
                drop(attempt);
                let cover = result.unwrap_or_else(|error| {
                    log::warn!(
                        "rustmix-wave=sleep-cover status=unavailable path={} error={error}",
                        book.path
                    );
                    blank_placeholder(width, height)
                });
                let bytes = write_inline_image_cache_bytes(fingerprint, &cover);
                if let Err(error) = atomic_write(&cache_path, &bytes) {
                    log::warn!(
                        "rustmix-wave=sleep-cover status=write-failed path={} error={error}",
                        book.path
                    );
                }
                cover
            }
        };
        (!cover.placeholder).then_some(cover)
    }
}

/// SD-backed, per-book/per-image/per-box-size bitmap cache for inline EPUB
/// images (`crate::epub::EpubImage`, via `.href`): the Reader's counterpart
/// to [`CoverCache`], covering everything downstream of pagination reserving
/// space for an image (`READER_INLINE_IMAGE_SLOT_SPAN` in reader.rs) --
/// extraction from the book's own ZIP archive, aspect-preserving decode/
/// dither into that reserved box, and an SD cache keyed so a font-size or
/// orientation change (a different box size) never reads back a
/// wrong-shaped bitmap instead of regenerating one, the same way
/// `CoverCache`'s own fingerprint already folds in `THUMB_WIDTH`/
/// `THUMB_HEIGHT`. Distinct `.EPI` extension keeps its cache files from ever
/// colliding with `.THB` cover thumbnails in the same cache root.
#[derive(Clone, Debug)]
pub struct EpubImageCache {
    root: PathBuf,
}

impl EpubImageCache {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 8.3 file name, like every other Reader cache (`.THB`, `.EPX`, `.EPP`,
    /// `.CCH`): the SD card's FAT volume has no long-file-name support, so a
    /// 16-hex-digit stem made every write fail with `EINVAL` and every draw
    /// of an image page re-extract and re-decode it from scratch. The full
    /// 64-bit fingerprint is still stored in and checked against the file's
    /// header, so a 32-bit name collision is only a cache miss.
    fn cache_path(
        &self,
        book: &ReaderBook,
        href: &str,
        max_width: u16,
        max_height: u16,
    ) -> PathBuf {
        self.root.join(format!(
            "{:08X}.EPI",
            inline_image_fingerprint(book, href, max_width, max_height) as u32
        ))
    }

    /// Read a cached inline-image bitmap iff it exists and matches this
    /// exact book+image+box-size fingerprint. Any I/O error, truncation, or
    /// fingerprint mismatch is a plain cache miss (`None`), same contract as
    /// [`CoverCache::load_cached_thumbnail`].
    #[must_use]
    pub fn load_cached_bitmap(
        &self,
        book: &ReaderBook,
        href: &str,
        max_width: u16,
        max_height: u16,
    ) -> Option<CachedThumbnail> {
        let fingerprint = inline_image_fingerprint(book, href, max_width, max_height);
        let bytes =
            crate::sd_io::read_file(self.cache_path(book, href, max_width, max_height)).ok()?;
        parse_inline_image_cache_bytes(&bytes, fingerprint)
    }

    /// `true` when [`load_cached_bitmap`] would return `None`, i.e. this
    /// image needs (re)generation.
    #[must_use]
    pub fn is_missing(
        &self,
        book: &ReaderBook,
        href: &str,
        max_width: u16,
        max_height: u16,
    ) -> bool {
        self.load_cached_bitmap(book, href, max_width, max_height)
            .is_none()
    }

    /// Extract + decode + dither one inline image and persist it to SD,
    /// overwriting any existing entry for this exact fingerprint. Real
    /// ZIP/DEFLATE and JPEG/PNG decode work, so -- exactly like
    /// [`CoverCache::generate_thumbnail`] -- this runs on a dedicated
    /// PSRAM-backed worker rather than the caller's own stack. A failure
    /// (corrupt image, unsupported format, oversized decode -- see
    /// `MAX_PNG_DECODED_BUFFER_BYTES`) is itself cached as a placeholder so
    /// it is not retried on every page visit, the same reasoning
    /// `CoverCache`'s own placeholder caching already documents. Callers
    /// must not call this from the render path; use [`Self::pump_pending`]
    /// to amortize it to one image per main-loop tick instead.
    pub fn generate_bitmap(
        &self,
        book: &ReaderBook,
        href: &str,
        max_width: u16,
        max_height: u16,
    ) -> CachedThumbnail {
        let fingerprint = inline_image_fingerprint(book, href, max_width, max_height);
        let cache_path = self.cache_path(book, href, max_width, max_height);
        let worker_book_path = book.path.clone();
        let worker_href = href.to_string();
        let worker_cache_path = cache_path.clone();
        let attempt = DecodeAttempt::begin(&cache_path);
        let result = attempt.as_ref().map_or_else(
            || Err(String::from("an earlier attempt crashed the device")),
            |_| crate::runtime_worker::run_named_worker_in_psram(
            "inline-image",
            COVER_WORKER_STACK_BYTES,
            move || -> Result<CachedThumbnail, String> {
                let started = std::time::Instant::now();
                let bytes = crate::epub::extract_member(&worker_book_path, &worker_href)?;
                let extract_ms = started.elapsed().as_millis();
                let media_type = guess_media_type_from_href(&worker_href);
                let mut timings = DecodeTimings::default();
                let thumbnail = decode_and_dither_fit_within_timed(
                    &bytes,
                    media_type,
                    u32::from(max_width),
                    u32::from(max_height),
                    &mut timings,
                )?;
                let write_started = std::time::Instant::now();
                let cache_bytes = write_inline_image_cache_bytes(fingerprint, &thumbnail);
                if let Err(error) = atomic_write(&worker_cache_path, &cache_bytes) {
                    log::warn!(
                        "rustmix-wave=inline-image-cache status=write-failed href={worker_href} error={error}"
                    );
                }
                log::info!(
                    "rustmix-wave=inline-image-timing href={worker_href} bytes={} source={}x{} decoded={}x{} target={}x{} extract-ms={extract_ms} decode-ms={} resize-ms={} dither-ms={} write-ms={} total-ms={}",
                    bytes.len(),
                    timings.source_width,
                    timings.source_height,
                    timings.decoded_width,
                    timings.decoded_height,
                    thumbnail.width,
                    thumbnail.height,
                    timings.decode_ms,
                    timings.resize_ms,
                    timings.dither_ms,
                    write_started.elapsed().as_millis(),
                    started.elapsed().as_millis()
                );
                Ok(thumbnail)
            },
            )
            .map_err(|error| error.to_string()),
        );
        drop(attempt);
        result.unwrap_or_else(|error| {
            log::warn!(
                "rustmix-wave=inline-image-cache status=decode-failed path={} href={href} error={error}",
                book.path
            );
            let placeholder = blank_placeholder(max_width, max_height);
            let cache_bytes = write_inline_image_cache_bytes(fingerprint, &placeholder);
            let _ = atomic_write(&cache_path, &cache_bytes);
            placeholder
        })
    }

    /// Amortized generation step, mirroring [`CoverCache::pump_pending`]:
    /// builds at most one missing/stale bitmap among `hrefs` (already
    /// narrowed by the caller to images referenced by lines on the current
    /// Reader page) and returns its href plus the freshly generated bitmap.
    /// Call once per main-loop iteration while the Reader screen is
    /// showing a page with at least one image; `None` means every
    /// currently-visible image already has a fresh cache hit this tick.
    #[must_use]
    pub fn pump_pending(
        &self,
        book: &ReaderBook,
        hrefs: &[String],
        max_width: u16,
        max_height: u16,
    ) -> Option<(String, CachedThumbnail)> {
        let href = hrefs
            .iter()
            .find(|href| self.is_missing(book, href, max_width, max_height))?;
        let thumbnail = self.generate_bitmap(book, href, max_width, max_height);
        Some((href.clone(), thumbnail))
    }
}

fn guess_media_type_from_href(href: &str) -> &'static str {
    let lower = href.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        ""
    }
}

/// All-white (no ink) bitmap at exactly `width x height`, used when an
/// inline image fails to extract or decode. Rendering draws its own bordered
/// placeholder box instead of blitting this, but a blank bitmap is a safe
/// default even if a caller ever blits it directly regardless of the
/// `placeholder` flag.
fn blank_placeholder(width: u16, height: u16) -> CachedThumbnail {
    let row_bytes = usize::from(width).div_ceil(8);
    CachedThumbnail {
        width,
        height,
        bits: vec![0u8; row_bytes * usize::from(height)],
        placeholder: true,
    }
}

fn inline_image_fingerprint(book: &ReaderBook, href: &str, max_width: u16, max_height: u16) -> u64 {
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
    feed(&mut hash, href.as_bytes());
    feed(&mut hash, &max_width.to_le_bytes());
    feed(&mut hash, &max_height.to_le_bytes());
    feed(&mut hash, INLINE_IMAGE_CACHE_FORMAT_VERSION.as_bytes());
    hash
}

/// Bumped whenever the on-disk `.EPI` format changes in a way that must
/// invalidate every existing cache entry -- mirrors
/// `COVER_CACHE_FORMAT_VERSION`. `"2"`: decoded by `esp_new_jpeg` on the
/// device, and regenerated once so its decode timing shows up in logs.
/// `"3"`: decoder-side downscale for any ratio, faster resize/dither.
/// `"4"`: progressive JPEGs decoded by `jpeg_luma`. `"5"`: PNGs reduced
/// while read, transparency on white.
const INLINE_IMAGE_CACHE_FORMAT_VERSION: &str = "5";
const INLINE_IMAGE_CACHE_MAGIC: [u8; 4] = *b"RWIM";
const INLINE_IMAGE_CACHE_VERSION: u8 = 1;
const INLINE_IMAGE_CACHE_HEADER_BYTES: usize = 4 + 1 + 1 + 2 + 2 + 8; // magic+version+flags+w+h+fingerprint
const INLINE_IMAGE_CACHE_FLAG_PLACEHOLDER: u8 = 0x01;

fn write_inline_image_cache_bytes(fingerprint: u64, thumbnail: &CachedThumbnail) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(INLINE_IMAGE_CACHE_HEADER_BYTES + thumbnail.bits.len());
    bytes.extend_from_slice(&INLINE_IMAGE_CACHE_MAGIC);
    bytes.push(INLINE_IMAGE_CACHE_VERSION);
    bytes.push(if thumbnail.placeholder {
        INLINE_IMAGE_CACHE_FLAG_PLACEHOLDER
    } else {
        0
    });
    bytes.extend_from_slice(&thumbnail.width.to_le_bytes());
    bytes.extend_from_slice(&thumbnail.height.to_le_bytes());
    bytes.extend_from_slice(&fingerprint.to_le_bytes());
    bytes.extend_from_slice(&thumbnail.bits);
    bytes
}

fn parse_inline_image_cache_bytes(
    bytes: &[u8],
    expected_fingerprint: u64,
) -> Option<CachedThumbnail> {
    if bytes.len() < INLINE_IMAGE_CACHE_HEADER_BYTES
        || bytes[0..4] != INLINE_IMAGE_CACHE_MAGIC
        || bytes[4] != INLINE_IMAGE_CACHE_VERSION
    {
        return None;
    }
    let flags = bytes[5];
    let width = u16::from_le_bytes([bytes[6], bytes[7]]);
    let height = u16::from_le_bytes([bytes[8], bytes[9]]);
    let fingerprint = u64::from_le_bytes(bytes[10..18].try_into().ok()?);
    if fingerprint != expected_fingerprint {
        return None;
    }
    let payload = &bytes[INLINE_IMAGE_CACHE_HEADER_BYTES..];
    let row_bytes = usize::from(width).div_ceil(8);
    let expected_len = row_bytes * usize::from(height);
    if payload.len() != expected_len {
        return None;
    }
    Some(CachedThumbnail {
        width,
        height,
        bits: payload.to_vec(),
        placeholder: flags & INLINE_IMAGE_CACHE_FLAG_PLACEHOLDER != 0,
    })
}

/// Runs on the dedicated worker spawned by [`CoverCache::generate_thumbnail`]
/// — never call this directly from the main task, it does real ZIP/DEFLATE
/// extraction and JPEG/PNG decode work.
fn build_thumbnail(book: &ReaderBook) -> CachedThumbnail {
    if book.format != BookFormat::Epub {
        return placeholder_bitmap();
    }
    let started = std::time::Instant::now();
    match crate::epub::extract_cover(&book.path) {
        Ok(Some(cover)) => {
            let extract_ms = started.elapsed().as_millis();
            let result = decode_and_dither_cover(&cover.bytes, &cover.media_type);
            log::info!(
                "rustmix-wave=cover-timing path={} cover-bytes={} extract-ms={extract_ms} decode-ms={}",
                book.path,
                cover.bytes.len(),
                started.elapsed().as_millis() - extract_ms
            );
            result.unwrap_or_else(|error| {
                log::warn!(
                    "rustmix-wave=cover-cache status=decode-failed path={} error={error}",
                    book.path
                );
                placeholder_bitmap()
            })
        }
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

/// Library and Home thumbnails: the whole cover, stretched to the cell's
/// shape. Covers come in many shapes and the centre crop cut their edges
/// off (often the title or the author); a 2:3 cover comes out about a
/// quarter wider in the 208x252 cell instead. Only the full-screen sleep
/// cover is still cropped ([`decode_and_dither_fill`]).
fn decode_and_dither_cover(bytes: &[u8], media_type: &str) -> Result<CachedThumbnail, String> {
    let gray = decode_gray(bytes, media_type, THUMB_WIDTH, THUMB_HEIGHT)?;
    let resized = resize_area_average(&gray, u32::from(THUMB_WIDTH), u32::from(THUMB_HEIGHT));
    drop(gray);
    let bits = floyd_steinberg_to_1bpp(resized);
    Ok(CachedThumbnail {
        width: THUMB_WIDTH,
        height: THUMB_HEIGHT,
        bits,
        placeholder: false,
    })
}

/// Dispatch to the right decoder by sniffed magic bytes (falling back to the
/// manifest-declared media type, which occasionally lies).
/// `scale_hint_{width,height}` is the size the decoded image must still
/// cover: the JPEG decoders pick their scale from it, [`decode_png`] its
/// reduction factor.
fn decode_gray(
    bytes: &[u8],
    media_type: &str,
    scale_hint_width: u16,
    scale_hint_height: u16,
) -> Result<GrayImage, String> {
    if is_jpeg(bytes) {
        decode_jpeg_fast_or_fallback(bytes, scale_hint_width, scale_hint_height)
    } else if is_png(bytes) {
        decode_png(bytes, scale_hint_width, scale_hint_height)
    } else if media_type.eq_ignore_ascii_case("image/jpeg") {
        decode_jpeg_fast_or_fallback(bytes, scale_hint_width, scale_hint_height)
    } else if media_type.eq_ignore_ascii_case("image/png") {
        decode_png(bytes, scale_hint_width, scale_hint_height)
    } else {
        Err(format!("unsupported image media type: {media_type}"))
    }
}

/// Decode + center-crop + resize + dither to exactly `target_width x
/// target_height`: the "fill" counterpart of [`decode_and_dither_fit_within`]
/// used for the full-screen sleep cover, where letterbox bars would waste
/// panel area and stretching would distort the art.
fn decode_and_dither_fill(
    bytes: &[u8],
    media_type: &str,
    target_width: u32,
    target_height: u32,
) -> Result<CachedThumbnail, String> {
    let target_width_u16 = u16::try_from(target_width)
        .map_err(|_| format!("target width exceeds u16: {target_width}"))?;
    let target_height_u16 = u16::try_from(target_height)
        .map_err(|_| format!("target height exceeds u16: {target_height}"))?;
    let gray = decode_gray(bytes, media_type, target_width_u16, target_height_u16)?;
    let cropped = crop_to_aspect(gray, target_width, target_height);
    let resized = resize_area_average(&cropped, target_width, target_height);
    drop(cropped);
    let bits = floyd_steinberg_to_1bpp(resized);
    Ok(CachedThumbnail {
        width: target_width_u16,
        height: target_height_u16,
        bits,
        placeholder: false,
    })
}

/// Largest centered region of `src` with the `aspect_w:aspect_h` ratio.
fn crop_to_aspect(src: GrayImage, aspect_w: u32, aspect_h: u32) -> GrayImage {
    let (source_w, source_h) = (u64::from(src.width), u64::from(src.height));
    let (aspect_w, aspect_h) = (u64::from(aspect_w.max(1)), u64::from(aspect_h.max(1)));
    let (crop_w, crop_h) = if source_w * aspect_h > source_h * aspect_w {
        ((source_h * aspect_w / aspect_h).max(1), source_h)
    } else {
        (source_w, (source_w * aspect_h / aspect_w).max(1))
    };
    if crop_w == source_w && crop_h == source_h {
        return src;
    }
    let x0 = ((source_w - crop_w) / 2) as usize;
    let y0 = ((source_h - crop_h) / 2) as usize;
    let (crop_w, crop_h) = (crop_w as usize, crop_h as usize);
    let source_width = src.width as usize;
    let mut pixels = Vec::with_capacity(crop_w * crop_h);
    for row in src.pixels.chunks_exact(source_width).skip(y0).take(crop_h) {
        pixels.extend_from_slice(&row[x0..x0 + crop_w]);
    }
    GrayImage {
        width: crop_w as u32,
        height: crop_h as u32,
        pixels,
    }
}

/// Decode + resize + dither one embedded EPUB image to fit within
/// `max_width x max_height`, preserving its own aspect ratio (scaled up or
/// down to fill as much of the box as possible on the binding axis) rather
/// than cropping to the box's exact shape the way
/// [`decode_and_dither_fill`] does. Used for an inline EPUB illustration,
/// whose real aspect ratio the Reader has no reason to know or reserve exact
/// layout space for ahead of decode (unlike a cover thumbnail grid cell,
/// which always wants the exact same shape) -- pagination only reserves a
/// fixed pixel box (`READER_INLINE_IMAGE_SLOT_SPAN` line-slots), and this
/// fits the actual image into it at render/decode time.
#[cfg(test)]
fn decode_and_dither_fit_within(
    bytes: &[u8],
    media_type: &str,
    max_width: u32,
    max_height: u32,
) -> Result<CachedThumbnail, String> {
    decode_and_dither_fit_within_timed(
        bytes,
        media_type,
        max_width,
        max_height,
        &mut DecodeTimings::default(),
    )
}

/// Per-stage cost of one inline-image decode, logged by
/// [`EpubImageCache::generate_bitmap`] so field logs show which stage an
/// image page's first-draw latency actually goes to.
#[derive(Default)]
struct DecodeTimings {
    source_width: u32,
    source_height: u32,
    decoded_width: u32,
    decoded_height: u32,
    decode_ms: u128,
    resize_ms: u128,
    dither_ms: u128,
}

fn decode_and_dither_fit_within_timed(
    bytes: &[u8],
    media_type: &str,
    max_width: u32,
    max_height: u32,
    timings: &mut DecodeTimings,
) -> Result<CachedThumbnail, String> {
    let max_width_u16 =
        u16::try_from(max_width).map_err(|_| format!("max width exceeds u16: {max_width}"))?;
    let max_height_u16 =
        u16::try_from(max_height).map_err(|_| format!("max height exceeds u16: {max_height}"))?;
    let stage = std::time::Instant::now();
    let gray = decode_gray(bytes, media_type, max_width_u16, max_height_u16)?;
    timings.decode_ms = stage.elapsed().as_millis();
    timings.decoded_width = gray.width;
    timings.decoded_height = gray.height;
    if let Some((width, height)) = crate::epub::image_header_size(bytes) {
        timings.source_width = width;
        timings.source_height = height;
    }
    let (fit_width, fit_height) = fit_within(gray.width, gray.height, max_width, max_height);
    let stage = std::time::Instant::now();
    let resized = resize_area_average(&gray, fit_width, fit_height);
    drop(gray);
    timings.resize_ms = stage.elapsed().as_millis();
    let stage = std::time::Instant::now();
    let bits = floyd_steinberg_to_1bpp(resized);
    timings.dither_ms = stage.elapsed().as_millis();
    Ok(CachedThumbnail {
        width: u16::try_from(fit_width).unwrap_or(u16::MAX),
        height: u16::try_from(fit_height).unwrap_or(u16::MAX),
        bits,
        placeholder: false,
    })
}

/// Largest `(width, height)` that preserves `source_w:source_h`'s aspect
/// ratio while fitting within `max_w x max_h` -- scaled up or down as
/// needed, since an inline EPUB image both much larger and much smaller than
/// its reserved page box are equally plausible. Integer cross-multiplication
/// instead of floating point: this target has no FPU-bound math need for a
/// one-time fit computation, and the rest of this module already avoids
/// floats for the same reason.
fn fit_within(source_w: u32, source_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let source_w = u64::from(source_w.max(1));
    let source_h = u64::from(source_h.max(1));
    let max_w = u64::from(max_w.max(1));
    let max_h = u64::from(max_h.max(1));
    let height_at_max_width = (source_h * max_w / source_w).max(1);
    if height_at_max_width <= max_h {
        (max_w as u32, height_at_max_width as u32)
    } else {
        let width_at_max_height = (source_w * max_h / source_h).max(1).min(max_w);
        (width_at_max_height as u32, max_h as u32)
    }
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
}

/// Working memory the luma-only decoder may take for one image: a 1165x1800
/// progressive cover needs about 2.4 MB at half size, for the full-screen
/// sleep cover. Over it, [`crate::jpeg_luma::decode`] steps down a scale.
const JPEG_LUMA_BUDGET_BYTES: usize = 4 * 1024 * 1024;

/// On the device, baseline JPEGs go to Espressif's SIMD-accelerated
/// `esp_new_jpeg` (see [`esp_jpeg::decode_gray`]). Progressive ones, which
/// it cannot read, and anything else it rejects go to the luma-only decoder
/// ([`crate::jpeg_luma`]), which keeps a fraction of what a general
/// progressive decoder needs. `jpeg-decoder` is left with the CMYK and RGB
/// files that one refuses, within [`JPEG_DECODER_BUDGET_BYTES`].
fn decode_jpeg_fast_or_fallback(
    bytes: &[u8],
    target_width: u16,
    target_height: u16,
) -> Result<GrayImage, String> {
    #[cfg(target_os = "espidf")]
    if !jpeg_frame(bytes).is_some_and(|frame| frame.progressive) {
        match esp_jpeg::decode_gray(bytes, u32::from(target_width), u32::from(target_height)) {
            Ok(image) => return Ok(image),
            Err(error) => log::info!("rustmix-wave=esp-jpeg status=fallback reason={error}"),
        }
    }
    let started = std::time::Instant::now();
    match crate::jpeg_luma::decode(
        bytes,
        u32::from(target_width),
        u32::from(target_height),
        JPEG_LUMA_BUDGET_BYTES,
    ) {
        Ok(image) => {
            log::info!(
                "rustmix-wave=jpeg-luma status=decoded progressive={} size={}x{} elapsed-ms={}",
                image.progressive,
                image.width,
                image.height,
                started.elapsed().as_millis()
            );
            return Ok(GrayImage {
                width: image.width,
                height: image.height,
                pixels: image.pixels,
            });
        }
        Err(error) => log::info!("rustmix-wave=jpeg-luma status=fallback reason={error}"),
    }
    decode_jpeg_scaled(bytes, target_width, target_height)
}

/// Thin safe wrapper over the `espressif/esp_new_jpeg` ESP-IDF component
/// (declared in `Cargo.toml`'s `extra_components`). Its decoder cannot emit
/// grayscale (`JPEG_PIXEL_FORMAT_GRAY` is encoder-only), so this decodes to
/// RGB888 and converts, using the decoder's own downscale when the source is
/// at least twice the size the caller needs, so a large cover never exists
/// at full resolution in RAM.
#[cfg(target_os = "espidf")]
mod esp_jpeg {
    use esp_idf_svc::sys::esp_new_jpeg as ffi;

    use super::{fit_within, rgb_to_gray, GrayImage};

    /// Output dimensions the decoder's scaler is asked for: the aspect-fit
    /// target rounded up to the multiple of 8 the scaler requires, never
    /// larger than the source and never below its 1/8 limit. `None` means
    /// decode at full size (no downscale needed, or not allowed).
    fn scaled_size(
        source_w: u32,
        source_h: u32,
        target_w: u32,
        target_h: u32,
    ) -> Option<(u16, u16)> {
        let (fit_w, fit_h) = fit_within(source_w, source_h, target_w, target_h);
        // Any real downscale is worth handing to the decoder: its scaler
        // takes arbitrary ratios down to 1/8, and every pixel it drops is one
        // the RGB conversion and `resize_area_average` never touch.
        if fit_w >= source_w || fit_h >= source_h {
            return None;
        }
        let round_up = |value: u32| value.div_ceil(8) * 8;
        let width = round_up(fit_w.max(source_w.div_ceil(8)));
        let height = round_up(fit_h.max(source_h.div_ceil(8)));
        if width > source_w || height > source_h {
            return None;
        }
        Some((u16::try_from(width).ok()?, u16::try_from(height).ok()?))
    }

    pub(super) fn decode_gray(
        bytes: &[u8],
        target_w: u32,
        target_h: u32,
    ) -> Result<GrayImage, String> {
        let (source_w, source_h) = crate::epub::image_header_size(bytes)
            .ok_or_else(|| "JPEG header not found".to_string())?;
        match scaled_size(source_w, source_h, target_w, target_h) {
            Some(scale) => {
                decode_with_scale(bytes, Some(scale)).or_else(|_| decode_with_scale(bytes, None))
            }
            None => decode_with_scale(bytes, None),
        }
    }

    /// Frees the decoder handle and the aligned output buffer on every exit
    /// path, including early returns on decode errors.
    struct Decoder {
        handle: ffi::jpeg_dec_handle_t,
        outbuf: *mut u8,
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            unsafe {
                if !self.outbuf.is_null() {
                    ffi::jpeg_free_align(self.outbuf.cast());
                }
                if !self.handle.is_null() {
                    ffi::jpeg_dec_close(self.handle);
                }
            }
        }
    }

    fn check(status: ffi::jpeg_error_t, stage: &str) -> Result<(), String> {
        if status == ffi::jpeg_error_t_JPEG_ERR_OK {
            Ok(())
        } else {
            Err(format!("{stage} failed with {status}"))
        }
    }

    fn decode_with_scale(bytes: &[u8], scale: Option<(u16, u16)>) -> Result<GrayImage, String> {
        let input_len = i32::try_from(bytes.len()).map_err(|_| "JPEG too large".to_string())?;
        let mut config = ffi::jpeg_dec_config_t {
            output_type: ffi::jpeg_pixel_format_t_JPEG_PIXEL_FORMAT_RGB888,
            scale: ffi::jpeg_resolution_t {
                width: scale.map_or(0, |value| value.0),
                height: scale.map_or(0, |value| value.1),
            },
            clipper: ffi::jpeg_resolution_t {
                width: 0,
                height: 0,
            },
            rotate: ffi::jpeg_rotate_t_JPEG_ROTATE_0D,
            block_enable: false,
        };
        let mut decoder = Decoder {
            handle: core::ptr::null_mut(),
            outbuf: core::ptr::null_mut(),
        };
        check(
            unsafe { ffi::jpeg_dec_open(&mut config, &mut decoder.handle) },
            "open",
        )?;
        // The decoder only reads `inbuf`; the C API just is not const-correct.
        let mut io = ffi::jpeg_dec_io_t {
            inbuf: bytes.as_ptr().cast_mut(),
            inbuf_len: input_len,
            inbuf_remain: 0,
            outbuf: core::ptr::null_mut(),
            out_size: 0,
        };
        let mut info = ffi::jpeg_dec_header_info_t {
            width: 0,
            height: 0,
        };
        check(
            unsafe { ffi::jpeg_dec_parse_header(decoder.handle, &mut io, &mut info) },
            "parse header",
        )?;
        let mut outbuf_len: i32 = 0;
        check(
            unsafe { ffi::jpeg_dec_get_outbuf_len(decoder.handle, &mut outbuf_len) },
            "output size",
        )?;
        let outbuf_len = usize::try_from(outbuf_len).map_err(|_| "bad output size".to_string())?;
        decoder.outbuf = unsafe { ffi::jpeg_calloc_align(outbuf_len, 16) }.cast();
        if decoder.outbuf.is_null() {
            return Err(format!(
                "output buffer allocation of {outbuf_len} bytes failed"
            ));
        }
        io.outbuf = decoder.outbuf;
        check(
            unsafe { ffi::jpeg_dec_process(decoder.handle, &mut io) },
            "decode",
        )?;
        let (width, height) = match scale {
            Some((width, height)) => (u32::from(width), u32::from(height)),
            None => (u32::from(info.width), u32::from(info.height)),
        };
        let rgb_len = (width * height) as usize * 3;
        if width == 0 || height == 0 || rgb_len > outbuf_len {
            return Err(format!(
                "unexpected output {width}x{height} for {outbuf_len} bytes"
            ));
        }
        let pixels = rgb_to_gray(unsafe { core::slice::from_raw_parts(decoder.outbuf, rgb_len) });
        Ok(GrayImage {
            width,
            height,
            pixels,
        })
    }
}

/// Most memory [`decode_jpeg_scaled`] may ask `jpeg-decoder` for, as
/// estimated by [`jpeg_decoder_peak_bytes`]. In Rust a failed allocation
/// aborts, and a cover that aborts is never cached, so it would be retried,
/// and abort again, every time the Library shows it. Over the budget the
/// image is refused instead, and the placeholder that replaces it is cached
/// like any other result.
const JPEG_DECODER_BUDGET_BYTES: u64 = 2 * 1024 * 1024;

/// The parts of a JPEG frame header that decide how much decoding costs.
#[derive(Debug, PartialEq)]
struct JpegFrame {
    width: u32,
    height: u32,
    progressive: bool,
    /// Horizontal and vertical sampling factors, one pair per component.
    sampling: Vec<(u32, u32)>,
}

/// Reads the frame header (SOF) without decoding anything.
fn jpeg_frame(bytes: &[u8]) -> Option<JpegFrame> {
    if !is_jpeg(bytes) {
        return None;
    }
    let mut cursor = 2;
    while cursor + 4 <= bytes.len() {
        if bytes[cursor] != 0xFF {
            return None;
        }
        let marker = bytes[cursor + 1];
        // Fill bytes and standalone markers carry no length field.
        if marker == 0xFF {
            cursor += 1;
            continue;
        }
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            cursor += 2;
            continue;
        }
        let length = usize::from(u16::from_be_bytes([bytes[cursor + 2], bytes[cursor + 3]]));
        if length < 2 {
            return None;
        }
        let is_sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            // Precision, height, width, component count, then 3 bytes per
            // component: id, sampling factors, quantization table.
            let segment = bytes.get(cursor + 4..cursor + 2 + length)?;
            let height = u32::from(u16::from_be_bytes([*segment.get(1)?, *segment.get(2)?]));
            let width = u32::from(u16::from_be_bytes([*segment.get(3)?, *segment.get(4)?]));
            let count = usize::from(*segment.get(5)?);
            let sampling = (0..count)
                .map(|index| {
                    let factors = *segment.get(6 + index * 3 + 1)?;
                    Some((u32::from(factors >> 4), u32::from(factors & 0x0F)))
                })
                .collect::<Option<Vec<_>>>()?;
            let valid = width > 0
                && height > 0
                && !sampling.is_empty()
                && sampling
                    .iter()
                    .all(|&(h, v)| (1..=4).contains(&h) && (1..=4).contains(&v));
            return valid.then_some(JpegFrame {
                width,
                height,
                progressive: matches!(marker, 0xC2 | 0xC6 | 0xCA | 0xCE),
                sampling,
            });
        }
        cursor += 2 + length;
    }
    None
}

/// The IDCT size, out of 8, that `jpeg-decoder`'s `scale()` picks: the
/// smallest that still covers the request in at least one axis.
fn jpeg_decoder_idct_size(frame: &JpegFrame, target_width: u16, target_height: u16) -> u32 {
    [1, 2, 4]
        .into_iter()
        .find(|&size| {
            (frame.width * size).div_ceil(8) >= u32::from(target_width)
                || (frame.height * size).div_ceil(8) >= u32::from(target_height)
        })
        .unwrap_or(8)
}

/// What `jpeg-decoder` holds at once to decode `frame` for the requested
/// size: the component planes at the IDCT size it picks, the interleaved
/// output, the grayscale copy made from it, and for a progressive image the
/// `i16` coefficients of every component at full resolution, which
/// `scale()` does not reduce (a 1165x1800 colour cover needs 6.3 MB of them).
fn jpeg_decoder_peak_bytes(frame: &JpegFrame, target_width: u16, target_height: u16) -> u64 {
    let max_h = frame.sampling.iter().map(|&(h, _)| h).max().unwrap_or(1);
    let max_v = frame.sampling.iter().map(|&(_, v)| v).max().unwrap_or(1);
    let mcus_x = u64::from(frame.width.div_ceil(8 * max_h));
    let mcus_y = u64::from(frame.height.div_ceil(8 * max_v));
    let blocks: u64 = frame
        .sampling
        .iter()
        .map(|&(h, v)| mcus_x * u64::from(h) * mcus_y * u64::from(v))
        .sum();
    let size = jpeg_decoder_idct_size(frame, target_width, target_height);
    let planes = blocks * u64::from(size * size);
    let output_pixels =
        u64::from((frame.width * size).div_ceil(8)) * u64::from((frame.height * size).div_ceil(8));
    let components = frame.sampling.len() as u64;
    let output = output_pixels * components;
    let gray = if components > 1 { output_pixels } else { 0 };
    let coefficients = if frame.progressive {
        blocks * 64 * 2
    } else {
        0
    };
    coefficients + planes + output + gray
}

/// Decode via `jpeg-decoder`'s native scaled IDCT (factors 1/8, 1/4, 1/2, 1):
/// the decoder picks the smallest factor that still covers the requested
/// thumbnail size in at least one axis, so a large embedded cover is decoded
/// near the target resolution instead of at full size before being thrown
/// away by resize. Refused when that would take more than
/// [`JPEG_DECODER_BUDGET_BYTES`].
fn decode_jpeg_scaled(
    bytes: &[u8],
    target_width: u16,
    target_height: u16,
) -> Result<GrayImage, String> {
    let frame = jpeg_frame(bytes).ok_or_else(|| "JPEG frame header not found".to_string())?;
    let peak_bytes = jpeg_decoder_peak_bytes(&frame, target_width, target_height);
    if peak_bytes > JPEG_DECODER_BUDGET_BYTES {
        return Err(format!(
            "{} JPEG {}x{} needs about {peak_bytes} bytes to decode, over the {JPEG_DECODER_BUDGET_BYTES} byte budget",
            if frame.progressive { "progressive" } else { "baseline" },
            frame.width,
            frame.height
        ));
    }
    let mut decoder = jpeg_decoder::Decoder::new(io::Cursor::new(bytes));
    decoder
        .scale(target_width, target_height)
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

/// PNG via the `png` crate, as gray. A plain image is read one row at a time
/// and reduced while read ([`decode_png_reduced`]); an interlaced one needs
/// the whole frame ([`decode_png_full`]).
fn decode_png(bytes: &[u8], target_width: u16, target_height: u16) -> Result<GrayImage, String> {
    let mut decoder = png::Decoder::new(io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let reader = decoder
        .read_info()
        .map_err(|error| format!("PNG header decode failed: {error}"))?;
    let (width, height, interlaced) = {
        let info = reader.info();
        (info.width, info.height, info.interlaced)
    };
    if width == 0 || height == 0 {
        return Err("PNG has zero dimensions".into());
    }
    if width > MAX_PNG_WIDTH {
        return Err(format!(
            "PNG {width}x{height} is wider than {MAX_PNG_WIDTH} pixels"
        ));
    }
    if interlaced {
        decode_png_full(reader)
    } else {
        decode_png_reduced(reader, u32::from(target_width), u32::from(target_height))
    }
}

/// A plain PNG read one row at a time and shrunk while it is read, by the
/// largest whole factor that still leaves it covering the target: each
/// output pixel is the average of a factor x factor block. In memory there
/// are the inflate window (the `png` crate compacts it past 128 KB), a few
/// rows and the image already reduced, never the full frame. A cover at
/// the size stores ask for, 1600x2560, is 12 MB as an RGB frame,
/// over the budget the full decode refused it on; for the thumbnail this
/// keeps 229x366 of it. The crop and the area-average resize that follow
/// work as for any other decode.
fn decode_png_reduced(
    mut reader: png::Reader<io::Cursor<&[u8]>>,
    target_width: u32,
    target_height: u32,
) -> Result<GrayImage, String> {
    let (width, height) = (reader.info().width, reader.info().height);
    let channels = png_channels(reader.output_color_type().0)?;
    let factor = png_reduction_factor(width, height, target_width, target_height);
    let (out_width, out_height) = (width.div_ceil(factor), height.div_ceil(factor));
    let output_len = out_width as usize * out_height as usize;
    if output_len as u64 > MAX_PNG_DECODED_BUFFER_BYTES {
        return Err(format!(
            "PNG {width}x{height} reduced by {factor} is still {output_len} bytes, over the {MAX_PNG_DECODED_BUFFER_BYTES} byte budget"
        ));
    }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(output_len).map_err(|_| {
        format!(
            "no memory for a {out_width}x{out_height} reduced PNG{}",
            largest_psram_block_note()
        )
    })?;
    let factor = factor as usize;
    let mut sums = vec![0u32; out_width as usize];
    let mut band_rows = 0usize;
    let mut rows_read = 0u32;
    while let Some(row) = reader
        .next_row()
        .map_err(|error| format!("PNG row decode failed: {error}"))?
    {
        for (sum, block) in sums.iter_mut().zip(row.data().chunks(factor * channels)) {
            *sum += block
                .chunks_exact(channels)
                .map(|pixel| u32::from(png_pixel_gray(pixel)))
                .sum::<u32>();
        }
        rows_read += 1;
        band_rows += 1;
        if band_rows == factor || rows_read == height {
            // The last column and the last band can be narrower than the
            // factor: each average counts only the pixels it covers.
            for (column, sum) in sums.iter_mut().enumerate() {
                let block_width = (width as usize - column * factor).min(factor);
                let count = (block_width * band_rows) as u32;
                pixels.push(((*sum + count / 2) / count) as u8);
                *sum = 0;
            }
            band_rows = 0;
        }
    }
    if rows_read != height {
        return Err(format!("PNG ended after {rows_read} of {height} rows"));
    }
    Ok(GrayImage {
        width: out_width,
        height: out_height,
        pixels,
    })
}

/// Largest whole factor the image can be shrunk by and still cover
/// `target_width x target_height` in both directions, as the JPEG decoders
/// pick their scale: the centre crop and the resize that follow start from
/// at least the target size. 1 when the image is smaller already.
fn png_reduction_factor(width: u32, height: u32, target_width: u32, target_height: u32) -> u32 {
    if target_width == 0 || target_height == 0 {
        return 1;
    }
    (width / target_width)
        .min(height / target_height)
        .clamp(1, MAX_PNG_REDUCTION_FACTOR)
}

fn png_channels(color_type: png::ColorType) -> Result<usize, String> {
    match color_type {
        png::ColorType::Grayscale => Ok(1),
        png::ColorType::GrayscaleAlpha => Ok(2),
        png::ColorType::Rgb => Ok(3),
        png::ColorType::Rgba => Ok(4),
        png::ColorType::Indexed => Err("PNG used indexed color after normalize_to_color8".into()),
    }
}

/// One 8-bit PNG pixel (gray, gray and alpha, RGB, RGBA) as gray, with
/// transparency drawn on white: the page is white, and the color hidden
/// under a transparent pixel is often black.
fn png_pixel_gray(pixel: &[u8]) -> u8 {
    let (value, alpha) = match *pixel {
        [] => return 255,
        [gray] => return gray,
        [gray, alpha] => (gray, alpha),
        [r, g, b] => return luma(r, g, b),
        [r, g, b, alpha, ..] => (luma(r, g, b), alpha),
    };
    if alpha == 255 {
        return value;
    }
    let (value, alpha) = (u32::from(value), u32::from(alpha));
    ((value * alpha + 255 * (255 - alpha) + 127) / 255) as u8
}

/// An interlaced (Adam7) PNG, whose seven sparse passes only a whole frame
/// puts back together: decoded at full size within
/// [`MAX_PNG_DECODED_BUFFER_BYTES`], then turned to gray in place.
fn decode_png_full(mut reader: png::Reader<io::Cursor<&[u8]>>) -> Result<GrayImage, String> {
    let (header_width, header_height) = (reader.info().width, reader.info().height);
    // The frame as decoded, after the expansion to 8-bit samples: a palette
    // image comes out RGB, three times the size its header suggests.
    let output_len = reader.output_buffer_size();
    if output_len as u64 > MAX_PNG_DECODED_BUFFER_BYTES {
        return Err(format!(
            "PNG {header_width}x{header_height} decodes to {output_len} bytes, over the {MAX_PNG_DECODED_BUFFER_BYTES} byte budget"
        ));
    }
    // Reserved fallibly: a failed allocation aborts, and a cover that aborts
    // the firmware brings it down again at every boot.
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(output_len).map_err(|_| {
        format!(
            "no memory for a {output_len}-byte PNG frame ({header_width}x{header_height}){}",
            largest_psram_block_note()
        )
    })?;
    buffer.resize(output_len, 0);
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| format!("PNG frame decode failed: {error}"))?;
    let channels = png_channels(info.color_type)?;
    let pixel_count = info.width as usize * info.height as usize;
    if buffer.len() < pixel_count * channels {
        return Err("PNG frame shorter than its header says".into());
    }
    // To gray in place: the value of pixel i goes to byte i, at or before
    // where its own samples start, so no second frame-sized buffer.
    for index in 0..pixel_count {
        let start = index * channels;
        let gray = png_pixel_gray(&buffer[start..start + channels]);
        buffer[index] = gray;
    }
    buffer.truncate(pixel_count);
    Ok(GrayImage {
        width: info.width,
        height: info.height,
        pixels: buffer,
    })
}

/// For allocation-failure messages: the largest block of PSRAM still free,
/// which tells fragmentation apart from plain exhaustion.
fn largest_psram_block_note() -> String {
    #[cfg(target_os = "espidf")]
    {
        let largest = unsafe {
            esp_idf_svc::sys::heap_caps_get_largest_free_block(esp_idf_svc::sys::MALLOC_CAP_SPIRAM)
        };
        let free = unsafe {
            esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_SPIRAM)
        };
        format!(", PSRAM free {free} bytes, largest block {largest}")
    }
    #[cfg(not(target_os = "espidf"))]
    String::new()
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
    // Same box average as computing each pixel's source span on the fly,
    // but each target column's span is computed once instead of once per
    // pixel: on the device the per-pixel divisions and bounds checks, not
    // the averaging itself, dominated this stage.
    let columns: Vec<(usize, usize)> = (0..target_width)
        .map(|target_x| {
            let x0 = target_x * src.width / target_width;
            let x1 = ((target_x + 1) * src.width / target_width)
                .max(x0 + 1)
                .min(src.width);
            (x0 as usize, x1 as usize)
        })
        .collect();
    let source_width = src.width as usize;
    let mut pixels = Vec::with_capacity((target_width * target_height) as usize);
    for target_y in 0..target_height {
        let y0 = target_y * src.height / target_height;
        let y1 = ((target_y + 1) * src.height / target_height)
            .max(y0 + 1)
            .min(src.height);
        let rows = &src.pixels[y0 as usize * source_width..y1 as usize * source_width];
        let row_count = (y1 - y0) as usize;
        for &(x0, x1) in &columns {
            let mut sum: u32 = 0;
            for row in rows.chunks_exact(source_width) {
                sum += row[x0..x1]
                    .iter()
                    .map(|value| u32::from(*value))
                    .sum::<u32>();
            }
            let count = (row_count * (x1 - x0)) as u32;
            pixels.push(if count > 0 { (sum / count) as u8 } else { 255 });
        }
    }
    GrayImage {
        width: target_width,
        height: target_height,
        pixels,
    }
}

/// Floyd-Steinberg error-diffusion dither straight to packed 1bpp.
///
/// Works on two small row buffers (the row being quantized and the one
/// below it) instead of scattering read-modify-write updates over the
/// whole image: those buffers are a few hundred bytes, so they stay in
/// internal RAM while the image itself usually lives in PSRAM. Each
/// diffusion still clamps to 0..=255 exactly as the original in-place
/// version did, so the output is bit-for-bit identical.
fn floyd_steinberg_to_1bpp(gray: GrayImage) -> Vec<u8> {
    let width = gray.width as usize;
    let height = gray.height as usize;
    let row_bytes = width.div_ceil(8);
    let mut bits = vec![0u8; row_bytes * height];
    if width == 0 || height == 0 {
        return bits;
    }
    let load = |y: usize, row: &mut Vec<i16>| {
        row.clear();
        row.extend(
            gray.pixels[y * width..(y + 1) * width]
                .iter()
                .map(|value| i16::from(*value)),
        );
    };
    let diffuse = |value: &mut i16, amount: i32| {
        *value = (i32::from(*value) + amount).clamp(0, 255) as i16;
    };
    let mut current: Vec<i16> = Vec::with_capacity(width);
    let mut next: Vec<i16> = Vec::with_capacity(width);
    load(0, &mut current);
    for y in 0..height {
        let has_next = y + 1 < height;
        if has_next {
            load(y + 1, &mut next);
        }
        let out = &mut bits[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..width {
            let old_value = i32::from(current[x]);
            let ink = old_value < 128;
            if ink {
                out[x / 8] |= 0x80 >> (x % 8);
            }
            let error = old_value - if ink { 0 } else { 255 };
            if x + 1 < width {
                diffuse(&mut current[x + 1], error * 7 / 16);
            }
            if has_next {
                if x > 0 {
                    diffuse(&mut next[x - 1], error * 3 / 16);
                }
                diffuse(&mut next[x], error * 5 / 16);
                if x + 1 < width {
                    diffuse(&mut next[x + 1], error * 1 / 16);
                }
            }
        }
        core::mem::swap(&mut current, &mut next);
    }
    bits
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

/// Bumped whenever the `.SLC` full-screen sleep cover decode or format
/// changes in a way that must invalidate every existing entry. `"2"`:
/// progressive JPEGs decoded by `jpeg_luma`. `"3"`: PNGs reduced while
/// read, transparency on white.
const FULLSCREEN_COVER_FORMAT_VERSION: &str = "3";

fn fullscreen_cover_fingerprint(book: &ReaderBook, width: u16, height: u16) -> u64 {
    let mut hash = CACHE_FNV_OFFSET;
    fn feed(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(CACHE_FNV_PRIME);
        }
    }
    feed(&mut hash, b"sleep-cover");
    feed(&mut hash, book.path.as_bytes());
    feed(&mut hash, &book.size_bytes.to_le_bytes());
    feed(&mut hash, &book.modified_seconds.to_le_bytes());
    feed(&mut hash, &width.to_le_bytes());
    feed(&mut hash, &height.to_le_bytes());
    feed(&mut hash, FULLSCREEN_COVER_FORMAT_VERSION.as_bytes());
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
    if bytes.len() < CACHE_HEADER_BYTES || bytes[0..4] != CACHE_MAGIC || bytes[4] != CACHE_VERSION {
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
/// A marker on the card while one image is decoded. Decoding runs on a
/// worker, but an out-of-memory abort or a stack overflow there still takes
/// the whole firmware down, and nothing gets cached: the next boot decodes
/// the same image, crashes again, and the device never gets past it (seen:
/// a PNG cover that the Home "Continue" card decodes at every boot). A
/// marker still there means the last attempt never came back, and the
/// caller caches its placeholder instead of trying again.
struct DecodeAttempt {
    marker: PathBuf,
}

impl DecodeAttempt {
    /// `None` when an earlier attempt for `cache_path` crashed the device.
    fn begin(cache_path: &Path) -> Option<Self> {
        let marker = cache_path.with_extension("PND");
        if marker.exists() {
            let _ = fs::remove_file(&marker);
            return None;
        }
        // Best effort: without a card, nothing gets cached either.
        let _ = fs::write(&marker, b"");
        Some(Self { marker })
    }
}

impl Drop for DecodeAttempt {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.marker);
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("TMP");
    fs::write(&tmp_path, bytes)?;
    // ELM FatFs's `f_rename` (what `fs::rename` reaches through ESP-IDF's FAT
    // VFS) refuses to replace an existing destination, unlike POSIX rename,
    // failing with `EEXIST` -- the same issue `network_config.rs`'s
    // `save_to_path` already documents and works around. Without clearing
    // the destination first, a pre-existing `.THB` that fails to parse
    // (corrupt, truncated, or written by an older cache format) can never be
    // replaced: this write silently fails every time, so `pump_pending`
    // regenerates the same thumbnail on every single loop tick forever.
    let _ = fs::remove_file(path);
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
        decode_and_dither_cover, decode_and_dither_fill, decode_and_dither_fit_within, decode_gray,
        fit_within, floyd_steinberg_to_1bpp, parse_cache_bytes, parse_inline_image_cache_bytes,
        placeholder_bitmap, png_pixel_gray, resize_area_average, write_cache_bytes,
        write_inline_image_cache_bytes, CoverCache, EpubImageCache, GrayImage, CACHE_HEADER_BYTES,
        MAX_PNG_DECODED_BUFFER_BYTES, MAX_PNG_WIDTH, THUMB_BITMAP_BYTES, THUMB_HEIGHT, THUMB_WIDTH,
    };
    use crate::reader::{BookFormat, ReaderBook};

    /// SOI, an APP0 segment and a frame header: all the budget check reads.
    fn jpeg_header(marker: u8, width: u16, height: u16, sampling: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        bytes.extend_from_slice(&[0xFF, marker]);
        bytes.extend_from_slice(&(8 + 3 * sampling.len() as u16).to_be_bytes());
        bytes.push(8);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.push(sampling.len() as u8);
        for (index, factors) in sampling.iter().enumerate() {
            bytes.extend_from_slice(&[index as u8 + 1, *factors, 0]);
        }
        bytes
    }

    #[test]
    fn jpeg_frame_reads_size_sampling_and_mode() {
        let frame = super::jpeg_frame(&jpeg_header(0xC2, 1165, 1800, &[0x22, 0x11, 0x11]));
        assert_eq!(
            frame,
            Some(super::JpegFrame {
                width: 1165,
                height: 1800,
                progressive: true,
                sampling: vec![(2, 2), (1, 1), (1, 1)],
            })
        );
        let gray = super::jpeg_frame(&jpeg_header(0xC0, 600, 900, &[0x11])).unwrap();
        assert!(!gray.progressive);
        assert_eq!(gray.sampling, vec![(1, 1)]);
        assert!(super::jpeg_frame(&jpeg_header(0xC0, 0, 900, &[0x11])).is_none());
        assert!(super::jpeg_frame(&jpeg_header(0xC0, 600, 900, &[0x10])).is_none());
        assert!(super::jpeg_frame(&[0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08]).is_none());
        assert!(super::jpeg_frame(b"\x89PNG\r\n\x1a\n").is_none());
    }

    #[test]
    fn idct_size_matches_jpeg_decoder_choice() {
        let frame = super::jpeg_frame(&jpeg_header(0xC0, 1165, 1800, &[0x11])).unwrap();
        // 1/8 gives 146x225, short of 208x252 in both axes; 1/4 gives 292.
        assert_eq!(
            super::jpeg_decoder_idct_size(&frame, THUMB_WIDTH, THUMB_HEIGHT),
            2
        );
        assert_eq!(super::jpeg_decoder_idct_size(&frame, 480, 800), 4);
        assert_eq!(super::jpeg_decoder_idct_size(&frame, 140, 2000), 1);
        assert_eq!(super::jpeg_decoder_idct_size(&frame, 2000, 2000), 8);
    }

    #[test]
    fn large_progressive_jpegs_are_refused_before_decoding() {
        // The review's example: 6.3 MB of coefficients alone.
        let verity = jpeg_header(0xC2, 1165, 1800, &[0x22, 0x11, 0x11]);
        let frame = super::jpeg_frame(&verity).unwrap();
        let coefficients: u64 = (146 * 226 + 2 * 73 * 113) * 64 * 2;
        assert!(super::jpeg_decoder_peak_bytes(&frame, THUMB_WIDTH, THUMB_HEIGHT) > coefficients);
        let error = super::decode_jpeg_scaled(&verity, THUMB_WIDTH, THUMB_HEIGHT)
            .err()
            .expect("over budget");
        assert!(error.contains("budget"), "{error}");
        // The same picture as a baseline JPEG is decoded near the thumbnail
        // size, and a small progressive one still fits.
        let baseline =
            super::jpeg_frame(&jpeg_header(0xC0, 1165, 1800, &[0x22, 0x11, 0x11])).unwrap();
        assert!(
            super::jpeg_decoder_peak_bytes(&baseline, THUMB_WIDTH, THUMB_HEIGHT)
                < super::JPEG_DECODER_BUDGET_BYTES
        );
        let small = super::jpeg_frame(&jpeg_header(0xC2, 400, 600, &[0x22, 0x11, 0x11])).unwrap();
        assert!(
            super::jpeg_decoder_peak_bytes(&small, THUMB_WIDTH, THUMB_HEIGHT)
                < super::JPEG_DECODER_BUDGET_BYTES
        );
    }

    #[test]
    fn crop_to_aspect_keeps_the_centered_region() {
        // 6x2 source, columns numbered 0..6; a 1:1 crop keeps columns 2..4.
        let wide = GrayImage {
            width: 6,
            height: 2,
            pixels: vec![0, 1, 2, 3, 4, 5, 0, 1, 2, 3, 4, 5],
        };
        let cropped = super::crop_to_aspect(wide, 1, 1);
        assert_eq!((cropped.width, cropped.height), (2, 2));
        assert_eq!(cropped.pixels, vec![2, 3, 2, 3]);
        // 2x6 source, rows numbered 0..6; a 1:1 crop keeps rows 2..4.
        let tall = GrayImage {
            width: 2,
            height: 6,
            pixels: vec![0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5],
        };
        let cropped = super::crop_to_aspect(tall, 1, 1);
        assert_eq!((cropped.width, cropped.height), (2, 2));
        assert_eq!(cropped.pixels, vec![2, 2, 3, 3]);
    }

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
    fn a_decode_that_crashed_the_device_is_not_tried_again() {
        let root = temp_dir("decode-guard");
        fs::create_dir_all(&root).unwrap();
        let cache = CoverCache::new(&root);
        let book = sample_book("/no/such/book.epub");
        // What a crash during the previous attempt leaves on the card.
        let marker = cache.cache_path(&book).with_extension("PND");
        fs::write(&marker, b"").unwrap();
        let thumbnail = cache.generate_thumbnail(&book);
        assert!(thumbnail.placeholder);
        assert!(!marker.exists());
        // Cached, so the next boot does not decode it either.
        assert_eq!(cache.load_cached_thumbnail(&book), Some(thumbnail));
        // A normal attempt leaves no marker behind.
        let other = sample_book("/no/such/other.epub");
        let _ = cache.generate_thumbnail(&other);
        assert!(!cache.cache_path(&other).with_extension("PND").exists());
        fs::remove_dir_all(&root).unwrap();
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
    fn progressive_and_cmyk_jpeg_covers_are_drawn() {
        let progressive = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/jpeg/progressive_420.jpg"
        ));
        let cmyk = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/jpeg/cmyk.jpg"
        ));
        // The progressive one goes through jpeg_luma, the CMYK one back to
        // jpeg-decoder.
        for bytes in [&progressive[..], &cmyk[..]] {
            let cover = decode_and_dither_cover(bytes, "image/jpeg").unwrap();
            assert!(!cover.placeholder);
            assert_eq!((cover.width, cover.height), (THUMB_WIDTH, THUMB_HEIGHT));
            // A picture: neither blank paper nor a solid block of ink.
            let ink: u32 = cover.bits.iter().map(|byte| byte.count_ones()).sum();
            let pixels = u32::from(THUMB_WIDTH) * u32::from(THUMB_HEIGHT);
            assert!(
                ink > pixels / 10 && ink < pixels * 9 / 10,
                "ink {ink} of {pixels}"
            );
        }
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

    /// The pre-optimization implementations, kept only as a reference: the
    /// fast versions must match them bit for bit.
    fn reference_resize(src: &GrayImage, target_width: u32, target_height: u32) -> Vec<u8> {
        let mut pixels = vec![0u8; (target_width * target_height) as usize];
        for target_y in 0..target_height {
            let y0 = target_y * src.height / target_height;
            let y1 = ((target_y + 1) * src.height / target_height)
                .max(y0 + 1)
                .min(src.height);
            for target_x in 0..target_width {
                let x0 = target_x * src.width / target_width;
                let x1 = ((target_x + 1) * src.width / target_width)
                    .max(x0 + 1)
                    .min(src.width);
                let (mut sum, mut count) = (0u32, 0u32);
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += u32::from(src.pixels[(y * src.width + x) as usize]);
                        count += 1;
                    }
                }
                pixels[(target_y * target_width + target_x) as usize] =
                    if count > 0 { (sum / count) as u8 } else { 255 };
            }
        }
        pixels
    }

    fn reference_dither(mut gray: GrayImage) -> Vec<u8> {
        let (width, height) = (gray.width as usize, gray.height as usize);
        let row_bytes = width.div_ceil(8);
        let mut bits = vec![0u8; row_bytes * height];
        let diffuse = |pixels: &mut [u8], index: usize, error: i32| {
            pixels[index] = (i32::from(pixels[index]) + error).clamp(0, 255) as u8;
        };
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

    fn noisy_gray(width: u32, height: u32) -> GrayImage {
        let mut state: u32 = 0x1234_5678;
        let pixels = (0..width * height)
            .map(|index| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((index % width) * 255 / width.max(1)) as u8 ^ (state >> 28) as u8
            })
            .collect();
        GrayImage {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn fast_resize_and_dither_match_the_reference_bit_for_bit() {
        for (sw, sh, tw, th) in [
            (800, 1228, 432, 663),
            (279, 280, 432, 433),
            (37, 11, 5, 9),
            (1, 1, 3, 2),
        ] {
            let src = noisy_gray(sw, sh);
            let resized = resize_area_average(&src, tw, th);
            assert_eq!(
                resized.pixels,
                reference_resize(&src, tw, th),
                "{sw}x{sh}->{tw}x{th}"
            );
            let reference = reference_dither(GrayImage {
                width: resized.width,
                height: resized.height,
                pixels: resized.pixels.clone(),
            });
            assert_eq!(
                floyd_steinberg_to_1bpp(resized),
                reference,
                "{sw}x{sh}->{tw}x{th}"
            );
        }
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

    /// A 4:1 picture, black in its left quarter and white elsewhere. The
    /// centre crop kept only white columns from the middle; stretched, the
    /// black quarter is the thumbnail's left quarter.
    #[test]
    fn covers_are_stretched_to_the_cell_not_cropped() {
        let (width, height) = (400u32, 100u32);
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let pixels: Vec<u8> = (0..width * height)
                .map(|index| if index % width < width / 4 { 0 } else { 255 })
                .collect();
            writer.write_image_data(&pixels).unwrap();
        }
        let thumbnail = decode_and_dither_cover(&png_bytes, "image/png").unwrap();
        assert_eq!(
            (thumbnail.width, thumbnail.height),
            (THUMB_WIDTH, THUMB_HEIGHT)
        );
        let row_bytes = usize::from(THUMB_WIDTH) / 8;
        let ink =
            |x: usize, y: usize| thumbnail.bits[y * row_bytes + x / 8] & (0x80 >> (x % 8)) != 0;
        let black_columns = usize::from(THUMB_WIDTH) / 4;
        for y in [
            0,
            usize::from(THUMB_HEIGHT) / 2,
            usize::from(THUMB_HEIGHT) - 1,
        ] {
            assert!(ink(0, y), "left edge of row {y}");
            assert!(
                ink(black_columns - 2, y),
                "end of the black quarter, row {y}"
            );
            assert!(
                !ink(black_columns + 2, y),
                "start of the white part, row {y}"
            );
            assert!(
                !ink(usize::from(THUMB_WIDTH) - 1, y),
                "right edge of row {y}"
            );
        }
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

    /// Inline-image spike, step 1: decode+resize+dither an embedded EPUB
    /// image at the Reader's full page content width (432px = 480px portrait
    /// panel minus `READER_BODY_MARGIN_PX` on each side, see
    /// `app::screens::reader::ReaderBodyGeometry`) instead of `THUMB_WIDTH`,
    /// to see whether the existing cover pipeline's cost stays reasonable
    /// outside thumbnail scale before any inline-image integration is built.
    ///
    /// Source resolution (1240x1754, RGB) approximates a 150dpi A5 scanned
    /// illustration -- a plausible embedded EPUB image, and deliberately
    /// larger than a typical cover. PNG has no native scaled decode: the
    /// rows are reduced while read ([`decode_png_reduced`]), so the full
    /// RGB frame the line below prints never exists. Host timing only, not
    /// representative of ESP32-S3 wall time.
    #[test]
    fn inline_image_poc_full_page_width_decode_timing() {
        let source_width = 1240u32;
        let source_height = 1754u32;
        let png_bytes = encode_gradient_png(source_width, source_height);
        let source_decoded_rgb_bytes = (source_width * source_height * 3) as usize;
        println!(
            "poc: source png bytes={} ({source_width}x{source_height}) \
             transient-full-res-rgb-decode-bytes={source_decoded_rgb_bytes}",
            png_bytes.len()
        );

        for (label, target_width, target_height) in [
            ("half-page", 432u32, 300u32),
            ("near-full-page", 432u32, 600u32),
        ] {
            let started = std::time::Instant::now();
            let thumbnail =
                decode_and_dither_fill(&png_bytes, "image/png", target_width, target_height)
                    .unwrap();
            let elapsed = started.elapsed();
            assert_eq!(thumbnail.width, target_width as u16);
            assert_eq!(thumbnail.height, target_height as u16);
            let expected_bits = (target_width as usize).div_ceil(8) * target_height as usize;
            assert_eq!(thumbnail.bits.len(), expected_bits);
            println!(
                "poc: {label} target={target_width}x{target_height} \
                 decode+resize+dither-elapsed={elapsed:?} packed-1bpp-bytes={}",
                thumbnail.bits.len()
            );
        }
    }

    /// A cover at the size stores ask for: as a full RGB frame it is 12 MB,
    /// over the budget the full-frame decode refused it on. Read row by row
    /// it comes out already reduced.
    #[test]
    fn a_large_png_is_reduced_while_it_is_read() {
        let png_bytes = encode_gradient_png(1600, 2560);
        let gray = decode_gray(&png_bytes, "image/png", THUMB_WIDTH, THUMB_HEIGHT).unwrap();
        // The largest factor still covering 208x252: min(1600/208, 2560/252) = 7.
        assert_eq!((gray.width, gray.height), (229, 366));
        assert_eq!(gray.pixels.len(), 229 * 366);
        assert!(
            !decode_and_dither_cover(&png_bytes, "image/png")
                .unwrap()
                .placeholder
        );
    }

    #[test]
    fn png_reduction_averages_each_block_including_the_narrow_edges() {
        // 10x6 gray reduced by 3 (target 3x2): blocks 3, 3, 3 and 1 wide.
        let (width, height) = (10usize, 6usize);
        let pixels: Vec<u8> = (0..width * height).map(|index| (index * 4) as u8).collect();
        let png_bytes = handmade_png(width as u32, height as u32, 0, false, &pixels);
        let gray = decode_gray(&png_bytes, "image/png", 3, 2).unwrap();
        assert_eq!((gray.width, gray.height), (4, 2));
        let mut expected = Vec::new();
        for band in 0..2 {
            for column in 0..4 {
                let (mut sum, mut count) = (0u32, 0u32);
                for y in band * 3..(band * 3 + 3).min(height) {
                    for x in column * 3..(column * 3 + 3).min(width) {
                        sum += u32::from(pixels[y * width + x]);
                        count += 1;
                    }
                }
                expected.push(((sum + count / 2) / count) as u8);
            }
        }
        assert_eq!(gray.pixels, expected);
    }

    #[test]
    fn interlaced_and_plain_pngs_decode_to_the_same_gray() {
        // RGBA with every level of transparency, for the compositing too.
        let (width, height) = (13u32, 11u32);
        let pixels: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                let value = (index * 37 % 256) as u8;
                [
                    value,
                    value.wrapping_mul(3),
                    255 - value,
                    (index * 11 % 256) as u8,
                ]
            })
            .collect();
        let plain = handmade_png(width, height, 6, false, &pixels);
        let interlaced = handmade_png(width, height, 6, true, &pixels);
        // A target as large as the image: nothing reduced, every pixel compared.
        let (hint_width, hint_height) = (width as u16, height as u16);
        let from_plain = decode_gray(&plain, "image/png", hint_width, hint_height).unwrap();
        let from_interlaced =
            decode_gray(&interlaced, "image/png", hint_width, hint_height).unwrap();
        assert_eq!((from_plain.width, from_plain.height), (width, height));
        assert_eq!(
            (from_interlaced.width, from_interlaced.height),
            (width, height)
        );
        let expected: Vec<u8> = pixels.chunks_exact(4).map(png_pixel_gray).collect();
        assert_eq!(from_plain.pixels, expected);
        assert_eq!(from_interlaced.pixels, expected);
    }

    #[test]
    fn transparent_png_pixels_are_drawn_on_white() {
        assert_eq!(png_pixel_gray(&[0, 0, 0, 0]), 255);
        assert_eq!(png_pixel_gray(&[0, 0, 0, 255]), 0);
        assert_eq!(png_pixel_gray(&[0, 128]), 127);
        assert_eq!(png_pixel_gray(&[200]), 200);
        assert_eq!(png_pixel_gray(&[255, 255, 255]), 255);
    }

    /// An interlaced PNG still needs the whole frame: past
    /// `MAX_PNG_DECODED_BUFFER_BYTES` it is refused from its header, before
    /// the frame is allocated.
    #[test]
    fn an_oversized_interlaced_png_is_refused_before_allocating() {
        let (width, height) = (2000u32, 2000u32);
        assert!(u64::from(width) * u64::from(height) * 4 > MAX_PNG_DECODED_BUFFER_BYTES);
        let pixels = vec![0u8; (width * height * 4) as usize];
        let png_bytes = handmade_png(width, height, 6, true, &pixels);
        let error = decode_and_dither_fill(&png_bytes, "image/png", 432, 300)
            .expect_err("an oversized interlaced PNG must be refused");
        assert!(
            error.contains("over the") && error.contains("byte budget"),
            "unexpected error message: {error}"
        );
    }

    /// The reduced image has the same budget: a PNG that cannot be reduced
    /// (shorter than the target) and is past it is refused too.
    #[test]
    fn a_png_too_large_even_reduced_is_refused() {
        let (width, height) = (MAX_PNG_WIDTH, 600u32);
        assert!(u64::from(width) * u64::from(height) > MAX_PNG_DECODED_BUFFER_BYTES);
        let pixels = vec![0u8; (width * height) as usize];
        let png_bytes = handmade_png(width, height, 0, false, &pixels);
        let error = decode_and_dither_fill(&png_bytes, "image/png", 432, 800)
            .expect_err("a PNG past the budget must be refused");
        assert!(error.contains("byte budget"), "unexpected error: {error}");
        let wider = handmade_png(MAX_PNG_WIDTH + 1, 1, 0, false, &vec![0u8; 16_385]);
        let error = decode_and_dither_fill(&wider, "image/png", 100, 100)
            .expect_err("a PNG past the width limit must be refused");
        assert!(error.contains("wider than"), "unexpected error: {error}");
    }

    /// CRC-32 of a PNG chunk's type and data, a bit at a time: fine for a
    /// test.
    fn png_crc(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A PNG put together by hand, because the `png` crate cannot write an
    /// interlaced one. 8-bit `color_type` 0 (gray), 2 (RGB), 4 (gray and
    /// alpha) or 6 (RGBA); rows unfiltered, in Adam7 passes when
    /// `interlaced`.
    fn handmade_png(
        width: u32,
        height: u32,
        color_type: u8,
        interlaced: bool,
        pixels: &[u8],
    ) -> Vec<u8> {
        let channels = match color_type {
            0 => 1,
            2 => 3,
            4 => 2,
            6 => 4,
            _ => panic!("unsupported color type {color_type}"),
        };
        let mut scanlines = Vec::new();
        let mut push_pass = |x0: u32, y0: u32, dx: u32, dy: u32| {
            if x0 >= width || y0 >= height {
                return;
            }
            for y in (y0..height).step_by(dy as usize) {
                scanlines.push(0); // filter type: none
                for x in (x0..width).step_by(dx as usize) {
                    let start = (y * width + x) as usize * channels;
                    scanlines.extend_from_slice(&pixels[start..start + channels]);
                }
            }
        };
        if interlaced {
            for (x0, y0, dx, dy) in [
                (0, 0, 8, 8),
                (4, 0, 8, 8),
                (0, 4, 4, 8),
                (2, 0, 4, 4),
                (0, 2, 2, 4),
                (1, 0, 2, 2),
                (0, 1, 1, 2),
            ] {
                push_pass(x0, y0, dx, dy);
            }
        } else {
            push_pass(0, 0, 1, 1);
        }
        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, color_type, 0, 0, u8::from(interlaced)]);
        let image_data = miniz_oxide::deflate::compress_to_vec_zlib(&scanlines, 6);
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        for (kind, data) in [
            (*b"IHDR", header),
            (*b"IDAT", image_data),
            (*b"IEND", Vec::new()),
        ] {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let start = png.len();
            png.extend_from_slice(&kind);
            png.extend_from_slice(&data);
            let crc = png_crc(&png[start..]);
            png.extend_from_slice(&crc.to_be_bytes());
        }
        png
    }

    /// Encode a flat (fast-to-compress) RGBA PNG at an arbitrary declared
    /// size -- used only to exercise the pre-allocation size guard above, so
    /// pixel content doesn't matter and a flat fill keeps the test fast.
    fn encode_flat_rgba_png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let pixels = vec![0u8; (width * height * 4) as usize];
            writer.write_image_data(&pixels).unwrap();
        }
        bytes
    }

    /// Encode a source-resolution RGB PNG with varying per-pixel content (an
    /// x/y/xor pattern, not a flat fill) so resize and Floyd-Steinberg dither
    /// in [`inline_image_poc_full_page_width_decode_timing`] do the same
    /// amount of branching/error-diffusion work a real photographic or
    /// line-art illustration would force, unlike `encode_test_png`'s flat
    /// black fixture above (fine for a decode round-trip check, useless for
    /// timing).
    fn encode_gradient_png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let mut pixels = vec![0u8; (width * height * 3) as usize];
            for y in 0..height {
                for x in 0..width {
                    let index = ((y * width + x) * 3) as usize;
                    pixels[index] = (x % 256) as u8;
                    pixels[index + 1] = (y % 256) as u8;
                    pixels[index + 2] = ((x ^ y) % 256) as u8;
                }
            }
            writer.write_image_data(&pixels).unwrap();
        }
        bytes
    }

    #[test]
    fn fit_within_binds_on_width_for_a_wide_source() {
        // 400x100 (4:1 landscape) into a 100x100 box: height would need to
        // be 25 to keep the source's aspect ratio at width=100, which
        // already fits within the 100 box on that axis, so width is what
        // binds.
        assert_eq!(fit_within(400, 100, 100, 100), (100, 25));
    }

    #[test]
    fn fit_within_binds_on_height_for_a_tall_source() {
        // 100x400 (1:4 portrait) into a 100x100 box: the mirror image of
        // the width-binding case above.
        assert_eq!(fit_within(100, 400, 100, 100), (25, 100));
    }

    #[test]
    fn fit_within_scales_up_a_source_smaller_than_the_box() {
        // A tiny inline image should still fill a reasonable amount of its
        // reserved page box rather than staying pixel-for-pixel tiny --
        // `fit_within` scales up as readily as it scales down.
        assert_eq!(fit_within(10, 10, 200, 100), (100, 100));
    }

    #[test]
    fn decode_and_dither_fit_within_preserves_aspect_and_fits_the_box() {
        // 400x100 landscape source into a 100x50 box: at width=100 the
        // implied height is 25, which fits under 50, so the result must be
        // exactly 100x25, not cropped to fill the full 100x50 box the way
        // `decode_and_dither_fill` deliberately would.
        let png_bytes = encode_flat_rgba_png(400, 100);
        let thumbnail = decode_and_dither_fit_within(&png_bytes, "image/png", 100, 50).unwrap();
        assert_eq!((thumbnail.width, thumbnail.height), (100, 25));
        assert_eq!(
            thumbnail.bits.len(),
            (thumbnail.width as usize).div_ceil(8) * thumbnail.height as usize
        );
        assert!(!thumbnail.placeholder);
    }

    #[test]
    fn inline_image_cache_round_trips_through_binary_header() {
        let thumbnail =
            decode_and_dither_fit_within(&encode_flat_rgba_png(40, 20), "image/png", 100, 100)
                .unwrap();
        let bytes = write_inline_image_cache_bytes(0xABCD_EF01_2345_6789, &thumbnail);
        let parsed = parse_inline_image_cache_bytes(&bytes, 0xABCD_EF01_2345_6789).unwrap();
        assert_eq!(parsed, thumbnail);
    }

    #[test]
    fn inline_image_cache_rejects_a_stale_fingerprint() {
        let thumbnail =
            decode_and_dither_fit_within(&encode_flat_rgba_png(40, 20), "image/png", 100, 100)
                .unwrap();
        let bytes = write_inline_image_cache_bytes(1, &thumbnail);
        assert!(parse_inline_image_cache_bytes(&bytes, 2).is_none());
    }

    #[test]
    fn inline_image_cache_caches_a_placeholder_on_extraction_failure_and_stops_retrying() {
        let root = temp_dir("inline-image");
        let cache = EpubImageCache::new(&root);
        // The book path does not exist on disk, so `extract_member` fails
        // with an I/O error -- this exercises `generate_bitmap`'s failure
        // path (corrupt/missing/oversized image) without needing a real
        // ZIP fixture, the same way the cover cache's own tests use a
        // `BookFormat::Text` book to sidestep needing one.
        let book = sample_book("/sdcard/BOOKS/does-not-exist.epub");
        let href = "OEBPS/images/fig1.jpg".to_string();

        assert!(cache.is_missing(&book, &href, 100, 100));
        let generated = cache.generate_bitmap(&book, &href, 100, 100);
        assert!(generated.placeholder);
        // The failure itself is now cached, so a second lookup is a cache
        // hit -- the expensive (here: doomed-to-fail) worker is not run
        // again on every subsequent page visit.
        assert!(!cache.is_missing(&book, &href, 100, 100));
        assert_eq!(
            cache.load_cached_bitmap(&book, &href, 100, 100),
            Some(generated)
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn inline_image_cache_file_names_fit_fat_8_3() {
        // The device's FAT volume has no long-file-name support: a longer
        // stem fails every write with EINVAL, so nothing is ever cached.
        let cache = EpubImageCache::new("/sdcard/RUSTMIX/CACHE");
        let book = sample_book("/sdcard/BOOKS/A.EPUB");
        for (href, width, height) in [("OEBPS/cover.jpg", 752, 663), ("x.png", 1, 1)] {
            let path = cache.cache_path(&book, href, width, height);
            let stem = path.file_stem().unwrap().to_str().unwrap();
            assert!(stem.len() <= 8, "{stem}");
            assert_eq!(path.extension().unwrap(), "EPI");
        }
    }

    #[test]
    fn inline_image_cache_pump_pending_generates_at_most_one_entry_per_call() {
        let root = temp_dir("inline-image-pump");
        let cache = EpubImageCache::new(&root);
        let book = sample_book("/sdcard/BOOKS/does-not-exist.epub");
        let hrefs = vec!["a.jpg".to_string(), "b.jpg".to_string()];

        let (first_href, first_thumbnail) = cache.pump_pending(&book, &hrefs, 100, 100).unwrap();
        assert_eq!(first_href, hrefs[0]);
        assert!(first_thumbnail.placeholder);
        assert!(!cache.is_missing(&book, &hrefs[0], 100, 100));
        assert!(cache.is_missing(&book, &hrefs[1], 100, 100));

        let (second_href, _) = cache.pump_pending(&book, &hrefs, 100, 100).unwrap();
        assert_eq!(second_href, hrefs[1]);
        assert!(cache.pump_pending(&book, &hrefs, 100, 100).is_none());

        let _ = fs::remove_dir_all(&root);
    }
}
