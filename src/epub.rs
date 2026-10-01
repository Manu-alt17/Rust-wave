// rustmix-wave=v0.17.0-parser-doc-repair-v2
// rustmix-wave=epub-xml-attribute-tokenizer-repair-ready
// rustmix-wave=epub-parser-stack-isolation-ready
// rustmix-wave=epub-chapter-aware-presentation-ready
// rustmix-wave=epub-watchdog-memory-pressure-repair-ready
//! Bounded reflowable EPUB reader foundation.
//!
//! The embedded target keeps EPUB processing deliberately small and explicit:
//! ZIP central-directory parsing is bounded, `META-INF/container.xml` selects
//! one OPF package, the manifest and spine are parsed without a general XML DOM,
//! XHTML is flattened into reflowable UTF-8 text, and EPUB3 navigation or EPUB2
//! NCX records become a compact table of contents. Images, CSS layout and
//! interactive links remain deferred.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs::File,
    io::{Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::Instant,
};

use miniz_oxide::inflate::decompress_to_vec_with_limit;

/// The `.EPX` file [`EpubDocument::text_window`] read last, kept open for
/// the next read: opening a file on FAT walks its directory, and the cache
/// directory holds several files per book. A single slot for the whole
/// firmware, so however many documents the Reader keeps parked, at most one
/// file stays open (the card is mounted with five descriptors).
static KEPT_TEXT_FILE: Mutex<Option<(PathBuf, File)>> = Mutex::new(None);

/// Close the kept `.EPX` handle. Called before an `.EPX` is written and
/// whenever the library is scanned again (the Wi-Fi portal may have deleted
/// or replaced files since), so a read never goes through a handle on a file
/// that has been replaced.
pub fn release_kept_text_file() {
    *KEPT_TEXT_FILE
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
}

/// Fill `buffer` from `offset`, stopping early only at the end of the file.
/// Returns the number of bytes read.
fn read_at(file: &mut File, offset: u64, buffer: &mut [u8]) -> Result<usize, String> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("EPUB cache seek failed: {error}"))?;
    crate::sd_io::read_full(file, buffer)
        .map_err(|error| format!("EPUB cache read failed: {error}"))
}

/// Maximum EPUB archive bytes accepted from removable storage.
pub const EPUB_ARCHIVE_BYTES_LIMIT: usize = 16 * 1024 * 1024;
/// Maximum central-directory records accepted from one EPUB.
pub const EPUB_ARCHIVE_ENTRY_LIMIT: usize = 4096;
/// Maximum compressed bytes extracted for one EPUB member.
pub const EPUB_MEMBER_COMPRESSED_LIMIT: usize = 2 * 1024 * 1024;
/// Maximum decompressed bytes extracted for one EPUB member.
pub const EPUB_MEMBER_UNCOMPRESSED_LIMIT: usize = 4 * 1024 * 1024;
/// Maximum flattened reflowable text retained for one EPUB, whether resident
/// in RAM (a freshly parsed book, still awaiting its background `.EPX` write)
/// or backed by its `.EPX` cache file on SD (a reopened book: the Reader
/// seeks/reads windows from that file instead of holding the whole text in
/// RAM, the same way the TXT reader already streams pages from disk). Raised
/// from the original 2 MiB RAM-only ceiling now that a reopened book no
/// longer needs to fit in RAM at all; the on-device PSRAM headroom (8 MiB
/// octal PSRAM on the Waveshare ESP32-S3-WROOM-1-N16R8) still bounds a
/// *fresh, uncached* open, since that one pass has to hold the flattened text
/// in RAM until the deferred background write lands it on SD.
pub const EPUB_REFLOW_TEXT_LIMIT: usize = 8 * 1024 * 1024;
/// Most spine records (chapter files) one EPUB may have. A book with more is
/// refused with an error rather than silently cut short: books split per
/// page by some converters reach a few hundred, so this leaves room.
pub const EPUB_SPINE_LIMIT: usize = 2048;
/// Maximum TOC records rendered by the Reader UI.
pub const EPUB_TOC_LIMIT: usize = 128;
/// Dedicated parser-worker stack budget. Real EPUB DEFLATE and XHTML work
/// must not run on the 16 KB firmware main task.
pub const EPUB_PARSER_WORKER_STACK_BYTES: usize = 64 * 1024;
/// Lightweight OPF-title worker stack budget. Library scans only read bounded
/// ZIP metadata and must not reserve the full parser stack for each title.
pub const EPUB_TITLE_WORKER_STACK_BYTES: usize = 32 * 1024;
/// Cover-extraction worker stack budget. Only ZIP central-directory parsing
/// and one member's DEFLATE expansion run here — no XHTML flattening — so
/// this sits between the title and full-parser budgets.
pub const EPUB_COVER_WORKER_STACK_BYTES: usize = 48 * 1024;
/// Maximum bytes accepted for one extracted cover image. PNG covers decode
/// at full resolution (no native scaled decoding, unlike JPEG), so this
/// bounds the largest buffer the cover-thumbnail pipeline decodes on
/// PSRAM-limited hardware.
pub const EPUB_COVER_BYTES_LIMIT: usize = 3 * 1024 * 1024;
/// Maximum inline `<img>` references retained per EPUB. Bounded the same way
/// TOC/manifest/spine records already are -- a heavily illustrated technical
/// book or novel plausibly has dozens of figures; comics/fixed-layout EPUBs
/// (deferred, see `docs/KNOWN_ISSUES.md`'s EPUB scope note) could have far
/// more, so this also acts as a soft signal that a book past the limit is
/// not this reader's target content. Images past the limit are silently
/// dropped from the flattened text (as if the `<img>` tag were never
/// there), matching the fallback every other size-bounded EPUB structure in
/// this module already uses.
pub const EPUB_IMAGE_LIMIT: usize = 64;
/// Sentinel character standing in for one inline `<img>` in the flattened
/// reflowable text, from the Unicode Private Use Area so it can never
/// collide with real book content. Exactly one [`char`] occupies exactly one
/// slot in the same byte-offset space `EpubChapter`/TOC/bookmarks already
/// index into -- an image is "one more character" to every offset-based
/// mechanism that already exists, including this document's own `.EPX` SD
/// cache, rather than needing a parallel indexing scheme. The Reader's
/// text-normalization pass (`push_normalized_character` in `reader.rs`)
/// passes it through untouched, pagination reserves page space for it, and
/// rendering blits the decoded image into that space.
pub const EPUB_IMAGE_SENTINEL: char = '\u{E000}';

/// One reflowable EPUB TOC destination. `text_offset` is an offset into the
/// flattened UTF-8 text buffer retained by [`EpubDocument`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpubTocEntry {
    pub label: String,
    pub text_offset: u64,
    pub spine_index: usize,
}

/// One readable spine chapter retained alongside flattened EPUB text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpubChapter {
    /// Sequential readable chapter number exposed by the Reader UI.
    pub number: usize,
    pub label: String,
    pub text_offset: u64,
    pub text_end_offset: u64,
    pub spine_index: usize,
}

/// One inline `<img>` reference captured while flattening EPUB spine XHTML to
/// reflowable text. `text_offset` is the byte offset, in the whole book's
/// flattened text, of the single [`EPUB_IMAGE_SENTINEL`] character this
/// image occupies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpubImage {
    /// Archive member path, resolved against the owning spine item's
    /// directory the same way spine hrefs already are
    /// (`normalize_archive_path`) -- ready to pass to `ZipArchive::extract`
    /// once a later change actually decodes it.
    pub href: String,
    /// `alt` attribute text, if present; empty when the source EPUB omitted
    /// it or left it blank.
    pub alt: String,
    pub text_offset: u64,
    pub spine_index: usize,
    /// Source pixel size read from the image header while the book is
    /// opened (see `ZipArchive::probe_image_size`), so pagination can
    /// reserve space matching the image's real aspect ratio before anything
    /// is decoded. `0` when the header could not be read (missing member,
    /// unsupported format); pagination then falls back to a fixed box.
    pub width: u32,
    pub height: u32,
}

/// Where one [`EpubDocument`]'s flattened text physically lives.
///
/// A freshly parsed book (`open_epub`) starts out `Resident`: the whole
/// flattened text is a `String` in RAM, exactly as before this type existed.
/// Once the Reader's background tick persists that text to its `.EPX` cache
/// file on SD, the document is converted to `OnDisk` and the RAM copy is
/// dropped; a *reopened* book (cache hit) is `OnDisk` from the start and
/// never materializes the full text in RAM at all. Either way, callers read
/// windows of text through [`EpubDocument::text_window`] rather than
/// matching on this enum directly.
#[derive(Clone, Debug, Eq, PartialEq)]
enum EpubTextStore {
    Resident(String),
    OnDisk { path: PathBuf, body_offset: u64 },
}

/// One bounded, reflowable EPUB book retained while the Reader session is open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpubDocument {
    pub title: String,
    text: EpubTextStore,
    text_len: u64,
    pub toc: Vec<EpubTocEntry>,
    pub chapters: Vec<EpubChapter>,
    pub images: Vec<EpubImage>,
    pub spine_count: usize,
}

impl EpubDocument {
    #[must_use]
    pub fn text_size_bytes(&self) -> u64 {
        self.text_len
    }

    /// Resolve the readable chapter containing one flattened UTF-8 byte
    /// offset. `chapters` is built in spine order with strictly increasing
    /// offsets (see `open_epub`), so a binary search finds it directly
    /// instead of scanning from the start on every page turn.
    #[must_use]
    pub fn chapter_for_offset(&self, offset: u64) -> Option<&EpubChapter> {
        let index = self
            .chapters
            .partition_point(|chapter| chapter.text_end_offset <= offset);
        if let Some(chapter) = self.chapters.get(index) {
            if offset >= chapter.text_offset {
                return Some(chapter);
            }
        }
        // `offset` lands exactly at the book's end, which only the last
        // chapter's `text_end_offset` can equal -- the search above skips
        // past it, since `text_end_offset <= offset` also holds there.
        self.chapters.last().filter(|chapter| {
            offset == chapter.text_end_offset && chapter.text_end_offset == self.text_size_bytes()
        })
    }

    /// The full flattened text, when still resident in RAM (a fresh,
    /// not-yet-persisted parse). `None` once the document has been converted
    /// to [`EpubTextStore::OnDisk`] — callers past that point must go through
    /// [`EpubDocument::text_window`] instead.
    #[must_use]
    pub fn resident_text(&self) -> Option<&str> {
        match &self.text {
            EpubTextStore::Resident(text) => Some(text),
            EpubTextStore::OnDisk { .. } => None,
        }
    }

    /// Read the raw flattened-text bytes in `[start, end)` (both clamped to
    /// the document's actual length), regardless of whether they currently
    /// live in RAM or on SD. Returned bytes are not adjusted to UTF-8
    /// character boundaries — callers that need that (Reader pagination)
    /// still do it themselves, the same way whether the slice came from RAM
    /// or from a fresh SD read.
    pub fn text_window(&self, start: usize, end: usize) -> Result<Cow<'_, [u8]>, String> {
        let text_len = usize::try_from(self.text_len).unwrap_or(usize::MAX);
        let end = end.min(text_len);
        let start = start.min(end);
        match &self.text {
            EpubTextStore::Resident(text) => Ok(Cow::Borrowed(&text.as_bytes()[start..end])),
            EpubTextStore::OnDisk { path, body_offset } => {
                let mut kept = KEPT_TEXT_FILE
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if !matches!(kept.as_ref(), Some((kept_path, _)) if kept_path == path) {
                    let file = File::open(path)
                        .map_err(|error| format!("EPUB cache read failed: {error}"))?;
                    *kept = Some((path.clone(), file));
                }
                let Some((_, file)) = kept.as_mut() else {
                    return Err("EPUB cache handle missing".into());
                };
                let mut buffer = vec![0_u8; end - start];
                let result = read_at(file, body_offset + start as u64, &mut buffer);
                if result.is_err() {
                    // Open the file afresh next time.
                    *kept = None;
                }
                let filled = result?;
                if filled < buffer.len() {
                    log::warn!(
                        "rustmix-wave=epub-cache-short-read path={} start={start} wanted={} got={filled}",
                        path.display(),
                        buffer.len()
                    );
                }
                buffer.truncate(filled);
                Ok(Cow::Owned(buffer))
            }
        }
    }

    /// Convert an `.EPX`-cache-hit document's parsed header/metadata into a
    /// full [`EpubDocument`] backed by that same file, without ever reading
    /// its (potentially large) body into RAM.
    #[must_use]
    pub fn from_cache_body(
        title: String,
        toc: Vec<EpubTocEntry>,
        chapters: Vec<EpubChapter>,
        images: Vec<EpubImage>,
        spine_count: usize,
        cache_path: PathBuf,
        body_offset: u64,
        text_len: u64,
    ) -> Self {
        Self {
            title,
            text: EpubTextStore::OnDisk {
                path: cache_path,
                body_offset,
            },
            text_len,
            toc,
            chapters,
            images,
            spine_count,
        }
    }

    /// Construct a resident (RAM-backed) document directly. Test-only: real
    /// callers always go through [`open_epub`] or [`EpubDocument::from_cache_body`].
    #[cfg(test)]
    pub(crate) fn from_resident_for_test(
        title: String,
        text: String,
        toc: Vec<EpubTocEntry>,
        chapters: Vec<EpubChapter>,
        images: Vec<EpubImage>,
        spine_count: usize,
    ) -> Self {
        let text_len = text.len() as u64;
        Self {
            title,
            text: EpubTextStore::Resident(text),
            text_len,
            toc,
            chapters,
            images,
            spine_count,
        }
    }

    /// Drop this document's resident RAM copy of the flattened text now that
    /// it has been durably written to `cache_path` (at `body_offset` within
    /// that file), and read future text windows from there instead — freeing
    /// up to [`EPUB_REFLOW_TEXT_LIMIT`] bytes of RAM for the rest of the
    /// reading session. A no-op (returns `self` unchanged) if the document is
    /// already `OnDisk`.
    #[must_use]
    pub fn into_on_disk(self, cache_path: PathBuf, body_offset: u64) -> Self {
        Self {
            text: EpubTextStore::OnDisk {
                path: cache_path,
                body_offset,
            },
            ..self
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ZipEntry {
    name: String,
    flags: u16,
    method: u16,
    compressed_size: usize,
    uncompressed_size: usize,
    local_header_offset: usize,
}

/// Archives up to this size are read into RAM in one sequential read;
/// larger ones are read member by member. See [`ZipArchive`].
const ZIP_IN_MEMORY_LIMIT: u64 = 2 * 1024 * 1024;

/// Compressed bytes read to probe one image member's header (see
/// [`ZipArchive::probe_image_size`]) when the archive is not in RAM.
const IMAGE_HEADER_PROBE_COMPRESSED_BYTES: usize = 64 * 1024;

/// ZIP reader for EPUB archives.
///
/// Small archives are read into RAM with one sequential read: an earlier
/// revision that always seeked+read each member individually measured no
/// faster on this SD/FAT stack for a typical 48-chapter book, because
/// per-seek latency dominates, and one large read is the safer default.
///
/// Archives above [`ZIP_IN_MEMORY_LIMIT`] keep only the central directory
/// in RAM and read each member on demand instead. Holding a whole
/// multi-megabyte omnibus in RAM *on top of* its flattened text (itself
/// growing by doubling while chapters are appended) exceeded the ~8 MB of
/// PSRAM on real hardware, and a failed allocation aborts the firmware.
struct ZipArchive {
    storage: ZipStorage,
    entries: Vec<ZipEntry>,
}

enum ZipStorage {
    InMemory(Vec<u8>),
    OnDisk {
        file: core::cell::RefCell<File>,
        len: usize,
    },
}

impl ZipStorage {
    fn len(&self) -> usize {
        match self {
            Self::InMemory(bytes) => bytes.len(),
            Self::OnDisk { len, .. } => *len,
        }
    }

    /// `len` bytes at `offset`, borrowed when in RAM, read from disk
    /// otherwise.
    fn range(&self, offset: usize, len: usize) -> Result<Cow<'_, [u8]>, String> {
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= self.len())
            .ok_or_else(|| "EPUB ZIP range exceeds archive".to_string())?;
        match self {
            Self::InMemory(bytes) => Ok(Cow::Borrowed(&bytes[offset..end])),
            Self::OnDisk { file, .. } => {
                read_file_range(&mut file.borrow_mut(), offset, len).map(Cow::Owned)
            }
        }
    }
}

impl ZipArchive {
    fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|error| format!("EPUB open failed: {error}"))?;
        let file_len = file
            .metadata()
            .map_err(|error| format!("EPUB open failed: {error}"))?
            .len();
        if file_len > EPUB_ARCHIVE_BYTES_LIMIT as u64 {
            return Err(format!(
                "EPUB archive exceeds {} byte limit",
                EPUB_ARCHIVE_BYTES_LIMIT
            ));
        }
        let storage = if file_len <= ZIP_IN_MEMORY_LIMIT {
            let mut bytes = vec![0_u8; file_len as usize];
            let read = crate::sd_io::read_full(&mut file, &mut bytes)
                .map_err(|error| format!("EPUB open failed: {error}"))?;
            bytes.truncate(read);
            ZipStorage::InMemory(bytes)
        } else {
            ZipStorage::OnDisk {
                file: core::cell::RefCell::new(file),
                len: file_len as usize,
            }
        };
        let archive_len = storage.len();
        let tail_len = archive_len.min(65_557);
        let tail_start = archive_len - tail_len;
        let tail = storage.range(tail_start, tail_len)?;
        let eocd = find_eocd(&tail).ok_or_else(|| "EPUB ZIP end record missing".to_string())?;
        let entry_count = read_u16(&tail, eocd + 10)? as usize;
        let central_size = read_u32(&tail, eocd + 12)? as usize;
        let central_offset = read_u32(&tail, eocd + 16)? as usize;
        if entry_count > EPUB_ARCHIVE_ENTRY_LIMIT {
            return Err(format!("EPUB ZIP has too many entries: {entry_count}"));
        }
        let central_end = central_offset
            .checked_add(central_size)
            .ok_or_else(|| "EPUB ZIP directory overflow".to_string())?;
        if central_end > archive_len {
            return Err("EPUB ZIP directory exceeds archive".into());
        }
        let directory = if central_offset >= tail_start {
            let start = central_offset - tail_start;
            Cow::Owned(tail[start..start + central_size].to_vec())
        } else {
            storage.range(central_offset, central_size)?
        };
        let entries = parse_central_entries(&directory, 0, directory.len(), entry_count)?;
        drop(directory);
        drop(tail);
        Ok(Self { storage, entries })
    }

    fn entry(&self, name: &str) -> Option<&ZipEntry> {
        find_entry(&self.entries, name)
    }

    /// Compressed payload of one member, located through its local header,
    /// truncated to `max_len` bytes when given (header probing needs only a
    /// prefix).
    fn compressed_bytes(
        &self,
        entry: &ZipEntry,
        max_len: Option<usize>,
    ) -> Result<Cow<'_, [u8]>, String> {
        let offset = entry.local_header_offset;
        let local_header = self.storage.range(offset, 30)?;
        let data_start = offset + local_data_offset(&local_header, entry)?;
        let data_end = data_start
            .checked_add(entry.compressed_size)
            .ok_or_else(|| "EPUB ZIP member overflow".to_string())?;
        if data_end > self.storage.len() {
            return Err(format!("EPUB ZIP member exceeds archive: {}", entry.name));
        }
        let len = max_len.map_or(entry.compressed_size, |max| max.min(entry.compressed_size));
        self.storage.range(data_start, len)
    }

    /// Pixel dimensions of one JPEG/PNG member, read from its header only:
    /// a stored member is parsed in place and a deflated one is inflated just
    /// far enough to reach the header, so probing every inline image while
    /// opening a book costs a bounded partial inflate each instead of full
    /// decodes.
    fn probe_image_size(&self, name: &str) -> Option<(u32, u32)> {
        let entry = self.entry(name)?;
        if entry.flags & 0x0001 != 0 {
            return None;
        }
        let compressed = self
            .compressed_bytes(entry, Some(IMAGE_HEADER_PROBE_COMPRESSED_BYTES))
            .ok()?;
        match entry.method {
            0 => image_header_size(&compressed),
            8 => {
                let prefix =
                    match decompress_to_vec_with_limit(&compressed, IMAGE_HEADER_PROBE_BYTES) {
                        Ok(output) => output,
                        Err(error) => error.output,
                    };
                image_header_size(&prefix)
            }
            _ => None,
        }
    }

    fn extract(&self, name: &str) -> Result<Vec<u8>, String> {
        let entry = self
            .entry(name)
            .ok_or_else(|| format!("EPUB member missing: {name}"))?;
        check_entry_limits(entry)?;
        inflate_entry(entry, &self.compressed_bytes(entry, None)?)
    }
}

/// Parse `entry_count` central-directory records from `bytes[start..central_end]`.
fn parse_central_entries(
    bytes: &[u8],
    start: usize,
    central_end: usize,
    entry_count: usize,
) -> Result<Vec<ZipEntry>, String> {
    let mut entries = Vec::new();
    let mut cursor = start;
    for _ in 0..entry_count {
        if read_u32(bytes, cursor)? != 0x0201_4B50 {
            return Err("EPUB ZIP central record signature mismatch".into());
        }
        let flags = read_u16(bytes, cursor + 8)?;
        let method = read_u16(bytes, cursor + 10)?;
        let compressed_size = read_u32(bytes, cursor + 20)? as usize;
        let uncompressed_size = read_u32(bytes, cursor + 24)? as usize;
        let name_len = read_u16(bytes, cursor + 28)? as usize;
        let extra_len = read_u16(bytes, cursor + 30)? as usize;
        let comment_len = read_u16(bytes, cursor + 32)? as usize;
        let local_header_offset = read_u32(bytes, cursor + 42)? as usize;
        let name_start = cursor + 46;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| "EPUB ZIP filename overflow".to_string())?;
        if name_end > central_end {
            return Err("EPUB ZIP filename exceeds directory".into());
        }
        let name = String::from_utf8_lossy(&bytes[name_start..name_end]).replace('\\', "/");
        entries.push(ZipEntry {
            name,
            flags,
            method,
            compressed_size,
            uncompressed_size,
            local_header_offset,
        });
        cursor = name_end
            .checked_add(extra_len)
            .and_then(|value| value.checked_add(comment_len))
            .ok_or_else(|| "EPUB ZIP central record overflow".to_string())?;
        if cursor > central_end {
            return Err("EPUB ZIP central record exceeds directory".into());
        }
    }
    Ok(entries)
}

/// Exact-name lookup first, then an ASCII case-insensitive fallback.
fn find_entry<'a>(entries: &'a [ZipEntry], name: &str) -> Option<&'a ZipEntry> {
    entries.iter().find(|entry| entry.name == name).or_else(|| {
        entries
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
    })
}

fn check_entry_limits(entry: &ZipEntry) -> Result<(), String> {
    if entry.flags & 0x0001 != 0 {
        return Err(format!(
            "Encrypted EPUB member is unsupported: {}",
            entry.name
        ));
    }
    if entry.compressed_size > EPUB_MEMBER_COMPRESSED_LIMIT {
        return Err(format!(
            "EPUB member compressed size is too large: {}",
            entry.name
        ));
    }
    if entry.uncompressed_size > EPUB_MEMBER_UNCOMPRESSED_LIMIT {
        return Err(format!(
            "EPUB member expanded size is too large: {}",
            entry.name
        ));
    }
    Ok(())
}

/// Byte distance from a member's local header to its compressed data.
/// `local_header` starts at the header's signature and must hold at least
/// its fixed 30-byte part.
fn local_data_offset(local_header: &[u8], entry: &ZipEntry) -> Result<usize, String> {
    if read_u32(local_header, 0)? != 0x0403_4B50 {
        return Err(format!("EPUB local ZIP header mismatch: {}", entry.name));
    }
    let name_len = read_u16(local_header, 26)? as usize;
    let extra_len = read_u16(local_header, 28)? as usize;
    30_usize
        .checked_add(name_len)
        .and_then(|value| value.checked_add(extra_len))
        .ok_or_else(|| "EPUB ZIP data offset overflow".to_string())
}

fn inflate_entry(entry: &ZipEntry, compressed: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = match entry.method {
        0 => compressed.to_vec(),
        // Bounded while inflating, not only after: the central directory's
        // declared size is checked up front (`check_entry_limits`) but can
        // lie, and an unbounded inflate of a malformed or hostile member
        // could exhaust PSRAM before the size check below ever ran.
        8 => decompress_to_vec_with_limit(compressed, EPUB_MEMBER_UNCOMPRESSED_LIMIT)
            .map_err(|error| format!("EPUB deflate failed for {}: {error:?}", entry.name))?,
        method => {
            return Err(format!(
                "Unsupported EPUB compression method {method} for {}",
                entry.name
            ))
        }
    };
    if output.len() > EPUB_MEMBER_UNCOMPRESSED_LIMIT {
        return Err(format!("EPUB member expanded beyond limit: {}", entry.name));
    }
    if entry.uncompressed_size != 0 && output.len() != entry.uncompressed_size {
        return Err(format!("EPUB member size mismatch: {}", entry.name));
    }
    // The inflate starts at twice the compressed size and doubles from
    // there: an image, which deflate cannot shrink, kept twice its size
    // allocated for as long as it was being decoded.
    output.shrink_to_fit();
    Ok(output)
}

/// Inflated prefix probed for an image header. PNG's IHDR always sits in
/// the first 33 bytes; a JPEG's SOF marker follows its APPn segments, which
/// an embedded EXIF thumbnail or ICC profile can push well past the first
/// few KB, so this leaves generous room while staying a small fraction of a
/// typical illustration.
const IMAGE_HEADER_PROBE_BYTES: usize = 128 * 1024;

/// Pixel dimensions from a PNG IHDR or the first JPEG SOFn marker.
pub fn image_header_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
        let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
        return (width > 0 && height > 0).then_some((width, height));
    }
    if !bytes.starts_with(&[0xFF, 0xD8]) {
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
        let is_sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let segment = bytes.get(cursor + 4..cursor + 9)?;
            let height = u32::from(u16::from_be_bytes([segment[1], segment[2]]));
            let width = u32::from(u16::from_be_bytes([segment[3], segment[4]]));
            return (width > 0 && height > 0).then_some((width, height));
        }
        if length < 2 {
            return None;
        }
        cursor += 2 + length;
    }
    None
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManifestItem {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

/// Parse one EPUB on a short-lived dedicated worker stack. The Reader keeps
/// its existing synchronous staged-loading contract, while archive parsing,
/// DEFLATE expansion and XHTML flattening no longer consume the firmware main
/// task's 16 KB stack budget.
pub fn open_epub_on_worker(path: impl AsRef<Path>) -> Result<EpubDocument, String> {
    let path = path.as_ref().to_path_buf();
    log::info!(
        "rustmix-wave=epub-parser-worker status=starting stack-bytes={}",
        EPUB_PARSER_WORKER_STACK_BYTES
    );
    // This worker only reads the SD card and computes in RAM/PSRAM, never
    // touches flash directly and never runs from an ISR, so its stack is
    // safe to place in PSRAM (see `runtime_worker::run_named_worker_in_psram`'s
    // doc comment for the full reasoning). That matters here specifically:
    // internal SRAM is shared with Wi-Fi/lwIP, and on this hardware merely
    // being Wi-Fi-connected has been observed to shrink the largest
    // contiguous internal block below this worker's own stack size, while
    // the 8 MB of PSRAM sits almost untouched.
    #[cfg(target_os = "espidf")]
    let psram_cfg_status = {
        let mut cfg = unsafe { esp_idf_svc::sys::esp_pthread_get_default_config() };
        cfg.stack_alloc_caps =
            esp_idf_svc::sys::MALLOC_CAP_SPIRAM | esp_idf_svc::sys::MALLOC_CAP_8BIT;
        unsafe { esp_idf_svc::sys::esp_pthread_set_cfg(&cfg) }
    };
    #[cfg(target_os = "espidf")]
    if psram_cfg_status != 0 {
        log::warn!(
            "rustmix-wave=epub-parser-worker status=psram-cfg-failed error-code={psram_cfg_status}"
        );
    }
    let worker = std::thread::Builder::new()
        .name("epub-parser".into())
        .stack_size(EPUB_PARSER_WORKER_STACK_BYTES)
        .spawn(move || open_epub(path));
    #[cfg(target_os = "espidf")]
    {
        let restore = unsafe { esp_idf_svc::sys::esp_pthread_get_default_config() };
        let restore_status = unsafe { esp_idf_svc::sys::esp_pthread_set_cfg(&restore) };
        if restore_status != 0 {
            log::warn!(
                "rustmix-wave=epub-parser-worker status=psram-cfg-restore-failed error-code={restore_status}"
            );
        }
    }
    let worker = worker.map_err(|error| {
        let message = format!("EPUB parser worker start failed: {error}");
        log::warn!("rustmix-wave=epub-parser-worker status=start-failed error={message}");
        message
    })?;
    let result = worker.join().map_err(|_| {
        let message = "EPUB parser worker panicked".to_string();
        log::warn!("rustmix-wave=epub-parser-worker status=panicked");
        message
    })?;
    match &result {
        Ok(document) => log::info!(
            "rustmix-wave=epub-parser-worker status=completed spine-items={} toc-entries={} text-bytes={}",
            document.spine_count,
            document.toc.len(),
            document.text_size_bytes()
        ),
        Err(error) => log::warn!("rustmix-wave=epub-parser-worker status=failed error={error}"),
    }
    result
}

/// Read only the OPF title on a lightweight bounded worker stack. Library scans
/// remain safe on the firmware main task and fall back to the FAT filename when
/// metadata cannot be read.
///
/// The stack comes from PSRAM: once Wi-Fi is up and a few books are warmed,
/// the largest free internal block sits just under 32 KB, so an internal
/// stack of this size failed to spawn ("pthread: Failed to create task!")
/// for every uncached title on each Library visit.
pub fn read_epub_title_on_worker(path: impl AsRef<Path>) -> Result<String, String> {
    let path = path.as_ref().to_path_buf();
    crate::runtime_worker::run_named_worker_in_psram(
        "epub-title",
        EPUB_TITLE_WORKER_STACK_BYTES,
        move || read_epub_title(path),
    )
    .map_err(|error| format!("EPUB title worker: {error}"))
}

/// Read one OPF metadata title without flattening the spine.
#[inline(never)]
pub fn read_epub_title(path: impl AsRef<Path>) -> Result<String, String> {
    let archive = ZipArchive::open(path)?;
    let (_, package, _) = epub_package(&archive)?;
    Ok(package_title(&package))
}

/// One EPUB cover image, extracted but not decoded: raw member bytes plus the
/// manifest's declared media type (a hint only — extraction does not trust it
/// over sniffing the bytes' own magic number, since manifests occasionally
/// lie).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpubCoverImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
}

/// Extract one EPUB's cover image on a short-lived bounded worker stack.
/// Returns `Ok(None)` when the book declares no cover (not an error: the
/// caller falls back to a placeholder thumbnail).
pub fn extract_cover_on_worker(path: impl AsRef<Path>) -> Result<Option<EpubCoverImage>, String> {
    let path = path.as_ref().to_path_buf();
    let worker = std::thread::Builder::new()
        .name("epub-cover".into())
        .stack_size(EPUB_COVER_WORKER_STACK_BYTES)
        .spawn(move || extract_cover(path))
        .map_err(|error| format!("EPUB cover worker start failed: {error}"))?;
    worker
        .join()
        .map_err(|_| "EPUB cover worker panicked".to_string())?
}

/// Locate and extract one EPUB's cover image member without flattening the
/// spine. Identifies the cover via, in order: the EPUB3
/// `properties="cover-image"` manifest hint, the EPUB2
/// `<meta name="cover" content="ID"/>` convention, then an `id="cover"`
/// manifest item as a last-resort fallback for malformed packages.
#[inline(never)]
pub fn extract_cover(path: impl AsRef<Path>) -> Result<Option<EpubCoverImage>, String> {
    let archive = ZipArchive::open(path)?;
    let (_, package, package_dir) = epub_package(&archive)?;
    let manifest = parse_manifest(&package)?;
    let Some(item) = find_cover_manifest_item(&package, &manifest) else {
        return Ok(None);
    };
    let member = normalize_archive_path(&package_dir, &item.href);
    let bytes = archive.extract(&member)?;
    if bytes.len() > EPUB_COVER_BYTES_LIMIT {
        return Err(format!(
            "EPUB cover exceeds {EPUB_COVER_BYTES_LIMIT} byte limit"
        ));
    }
    Ok(Some(EpubCoverImage {
        bytes,
        media_type: item.media_type.clone(),
    }))
}

/// Extract one arbitrary archive member's raw bytes on a short-lived bounded
/// worker stack, given an already-resolved path such as an
/// [`EpubImage::href`]. Reuses [`extract_cover_on_worker`]'s stack budget:
/// this is the same kind of work (ZIP central-directory lookup plus one
/// member's DEFLATE expansion, no XHTML flattening).
pub fn extract_member_on_worker(path: impl AsRef<Path>, href: &str) -> Result<Vec<u8>, String> {
    let path = path.as_ref().to_path_buf();
    let href = href.to_string();
    let worker = std::thread::Builder::new()
        .name("epub-member".into())
        .stack_size(EPUB_COVER_WORKER_STACK_BYTES)
        .spawn(move || extract_member(path, &href))
        .map_err(|error| format!("EPUB member worker start failed: {error}"))?;
    worker
        .join()
        .map_err(|_| "EPUB member worker panicked".to_string())?
}

/// Extract one arbitrary archive member's raw bytes by an already-resolved
/// path, bounded the same way [`extract_cover`] already is. Used by the
/// Reader to decode one inline image (see [`EpubImage`]) the first time it
/// is about to be shown.
///
/// Unlike [`ZipArchive::open`], this never reads the whole archive: it reads
/// the end-of-central-directory tail, the central directory itself, and then
/// only the one member's local header and payload. `ZipArchive`'s single
/// sequential read wins when a book open touches dozens of members, but an
/// inline image needs exactly one, and reading a multi-megabyte illustrated
/// EPUB off SD just to reach it was the dominant cost of showing a page with
/// an image on it.
#[inline(never)]
pub fn extract_member(path: impl AsRef<Path>, href: &str) -> Result<Vec<u8>, String> {
    let mut file =
        File::open(path.as_ref()).map_err(|error| format!("EPUB open failed: {error}"))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("EPUB open failed: {error}"))?
        .len();
    if file_len > EPUB_ARCHIVE_BYTES_LIMIT as u64 {
        return Err(format!(
            "EPUB archive exceeds {} byte limit",
            EPUB_ARCHIVE_BYTES_LIMIT
        ));
    }
    let file_len = file_len as usize;
    let tail_len = file_len.min(65_557);
    let tail_start = file_len - tail_len;
    let tail = read_file_range(&mut file, tail_start, tail_len)?;
    let eocd = find_eocd(&tail).ok_or_else(|| "EPUB ZIP end record missing".to_string())?;
    let entry_count = read_u16(&tail, eocd + 10)? as usize;
    let central_size = read_u32(&tail, eocd + 12)? as usize;
    let central_offset = read_u32(&tail, eocd + 16)? as usize;
    if entry_count > EPUB_ARCHIVE_ENTRY_LIMIT {
        return Err(format!("EPUB ZIP has too many entries: {entry_count}"));
    }
    if central_offset
        .checked_add(central_size)
        .map_or(true, |end| end > file_len)
    {
        return Err("EPUB ZIP directory exceeds archive".into());
    }
    let directory = if central_offset >= tail_start {
        let start = central_offset - tail_start;
        tail[start..start + central_size].to_vec()
    } else {
        read_file_range(&mut file, central_offset, central_size)?
    };
    let entries = parse_central_entries(&directory, 0, directory.len(), entry_count)?;
    let entry = find_entry(&entries, href).ok_or_else(|| format!("EPUB member missing: {href}"))?;
    check_entry_limits(entry)?;
    if entry.compressed_size > EPUB_COVER_BYTES_LIMIT {
        return Err(format!(
            "EPUB image member exceeds {EPUB_COVER_BYTES_LIMIT} byte limit"
        ));
    }
    let local_header = read_file_range(&mut file, entry.local_header_offset, 30)?;
    let name_and_extra = local_data_offset(&local_header, entry)?;
    let data_start = entry
        .local_header_offset
        .checked_add(name_and_extra)
        .ok_or_else(|| "EPUB ZIP data offset overflow".to_string())?;
    if data_start
        .checked_add(entry.compressed_size)
        .map_or(true, |end| end > file_len)
    {
        return Err(format!("EPUB ZIP member exceeds archive: {}", entry.name));
    }
    let compressed = read_file_range(&mut file, data_start, entry.compressed_size)?;
    let bytes = inflate_entry(entry, &compressed)?;
    if bytes.len() > EPUB_COVER_BYTES_LIMIT {
        return Err(format!(
            "EPUB image member exceeds {EPUB_COVER_BYTES_LIMIT} byte limit"
        ));
    }
    Ok(bytes)
}

fn read_file_range(file: &mut File, offset: usize, len: usize) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(offset as u64))
        .map_err(|error| format!("EPUB seek failed: {error}"))?;
    let mut buffer = vec![0_u8; len];
    let read = crate::sd_io::read_full(file, &mut buffer)
        .map_err(|error| format!("EPUB read failed: {error}"))?;
    if read < len {
        return Err("EPUB read failed: archive ends early".into());
    }
    Ok(buffer)
}

fn find_cover_manifest_item<'a>(
    package: &str,
    manifest: &'a BTreeMap<String, ManifestItem>,
) -> Option<&'a ManifestItem> {
    if let Some(item) = manifest.values().find(|item| {
        item.properties
            .split_whitespace()
            .any(|value| value == "cover-image")
    }) {
        return Some(item);
    }
    if let Some(id) = first_meta_cover_id(package) {
        if let Some(item) = manifest.get(&id) {
            return Some(item);
        }
    }
    manifest
        .get("cover")
        .filter(|item| item.media_type.starts_with("image/"))
}

fn first_meta_cover_id(package: &str) -> Option<String> {
    open_tags(package, "meta")
        .into_iter()
        .find(|tag| attribute(tag, "name").as_deref() == Some("cover"))
        .and_then(|tag| attribute(tag, "content"))
}

fn epub_package(archive: &ZipArchive) -> Result<(String, String, String), String> {
    let container = utf8_member(archive, "META-INF/container.xml")?;
    let rootfile = first_open_tag(&container, "rootfile")
        .and_then(|tag| attribute(tag, "full-path"))
        .ok_or_else(|| "EPUB container rootfile missing".to_string())?;
    let package_path = normalize_archive_path("", &rootfile);
    let package = utf8_member(archive, &package_path)?;
    let package_dir = archive_parent(&package_path);
    Ok((package_path, package, package_dir))
}

fn package_title(package: &str) -> String {
    first_element_text(package, "title")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Untitled EPUB".into())
}

/// Open one EPUB archive and produce a bounded reflowable document.
#[inline(never)]
pub fn open_epub(path: impl AsRef<Path>) -> Result<EpubDocument, String> {
    let zip_open_started_at = Instant::now();
    let archive = ZipArchive::open(path)?;
    log::info!(
        "rustmix-wave=epub-parse-timing stage=zip-open elapsed-ms={} entries={}",
        zip_open_started_at.elapsed().as_millis(),
        archive.entries.len()
    );
    let (_, package, package_dir) = epub_package(&archive)?;
    let title = package_title(&package);

    let manifest = parse_manifest(&package)?;
    let spine_ids = parse_spine_ids(&package)?;
    if spine_ids.is_empty() {
        return Err("EPUB spine is empty".into());
    }
    let spine_started_at = Instant::now();

    let mut text = String::new();
    let mut chapter_offsets = BTreeMap::new();
    let mut chapter_labels = Vec::new();
    let mut chapters = Vec::new();
    let mut images = Vec::new();
    for (spine_index, idref) in spine_ids.iter().enumerate() {
        let item = manifest
            .get(idref)
            .ok_or_else(|| format!("EPUB spine item missing from manifest: {idref}"))?;
        let member = normalize_archive_path(&package_dir, &item.href);
        let xhtml = utf8_member(&archive, &member)?;
        let member_dir = archive_parent(&member);
        let (chapter, flattened_images) = html_to_text_with_images(&xhtml, &member_dir);
        if chapter.trim().is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let offset = text.len() as u64;
        let label = fallback_chapter_label(&xhtml, spine_index);
        chapter_offsets.insert(member.clone(), (spine_index, offset));
        chapter_labels.push((spine_index, offset, label.clone()));
        text.push_str(chapter.trim());
        let text_end_offset = text.len() as u64;
        chapters.push(EpubChapter {
            number: chapters.len() + 1,
            label,
            text_offset: offset,
            text_end_offset,
            spine_index,
        });
        for image in flattened_images {
            if images.len() >= EPUB_IMAGE_LIMIT {
                break;
            }
            let (width, height) = archive.probe_image_size(&image.href).unwrap_or((0, 0));
            images.push(EpubImage {
                href: image.href,
                alt: image.alt,
                text_offset: offset + image.offset as u64,
                spine_index,
                width,
                height,
            });
        }
        if text.len() > EPUB_REFLOW_TEXT_LIMIT {
            return Err(format!(
                "EPUB reflow text exceeds {} byte limit",
                EPUB_REFLOW_TEXT_LIMIT
            ));
        }
    }
    if text.trim().is_empty() {
        return Err("EPUB spine did not contain readable text".into());
    }
    log::info!(
        "rustmix-wave=epub-parse-timing stage=spine-extract-and-flatten elapsed-ms={} chapters={} text-bytes={}",
        spine_started_at.elapsed().as_millis(),
        chapters.len(),
        text.len()
    );

    let toc_started_at = Instant::now();
    let mut toc = parse_navigation_toc(
        &archive,
        &package,
        &package_dir,
        &manifest,
        &chapter_offsets,
    )?;
    log::info!(
        "rustmix-wave=epub-parse-timing stage=navigation-toc elapsed-ms={}",
        toc_started_at.elapsed().as_millis()
    );
    if toc.is_empty() {
        toc = chapter_labels
            .into_iter()
            .take(EPUB_TOC_LIMIT)
            .map(|(spine_index, text_offset, label)| EpubTocEntry {
                label,
                text_offset,
                spine_index,
            })
            .collect();
    }
    dedupe_toc(&mut toc);
    toc.truncate(EPUB_TOC_LIMIT);
    let text_len = text.len() as u64;
    Ok(EpubDocument {
        title,
        text: EpubTextStore::Resident(text),
        text_len,
        toc,
        chapters,
        images,
        spine_count: spine_ids.len(),
    })
}

/// Every manifest item, however many: a few bytes each, and a chapter
/// listed past an arbitrary cutoff made the whole book fail to open ("spine
/// item missing from manifest") in books with many images or fonts.
fn parse_manifest(package: &str) -> Result<BTreeMap<String, ManifestItem>, String> {
    let mut manifest = BTreeMap::new();
    for tag in open_tags(package, "item") {
        let Some(id) = attribute(tag, "id") else {
            continue;
        };
        let Some(href) = attribute(tag, "href") else {
            continue;
        };
        let media_type = attribute(tag, "media-type").unwrap_or_default();
        let properties = attribute(tag, "properties").unwrap_or_default();
        manifest.insert(
            id.clone(),
            ManifestItem {
                id,
                href,
                media_type,
                properties,
            },
        );
    }
    if manifest.is_empty() {
        return Err("EPUB manifest is empty".into());
    }
    Ok(manifest)
}

fn parse_spine_ids(package: &str) -> Result<Vec<String>, String> {
    let ids: Vec<String> = open_tags(package, "itemref")
        .into_iter()
        .filter_map(|tag| attribute(tag, "idref"))
        .collect();
    if ids.len() > EPUB_SPINE_LIMIT {
        return Err(format!(
            "EPUB has {} chapter files, more than the {EPUB_SPINE_LIMIT} this reader opens",
            ids.len()
        ));
    }
    Ok(ids)
}

fn parse_navigation_toc(
    archive: &ZipArchive,
    package: &str,
    package_dir: &str,
    manifest: &BTreeMap<String, ManifestItem>,
    chapter_offsets: &BTreeMap<String, (usize, u64)>,
) -> Result<Vec<EpubTocEntry>, String> {
    if let Some(nav) = manifest.values().find(|item| {
        item.properties
            .split_whitespace()
            .any(|value| value == "nav")
    }) {
        let member = normalize_archive_path(package_dir, &nav.href);
        let nav_text = utf8_member(archive, &member)?;
        let base = archive_parent(&member);
        let toc = links_to_toc(toc_nav_region(&nav_text), &base, chapter_offsets);
        if !toc.is_empty() {
            return Ok(toc);
        }
    }

    let spine_toc = first_open_tag(package, "spine").and_then(|tag| attribute(tag, "toc"));
    let ncx = spine_toc
        .as_ref()
        .and_then(|id| manifest.get(id))
        .or_else(|| {
            manifest
                .values()
                .find(|item| item.media_type == "application/x-dtbncx+xml")
        });
    if let Some(ncx) = ncx {
        let member = normalize_archive_path(package_dir, &ncx.href);
        let ncx_text = utf8_member(archive, &member)?;
        let base = archive_parent(&member);
        return Ok(ncx_to_toc(&ncx_text, &base, chapter_offsets));
    }
    Ok(Vec::new())
}

/// The table of contents inside an EPUB 3 navigation document: the
/// `<nav epub:type="toc">` element. The same document usually also holds a
/// page list and landmarks, whose links would fill the Reader's contents
/// with page numbers. Without a marked `toc` nav, the whole document.
fn toc_nav_region(html: &str) -> &str {
    let mut cursor = 0;
    while let Some(start_rel) = html[cursor..].find("<nav") {
        let start = cursor + start_rel;
        let Some(open_end_rel) = html[start..].find('>') else {
            break;
        };
        let tag = &html[start + 1..start + open_end_rel];
        let open_end = start + open_end_rel + 1;
        // `attribute` matches local names, so "type" finds `epub:type`.
        let is_toc = attribute(tag, "type")
            .is_some_and(|value| value.split_whitespace().any(|word| word == "toc"))
            || attribute(tag, "role").is_some_and(|value| value == "doc-toc");
        if is_toc {
            let end = html[open_end..]
                .find("</nav>")
                .map_or(html.len(), |end| open_end + end);
            return &html[open_end..end];
        }
        cursor = open_end;
    }
    html
}

fn links_to_toc(
    html: &str,
    base: &str,
    chapter_offsets: &BTreeMap<String, (usize, u64)>,
) -> Vec<EpubTocEntry> {
    let mut toc = Vec::new();
    let mut cursor = 0;
    while toc.len() < EPUB_TOC_LIMIT {
        let Some(start_rel) = html[cursor..].find("<a") else {
            break;
        };
        let start = cursor + start_rel;
        let Some(open_end_rel) = html[start..].find('>') else {
            break;
        };
        let open_end = start + open_end_rel;
        let tag = &html[start + 1..open_end];
        let Some(href) = attribute(tag, "href") else {
            cursor = open_end + 1;
            continue;
        };
        let Some(close_rel) = html[open_end + 1..].find("</a>") else {
            break;
        };
        let close = open_end + 1 + close_rel;
        let label = html_to_text(&html[open_end + 1..close]);
        if let Some(entry) = toc_for_href(base, &href, label.trim(), chapter_offsets) {
            toc.push(entry);
        }
        cursor = close + 4;
    }
    toc
}

fn ncx_to_toc(
    ncx: &str,
    base: &str,
    chapter_offsets: &BTreeMap<String, (usize, u64)>,
) -> Vec<EpubTocEntry> {
    let mut toc = Vec::new();
    let mut cursor = 0;
    while toc.len() < EPUB_TOC_LIMIT {
        let Some(start_rel) = ncx[cursor..].find("<navPoint") else {
            break;
        };
        let start = cursor + start_rel;
        let open_end = ncx[start..]
            .find('>')
            .map_or(ncx.len(), |value| start + value + 1);
        // A point's own label and target come before its first nested point
        // (or its end). The next search starts right after its opening tag,
        // so nested points are read too: stopping at the first
        // `</navPoint>`, which closes the first child, skipped that child.
        let own_end = [
            ncx[open_end..].find("<navPoint"),
            ncx[open_end..].find("</navPoint>"),
        ]
        .into_iter()
        .flatten()
        .min()
        .map_or(ncx.len(), |value| open_end + value);
        let block = &ncx[open_end..own_end];
        let href = first_open_tag(block, "content").and_then(|tag| attribute(tag, "src"));
        let label = first_element_text(block, "text").unwrap_or_else(|| "Chapter".into());
        if let Some(href) = href {
            if let Some(entry) = toc_for_href(base, &href, label.trim(), chapter_offsets) {
                toc.push(entry);
            }
        }
        cursor = open_end;
    }
    toc
}

fn toc_for_href(
    base: &str,
    href: &str,
    label: &str,
    chapter_offsets: &BTreeMap<String, (usize, u64)>,
) -> Option<EpubTocEntry> {
    let member = normalize_archive_path(base, href);
    let (spine_index, text_offset) = chapter_offsets.get(&member).copied()?;
    Some(EpubTocEntry {
        label: if label.is_empty() {
            format!("Chapter {}", spine_index + 1)
        } else {
            label.to_string()
        },
        text_offset,
        spine_index,
    })
}

fn dedupe_toc(toc: &mut Vec<EpubTocEntry>) {
    let mut unique = Vec::new();
    for entry in toc.drain(..) {
        if unique.iter().any(|existing: &EpubTocEntry| {
            existing.text_offset == entry.text_offset && existing.label == entry.label
        }) {
            continue;
        }
        unique.push(entry);
    }
    *toc = unique;
}

fn fallback_chapter_label(xhtml: &str, spine_index: usize) -> String {
    for tag in ["h1", "h2", "h3", "title"] {
        if let Some(label) = first_element_text(xhtml, tag).filter(|value| !value.is_empty()) {
            return label;
        }
    }
    format!("Chapter {}", spine_index + 1)
}

fn utf8_member(archive: &ZipArchive, name: &str) -> Result<String, String> {
    let bytes = archive.extract(name)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn find_eocd(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 22 {
        return None;
    }
    let start = bytes.len().saturating_sub(65_557);
    (start..=bytes.len() - 22)
        .rev()
        .find(|offset| bytes.get(*offset..*offset + 4) == Some(&[0x50_u8, 0x4B, 0x05, 0x06][..]))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "EPUB ZIP truncated u16".to_string())?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "EPUB ZIP truncated u32".to_string())?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn archive_parent(path: &str) -> String {
    path.rsplit_once('/')
        .map_or("".into(), |(parent, _)| parent.into())
}

fn normalize_archive_path(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let decoded = percent_decode(href);
    let raw = if decoded.starts_with('/') || base.is_empty() {
        decoded.trim_start_matches('/').to_string()
    } else {
        format!("{base}/{decoded}")
    };
    let mut parts = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }
    parts.join("/")
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                output.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn first_open_tag<'a>(xml: &'a str, local_name: &str) -> Option<&'a str> {
    open_tags(xml, local_name).into_iter().next()
}

fn open_tags<'a>(xml: &'a str, local_name: &str) -> Vec<&'a str> {
    let mut tags = Vec::new();
    let mut cursor = 0;
    while cursor < xml.len() {
        let Some(start_rel) = xml[cursor..].find('<') else {
            break;
        };
        let start = cursor + start_rel;
        let Some(end_rel) = xml[start + 1..].find('>') else {
            break;
        };
        let end = start + 1 + end_rel;
        let tag = &xml[start + 1..end];
        let trimmed = tag.trim_start();
        if !trimmed.starts_with('/') && !trimmed.starts_with('!') && !trimmed.starts_with('?') {
            let name = trimmed
                .split(|character: char| character.is_whitespace() || character == '/')
                .next()
                .unwrap_or("");
            if name.rsplit(':').next() == Some(local_name) {
                tags.push(tag);
            }
        }
        cursor = end + 1;
    }
    tags
}

fn attribute(tag: &str, key: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut cursor = 0;

    // Skip the opening element name before scanning attribute tokens. Without
    // this boundary, `<rootfile full-path='...'>` consumes `full-path` while
    // recovering from the non-attribute `rootfile` token.
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'/' {
        cursor += 1;
    }

    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] == b'/' {
            break;
        }

        let start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && bytes[cursor] != b'='
            && bytes[cursor] != b'/'
        {
            cursor += 1;
        }
        let name = &tag[start..cursor];
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            while cursor < bytes.len()
                && !bytes[cursor].is_ascii_whitespace()
                && bytes[cursor] != b'/'
            {
                cursor += 1;
            }
            continue;
        }

        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || (bytes[cursor] != b'\'' && bytes[cursor] != b'"') {
            while cursor < bytes.len()
                && !bytes[cursor].is_ascii_whitespace()
                && bytes[cursor] != b'/'
            {
                cursor += 1;
            }
            continue;
        }

        let quote = bytes[cursor];
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let value = &tag[value_start..cursor];
        cursor += 1;
        if name.rsplit(':').next() == Some(key) {
            return Some(decode_entities(value));
        }
    }
    None
}

fn first_element_text(xml: &str, local_name: &str) -> Option<String> {
    let mut cursor = 0;
    while cursor < xml.len() {
        let Some(start_rel) = xml[cursor..].find('<') else {
            return None;
        };
        let start = cursor + start_rel;
        let Some(end_rel) = xml[start + 1..].find('>') else {
            return None;
        };
        let end = start + 1 + end_rel;
        let tag = &xml[start + 1..end];
        let trimmed = tag.trim_start();
        let name = trimmed
            .split(|character: char| character.is_whitespace() || character == '/')
            .next()
            .unwrap_or("");
        if !trimmed.starts_with('/') && name.rsplit(':').next() == Some(local_name) {
            let close = format!("</{name}>");
            if let Some(close_rel) = xml[end + 1..].find(&close) {
                return Some(html_to_text(&xml[end + 1..end + 1 + close_rel]));
            }
        }
        cursor = end + 1;
    }
    None
}

/// One `<img>` captured by [`flatten_html`], with its byte offset in that
/// call's *untrimmed* output buffer -- `flatten_html` itself adjusts this to
/// the trimmed string before returning, callers never see the untrimmed
/// value.
struct FlattenedImage {
    href: String,
    alt: String,
    offset: usize,
}

/// Convert XHTML into bounded, paragraph-aware reflowable UTF-8 text.
///
/// Callers pass either a full XHTML document or a small inner fragment (a TOC
/// link label, a heading's own text). When a `<body>` tag is present this
/// skips straight to it, so `<head>` metadata, `<title>` and any `<style>`/
/// `<script>` content never leaks into reflowed body text. Fragments without
/// a `<body>` tag are processed as-is.
pub fn html_to_text(html: &str) -> String {
    flatten_html(html, None).0
}

/// Same flattening as [`html_to_text`], but also captures each `<img>`'s
/// resolved archive href and alt text, emitting [`EPUB_IMAGE_SENTINEL`] into
/// the returned text at each one's position. `base` resolves `src` the same
/// way spine hrefs already are (`normalize_archive_path`). Only
/// [`open_epub`]'s spine loop calls this -- TOC labels and chapter-title
/// fallbacks keep using plain [`html_to_text`], since an `<img>` inside a
/// heading or nav link is not something those short fragments need to track.
fn html_to_text_with_images(html: &str, base: &str) -> (String, Vec<FlattenedImage>) {
    flatten_html(html, Some(base))
}

/// Shared implementation behind [`html_to_text`] and
/// [`html_to_text_with_images`]. `image_base` being `Some` is what turns on
/// `<img>` capture; `None` leaves `<img>` dropped exactly as before this
/// function existed.
fn flatten_html(html: &str, image_base: Option<&str>) -> (String, Vec<FlattenedImage>) {
    let html = find_tag_start_ci(html, "body").map_or(html, |start| &html[start..]);
    let mut output = String::new();
    let mut images: Vec<FlattenedImage> = Vec::new();
    let mut cursor = 0;
    while cursor < html.len() {
        let rest = &html[cursor..];
        if rest.starts_with('<') {
            let Some(end_rel) = rest.find('>') else { break };
            let tag = rest[1..end_rel].trim();
            let closing = tag.starts_with('/');
            let name = tag
                .trim_start_matches('/')
                .split(|character: char| character.is_whitespace() || character == '/')
                .next()
                .unwrap_or("")
                .rsplit(':')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            // `<style>`/`<script>` bodies are raw CSS/JS, not XML-escaped
            // text: skip straight to the matching close tag so their content
            // never leaks into reflowed prose (most visibly right before a
            // chapter's opening heading, where a stylesheet block usually
            // sits). Self-closed elements (`<script src="x.js"/>`) have no
            // body and no separate close tag; searching for one would either
            // hit a later, unrelated element's close tag or, if none exists,
            // swallow the rest of the chapter's text into `html.len()`.
            let self_closed = tag.ends_with('/');
            if !closing && !self_closed && matches!(name.as_str(), "script" | "style") {
                let close_tag = format!("</{name}>");
                cursor = find_ci(&html[cursor..], &close_tag)
                    .map_or(html.len(), |relative| cursor + relative + close_tag.len());
                continue;
            }
            // `<img>` is a void element (no separate close tag, whether or
            // not the source bothers with the trailing `/`) -- capture it
            // once here, ahead of the paragraph/newline match below, which
            // does not otherwise recognize "img" and would just fall
            // through to the plain `cursor += end_rel + 1; continue;` this
            // branch also ends with.
            if !closing && name == "img" {
                if let Some(base) = image_base {
                    if images.len() < EPUB_IMAGE_LIMIT {
                        if let Some(src) = attribute(tag, "src") {
                            images.push(FlattenedImage {
                                href: normalize_archive_path(base, &src),
                                alt: attribute(tag, "alt").unwrap_or_default(),
                                offset: output.len(),
                            });
                            output.push(EPUB_IMAGE_SENTINEL);
                        }
                    }
                }
                cursor += end_rel + 1;
                continue;
            }
            if matches!(
                name.as_str(),
                "p" | "div"
                    | "section"
                    | "article"
                    | "blockquote"
                    | "li"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "br"
            ) {
                push_newline(&mut output);
                if closing || name == "br" {
                    push_newline(&mut output);
                }
            }
            cursor += end_rel + 1;
            continue;
        }
        if rest.starts_with('&') {
            if let Some(end_rel) = rest.find(';').filter(|value| *value <= 12) {
                let decoded = decode_entity(&rest[1..end_rel]);
                for character in decoded.chars() {
                    push_text_character(&mut output, character);
                }
                cursor += end_rel + 1;
                continue;
            }
        }
        let character = rest.chars().next().unwrap_or(' ');
        push_text_character(&mut output, character);
        cursor += character.len_utf8();
    }
    // `.trim()` below can drop a leading/trailing run of whitespace that an
    // image offset recorded above was measured against -- shift every
    // recorded offset by the trimmed prefix, and drop any image whose
    // sentinel would otherwise land outside the trimmed string entirely
    // (only possible if trailing whitespace somehow followed the sentinel,
    // since the sentinel itself is never whitespace and so is never trimmed
    // away on its own).
    let trim_start = output.len() - output.trim_start().len();
    let trimmed = output.trim().to_string();
    let trimmed_end = trim_start + trimmed.len();
    let images = images
        .into_iter()
        .filter(|image| image.offset >= trim_start && image.offset < trimmed_end)
        .map(|image| FlattenedImage {
            offset: image.offset - trim_start,
            ..image
        })
        .collect();
    (trimmed, images)
}

/// Case-insensitive ASCII substring search that avoids allocating a
/// lowercased copy of `haystack`, which may be up to
/// [`EPUB_MEMBER_UNCOMPRESSED_LIMIT`] bytes.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&start| {
        haystack[start..start + needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

/// Locate the byte offset of a `<tag_name` open-tag start, case-insensitively,
/// rejecting names that merely share a prefix (`<bodyfoo>` does not match
/// `body`).
fn find_tag_start_ci(html: &str, tag_name: &str) -> Option<usize> {
    let needle = format!("<{tag_name}");
    let mut search_from = 0;
    while let Some(relative) = find_ci(&html[search_from..], &needle) {
        let start = search_from + relative;
        let after = start + needle.len();
        let boundary = html[after..].chars().next().map_or(true, |character| {
            character.is_whitespace() || character == '>' || character == '/'
        });
        if boundary {
            return Some(start);
        }
        search_from = after;
    }
    None
}

fn push_text_character(output: &mut String, character: char) {
    if character == '\r' {
        return;
    }
    if character == '\n' || character == '\t' || character.is_whitespace() {
        if !output.ends_with(' ') && !output.ends_with('\n') && !output.is_empty() {
            output.push(' ');
        }
    } else {
        output.push(character);
    }
}

fn push_newline(output: &mut String) {
    while output.ends_with(' ') {
        output.pop();
    }
    if !output.ends_with("\n\n") && !output.is_empty() {
        output.push('\n');
    }
}

fn decode_entities(value: &str) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    while cursor < value.len() {
        let rest = &value[cursor..];
        if rest.starts_with('&') {
            if let Some(end_rel) = rest.find(';').filter(|value| *value <= 12) {
                output.push_str(&decode_entity(&rest[1..end_rel]));
                cursor += end_rel + 1;
                continue;
            }
        }
        let character = rest.chars().next().unwrap_or(' ');
        output.push(character);
        cursor += character.len_utf8();
    }
    output
}

/// HTML's named entities for Latin-1, U+00A0..=U+00FF in code point order.
const LATIN1_ENTITIES: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf",
    "laquo", "not", "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro",
    "para", "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest",
    "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig", "Ccedil", "Egrave", "Eacute",
    "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve", "Oacute",
    "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute",
    "THORN", "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil",
    "egrave", "eacute", "ecirc", "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde",
    "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc",
    "uuml", "yacute", "thorn", "yuml",
];

/// Decode one character reference (the text between `&` and `;`): numeric
/// ones, and HTML's named entities for Latin-1 and for the punctuation
/// books use. Anything else becomes `?`.
fn decode_entity(entity: &str) -> String {
    let character = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        // Some converters write &nbsp; for every space: an ordinary space
        // keeps those paragraphs breakable.
        "nbsp" => ' ',
        "OElig" => '\u{0152}',
        "oelig" => '\u{0153}',
        "Scaron" => '\u{0160}',
        "scaron" => '\u{0161}',
        "Yuml" => '\u{0178}',
        "fnof" => '\u{0192}',
        "circ" => '\u{02C6}',
        "tilde" => '\u{02DC}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "zwnj" => '\u{200C}',
        "zwj" => '\u{200D}',
        "lrm" => '\u{200E}',
        "rlm" => '\u{200F}',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "sbquo" => '\u{201A}',
        "ldquo" => '\u{201C}',
        "rdquo" => '\u{201D}',
        "bdquo" => '\u{201E}',
        "dagger" => '\u{2020}',
        "Dagger" => '\u{2021}',
        "bull" => '\u{2022}',
        "hellip" => '\u{2026}',
        "permil" => '\u{2030}',
        "prime" => '\u{2032}',
        "Prime" => '\u{2033}',
        "lsaquo" => '\u{2039}',
        "rsaquo" => '\u{203A}',
        "euro" => '\u{20AC}',
        "trade" => '\u{2122}',
        "minus" => '\u{2212}',
        value if value.starts_with("#x") || value.starts_with("#X") => {
            u32::from_str_radix(&value[2..], 16)
                .ok()
                .and_then(char::from_u32)
                .unwrap_or('?')
        }
        value if value.starts_with('#') => value[1..]
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .unwrap_or('?'),
        value => LATIN1_ENTITIES
            .iter()
            .position(|name| *name == value)
            .and_then(|index| char::from_u32(0xA0 + index as u32))
            .unwrap_or('?'),
    };
    character.to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        attribute, extract_cover, extract_cover_on_worker, extract_member, first_open_tag,
        html_to_text, html_to_text_with_images, image_header_size, open_epub, open_epub_on_worker,
        read_epub_title_on_worker, EPUB_IMAGE_LIMIT, EPUB_IMAGE_SENTINEL,
        EPUB_PARSER_WORKER_STACK_BYTES, EPUB_REFLOW_TEXT_LIMIT, EPUB_TITLE_WORKER_STACK_BYTES,
    };

    fn temp_epub(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustmix-{name}-{nonce}.epu"))
    }

    #[test]
    fn on_disk_text_is_read_through_the_kept_handle_until_released() {
        let path = temp_epub("kept-handle").with_extension("EPX");
        fs::write(&path, b"HEADERfirst text").unwrap();
        let document = super::EpubDocument::from_cache_body(
            "Title".into(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            1,
            path.clone(),
            6,
            10,
        );
        assert_eq!(&*document.text_window(0, 5).unwrap(), b"first");
        assert_eq!(&*document.text_window(6, 99).unwrap(), b"text");
        // Rewritten the way the Reader does it: release, then replace.
        super::release_kept_text_file();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"HEADERother text").unwrap();
        assert_eq!(&*document.text_window(0, 5).unwrap(), b"other");
        super::release_kept_text_file();
        fs::remove_file(&path).unwrap();
        assert!(document.text_window(0, 5).is_err());
    }

    fn push_u16(output: &mut Vec<u8>, value: u16) {
        output.extend(value.to_le_bytes());
    }
    fn push_u32(output: &mut Vec<u8>, value: u32) {
        output.extend(value.to_le_bytes());
    }

    fn stored_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut central = Vec::new();
        for (name, body) in entries {
            let offset = output.len() as u32;
            push_u32(&mut output, 0x0403_4B50);
            push_u16(&mut output, 20);
            push_u16(&mut output, 0);
            push_u16(&mut output, 0);
            push_u16(&mut output, 0);
            push_u16(&mut output, 0);
            push_u32(&mut output, 0);
            push_u32(&mut output, body.len() as u32);
            push_u32(&mut output, body.len() as u32);
            push_u16(&mut output, name.len() as u16);
            push_u16(&mut output, 0);
            output.extend(name.as_bytes());
            output.extend(body.as_bytes());

            push_u32(&mut central, 0x0201_4B50);
            push_u16(&mut central, 20);
            push_u16(&mut central, 20);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, body.len() as u32);
            push_u32(&mut central, body.len() as u32);
            push_u16(&mut central, name.len() as u16);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, offset);
            central.extend(name.as_bytes());
        }
        let central_offset = output.len() as u32;
        let central_size = central.len() as u32;
        output.extend(central);
        push_u32(&mut output, 0x0605_4B50);
        push_u16(&mut output, 0);
        push_u16(&mut output, 0);
        push_u16(&mut output, entries.len() as u16);
        push_u16(&mut output, entries.len() as u16);
        push_u32(&mut output, central_size);
        push_u32(&mut output, central_offset);
        push_u16(&mut output, 0);
        output
    }

    #[test]
    fn xml_attribute_tokenizer_reads_attributes_after_element_name() {
        let tag =
            "rootfile full-path='OEBPS/book.opf' media-type=\"application/oebps-package+xml\"/";
        assert_eq!(
            attribute(tag, "full-path").as_deref(),
            Some("OEBPS/book.opf")
        );
        assert_eq!(
            attribute(tag, "media-type").as_deref(),
            Some("application/oebps-package+xml")
        );
    }

    #[test]
    fn container_rootfile_lookup_ignores_plural_wrapper() {
        let container =
            "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>";
        let rootfile =
            first_open_tag(container, "rootfile").and_then(|tag| attribute(tag, "full-path"));
        assert_eq!(rootfile.as_deref(), Some("OEBPS/book.opf"));
    }

    #[test]
    fn parser_worker_stack_budget_is_explicit() {
        assert_eq!(EPUB_PARSER_WORKER_STACK_BYTES, 64 * 1024);
    }

    #[test]
    fn title_worker_uses_a_smaller_bounded_stack() {
        assert_eq!(EPUB_TITLE_WORKER_STACK_BYTES, 32 * 1024);
        assert!(EPUB_TITLE_WORKER_STACK_BYTES < EPUB_PARSER_WORKER_STACK_BYTES);
    }

    #[test]
    fn flattens_xhtml_into_reflowable_paragraphs() {
        assert_eq!(
            html_to_text("<h1>Title</h1><p>Hello &amp; goodbye.</p>"),
            "Title\n\nHello & goodbye."
        );
    }

    #[test]
    fn decodes_named_typographic_and_accent_entities_instead_of_a_stray_question_mark() {
        assert_eq!(
            html_to_text(
                "<p>Perch&eacute; &laquo;cos&igrave;&raquo;&hellip; disse lei&mdash;e tacque.</p>"
            ),
            "Perché «così»… disse lei\u{2014}e tacque."
        );
    }

    fn offsets_for(names: &[&str]) -> std::collections::BTreeMap<String, (usize, u64)> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    super::normalize_archive_path("OEBPS", name),
                    (index, index as u64 * 100),
                )
            })
            .collect()
    }

    #[test]
    fn ncx_keeps_every_nested_point_in_order() {
        let ncx = r#"<navMap>
            <navPoint id="p1"><navLabel><text>Parte prima</text></navLabel><content src="p1.xhtml"/>
                <navPoint id="c1"><navLabel><text>Capitolo 1</text></navLabel><content src="c1.xhtml"/></navPoint>
                <navPoint id="c2"><navLabel><text>Capitolo 2</text></navLabel><content src="c2.xhtml"/></navPoint>
            </navPoint>
            <navPoint id="p2"><navLabel><text>Parte seconda</text></navLabel><content src="p2.xhtml"/></navPoint>
        </navMap>"#;
        let offsets = offsets_for(&["p1.xhtml", "c1.xhtml", "c2.xhtml", "p2.xhtml"]);
        let toc = super::ncx_to_toc(ncx, "OEBPS", &offsets);
        let labels: Vec<&str> = toc.iter().map(|entry| entry.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Parte prima", "Capitolo 1", "Capitolo 2", "Parte seconda"]
        );
    }

    #[test]
    fn epub3_contents_come_from_the_toc_nav_only() {
        let nav = r#"<body>
            <nav epub:type="landmarks"><ol><li><a href="c1.xhtml">Inizio</a></li></ol></nav>
            <nav epub:type="toc"><ol>
                <li><a href="c1.xhtml">Uno</a></li><li><a href="c2.xhtml">Due</a></li>
            </ol></nav>
            <nav epub:type="page-list"><ol><li><a href="c1.xhtml#p1">1</a></li><li><a href="c2.xhtml#p9">9</a></li></ol></nav>
        </body>"#;
        let offsets = offsets_for(&["c1.xhtml", "c2.xhtml"]);
        let toc = super::links_to_toc(super::toc_nav_region(nav), "OEBPS", &offsets);
        let labels: Vec<&str> = toc.iter().map(|entry| entry.label.as_str()).collect();
        assert_eq!(labels, ["Uno", "Due"]);
        // No marked toc nav: every link, as before.
        assert_eq!(
            super::toc_nav_region("<a href=\"x\">x</a>"),
            "<a href=\"x\">x</a>"
        );
    }

    #[test]
    fn long_spines_and_manifests_are_read_whole() {
        let items: String = (0..600)
            .map(|index| format!(r#"<item id="c{index}" href="c{index}.xhtml" media-type="application/xhtml+xml"/>"#))
            .collect();
        let itemrefs: String = (0..600)
            .map(|index| format!(r#"<itemref idref="c{index}"/>"#))
            .collect();
        let package = format!("<manifest>{items}</manifest><spine>{itemrefs}</spine>");
        // Past the old 256-item manifest and 128-item spine cutoffs.
        let manifest = super::parse_manifest(&package).unwrap();
        assert!(manifest.contains_key("c599"));
        let spine = super::parse_spine_ids(&package).unwrap();
        assert_eq!(spine.len(), 600);
        assert_eq!(spine[599], "c599");
        // Beyond the limit: an explicit error, never a silently short book.
        let huge: String = (0..=super::EPUB_SPINE_LIMIT)
            .map(|index| format!(r#"<itemref idref="c{index}"/>"#))
            .collect();
        let error = super::parse_spine_ids(&format!("<spine>{huge}</spine>")).unwrap_err();
        assert!(error.contains("chapter files"), "{error}");
    }

    #[test]
    fn decodes_every_latin1_and_book_punctuation_entity_to_its_own_character() {
        assert_eq!(
            html_to_text(
                "<p>&bdquo;Gr&uuml;&szlig;e&ldquo; &sbquo;&OElig;uvre&lsquo; na&iuml;ve &Aring;&yuml; 3&euro; &frac12;&trade; &bogus;</p>"
            ),
            "\u{201E}Grüße\u{201C} \u{201A}Œuvre\u{2018} naïve Åÿ 3€ ½™ ?"
        );
    }

    #[test]
    fn skips_head_style_and_script_content_before_body() {
        let xhtml = "<html><head><title>Meta Title</title><style type=\"text/css\">/* chapter heading */ h1 { color: red; }</style><script>var x = 1;</script></head><body><h1>Real Title</h1><p>First line.</p></body></html>";
        assert_eq!(html_to_text(xhtml), "Real Title\n\nFirst line.");
    }

    #[test]
    fn skips_style_and_script_embedded_inside_body() {
        let xhtml = "<body><style>/* inline */</style><h1>Title</h1><script>track();</script><p>Body text.</p></body>";
        assert_eq!(html_to_text(xhtml), "Title\n\nBody text.");
    }

    #[test]
    fn fragments_without_a_body_tag_still_flatten() {
        assert_eq!(html_to_text("<a href='c1.xhtml'>Start</a>"), "Start");
    }

    #[test]
    fn self_closed_script_and_style_do_not_swallow_rest_of_chapter() {
        let xhtml = "<html><body><h1>Title</h1><p>First paragraph.</p><script src=\"x.js\"/><p>Second paragraph.</p><style href=\"x.css\"/><p>Third paragraph.</p></body></html>";
        assert_eq!(
            html_to_text(xhtml),
            "Title\n\nFirst paragraph.\n\nSecond paragraph.\n\nThird paragraph."
        );
    }

    #[test]
    fn html_to_text_ignores_images_when_not_capturing() {
        // TOC labels and chapter-title fallbacks go through plain
        // `html_to_text`, which must keep dropping `<img>` exactly as before
        // image capture existed -- only `open_epub`'s spine loop (via
        // `html_to_text_with_images`) should ever record one.
        let xhtml = "<body><p>Before</p><img src='pic.jpg' alt='A cat'/><p>After</p></body>";
        assert_eq!(html_to_text(xhtml), "Before\n\nAfter");
    }

    #[test]
    fn html_to_text_with_images_captures_src_alt_and_sentinel_offset() {
        let xhtml =
            "<body><p>Before</p><img src='../images/pic.jpg' alt='A cat'/><p>After</p></body>";
        let (text, images) = html_to_text_with_images(xhtml, "OEBPS/text");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].href, "OEBPS/images/pic.jpg");
        assert_eq!(images[0].alt, "A cat");
        assert_eq!(
            text[images[0].offset..].chars().next(),
            Some(EPUB_IMAGE_SENTINEL)
        );
        // `<img>` itself inserts no paragraph break (it is void, not a
        // block tag in the newline-triggering match); the single `\n`
        // before "After" comes from `<p>` opening once, matching how the
        // tokenizer already treats other non-paragraph inline content.
        assert_eq!(text, format!("Before\n\n{EPUB_IMAGE_SENTINEL}\nAfter"));
    }

    #[test]
    fn html_to_text_with_images_skips_img_without_src_and_respects_limit() {
        let mut xhtml = String::from("<body>");
        for index in 0..EPUB_IMAGE_LIMIT + 5 {
            xhtml.push_str(&format!("<img src='p{index}.jpg'/>"));
        }
        xhtml.push_str("<img alt='no src, must not count or panic'/></body>");
        let (_, images) = html_to_text_with_images(&xhtml, "OEBPS");
        assert_eq!(images.len(), EPUB_IMAGE_LIMIT);
    }

    #[test]
    fn opens_stored_epub_manifest_spine_and_nav_toc() {
        let path = temp_epub("stored");
        let bytes = stored_zip(&[
            ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>"),
            ("OEBPS/book.opf", "<package><metadata><dc:title>Sample EPUB</dc:title></metadata><manifest><item id='nav' href='nav.xhtml' media-type='application/xhtml+xml' properties='nav'/><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/><item id='c2' href='c2.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/><itemref idref='c2'/></spine></package>"),
            ("OEBPS/nav.xhtml", "<nav><ol><li><a href='c1.xhtml'>Start</a></li><li><a href='c2.xhtml'>Second</a></li></ol></nav>"),
            ("OEBPS/c1.xhtml", "<html><body><h1>Start</h1><p>First chapter.</p></body></html>"),
            ("OEBPS/c2.xhtml", "<html><body><h1>Second</h1><p>Second chapter.</p></body></html>"),
        ]);
        fs::write(&path, bytes).unwrap();
        let epub = open_epub(&path).unwrap();
        let worker_epub = open_epub_on_worker(&path).unwrap();
        assert_eq!(worker_epub, epub);
        assert_eq!(epub.title, "Sample EPUB");
        assert_eq!(epub.spine_count, 2);
        assert_eq!(epub.chapters.len(), 2);
        assert_eq!(epub.chapters[0].number, 1);
        assert_eq!(epub.chapters[1].number, 2);
        assert_eq!(
            epub.chapter_for_offset(epub.chapters[1].text_offset)
                .unwrap()
                .number,
            2
        );
        assert_eq!(read_epub_title_on_worker(&path).unwrap(), "Sample EPUB");
        let resident = epub.resident_text().unwrap();
        assert!(resident.contains("First chapter."));
        assert!(resident.contains("Second chapter."));
        assert_eq!(epub.toc.len(), 2);
        assert_eq!(epub.toc[0].label, "Start");
        assert!(epub.toc[1].text_offset > epub.toc[0].text_offset);
        let _ = fs::remove_file(path);
    }

    /// End-to-end spike check: an inline `<img>` parsed through the real
    /// `open_epub` path becomes one [`super::EpubImage`] whose `text_offset`
    /// lands exactly on the sentinel character in the whole book's flattened
    /// text, and every other offset-based mechanism (`chapter_for_offset`,
    /// the following chapter's own offsets) stays coherent around it -- the
    /// actual thing this step was meant to prove, not just the isolated
    /// `flatten_html` unit tests above.
    #[test]
    fn opens_a_book_with_an_inline_image_and_keeps_offsets_coherent() {
        let path = temp_epub("inline-image");
        let bytes = stored_zip(&[
            ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>"),
            ("OEBPS/book.opf", "<package><metadata><dc:title>Illustrated Book</dc:title></metadata><manifest><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/><item id='c2' href='c2.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/><itemref idref='c2'/></spine></package>"),
            ("OEBPS/c1.xhtml", "<html><body><h1>One</h1><p>Before the picture.</p><img src=\"images/fig1.jpg\" alt=\"A figure\"/><p>After the picture.</p></body></html>"),
            ("OEBPS/c2.xhtml", "<html><body><h1>Two</h1><p>Second chapter text.</p></body></html>"),
        ]);
        fs::write(&path, bytes).unwrap();
        let epub = open_epub(&path).unwrap();

        assert_eq!(epub.images.len(), 1);
        let image = &epub.images[0];
        assert_eq!(image.href, "OEBPS/images/fig1.jpg");
        assert_eq!(image.alt, "A figure");
        assert_eq!(image.spine_index, 0);

        let text = epub.resident_text().unwrap();
        assert_eq!(
            text[image.text_offset as usize..].chars().next(),
            Some(EPUB_IMAGE_SENTINEL)
        );
        assert_eq!(
            epub.chapter_for_offset(image.text_offset)
                .unwrap()
                .spine_index,
            0
        );
        assert!(text.contains("Before the picture."));
        assert!(text.contains("After the picture."));

        // Chapter two's own offset math is unaffected by chapter one's
        // image -- it is simply one character (the sentinel) further along
        // than it would be without the image, not corrupted or misaligned.
        assert_eq!(
            epub.chapter_for_offset(epub.chapters[1].text_offset)
                .unwrap()
                .number,
            2
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reads_png_and_jpeg_pixel_size_from_headers() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640_u32.to_be_bytes());
        png.extend_from_slice(&960_u32.to_be_bytes());
        assert_eq!(image_header_size(&png), Some((640, 960)));

        // SOI, one APP0 segment to skip, then SOF0 (height 300, width 200).
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0xAA, 0xBB, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x01,
            0x2C, 0x00, 0xC8, 0x01, 0x01, 0x11, 0x00,
        ];
        assert_eq!(image_header_size(&jpeg), Some((200, 300)));
        // A DHT marker (0xC4) is not a frame header and must be skipped.
        let with_dht = [
            0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x03, 0x00, 0xFF, 0xC2, 0x00, 0x0B, 0x08, 0x00, 0x10,
            0x00, 0x20, 0x01, 0x01, 0x11, 0x00,
        ];
        assert_eq!(image_header_size(&with_dht), Some((32, 16)));
        assert_eq!(image_header_size(b"GIF89a"), None);
    }

    #[test]
    fn extract_member_reads_one_member_without_the_whole_archive() {
        let path = temp_epub("extract-member");
        let bytes = stored_zip(&[
            ("META-INF/container.xml", "<container/>"),
            ("OEBPS/images/fig1.jpg", "not really a jpeg"),
            ("OEBPS/c1.xhtml", "<html><body><p>Text</p></body></html>"),
        ]);
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            extract_member(&path, "OEBPS/images/fig1.jpg").unwrap(),
            b"not really a jpeg"
        );
        assert_eq!(
            extract_member(&path, "oebps/c1.xhtml").unwrap(),
            b"<html><body><p>Text</p></body></html>"
        );
        assert!(extract_member(&path, "OEBPS/missing.png").is_err());
        let _ = fs::remove_file(path);
    }

    /// A book whose flattened text lands well past the old 2 MiB RAM ceiling
    /// (but under the current, raised `EPUB_REFLOW_TEXT_LIMIT`) must open
    /// successfully instead of failing with "byte limit" — this is the exact
    /// failure a reader hit with a real large EPUB before the limit was
    /// raised to give large books PSRAM headroom to actually use.
    #[test]
    fn opens_a_book_whose_flattened_text_exceeds_the_old_two_mebibyte_limit() {
        let path = temp_epub("large");
        // Comfortably over the old 2 MiB cap, comfortably under the new one,
        // and each chapter's raw XHTML stays under `EPUB_MEMBER_COMPRESSED_LIMIT`
        // (2 MiB) so `stored_zip`'s uncompressed entries still extract.
        let paragraph = "Lorem ipsum dolor sit amet consectetur adipiscing elit. ".repeat(28_000);
        assert!(paragraph.len() > 1_500_000 && paragraph.len() < 2 * 1024 * 1024);
        let chapter_one = format!("<html><body><h1>One</h1><p>{paragraph}</p></body></html>");
        let chapter_two = format!("<html><body><h1>Two</h1><p>{paragraph}</p></body></html>");
        let bytes = stored_zip(&[
            ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>"),
            ("OEBPS/book.opf", "<package><metadata><dc:title>Large Book</dc:title></metadata><manifest><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/><item id='c2' href='c2.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/><itemref idref='c2'/></spine></package>"),
            ("OEBPS/c1.xhtml", &chapter_one),
            ("OEBPS/c2.xhtml", &chapter_two),
        ]);
        fs::write(&path, bytes).unwrap();
        let epub = open_epub(&path).unwrap();
        assert!(epub.text_size_bytes() > 2 * 1024 * 1024);
        assert!(epub.text_size_bytes() < EPUB_REFLOW_TEXT_LIMIT as u64);
        assert_eq!(epub.chapters.len(), 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn extracts_epub3_cover_via_properties_hint() {
        let path = temp_epub("cover-epub3");
        let bytes = stored_zip(&[
            ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>"),
            ("OEBPS/book.opf", "<package><metadata><dc:title>Cover Book</dc:title></metadata><manifest><item id='cover-img' href='images/cover.jpg' media-type='image/jpeg' properties='cover-image'/><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/></spine></package>"),
            ("OEBPS/images/cover.jpg", "fake-jpeg-bytes"),
            ("OEBPS/c1.xhtml", "<html><body><p>Body.</p></body></html>"),
        ]);
        fs::write(&path, bytes).unwrap();
        let cover = extract_cover(&path).unwrap().unwrap();
        assert_eq!(cover.media_type, "image/jpeg");
        assert_eq!(cover.bytes, b"fake-jpeg-bytes");
        let worker_cover = extract_cover_on_worker(&path).unwrap().unwrap();
        assert_eq!(worker_cover, cover);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn extracts_epub2_cover_via_meta_name_convention() {
        let path = temp_epub("cover-epub2");
        let bytes = stored_zip(&[
            ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>"),
            ("OEBPS/book.opf", "<package><metadata><dc:title>Cover Book</dc:title><meta name='cover' content='cover-img'/></metadata><manifest><item id='cover-img' href='cover.png' media-type='image/png'/><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/></spine></package>"),
            ("OEBPS/cover.png", "fake-png-bytes"),
            ("OEBPS/c1.xhtml", "<html><body><p>Body.</p></body></html>"),
        ]);
        fs::write(&path, bytes).unwrap();
        let cover = extract_cover(&path).unwrap().unwrap();
        assert_eq!(cover.media_type, "image/png");
        assert_eq!(cover.bytes, b"fake-png-bytes");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_cover_is_not_an_error() {
        let path = temp_epub("cover-missing");
        let bytes = stored_zip(&[
            ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>"),
            ("OEBPS/book.opf", "<package><metadata><dc:title>No Cover</dc:title></metadata><manifest><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/></spine></package>"),
            ("OEBPS/c1.xhtml", "<html><body><p>Body.</p></body></html>"),
        ]);
        fs::write(&path, bytes).unwrap();
        assert_eq!(extract_cover(&path).unwrap(), None);
        let _ = fs::remove_file(path);
    }
}
