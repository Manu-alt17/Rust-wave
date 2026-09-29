//! Offline Reader state, TXT / EPUB pagination and Reader-owned persistence.
//!
//! v0.17.1 adds chapter-aware EPUB page labels, persistent chapter-aware EPUB
//! bookmark labels and OPF-title Library rows while preserving the accepted TXT
//! Reader, FAT 8.3 persistence, per-book resume and staged loading architecture.
// rustmix-wave=epub-watchdog-memory-pressure-repair-ready

use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

use crate::{
    buttons::ButtonEvent,
    dictionary::{
        compact_error, lookup_dictionary_explained, DictionaryIndex, DICTIONARY_ROOT,
    },
    epub::{
        open_epub_on_worker, read_epub_title_on_worker, EpubChapter, EpubDocument, EpubImage,
        EpubTocEntry, EPUB_IMAGE_LIMIT, EPUB_IMAGE_SENTINEL, EPUB_REFLOW_TEXT_LIMIT,
        EPUB_SPINE_LIMIT, EPUB_TOC_LIMIT,
    },
    reading_stats::CurrentBookProgress,
    regional::Locale,
};

/// SD-card library owned by the Reader subsystem.
pub const READER_BOOKS_DIRECTORY: &str = "/sdcard/RUSTMIX/BOOKS";
/// SD-card state directory owned by the Reader subsystem.
pub const READER_STATE_DIRECTORY: &str = "/sdcard/RUSTMIX/READER";
/// Persistent last-read state file.
pub const READER_STATE_FILE: &str = "STATE.TXT";
/// Persistent per-book last-position map.
pub const READER_POSITIONS_FILE: &str = "POSITS.TXT";
/// Legacy long-name per-book positions file accepted read-only for migration.
pub const LEGACY_READER_POSITIONS_FILE: &str = "POSITIONS.TXT";
/// Persistent recent-book list.
pub const READER_RECENT_FILE: &str = "RECENT.TXT";
/// Persistent bookmark list.
pub const READER_BOOKMARKS_FILE: &str = "MARKS.TXT";
/// Persistent Reader-specific preferences.
pub const READER_PREFS_FILE: &str = "PREFS.TXT";
/// Marker recording whether the Reader was the active screen at the moment
/// hardware deep sleep was entered. A real deep-sleep wake is a full MCU
/// reboot, so nothing in RAM (including the router's current route) survives
/// it; this tiny file lets the fresh boot decide whether to auto-resume the
/// last book instead of landing on Home.
pub const READER_DEEP_SLEEP_ACTIVE_FILE: &str = "DSACTIVE.TXT";
/// SD-backed TXT anchor-cache directory.
pub const READER_CACHE_DIRECTORY: &str = "CACHE";
/// Number of text lines rendered on one portrait Reader page.
pub const READER_LINES_PER_PAGE: usize = 22;
/// Maximum wrapped characters per line for the current Reader body profile.
pub const READER_CHARS_PER_LINE: usize = 43;
/// Nearby page cache retained in RAM while one book is open.
pub const READER_NEARBY_PAGE_CACHE: usize = 8;
/// Maximum bytes read while generating a single page.
pub const READER_PAGE_READ_BYTES: usize = 16 * 1024;
/// First read window tried for one page. A page holds roughly 1-2 KB of
/// text, so decoding the full [`READER_PAGE_READ_BYTES`] window up front
/// (two `(char, u64)` vectors of 16 bytes per character) did about ten times
/// more work than the page needs. The window doubles, up to
/// `READER_PAGE_READ_BYTES`, only when the page does not fill inside it (see
/// [`paginate_decoded_window`]), so every page comes out byte-for-byte
/// identical to a single full-window pass.
const READER_PAGE_INITIAL_READ_BYTES: usize = 4 * 1024;
/// Maximum library rows retained for the embedded product UI.
pub const READER_LIBRARY_LIMIT: usize = 128;
/// Maximum per-book last-position records retained on removable storage.
pub const READER_POSITION_LIMIT: usize = 64;
/// Maximum recent-book records retained on removable storage.
pub const READER_RECENT_LIMIT: usize = 16;
/// How long the reader must sit on a page before a page turn's deferred
/// STATE/POSITS/RECENT save actually hits SD (see `pending_persist`).
/// Flipping through several pages within this window coalesces into a
/// single save instead of one `fsync`-heavy save per page.
pub const READER_PERSIST_DEBOUNCE: Duration = Duration::from_millis(2_500);
/// Maximum warm `ReaderSession`s kept resident at once (the active session
/// plus this many parked ones): the last book opened past the limit evicts
/// the oldest parked entry. Bounded low because each parked session still
/// holds small-but-nonzero RAM (`page_offsets`, the nearby-page cache, EPUB
/// TOC/chapter metadata) even though its book text itself stays SD-backed
/// (see `EpubTextStore::OnDisk`).
pub const READER_SESSION_CACHE_LIMIT: usize = 3;
/// Maximum bookmark records retained on removable storage.
pub const READER_BOOKMARK_LIMIT: usize = 128;
/// Maximum page anchors accepted from one SD-backed cache file.
pub const READER_CACHE_OFFSET_LIMIT: usize = 4096;
/// Persist an anchor-cache checkpoint after this many newly indexed pages
/// while a book's index is still short (see [`is_index_checkpoint`]).
pub const READER_CACHE_CHECKPOINT_PAGES: usize = 4;
/// Index length up to which checkpoints stay at the dense
/// [`READER_CACHE_CHECKPOINT_PAGES`] cadence.
const READER_CACHE_CHECKPOINT_DENSE_UNTIL: usize = 32;
/// Checkpoint cadence past [`READER_CACHE_CHECKPOINT_DENSE_UNTIL`].
const READER_CACHE_CHECKPOINT_SPARSE_PAGES: usize = 64;

/// Whether an index that just reached `indexed_pages` should be persisted.
/// Every checkpoint rewrites the whole index file through an atomic
/// replace (create, write, two renames, cleanup), and that file grows with
/// the book, so a fixed every-4-pages cadence cost SD writes quadratic in
/// book length -- tens of MB over a long book, on the main loop's tick.
/// Early pages keep the dense cadence (a freshly opened book gets a usable
/// cache quickly); after that at most 63 pages of background indexing are
/// redone after an unexpected stop, a few seconds of background work.
fn is_index_checkpoint(indexed_pages: usize) -> bool {
    indexed_pages % READER_CACHE_CHECKPOINT_PAGES == 0
        && (indexed_pages <= READER_CACHE_CHECKPOINT_DENSE_UNTIL
            || indexed_pages % READER_CACHE_CHECKPOINT_SPARSE_PAGES == 0)
}
/// Maximum pre-indexed EPUB page anchors retained for chapter-aware labels.
pub const READER_EPUB_PAGE_ANCHOR_LIMIT: usize = 4096;

const READER_PERSISTENCE_VERSION: &str = "1";
/// Bumped to `"4"` when inline images started reserving aspect-correct
/// (and, for standalone images, full-page) slot spans, which changes where
/// EPUB page breaks fall. Bumped to `"5"` when a hard-broken word that
/// fills a page started resuming on the next page right after its last
/// shown character, instead of from the word's start (see
/// [`WordPlacement::PageFilledMidWord`]).
const READER_CACHE_VERSION: &str = "5";
const READER_PREFS_VERSION: &str = "1";
/// SD-backed flattened-EPUB-text cache format version. Independent of Reader
/// layout: the cached reflowed text and TOC never change with font/orientation.
/// Bumped from `"1"` to `"2"` when `image=` records were added -- an older
/// cache file simply has none, but bumping still forces one clean reparse per
/// book on upgrade so every already-cached book picks up its image table
/// instead of silently reading back an empty one forever. Bumped to `"3"`
/// when image records gained their probed pixel width/height.
const EPUB_DOCUMENT_CACHE_VERSION: &str = "3";
/// SD-backed EPUB page-offset index cache format version. Unlike the
/// flattened-text cache, this one is layout-dependent (see [`book_fingerprint`]):
/// a font or orientation change must invalidate it, since page breaks move.
const EPUB_PAGE_INDEX_CACHE_VERSION: &str = "1";
const CACHE_FNV_OFFSET: u64 = 0xcbf29ce484222325;
const CACHE_FNV_PRIME: u64 = 0x100000001b3;

/// Reader-supported content types. TXT and bounded reflowable EPUB are active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BookFormat {
    Text,
    Epub,
}

impl BookFormat {
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Text => "TXT",
            Self::Epub => "EPUB",
        }
    }

    #[must_use]
    const fn marker(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Epub => "epub",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "txt" => Some(Self::Text),
            "epub" => Some(Self::Epub),
            _ => None,
        }
    }
}

/// One Reader library row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderBook {
    pub path: String,
    pub title: String,
    pub format: BookFormat,
    pub size_bytes: u64,
    pub modified_seconds: u64,
}

/// Chapter-relative EPUB page presentation retained with bookmarks so MARKS.TXT
/// remains useful after restart and before the matching book is reopened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderChapterPageLabel {
    pub chapter_number: usize,
    pub page_number: usize,
    pub page_count: usize,
}

impl ReaderChapterPageLabel {
    #[must_use]
    pub fn page_text(&self) -> String {
        format!("{}/{}", self.page_number, self.page_count)
    }
}

/// Stable logical reading position used by STATE.TXT, RECENT.TXT and
/// MARKS.TXT. TXT byte offsets remain valid independently of generated UI page
/// labels. EPUB reuses this byte-offset boundary against its flattened text buffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderLocation {
    pub path: String,
    pub title: String,
    pub format: BookFormat,
    pub size_bytes: u64,
    pub modified_seconds: u64,
    pub page_index: usize,
    pub byte_offset: u64,
    pub epub_chapter: Option<ReaderChapterPageLabel>,
    /// [`ReaderSession::reading_percent`] at the moment this location was
    /// saved, carried along so a consumer that never opens the book (the
    /// Library grid's completion badge) can show the same number the reader
    /// itself displayed, instead of recomputing a cruder byte-offset
    /// estimate from scratch. `None` for a location saved before this field
    /// existed, or one built outside an open session.
    pub reading_percent: Option<u8>,
}

impl ReaderLocation {
    #[must_use]
    pub fn as_book(&self) -> ReaderBook {
        ReaderBook {
            path: self.path.clone(),
            title: self.title.clone(),
            format: self.format,
            size_bytes: self.size_bytes,
            modified_seconds: self.modified_seconds,
        }
    }

    #[must_use]
    fn matches_book(&self, book: &ReaderBook) -> bool {
        self.path == book.path
            && self.size_bytes == book.size_bytes
            && self.modified_seconds == book.modified_seconds
            && self.format == book.format
    }

    #[must_use]
    fn same_position(&self, other: &Self) -> bool {
        self.path == other.path && self.byte_offset == other.byte_offset
    }

    /// This location's reading-completion percentage, preferring the exact
    /// figure stashed at save time and falling back to a byte-offset
    /// estimate otherwise (mirrors `ReaderUiState::library_progress_percent`,
    /// but works straight off a `ReaderLocation` the caller already has in
    /// hand — used by the Library grid to classify a Recent entry as still
    /// in progress vs. finished without a second `positions` lookup).
    #[must_use]
    pub fn reading_percent_estimate(&self) -> u8 {
        if let Some(percent) = self.reading_percent {
            return percent.min(100);
        }
        if self.size_bytes == 0 {
            return 100;
        }
        (self.byte_offset.saturating_mul(100) / self.size_bytes).min(100) as u8
    }
}

/// One list row rendered by Recent, Books, Files or Bookmarks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderLibraryEntry {
    pub book: ReaderBook,
    pub location: Option<ReaderLocation>,
}

/// Text decoding mode detected when a TXT book is opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Windows1252,
}

impl TextEncoding {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 BOM",
            Self::Windows1252 => "WIN-1252",
        }
    }
}

/// E-paper-friendly Reader page theme.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReadingTheme {
    #[default]
    Classic,
    HighContrast,
}

impl ReadingTheme {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::HighContrast => "High Contrast",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for on-screen preference rows.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::Classic => "Classico",
                Self::HighContrast => "Alto contrasto",
            },
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::HighContrast => "high-contrast",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Classic => Self::HighContrast,
            Self::HighContrast => Self::Classic,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        self.next()
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "classic" => Ok(Self::Classic),
            "high-contrast" | "high_contrast" => Ok(Self::HighContrast),
            other => Err(format!("unsupported theme value {other:?}")),
        }
    }
}

/// Reader-page orientation independent from the portrait system UI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReaderOrientation {
    #[default]
    Portrait,
    Landscape,
}

impl ReaderOrientation {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Portrait => "Portrait",
            Self::Landscape => "Landscape",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for on-screen preference rows.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::Portrait => "Verticale",
                Self::Landscape => "Orizzontale",
            },
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Portrait => Self::Landscape,
            Self::Landscape => Self::Portrait,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        self.next()
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "portrait" => Ok(Self::Portrait),
            "landscape" => Ok(Self::Landscape),
            other => Err(format!("unsupported orientation value {other:?}")),
        }
    }
}

/// Reader-specific book font size. This is intentionally independent from
/// `/sdcard/RUSTMIX/DISPLAY.TXT`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BookFontSize {
    Large,
    #[default]
    XLarge,
    XXLarge,
    XXXLarge,
}

impl BookFontSize {
    /// On-screen label. Deliberately its own naming ladder (`Little` ..
    /// `XLarge`) rather than the variant names below: the variants and
    /// `marker()` keep their original identifiers so persisted preference
    /// files and internal matches stay stable across this rename.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Large => "Little",
            Self::XLarge => "Medium",
            Self::XXLarge => "Large",
            Self::XXXLarge => "XLarge",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for on-screen preference rows.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::Large => "Piccolo",
                Self::XLarge => "Medio",
                Self::XXLarge => "Grande",
                Self::XXXLarge => "Molto grande",
            },
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Large => "large",
            Self::XLarge => "xlarge",
            Self::XXLarge => "xxlarge",
            Self::XXXLarge => "xxxlarge",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Large => Self::XLarge,
            Self::XLarge => Self::XXLarge,
            Self::XXLarge => Self::XXXLarge,
            Self::XXXLarge => Self::Large,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Large => Self::XXXLarge,
            Self::XLarge => Self::Large,
            Self::XXLarge => Self::XLarge,
            Self::XXXLarge => Self::XXLarge,
        }
    }

    /// `small` and `medium` were removed (replaced by two larger tiers,
    /// `xxlarge` and `xxxlarge`, above the old `xlarge` ceiling) but still
    /// map to the new smallest tier so preference files saved by older
    /// firmware keep loading instead of falling back to every field's
    /// default.
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "small" | "medium" | "large" => Ok(Self::Large),
            "xlarge" | "extra-large" | "extra_large" => Ok(Self::XLarge),
            "xxlarge" => Ok(Self::XXLarge),
            "xxxlarge" => Ok(Self::XXXLarge),
            other => Err(format!("unsupported book_font_size value {other:?}")),
        }
    }
}

/// Reader-specific body font family. Reader-only generated bitmap strikes are
/// printable-ASCII subsets; raw font files are not distributed. Persisted
/// `serif` and `atkinson-hyperlegible` keys remain stable for compatibility.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BookFont {
    Inter,
    AtkinsonHyperlegible,
    #[default]
    Serif,
    Literata,
}

impl BookFont {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inter => "Inter",
            Self::AtkinsonHyperlegible => "Atkinson",
            Self::Serif => "Serif",
            Self::Literata => "Literata",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for on-screen preference rows.
    /// Font family names are proper nouns / typographic terms shared across
    /// languages, so the Italian text matches the English label verbatim.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English | Locale::Italian => self.label(),
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Inter => "inter",
            Self::AtkinsonHyperlegible => "atkinson-hyperlegible",
            Self::Serif => "serif",
            Self::Literata => "literata",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Inter => Self::AtkinsonHyperlegible,
            Self::AtkinsonHyperlegible => Self::Serif,
            Self::Serif => Self::Literata,
            Self::Literata => Self::Inter,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Inter => Self::Literata,
            Self::AtkinsonHyperlegible => Self::Inter,
            Self::Serif => Self::AtkinsonHyperlegible,
            Self::Literata => Self::Serif,
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "inter" => Ok(Self::Inter),
            "atkinson" | "atkinson-hyperlegible" | "atkinson_hyperlegible" => {
                Ok(Self::AtkinsonHyperlegible)
            }
            "serif" | "dejavu-serif" => Ok(Self::Serif),
            "literata" => Ok(Self::Literata),
            other => Err(format!("unsupported book_font value {other:?}")),
        }
    }
}

/// Reader paragraph alignment. Justified is the default e-book presentation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ParagraphAlignment {
    #[default]
    Justified,
    Left,
    Center,
    Right,
}

impl ParagraphAlignment {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Justified => "Justified",
            Self::Left => "Left",
            Self::Center => "Center",
            Self::Right => "Right",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for on-screen preference rows.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::Justified => "Giustificato",
                Self::Left => "Sinistra",
                Self::Center => "Centro",
                Self::Right => "Destra",
            },
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Justified => "justified",
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Justified => Self::Left,
            Self::Left => Self::Center,
            Self::Center => Self::Right,
            Self::Right => Self::Justified,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Justified => Self::Right,
            Self::Left => Self::Justified,
            Self::Center => Self::Left,
            Self::Right => Self::Center,
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "justified" | "justify" => Ok(Self::Justified),
            "left" => Ok(Self::Left),
            "center" | "centred" => Ok(Self::Center),
            "right" => Ok(Self::Right),
            other => Err(format!("unsupported paragraph_alignment value {other:?}")),
        }
    }
}

/// Horizontal margin, in logical pixels, reserved on each side of the Reader
/// body viewport. Shared with `app::screens::reader::ReaderBodyGeometry` so
/// pagination wraps against the exact width the renderer draws into, instead
/// of a fixed character budget that leaves the right edge of most lines
/// unused.
pub const READER_BODY_MARGIN_PX: i32 = 24;

/// Layout dimensions affecting TXT pagination and cache fingerprints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderLayout {
    /// Pixel width of the Reader body viewport that wrapped lines must fit
    /// within, measured with the layout's own `book_font` / `font_size`
    /// strike. Replaces a fixed character-count budget so proportional
    /// fonts (Serif, Literata) pack lines to the real available width
    /// instead of wrapping early and leaving pixels unused on the right.
    pub available_width_px: i32,
    pub lines_per_page: usize,
    pub orientation: ReaderOrientation,
    pub font_size: BookFontSize,
    pub book_font: BookFont,
    pub paragraph_alignment: ParagraphAlignment,
    /// Full-screen reading: no progress bar, no footer (hints, clock,
    /// battery); the book text takes the whole panel height, so more lines
    /// fit per page. See `app::screens::reader::reader_body_geometry`.
    pub full_screen: bool,
}

/// Reader-owned preference file persisted as `/RUSTMIX/READER/PREFS.TXT`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderPreferences {
    pub theme: ReadingTheme,
    pub orientation: ReaderOrientation,
    pub font_size: BookFontSize,
    pub book_font: BookFont,
    pub paragraph_alignment: ParagraphAlignment,
    pub show_progress: bool,
    /// Single tap = next page, double tap = previous page, via the QMI8658
    /// hardware tap engine. Off restores the Reader's normal battery-save
    /// behavior on the page route: `main.rs` only keeps the IMU at its full
    /// 1000 Hz profile (tap timing needs that; see `imu_tap_diagnostics`)
    /// and only polls the tap engine while this is on.
    pub tap_page_turn_enabled: bool,
    /// Hide every piece of page chrome and give the whole panel to the text
    /// (see [`ReaderLayout::full_screen`]).
    pub full_screen: bool,
}

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            theme: ReadingTheme::Classic,
            orientation: ReaderOrientation::Portrait,
            font_size: BookFontSize::XLarge,
            book_font: BookFont::Serif,
            paragraph_alignment: ParagraphAlignment::Justified,
            show_progress: true,
            tap_page_turn_enabled: true,
            full_screen: false,
        }
    }
}

impl ReaderPreferences {
    #[must_use]
    pub const fn layout(self) -> ReaderLayout {
        // Reader pages share one bounded body viewport across Classic and
        // High Contrast: `READER_BODY_MARGIN_PX` on each side of the logical
        // screen width for the current orientation. Pagination wraps lines
        // against this real pixel width (measured with the layout's own
        // font strike) rather than a fixed character count, so proportional
        // glyphs (Serif, Literata) pack every line to the available space
        // instead of breaking early. A final pixel clip in the renderer
        // still guards the rare line that undershoots the measurement.
        let available_width_px = match self.orientation {
            ReaderOrientation::Portrait => crate::framebuffer::HEIGHT as i32,
            ReaderOrientation::Landscape => crate::framebuffer::WIDTH as i32,
        } - 2 * READER_BODY_MARGIN_PX;

        // `lines_per_page` is calibrated against the Reader page's fixed
        // chrome: a slim progress bar up top (no title/header) and the
        // button-hint footer at the bottom, leaving the same body viewport
        // in both orientations. It is set to the largest count that still
        // fits every book font's line height at that size (book_font does
        // not change the result), so the shorter UI-family strikes (Inter,
        // Atkinson Hyperlegible) always clear it with room to spare.
        //
        // Portrait/XLarge is the one exception worth calling out: at
        // line_height 33 (Atkinson Hyperlegible, Serif and Literata all
        // share it at this size) a naive largest-that-fits count of 20 lands
        // the last line's baseline exactly on the render viewport's bottom
        // edge, which `render_page`'s own half-open clip guard
        // (`baseline >= body.text.bottom`) then silently drops -- paginated,
        // but never drawn. 19 leaves that line strictly inside the guard.
        // See `diagnostic_last_configured_line_clears_the_render_clip_guard`
        // in `app::screens::reader`'s tests for the check that caught this.
        //
        // Full screen drops the progress bar and the footer, so the body
        // viewport grows and each size gets its own, larger calibrated
        // count (same "largest that clears the render clip guard for every
        // book font" rule, checked by the same test).
        let lines_per_page = match (self.full_screen, self.orientation, self.font_size) {
            (true, ReaderOrientation::Portrait, BookFontSize::Large) => 25,
            (true, ReaderOrientation::Portrait, BookFontSize::XLarge) => 21,
            (true, ReaderOrientation::Portrait, BookFontSize::XXLarge) => 19,
            (true, ReaderOrientation::Portrait, BookFontSize::XXXLarge) => 16,
            (true, ReaderOrientation::Landscape, BookFontSize::Large) => 14,
            (true, ReaderOrientation::Landscape, BookFontSize::XLarge) => 12,
            (true, ReaderOrientation::Landscape, BookFontSize::XXLarge) => 11,
            (true, ReaderOrientation::Landscape, BookFontSize::XXXLarge) => 9,
            (false, ReaderOrientation::Portrait, BookFontSize::Large) => 23,
            (false, ReaderOrientation::Portrait, BookFontSize::XLarge) => 19,
            (false, ReaderOrientation::Portrait, BookFontSize::XXLarge) => 17,
            (false, ReaderOrientation::Portrait, BookFontSize::XXXLarge) => 15,
            (false, ReaderOrientation::Landscape, BookFontSize::Large) => 12,
            (false, ReaderOrientation::Landscape, BookFontSize::XLarge) => 10,
            (false, ReaderOrientation::Landscape, BookFontSize::XXLarge) => 9,
            (false, ReaderOrientation::Landscape, BookFontSize::XXXLarge) => 8,
        };
        ReaderLayout {
            available_width_px,
            lines_per_page,
            orientation: self.orientation,
            font_size: self.font_size,
            book_font: self.book_font,
            paragraph_alignment: self.paragraph_alignment,
            full_screen: self.full_screen,
        }
    }

    #[must_use]
    pub fn serialized(self) -> String {
        let show_progress = if self.show_progress { "true" } else { "false" };
        let tap_page_turn_enabled = if self.tap_page_turn_enabled {
            "true"
        } else {
            "false"
        };
        let full_screen = if self.full_screen { "true" } else { "false" };
        format!(
            "version={}\ntheme={}\norientation={}\nfont_size={}\nbook_font={}\nparagraph_alignment={}\nshow_progress={}\ntap_page_turn_enabled={}\nfull_screen={}\n",
            READER_PREFS_VERSION,
            self.theme.marker(),
            self.orientation.marker(),
            self.font_size.marker(),
            self.book_font.marker(),
            self.paragraph_alignment.marker(),
            show_progress,
            tap_page_turn_enabled,
            full_screen,
        )
    }

    fn parse(text: &str) -> Result<Self, String> {
        let mut prefs = Self::default();
        let mut version = None;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| "Reader preference line must contain '='".to_string())?;
            match key.trim() {
                "version" => version = Some(value.trim().to_string()),
                "theme" => prefs.theme = ReadingTheme::parse(value)?,
                "orientation" => prefs.orientation = ReaderOrientation::parse(value)?,
                "font_size" => prefs.font_size = BookFontSize::parse(value)?,
                "book_font" => prefs.book_font = BookFont::parse(value)?,
                "paragraph_alignment" => {
                    prefs.paragraph_alignment = ParagraphAlignment::parse(value)?
                }
                "show_progress" => {
                    prefs.show_progress = match value.trim() {
                        "true" => true,
                        "false" => false,
                        _ => return Err("show_progress must be true or false".into()),
                    }
                }
                "tap_page_turn_enabled" => {
                    prefs.tap_page_turn_enabled = match value.trim() {
                        "true" => true,
                        "false" => false,
                        _ => return Err("tap_page_turn_enabled must be true or false".into()),
                    }
                }
                "full_screen" => {
                    prefs.full_screen = match value.trim() {
                        "true" => true,
                        "false" => false,
                        _ => return Err("full_screen must be true or false".into()),
                    }
                }
                other => return Err(format!("unsupported Reader preference key {other:?}")),
            }
        }
        if version.as_deref() != Some(READER_PREFS_VERSION) {
            return Err("unsupported Reader preference version".into());
        }
        Ok(prefs)
    }
}

/// Coarse stages used by the e-paper loading screen. The runtime advances only
/// at meaningful boundaries so progress remains visible without excessive
/// refreshes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderLoadingStage {
    OpeningFile,
    InspectingEpubArchive,
    ReadingEpubPackage,
    LoadingEpubSpine,
    DetectingEncoding,
    LoadingSavedPosition,
    UpdatingLayout,
    BuildingFirstPage,
    IndexingNearbyPages,
    Ready,
    UnsupportedEpub,
    Failed,
}

impl ReaderLoadingStage {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::OpeningFile => "Opening file",
            Self::InspectingEpubArchive => "Inspecting EPUB archive",
            Self::ReadingEpubPackage => "Reading EPUB package",
            Self::LoadingEpubSpine => "Loading EPUB spine",
            Self::DetectingEncoding => "Detecting text encoding",
            Self::LoadingSavedPosition => "Loading saved position",
            Self::UpdatingLayout => "Updating layout cache",
            Self::BuildingFirstPage => "Building first page",
            Self::IndexingNearbyPages => "Caching nearby pages",
            Self::Ready => "Ready",
            Self::UnsupportedEpub => "Unsupported EPUB",
            Self::Failed => "Unable to open book",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for the loading screen's stage
    /// caption. This is a fixed set of stage names, not the dynamic
    /// status-message text built elsewhere in this module.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::OpeningFile => "Apertura file",
                Self::InspectingEpubArchive => "Controllo archivio EPUB",
                Self::ReadingEpubPackage => "Lettura pacchetto EPUB",
                Self::LoadingEpubSpine => "Caricamento struttura EPUB",
                Self::DetectingEncoding => "Rilevamento codifica testo",
                Self::LoadingSavedPosition => "Caricamento posizione salvata",
                Self::UpdatingLayout => "Aggiornamento cache layout",
                Self::BuildingFirstPage => "Creazione prima pagina",
                Self::IndexingNearbyPages => "Memorizzazione pagine vicine",
                Self::Ready => "Pronto",
                Self::UnsupportedEpub => "EPUB non supportato",
                Self::Failed => "Impossibile aprire il libro",
            },
        }
    }

    #[must_use]
    pub const fn progress(self) -> u8 {
        match self {
            Self::OpeningFile => 10,
            Self::InspectingEpubArchive => 20,
            Self::ReadingEpubPackage => 32,
            Self::LoadingEpubSpine => 44,
            Self::DetectingEncoding => 25,
            Self::LoadingSavedPosition => 40,
            Self::UpdatingLayout => 45,
            Self::BuildingFirstPage => 55,
            Self::IndexingNearbyPages => 80,
            Self::Ready => 100,
            Self::UnsupportedEpub | Self::Failed => 100,
        }
    }
}

/// Pending staged book open retained while the loading screen is visible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingReaderOpen {
    pub book: ReaderBook,
    pub stage: ReaderLoadingStage,
    pub encoding: Option<TextEncoding>,
    pub epub_document: Option<EpubDocument>,
    pub resume: Option<ReaderLocation>,
    pub message: String,
    /// Set when a freshly-parsed (never-cached) `EpubDocument` still needs
    /// its `.EPX` written. Carried into the resulting `ReaderSession` and
    /// persisted on a later background tick — see the comment on
    /// `epub_document_cache_pending` there for why this must not happen
    /// synchronously here.
    pub epub_document_cache_pending: bool,
}

/// One wrapped Reader line. `paragraph_end` prevents Justified rendering from
/// stretching the final line of a paragraph.
///
/// `image` is `Some` on the *first* of `ReaderPageImage::slot_span`
/// consecutive entries an inline EPUB image reserves; the remaining
/// `slot_span - 1` entries are blank continuation lines (`image: None`,
/// `text` empty). Every page-full/line-counting check in this module already
/// assumes one `Vec<ReaderPageLine>` entry equals one uniform `line_step` of
/// vertical space -- padding an image out to that many entries, instead of
/// teaching pagination and rendering a variable-height layout model, is what
/// lets an inline image slot into that existing grid unmodified. TXT
/// pagination and plain-text EPUB lines never set this field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderPageLine {
    pub text: String,
    pub paragraph_end: bool,
    pub image: Option<ReaderPageImage>,
}

/// One inline EPUB image reserved across `slot_span` consecutive
/// [`ReaderPageLine`] entries. `box_width` x `box_height` is the pixel box
/// the decoded bitmap is fitted into (aspect-ratio-preserving) and centered
/// within the slot span; it is also part of the SD bitmap cache key, so the
/// renderer and the Reader's own image prewarm must both use exactly these
/// values. See [`inline_image_slots`] for how both are chosen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderPageImage {
    pub href: String,
    pub alt: String,
    pub slot_span: usize,
    pub box_width: u16,
    pub box_height: u16,
}

/// Line-slots reserved for an inline EPUB image whose pixel size could not
/// be read from its header (see `EpubImage::width`). Clamped against
/// `ReaderLayout::lines_per_page` wherever it is used, so a very large font
/// size (few lines per page) still reserves a sane fraction of the page
/// rather than more slots than the page has.
const READER_INLINE_IMAGE_SLOT_SPAN: usize = 8;

/// Bound on `ReaderState::prewarmed_images`: comfortably more than the
/// images three consecutive pages can hold.
const READER_PREWARMED_IMAGE_LIMIT: usize = 32;

/// Largest upscale applied to a small inline image. Illustrations are
/// usually authored for screens around this panel's width, so filling the
/// column is right for them, but a small ornament or icon blown up many
/// times over turns into a blocky smear on a 1bpp panel.
const READER_INLINE_IMAGE_MAX_UPSCALE: u32 = 2;

/// Vertical distance between two consecutive Reader line baselines for
/// `layout`'s book font and size. Must match `render_page`'s own `line_step`.
fn reader_line_step(layout: &ReaderLayout) -> i32 {
    let style = crate::app::reader_typography::reader_body_style(
        layout.book_font,
        layout.font_size,
        ReadingTheme::Classic,
    );
    i32::from(style.line_height()) + 2
}

/// Slot span and fit box for one inline image.
///
/// * `standalone` (the image opens a page and nothing but whitespace
///   follows it in its chapter -- a cover or full-page plate) gets the whole
///   page, and the bitmap is fitted to fill it.
/// * An image whose pixel size is known gets exactly the slots its
///   aspect-correct height needs at column width (upscaled at most
///   [`READER_INLINE_IMAGE_MAX_UPSCALE`] times, and never taller than a
///   page).
/// * An image of unknown size falls back to [`READER_INLINE_IMAGE_SLOT_SPAN`].
fn inline_image_slots(
    image: &EpubImage,
    layout: &ReaderLayout,
    line_step: i32,
    standalone: bool,
) -> (usize, u16, u16) {
    let lines_per_page = layout.lines_per_page.max(1);
    let line_step = line_step.max(1);
    let column_width = layout.available_width_px.max(1) as u32;
    let box_for = |span: usize| (span as i32 * line_step - 2).max(1) as u32;
    let clamp_u16 = |value: u32| u16::try_from(value.max(1)).unwrap_or(u16::MAX);
    if standalone {
        return (
            lines_per_page,
            clamp_u16(column_width),
            clamp_u16(box_for(lines_per_page)),
        );
    }
    if image.width == 0 || image.height == 0 {
        let span = READER_INLINE_IMAGE_SLOT_SPAN.min(lines_per_page).max(1);
        return (span, clamp_u16(column_width), clamp_u16(box_for(span)));
    }
    let page_height = u64::from(box_for(lines_per_page));
    let (source_w, source_h) = (u64::from(image.width), u64::from(image.height));
    let mut width = u64::from(column_width)
        .min(source_w * u64::from(READER_INLINE_IMAGE_MAX_UPSCALE))
        .max(1);
    let mut height = (source_h * width / source_w).max(1);
    if height > page_height {
        height = page_height;
        width = (source_w * height / source_h).max(1);
    }
    let span = ((height as i32 + 2 + line_step - 1) / line_step)
        .clamp(1, lines_per_page as i32) as usize;
    (span, clamp_u16(width as u32), clamp_u16(height as u32))
}

/// In-reader dictionary lookup: hold SELECT to enter, then step from a line
/// cursor to a word cursor to a looked-up definition, one level per phase.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum ReaderDictionaryMode {
    #[default]
    Off,
    LineSelect {
        line_index: usize,
    },
    WordSelect {
        line_index: usize,
        word_index: usize,
    },
    Definition {
        line_index: usize,
        word_index: usize,
        word: String,
        /// The looked-up definition, or a diagnostic ("word not found" /
        /// dictionary-pack error) when no definition is available — always
        /// something displayable, so the panel never has to guess why.
        message: String,
    },
}

/// Word spans eligible for dictionary-mode selection: whitespace-delimited
/// tokens with surrounding punctuation trimmed, kept only when at least 3
/// characters remain. That length floor is the filter that keeps short
/// conjunctions and articles out of the word cursor.
#[must_use]
pub fn eligible_word_spans(line: &str) -> Vec<(usize, usize)> {
    word_token_spans(line)
        .into_iter()
        .filter_map(|(start, end)| trim_to_alnum_span(line, start, end))
        .filter(|(start, end)| line[*start..*end].chars().count() >= 3)
        .collect()
}

fn word_token_spans(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_end = 0;
    for (index, character) in line.char_indices() {
        if character.is_whitespace() {
            if let Some(span_start) = start.take() {
                spans.push((span_start, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
        last_end = index + character.len_utf8();
    }
    if let Some(span_start) = start {
        spans.push((span_start, last_end));
    }
    spans
}

/// Trims a token span down to its leading/trailing alphanumeric core, e.g.
/// `"casa,"` -> `"casa"`. Returns `None` for tokens with no alphanumeric
/// characters at all (bare punctuation).
fn trim_to_alnum_span(line: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let token = &line[start..end];
    let trim_start = token
        .char_indices()
        .find(|(_, character)| character.is_alphanumeric())
        .map(|(index, _)| start + index)?;
    let trim_end = token
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_alphanumeric())
        .map(|(index, character)| start + index + character.len_utf8())?;
    (trim_start < trim_end).then_some((trim_start, trim_end))
}

/// One cached portrait page and its byte anchor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderCachedPage {
    /// Absolute book-page index, independent of a cache-recovery base offset.
    pub page_index: usize,
    pub byte_offset: u64,
    pub next_byte_offset: u64,
    pub lines: Vec<ReaderPageLine>,
}

/// SD-backed page-anchor cache. The cache is intentionally text-based and
/// bounded so corrupt records can be rejected without blocking book opening.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ReaderAnchorCache {
    fingerprint: u64,
    base_page: usize,
    offsets: Vec<u64>,
    indexed_through: u64,
    complete: bool,
}

/// One EPUB chapter's layout-specific page anchors. Rebuilding this index scans
/// every page of the book, so it is persisted to a layout-fingerprinted SD
/// cache (see [`EPUB_PAGE_INDEX_CACHE_VERSION`]) and only recomputed when the
/// book or Reader layout changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderEpubChapterPages {
    pub chapter_number: usize,
    pub text_offset: u64,
    pub text_end_offset: u64,
    pub page_offsets: Vec<u64>,
}

/// The chapter currently being paginated page-by-page in the background (see
/// [`ReaderSession::index_one_epub_page`]), not yet complete enough to
/// finalize into `epub_chapter_pages`. Keeping this as in-progress state
/// (rather than pagination one whole chapter per step) bounds every single
/// background/on-demand indexing step to one page's wrap cost, matching TXT,
/// even for a book with one very large chapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingEpubChapterIndex {
    chapter: EpubChapter,
    page_offsets: Vec<u64>,
    next_offset: u64,
}

/// Active Reader session. Generated page anchors and nearby rendered pages remain
/// bounded in RAM and are rebuilt lazily when the reader advances.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderSession {
    pub book: ReaderBook,
    pub encoding: TextEncoding,
    pub epub_document: Option<EpubDocument>,
    pub layout: ReaderLayout,
    /// Local index within page_offsets.
    pub current_page: usize,
    /// Absolute page index represented by page_offsets[0]. Normally zero. A
    /// non-zero value is allowed when STATE.TXT survives but a cache is absent.
    pub page_number_base: usize,
    pub page_offsets: Vec<u64>,
    pub indexed_through: u64,
    pub index_complete: bool,
    pub cache: Vec<ReaderCachedPage>,
    pub epub_chapter_pages: Vec<ReaderEpubChapterPages>,
    pub epub_pending_chapter: Option<PendingEpubChapterIndex>,
    /// Set for a freshly-parsed EPUB (no `.EPX` cache hit) whose flattened
    /// text still needs to be persisted. Checked and cleared on the first
    /// background `tick()` after the session exists, so the (potentially
    /// multi-second, on this hardware's SD stack) `.EPX` write happens after
    /// the first page is already visible rather than before.
    pub epub_document_cache_pending: bool,
}

impl ReaderSession {
    #[must_use]
    pub fn current_absolute_page(&self) -> usize {
        self.page_number_base.saturating_add(self.current_page)
    }

    #[must_use]
    pub fn source_size_bytes(&self) -> u64 {
        self.epub_document
            .as_ref()
            .map_or(self.book.size_bytes, EpubDocument::text_size_bytes)
    }

    #[must_use]
    pub fn content_badge(&self) -> &'static str {
        self.book.format.badge()
    }

    #[must_use]
    pub fn toc_entries(&self) -> &[EpubTocEntry] {
        self.epub_document
            .as_ref()
            .map_or(&[], |document| document.toc.as_slice())
    }

    #[must_use]
    pub fn current_cached_page(&self) -> Option<&ReaderCachedPage> {
        let absolute = self.current_absolute_page();
        self.cache.iter().find(|page| page.page_index == absolute)
    }

    /// Whether the current page is a full-page image (a standalone cover
    /// or plate, see `inline_image_slots`), which the panel refresh policy
    /// cleans up around with a fast global refresh.
    #[must_use]
    pub fn current_page_is_full_page_image(&self) -> bool {
        self.current_cached_page().is_some_and(|page| {
            page.lines.iter().any(|line| {
                line.image
                    .as_ref()
                    .is_some_and(|image| image.slot_span >= self.layout.lines_per_page)
            })
        })
    }

    #[must_use]
    pub fn current_location(&self) -> ReaderLocation {
        let byte_offset = self
            .page_offsets
            .get(self.current_page)
            .copied()
            .or_else(|| self.current_cached_page().map(|page| page.byte_offset))
            .unwrap_or(0);
        ReaderLocation {
            path: self.book.path.clone(),
            title: self.book.title.clone(),
            format: self.book.format,
            size_bytes: self.book.size_bytes,
            modified_seconds: self.book.modified_seconds,
            page_index: self.current_absolute_page(),
            byte_offset,
            epub_chapter: self.epub_chapter_page_label_for_offset(byte_offset),
            reading_percent: self.reading_percent(),
        }
    }

    /// Character-offset upper bound of the EPUB chapter containing the
    /// current position, for reading-time-remaining estimates (see
    /// `reading_stats`). `None` for TXT books (no chapter boundaries) or
    /// before the containing chapter has finished indexing.
    #[must_use]
    pub fn current_chapter_end_offset(&self) -> Option<u64> {
        let offset = self.current_location().byte_offset;
        self.epub_chapter_pages
            .iter()
            .find(|chapter| offset >= chapter.text_offset && offset < chapter.text_end_offset)
            .map(|chapter| chapter.text_end_offset)
    }

    #[must_use]
    pub fn progress_percent(&self) -> u8 {
        let source_size = self.source_size_bytes();
        if source_size == 0 {
            return 100;
        }
        ((self.indexed_through.saturating_mul(100) / source_size).min(100)) as u8
    }

    #[must_use]
    pub fn page_label(&self) -> String {
        if self.index_complete {
            format!(
                "{}/{}",
                self.current_absolute_page() + 1,
                self.page_number_base + self.page_offsets.len()
            )
        } else {
            format!("{}+", self.current_absolute_page() + 1)
        }
    }

    /// Reading-progress percentage. Once background indexing has walked all
    /// the way to the book's end *and* the known page run started from the
    /// book's true first page, this is exact (current page versus the now
    /// fully-known total page count). Otherwise it falls back to the current
    /// byte offset versus the total byte size, which is always known
    /// immediately (an EPUB's flattened text length comes from parsing, not
    /// pagination; a TXT's file size is just its size on disk). This is an
    /// approximation — pages aren't evenly sized in bytes — but it moves in
    /// the right direction and lands close, which is a large improvement
    /// over showing nothing at all for most of a book.
    ///
    /// The "started from the true first page" caveat matters because an EPUB
    /// resume can land mid-book with no matching `.EPP` cache: indexing then
    /// only ever paginates forward from the resume chapter, never revisiting
    /// the chapters before it (see `open_epub_session`), so `index_complete`
    /// can go true — meaning indexing reached the book's *end* — while
    /// `page_number_base` (hardcoded to 0 for EPUB) and `page_offsets` still
    /// omit every page before the resume point entirely. Trusting the
    /// page-count formula there would divide a near-zero "current page"
    /// (counted from the resume point, not the book's start) by an
    /// undercounted total, reporting close to 0% for a book the reader may
    /// be well into.
    #[must_use]
    pub fn reading_percent(&self) -> Option<u8> {
        let source_size = self.source_size_bytes();
        if source_size == 0 {
            return Some(100);
        }
        if self.index_complete && self.epub_index_covers_book_start() {
            let total = self.page_number_base + self.page_offsets.len();
            if total == 0 {
                return Some(100);
            }
            let current = (self.current_absolute_page() + 1).min(total);
            return Some(((current * 100) / total) as u8);
        }
        let current_offset = self
            .page_offsets
            .get(self.current_page)
            .copied()
            .or_else(|| self.current_cached_page().map(|page| page.byte_offset))
            .unwrap_or(0);
        Some((current_offset.saturating_mul(100) / source_size).min(100) as u8)
    }

    /// Whether the known EPUB chapter/page range includes the book's true
    /// first chapter (text offset 0). Always `true` for TXT, which anchors
    /// `page_number_base` to the resumed location's real absolute page index
    /// instead of hardcoding it to 0 (see `open_txt_session`), so its page
    /// count is trustworthy regardless of where a session resumed.
    #[must_use]
    fn epub_index_covers_book_start(&self) -> bool {
        if self.epub_document.is_none() {
            return true;
        }
        if let Some(first) = self.epub_chapter_pages.first() {
            return first.text_offset == 0;
        }
        self.epub_pending_chapter
            .as_ref()
            .is_none_or(|pending| pending.chapter.text_offset == 0)
    }

    #[must_use]
    pub fn reading_percent_label(&self) -> String {
        self.reading_percent()
            .map_or_else(|| "--%".into(), |percent| format!("{percent}%"))
    }

    /// Product-facing page label. TXT keeps the accepted book-relative label;
    /// EPUB uses a chapter-relative label as requested by the Reader UI.
    #[must_use]
    pub fn display_page_label(&self) -> String {
        self.current_epub_chapter_page_label().map_or_else(
            || format!("PAGE {}", self.page_label()),
            |chapter| {
                format!(
                    "CH {}  PAGE {}",
                    chapter.chapter_number,
                    chapter.page_text()
                )
            },
        )
    }

    #[must_use]
    pub fn current_epub_chapter_page_label(&self) -> Option<ReaderChapterPageLabel> {
        let offset = self
            .page_offsets
            .get(self.current_page)
            .copied()
            .or_else(|| self.current_cached_page().map(|page| page.byte_offset))?;
        self.epub_chapter_page_label_for_offset(offset)
    }

    #[must_use]
    pub fn epub_chapter_page_label_for_offset(
        &self,
        offset: u64,
    ) -> Option<ReaderChapterPageLabel> {
        let chapter = self.epub_chapter_pages.iter().find(|chapter| {
            offset >= chapter.text_offset
                && (offset < chapter.text_end_offset
                    || (offset == chapter.text_end_offset
                        && chapter.text_end_offset == self.source_size_bytes()))
        })?;
        let page_number = chapter
            .page_offsets
            .partition_point(|anchor| *anchor <= offset)
            .max(1);
        let chapter_number = self
            .toc_chapter_number_for_offset(offset)
            .unwrap_or(chapter.chapter_number);
        Some(ReaderChapterPageLabel {
            chapter_number,
            page_number,
            page_count: chapter.page_offsets.len().max(1),
        })
    }

    /// The 1-based position, within the book's actual table of contents, of
    /// the TOC entry that owns `offset` (the last entry whose `text_offset`
    /// does not exceed it). EPUB spine files — what `ReaderEpubChapterPages`
    /// counts — rarely line up one-to-one with TOC entries: a cover, title
    /// page or copyright page is often its own spine file with no TOC entry
    /// of its own, so the raw spine ordinal drifts ahead of the chapter
    /// number the TOC (and the reader's own index) shows for the same
    /// position. `None` when the book has no structured TOC, so callers fall
    /// back to the spine ordinal.
    #[must_use]
    fn toc_chapter_number_for_offset(&self, offset: u64) -> Option<usize> {
        let toc = &self.epub_document.as_ref()?.toc;
        toc.iter()
            .rposition(|entry| entry.text_offset <= offset)
            .map(|index| index + 1)
    }

    fn push_cached_page(&mut self, page: ReaderCachedPage) {
        if let Some(existing) = self
            .cache
            .iter_mut()
            .find(|cached| cached.page_index == page.page_index)
        {
            *existing = page;
            return;
        }
        self.cache.push(page);
        self.cache.sort_by_key(|page| page.page_index);
        while self.cache.len() > READER_NEARBY_PAGE_CACHE {
            let current = self.current_absolute_page();
            let remove = if current.saturating_sub(self.cache[0].page_index)
                > self
                    .cache
                    .last()
                    .map_or(0, |page| page.page_index.saturating_sub(current))
            {
                0
            } else {
                self.cache.len() - 1
            };
            self.cache.remove(remove);
        }
    }

    fn ensure_page_cached(&mut self, local_page_index: usize) -> Result<(), String> {
        let absolute = self.page_number_base.saturating_add(local_page_index);
        if self.cache.iter().any(|page| page.page_index == absolute) {
            return Ok(());
        }
        let offset = *self
            .page_offsets
            .get(local_page_index)
            .ok_or_else(|| "page anchor is not indexed yet".to_string())?;
        let page = read_reader_page(
            &self.book,
            self.encoding,
            self.layout,
            self.epub_document.as_ref(),
            offset,
            absolute,
        )?;
        self.push_cached_page(page);
        Ok(())
    }

    /// Advance background indexing by exactly one page, for TXT or EPUB
    /// alike (see [`Self::index_one_epub_page`] for EPUB's chapter-crossing
    /// bookkeeping). Callers (`tick()`, `next_page()`) stay format-agnostic.
    fn index_one_page(&mut self) -> Result<bool, String> {
        match self.book.format {
            BookFormat::Text => self.index_one_txt_page(),
            BookFormat::Epub => self.index_one_epub_page(),
        }
    }

    fn index_one_txt_page(&mut self) -> Result<bool, String> {
        if self.index_complete {
            return Ok(false);
        }
        let absolute_page = self
            .page_number_base
            .saturating_add(self.page_offsets.len());
        let offset = self.indexed_through;
        let source_size = self.source_size_bytes();
        if offset >= source_size {
            self.index_complete = true;
            return Ok(false);
        }
        let page = read_reader_page(
            &self.book,
            self.encoding,
            self.layout,
            self.epub_document.as_ref(),
            offset,
            absolute_page,
        )?;
        if page.next_byte_offset <= offset {
            self.index_complete = true;
            return Ok(false);
        }
        self.page_offsets.push(offset);
        self.indexed_through = page.next_byte_offset;
        self.index_complete = self.indexed_through >= source_size;
        self.push_cached_page(page);
        Ok(true)
    }

    /// Paginate exactly the next not-yet-indexed EPUB chapter (the one
    /// starting at `indexed_through`, which only ever advances to a chapter
    /// boundary) and append it to `epub_chapter_pages`/`page_offsets`. Unlike
    /// [`Self::index_one_txt_page`] this does not push pages into the RAM
    /// nearby-page cache: only the page the reader actually navigates to is
    /// cached, via `ensure_page_cached`. That's deliberate, not an
    /// oversight — this step must keep running to completion in the
    /// background (building `epub_chapter_pages` for the persisted `.EPX`
    /// cache) independent of the small nearby-page cache's size, and gating
    /// it on cache room the way TXT does would stall whole-book indexing
    /// after the first `READER_NEARBY_PAGE_CACHE` pages whenever the reader
    /// hasn't moved far enough to evict any of them. `tick()` instead runs a
    /// separate, small lookahead step (see the call to `ensure_page_cached`
    /// for `current_page + 1` below) to get the *next* page pre-rendered
    /// without coupling it to indexing progress. Bounding synchronous work to one
    /// chapter (rather than one page) reuses the existing per-chapter
    /// pagination loop and keeps `tick()`'s background loop making real
    /// forward progress every call instead of needing page-level plumbing.
    /// Advance background EPUB indexing by exactly one page: bounds every
    /// step to one page's word-wrap cost (same as TXT), including across a
    /// chapter boundary — a whole-chapter-at-once step turned out to still be
    /// perceptible on `next_page()` when the reader ran ahead of background
    /// indexing and crossed into a fresh chapter, and would fully block on a
    /// single very large chapter. Progress toward the current in-progress
    /// chapter lives in `epub_pending_chapter`, chosen by ordinal position
    /// (not by looking up `indexed_through` as a byte offset — chapters are
    /// joined with a "\n\n" separator in the flattened text, so a finished
    /// chapter's end normally lands *between* chapters, not inside the next
    /// one, and `chapter_for_offset` would spuriously find no match there).
    /// It is finalized into `epub_chapter_pages` once fully paginated.
    fn index_one_epub_page(&mut self) -> Result<bool, String> {
        if self.index_complete {
            return Ok(false);
        }
        let Some(document) = self.epub_document.as_ref() else {
            self.index_complete = true;
            return Ok(false);
        };
        let source_size = document.text_size_bytes();

        if self.epub_pending_chapter.is_none() {
            let next_chapter_number = self
                .epub_chapter_pages
                .last()
                .map_or(1, |chapter| chapter.chapter_number + 1);
            let Some(chapter) = document
                .chapters
                .iter()
                .find(|chapter| chapter.number == next_chapter_number)
                .cloned()
            else {
                self.index_complete = true;
                return Ok(false);
            };
            self.epub_pending_chapter = Some(PendingEpubChapterIndex {
                next_offset: chapter.text_offset,
                chapter,
                page_offsets: Vec::new(),
            });
        }

        let pending = self
            .epub_pending_chapter
            .as_mut()
            .expect("seeded immediately above");
        if pending.next_offset >= pending.chapter.text_end_offset {
            let finished = self
                .epub_pending_chapter
                .take()
                .expect("checked Some above");
            self.epub_chapter_pages.push(ReaderEpubChapterPages {
                chapter_number: finished.chapter.number,
                text_offset: finished.chapter.text_offset,
                text_end_offset: finished.chapter.text_end_offset,
                page_offsets: finished.page_offsets,
            });
            self.index_complete = self.indexed_through >= source_size;
            return Ok(true);
        }

        if self.page_offsets.len() >= READER_EPUB_PAGE_ANCHOR_LIMIT {
            return Err(format!(
                "EPUB pagination exceeds {} page anchor limit",
                READER_EPUB_PAGE_ANCHOR_LIMIT
            ));
        }
        let offset = pending.next_offset;
        let page = read_epub_page_until(
            document,
            self.layout,
            offset,
            pending.page_offsets.len(),
            pending.chapter.text_end_offset,
        )?;
        if page.next_byte_offset <= offset {
            return Err(format!(
                "EPUB chapter {} pagination did not advance",
                pending.chapter.number
            ));
        }
        pending.page_offsets.push(offset);
        pending.next_offset = page.next_byte_offset.min(pending.chapter.text_end_offset);
        self.page_offsets.push(offset);
        self.indexed_through = pending.next_offset;
        Ok(true)
    }

    pub fn next_page(&mut self) -> Result<(), String> {
        let target = self.current_page.saturating_add(1);
        while target >= self.page_offsets.len() && !self.index_complete {
            self.index_one_page()?;
        }
        if target < self.page_offsets.len() {
            self.current_page = target;
            self.ensure_page_cached(target)?;
        }
        Ok(())
    }

    pub fn previous_page(&mut self) -> Result<(), String> {
        if self.current_page == 0 && self.book.format == BookFormat::Epub {
            self.extend_backward()?;
        }
        if self.current_page > 0 {
            self.current_page -= 1;
            self.ensure_page_cached(self.current_page)?;
        }
        Ok(())
    }

    /// Pull one more chapter's worth of already-passed pages into
    /// `page_offsets` when the reader hits the start of what's currently
    /// known. This only happens for a session that was lazily seeded at an
    /// arbitrary resume point (see `open_epub_session`): a fresh open
    /// already starts at the book's true beginning, so there is nothing to
    /// pull in and this is a no-op. Bounded to one chapter per call — the
    /// same chapter's earlier pages if `page_offsets[0]` isn't already at
    /// its chapter's start, otherwise the whole previous chapter — never the
    /// rest of the book, so a long backward skim still costs one bounded
    /// step per chapter crossed instead of a single unbounded scan.
    fn extend_backward(&mut self) -> Result<bool, String> {
        let Some(document) = self.epub_document.as_ref() else {
            return Ok(false);
        };
        let Some(&first_offset) = self.page_offsets.first() else {
            return Ok(false);
        };
        let Some(current_chapter) = document.chapter_for_offset(first_offset).cloned() else {
            return Ok(false);
        };
        let (target_chapter, limit) = if first_offset > current_chapter.text_offset {
            let limit = first_offset.saturating_sub(1);
            (current_chapter, limit)
        } else if current_chapter.number > 1 {
            let Some(previous) = document
                .chapters
                .iter()
                .find(|chapter| chapter.number == current_chapter.number - 1)
                .cloned()
            else {
                return Ok(false);
            };
            let limit = previous.text_end_offset;
            (previous, limit)
        } else {
            // `page_offsets[0]` is already the book's true first page.
            return Ok(false);
        };
        let prepend = paginate_epub_chapter_up_to(
            document,
            self.layout,
            &target_chapter,
            limit,
            self.page_offsets.len(),
        )?;
        if prepend.is_empty() {
            return Ok(false);
        }
        let prepended_count = prepend.len();
        let mut new_offsets = prepend;
        new_offsets.extend(self.page_offsets.iter().copied());
        self.page_offsets = new_offsets;
        self.current_page += prepended_count;
        // Cached pages are keyed by absolute index (`page_number_base` plus
        // local index). When the base can absorb the prepended pages, every
        // existing page keeps its absolute index. When it cannot (always the
        // case for EPUB, whose base starts at 0), the pages already cached
        // move `shift` places later: without renumbering them, the newly
        // prepended pages' absolute indices collide with theirs, so
        // `ensure_page_cached` "finds" a stale later page and keeps showing
        // it instead of the chapter the reader just paged back into (the
        // cover never appeared when paging back to the start of a book).
        let shift = prepended_count.saturating_sub(self.page_number_base);
        self.page_number_base = self.page_number_base.saturating_sub(prepended_count);
        if shift > 0 {
            for page in &mut self.cache {
                page.page_index += shift;
            }
        }
        Ok(true)
    }

    #[must_use]
    fn anchor_cache(&self) -> Option<ReaderAnchorCache> {
        if self.book.format != BookFormat::Text {
            return None;
        }
        Some(ReaderAnchorCache {
            fingerprint: book_fingerprint(&self.book, self.layout),
            base_page: self.page_number_base,
            offsets: self.page_offsets.clone(),
            indexed_through: self.indexed_through,
            complete: self.index_complete,
        })
    }
}

/// Reader Options actions, drawn as a 2x2 grid of Home-style icon tiles
/// (`screens::reader::render_options`). Editable values live on the
/// separate Reading Preferences editor so menu controls match the rest of
/// the firmware. Ghost cleanup is not offered here: the panel already runs a
/// full cleanup refresh periodically (`PANEL_PARTIAL_REFRESH_LIMIT`) and the
/// power-key menu keeps a manual one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderOption {
    TableOfContents,
    Bookmarks,
    Bookmark,
    ReadingPreferences,
}

impl ReaderOption {
    /// Grid order, row by row: the wheel walks it linearly.
    pub const ALL: [Self; 4] = [
        Self::TableOfContents,
        Self::Bookmarks,
        Self::Bookmark,
        Self::ReadingPreferences,
    ];

    /// Short tile title. `bookmarked` (whether the current page already
    /// carries a bookmark) picks the add/remove wording for the toggle.
    #[must_use]
    pub const fn tile_label_i18n(self, locale: Locale, bookmarked: bool) -> &'static str {
        match (locale, self) {
            (Locale::English, Self::TableOfContents) => "Contents",
            (Locale::English, Self::Bookmarks) => "Bookmarks",
            (Locale::English, Self::Bookmark) if bookmarked => "Unmark page",
            (Locale::English, Self::Bookmark) => "Mark page",
            (Locale::English, Self::ReadingPreferences) => "Preferences",
            (Locale::Italian, Self::TableOfContents) => "Indice",
            (Locale::Italian, Self::Bookmarks) => "Segnalibri",
            (Locale::Italian, Self::Bookmark) if bookmarked => "Togli segno",
            (Locale::Italian, Self::Bookmark) => "Segna pagina",
            (Locale::Italian, Self::ReadingPreferences) => "Preferenze",
        }
    }
}

/// Rows on the Library long-press "book actions" overlay
/// (`ScreenRoute::LibraryBookActions`), opened by holding SELECT on a cover
/// in the Library grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryBookAction {
    MarkCompleted,
    Bookmarks,
}

impl LibraryBookAction {
    pub const ALL: [Self; 2] = [Self::MarkCompleted, Self::Bookmarks];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MarkCompleted => "Mark as Completed",
            Self::Bookmarks => "Bookmarks",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for the book-actions rows.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::MarkCompleted => "Segna come completato",
                Self::Bookmarks => "Segnalibri",
            },
        }
    }
}

/// Reading Preferences editor rows. On the flat list, UP/DOWN move the
/// highlighted row and SELECT opens that row's editor; inside the editor,
/// UP/DOWN browse candidate values (with a live preview) and SELECT commits
/// the highlighted candidate, while BACK discards it and steps back to the
/// list one level at a time (only a second BACK, from the flat list itself,
/// leaves for Reader Options). See `ReaderUiState::preference_edit`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadingPreference {
    ReadingTheme,
    Orientation,
    BookFontSize,
    BookFont,
    ParagraphAlignment,
    ShowProgress,
    TapPageTurn,
    FullScreen,
}

impl ReadingPreference {
    pub const ALL: [Self; 7] = [
        Self::ReadingTheme,
        Self::Orientation,
        Self::BookFontSize,
        Self::BookFont,
        Self::ParagraphAlignment,
        Self::TapPageTurn,
        Self::FullScreen,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReadingTheme => "Reading Theme",
            Self::Orientation => "Orientation",
            Self::BookFontSize => "Book Font Size",
            Self::BookFont => "Book Font",
            Self::ParagraphAlignment => "Paragraph Alignment",
            Self::ShowProgress => "Show Progress",
            Self::TapPageTurn => "Tap Page-Turn",
            Self::FullScreen => "Full Screen",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for the Reading Preferences
    /// list rows and editor headers.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::ReadingTheme => "Tema di lettura",
                Self::Orientation => "Orientamento",
                Self::BookFontSize => "Dimensione carattere",
                Self::BookFont => "Carattere libro",
                Self::ParagraphAlignment => "Allineamento paragrafo",
                Self::ShowProgress => "Mostra progresso",
                Self::TapPageTurn => "Cambio pagina a tocco",
                Self::FullScreen => "Schermo intero",
            },
        }
    }

    /// Short uppercase title for the row's editor header, which shares its
    /// line with the battery status and the clock: the full row label (e.g.
    /// "Allineamento paragrafo") is too wide to fit between them.
    #[must_use]
    pub const fn header_label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => match self {
                Self::ReadingTheme => "THEME",
                Self::Orientation => "ORIENTATION",
                Self::BookFontSize => "FONT SIZE",
                Self::BookFont => "FONT",
                Self::ParagraphAlignment => "ALIGNMENT",
                Self::ShowProgress => "PROGRESS",
                Self::TapPageTurn => "PAGE TURN",
                Self::FullScreen => "FULL SCREEN",
            },
            Locale::Italian => match self {
                Self::ReadingTheme => "TEMA",
                Self::Orientation => "ROTAZIONE",
                Self::BookFontSize => "DIMENSIONE",
                Self::BookFont => "CARATTERE",
                Self::ParagraphAlignment => "ALLINEAMENTO",
                Self::ShowProgress => "PROGRESSO",
                Self::TapPageTurn => "GIRA PAGINA",
                Self::FullScreen => "SCHERMO",
            },
        }
    }
}

/// Coarse background tick result used by main.rs to refresh only meaningful
/// loading-screen transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderTickOutcome {
    None,
    LoadingStageChanged,
    FirstPageReady,
    BackgroundCacheAdvanced,
    Failed,
}

/// Non-fatal Reader persistence startup report.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReaderPersistenceReport {
    pub state_loaded: bool,
    pub preferences_loaded: bool,
    pub position_count: usize,
    pub recent_count: usize,
    pub bookmark_count: usize,
    pub warning: Option<String>,
}

/// Hardware-independent Reader UI state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderUiState {
    pub books_root: String,
    pub state_root: String,
    pub books: Vec<ReaderBook>,
    pub positions: Vec<ReaderLocation>,
    pub recent: Vec<ReaderLocation>,
    pub bookmarks: Vec<ReaderLocation>,
    pub resume: Option<ReaderLocation>,
    pub preferences: ReaderPreferences,
    pub library_error: Option<String>,
    pub persistence_warning: Option<String>,
    /// Index into the merged Recent + All entry list (`visible_entries`).
    pub library_selected: usize,
    pub bookmarks_selected: usize,
    /// The book the Library long-press "book actions" overlay
    /// (`ScreenRoute::LibraryBookActions`) and its bookmarks sub-screen
    /// (`ScreenRoute::LibraryBookBookmarks`) operate on. `None` when neither
    /// is open.
    pub book_actions_target: Option<ReaderBook>,
    pub book_actions_selected: usize,
    pub book_bookmarks_selected: usize,
    pub toc_selected: usize,
    pub loading: Option<PendingReaderOpen>,
    pub session: Option<ReaderSession>,
    /// Warm, already-opened sessions for books other than the active one,
    /// most-recently-used first, bounded to `READER_SESSION_CACHE_LIMIT`.
    /// Reopening one of these (`promote_cached_session`) is a plain swap —
    /// no SD access at all — the same shortcut `request_open_visible`
    /// already gave the single active session. Populated both by switching
    /// away from a book during this boot (`release_active_session_for_open`)
    /// and by `tick_background_warmup` pre-loading Recent's top entries
    /// right after boot.
    pub session_cache: Vec<ReaderSession>,
    /// Books still waiting for `tick_background_warmup` to try warming them
    /// into `session_cache`. Seeded once at boot from Recent.
    warmup_queue: Vec<ReaderLocation>,
    /// Inline images `prewarm_inline_images_best_effort` has already made
    /// sure are in the SD bitmap cache, keyed by book path, href and fit
    /// box. Only saves re-checking SD every tick; bounded to
    /// [`READER_PREWARMED_IMAGE_LIMIT`] most recent entries.
    prewarmed_images: Vec<(String, String, u16, u16)>,
    /// Decoded thumbnails for books currently visible on the Library screen,
    /// keyed by [`ReaderBook::path`]. Populated by the main loop (which owns
    /// SD access) from [`crate::cover_cache::CoverCache`]; `render_library`
    /// only ever reads this map, never touches SD itself. Cleared whenever
    /// the Library screen is left, so this stays bounded to roughly one
    /// screenful rather than growing across a full library scroll.
    pub library_thumbnails: std::collections::HashMap<String, crate::cover_cache::CachedThumbnail>,
    /// Cover thumbnail for the Home dashboard's Continue Reading tile, keyed
    /// by the book path it was generated for so a book change is detected
    /// without re-fingerprinting on every frame. Unlike `library_thumbnails`,
    /// this is never cleared on route change: it is a single book's cover
    /// rather than a full screenful, so keeping it around is cheap and
    /// avoids a re-fetch flicker each time Home is revisited.
    pub continue_reading_thumbnail: Option<(String, crate::cover_cache::CachedThumbnail)>,
    pub options_selected: usize,
    pub dictionary_mode: ReaderDictionaryMode,
    /// INDEX.TXT handle, opened once. It holds no rows: loading a full pack's
    /// index (~600 KB) up front was what made the first lookup slow, so
    /// lookups binary-search the sorted file on SD instead.
    dictionary_index_cache: Option<DictionaryIndex>,
    pub preferences_selected: usize,
    /// `None` while the ReadingPreferences list is flat; `Some(candidate)`
    /// while a row's editor is open. `candidate` is a full copy of
    /// `preferences` with only the active field varying as UP/DOWN browse
    /// options, so nothing is persisted or re-paginated until SELECT commits
    /// it (BACK just drops it).
    pub preference_edit: Option<ReaderPreferences>,
    preferences_layout_dirty: bool,
    pub last_message: Option<String>,
    persistence_event: Option<String>,
    last_persistence_event: Option<String>,
    clear_ghost_requested: bool,
    /// Set by `next_page`/`previous_page` instead of saving inline: the
    /// e-paper refresh that shows the new page is the part the user is
    /// actually waiting on, and STATE/POSITS/RECENT's `fsync` can cost
    /// hundreds of ms to low seconds on this hardware's SD/FAT stack (see
    /// `persist_current_session_best_effort`). Actually written once
    /// `pending_persist_since` shows the reader has been sitting on the page
    /// for `READER_PERSIST_DEBOUNCE` (checked from `tick`), so flipping
    /// through several pages in a row costs one SD save, not one per page.
    /// `flush_pending_persist` forces the save immediately regardless of
    /// that timer, for the moments a deferred save would otherwise be lost
    /// or delayed indefinitely: leaving the Reader route (main.rs) and
    /// entering deep sleep (real deep sleep is a full reboot -- nothing in
    /// RAM survives it).
    pending_persist: bool,
    /// When `pending_persist` was last set; `None` once flushed. See
    /// `pending_persist`.
    pending_persist_since: Option<Instant>,
}

impl Default for ReaderUiState {
    fn default() -> Self {
        Self {
            books_root: READER_BOOKS_DIRECTORY.into(),
            state_root: READER_STATE_DIRECTORY.into(),
            books: Vec::new(),
            positions: Vec::new(),
            recent: Vec::new(),
            bookmarks: Vec::new(),
            resume: None,
            preferences: ReaderPreferences::default(),
            library_error: None,
            persistence_warning: None,
            library_selected: 0,
            bookmarks_selected: 0,
            book_actions_target: None,
            book_actions_selected: 0,
            book_bookmarks_selected: 0,
            toc_selected: 0,
            loading: None,
            session: None,
            session_cache: Vec::new(),
            warmup_queue: Vec::new(),
            prewarmed_images: Vec::new(),
            library_thumbnails: std::collections::HashMap::new(),
            continue_reading_thumbnail: None,
            options_selected: 0,
            dictionary_mode: ReaderDictionaryMode::Off,
            dictionary_index_cache: None,
            preferences_selected: 0,
            preference_edit: None,
            preferences_layout_dirty: false,
            last_message: None,
            persistence_event: None,
            last_persistence_event: None,
            clear_ghost_requested: false,
            pending_persist: false,
            pending_persist_since: None,
        }
    }
}

impl ReaderUiState {
    #[must_use]
    pub fn with_books_root(root: impl Into<String>) -> Self {
        Self {
            books_root: root.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_roots(books_root: impl Into<String>, state_root: impl Into<String>) -> Self {
        Self {
            books_root: books_root.into(),
            state_root: state_root.into(),
            ..Self::default()
        }
    }

    /// Load persisted state without making startup dependent on removable
    /// storage. Corrupt records are ignored and reported as a warning.
    pub fn load_persistent_state(&mut self) -> ReaderPersistenceReport {
        let _span = crate::boot_profile::span("reader-load-persistent-state");
        let mut warnings = Vec::new();
        let file_span = crate::boot_profile::span("reader-load-prefs");
        let preferences_loaded = match load_preferences(&self.preferences_path()) {
            Ok(Some(preferences)) => {
                self.preferences = preferences;
                true
            }
            Ok(None) => false,
            Err(error) => {
                warnings.push(format!("PREFS.TXT: {error}"));
                false
            }
        };
        file_span.end();
        let file_span = crate::boot_profile::span("reader-load-state");
        self.resume = match load_location_record(&self.state_path()) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("STATE.TXT: {error}"));
                None
            }
        };
        file_span.end();
        let file_span = crate::boot_profile::span("reader-load-positions");
        self.positions = match self.load_positions_with_legacy_migration() {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("POSITS.TXT: {error}"));
                quarantine_unreadable_state(&self.positions_path());
                Vec::new()
            }
        };
        file_span.end();
        let file_span = crate::boot_profile::span("reader-load-recent");
        self.recent = match load_location_list(&self.recent_path(), READER_RECENT_LIMIT) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("RECENT.TXT: {error}"));
                quarantine_unreadable_state(&self.recent_path());
                Vec::new()
            }
        };
        // Every Recent entry is also a saved position (both are written
        // together on each save), so Recent doubles as a second copy: any
        // book it holds that `positions` lost is restored from it here,
        // instead of showing as "New" and restarting from the beginning once
        // it drops out of Recent.
        let mut restored = 0_usize;
        for location in &self.recent {
            if !self.positions.iter().any(|entry| entry.path == location.path) {
                self.positions.push(location.clone());
                restored += 1;
            }
        }
        self.positions.truncate(READER_POSITION_LIMIT);
        if restored > 0 {
            log::warn!(
                "rustmix-wave=reader-persistence status=positions-restored-from-recent count={restored}"
            );
        }
        file_span.end();
        let file_span = crate::boot_profile::span("reader-load-bookmarks");
        self.bookmarks = match load_location_list(&self.bookmarks_path(), READER_BOOKMARK_LIMIT) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("MARKS.TXT: {error}"));
                quarantine_unreadable_state(&self.bookmarks_path());
                Vec::new()
            }
        };
        file_span.end();
        self.bookmarks_selected = self
            .bookmarks_selected
            .min(self.bookmarks.len().saturating_sub(1));
        let warning = if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("; "))
        };
        self.persistence_warning = warning.clone();
        ReaderPersistenceReport {
            state_loaded: self.resume.is_some(),
            preferences_loaded,
            position_count: self.positions.len(),
            recent_count: self.recent.len(),
            bookmark_count: self.bookmarks.len(),
            warning,
        }
    }

    pub fn refresh_library(&mut self) {
        match scan_txt_library(&self.books_root, &self.books) {
            Ok(books) => {
                self.books = books;
                self.library_error = None;
                self.prune_stale_locations();
            }
            Err(error) => {
                // Scan failure (e.g. SD not yet mounted) doesn't mean the books are
                // gone: pruning against an empty list here would wipe Recent/positions
                // for a merely-transient error, so only prune on a successful scan.
                self.books.clear();
                self.library_error = Some(error);
            }
        }
        self.library_selected = 0;
    }

    /// Drop Recent/positions entries whose book no longer matches a file found
    /// by the just-completed scan, so books deleted from the SD card stop
    /// appearing in the Library's Recent section and stay gone after a reboot.
    fn prune_stale_locations(&mut self) {
        let still_present =
            |location: &ReaderLocation| self.books.iter().any(|book| location.matches_book(book));
        let recent_before = self.recent.len();
        self.recent.retain(still_present);
        let positions_before = self.positions.len();
        self.positions.retain(still_present);
        if self.recent.len() == recent_before && self.positions.len() == positions_before {
            return;
        }
        let mut errors = Vec::new();
        if let Err(error) = atomic_replace_text(
            &self.positions_path(),
            &serialize_location_list(&self.positions),
        ) {
            errors.push(format!("POSITS.TXT: {error}"));
        }
        if let Err(error) =
            atomic_replace_text(&self.recent_path(), &serialize_location_list(&self.recent))
        {
            errors.push(format!("RECENT.TXT: {error}"));
        }
        self.finish_persistence("prune-stale-locations", errors);
    }

    #[must_use]
    pub fn can_continue(&self) -> bool {
        self.session.is_some() || self.resume.is_some() || !self.recent.is_empty()
    }

    /// Callers must check `loading.is_some()` afterward to route to
    /// `ReaderLoading` (a real reload was queued) versus straight to
    /// `ReaderPage` (a cache hit already made `session` current).
    pub fn request_continue(&mut self) -> bool {
        let Some(location) = self.resume.clone().or_else(|| self.recent.first().cloned()) else {
            return false;
        };
        if self.promote_cached_session(&location.as_book()) {
            return true;
        }
        self.request_open_book(location.as_book(), Some(location));
        true
    }

    /// Park `session` in the bounded warm-session cache (most-recently-used
    /// first), evicting the oldest entry past `READER_SESSION_CACHE_LIMIT`.
    /// Replaces any existing entry for the same book rather than
    /// duplicating it.
    fn cache_session(&mut self, session: ReaderSession) {
        self.session_cache
            .retain(|cached| cached.book.path != session.book.path);
        self.session_cache.insert(0, session);
        self.session_cache.truncate(READER_SESSION_CACHE_LIMIT);
    }

    /// Drop every parked (warmed but not actively open) book session right
    /// before an OTA install attempt, so its worker gets the largest
    /// possible contiguous internal-RAM block for its 64 KiB stack. Safe to
    /// call here specifically: an install either reboots into the new
    /// firmware within moments (background warm-up rebuilds these from
    /// Recent on the next boot exactly as it does on any boot) or fails, in
    /// which case `tick_background_warmup` simply rebuilds them again over
    /// the next few idle loop iterations. The active `session`, if any, is
    /// left untouched -- only the parked/background copies are freed.
    pub fn release_parked_sessions_for_install(&mut self) {
        self.session_cache.clear();
    }

    /// Instantly swap a warm cached session for `book` into the foreground
    /// `session` slot, parking whatever was active before back into the
    /// cache. No SD access either way. Returns `false` (no-op) on a cache
    /// miss.
    fn promote_cached_session(&mut self, book: &ReaderBook) -> bool {
        let Some(index) = self.session_cache.iter().position(|s| s.book == *book) else {
            return false;
        };
        let promoted = self.session_cache.remove(index);
        if let Some(previous) = self.session.replace(promoted) {
            self.cache_session(previous);
        }
        true
    }

    /// Queue up to `READER_SESSION_CACHE_LIMIT` of Recent's other books for
    /// silent background warm-up into `session_cache`, skipping `active`
    /// (the book this boot is already resuming as the foreground session,
    /// if any) so it isn't warmed twice. Call once, right after
    /// `load_persistent_state`.
    pub fn seed_background_warmup(&mut self, active: Option<&str>) {
        self.warmup_queue = self
            .recent
            .iter()
            .filter(|location| Some(location.path.as_str()) != active)
            .take(READER_SESSION_CACHE_LIMIT)
            .cloned()
            .collect();
    }

    /// Advance the background warm-up queue by one book, best effort.
    /// Reads only already-warm SD caches (the EPUB `.EPX`/`.EPP` pair, or
    /// the TXT anchor cache) — a cache miss is skipped rather than falling
    /// back to a full parse, so this never spends the cost of a cold open
    /// on a book the user hasn't actually asked for. Returns `true` while
    /// the queue still had an entry to process (whether or not it ended up
    /// cached), so the caller can keep calling this once per idle loop
    /// iteration until it returns `false`.
    pub fn tick_background_warmup(&mut self) -> bool {
        if self.loading.is_some() || self.warmup_queue.is_empty() {
            return false;
        }
        let location = self.warmup_queue.remove(0);
        let already_warm = self
            .session
            .as_ref()
            .is_some_and(|session| session.book.path == location.path)
            || self
                .session_cache
                .iter()
                .any(|session| session.book.path == location.path);
        if already_warm {
            return true;
        }
        let book = location.as_book();
        let mut span = crate::boot_profile::span("reader-background-warmup");
        if crate::boot_profile::is_active() {
            span.detail(format_args!("{:?} {}", location.format, book.path));
        }
        let session = match location.format {
            BookFormat::Text => detect_txt_encoding(&book.path)
                .ok()
                .and_then(|encoding| self.open_txt_session(&book, encoding, Some(&location)).ok()),
            BookFormat::Epub => {
                // Internal-RAM fragmentation investigation: brackets the two
                // sub-steps of a cache-hit reopen separately, so the delta
                // between consecutive lines attributes fragmentation to
                // either the document cache (title/TOC/chapter parsing,
                // including `read_epub_cache_header`'s header buffer) or to
                // `open_epub_session` (page-index cache load plus session
                // construction).
                crate::runtime_memory::debug_runtime_memory("before-epub-document-cache-load");
                let result =
                    self.load_epub_document_cache_best_effort(&book)
                        .and_then(|document| {
                            crate::runtime_memory::debug_runtime_memory(
                                "after-epub-document-cache-load",
                            );
                            let session = self
                                .open_epub_session(&book, document, Some(&location), false, true)
                                .ok();
                            crate::runtime_memory::debug_runtime_memory("after-open-epub-session");
                            session
                        });
                result
            }
        };
        match session {
            Some(session) => {
                self.cache_session(session);
                log::info!(
                    "rustmix-wave=reader-session-warmup status=cached path={}",
                    location.path
                );
            }
            None => {
                log::info!(
                    "rustmix-wave=reader-session-warmup status=skipped-cold path={}",
                    location.path
                );
            }
        }
        true
    }

    /// Recent-opens section: most-recently-opened book first.
    #[must_use]
    pub fn recent_entries(&self) -> Vec<ReaderLibraryEntry> {
        self.recent
            .iter()
            .cloned()
            .map(|location| ReaderLibraryEntry {
                book: location.as_book(),
                location: Some(location),
            })
            .collect()
    }

    /// Every library book not already shown in the Recent section.
    #[must_use]
    pub fn other_library_entries(&self) -> Vec<ReaderLibraryEntry> {
        self.books
            .iter()
            .cloned()
            .filter(|book| {
                !self
                    .recent
                    .iter()
                    .any(|location| location.matches_book(book))
            })
            .map(|book| ReaderLibraryEntry {
                location: self.saved_position_for_book(&book),
                book,
            })
            .collect()
    }

    /// The Library screen's two sections: Recent, then the rest of the library.
    #[must_use]
    pub fn library_sections(&self) -> (Vec<ReaderLibraryEntry>, Vec<ReaderLibraryEntry>) {
        (self.recent_entries(), self.other_library_entries())
    }

    /// Books in the order the Library screen's two-section grid draws them
    /// (see `screens::reader::render_library`): Recent entries with *some*
    /// real progress (1-99%) first, then never-opened books and Recent
    /// entries still at 0% together, then finished reads last — so
    /// `library_selected`, which indexes into this list, always lands on the
    /// cell actually drawn at that index. A Recent entry at 0% (opened once
    /// but never read past the first page) sorts with the never-opened
    /// books rather than the in-progress ones, matching how the grid labels
    /// it ("New", not "Reading Now" — a saved position alone isn't progress
    /// until it has a percentage to show for it).
    ///
    /// Books outside Recent are classified by their saved position too
    /// (`other_library_entries` already attaches it from `positions`, which
    /// keeps far more books than Recent's 16): otherwise opening enough other
    /// books to push a half-read or finished one out of Recent relabeled it
    /// "New" even though its position was still saved.
    #[must_use]
    pub fn visible_entries(&self) -> Vec<ReaderLibraryEntry> {
        let (recent, other) = self.library_sections();
        let mut in_progress = Vec::new();
        let mut new = Vec::new();
        let mut completed = Vec::new();
        for entry in recent.into_iter().chain(other) {
            let percent = entry
                .location
                .as_ref()
                .map_or(0, ReaderLocation::reading_percent_estimate);
            if percent >= 100 {
                completed.push(entry);
            } else if percent == 0 {
                new.push(entry);
            } else {
                in_progress.push(entry);
            }
        }
        in_progress
            .into_iter()
            .chain(new)
            .chain(completed)
            .collect()
    }

    #[must_use]
    pub fn library_row_count(&self) -> usize {
        self.visible_entries().len()
    }

    pub fn apply_library_button(&mut self, event: ButtonEvent) -> bool {
        let count = self.library_row_count().max(1);
        match event {
            ButtonEvent::Up => {
                self.library_selected = self.library_selected.checked_sub(1).unwrap_or(count - 1);
                false
            }
            ButtonEvent::Down => {
                self.library_selected = (self.library_selected + 1) % count;
                false
            }
            ButtonEvent::Select => self.request_open_visible(self.library_selected),
        }
    }

    pub fn apply_bookmarks_button(&mut self, event: ButtonEvent) -> bool {
        if self.bookmarks.is_empty() {
            return false;
        }
        match event {
            ButtonEvent::Up => {
                self.bookmarks_selected = self
                    .bookmarks_selected
                    .checked_sub(1)
                    .unwrap_or(self.bookmarks.len() - 1);
                false
            }
            ButtonEvent::Down => {
                self.bookmarks_selected = (self.bookmarks_selected + 1) % self.bookmarks.len();
                false
            }
            ButtonEvent::Select => self.request_open_bookmark(self.bookmarks_selected),
        }
    }

    /// Opens the Library long-press "book actions" overlay for `book`,
    /// resetting its selection to the first row.
    pub fn open_book_actions(&mut self, book: ReaderBook) {
        self.book_actions_target = Some(book);
        self.book_actions_selected = 0;
    }

    pub fn cycle_book_action_previous(&mut self) {
        self.book_actions_selected = self
            .book_actions_selected
            .checked_sub(1)
            .unwrap_or(LibraryBookAction::ALL.len() - 1);
    }

    pub fn cycle_book_action_next(&mut self) {
        self.book_actions_selected =
            (self.book_actions_selected + 1) % LibraryBookAction::ALL.len();
    }

    #[must_use]
    pub fn selected_book_action(&self) -> LibraryBookAction {
        LibraryBookAction::ALL[self.book_actions_selected]
    }

    /// Bookmarks belonging to [`Self::book_actions_target`], in the same
    /// relative order as `self.bookmarks`. Used by both the book-actions
    /// overlay's bookmarks sub-screen and [`Self::request_open_book_bookmark`].
    #[must_use]
    pub fn book_actions_bookmarks(&self) -> Vec<ReaderLocation> {
        let Some(book) = self.book_actions_target.as_ref() else {
            return Vec::new();
        };
        self.bookmarks
            .iter()
            .filter(|location| location.path == book.path)
            .cloned()
            .collect()
    }

    pub fn apply_book_bookmarks_button(&mut self, event: ButtonEvent) -> bool {
        let bookmarks = self.book_actions_bookmarks();
        if bookmarks.is_empty() {
            return false;
        }
        match event {
            ButtonEvent::Up => {
                self.book_bookmarks_selected = self
                    .book_bookmarks_selected
                    .checked_sub(1)
                    .unwrap_or(bookmarks.len() - 1);
                false
            }
            ButtonEvent::Down => {
                self.book_bookmarks_selected = (self.book_bookmarks_selected + 1) % bookmarks.len();
                false
            }
            ButtonEvent::Select => self.request_open_book_bookmark(self.book_bookmarks_selected),
        }
    }

    /// Mirrors [`Self::request_open_bookmark`], but resolves the index
    /// against [`Self::book_actions_bookmarks`] (a single book's bookmarks)
    /// instead of the full `self.bookmarks` list.
    pub fn request_open_book_bookmark(&mut self, bookmark_index: usize) -> bool {
        let Some(location) = self.book_actions_bookmarks().get(bookmark_index).cloned() else {
            return false;
        };
        self.request_open_book(location.as_book(), Some(location));
        true
    }

    /// Marks [`Self::book_actions_target`] as finished: creates or updates
    /// its saved position with `reading_percent = 100%` and files it under
    /// Recent, so the Library grid's Recent section shows it as Completed
    /// (see `screens::reader::library_grid_entries`) the same way finishing
    /// a book normally would — persisted to disk immediately, not just held
    /// in memory, so it survives a reboot.
    pub fn mark_book_actions_target_completed(&mut self) -> bool {
        let Some(book) = self.book_actions_target.clone() else {
            return false;
        };
        let mut location = self
            .saved_position_for_book(&book)
            .unwrap_or_else(|| ReaderLocation {
                path: book.path.clone(),
                title: book.title.clone(),
                format: book.format,
                size_bytes: book.size_bytes,
                modified_seconds: book.modified_seconds,
                page_index: 0,
                byte_offset: 0,
                epub_chapter: None,
                reading_percent: None,
            });
        location.byte_offset = book.size_bytes;
        location.reading_percent = Some(100);

        self.positions.retain(|entry| entry.path != location.path);
        self.positions.insert(0, location.clone());
        self.positions.truncate(READER_POSITION_LIMIT);
        self.recent.retain(|entry| entry.path != location.path);
        self.recent.insert(0, location);
        self.recent.truncate(READER_RECENT_LIMIT);

        let mut errors = Vec::new();
        if let Err(error) = atomic_replace_text(
            &self.positions_path(),
            &serialize_location_list(&self.positions),
        ) {
            errors.push(format!("POSITS.TXT: {error}"));
        }
        if let Err(error) =
            atomic_replace_text(&self.recent_path(), &serialize_location_list(&self.recent))
        {
            errors.push(format!("RECENT.TXT: {error}"));
        }
        self.finish_persistence("book-actions-mark-completed", errors);
        true
    }

    pub fn request_open_visible(&mut self, visible_index: usize) -> bool {
        let Some(entry) = self.visible_entries().get(visible_index).cloned() else {
            return false;
        };
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.book == entry.book)
        {
            // Already the open book: reuse the live session instead of
            // tearing it down and reparsing from disk, mirroring the
            // Continue Reading shortcut (`AppState::activate_continue_reading`).
            // `self.loading` stays `None`, so the caller routes straight to
            // `ReaderPage` instead of `ReaderLoading`.
            return true;
        }
        if self.promote_cached_session(&entry.book) {
            // A different but recently-open book, warm in `session_cache`
            // (either from this boot's background warm-up or from switching
            // away from it earlier): same zero-I/O swap as above.
            return true;
        }
        let resume = entry
            .location
            .or_else(|| self.saved_position_for_book(&entry.book))
            .or_else(|| {
                self.resume
                    .clone()
                    .filter(|location| location.matches_book(&entry.book))
            });
        self.request_open_book(entry.book, resume);
        true
    }

    #[must_use]
    fn saved_position_for_book(&self, book: &ReaderBook) -> Option<ReaderLocation> {
        self.positions
            .iter()
            .find(|location| location.matches_book(book))
            .cloned()
    }

    /// Reading-completion percentage for the Library grid's cover badge.
    /// Cheap enough to call for every cover on the current page (a linear
    /// scan of the small `positions` list, no SD access, no session).
    ///
    /// Prefers [`ReaderLocation::reading_percent`], the exact figure
    /// [`ReaderSession::reading_percent`] computed and stashed the last time
    /// this book's position was saved — so the badge matches what the
    /// reader itself showed, without re-deriving it here. Only a location
    /// saved before that field existed falls back to the cruder byte-offset
    /// estimate. `None` for a book with no saved position (never opened).
    #[must_use]
    pub fn library_progress_percent(&self, book: &ReaderBook) -> Option<u8> {
        let location = self.saved_position_for_book(book)?;
        if let Some(percent) = location.reading_percent {
            return Some(percent);
        }
        if book.size_bytes == 0 {
            return Some(100);
        }
        Some((location.byte_offset.saturating_mul(100) / book.size_bytes).min(100) as u8)
    }

    /// The book the Home dashboard's Continue Reading tile should show: the
    /// actively open session's book if one exists, else the last saved
    /// resume position. `None` when neither exists.
    #[must_use]
    pub fn continue_reading_book(&self) -> Option<ReaderBook> {
        if let Some(session) = self.session.as_ref() {
            return Some(session.book.clone());
        }
        self.resume.as_ref().map(ReaderLocation::as_book)
    }

    /// Reading-completion percentage for [`Self::continue_reading_book`].
    /// Mirrors [`Self::library_progress_percent`]'s preference for the
    /// live/stashed exact figure over a byte-offset estimate, but reads
    /// straight from `session`/`resume` instead of scanning `positions`,
    /// since the Continue Reading tile already has the book it needs at
    /// hand.
    #[must_use]
    pub fn continue_reading_percent(&self) -> Option<u8> {
        if let Some(session) = self.session.as_ref() {
            return session.reading_percent();
        }
        let resume = self.resume.as_ref()?;
        if let Some(percent) = resume.reading_percent {
            return Some(percent);
        }
        if resume.size_bytes == 0 {
            return Some(100);
        }
        Some((resume.byte_offset.saturating_mul(100) / resume.size_bytes).min(100) as u8)
    }

    /// Reading-time-remaining inputs for [`Self::continue_reading_book`],
    /// consumed by the Continue Reading tile and the Reading Stats screen.
    /// Prefers the open session's exact chapter/book end offsets; falls back
    /// to an estimate from the last-saved percentage
    /// (`byte_offset * 100 / percent`) when no book is currently open, which
    /// is close enough for a "time remaining" figure and avoids reopening
    /// the book just to answer that question. `None` when there is no
    /// continue-reading book, or its saved percentage is `0` (nothing to
    /// divide by).
    #[must_use]
    pub fn continue_reading_progress(&self) -> Option<CurrentBookProgress> {
        if let Some(session) = self.session.as_ref() {
            let location = session.current_location();
            return Some(CurrentBookProgress {
                current_position: location.byte_offset,
                chapter_end_position: session.current_chapter_end_offset(),
                book_end_position: Some(session.source_size_bytes()),
            });
        }
        let resume = self.resume.as_ref()?;
        let percent = resume.reading_percent?;
        if percent == 0 {
            return None;
        }
        let estimated_book_end = resume.byte_offset.saturating_mul(100) / u64::from(percent);
        Some(CurrentBookProgress {
            current_position: resume.byte_offset,
            chapter_end_position: None,
            book_end_position: Some(estimated_book_end),
        })
    }

    pub fn request_open_bookmark(&mut self, bookmark_index: usize) -> bool {
        let Some(location) = self.bookmarks.get(bookmark_index).cloned() else {
            return false;
        };
        self.request_open_book(location.as_book(), Some(location));
        true
    }

    fn request_open_book(&mut self, book: ReaderBook, resume: Option<ReaderLocation>) {
        self.release_active_session_for_open();
        self.loading = Some(PendingReaderOpen {
            book,
            stage: ReaderLoadingStage::OpeningFile,
            encoding: None,
            epub_document: None,
            resume,
            message: "Preparing reader...".into(),
            epub_document_cache_pending: false,
        });
    }

    /// Persist the previous session, then either park it in `session_cache`
    /// for a possible instant resume later, or drop it outright, before the
    /// next book is parsed.
    ///
    /// A session whose EPUB text is still `Resident` (just parsed, `.EPX`
    /// not written yet — see `epub_document_cache_pending`) is never parked:
    /// it can hold up to `EPUB_REFLOW_TEXT_LIMIT` bytes of RAM, and keeping
    /// it alive while allocating the next parser-worker stack can exhaust
    /// the embedded heap after repeated book switches. Every other session
    /// is already SD-backed (`EpubTextStore::OnDisk`) and cheap to park —
    /// see `READER_SESSION_CACHE_LIMIT`.
    fn release_active_session_for_open(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.persist_current_session_best_effort();
        let Some(session) = self.session.take() else {
            return;
        };
        if session.epub_document_cache_pending {
            log::info!(
                "rustmix-wave=reader-session-memory-release status=completed reason=book-open policy=dropped-resident"
            );
        } else {
            self.cache_session(session);
            log::info!(
                "rustmix-wave=reader-session-memory-release status=completed reason=book-open policy=parked"
            );
        }
    }

    fn request_layout_rebuild(&mut self) -> bool {
        if self.session.is_none() {
            self.persist_preferences_best_effort();
            return false;
        }
        self.persist_current_session_best_effort();
        let Some(mut session) = self.session.take() else {
            return false;
        };
        let book = session.book.clone();
        let encoding = session.encoding;
        let resume = session.current_location();
        self.loading = Some(PendingReaderOpen {
            book,
            stage: ReaderLoadingStage::UpdatingLayout,
            encoding: Some(encoding),
            epub_document: session.epub_document.take(),
            resume: Some(resume),
            message: "Rebuilding the current page first...".into(),
            epub_document_cache_pending: false,
        });
        log::info!(
            "rustmix-wave=reader-session-memory-release status=completed reason=layout-rebuild"
        );
        self.persist_preferences_best_effort();
        true
    }

    pub fn cancel_loading(&mut self) {
        self.loading = None;
        self.last_message = Some("Book opening cancelled".into());
    }

    #[must_use]
    pub fn loading_stage(&self) -> Option<ReaderLoadingStage> {
        self.loading.as_ref().map(|loading| loading.stage)
    }

    /// Whether the pending open is an EPUB whose flattened-document cache
    /// (`.EPX`) is already on the card. Such an open skips the ZIP/DEFLATE
    /// parse entirely and finishes in about 100 ms, so it can complete
    /// before anything is drawn instead of showing a loading screen first.
    /// Anything else (TXT, or an EPUB never opened before) may take seconds.
    #[must_use]
    pub fn pending_open_has_warm_cache(&self) -> bool {
        self.loading.as_ref().is_some_and(|loading| {
            loading.book.format == BookFormat::Epub
                && self.epub_document_cache_path_for(&loading.book).exists()
        })
    }

    pub fn tick(&mut self) -> ReaderTickOutcome {
        if let Some(mut loading) = self.loading.take() {
            let outcome = loop {
                let stage_before = loading.stage;
                let stage_started_at = Instant::now();
                let mut stage_span = crate::boot_profile::span("reader-open-stage");
                if crate::boot_profile::is_active() {
                    stage_span.detail(format_args!("{stage_before:?}"));
                }
                let outcome = match loading.stage {
                    ReaderLoadingStage::OpeningFile => {
                        loading.stage = match loading.book.format {
                            BookFormat::Text => ReaderLoadingStage::DetectingEncoding,
                            BookFormat::Epub => ReaderLoadingStage::InspectingEpubArchive,
                        };
                        loading.message = loading.stage.label().into();
                        ReaderTickOutcome::LoadingStageChanged
                    }
                    ReaderLoadingStage::InspectingEpubArchive => {
                        if let Some(document) =
                            self.load_epub_document_cache_best_effort(&loading.book)
                        {
                            loading.message = format!(
                                "{} spine items / {} TOC entries (cached)",
                                document.spine_count,
                                document.toc.len()
                            );
                            loading.epub_document = Some(document);
                            loading.stage = ReaderLoadingStage::ReadingEpubPackage;
                            ReaderTickOutcome::LoadingStageChanged
                        } else {
                            match open_epub_on_worker(&loading.book.path) {
                                Ok(document) => {
                                    loading.message = format!(
                                        "{} spine items / {} TOC entries",
                                        document.spine_count,
                                        document.toc.len()
                                    );
                                    // Don't write `.EPX` here: on this
                                    // hardware's SD/FAT stack a single write
                                    // this size (the whole flattened book
                                    // text) can itself take seconds — doing
                                    // it before the first page is even shown
                                    // would add that on top of the parse
                                    // time the user is already waiting
                                    // through. Deferred to the first
                                    // background tick after the page is on
                                    // screen (see `epub_document_cache_pending`
                                    // in `tick()`).
                                    loading.epub_document_cache_pending = true;
                                    loading.epub_document = Some(document);
                                    loading.stage = ReaderLoadingStage::ReadingEpubPackage;
                                    ReaderTickOutcome::LoadingStageChanged
                                }
                                Err(error) => {
                                    loading.stage = ReaderLoadingStage::Failed;
                                    loading.message = friendly_worker_start_error(&error);
                                    ReaderTickOutcome::Failed
                                }
                            }
                        }
                    }
                    ReaderLoadingStage::ReadingEpubPackage => {
                        loading.stage = ReaderLoadingStage::LoadingEpubSpine;
                        loading.message = "EPUB package and navigation ready".into();
                        ReaderTickOutcome::LoadingStageChanged
                    }
                    ReaderLoadingStage::LoadingEpubSpine => {
                        loading.stage = if loading.resume.is_some() {
                            ReaderLoadingStage::LoadingSavedPosition
                        } else {
                            ReaderLoadingStage::BuildingFirstPage
                        };
                        loading.message = "Reflowable EPUB text ready".into();
                        ReaderTickOutcome::LoadingStageChanged
                    }
                    ReaderLoadingStage::DetectingEncoding => {
                        match detect_txt_encoding(&loading.book.path) {
                            Ok(encoding) => {
                                loading.encoding = Some(encoding);
                                loading.stage = if loading.resume.is_some() {
                                    ReaderLoadingStage::LoadingSavedPosition
                                } else {
                                    ReaderLoadingStage::BuildingFirstPage
                                };
                                loading.message = format!("{} detected", encoding.label());
                                ReaderTickOutcome::LoadingStageChanged
                            }
                            Err(error) => {
                                loading.stage = ReaderLoadingStage::Failed;
                                loading.message = error;
                                ReaderTickOutcome::Failed
                            }
                        }
                    }
                    ReaderLoadingStage::LoadingSavedPosition => {
                        loading.stage = ReaderLoadingStage::BuildingFirstPage;
                        loading.message = "Resume anchor ready".into();
                        ReaderTickOutcome::LoadingStageChanged
                    }
                    ReaderLoadingStage::UpdatingLayout => {
                        loading.stage = ReaderLoadingStage::BuildingFirstPage;
                        loading.message = "Layout cache update ready".into();
                        ReaderTickOutcome::LoadingStageChanged
                    }
                    ReaderLoadingStage::BuildingFirstPage => {
                        let encoding = loading.encoding.unwrap_or(TextEncoding::Utf8);
                        let session = match loading.book.format {
                            BookFormat::Text => self.open_txt_session(
                                &loading.book,
                                encoding,
                                loading.resume.as_ref(),
                            ),
                            BookFormat::Epub => loading
                                .epub_document
                                .take()
                                .ok_or_else(|| "EPUB document is not staged".to_string())
                                .and_then(|document| {
                                    self.open_epub_session(
                                        &loading.book,
                                        document,
                                        loading.resume.as_ref(),
                                        loading.epub_document_cache_pending,
                                        false,
                                    )
                                }),
                        };
                        match session {
                            Ok(session) => {
                                let location = session.current_location();
                                self.session = Some(session);
                                self.last_message =
                                    Some("Saved position ready; caching continues lazily".into());
                                // Reopening exactly where the saved state
                                // already points (every resume, including the
                                // one on a deep-sleep wake) would rewrite
                                // STATE/POSITS/RECENT byte for byte: ~150 ms
                                // of fsync'd atomic replaces, plus ~170 ms
                                // more when it is the first FAT allocation
                                // since mount, all before the first frame.
                                if self.persisted_state_matches(&location) {
                                    self.persist_anchor_cache_best_effort();
                                } else {
                                    self.persist_current_session_best_effort();
                                }
                                ReaderTickOutcome::FirstPageReady
                            }
                            Err(error) => {
                                loading.stage = ReaderLoadingStage::Failed;
                                loading.message = error;
                                ReaderTickOutcome::Failed
                            }
                        }
                    }
                    ReaderLoadingStage::UnsupportedEpub | ReaderLoadingStage::Failed => {
                        self.loading = Some(loading);
                        return ReaderTickOutcome::None;
                    }
                    ReaderLoadingStage::IndexingNearbyPages | ReaderLoadingStage::Ready => {
                        ReaderTickOutcome::None
                    }
                };
                // Chain straight through free bookkeeping stages within this
                // same call instead of returning to the caller after each one:
                // on a fully warm reopen, `OpeningFile` through `BuildingFirstPage`
                // used to cost one `tick()` (250ms floor, plus a full e-paper
                // redraw) *per stage*, most of which do no real work. Only the
                // stages that put an informative message on screen right before
                // work that can actually be slow (`InspectingEpubArchive` on an
                // EPUB `.EPX` cache miss, `DetectingEncoding`'s file sniff) — and
                // `OpeningFile`, which kicks that first message off — still stop
                // here for their own tick and redraw.
                let stops_here = matches!(
                    stage_before,
                    ReaderLoadingStage::OpeningFile
                        | ReaderLoadingStage::InspectingEpubArchive
                        | ReaderLoadingStage::DetectingEncoding
                );
                log::info!(
                    "rustmix-wave=reader-stage-timing stage={:?} elapsed-ms={}",
                    stage_before,
                    stage_started_at.elapsed().as_millis()
                );
                if !matches!(outcome, ReaderTickOutcome::LoadingStageChanged) || stops_here {
                    break outcome;
                }
            };
            if !matches!(outcome, ReaderTickOutcome::FirstPageReady) {
                self.loading = Some(loading);
            }
            return outcome;
        }

        // Runs on the first background tick after the session exists, i.e.
        // strictly after `FirstPageReady` (and its redraw) already happened
        // — see `epub_document_cache_pending` on `ReaderSession` for why this
        // multi-second SD write must not block the page the user is waiting
        // to see. Independent of the pagination gate below: a short book can
        // reach `index_complete` immediately and never enter that branch
        // again, but the `.EPX` write still needs to happen exactly once.
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.epub_document_cache_pending)
        {
            if let Some(mut session) = self.session.take() {
                if let Some(document) = session.epub_document.take() {
                    session.epub_document = Some(
                        match self.persist_epub_document_cache_best_effort(&session.book, &document)
                        {
                            Some((path, body_offset)) => document.into_on_disk(path, body_offset),
                            None => document,
                        },
                    );
                }
                session.epub_document_cache_pending = false;
                self.session = Some(session);
            }
        }

        let (outcome, checkpoint, epub_cache_ready) = if let Some(session) = self.session.as_mut() {
            if session.cache.len() < READER_NEARBY_PAGE_CACHE && !session.index_complete {
                let advanced = match session.index_one_page() {
                    Ok(value) => value,
                    Err(error) => {
                        self.last_message = Some(error);
                        return ReaderTickOutcome::Failed;
                    }
                };
                let outcome = if advanced {
                    ReaderTickOutcome::BackgroundCacheAdvanced
                } else {
                    ReaderTickOutcome::None
                };
                let checkpoint = if advanced {
                    is_index_checkpoint(session.page_offsets.len()) || session.index_complete
                } else {
                    session.index_complete
                };
                // Persisted on the same checkpoint cadence as the TXT anchor
                // cache below, not gated on reaching the book's true end:
                // most reading sessions resume mid-book and only ever index
                // forward from there, so waiting for `index_complete` (which
                // requires walking all the way to the literal last page)
                // would mean the common "reopen where I left off" case never
                // gets a cache hit at all. Each persisted chapter carries its
                // own `text_offset`/`text_end_offset`, so a partial,
                // mid-book run is self-describing on disk; `open_epub_session`
                // only accepts it as a hit if it actually covers the
                // requested resume offset, and otherwise falls back to a
                // fresh single-chapter pagination exactly as if no cache
                // file existed.
                let epub_cache_ready = (session.book.format == BookFormat::Epub
                    && checkpoint
                    && !session.epub_chapter_pages.is_empty())
                .then(|| {
                    (
                        session.book.clone(),
                        session.layout,
                        session.epub_chapter_pages.clone(),
                    )
                });
                (outcome, checkpoint, epub_cache_ready)
            } else {
                (ReaderTickOutcome::None, false, None)
            }
        } else {
            (ReaderTickOutcome::None, false, None)
        };
        if checkpoint {
            self.persist_anchor_cache_best_effort();
        }
        if let Some((book, layout, pages)) = epub_cache_ready {
            self.persist_epub_chapter_pages_cache_best_effort(&book, layout, &pages);
        }
        self.flush_pending_persist_if_idle();
        self.prefetch_next_page_best_effort();
        self.prewarm_inline_images_best_effort();
        outcome
    }

    /// Decode and SD-cache at most one inline EPUB image per tick, for the
    /// pages around the current one (previous, current, next). Without this
    /// the first draw of a page with an image paid the whole ZIP extract +
    /// JPEG/PNG decode + dither synchronously inside `render_page`, right
    /// after the page-turn button press; doing it here while the reader is
    /// still on the page before means the turn itself is just an SD-cache
    /// read. `render_page` keeps its synchronous fallback for pages this
    /// never reached (a TOC jump, a fast run of page turns).
    fn prewarm_inline_images_best_effort(&mut self) {
        if self.loading.is_some() {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if session.book.format != BookFormat::Epub {
            return;
        }
        let current = session.page_number_base.saturating_add(session.current_page);
        let book_path = session.book.path.clone();
        let pending = session
            .cache
            .iter()
            .filter(|page| page.page_index + 1 >= current && page.page_index <= current + 1)
            .flat_map(|page| page.lines.iter())
            .filter_map(|line| line.image.as_ref())
            .find(|image| {
                !self.prewarmed_images.iter().any(|(path, href, width, height)| {
                    *path == book_path
                        && *href == image.href
                        && *width == image.box_width
                        && *height == image.box_height
                })
            })
            .cloned();
        let Some(image) = pending else {
            return;
        };
        let cache = crate::cover_cache::EpubImageCache::new(self.cache_directory());
        if cache.is_missing(&session.book, &image.href, image.box_width, image.box_height) {
            let _ = cache.generate_bitmap(
                &session.book,
                &image.href,
                image.box_width,
                image.box_height,
            );
        }
        if self.prewarmed_images.len() >= READER_PREWARMED_IMAGE_LIMIT {
            self.prewarmed_images.remove(0);
        }
        self.prewarmed_images
            .push((book_path, image.href, image.box_width, image.box_height));
    }

    /// Render-and-cache the page right after the current one, if its offset
    /// is already indexed and it isn't cached yet, so a later `next_page()`
    /// finds it as a cache hit instead of paying the word-wrap cost
    /// synchronously on the button press. Deliberately independent of the
    /// whole-book indexing gate above (`cache.len() < READER_NEARBY_PAGE_CACHE`):
    /// that gate exists so background EPUB indexing keeps completing
    /// `epub_chapter_pages` regardless of how much of the small nearby-page
    /// cache is already full, and coupling this lookahead to it would mean a
    /// full cache (the common case once a session has read a few pages)
    /// blocks prefetch just as much as it (correctly) blocks re-indexing.
    /// `push_cached_page`'s own distance-from-current eviction still applies,
    /// so this can never grow the cache past its normal bound.
    fn prefetch_next_page_best_effort(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let lookahead = session.current_page.saturating_add(1);
        if lookahead >= session.page_offsets.len() {
            // Not indexed yet -- the indexing step above will get there.
            return;
        }
        let absolute = session.page_number_base.saturating_add(lookahead);
        if session.cache.iter().any(|page| page.page_index == absolute) {
            return;
        }
        let _ = session.ensure_page_cached(lookahead);
    }

    pub fn previous_page(&mut self) {
        if let Some(session) = self.session.as_mut() {
            if let Err(error) = session.previous_page() {
                self.last_message = Some(error);
                return;
            }
            self.mark_pending_persist();
        }
    }

    pub fn next_page(&mut self) {
        if let Some(session) = self.session.as_mut() {
            if let Err(error) = session.next_page() {
                self.last_message = Some(error);
                return;
            }
            self.mark_pending_persist();
        }
    }

    /// Marks a page turn's save as pending and (re)starts the debounce
    /// timer, so a run of consecutive page turns keeps deferring the save
    /// instead of writing on the first one and again on every one after.
    fn mark_pending_persist(&mut self) {
        self.pending_persist = true;
        self.pending_persist_since = Some(Instant::now());
    }

    /// Runs the STATE/POSITS/RECENT save a page turn deferred via
    /// `pending_persist`, regardless of the debounce timer. No-op when
    /// nothing is pending, so it's cheap to call unconditionally. Used for
    /// the moments a debounced save must not be left waiting: leaving the
    /// Reader route and entering deep sleep (see `pending_persist`'s doc
    /// comment) -- ordinary page turns instead go through
    /// `flush_pending_persist_if_idle`, called from `tick`.
    pub fn flush_pending_persist(&mut self) {
        if core::mem::take(&mut self.pending_persist) {
            self.pending_persist_since = None;
            self.persist_current_session_best_effort();
        }
    }

    /// Runs the deferred page-turn save only once the reader has been
    /// sitting on the current page for `READER_PERSIST_DEBOUNCE` -- called
    /// every `tick()` while a reader route is active (roughly every 250ms,
    /// see main.rs), so this is how an idle reader's page turn actually
    /// reaches SD.
    fn flush_pending_persist_if_idle(&mut self) {
        let idle_long_enough = self
            .pending_persist_since
            .is_some_and(|since| since.elapsed() >= READER_PERSIST_DEBOUNCE);
        if idle_long_enough {
            self.flush_pending_persist();
        }
    }

    /// Lines available for dictionary-mode selection on the current page,
    /// bounded exactly like `render_page`'s draw loop (`lines_per_page` can
    /// cut a cached page short before its Vec ends).
    fn dictionary_page_lines(&self) -> &[ReaderPageLine] {
        let Some(session) = self.session.as_ref() else {
            return &[];
        };
        let Some(page) = session.current_cached_page() else {
            return &[];
        };
        let limit = session.layout.lines_per_page.min(page.lines.len());
        &page.lines[..limit]
    }

    fn line_has_eligible_word(&self, line_index: usize) -> bool {
        self.dictionary_page_lines()
            .get(line_index)
            .is_some_and(|line| !eligible_word_spans(&line.text).is_empty())
    }

    fn first_eligible_line(&self) -> Option<usize> {
        (0..self.dictionary_page_lines().len()).find(|&index| self.line_has_eligible_word(index))
    }

    /// Enter dictionary line-select mode at the page's first eligible line,
    /// or exit immediately from any dictionary-mode phase back to normal
    /// reading. Returns `false` when there is nothing to enter (no line on
    /// the current page has a selectable word) so the caller can treat the
    /// long-press as not consumed.
    pub fn toggle_dictionary_mode(&mut self) -> bool {
        if matches!(self.dictionary_mode, ReaderDictionaryMode::Off) {
            match self.first_eligible_line() {
                Some(line_index) => {
                    self.dictionary_mode = ReaderDictionaryMode::LineSelect { line_index };
                    true
                }
                None => false,
            }
        } else {
            self.dictionary_mode = ReaderDictionaryMode::Off;
            true
        }
    }

    /// Step back one dictionary-mode level (Definition -> WordSelect ->
    /// LineSelect -> Off). Returns `false` when already `Off` so the caller
    /// can fall through to ordinary Back navigation.
    pub fn dictionary_step_back(&mut self) -> bool {
        self.dictionary_mode = match std::mem::take(&mut self.dictionary_mode) {
            ReaderDictionaryMode::Off => return false,
            ReaderDictionaryMode::LineSelect { .. } => ReaderDictionaryMode::Off,
            ReaderDictionaryMode::WordSelect { line_index, .. } => {
                ReaderDictionaryMode::LineSelect { line_index }
            }
            ReaderDictionaryMode::Definition {
                line_index,
                word_index,
                ..
            } => ReaderDictionaryMode::WordSelect {
                line_index,
                word_index,
            },
        };
        true
    }

    /// Moves the line cursor to the next eligible line in `direction` (-1 up,
    /// +1 down). Clamps at the first/last eligible line on the page rather
    /// than turning pages or wrapping around.
    pub fn dictionary_move_line(&mut self, direction: i32) {
        let ReaderDictionaryMode::LineSelect { line_index } = &self.dictionary_mode else {
            return;
        };
        let line_index = *line_index;
        let total = self.dictionary_page_lines().len();
        let mut cursor = line_index as i32;
        loop {
            cursor += direction;
            if cursor < 0 || cursor as usize >= total {
                return;
            }
            if self.line_has_eligible_word(cursor as usize) {
                self.dictionary_mode = ReaderDictionaryMode::LineSelect {
                    line_index: cursor as usize,
                };
                return;
            }
        }
    }

    /// Confirms the current line-select cursor, entering word-select mode on
    /// its first eligible word.
    pub fn dictionary_confirm_line(&mut self) {
        let ReaderDictionaryMode::LineSelect { line_index } = &self.dictionary_mode else {
            return;
        };
        let line_index = *line_index;
        if self.line_has_eligible_word(line_index) {
            self.dictionary_mode = ReaderDictionaryMode::WordSelect {
                line_index,
                word_index: 0,
            };
        }
    }

    /// Moves the word cursor within the confirmed line's eligible words,
    /// clamped at the first/last word.
    pub fn dictionary_move_word(&mut self, direction: i32) {
        let ReaderDictionaryMode::WordSelect {
            line_index,
            word_index,
        } = &self.dictionary_mode
        else {
            return;
        };
        let (line_index, word_index) = (*line_index, *word_index);
        let count = self
            .dictionary_page_lines()
            .get(line_index)
            .map_or(0, |line| eligible_word_spans(&line.text).len());
        if count == 0 {
            return;
        }
        let next = (word_index as i32 + direction).clamp(0, count as i32 - 1) as usize;
        self.dictionary_mode = ReaderDictionaryMode::WordSelect {
            line_index,
            word_index: next,
        };
    }

    /// Opens the INDEX.TXT handle on first use; it holds no rows (lookups
    /// binary-search the file on SD), so this is a single `stat`.
    fn cached_dictionary_index(&mut self) -> Result<&DictionaryIndex, String> {
        if self.dictionary_index_cache.is_none() {
            self.dictionary_index_cache = Some(
                DictionaryIndex::open(Path::new(DICTIONARY_ROOT))
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(self
            .dictionary_index_cache
            .as_ref()
            .expect("just populated above"))
    }

    /// Confirms the current word-select cursor: looks the word up in the
    /// on-SD dictionary pack and moves to the Definition phase.
    pub fn dictionary_confirm_word(&mut self) {
        let ReaderDictionaryMode::WordSelect {
            line_index,
            word_index,
        } = &self.dictionary_mode
        else {
            return;
        };
        let (line_index, word_index) = (*line_index, *word_index);
        let Some(word) = self
            .dictionary_page_lines()
            .get(line_index)
            .and_then(|line| {
                eligible_word_spans(&line.text)
                    .get(word_index)
                    .map(|&(start, end)| line.text[start..end].to_string())
            })
        else {
            return;
        };
        let message = match self.cached_dictionary_index() {
            Ok(index) => match lookup_dictionary_explained(Path::new(DICTIONARY_ROOT), index, &word) {
                Ok(Some(entry)) => entry.definition,
                Ok(None) => "Word not found in dictionary.".to_string(),
                Err(error) => format!("Dictionary: {}", compact_error(&error.to_string())),
            },
            Err(error) => format!("Dictionary: {}", compact_error(&error)),
        };
        self.dictionary_mode = ReaderDictionaryMode::Definition {
            line_index,
            word_index,
            word,
            message,
        };
    }

    pub fn cycle_option_previous(&mut self) {
        self.options_selected = self
            .options_selected
            .checked_sub(1)
            .unwrap_or(ReaderOption::ALL.len() - 1);
    }

    pub fn cycle_option_next(&mut self) {
        self.options_selected = (self.options_selected + 1) % ReaderOption::ALL.len();
    }

    #[must_use]
    pub fn selected_option(&self) -> ReaderOption {
        ReaderOption::ALL[self.options_selected]
    }

    /// Resolve a bookmark's user-facing page label against the active layout
    /// when nearby anchors are available. The persisted byte offset remains the
    /// canonical bookmark authority; the stored page index is a safe fallback.
    #[must_use]
    pub fn bookmark_display_page(&self, bookmark: &ReaderLocation) -> usize {
        self.session
            .as_ref()
            .filter(|session| bookmark.matches_book(&session.book))
            .and_then(|session| {
                session
                    .page_offsets
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, offset)| **offset <= bookmark.byte_offset)
                    .map(|(index, _)| {
                        session
                            .page_number_base
                            .saturating_add(index)
                            .saturating_add(1)
                    })
            })
            .unwrap_or_else(|| bookmark.page_index.saturating_add(1))
    }

    /// Resolve an EPUB bookmark against the active layout when possible and
    /// otherwise use the persisted chapter-relative fallback stored in MARKS.TXT.
    #[must_use]
    pub fn bookmark_display_chapter_page(
        &self,
        bookmark: &ReaderLocation,
    ) -> Option<ReaderChapterPageLabel> {
        if bookmark.format != BookFormat::Epub {
            return None;
        }
        self.session
            .as_ref()
            .filter(|session| bookmark.matches_book(&session.book))
            .and_then(|session| session.epub_chapter_page_label_for_offset(bookmark.byte_offset))
            .or_else(|| bookmark.epub_chapter.clone())
    }

    #[must_use]
    pub fn has_structured_toc(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| !session.toc_entries().is_empty())
    }

    #[must_use]
    pub fn toc_entries(&self) -> &[EpubTocEntry] {
        self.session
            .as_ref()
            .map_or(&[], ReaderSession::toc_entries)
    }

    pub fn apply_toc_button(&mut self, event: ButtonEvent) -> bool {
        let count = self.toc_entries().len();
        if count == 0 {
            return false;
        }
        match event {
            ButtonEvent::Up => {
                self.toc_selected = self.toc_selected.checked_sub(1).unwrap_or(count - 1);
                false
            }
            ButtonEvent::Down => {
                self.toc_selected = (self.toc_selected + 1) % count;
                false
            }
            ButtonEvent::Select => self.open_selected_toc_entry(),
        }
    }

    fn open_selected_toc_entry(&mut self) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let Some(entry) = session
            .epub_document
            .as_ref()
            .and_then(|document| document.toc.get(self.toc_selected))
            .cloned()
        else {
            return false;
        };
        let outcome = {
            let Some(document) = session.epub_document.as_ref() else {
                return false;
            };
            let Some(chapter) = document.chapter_for_offset(entry.text_offset).cloned() else {
                return false;
            };
            read_epub_page(document, session.layout, entry.text_offset, 0)
                .map(|page| (page, document.text_size_bytes(), chapter))
        };
        session.page_number_base = 0;
        session.current_page = 0;
        session.page_offsets = vec![entry.text_offset];
        session.indexed_through = entry.text_offset;
        session.index_complete = false;
        session.cache.clear();
        match outcome {
            Ok((page, source_size, chapter)) => {
                let indexed_through = page.next_byte_offset.min(chapter.text_end_offset);
                session.indexed_through = indexed_through;
                session.index_complete = indexed_through >= source_size;
                // `epub_chapter_pages`/`epub_pending_chapter` still describe
                // wherever the reader was before the jump. Left alone,
                // background pagination (`index_one_epub_page`) picks "next
                // chapter" from that stale history and keeps extending the
                // wrong chapter onto the just-reset `page_offsets`, corrupting
                // both page-turning and `reading_percent` (and, once
                // persisted, the on-disk `.EPP` cache too). Re-seed both from
                // the jump target's own chapter instead, mirroring the
                // cache-miss path in `open_epub_session`.
                if session.index_complete {
                    session.epub_chapter_pages = vec![ReaderEpubChapterPages {
                        chapter_number: chapter.number,
                        text_offset: chapter.text_offset,
                        text_end_offset: chapter.text_end_offset,
                        page_offsets: vec![entry.text_offset],
                    }];
                    session.epub_pending_chapter = None;
                } else {
                    session.epub_chapter_pages = Vec::new();
                    session.epub_pending_chapter = Some(PendingEpubChapterIndex {
                        next_offset: indexed_through,
                        chapter,
                        page_offsets: vec![entry.text_offset],
                    });
                }
                session.push_cached_page(page);
                self.last_message = Some(format!("TOC: {}", entry.label));
                self.persist_current_session_best_effort();
                true
            }
            Err(error) => {
                self.last_message = Some(error);
                false
            }
        }
    }

    #[must_use]
    pub fn current_page_is_bookmarked(&self) -> bool {
        let Some(location) = self.session.as_ref().map(ReaderSession::current_location) else {
            return false;
        };
        self.bookmarks
            .iter()
            .any(|bookmark| bookmark.same_position(&location))
    }

    pub fn toggle_current_bookmark(&mut self) {
        let Some(location) = self.session.as_ref().map(ReaderSession::current_location) else {
            self.last_message = Some("Open a Reader page before adding a bookmark".into());
            return;
        };
        if let Some(index) = self
            .bookmarks
            .iter()
            .position(|bookmark| bookmark.same_position(&location))
        {
            self.bookmarks.remove(index);
            self.bookmarks_selected = self
                .bookmarks_selected
                .min(self.bookmarks.len().saturating_sub(1));
            self.last_message = Some("Bookmark removed".into());
        } else {
            self.bookmarks.insert(0, location);
            self.bookmarks.truncate(READER_BOOKMARK_LIMIT);
            self.bookmarks_selected = 0;
            self.last_message = Some("Bookmark saved".into());
        }
        self.persist_bookmarks_best_effort();
    }

    pub fn begin_preferences_edit(&mut self) {
        self.preferences_selected = 0;
        self.preference_edit = None;
        self.preferences_layout_dirty = false;
    }

    pub fn cycle_preference_previous(&mut self) {
        self.preferences_selected = self
            .preferences_selected
            .checked_sub(1)
            .unwrap_or(ReadingPreference::ALL.len() - 1);
    }

    pub fn cycle_preference_next(&mut self) {
        self.preferences_selected = (self.preferences_selected + 1) % ReadingPreference::ALL.len();
    }

    #[must_use]
    pub fn selected_preference(&self) -> ReadingPreference {
        ReadingPreference::ALL[self.preferences_selected]
    }

    /// Open the highlighted row's editor: a candidate copy of `preferences`
    /// that UP/DOWN will browse in place, leaving the real `preferences`
    /// (and persistence, and pagination) untouched until `commit_preference_edit`.
    pub fn open_preference_editor(&mut self) {
        self.preference_edit = Some(self.preferences);
    }

    /// Browse to the previous/next candidate for the field the editor is
    /// currently open on. No-op (aside from the `expect`) if no editor is
    /// open; callers only reach this while `preference_edit.is_some()`.
    pub fn cycle_preference_editor_previous(&mut self) {
        let preference = self.selected_preference();
        let candidate = self
            .preference_edit
            .as_mut()
            .expect("cycle_preference_editor_previous called with no editor open");
        match preference {
            ReadingPreference::ReadingTheme => candidate.theme = candidate.theme.previous(),
            ReadingPreference::Orientation => {
                candidate.orientation = candidate.orientation.previous();
            }
            ReadingPreference::BookFontSize => candidate.font_size = candidate.font_size.previous(),
            ReadingPreference::BookFont => candidate.book_font = candidate.book_font.previous(),
            ReadingPreference::ParagraphAlignment => {
                candidate.paragraph_alignment = candidate.paragraph_alignment.previous();
            }
            ReadingPreference::ShowProgress => candidate.show_progress = !candidate.show_progress,
            ReadingPreference::TapPageTurn => {
                candidate.tap_page_turn_enabled = !candidate.tap_page_turn_enabled;
            }
            ReadingPreference::FullScreen => candidate.full_screen = !candidate.full_screen,
        }
    }

    pub fn cycle_preference_editor_next(&mut self) {
        let preference = self.selected_preference();
        let candidate = self
            .preference_edit
            .as_mut()
            .expect("cycle_preference_editor_next called with no editor open");
        match preference {
            ReadingPreference::ReadingTheme => candidate.theme = candidate.theme.next(),
            ReadingPreference::Orientation => candidate.orientation = candidate.orientation.next(),
            ReadingPreference::BookFontSize => candidate.font_size = candidate.font_size.next(),
            ReadingPreference::BookFont => candidate.book_font = candidate.book_font.next(),
            ReadingPreference::ParagraphAlignment => {
                candidate.paragraph_alignment = candidate.paragraph_alignment.next();
            }
            ReadingPreference::ShowProgress => candidate.show_progress = !candidate.show_progress,
            ReadingPreference::TapPageTurn => {
                candidate.tap_page_turn_enabled = !candidate.tap_page_turn_enabled;
            }
            ReadingPreference::FullScreen => candidate.full_screen = !candidate.full_screen,
        }
    }

    /// Discard the open editor, if any, restoring the list to its
    /// pre-edit state. Returns whether an editor was actually open, so
    /// callers (BACK) know whether they should stay on the list or fall
    /// through to normal back-navigation.
    #[must_use]
    pub fn cancel_preference_edit(&mut self) -> bool {
        self.preference_edit.take().is_some()
    }

    /// Commit the open editor's candidate as the new `preferences`. Mirrors
    /// the previous immediate-apply behavior once a value is actually
    /// chosen: redraw-only settings persist immediately in place,
    /// layout-sensitive settings persist and request a staged current-page
    /// rebuild. Returns `false` with no effect if no editor was open.
    #[must_use]
    pub fn commit_preference_edit(&mut self) -> bool {
        let Some(candidate) = self.preference_edit.take() else {
            return false;
        };
        self.preferences = candidate;
        // Parked sessions were paginated against the old layout; rather than
        // track which specific setting invalidates them, just drop them —
        // they are cheap to rebuild (`tick_background_warmup`/on next visit)
        // and a preference commit is rare compared to page turns.
        self.session_cache.clear();
        let layout_sensitive = match self.selected_preference() {
            ReadingPreference::ReadingTheme => {
                self.last_message =
                    Some(format!("Reading theme: {}", self.preferences.theme.label()));
                self.persist_preferences_best_effort();
                self.request_clear_ghosting();
                false
            }
            ReadingPreference::Orientation => {
                self.last_message = Some(format!(
                    "Orientation: {}",
                    self.preferences.orientation.label()
                ));
                true
            }
            ReadingPreference::BookFontSize => {
                self.last_message = Some(format!(
                    "Book font size: {}",
                    self.preferences.font_size.label()
                ));
                true
            }
            ReadingPreference::BookFont => {
                self.last_message =
                    Some(format!("Book font: {}", self.preferences.book_font.label()));
                true
            }
            ReadingPreference::ParagraphAlignment => {
                self.last_message = Some(format!(
                    "Paragraph alignment: {}",
                    self.preferences.paragraph_alignment.label()
                ));
                true
            }
            ReadingPreference::ShowProgress => {
                self.last_message = Some(format!(
                    "Show progress: {}",
                    if self.preferences.show_progress {
                        "On"
                    } else {
                        "Off"
                    }
                ));
                self.persist_preferences_best_effort();
                false
            }
            ReadingPreference::TapPageTurn => {
                self.last_message = Some(format!(
                    "Tap page-turn: {}",
                    if self.preferences.tap_page_turn_enabled {
                        "On"
                    } else {
                        "Off"
                    }
                ));
                self.persist_preferences_best_effort();
                false
            }
            ReadingPreference::FullScreen => {
                self.last_message = Some(format!(
                    "Full screen: {}",
                    if self.preferences.full_screen {
                        "On"
                    } else {
                        "Off"
                    }
                ));
                // More (or fewer) lines per page: repaginate like a font
                // size change does.
                true
            }
        };
        if layout_sensitive {
            self.request_layout_rebuild()
        } else {
            false
        }
    }

    pub fn cycle_reading_theme(&mut self) {
        self.preferences.theme = self.preferences.theme.next();
        self.last_message = Some(format!("Reading theme: {}", self.preferences.theme.label()));
        self.persist_preferences_best_effort();
        self.request_clear_ghosting();
    }

    pub fn cycle_orientation(&mut self) -> bool {
        self.preferences.orientation = self.preferences.orientation.next();
        self.last_message = Some(format!(
            "Orientation: {}",
            self.preferences.orientation.label()
        ));
        self.request_layout_rebuild()
    }

    pub fn cycle_book_font_size(&mut self) -> bool {
        self.preferences.font_size = self.preferences.font_size.next();
        self.last_message = Some(format!(
            "Book font size: {}",
            self.preferences.font_size.label()
        ));
        self.request_layout_rebuild()
    }

    pub fn cycle_book_font(&mut self) -> bool {
        self.preferences.book_font = self.preferences.book_font.next();
        self.last_message = Some(format!("Book font: {}", self.preferences.book_font.label()));
        self.request_layout_rebuild()
    }

    pub fn toggle_show_progress(&mut self) {
        self.preferences.show_progress = !self.preferences.show_progress;
        self.last_message = Some(format!(
            "Show progress: {}",
            if self.preferences.show_progress {
                "On"
            } else {
                "Off"
            }
        ));
        self.persist_preferences_best_effort();
    }

    pub fn request_clear_ghosting(&mut self) {
        self.clear_ghost_requested = true;
        self.last_message = Some("Global ghost-clearing refresh requested".into());
    }

    #[must_use]
    pub fn take_clear_ghost_request(&mut self) -> bool {
        core::mem::take(&mut self.clear_ghost_requested)
    }

    #[must_use]
    pub fn take_persistence_event(&mut self) -> Option<String> {
        self.persistence_event.take()
    }

    #[must_use]
    fn state_path(&self) -> PathBuf {
        Path::new(&self.state_root).join(READER_STATE_FILE)
    }

    #[must_use]
    fn positions_path(&self) -> PathBuf {
        Path::new(&self.state_root).join(READER_POSITIONS_FILE)
    }

    #[must_use]
    fn legacy_positions_path(&self) -> PathBuf {
        Path::new(&self.state_root).join(LEGACY_READER_POSITIONS_FILE)
    }

    fn load_positions_with_legacy_migration(&mut self) -> Result<Vec<ReaderLocation>, String> {
        let positions = self.positions_path();
        let positions_backup = with_extension(&positions, "BAK");
        if positions.exists() || positions_backup.exists() {
            return load_location_list(&positions, READER_POSITION_LIMIT);
        }

        let legacy = self.legacy_positions_path();
        let legacy_backup = with_extension(&legacy, "BAK");
        if !legacy.exists() && !legacy_backup.exists() {
            return Ok(Vec::new());
        }

        let migrated = load_location_list(&legacy, READER_POSITION_LIMIT)?;
        if !migrated.is_empty() {
            if let Err(error) = atomic_replace_text(&positions, &serialize_location_list(&migrated))
            {
                self.persistence_warning = Some(format!(
                    "legacy POSITIONS.TXT loaded; POSITS.TXT migration deferred: {error}"
                ));
            }
        }
        Ok(migrated)
    }

    #[must_use]
    fn recent_path(&self) -> PathBuf {
        Path::new(&self.state_root).join(READER_RECENT_FILE)
    }

    #[must_use]
    fn bookmarks_path(&self) -> PathBuf {
        Path::new(&self.state_root).join(READER_BOOKMARKS_FILE)
    }

    #[must_use]
    fn preferences_path(&self) -> PathBuf {
        Path::new(&self.state_root).join(READER_PREFS_FILE)
    }

    #[must_use]
    fn deep_sleep_active_path(&self) -> PathBuf {
        Path::new(&self.state_root).join(READER_DEEP_SLEEP_ACTIVE_FILE)
    }

    /// Record whether the Reader was the active screen when hardware deep
    /// sleep was entered, so the next boot can decide whether to auto-resume
    /// the last book instead of landing on Home. Best-effort, like the rest
    /// of Reader persistence: the caller logs failures and keeps going
    /// either way.
    pub fn record_deep_sleep_active_marker(&self, active: bool) -> io::Result<()> {
        fs::write(
            self.deep_sleep_active_path(),
            if active { "1" } else { "0" },
        )
    }

    /// Whether the last-recorded deep-sleep-entry marker says the Reader was
    /// active. Used only to decide whether to auto-resume on a hardware
    /// deep-sleep wake boot; a missing or unreadable marker safely means
    /// "no".
    #[must_use]
    pub fn deep_sleep_marker_indicates_active(&self) -> bool {
        fs::read_to_string(self.deep_sleep_active_path()).is_ok_and(|content| content.trim() == "1")
    }

    /// Shared cache root (`<state_root>/CACHE`) already used for `.EPX`/
    /// `.EPP`/`.CCH` sidecars. Public so [`crate::cover_cache::CoverCache`]
    /// can be constructed with the same root and keep `.THB` thumbnail files
    /// alongside them.
    #[must_use]
    pub fn cache_directory(&self) -> PathBuf {
        Path::new(&self.state_root).join(READER_CACHE_DIRECTORY)
    }

    #[must_use]
    fn cache_file_name_for(book: &ReaderBook, layout: ReaderLayout) -> String {
        format!("{:08X}.CCH", book_fingerprint(book, layout) as u32)
    }

    #[must_use]
    fn cache_path_for(&self, book: &ReaderBook, layout: ReaderLayout) -> PathBuf {
        self.cache_directory()
            .join(Self::cache_file_name_for(book, layout))
    }

    #[must_use]
    fn epub_document_cache_file_name_for(book: &ReaderBook) -> String {
        format!("{:08X}.EPX", epub_document_fingerprint(book) as u32)
    }

    #[must_use]
    fn epub_document_cache_path_for(&self, book: &ReaderBook) -> PathBuf {
        self.cache_directory()
            .join(Self::epub_document_cache_file_name_for(book))
    }

    /// Load one flattened-EPUB-text cache written by a previous open of the
    /// same book. A corrupt or stale (fingerprint-mismatched) cache is
    /// reported as a warning and ignored rather than blocking the open.
    fn load_epub_document_cache_best_effort(&mut self, book: &ReaderBook) -> Option<EpubDocument> {
        match load_epub_document_cache(&self.epub_document_cache_path_for(book), book) {
            Ok(Some(document)) => {
                log::info!(
                    "rustmix-wave=epub-document-cache status=hit spine-items={} toc-entries={} text-bytes={}",
                    document.spine_count,
                    document.toc.len(),
                    document.text_size_bytes()
                );
                Some(document)
            }
            Ok(None) => None,
            Err(error) => {
                log::warn!("rustmix-wave=epub-document-cache status=ignored error={error}");
                self.persistence_warning = Some(format!("EPUB cache ignored: {error}"));
                None
            }
        }
    }

    /// Persist the just-parsed flattened EPUB text so the next open of the
    /// same book can skip ZIP/DEFLATE/HTML-flatten work entirely. On success,
    /// returns the cache file's path and its text body offset so the caller
    /// can drop `document`'s RAM copy via [`EpubDocument::into_on_disk`].
    fn persist_epub_document_cache_best_effort(
        &mut self,
        book: &ReaderBook,
        document: &EpubDocument,
    ) -> Option<(PathBuf, u64)> {
        let path = self.epub_document_cache_path_for(book);
        let fingerprint = epub_document_fingerprint(book);
        let (content, body_offset) = match serialize_epub_document_cache(document, fingerprint) {
            Ok(value) => value,
            Err(error) => {
                log::warn!("rustmix-wave=epub-document-cache status=save-failed error={error}");
                self.persistence_warning = Some(format!("EPUB cache not saved: {error}"));
                return None;
            }
        };
        match atomic_replace_cache_text(&path, &content) {
            Ok(()) => {
                log::info!("rustmix-wave=epub-document-cache status=saved");
                Some((path, body_offset))
            }
            Err(error) => {
                log::warn!("rustmix-wave=epub-document-cache status=save-failed error={error}");
                self.persistence_warning = Some(format!("EPUB cache not saved: {error}"));
                None
            }
        }
    }

    #[must_use]
    fn epub_page_index_cache_file_name_for(book: &ReaderBook, layout: ReaderLayout) -> String {
        format!("{:08X}.EPP", book_fingerprint(book, layout) as u32)
    }

    #[must_use]
    fn epub_page_index_cache_path_for(&self, book: &ReaderBook, layout: ReaderLayout) -> PathBuf {
        self.cache_directory()
            .join(Self::epub_page_index_cache_file_name_for(book, layout))
    }

    /// Load one EPUB page-offset index cache written by a previous open of the
    /// same book at the same Reader layout. Pagination scans every page of the
    /// book to build this index, so a hit skips the single most expensive step
    /// left in EPUB session startup.
    fn load_epub_chapter_pages_cache_best_effort(
        &mut self,
        book: &ReaderBook,
        layout: ReaderLayout,
    ) -> Option<Vec<ReaderEpubChapterPages>> {
        match load_epub_page_index_cache(
            &self.epub_page_index_cache_path_for(book, layout),
            book,
            layout,
        ) {
            Ok(Some(pages)) => {
                let total_pages: usize =
                    pages.iter().map(|chapter| chapter.page_offsets.len()).sum();
                log::info!(
                    "rustmix-wave=epub-page-index-cache status=hit chapters={} pages={total_pages}",
                    pages.len()
                );
                Some(pages)
            }
            Ok(None) => None,
            Err(error) => {
                log::warn!("rustmix-wave=epub-page-index-cache status=ignored error={error}");
                self.persistence_warning = Some(format!("EPUB page cache ignored: {error}"));
                None
            }
        }
    }

    /// Persist a just-computed EPUB page-offset index so the next open of the
    /// same book at the same layout can skip full-book pagination entirely.
    fn persist_epub_chapter_pages_cache_best_effort(
        &mut self,
        book: &ReaderBook,
        layout: ReaderLayout,
        pages: &[ReaderEpubChapterPages],
    ) {
        let path = self.epub_page_index_cache_path_for(book, layout);
        let fingerprint = book_fingerprint(book, layout);
        match atomic_replace_cache_text(&path, &serialize_epub_page_index_cache(pages, fingerprint))
        {
            Ok(()) => {
                log::info!("rustmix-wave=epub-page-index-cache status=saved");
            }
            Err(error) => {
                log::warn!("rustmix-wave=epub-page-index-cache status=save-failed error={error}");
                self.persistence_warning = Some(format!("EPUB page cache not saved: {error}"));
            }
        }
    }

    fn open_txt_session(
        &mut self,
        book: &ReaderBook,
        encoding: TextEncoding,
        requested: Option<&ReaderLocation>,
    ) -> Result<ReaderSession, String> {
        let cached = match load_anchor_cache(
            &self.cache_path_for(book, self.preferences.layout()),
            book,
            self.preferences.layout(),
        ) {
            Ok(value) => value,
            Err(error) => {
                self.persistence_warning = Some(format!("TXT cache ignored: {error}"));
                None
            }
        };
        let (page_number_base, page_offsets, current_page, indexed_through, index_complete) =
            if let Some(cache) = cached {
                let selected = requested
                    .filter(|location| location.matches_book(book))
                    .and_then(|location| {
                        location
                            .page_index
                            .checked_sub(cache.base_page)
                            .filter(|index| *index < cache.offsets.len())
                    })
                    .unwrap_or(0);
                (
                    cache.base_page,
                    cache.offsets,
                    selected,
                    cache.indexed_through,
                    cache.complete,
                )
            } else if let Some(location) = requested.filter(|location| location.matches_book(book))
            {
                (
                    location.page_index,
                    vec![location.byte_offset.min(book.size_bytes)],
                    0,
                    location.byte_offset.min(book.size_bytes),
                    false,
                )
            } else {
                (0, vec![0], 0, 0, false)
            };
        let offset = page_offsets.get(current_page).copied().unwrap_or(0);
        let absolute_page = page_number_base.saturating_add(current_page);
        let layout = self.preferences.layout();
        let page = read_txt_page(book, encoding, layout, offset, absolute_page)?;
        let indexed_through = indexed_through.max(page.next_byte_offset);
        let index_complete = index_complete || indexed_through >= book.size_bytes;
        Ok(ReaderSession {
            book: book.clone(),
            encoding,
            epub_document: None,
            layout,
            current_page,
            page_number_base,
            page_offsets,
            indexed_through,
            index_complete,
            cache: vec![page],
            epub_chapter_pages: Vec::new(),
            epub_pending_chapter: None,
            epub_document_cache_pending: false,
        })
    }

    fn open_epub_session(
        &mut self,
        book: &ReaderBook,
        document: EpubDocument,
        requested: Option<&ReaderLocation>,
        epub_document_cache_pending: bool,
        bounded_pagination_only: bool,
    ) -> Result<ReaderSession, String> {
        let source_size = document.text_size_bytes();
        let layout = self.preferences.layout();
        let requested = requested.filter(|location| location.matches_book(book));
        let requested_offset =
            requested.map_or(0, |location| location.byte_offset.min(source_size));

        // On a `.EPP` hit, the full layout-specific page index is already
        // known — keep the eager, immediately-complete path (fast, warm
        // open). On a miss (first-ever open, or any font/size/orientation
        // change), pre-paginating the *whole* book synchronously would
        // freeze the UI on `BuildingFirstPage`; instead paginate from the
        // resume chapter's start up to the resume page (bounded by how many
        // pages precede it within that one chapter, not the whole book —
        // just one page for a fresh, resume-less open) so the reader can
        // still page backward through everything already read before this
        // session, exactly as if it had been indexed forward normally.
        // Earlier chapters remain unindexed until `previous_page` actually
        // needs them (see `ReaderSession::extend_backward`), and `tick()`'s
        // background loop (`ReaderSession::index_one_epub_page`) extends
        // forward one page at a time from the resume point.
        // Background warm-up (`bounded_pagination_only`) never looks at the
        // full persisted page index at all, even when one exists: a real
        // open benefits from having every chapter's page offsets ready for
        // immediate forward/backward navigation, but warm-up only exists to
        // make the *resume page* instant, so loading (and holding in RAM)
        // the whole book's index -- up to dozens of small per-chapter
        // allocations, measured in the field to fragment internal SRAM by
        // tens of KiB per book -- buys nothing a warmed session actually
        // uses. Falling through to the same bounded one-chapter pagination
        // used for a genuine cache miss below caps the cost at one chapter
        // regardless of book size, with the rest extended lazily by
        // `tick()`'s background loop exactly as a cold open already does.
        let cache_candidate = if bounded_pagination_only {
            None
        } else {
            self.load_epub_chapter_pages_cache_best_effort(book, layout)
                // The persisted index only ever covers a contiguous run of
                // chapters starting from wherever some earlier session began
                // indexing (not necessarily the book's true start — see the
                // persistence comment in `tick()`). Treat it as a hit only if
                // that run actually contains the page we're resuming to; a
                // cache from a different part of the book (a stale TOC jump, or
                // reopening at a spot indexing never reached) falls through to
                // the same one-chapter pagination as a fresh cache miss below.
                .filter(|cached| {
                    cached.iter().any(|chapter| {
                        requested_offset >= chapter.text_offset
                            && (requested_offset < chapter.text_end_offset
                                || (requested_offset == chapter.text_end_offset
                                    && chapter.text_end_offset == source_size))
                    })
                })
        };
        let (
            epub_chapter_pages,
            epub_pending_chapter,
            page_offsets,
            indexed_through,
            index_complete,
        ) = match cache_candidate {
            Some(cached) => {
                let page_offsets: Vec<u64> = cached
                    .iter()
                    .flat_map(|chapter| chapter.page_offsets.iter().copied())
                    .collect();
                if page_offsets.is_empty() {
                    return Err("EPUB chapter pagination produced no readable pages".into());
                }
                let indexed_through = cached
                    .last()
                    .map_or(source_size, |chapter| chapter.text_end_offset);
                let index_complete = indexed_through >= source_size;
                (cached, None, page_offsets, indexed_through, index_complete)
            }
            None => {
                let chapter = document
                    .chapter_for_offset(requested_offset)
                    .cloned()
                    .ok_or_else(|| "EPUB requested offset is out of range".to_string())?;
                let page_offsets =
                    paginate_epub_chapter_up_to(&document, layout, &chapter, requested_offset, 0)?;
                if page_offsets.is_empty() {
                    return Err("EPUB chapter pagination produced no readable pages".into());
                }
                let resume_offset = *page_offsets.last().expect("checked not empty above");
                let resume_page = read_epub_page_until(
                    &document,
                    layout,
                    resume_offset,
                    page_offsets.len() - 1,
                    chapter.text_end_offset,
                )?;
                let indexed_through = resume_page.next_byte_offset.min(chapter.text_end_offset);
                let index_complete = indexed_through >= source_size;
                let (epub_chapter_pages, epub_pending_chapter) = if index_complete {
                    let finished = vec![ReaderEpubChapterPages {
                        chapter_number: chapter.number,
                        text_offset: chapter.text_offset,
                        text_end_offset: chapter.text_end_offset,
                        page_offsets: page_offsets.clone(),
                    }];
                    // A resume landing on the last page of its chapter, with
                    // that chapter reaching the book's end, goes straight
                    // from "just opened" to "fully indexed" without ever
                    // revisiting `tick()`'s background loop, which is what
                    // normally triggers the persist. Persist here too so
                    // this edge case still gets its `.EPP` written.
                    self.persist_epub_chapter_pages_cache_best_effort(book, layout, &finished);
                    (finished, None)
                } else {
                    (
                        Vec::new(),
                        Some(PendingEpubChapterIndex {
                            next_offset: indexed_through,
                            chapter,
                            page_offsets: page_offsets.clone(),
                        }),
                    )
                };
                (
                    epub_chapter_pages,
                    epub_pending_chapter,
                    page_offsets,
                    indexed_through,
                    index_complete,
                )
            }
        };
        let current_page = page_offsets
            .partition_point(|anchor| *anchor <= requested_offset)
            .saturating_sub(1)
            .min(page_offsets.len().saturating_sub(1));
        let offset = page_offsets[current_page];
        let page = read_epub_page(&document, layout, offset, current_page)?;
        let mut session_book = book.clone();
        if !document.title.trim().is_empty() {
            session_book.title = document.title.clone();
        }
        Ok(ReaderSession {
            book: session_book,
            encoding: TextEncoding::Utf8,
            epub_document: Some(document),
            layout,
            current_page,
            page_number_base: 0,
            page_offsets,
            indexed_through,
            index_complete,
            cache: vec![page],
            epub_chapter_pages,
            epub_pending_chapter,
            epub_document_cache_pending,
        })
    }

    fn persist_current_session_best_effort(&mut self) {
        let Some(location) = self.session.as_ref().map(ReaderSession::current_location) else {
            return;
        };
        let _span = crate::boot_profile::span("reader-persist-session");
        self.resume = Some(location.clone());
        self.positions.retain(|entry| entry.path != location.path);
        self.positions.insert(0, location.clone());
        self.positions.truncate(READER_POSITION_LIMIT);
        self.recent.retain(|entry| entry.path != location.path);
        self.recent.insert(0, location);
        self.recent.truncate(READER_RECENT_LIMIT);
        let mut errors = Vec::new();
        if let Some(location) = self.resume.as_ref() {
            if let Err(error) =
                atomic_replace_text(&self.state_path(), &serialize_location(location))
            {
                errors.push(format!("STATE.TXT: {error}"));
            }
        }
        if let Err(error) = atomic_replace_text(
            &self.positions_path(),
            &serialize_location_list(&self.positions),
        ) {
            errors.push(format!("POSITS.TXT: {error}"));
        }
        if let Err(error) =
            atomic_replace_text(&self.recent_path(), &serialize_location_list(&self.recent))
        {
            errors.push(format!("RECENT.TXT: {error}"));
        }
        if let Err(error) = self.persist_anchor_cache() {
            errors.push(format!("CACHE: {error}"));
        }
        self.finish_persistence("state-positions-recent-cache", errors);
    }

    fn persist_bookmarks_best_effort(&mut self) {
        let mut errors = Vec::new();
        if let Err(error) = atomic_replace_text(
            &self.bookmarks_path(),
            &serialize_location_list(&self.bookmarks),
        ) {
            errors.push(format!("MARKS.TXT: {error}"));
        }
        self.finish_persistence("bookmarks", errors);
    }

    /// Whether [`Self::persist_current_session_best_effort`] for `location`
    /// would leave STATE/POSITS/RECENT exactly as they already are.
    fn persisted_state_matches(&self, location: &ReaderLocation) -> bool {
        self.resume.as_ref() == Some(location)
            && self.positions.first() == Some(location)
            && self.recent.first() == Some(location)
    }

    fn persist_anchor_cache_best_effort(&mut self) {
        let mut errors = Vec::new();
        if let Err(error) = self.persist_anchor_cache() {
            errors.push(format!("CACHE: {error}"));
        }
        self.finish_persistence("anchor-cache", errors);
    }

    fn persist_anchor_cache(&self) -> Result<(), String> {
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        let Some(cache) = session.anchor_cache() else {
            return Ok(());
        };
        atomic_replace_cache_text(
            &self.cache_path_for(&session.book, session.layout),
            &serialize_anchor_cache(&cache),
        )
    }

    fn persist_preferences_best_effort(&mut self) {
        let mut errors = Vec::new();
        if let Err(error) =
            atomic_replace_text(&self.preferences_path(), &self.preferences.serialized())
        {
            errors.push(format!("PREFS.TXT: {error}"));
        }
        self.finish_persistence("preferences", errors);
    }

    fn finish_persistence(&mut self, scope: &str, errors: Vec<String>) {
        let event = if errors.is_empty() {
            format!("status=saved scope={scope}")
        } else {
            let warning = errors.join("; ");
            self.persistence_warning = Some(warning.clone());
            format!("status=degraded scope={scope} error={warning}")
        };
        if self.last_persistence_event.as_deref() != Some(event.as_str()) {
            self.last_persistence_event = Some(event.clone());
            self.persistence_event = Some(event);
        }
    }
}

/// Scan one bounded Reader library. TXT and EPUB/EPU rows open through the
/// shared staged Reader architecture.
///
/// `previous` is the prior scan's results (empty on the first scan of a
/// process). An EPUB whose path, size, and modification time still match an
/// entry in `previous` reuses that entry's title instead of reopening the
/// archive to reparse OPF metadata, so re-entering the Library screen after
/// the first scan doesn't pay the zip-parsing cost again for unchanged files.
///
/// `previous` alone only helps within one running process: it is empty again
/// after every deep-sleep wake (a full reboot -- see `mcu_deep_sleep`) and
/// every fresh Wi-Fi-transfer-portal session, both of which call this
/// function with their own separate, short-lived `previous` list. To avoid
/// reopening every EPUB's ZIP archive again in those cases too, titles are
/// also checked against (and, for freshly parsed ones, written back to) a
/// small SD-backed cache (see [`load_title_cache`]) shared by every caller.
pub fn scan_txt_library(
    root: impl AsRef<Path>,
    previous: &[ReaderBook],
) -> Result<Vec<ReaderBook>, String> {
    let root = root.as_ref();
    let mut books = Vec::new();
    let entries =
        fs::read_dir(root).map_err(|error| format!("Books folder unavailable: {error}"))?;
    let persisted_titles = load_title_cache();
    let mut freshly_parsed: Vec<TitleCacheEntry> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(format) = book_format_from_path(&path) else {
            continue;
        };
        let metadata = entry.metadata().ok();
        let size_bytes = metadata.as_ref().map_or(0, |meta| meta.len());
        let modified_seconds = metadata
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_secs());
        let path_str = path.to_string_lossy();
        let fallback_title = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Untitled book")
            .to_string();
        let cached_title = previous
            .iter()
            .find(|book| {
                book.path == path_str
                    && book.size_bytes == size_bytes
                    && book.modified_seconds == modified_seconds
            })
            .map(|book| book.title.clone())
            .or_else(|| {
                title_cache_lookup(&persisted_titles, &path_str, size_bytes, modified_seconds)
            });
        let title = if let Some(cached_title) = cached_title {
            cached_title
        } else if format == BookFormat::Epub {
            match read_epub_title_on_worker(&path)
                .ok()
                .filter(|value| !value.trim().is_empty())
            {
                Some(parsed) => {
                    freshly_parsed.push(TitleCacheEntry {
                        path: path_str.to_string(),
                        size_bytes,
                        modified_seconds,
                        title: parsed.clone(),
                    });
                    parsed
                }
                None => fallback_title,
            }
        } else {
            fallback_title
        };
        books.push(ReaderBook {
            path: path_str.into_owned(),
            title,
            format,
            size_bytes,
            modified_seconds,
        });
        if books.len() >= READER_LIBRARY_LIMIT {
            break;
        }
    }
    books.sort_by(|left, right| left.title.to_lowercase().cmp(&right.title.to_lowercase()));
    if !freshly_parsed.is_empty() {
        save_title_cache(&merge_title_cache(persisted_titles, freshly_parsed, &books));
    }
    Ok(books)
}

/// One SD-backed EPUB title cache row: the same `(path, size, modified)`
/// triple used everywhere else in Reader persistence to detect an unchanged
/// file, plus the title that was read from its OPF the last time it was
/// parsed.
struct TitleCacheEntry {
    path: String,
    size_bytes: u64,
    modified_seconds: u64,
    title: String,
}

/// SD-backed cache of EPUB titles read from each book's OPF metadata. Shared
/// by every [`scan_txt_library`] caller (the on-device Library screen and the
/// Wi-Fi-transfer portal's `/api/books`) so a title learned once survives a
/// deep-sleep wake (full reboot) and a fresh portal session alike, instead of
/// reopening and re-parsing that EPUB's ZIP archive every time either one
/// starts from an empty in-memory `previous` list. Purely regenerable: a
/// missing or corrupt cache just means the next scan re-derives every title
/// once, exactly like before this cache existed.
const READER_TITLE_CACHE_FILE: &str = "TITLES.TXT";
const READER_TITLE_CACHE_VERSION: &str = "1";

fn title_cache_path() -> PathBuf {
    Path::new(READER_STATE_DIRECTORY)
        .join(READER_CACHE_DIRECTORY)
        .join(READER_TITLE_CACHE_FILE)
}

fn load_title_cache() -> Vec<TitleCacheEntry> {
    let Ok(text) = fs::read_to_string(title_cache_path()) else {
        return Vec::new();
    };
    let mut version = None;
    let mut entries = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("version=") {
            version = Some(value);
        } else if let Some(value) = line.strip_prefix("entry=") {
            if entries.len() >= READER_LIBRARY_LIMIT {
                continue;
            }
            let Ok(fields) = split_escaped_tabs(value) else {
                continue;
            };
            if fields.len() != 4 {
                continue;
            }
            let (Ok(size_bytes), Ok(modified_seconds)) = (fields[1].parse(), fields[2].parse())
            else {
                continue;
            };
            entries.push(TitleCacheEntry {
                path: fields[0].clone(),
                size_bytes,
                modified_seconds,
                title: fields[3].clone(),
            });
        }
    }
    if version != Some(READER_TITLE_CACHE_VERSION) {
        return Vec::new();
    }
    entries
}

fn save_title_cache(entries: &[TitleCacheEntry]) {
    let mut output = format!("version={READER_TITLE_CACHE_VERSION}\n");
    for entry in entries.iter().take(READER_LIBRARY_LIMIT) {
        output.push_str(&format!(
            "entry={}\t{}\t{}\t{}\n",
            escape_field(&entry.path),
            entry.size_bytes,
            entry.modified_seconds,
            escape_field(&entry.title)
        ));
    }
    // Best-effort and skips fsync, like the other regenerable caches under
    // `CACHE/`: losing this write to a power cut just means the next scan
    // re-parses whichever titles didn't make it to disk, same as today.
    let _ = atomic_replace_cache_text(&title_cache_path(), &output);
}

fn title_cache_lookup(
    cache: &[TitleCacheEntry],
    path: &str,
    size_bytes: u64,
    modified_seconds: u64,
) -> Option<String> {
    cache
        .iter()
        .find(|entry| {
            entry.path == path
                && entry.size_bytes == size_bytes
                && entry.modified_seconds == modified_seconds
        })
        .map(|entry| entry.title.clone())
}

/// Combine the cache loaded at the start of a scan with titles freshly
/// parsed during it, dropping any row whose file no longer matches something
/// in the just-completed `books` result (deleted, renamed, or changed) so
/// the persisted cache never grows stale or unbounded.
fn merge_title_cache(
    persisted: Vec<TitleCacheEntry>,
    freshly_parsed: Vec<TitleCacheEntry>,
    books: &[ReaderBook],
) -> Vec<TitleCacheEntry> {
    let still_current = |entry: &TitleCacheEntry| {
        books.iter().any(|book| {
            book.path == entry.path
                && book.size_bytes == entry.size_bytes
                && book.modified_seconds == entry.modified_seconds
        })
    };
    let mut merged: Vec<TitleCacheEntry> = persisted.into_iter().filter(still_current).collect();
    for fresh in freshly_parsed {
        merged.retain(|entry| entry.path != fresh.path);
        merged.push(fresh);
    }
    merged
}

/// Turns a raw background-worker start failure (out-of-memory spawning the
/// EPUB parser's dedicated thread — an `ENOMEM`/"Not enough space" the
/// reader has no way to act on) into a message that actually tells them
/// what to do. Any other error passes through unchanged.
#[must_use]
fn friendly_worker_start_error(error: &str) -> String {
    if error.contains("worker start failed") && error.contains("Not enough space") {
        "Not enough free memory to open this book right now. Restart the device and try again."
            .into()
    } else {
        error.into()
    }
}

#[must_use]
pub fn book_format_from_path(path: &Path) -> Option<BookFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "txt" => Some(BookFormat::Text),
        "epub" | "epu" => Some(BookFormat::Epub),
        _ => None,
    }
}

pub fn detect_txt_encoding(path: impl AsRef<Path>) -> Result<TextEncoding, String> {
    let mut file = File::open(path.as_ref()).map_err(|error| format!("Open failed: {error}"))?;
    let mut sample = vec![0_u8; 4096];
    let read = file
        .read(&mut sample)
        .map_err(|error| format!("Read failed: {error}"))?;
    sample.truncate(read);
    if sample.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Ok(TextEncoding::Utf8Bom);
    }
    match std::str::from_utf8(&sample) {
        Ok(_) => Ok(TextEncoding::Utf8),
        Err(error) if error.error_len().is_none() => Ok(TextEncoding::Utf8),
        Err(_) => Ok(TextEncoding::Windows1252),
    }
}

fn read_reader_page(
    book: &ReaderBook,
    encoding: TextEncoding,
    layout: ReaderLayout,
    epub_document: Option<&EpubDocument>,
    byte_offset: u64,
    page_index: usize,
) -> Result<ReaderCachedPage, String> {
    match book.format {
        BookFormat::Text => read_txt_page(book, encoding, layout, byte_offset, page_index),
        BookFormat::Epub => read_epub_page(
            epub_document.ok_or_else(|| "EPUB document is unavailable".to_string())?,
            layout,
            byte_offset,
            page_index,
        ),
    }
}

/// Paginate `chapter` from its start, collecting page offsets for as long as
/// each page's start is at or before `limit` and still within the chapter.
/// Bounded by "distance from the chapter's start to `limit`", not the whole
/// chapter — used both to seed a session at an arbitrary resume point
/// (`limit` = the resume offset, so pagination stops right at the resume
/// page instead of continuing through the rest of the chapter) and to
/// extend an already-open session backward on demand (`limit` = the offset
/// just before an already-known page, or a previous chapter's end to pull
/// it in whole). Word-wrap only accumulates forward, so finding "the page
/// before X" has no shortcut other than re-wrapping from the chapter start.
fn paginate_epub_chapter_up_to(
    document: &EpubDocument,
    layout: ReaderLayout,
    chapter: &EpubChapter,
    limit: u64,
    already_indexed_pages: usize,
) -> Result<Vec<u64>, String> {
    let mut page_offsets = Vec::new();
    let mut offset = chapter.text_offset;
    while offset < chapter.text_end_offset && offset <= limit {
        if already_indexed_pages + page_offsets.len() >= READER_EPUB_PAGE_ANCHOR_LIMIT {
            return Err(format!(
                "EPUB pagination exceeds {} page anchor limit",
                READER_EPUB_PAGE_ANCHOR_LIMIT
            ));
        }
        page_offsets.push(offset);
        let page = read_epub_page_until(
            document,
            layout,
            offset,
            page_offsets.len() - 1,
            chapter.text_end_offset,
        )?;
        if page.next_byte_offset <= offset {
            return Err(format!(
                "EPUB chapter {} pagination did not advance",
                chapter.number
            ));
        }
        offset = page.next_byte_offset.min(chapter.text_end_offset);
    }
    Ok(page_offsets)
}

fn read_epub_page(
    document: &EpubDocument,
    layout: ReaderLayout,
    byte_offset: u64,
    page_index: usize,
) -> Result<ReaderCachedPage, String> {
    let chapter_end = document
        .chapter_for_offset(byte_offset)
        .map_or(document.text_size_bytes(), |chapter| {
            chapter.text_end_offset
        });
    read_epub_page_until(document, layout, byte_offset, page_index, chapter_end)
}

fn read_epub_page_until(
    document: &EpubDocument,
    layout: ReaderLayout,
    byte_offset: u64,
    page_index: usize,
    text_end_offset: u64,
) -> Result<ReaderCachedPage, String> {
    let text_len = usize::try_from(document.text_size_bytes()).unwrap_or(usize::MAX);
    let start = usize::try_from(byte_offset)
        .map_err(|_| "EPUB byte offset exceeds platform range".to_string())?
        .min(text_len);
    let bounded_end = usize::try_from(text_end_offset)
        .map_err(|_| "EPUB chapter end exceeds platform range".to_string())?
        .min(text_len);
    let width_of = reader_layout_measure(&layout);
    let mut window_len = READER_PAGE_INITIAL_READ_BYTES.min(READER_PAGE_READ_BYTES);
    loop {
        let window_end = start.saturating_add(window_len).min(bounded_end);
        let input_is_final = window_end >= bounded_end || window_len >= READER_PAGE_READ_BYTES;
        let window = document.text_window(start, window_end)?;
        let local_start = next_utf8_boundary(&window, 0);
        let local_end = previous_utf8_boundary(&window, window.len()).max(local_start);
        let page_start = start + local_start;
        let bytes = &window[local_start..local_end];
        let decoded = decode_with_offsets(bytes, TextEncoding::Utf8, page_start as u64);
        let normalized = normalize_decoded(&decoded);
        if let Some((lines, consumed)) = paginate_decoded_window(
            &normalized,
            layout,
            &document.images,
            &width_of,
            input_is_final,
        ) {
            let next_byte_offset = consumed.max(page_start as u64).min(text_end_offset);
            return Ok(ReaderCachedPage {
                page_index,
                byte_offset: page_start as u64,
                next_byte_offset,
                lines,
            });
        }
        window_len = window_len.saturating_mul(2).min(READER_PAGE_READ_BYTES);
    }
}

fn next_utf8_boundary(bytes: &[u8], mut offset: usize) -> usize {
    while offset < bytes.len() && offset > 0 && bytes[offset] & 0xC0 == 0x80 {
        offset += 1;
    }
    offset.min(bytes.len())
}

fn previous_utf8_boundary(bytes: &[u8], mut offset: usize) -> usize {
    offset = offset.min(bytes.len());
    while offset > 0 && offset < bytes.len() && bytes[offset] & 0xC0 == 0x80 {
        offset -= 1;
    }
    offset
}

/// Pixel-width measuring function for `layout`'s own book font and size,
/// used to word-wrap TXT/EPUB pages against the real Reader body viewport
/// instead of a fixed character budget.
fn reader_layout_measure(layout: &ReaderLayout) -> impl Fn(&str) -> i32 {
    let style = crate::app::reader_typography::reader_body_style(
        layout.book_font,
        layout.font_size,
        ReadingTheme::Classic,
    );
    move |text: &str| style.text_width(text)
}

fn read_txt_page(
    book: &ReaderBook,
    encoding: TextEncoding,
    layout: ReaderLayout,
    byte_offset: u64,
    page_index: usize,
) -> Result<ReaderCachedPage, String> {
    let mut file = File::open(&book.path).map_err(|error| format!("Open failed: {error}"))?;
    file.seek(SeekFrom::Start(byte_offset))
        .map_err(|error| format!("Seek failed: {error}"))?;
    let width_of = reader_layout_measure(&layout);
    let mut bytes = Vec::new();
    let mut window_len = READER_PAGE_INITIAL_READ_BYTES.min(READER_PAGE_READ_BYTES);
    loop {
        // Extend the same buffer from where the previous, smaller window
        // stopped instead of re-reading it.
        let filled = bytes.len();
        bytes.resize(window_len, 0);
        let read = read_until_full_or_eof(&mut file, &mut bytes[filled..])
            .map_err(|error| format!("Read failed: {error}"))?;
        bytes.truncate(filled + read);
        let input_is_final = bytes.len() < window_len || window_len >= READER_PAGE_READ_BYTES;
        let skip_bom = byte_offset == 0 && bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let base = byte_offset + if skip_bom { 3 } else { 0 };
        let decoded = decode_with_offsets(&bytes[if skip_bom { 3 } else { 0 }..], encoding, base);
        let normalized = normalize_decoded(&decoded);
        if let Some((lines, consumed)) =
            paginate_decoded_window(&normalized, layout, &[], &width_of, input_is_final)
        {
            let next_byte_offset = consumed.max(base).min(book.size_bytes);
            return Ok(ReaderCachedPage {
                page_index,
                byte_offset,
                next_byte_offset,
                lines,
            });
        }
        window_len = window_len.saturating_mul(2).min(READER_PAGE_READ_BYTES);
    }
}

/// Fill `buffer` from `file`, stopping early only at end of file. Returns
/// the number of bytes read, so a short count means EOF was reached.
fn read_until_full_or_eof(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

fn decode_with_offsets(bytes: &[u8], encoding: TextEncoding, base: u64) -> Vec<(char, u64)> {
    match encoding {
        TextEncoding::Windows1252 => bytes
            .iter()
            .enumerate()
            .map(|(index, byte)| (decode_windows_1252(*byte), base + index as u64 + 1))
            .collect(),
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => {
            let valid = match std::str::from_utf8(bytes) {
                Ok(text) => text,
                Err(error) => {
                    // A split character at the very end of a bounded read
                    // window is expected; invalid bytes earlier than that
                    // mean this window is not the UTF-8 text it should be.
                    if error.valid_up_to() + 4 < bytes.len() {
                        log::warn!(
                            "rustmix-wave=reader-invalid-utf8 base={base} valid-up-to={} len={}",
                            error.valid_up_to(),
                            bytes.len()
                        );
                    }
                    std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or("")
                }
            };
            valid
                .char_indices()
                .map(|(index, character)| {
                    (character, base + index as u64 + character.len_utf8() as u64)
                })
                .collect()
        }
    }
}

fn normalize_decoded(decoded: &[(char, u64)]) -> Vec<(char, u64)> {
    let mut normalized = Vec::new();
    let mut unmapped = 0_usize;
    let mut unmapped_samples: Vec<char> = Vec::new();
    let mut first_unmapped_offset: Option<u64> = None;
    for (index, (character, next_offset)) in decoded.iter().copied().enumerate() {
        if character == '_' {
            let previous = index
                .checked_sub(1)
                .and_then(|value| decoded.get(value))
                .map(|value| value.0);
            let next = decoded.get(index + 1).map(|value| value.0);
            let word_internal =
                previous.is_some_and(is_word_character) && next.is_some_and(is_word_character);
            let repeated_separator = previous == Some('_') || next == Some('_');

            // Project Gutenberg TXT files often wrap emphasis across multiple
            // source lines: `_first line ... last line_`. Remove each bounded
            // delimiter independently so closing markers after punctuation do
            // not leak into rendered pages. Keep filename-style word_internal
            // underscores and repeated separator rows intact.
            if !word_internal && !repeated_separator {
                continue;
            }
        }
        if push_normalized_character(&mut normalized, character, next_offset) {
            unmapped += 1;
            if unmapped_samples.len() < 8 && !unmapped_samples.contains(&character) {
                unmapped_samples.push(character);
            }
            first_unmapped_offset.get_or_insert(next_offset);
        }
    }
    // Diagnostic for field reports of whole pages turning into `?`: a real
    // book only ever has the odd unmapped symbol, so a burst of them means
    // the bytes being paginated were not the text they should have been.
    if unmapped >= READER_UNMAPPED_CHARACTER_REPORT_THRESHOLD {
        let codepoints: Vec<String> = unmapped_samples
            .iter()
            .map(|character| format!("U+{:04X}", u32::from(*character)))
            .collect();
        log::warn!(
            "rustmix-wave=reader-unmapped-text count={unmapped} of={} first-offset={} samples={}",
            decoded.len(),
            first_unmapped_offset.unwrap_or(0),
            codepoints.join(",")
        );
    }
    normalized
}

/// Unmapped characters in one normalized window that trigger the
/// `reader-unmapped-text` diagnostic log.
const READER_UNMAPPED_CHARACTER_REPORT_THRESHOLD: usize = 16;

/// Returns `true` when `character` had no mapping and was replaced by the
/// generic `?` fallback, so callers can report where unrenderable text
/// came from.
fn push_normalized_character(
    output: &mut Vec<(char, u64)>,
    character: char,
    next_offset: u64,
) -> bool {
    let replacement: &str = match character {
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{00AB}' | '\u{00BB}' => "\"",
        '\u{2018}' | '\u{2019}' | '\u{201A}' => "'",
        '\u{2014}' => "--",
        '\u{2013}' => "-",
        '\u{2026}' => "...",
        '\u{00A0}' => " ",
        // Italian accents have real glyphs in the reader-only bitmap fonts
        // (see BitmapFont::extra), so they pass through unchanged instead of
        // being collapsed to their unaccented ASCII base letter.
        'à' => "à",
        'è' => "è",
        'é' => "é",
        'ì' => "ì",
        'ò' => "ò",
        'ù' => "ù",
        'À' => "À",
        'È' => "È",
        'É' => "É",
        'Ì' => "Ì",
        'Ò' => "Ò",
        'Ù' => "Ù",
        'ê' | 'ë' | 'Ê' | 'Ë' => "e",
        'á' | 'â' | 'ä' | 'Á' | 'Â' | 'Ä' => "a",
        'ç' | 'Ç' => "c",
        'ï' | 'î' | 'í' | 'Ï' | 'Î' | 'Í' => "i",
        'ô' | 'ö' | 'ó' | 'Ô' | 'Ö' | 'Ó' => "o",
        'û' | 'ü' | 'ú' | 'Û' | 'Ü' | 'Ú' => "u",
        'ñ' | 'Ñ' => "n",
        // Invisible formatting characters common in EPUB XHTML: soft
        // hyphens, zero-width spaces/joiners, bidi marks, word joiner and a
        // stray BOM, plus combining diacritics from NFD-normalized text.
        // They carry no glyph of their own, so each is dropped instead of
        // surfacing as a `?` in the middle of a word.
        '\u{00AD}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{FEFF}'
        | '\u{0300}'..='\u{036F}' => "",
        '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => " ",
        '\u{2028}' | '\u{2029}' => "\n",
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2212}' | '\u{2043}' => "-",
        '\u{2015}' => "--",
        '\u{201B}' | '\u{2032}' | '\u{2039}' | '\u{203A}' | '\u{00B4}' => "'",
        '\u{201F}' | '\u{2033}' => "\"",
        '\u{2022}' | '\u{2023}' | '\u{2219}' | '\u{25CF}' | '\u{25AA}' => "*",
        '\u{00B7}' | '\u{2027}' => ".",
        '\u{2020}' => "+",
        '\u{2021}' => "++",
        '\u{2042}' => "* * *",
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB05}' | '\u{FB06}' => "st",
        'æ' => "ae",
        'Æ' => "AE",
        'œ' => "oe",
        'Œ' => "OE",
        'ß' => "ss",
        '×' => "x",
        '÷' => "/",
        '©' => "(c)",
        '®' => "(R)",
        '™' => "TM",
        '€' => "EUR",
        '£' => "GBP",
        '½' => "1/2",
        '¼' => "1/4",
        '¾' => "3/4",
        '¿' => "?",
        '¡' => "!",
        'ã' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'Ã' | 'Å' | 'Ā' | 'Ă' | 'Ą' => "A",
        'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'Ć' | 'Č' | 'Ĉ' | 'Ċ' => "C",
        'ď' | 'đ' => "d",
        'Ď' | 'Đ' => "D",
        'ē' | 'ė' | 'ę' | 'ě' => "e",
        'Ē' | 'Ė' | 'Ę' | 'Ě' => "E",
        'ğ' | 'ģ' => "g",
        'Ğ' | 'Ģ' => "G",
        'ī' | 'į' | 'ı' => "i",
        'Ī' | 'Į' | 'İ' => "I",
        'ł' | 'ľ' | 'ĺ' => "l",
        'Ł' | 'Ľ' | 'Ĺ' => "L",
        'ń' | 'ň' | 'ņ' => "n",
        'Ń' | 'Ň' | 'Ņ' => "N",
        'õ' | 'ø' | 'ō' | 'ő' => "o",
        'Õ' | 'Ø' | 'Ō' | 'Ő' => "O",
        'ŕ' | 'ř' => "r",
        'Ŕ' | 'Ř' => "R",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'Ś' | 'Š' | 'Ş' | 'Ș' => "S",
        'ť' | 'ţ' | 'ț' => "t",
        'Ť' | 'Ţ' | 'Ț' => "T",
        'ū' | 'ů' | 'ű' | 'ų' => "u",
        'Ū' | 'Ů' | 'Ű' | 'Ų' => "U",
        'ý' | 'ÿ' => "y",
        'Ý' | 'Ÿ' => "Y",
        'ź' | 'ż' | 'ž' => "z",
        'Ź' | 'Ż' | 'Ž' => "Z",
        // One EPUB inline image occupies this offset. It must reach
        // `paginate_decoded` unchanged -- that is what now recognizes it and
        // reserves page space for it (see `EPUB_IMAGE_SENTINEL`'s own doc
        // comment) -- rather than being folded away here like every other
        // unsupported codepoint.
        EPUB_IMAGE_SENTINEL => {
            output.push((character, next_offset));
            return false;
        }
        value
            if value == '\n'
                || value == '\r'
                || value == '\t'
                || value.is_ascii_graphic()
                || value == ' ' =>
        {
            output.push((value, next_offset));
            return false;
        }
        _ => {
            output.push(('?', next_offset));
            return true;
        }
    };
    for value in replacement.chars() {
        output.push((value, next_offset));
    }
    false
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric()
}

/// Line being assembled by [`paginate_decoded`], with its measured width
/// kept alongside the text so [`place_word`] does not re-measure the whole
/// line for every word it appends (quadratic in the words per line).
/// Relies on `width_of` being additive over concatenation, which holds for
/// the Reader's bitmap strikes: a string's width is the plain sum of its
/// glyph advances, with no kerning between neighbors.
#[derive(Default)]
struct PendingLine {
    text: String,
    width: i32,
}

impl PendingLine {
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn push_str(&mut self, text: &str, width: i32) {
        self.text.push_str(text);
        self.width += width;
    }

    fn take(&mut self) -> String {
        self.width = 0;
        core::mem::take(&mut self.text)
    }
}

/// Outcome of [`place_word`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WordPlacement {
    /// The whole word is on the page.
    Placed,
    /// The page filled before any of the word was shown: the next page
    /// re-reads the word whole, so it is never split across the boundary.
    Deferred,
    /// The word was wider than one line and was being hard-broken when the
    /// page filled, after its first `chars` characters were already shown.
    /// The next page must resume right after them. Resuming at the word's
    /// start instead (the old behavior) repeated those characters on the
    /// next page, and a word longer than a whole page never advanced at
    /// all, stalling pagination for the rest of the book.
    PageFilledMidWord { chars: usize },
}

/// Move `word` onto `line`, wrapping to a new line first if it would not
/// fit, and hard-breaking only when the word alone is wider than one line
/// (no narrower unit exists to wrap on). See [`WordPlacement`] for what the
/// caller must do when the page's line budget runs out first.
fn place_word(
    lines: &mut Vec<ReaderPageLine>,
    line: &mut PendingLine,
    word: &mut String,
    layout: &ReaderLayout,
    width_of: &impl Fn(&str) -> i32,
) -> WordPlacement {
    if word.is_empty() {
        return WordPlacement::Placed;
    }
    if lines.len() >= layout.lines_per_page {
        return WordPlacement::Deferred;
    }
    debug_assert_eq!(
        line.width,
        width_of(&line.text),
        "PendingLine width drifted"
    );
    let space_width = width_of(" ");
    let mut word_width = width_of(word);
    let fits =
        line.is_empty() || line.width + space_width + word_width <= layout.available_width_px;
    if !line.is_empty() && !fits {
        lines.push(ReaderPageLine {
            text: line.take(),
            paragraph_end: false,
            image: None,
        });
        if lines.len() >= layout.lines_per_page {
            return WordPlacement::Deferred;
        }
    }
    let mut shown_chars = 0;
    while word_width > layout.available_width_px {
        let split_at = pixel_split_point(word, layout.available_width_px, width_of);
        let (head, tail) = word.split_at(split_at);
        if !line.is_empty() {
            line.push_str(" ", space_width);
        }
        line.push_str(head, width_of(head));
        shown_chars += head.chars().count();
        let tail = tail.to_string();
        lines.push(ReaderPageLine {
            text: line.take(),
            paragraph_end: false,
            image: None,
        });
        *word = tail;
        word_width = width_of(word);
        if lines.len() >= layout.lines_per_page {
            return if word.is_empty() {
                WordPlacement::Placed
            } else {
                WordPlacement::PageFilledMidWord { chars: shown_chars }
            };
        }
    }
    if !line.is_empty() {
        line.push_str(" ", space_width);
    }
    line.push_str(word, word_width);
    word.clear();
    WordPlacement::Placed
}

/// Resume offset for a page that filled after showing the first `chars`
/// characters of a hard-broken word whose characters end at `char_ends`.
/// Normalization can expand one source character into several (`ﬁ` into
/// `fi`, `…` into `...`), all sharing one end offset; when the break lands
/// inside such a group, back off to the previous whole source character so
/// the group's remaining glyphs are repeated on the next page rather than
/// lost. `None` only when no character was shown.
fn mid_word_resume_offset(char_ends: &[u64], chars: usize) -> Option<u64> {
    let shown = chars.min(char_ends.len());
    (1..=shown)
        .rev()
        .find(|&count| count == char_ends.len() || char_ends[count] != char_ends[count - 1])
        .or((shown > 0).then_some(shown))
        .map(|count| char_ends[count - 1])
}

/// Byte index of the longest prefix of `word` whose measured width fits
/// within `available` pixels, always including at least one character (the
/// caller has no narrower unit to wrap on than a single glyph).
fn pixel_split_point(word: &str, available: i32, width_of: &impl Fn(&str) -> i32) -> usize {
    let mut consumed_width = 0;
    let mut end = 0;
    for (index, character) in word.char_indices() {
        let mut buffer = [0_u8; 4];
        let glyph = character.encode_utf8(&mut buffer);
        let glyph_width = width_of(glyph);
        if end > 0 && consumed_width + glyph_width > available {
            break;
        }
        consumed_width += glyph_width;
        end = index + character.len_utf8();
    }
    end
}

/// Paginate decoded characters into word-wrapped lines. Lines only ever
/// break at whitespace; a single word wider than one line is the sole
/// exception, and even then it hard-breaks whole lines at a time rather than
/// leaving a stray character behind.
#[cfg(test)]
fn paginate_decoded(
    decoded: &[(char, u64)],
    layout: ReaderLayout,
    images: &[EpubImage],
    width_of: &impl Fn(&str) -> i32,
) -> (Vec<ReaderPageLine>, u64) {
    paginate_decoded_window(decoded, layout, images, width_of, true)
        .expect("final input always produces a page")
}

/// [`paginate_decoded`] over a window that may be a prefix of the real
/// input. With `input_is_final == false` it returns `None` whenever the
/// result could depend on characters past the window's end -- the page did
/// not fill inside it, or an image's "nothing but whitespace follows"
/// lookahead ran off its end -- so the caller retries with a larger window.
/// Any `Some` result is therefore exactly what the full input would give.
fn paginate_decoded_window(
    decoded: &[(char, u64)],
    layout: ReaderLayout,
    images: &[EpubImage],
    width_of: &impl Fn(&str) -> i32,
    input_is_final: bool,
) -> Option<(Vec<ReaderPageLine>, u64)> {
    let mut lines = Vec::new();
    let mut line = PendingLine::default();
    let mut word = String::new();
    // End offset of each character in `word`, for resuming mid-word (see
    // `WordPlacement::PageFilledMidWord`).
    let mut word_char_ends: Vec<u64> = Vec::new();
    let line_step = reader_line_step(&layout);
    let mut consumed = decoded
        .first()
        .map_or(0, |(_, offset)| offset.saturating_sub(1));
    // Whether the loop below stopped because the page's line budget was
    // reached, as opposed to running out of decoded input. Only the latter
    // means `decoded` holds this call's true end (chapter end, or a bounded
    // read window's end): only then is it correct to flush a final pending
    // word and advance `consumed` to it.
    let mut page_full = false;

    for (character, next_offset) in decoded.iter().copied() {
        if lines.len() >= layout.lines_per_page {
            page_full = true;
            break;
        }
        if character == EPUB_IMAGE_SENTINEL {
            // Flush any pending word first, as its own atomic step -- this
            // can defer exactly like it already can before whitespace/'\n'.
            match place_word(&mut lines, &mut line, &mut word, &layout, width_of) {
                WordPlacement::Placed => word_char_ends.clear(),
                WordPlacement::Deferred => {
                    page_full = true;
                    break;
                }
                WordPlacement::PageFilledMidWord { chars } => {
                    if let Some(offset) = mid_word_resume_offset(&word_char_ends, chars) {
                        consumed = offset;
                    }
                    page_full = true;
                    break;
                }
            }
            let sentinel_offset =
                next_offset.saturating_sub(EPUB_IMAGE_SENTINEL.len_utf8() as u64);
            let Some(matched_image) = images
                .iter()
                .find(|image| image.text_offset == sentinel_offset)
            else {
                // No matching table entry (should not happen -- the
                // sentinel and its `EpubImage` are always written together
                // in `open_epub`'s spine loop) -- skip it as if the tag
                // were never there instead of risking a panic on
                // malformed/stale input.
                consumed = next_offset;
                continue;
            };
            let standalone = lines.is_empty()
                && line.is_empty()
                && decoded
                    .iter()
                    .skip_while(|(_, offset)| *offset <= next_offset)
                    .all(|(value, _)| value.is_whitespace());
            if standalone && !input_is_final {
                // Only whitespace up to the window's end: text right after
                // it would make this an ordinary inline image instead.
                return None;
            }
            let (slot_span, box_width, box_height) =
                inline_image_slots(matched_image, &layout, line_step, standalone);
            // The image is one atomic unit: itself plus, if `line` already
            // holds pending text, the line-slot that text needs first --
            // same as `place_word`'s own hard-break already accounts for
            // its pending `line` before wrapping. `consumed` must not move
            // past the sentinel unless the whole unit fits and is pushed;
            // otherwise the image (not just its trailing text) would be
            // silently dropped rather than deferred to the next page.
            let pending_line_slots = usize::from(!line.is_empty());
            if lines.len() + pending_line_slots + slot_span > layout.lines_per_page {
                page_full = true;
                break;
            }
            if !line.is_empty() {
                lines.push(ReaderPageLine {
                    text: line.take(),
                    paragraph_end: false,
                    image: None,
                });
            }
            lines.push(ReaderPageLine {
                text: String::new(),
                paragraph_end: false,
                image: Some(ReaderPageImage {
                    href: matched_image.href.clone(),
                    alt: matched_image.alt.clone(),
                    slot_span,
                    box_width,
                    box_height,
                }),
            });
            for _ in 1..slot_span {
                lines.push(ReaderPageLine {
                    text: String::new(),
                    paragraph_end: false,
                    image: None,
                });
            }
            consumed = next_offset;
            continue;
        }
        let character = match character {
            '\r' => continue,
            '\n' => {
                match place_word(&mut lines, &mut line, &mut word, &layout, width_of) {
                    WordPlacement::Placed => word_char_ends.clear(),
                    WordPlacement::Deferred => {
                        page_full = true;
                        break;
                    }
                    WordPlacement::PageFilledMidWord { chars } => {
                        if let Some(offset) = mid_word_resume_offset(&word_char_ends, chars) {
                            consumed = offset;
                        }
                        page_full = true;
                        break;
                    }
                }
                lines.push(ReaderPageLine {
                    text: line.take(),
                    paragraph_end: true,
                    image: None,
                });
                consumed = next_offset;
                if lines.len() >= layout.lines_per_page {
                    page_full = true;
                    break;
                }
                continue;
            }
            value if value.is_control() => ' ',
            value => value,
        };
        if character.is_whitespace() {
            match place_word(&mut lines, &mut line, &mut word, &layout, width_of) {
                WordPlacement::Placed => word_char_ends.clear(),
                WordPlacement::Deferred => {
                    page_full = true;
                    break;
                }
                WordPlacement::PageFilledMidWord { chars } => {
                    if let Some(offset) = mid_word_resume_offset(&word_char_ends, chars) {
                        consumed = offset;
                    }
                    page_full = true;
                    break;
                }
            }
            consumed = next_offset;
        } else {
            word.push(character);
            word_char_ends.push(next_offset);
        }
    }

    // When the page filled up, `word` is either empty (the break landed on a
    // clean word boundary), holds a word deferred to the next page, or holds
    // the unshown tail of a hard-broken word (`consumed` then already points
    // right after its last shown character). Either way
    // `consumed` already reflects exactly what this page rendered, and must
    // not be pulled forward to the end of the decoded window here: doing so
    // silently skipped every byte between the true page end and the end of
    // the up-to-16 KB read window on every page that happened to break on a
    // clean word boundary, discarding whole chunks of chapter text
    // (sometimes mid-word once the next page resumed past the gap).
    if !page_full && !input_is_final {
        // The page may continue past this window: only the caller can
        // supply the rest.
        return None;
    }
    if !page_full {
        if let Some((_, last_offset)) = decoded.last() {
            match place_word(&mut lines, &mut line, &mut word, &layout, width_of) {
                WordPlacement::Placed => consumed = consumed.max(*last_offset),
                WordPlacement::Deferred => {}
                WordPlacement::PageFilledMidWord { chars } => {
                    if let Some(offset) = mid_word_resume_offset(&word_char_ends, chars) {
                        consumed = offset;
                    }
                }
            }
        }
    }
    if lines.len() < layout.lines_per_page && (!line.is_empty() || lines.is_empty()) {
        lines.push(ReaderPageLine {
            text: line.text,
            paragraph_end: true,
            image: None,
        });
    }
    Some((lines, consumed))
}

fn decode_windows_1252(byte: u8) -> char {
    match byte {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        value => char::from(value),
    }
}

fn book_fingerprint(book: &ReaderBook, layout: ReaderLayout) -> u64 {
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
    feed(&mut hash, book.format.marker().as_bytes());
    feed(&mut hash, &layout.lines_per_page.to_le_bytes());
    feed(&mut hash, &layout.available_width_px.to_le_bytes());
    feed(&mut hash, layout.orientation.marker().as_bytes());
    feed(&mut hash, layout.font_size.marker().as_bytes());
    feed(&mut hash, layout.book_font.marker().as_bytes());
    feed(&mut hash, layout.paragraph_alignment.marker().as_bytes());
    // Fed only when on, so every existing (non-full-screen) page cache keeps
    // its fingerprint and is not needlessly rebuilt.
    if layout.full_screen {
        feed(&mut hash, b"full-screen");
    }
    feed(&mut hash, READER_CACHE_VERSION.as_bytes());
    hash
}

/// Fingerprint used by the flattened-EPUB-text cache. Unlike [`book_fingerprint`]
/// this intentionally excludes Reader layout: reflowed EPUB text and its TOC
/// are layout-independent, so a font or orientation change must not invalidate
/// the expensive ZIP/DEFLATE/HTML-flatten work already cached for this book.
fn epub_document_fingerprint(book: &ReaderBook) -> u64 {
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
    feed(&mut hash, book.format.marker().as_bytes());
    feed(&mut hash, EPUB_DOCUMENT_CACHE_VERSION.as_bytes());
    hash
}

/// Serialize one parsed [`EpubDocument`] as a text header (version, fingerprint,
/// title, TOC and chapter records) followed by a `text_start` marker line and
/// the raw flattened UTF-8 text, unescaped, so the header never has to
/// duplicate up to [`EPUB_REFLOW_TEXT_LIMIT`] bytes. Returns the assembled
/// content together with the body's byte offset within it, so the caller can
/// hand the document straight to [`EpubDocument::into_on_disk`] once the
/// write lands, instead of re-parsing its own output to find that offset.
/// Errors if `document`'s text is not resident (a document already backed by
/// its own `.EPX` file has nothing new to persist).
fn serialize_epub_document_cache(
    document: &EpubDocument,
    fingerprint: u64,
) -> Result<(String, u64), String> {
    let text = document
        .resident_text()
        .ok_or_else(|| "EPUB document text is not resident".to_string())?;
    let mut output = format!(
        "version={EPUB_DOCUMENT_CACHE_VERSION}\nfingerprint={fingerprint:016X}\ntitle={}\nspine_count={}\n",
        escape_field(&document.title),
        document.spine_count,
    );
    for entry in &document.toc {
        output.push_str(&format!(
            "toc={}\t{}\t{}\n",
            escape_field(&entry.label),
            entry.text_offset,
            entry.spine_index
        ));
    }
    for chapter in &document.chapters {
        output.push_str(&format!(
            "chapter={}\t{}\t{}\t{}\t{}\n",
            chapter.number,
            escape_field(&chapter.label),
            chapter.text_offset,
            chapter.text_end_offset,
            chapter.spine_index
        ));
    }
    for image in &document.images {
        output.push_str(&format!(
            "image={}\t{}\t{}\t{}\t{}\t{}\n",
            escape_field(&image.href),
            escape_field(&image.alt),
            image.text_offset,
            image.spine_index,
            image.width,
            image.height
        ));
    }
    output.push_str(&format!("text_bytes={}\ntext_start\n", text.len()));
    let body_offset = output.len() as u64;
    output.push_str(text);
    Ok((output, body_offset))
}

/// Parse the bounded header of one `.EPX` cache file — everything up to and
/// including the `text_start` marker line, as read by
/// [`read_epub_document_cache_header`] — into an [`EpubDocument`] backed by
/// `path` itself. Unlike the old whole-file parser this never materializes
/// the (potentially large) flattened body as a `String`: `path`'s total size
/// is checked against the declared `text_bytes` via [`fs::metadata`] instead,
/// and pagination later reads windows of the body straight off disk through
/// [`EpubDocument::text_window`].
fn parse_epub_document_cache(
    header: &str,
    book: &ReaderBook,
    path: &Path,
) -> Result<EpubDocument, String> {
    let mut version = None;
    let mut fingerprint = None;
    let mut title = None;
    let mut spine_count = None;
    let mut text_bytes = None;
    let mut toc = Vec::new();
    let mut chapters = Vec::new();
    let mut images = Vec::new();
    let mut saw_text_start = false;
    for line in header.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if trimmed == "text_start" {
            saw_text_start = true;
            break;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            match key {
                "version" => version = Some(value.to_string()),
                "fingerprint" => fingerprint = u64::from_str_radix(value, 16).ok(),
                "title" => title = Some(unescape_field(value)?),
                "spine_count" => spine_count = value.parse().ok(),
                "text_bytes" => text_bytes = value.parse().ok(),
                "toc" if toc.len() < EPUB_TOC_LIMIT => {
                    let fields = split_escaped_tabs(value)?;
                    if fields.len() != 3 {
                        return Err("invalid EPUB cache TOC record".into());
                    }
                    toc.push(EpubTocEntry {
                        label: fields[0].clone(),
                        text_offset: fields[1]
                            .parse()
                            .map_err(|_| "invalid EPUB cache TOC offset".to_string())?,
                        spine_index: fields[2]
                            .parse()
                            .map_err(|_| "invalid EPUB cache TOC spine index".to_string())?,
                    });
                }
                "chapter" if chapters.len() < EPUB_SPINE_LIMIT => {
                    let fields = split_escaped_tabs(value)?;
                    if fields.len() != 5 {
                        return Err("invalid EPUB cache chapter record".into());
                    }
                    chapters.push(EpubChapter {
                        number: fields[0]
                            .parse()
                            .map_err(|_| "invalid EPUB cache chapter number".to_string())?,
                        label: fields[1].clone(),
                        text_offset: fields[2]
                            .parse()
                            .map_err(|_| "invalid EPUB cache chapter offset".to_string())?,
                        text_end_offset: fields[3]
                            .parse()
                            .map_err(|_| "invalid EPUB cache chapter end offset".to_string())?,
                        spine_index: fields[4]
                            .parse()
                            .map_err(|_| "invalid EPUB cache chapter spine index".to_string())?,
                    });
                }
                "image" if images.len() < EPUB_IMAGE_LIMIT => {
                    let fields = split_escaped_tabs(value)?;
                    if fields.len() != 6 {
                        return Err("invalid EPUB cache image record".into());
                    }
                    images.push(EpubImage {
                        href: fields[0].clone(),
                        alt: fields[1].clone(),
                        text_offset: fields[2]
                            .parse()
                            .map_err(|_| "invalid EPUB cache image offset".to_string())?,
                        spine_index: fields[3]
                            .parse()
                            .map_err(|_| "invalid EPUB cache image spine index".to_string())?,
                        width: fields[4]
                            .parse()
                            .map_err(|_| "invalid EPUB cache image width".to_string())?,
                        height: fields[5]
                            .parse()
                            .map_err(|_| "invalid EPUB cache image height".to_string())?,
                    });
                }
                _ => {}
            }
        }
    }
    if !saw_text_start {
        return Err("missing EPUB cache text marker".into());
    }
    if version.as_deref() != Some(EPUB_DOCUMENT_CACHE_VERSION) {
        return Err("unsupported EPUB cache version".into());
    }
    let fingerprint = fingerprint.ok_or_else(|| "missing EPUB cache fingerprint".to_string())?;
    if fingerprint != epub_document_fingerprint(book) {
        return Err("EPUB cache fingerprint mismatch".into());
    }
    let text_bytes: u64 = text_bytes.ok_or_else(|| "missing EPUB cache text length".to_string())?;
    if text_bytes > EPUB_REFLOW_TEXT_LIMIT as u64 {
        return Err("EPUB cache text exceeds byte limit".into());
    }
    let body_offset = header.len() as u64;
    let file_len = fs::metadata(path)
        .map_err(|error| format!("EPUB cache stat failed: {error}"))?
        .len();
    if body_offset.checked_add(text_bytes) != Some(file_len) {
        return Err("EPUB cache text length mismatch".into());
    }
    if toc.is_empty() && chapters.is_empty() {
        return Err("EPUB cache produced no chapters".into());
    }
    if chapters.iter().any(|chapter| {
        chapter.text_offset > chapter.text_end_offset || chapter.text_end_offset > text_bytes
    }) {
        return Err("EPUB cache chapter offset out of range".into());
    }
    if toc.iter().any(|entry| entry.text_offset > text_bytes) {
        return Err("EPUB cache TOC offset out of range".into());
    }
    if images.iter().any(|image| image.text_offset > text_bytes) {
        return Err("EPUB cache image offset out of range".into());
    }
    Ok(EpubDocument::from_cache_body(
        title.ok_or_else(|| "missing EPUB cache title".to_string())?,
        toc,
        chapters,
        images,
        spine_count.ok_or_else(|| "missing EPUB cache spine count".to_string())?,
        path.to_path_buf(),
        body_offset,
        text_bytes,
    ))
}

/// Bounded prefix read for one `.EPX` cache file, big enough for any header
/// this format can produce (`EPUB_TOC_LIMIT` + `EPUB_SPINE_LIMIT` records,
/// generously sized) without ever reading the flattened body that follows.
const EPUB_CACHE_HEADER_MAX_BYTES: usize = 256 * 1024;

/// Read granularity for [`read_epub_cache_header`]. Real headers are a few KB,
/// so this usually finds the marker in the first one or two reads.
const EPUB_CACHE_HEADER_READ_CHUNK_BYTES: usize = 4 * 1024;

const EPUB_CACHE_TEXT_MARKER: &[u8] = b"text_start\n";

/// Read and return just the header portion of an `.EPX` cache file — from the
/// start of the file through the end of its `text_start\n` marker line — as a
/// `String`. Cutting exactly after that line is always a valid UTF-8 boundary
/// (`\n` is never part of a multi-byte sequence), regardless of what non-ASCII
/// text the title/TOC/chapter labels contain.
///
/// Reads in `EPUB_CACHE_HEADER_READ_CHUNK_BYTES` steps and stops at the marker.
/// The whole flattened book follows the header in the same file, so filling
/// the full `EPUB_CACHE_HEADER_MAX_BYTES` bound before searching (as this
/// used to) read 256 KiB of body text off the SD card on every cached open:
/// ~440 ms measured, on every EPUB resume, open and background warm-up, plus
/// a 256 KiB allocation on an already fragmented heap.
fn read_epub_cache_header(path: &Path) -> Result<String, String> {
    let mut span = crate::boot_profile::span("epub-cache-header-read");
    let mut file = File::open(path).map_err(|error| format!("EPUB cache open failed: {error}"))?;
    let mut buffer = Vec::with_capacity(EPUB_CACHE_HEADER_READ_CHUNK_BYTES);
    let mut chunk = vec![0_u8; EPUB_CACHE_HEADER_READ_CHUNK_BYTES];
    while buffer.len() < EPUB_CACHE_HEADER_MAX_BYTES {
        let want = chunk
            .len()
            .min(EPUB_CACHE_HEADER_MAX_BYTES - buffer.len());
        let read = file
            .read(&mut chunk[..want])
            .map_err(|error| format!("EPUB cache read failed: {error}"))?;
        if read == 0 {
            break;
        }
        // The marker may straddle the previous read, so rescan its tail too.
        let search_from = buffer
            .len()
            .saturating_sub(EPUB_CACHE_TEXT_MARKER.len() - 1);
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(offset) = buffer[search_from..]
            .windows(EPUB_CACHE_TEXT_MARKER.len())
            .position(|window| window == EPUB_CACHE_TEXT_MARKER)
        {
            buffer.truncate(search_from + offset + EPUB_CACHE_TEXT_MARKER.len());
            if crate::boot_profile::is_active() {
                span.detail(format_args!("header-bytes={}", buffer.len()));
            }
            return String::from_utf8(buffer)
                .map_err(|_| "EPUB cache header is not valid UTF-8".to_string());
        }
    }
    Err("EPUB cache header exceeds bound or is missing text marker".to_string())
}

fn load_epub_document_cache(
    path: &Path,
    book: &ReaderBook,
) -> Result<Option<EpubDocument>, String> {
    let backup = with_extension(path, "BAK");
    let mut errors = Vec::new();
    for candidate in [path.to_path_buf(), backup] {
        if !candidate.exists() {
            continue;
        }
        match read_epub_cache_header(&candidate) {
            Ok(header) => match parse_epub_document_cache(&header, book, &candidate) {
                Ok(document) => return Ok(Some(document)),
                Err(error) => errors.push(format!("{}: {error}", candidate.display())),
            },
            Err(error) => errors.push(format!("{}: {error}", candidate.display())),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(errors.join("; "))
    }
}

/// Serialize one EPUB page-offset index (see [`ReaderEpubChapterPages`]) as one
/// `chapter=` record per chapter, with that chapter's page offsets packed as a
/// comma-separated list (bounded by [`READER_EPUB_PAGE_ANCHOR_LIMIT`] total).
fn serialize_epub_page_index_cache(pages: &[ReaderEpubChapterPages], fingerprint: u64) -> String {
    let mut output =
        format!("version={EPUB_PAGE_INDEX_CACHE_VERSION}\nfingerprint={fingerprint:016X}\n");
    for chapter in pages {
        let offsets = chapter
            .page_offsets
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        output.push_str(&format!(
            "chapter={}\t{}\t{}\t{offsets}\n",
            chapter.chapter_number, chapter.text_offset, chapter.text_end_offset
        ));
    }
    output
}

fn parse_epub_page_index_cache(
    text: &str,
    book: &ReaderBook,
    layout: ReaderLayout,
) -> Result<Vec<ReaderEpubChapterPages>, String> {
    let mut version = None;
    let mut fingerprint = None;
    // Pre-sized from a cheap byte-scan instead of growing via repeated
    // `push()`: on a large EPUB (dozens of chapters), letting this and each
    // chapter's `page_offsets` below grow one push at a time forces many
    // small reallocations, each abandoning its previous (smaller) buffer --
    // measured in the field to fragment internal SRAM by tens of KiB per
    // book on this hardware, since every one of those reallocations lands
    // under `CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL`'s internal-only threshold.
    let mut chapters = Vec::with_capacity(text.matches("\nchapter=").count() + 1);
    let mut total_pages = 0usize;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "version" => version = Some(value.to_string()),
            "fingerprint" => fingerprint = u64::from_str_radix(value, 16).ok(),
            "chapter" => {
                let fields = split_escaped_tabs(value)?;
                if fields.len() != 4 {
                    return Err("invalid EPUB page cache chapter record".into());
                }
                let mut page_offsets = if fields[3].is_empty() {
                    Vec::new()
                } else {
                    Vec::with_capacity(fields[3].matches(',').count() + 1)
                };
                if !fields[3].is_empty() {
                    for token in fields[3].split(',') {
                        total_pages += 1;
                        if total_pages > READER_EPUB_PAGE_ANCHOR_LIMIT {
                            return Err("EPUB page cache exceeds page anchor limit".into());
                        }
                        page_offsets.push(
                            token
                                .parse()
                                .map_err(|_| "invalid EPUB page cache offset".to_string())?,
                        );
                    }
                }
                chapters.push(ReaderEpubChapterPages {
                    chapter_number: fields[0]
                        .parse()
                        .map_err(|_| "invalid EPUB page cache chapter number".to_string())?,
                    text_offset: fields[1]
                        .parse()
                        .map_err(|_| "invalid EPUB page cache chapter offset".to_string())?,
                    text_end_offset: fields[2]
                        .parse()
                        .map_err(|_| "invalid EPUB page cache chapter end offset".to_string())?,
                    page_offsets,
                });
            }
            _ => {}
        }
    }
    if version.as_deref() != Some(EPUB_PAGE_INDEX_CACHE_VERSION) {
        return Err("unsupported EPUB page cache version".into());
    }
    let fingerprint =
        fingerprint.ok_or_else(|| "missing EPUB page cache fingerprint".to_string())?;
    if fingerprint != book_fingerprint(book, layout) {
        return Err("EPUB page cache fingerprint mismatch".into());
    }
    if chapters.is_empty() {
        return Err("EPUB page cache produced no chapters".into());
    }
    for chapter in &chapters {
        if chapter.page_offsets.is_empty() || chapter.text_offset > chapter.text_end_offset {
            return Err("EPUB page cache chapter is malformed".into());
        }
        if chapter
            .page_offsets
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("EPUB page cache offsets are not strictly increasing".into());
        }
        if chapter.page_offsets[0] != chapter.text_offset {
            return Err("EPUB page cache first offset does not match chapter start".into());
        }
        if *chapter.page_offsets.last().unwrap() >= chapter.text_end_offset {
            return Err("EPUB page cache last offset exceeds chapter end".into());
        }
    }
    Ok(chapters)
}

fn load_epub_page_index_cache(
    path: &Path,
    book: &ReaderBook,
    layout: ReaderLayout,
) -> Result<Option<Vec<ReaderEpubChapterPages>>, String> {
    load_with_backup(path, |text| parse_epub_page_index_cache(text, book, layout))
}

fn serialize_location(location: &ReaderLocation) -> String {
    format!(
        "version={}\npath={}\ntitle={}\nformat={}\nsize={}\nmodified={}\npage={}\noffset={}\nchapter={}\nchapter_page={}\nchapter_pages={}\npercent={}\n",
        READER_PERSISTENCE_VERSION,
        escape_field(&location.path),
        escape_field(&location.title),
        location.format.marker(),
        location.size_bytes,
        location.modified_seconds,
        location.page_index,
        location.byte_offset,
        optional_usize(location.epub_chapter.as_ref().map(|chapter| chapter.chapter_number)),
        optional_usize(location.epub_chapter.as_ref().map(|chapter| chapter.page_number)),
        optional_usize(location.epub_chapter.as_ref().map(|chapter| chapter.page_count)),
        optional_usize(location.reading_percent.map(usize::from))
    )
}

fn parse_location_record(text: &str) -> Result<ReaderLocation, String> {
    let mut version = None;
    let mut path = None;
    let mut title = None;
    let mut format = None;
    let mut size = None;
    let mut modified = None;
    let mut page = None;
    let mut offset = None;
    let mut chapter = None;
    let mut chapter_page = None;
    let mut chapter_pages = None;
    let mut percent = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "version" => version = Some(value),
            "path" => path = Some(unescape_field(value)?),
            "title" => title = Some(unescape_field(value)?),
            "format" => format = BookFormat::parse(value),
            "size" => size = value.parse().ok(),
            "modified" => modified = value.parse().ok(),
            "page" => page = value.parse().ok(),
            "offset" => offset = value.parse().ok(),
            "chapter" => chapter = parse_optional_usize(value),
            "chapter_page" => chapter_page = parse_optional_usize(value),
            "chapter_pages" => chapter_pages = parse_optional_usize(value),
            "percent" => percent = parse_optional_usize(value),
            _ => {}
        }
    }
    if version != Some(READER_PERSISTENCE_VERSION) {
        return Err("unsupported persistence version".into());
    }
    Ok(ReaderLocation {
        path: path.ok_or_else(|| "missing path".to_string())?,
        title: title.ok_or_else(|| "missing title".to_string())?,
        format: format.ok_or_else(|| "missing format".to_string())?,
        size_bytes: size.ok_or_else(|| "missing size".to_string())?,
        modified_seconds: modified.unwrap_or(0),
        page_index: page.ok_or_else(|| "missing page".to_string())?,
        byte_offset: offset.ok_or_else(|| "missing offset".to_string())?,
        epub_chapter: chapter_page_label(chapter, chapter_page, chapter_pages),
        reading_percent: percent.map(|value| value.min(100) as u8),
    })
}

fn serialize_location_list(locations: &[ReaderLocation]) -> String {
    let mut output = format!("version={}\n", READER_PERSISTENCE_VERSION);
    for location in locations {
        output.push_str("entry=");
        output.push_str(&serialize_location_fields(location));
        output.push('\n');
    }
    output
}

fn serialize_location_fields(location: &ReaderLocation) -> String {
    [
        escape_field(&location.path),
        escape_field(&location.title),
        location.format.marker().into(),
        location.size_bytes.to_string(),
        location.modified_seconds.to_string(),
        location.page_index.to_string(),
        location.byte_offset.to_string(),
        optional_usize(
            location
                .epub_chapter
                .as_ref()
                .map(|chapter| chapter.chapter_number),
        ),
        optional_usize(
            location
                .epub_chapter
                .as_ref()
                .map(|chapter| chapter.page_number),
        ),
        optional_usize(
            location
                .epub_chapter
                .as_ref()
                .map(|chapter| chapter.page_count),
        ),
        optional_usize(location.reading_percent.map(usize::from)),
    ]
    .join("\t")
}

fn parse_location_fields(value: &str) -> Result<ReaderLocation, String> {
    let fields = split_escaped_tabs(value)?;
    if fields.len() != 7 && fields.len() != 10 && fields.len() != 11 {
        return Err("invalid location field count".into());
    }
    let epub_chapter = if fields.len() >= 10 {
        chapter_page_label(
            parse_optional_usize(&fields[7]),
            parse_optional_usize(&fields[8]),
            parse_optional_usize(&fields[9]),
        )
    } else {
        None
    };
    let reading_percent = if fields.len() == 11 {
        parse_optional_usize(&fields[10]).map(|value| value.min(100) as u8)
    } else {
        None
    };
    Ok(ReaderLocation {
        path: fields[0].clone(),
        title: fields[1].clone(),
        format: BookFormat::parse(&fields[2]).ok_or_else(|| "invalid format".to_string())?,
        size_bytes: fields[3].parse().map_err(|_| "invalid size".to_string())?,
        modified_seconds: fields[4]
            .parse()
            .map_err(|_| "invalid modified time".to_string())?,
        page_index: fields[5].parse().map_err(|_| "invalid page".to_string())?,
        byte_offset: fields[6]
            .parse()
            .map_err(|_| "invalid offset".to_string())?,
        epub_chapter,
        reading_percent,
    })
}

fn optional_usize(value: Option<usize>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn parse_optional_usize(value: &str) -> Option<usize> {
    if value.is_empty() {
        None
    } else {
        value.parse().ok()
    }
}

fn chapter_page_label(
    chapter_number: Option<usize>,
    page_number: Option<usize>,
    page_count: Option<usize>,
) -> Option<ReaderChapterPageLabel> {
    Some(ReaderChapterPageLabel {
        chapter_number: chapter_number?,
        page_number: page_number?,
        page_count: page_count?,
    })
}

fn parse_location_list(text: &str, limit: usize) -> Result<Vec<ReaderLocation>, String> {
    let mut version = None;
    let mut output = Vec::new();
    let mut skipped = 0_usize;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("version=") {
            version = Some(value);
        } else if let Some(value) = line.strip_prefix("entry=") {
            if output.len() < limit {
                // One damaged record must not cost every other book its
                // saved position: rejecting the whole list here used to
                // leave the caller with an empty list, which the next save
                // then wrote back over the file for good.
                match parse_location_fields(value) {
                    Ok(location) => output.push(location),
                    Err(_) => skipped += 1,
                }
            }
        }
    }
    if version != Some(READER_PERSISTENCE_VERSION) {
        return Err("unsupported persistence version".into());
    }
    if skipped > 0 {
        log::warn!(
            "rustmix-wave=reader-persistence status=skipped-invalid-entries count={skipped} kept={}",
            output.len()
        );
    }
    Ok(output)
}

fn serialize_anchor_cache(cache: &ReaderAnchorCache) -> String {
    let mut output = format!(
        "version={}\nfingerprint={:016X}\nbase_page={}\nindexed_through={}\ncomplete={}\n",
        READER_CACHE_VERSION,
        cache.fingerprint,
        cache.base_page,
        cache.indexed_through,
        cache.complete
    );
    for offset in cache.offsets.iter().take(READER_CACHE_OFFSET_LIMIT) {
        output.push_str(&format!("offset={offset}\n"));
    }
    output
}

fn parse_anchor_cache(
    text: &str,
    book: &ReaderBook,
    layout: ReaderLayout,
) -> Result<ReaderAnchorCache, String> {
    let mut version = None;
    let mut fingerprint = None;
    let mut base_page = None;
    let mut indexed_through: Option<u64> = None;
    let mut complete = None;
    let mut offsets = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "version" => version = Some(value),
            "fingerprint" => fingerprint = u64::from_str_radix(value, 16).ok(),
            "base_page" => base_page = value.parse().ok(),
            "indexed_through" => indexed_through = value.parse().ok(),
            "complete" => complete = value.parse().ok(),
            "offset" if offsets.len() < READER_CACHE_OFFSET_LIMIT => {
                offsets.push(
                    value
                        .parse()
                        .map_err(|_| "invalid cache offset".to_string())?,
                );
            }
            _ => {}
        }
    }
    if version != Some(READER_CACHE_VERSION) {
        return Err("unsupported cache version".into());
    }
    let fingerprint = fingerprint.ok_or_else(|| "missing cache fingerprint".to_string())?;
    if fingerprint != book_fingerprint(book, layout) {
        return Err("cache fingerprint mismatch".into());
    }
    if offsets.is_empty() {
        return Err("cache contains no offsets".into());
    }
    if offsets.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("cache offsets are not strictly increasing".into());
    }
    if offsets.iter().any(|offset| *offset > book.size_bytes) {
        return Err("cache offset exceeds book size".into());
    }
    Ok(ReaderAnchorCache {
        fingerprint,
        base_page: base_page.ok_or_else(|| "missing base page".to_string())?,
        offsets,
        indexed_through: indexed_through
            .ok_or_else(|| "missing indexed offset".to_string())?
            .min(book.size_bytes),
        complete: complete.ok_or_else(|| "missing complete flag".to_string())?,
    })
}

fn load_preferences(path: &Path) -> Result<Option<ReaderPreferences>, String> {
    load_with_backup(path, ReaderPreferences::parse)
}

fn load_location_record(path: &Path) -> Result<Option<ReaderLocation>, String> {
    load_with_backup(path, parse_location_record)
}

fn load_location_list(path: &Path, limit: usize) -> Result<Vec<ReaderLocation>, String> {
    load_with_backup(path, |text| parse_location_list(text, limit))
        .map(|value| value.unwrap_or_default())
}

fn load_anchor_cache(
    path: &Path,
    book: &ReaderBook,
    layout: ReaderLayout,
) -> Result<Option<ReaderAnchorCache>, String> {
    load_with_backup(path, |text| parse_anchor_cache(text, book, layout))
}

fn load_with_backup<T>(
    path: &Path,
    parser: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let backup = with_extension(path, "BAK");
    let mut errors = Vec::new();
    for candidate in [path.to_path_buf(), backup] {
        if !candidate.exists() {
            continue;
        }
        // Lossy: a single corrupted byte must not make the whole file
        // unreadable; the parser then skips just the record it landed in.
        match fs::read(&candidate).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()) {
            Ok(text) => match parser(&text) {
                Ok(value) => return Ok(Some(value)),
                Err(error) => errors.push(format!("{}: {error}", candidate.display())),
            },
            Err(error) => errors.push(format!("{}: {error}", candidate.display())),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(errors.join("; "))
    }
}

fn is_fat83_safe_file_name(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some((stem, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 8
        && !extension.is_empty()
        && extension.len() <= 3
        && stem
            .bytes()
            .chain(extension.bytes())
            .all(|value| value.is_ascii_alphanumeric() || value == b'_')
}

/// Power-safe bounded text replacement for Reader-owned state. The previous
/// primary is retained as .BAK until the new .TMP file has been renamed into
/// place. Readers accept the backup if startup observes an interrupted write.
fn atomic_replace_text(path: &Path, text: &str) -> Result<(), String> {
    atomic_replace_text_with_durability(path, text, true)
}

/// Same atomic temp-then-rename replace, but for regenerable cache files
/// (`.EPX`/`.EPP`/the TXT anchor `.CCH`, all under `CACHE/`) where losing the
/// last few writes to a power cut is harmless — the cache is simply rebuilt
/// on the next open. `fsync` on this hardware's SD/FAT stack is frequently
/// the single most expensive part of a save (routinely hundreds of ms to
/// low seconds), so skipping it here is a real, safe win. Files outside
/// `CACHE/` (reading position, bookmarks, preferences) are irreplaceable and
/// must keep going through [`atomic_replace_text`] instead.
fn atomic_replace_cache_text(path: &Path, text: &str) -> Result<(), String> {
    atomic_replace_text_with_durability(path, text, false)
}

fn atomic_replace_text_with_durability(path: &Path, text: &str, fsync: bool) -> Result<(), String> {
    let mut span = crate::boot_profile::span("atomic-replace");
    if crate::boot_profile::is_active() {
        span.detail(format_args!(
            "{} fsync={fsync} bytes={}",
            path.file_name().and_then(|name| name.to_str()).unwrap_or("?"),
            text.len()
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| "state path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let temp = with_extension(path, "TMP");
    let backup = with_extension(path, "BAK");
    for candidate in [path, temp.as_path(), backup.as_path()] {
        if !is_fat83_safe_file_name(candidate) {
            return Err(format!(
                "Reader state filename is not FAT 8.3 safe: {}",
                candidate.display()
            ));
        }
    }
    let _ = fs::remove_file(&temp);
    let _ = fs::remove_file(&backup);
    {
        let mut file =
            File::create(&temp).map_err(|error| format!("create {}: {error}", temp.display()))?;
        file.write_all(text.as_bytes())
            .map_err(|error| format!("write {}: {error}", temp.display()))?;
        if fsync {
            let _fsync_span = crate::boot_profile::span("atomic-replace-fsync");
            file.sync_all()
                .map_err(|error| format!("sync {}: {error}", temp.display()))?;
        }
    }
    if path.exists() {
        fs::rename(path, &backup).map_err(|error| format!("backup {}: {error}", path.display()))?;
    }
    if let Err(error) = fs::rename(&temp, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(format!("replace {}: {error}", path.display()));
    }
    // Durable state (positions, recent, bookmarks, state, preferences) keeps
    // the previous version as `.BAK`: `load_with_backup` falls back to it
    // when the main file cannot be read. Regenerable cache files do not need
    // the extra copy.
    if !fsync {
        let _ = fs::remove_file(&backup);
    }
    Ok(())
}

/// Copy a state file that failed to load (and its `.BAK`, if any) aside to
/// `.BAD`, before the next save replaces it with the empty list the loader
/// fell back to. The data stays on the card for recovery instead of being
/// silently overwritten. Never overwrites an existing `.BAD`, so the first
/// failure's evidence is the one kept.
fn quarantine_unreadable_state(path: &Path) {
    let quarantine = with_extension(path, "BAD");
    if quarantine.exists() {
        return;
    }
    let backup = with_extension(path, "BAK");
    let source = if path.exists() {
        path.to_path_buf()
    } else if backup.exists() {
        backup
    } else {
        return;
    };
    match fs::copy(&source, &quarantine) {
        Ok(_) => log::warn!(
            "rustmix-wave=reader-persistence status=quarantined source={} copy={}",
            source.display(),
            quarantine.display()
        ),
        Err(error) => log::warn!(
            "rustmix-wave=reader-persistence status=quarantine-failed source={} error={error}",
            source.display()
        ),
    }
}

fn with_extension(path: &Path, extension: &str) -> PathBuf {
    let mut output = path.to_path_buf();
    output.set_extension(extension);
    output
}

fn escape_field(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            value => output.push(value),
        }
    }
    output
}

fn unescape_field(value: &str) -> Result<String, String> {
    let mut output = String::new();
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            match character {
                '\\' => output.push('\\'),
                't' => output.push('\t'),
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                _ => return Err("invalid escape sequence".into()),
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            output.push(character);
        }
    }
    if escaped {
        return Err("trailing escape sequence".into());
    }
    Ok(output)
}

fn split_escaped_tabs(value: &str) -> Result<Vec<String>, String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push('\\');
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '\t' {
            output.push(unescape_field(&current)?);
            current.clear();
        } else {
            current.push(character);
        }
    }
    if escaped {
        return Err("trailing escape sequence".into());
    }
    output.push(unescape_field(&current)?);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::{
        atomic_replace_text, book_fingerprint, book_format_from_path, detect_txt_encoding,
        eligible_word_spans, epub_document_fingerprint, friendly_worker_start_error,
        is_fat83_safe_file_name, is_index_checkpoint, load_epub_page_index_cache,
        load_location_record, mid_word_resume_offset, normalize_decoded, paginate_decoded,
        parse_epub_document_cache, parse_location_fields, parse_location_record,
        read_epub_cache_header, reader_line_step, scan_txt_library, serialize_epub_document_cache,
        serialize_epub_page_index_cache, serialize_location, serialize_location_fields, BookFont,
        BookFontSize, BookFormat, EpubChapter, EpubDocument, EpubImage, EpubTocEntry,
        LibraryBookAction, ParagraphAlignment, ReaderBook, ReaderCachedPage,
        ReaderChapterPageLabel, ReaderDictionaryMode, ReaderLayout, ReaderLoadingStage,
        ReaderLocation, ReaderOrientation, ReaderPageLine, ReaderPreferences, ReaderSession,
        ReaderTickOutcome, ReaderUiState, ReadingPreference, ReadingTheme, TextEncoding,
        EPUB_IMAGE_SENTINEL, LEGACY_READER_POSITIONS_FILE, READER_BOOKMARKS_FILE,
        READER_CACHE_DIRECTORY, READER_INLINE_IMAGE_SLOT_SPAN, READER_POSITIONS_FILE,
        READER_PREFS_FILE, READER_RECENT_FILE, READER_SESSION_CACHE_LIMIT, READER_STATE_FILE,
    };
    use crate::buttons::ButtonEvent;

    fn temp_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("rustmix-reader-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn position_fixture(path: &str) -> ReaderLocation {
        ReaderLocation {
            path: path.into(),
            title: "Book".into(),
            format: BookFormat::Epub,
            size_bytes: 1000,
            modified_seconds: 42,
            page_index: 3,
            byte_offset: 500,
            epub_chapter: None,
            reading_percent: Some(21),
        }
    }

    #[test]
    fn a_half_read_book_pushed_out_of_recent_keeps_its_progress_and_resume_point() {
        let mut reader = ReaderUiState::with_roots("/nowhere/BOOKS", "/nowhere/STATE");
        let book = |index: usize| ReaderBook {
            path: format!("/sdcard/BOOKS/B{index}.EPUB"),
            title: format!("Book {index}"),
            format: BookFormat::Epub,
            size_bytes: 1000,
            modified_seconds: 42,
        };
        reader.books = (0..20).map(book).collect();
        let mut half_read = position_fixture(&book(0).path);
        half_read.byte_offset = 210;
        half_read.reading_percent = Some(21);
        let mut finished = position_fixture(&book(1).path);
        finished.reading_percent = Some(100);
        reader.positions = vec![half_read, finished];
        // 16 other books opened afterwards fill Recent completely.
        reader.recent = (4..20)
            .map(|index| {
                let mut location = position_fixture(&book(index).path);
                location.reading_percent = Some(0);
                location.byte_offset = 0;
                location
            })
            .collect();

        let visible = reader.visible_entries();
        assert_eq!(visible[0].book.path, book(0).path, "21% book sorts as in progress");
        assert_eq!(
            visible.last().unwrap().book.path,
            book(1).path,
            "finished book sorts as completed, not new"
        );

        assert!(reader.request_open_visible(0));
        let resume = reader
            .loading
            .as_ref()
            .and_then(|loading| loading.resume.as_ref())
            .expect("opening resumes from the saved position");
        assert_eq!(resume.byte_offset, 210);
    }

    #[test]
    fn one_damaged_position_record_does_not_lose_the_others() {
        let good = super::serialize_location_list(&[
            position_fixture("/sdcard/BOOKS/A.EPUB"),
            position_fixture("/sdcard/BOOKS/B.EPUB"),
        ]);
        let damaged = good.replacen("entry=", "entry=garbage\nentry=", 1);
        let positions = super::parse_location_list(&damaged, 64).unwrap();
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[1].path, "/sdcard/BOOKS/B.EPUB");
    }

    #[test]
    fn a_corrupted_positions_file_falls_back_to_the_kept_backup() {
        let root = temp_dir("positions-backup");
        let path = root.join(READER_POSITIONS_FILE);
        let first = super::serialize_location_list(&[position_fixture("/sdcard/BOOKS/A.EPUB")]);
        let second = super::serialize_location_list(&[
            position_fixture("/sdcard/BOOKS/A.EPUB"),
            position_fixture("/sdcard/BOOKS/B.EPUB"),
        ]);
        atomic_replace_text(&path, &first).unwrap();
        atomic_replace_text(&path, &second).unwrap();
        // The previous version stays on the card as `.BAK`.
        assert!(path.with_extension("BAK").exists());
        // The main file gets corrupted (unsupported version, invalid UTF-8).
        fs::write(&path, b"version=\xFF\xFE\nentry=x\n").unwrap();
        let recovered = super::load_location_list(&path, 64).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].path, "/sdcard/BOOKS/A.EPUB");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn worker_out_of_memory_gets_an_actionable_message() {
        let raw = "EPUB parser worker start failed: Not enough space (os error 12)";
        assert_eq!(
            friendly_worker_start_error(raw),
            "Not enough free memory to open this book right now. Restart the device and try again."
        );
    }

    #[test]
    fn unrelated_worker_errors_pass_through_unchanged() {
        let raw = "EPUB open failed: archive is not a valid zip";
        assert_eq!(friendly_worker_start_error(raw), raw);
    }

    #[test]
    fn detects_txt_epub_and_short_epu_aliases() {
        assert_eq!(
            book_format_from_path(PathBuf::from("a.TXT").as_path()),
            Some(BookFormat::Text)
        );
        assert_eq!(
            book_format_from_path(PathBuf::from("a.epub").as_path()),
            Some(BookFormat::Epub)
        );
        assert_eq!(
            book_format_from_path(PathBuf::from("a.EPU").as_path()),
            Some(BookFormat::Epub)
        );
    }

    #[test]
    fn detects_utf8_bom_and_windows_1252() {
        let root = temp_dir("encoding");
        let bom = root.join("bom.txt");
        let cp = root.join("cp.txt");
        fs::write(&bom, [0xEF, 0xBB, 0xBF, b'H', b'i']).unwrap();
        fs::write(&cp, [b'H', 0x92, b'i']).unwrap();
        assert_eq!(detect_txt_encoding(&bom).unwrap(), TextEncoding::Utf8Bom);
        assert_eq!(detect_txt_encoding(&cp).unwrap(), TextEncoding::Windows1252);
    }

    #[test]
    fn scans_txt_and_epub_rows_but_ignores_other_files() {
        let root = temp_dir("scan");
        fs::write(root.join("Dracula.txt"), "hello").unwrap();
        fs::write(root.join("Later.epu"), "zip").unwrap();
        fs::write(root.join("ignore.bin"), "no").unwrap();
        let books = scan_txt_library(&root, &[]).unwrap();
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].title, "Dracula");
        assert_eq!(books[1].format, BookFormat::Epub);
    }

    #[test]
    fn opening_txt_is_staged_first_page_first_and_lazy() {
        let root = temp_dir("open");
        let state = temp_dir("open-state");
        fs::write(root.join("Book.txt"), "hello world ".repeat(600)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::FirstPageReady);
        let session = reader.session.as_ref().unwrap();
        assert_eq!(session.current_page, 0);
        assert!(!session.cache.is_empty());
        assert!(session.indexed_through > 0);
    }

    #[test]
    fn persists_continue_recent_bookmarks_and_anchor_cache() {
        let root = temp_dir("persist-books");
        let state = temp_dir("persist-state");
        fs::write(root.join("Dracula.txt"), "Dracula text ".repeat(1000)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::FirstPageReady);
        reader.next_page();
        // `next_page` only marks the save pending now (it runs after the
        // panel refresh in production, see `ReaderUiState::pending_persist`);
        // flush it explicitly here since this test has no refresh to piggyback on.
        reader.flush_pending_persist();
        reader.toggle_current_bookmark();
        assert!(state.join(READER_STATE_FILE).exists());
        assert!(state.join(READER_POSITIONS_FILE).exists());
        assert!(state.join(READER_RECENT_FILE).exists());
        assert!(state.join(READER_BOOKMARKS_FILE).exists());
        assert!(state.join("CACHE").read_dir().unwrap().next().is_some());

        let mut restored = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        let report = restored.load_persistent_state();
        assert!(report.state_loaded);
        assert_eq!(report.recent_count, 1);
        assert_eq!(report.bookmark_count, 1);
        assert!(restored.request_continue());
        // `LoadingSavedPosition` is pure bookkeeping (no informative message
        // worth its own tick/redraw), so it now chains straight into
        // `BuildingFirstPage` within the same `tick()` call — see the
        // `stops_here` comment in `ReaderUiState::tick`.
        assert_eq!(restored.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(restored.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(restored.tick(), ReaderTickOutcome::FirstPageReady);
        assert_eq!(
            restored.session.as_ref().unwrap().current_absolute_page(),
            1
        );
    }

    #[test]
    fn deep_sleep_active_marker_round_trips_and_defaults_to_inactive() {
        let root = temp_dir("deep-sleep-marker-books");
        let state = temp_dir("deep-sleep-marker-state");
        let reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        assert!(!reader.deep_sleep_marker_indicates_active());

        reader.record_deep_sleep_active_marker(true).unwrap();
        assert!(reader.deep_sleep_marker_indicates_active());

        reader.record_deep_sleep_active_marker(false).unwrap();
        assert!(!reader.deep_sleep_marker_indicates_active());
    }

    #[test]
    fn invalid_anchor_cache_fingerprint_falls_back_to_saved_offset() {
        let root = temp_dir("fingerprint-books");
        let state = temp_dir("fingerprint-state");
        fs::write(root.join("Book.txt"), "text body ".repeat(1000)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::FirstPageReady);
        reader.next_page();
        // See the comment in `persists_continue_recent_bookmarks_and_anchor_cache`:
        // the save is deferred until a refresh flushes it.
        reader.flush_pending_persist();
        let cache = state
            .join("CACHE")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = fs::read_to_string(&cache).unwrap();
        fs::write(
            &cache,
            text.replace("fingerprint=", "fingerprint=0000000000000000#"),
        )
        .unwrap();

        let mut restored = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        restored.load_persistent_state();
        assert!(restored.request_continue());
        // `LoadingSavedPosition` now chains into `BuildingFirstPage` within
        // the same `tick()` call (see the `stops_here` comment in
        // `ReaderUiState::tick`).
        assert_eq!(restored.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(restored.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(restored.tick(), ReaderTickOutcome::FirstPageReady);
        assert!(restored
            .persistence_warning
            .as_deref()
            .unwrap_or("")
            .contains("TXT cache ignored"));
        assert_eq!(
            restored.session.as_ref().unwrap().current_absolute_page(),
            1
        );
    }

    #[test]
    fn bookmark_toggle_removes_existing_mark() {
        let root = temp_dir("toggle-books");
        let state = temp_dir("toggle-state");
        fs::write(root.join("Book.txt"), "text ".repeat(100)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::FirstPageReady);
        reader.toggle_current_bookmark();
        assert_eq!(reader.bookmarks.len(), 1);
        reader.toggle_current_bookmark();
        assert!(reader.bookmarks.is_empty());
    }

    #[test]
    fn interrupted_atomic_replace_recovers_backup() {
        let root = temp_dir("backup");
        let state = root.join(READER_STATE_FILE);
        atomic_replace_text(
            &state,
            "version=1\npath=a.txt\ntitle=A\nformat=txt\nsize=1\nmodified=0\npage=0\noffset=0\n",
        )
        .unwrap();
        let backup = root.join("STATE.BAK");
        fs::rename(&state, &backup).unwrap();
        let restored = load_location_record(&state).unwrap().unwrap();
        assert_eq!(restored.title, "A");
    }

    #[test]
    fn corrupt_primary_falls_back_to_backup() {
        let root = temp_dir("corrupt");
        let state = root.join(READER_STATE_FILE);
        fs::write(&state, "not-valid").unwrap();
        fs::write(
            root.join("STATE.BAK"),
            "version=1\npath=b.txt\ntitle=B\nformat=txt\nsize=2\nmodified=0\npage=3\noffset=4\n",
        )
        .unwrap();
        let restored = load_location_record(&state).unwrap().unwrap();
        assert_eq!(restored.title, "B");
    }

    #[test]
    fn reader_options_request_manual_clear_ghosting() {
        let mut reader = ReaderUiState::default();
        reader.request_clear_ghosting();
        assert!(reader.take_clear_ghost_request());
        assert!(!reader.take_clear_ghost_request());
    }

    #[test]
    fn normalizes_utf8_punctuation_unsupported_accents_and_simple_emphasis() {
        let decoded: Vec<(char, u64)> = "“En vêrité!” _I_—once…"
            .chars()
            .enumerate()
            .map(|(index, value)| (value, index as u64 + 1))
            .collect();
        let normalized: String = normalize_decoded(&decoded)
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        assert_eq!(normalized, "\"En verité!\" I--once...");
    }

    #[test]
    fn normalization_passes_the_epub_image_sentinel_through_unchanged() {
        // `EPUB_IMAGE_SENTINEL` marks one inline `<img>`'s position in the
        // flattened text (see its own doc comment in epub.rs). Normalization
        // must leave it exactly as-is rather than folding it away like an
        // unsupported codepoint -- `paginate_decoded` is what recognizes it
        // and reserves page space for it, and can only do that if it
        // survives this pass.
        let decoded: Vec<(char, u64)> = format!("Before{EPUB_IMAGE_SENTINEL}After")
            .chars()
            .enumerate()
            .map(|(index, value)| (value, index as u64 + 1))
            .collect();
        let normalized: String = normalize_decoded(&decoded)
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        assert_eq!(normalized, format!("Before{EPUB_IMAGE_SENTINEL}After"));
    }

    #[test]
    fn keeps_italian_accents_the_reader_fonts_can_render() {
        let decoded: Vec<(char, u64)> = "città perché così più è È"
            .chars()
            .enumerate()
            .map(|(index, value)| (value, index as u64 + 1))
            .collect();
        let normalized: String = normalize_decoded(&decoded)
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        assert_eq!(normalized, "città perché così più è È");
    }

    #[test]
    fn removes_multiline_gutenberg_emphasis_but_preserves_safe_underscores() {
        let decoded: Vec<(char, u64)> =
            "'_You have lost your\ngold pencil-case? Couragez!'_ file_name\n_____"
                .chars()
                .enumerate()
                .map(|(index, value)| (value, index as u64 + 1))
                .collect();
        let normalized: String = normalize_decoded(&decoded)
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        assert_eq!(
            normalized,
            "'You have lost your\ngold pencil-case? Couragez!' file_name\n_____"
        );
    }

    fn word_wrap_layout(chars_per_line: usize, lines_per_page: usize) -> ReaderLayout {
        ReaderLayout {
            available_width_px: chars_per_line as i32,
            lines_per_page,
            orientation: ReaderOrientation::Portrait,
            font_size: BookFontSize::Large,
            book_font: BookFont::Serif,
            paragraph_alignment: ParagraphAlignment::Left,
            full_screen: false,
        }
    }

    /// One "pixel" per character, so these pagination tests can exercise
    /// `place_word` / `paginate_decoded` with the same simple character
    /// budgets they used before pagination switched to real glyph widths.
    fn monospace_width(text: &str) -> i32 {
        text.chars().count() as i32
    }

    fn decoded_from(text: &str) -> Vec<(char, u64)> {
        text.chars()
            .enumerate()
            .map(|(index, value)| (value, index as u64 + 1))
            .collect()
    }

    /// Real UTF-8 byte offsets, matching `decode_with_offsets`'s own
    /// `TextEncoding::Utf8` case exactly (`base=0`) -- unlike
    /// [`decoded_from`]'s simplified per-character index, this is needed for
    /// the inline-image tests below, since a real `EpubImage::text_offset`
    /// is a true byte offset and `EPUB_IMAGE_SENTINEL` is multiple bytes.
    fn decoded_from_bytes(text: &str) -> Vec<(char, u64)> {
        text.char_indices()
            .map(|(index, value)| (value, index as u64 + value.len_utf8() as u64))
            .collect()
    }

    /// Paginate `text` page by page from its start, the way the Reader
    /// does: each page resumes at the previous page's resume offset.
    fn paginate_all(text: &str, layout: ReaderLayout) -> Vec<Vec<ReaderPageLine>> {
        let decoded = decoded_from(text);
        let mut pages = Vec::new();
        let mut consumed = 0;
        while consumed < decoded.len() as u64 {
            let remaining: Vec<_> = decoded
                .iter()
                .copied()
                .filter(|(_, offset)| *offset > consumed)
                .collect();
            let (lines, next) = paginate_decoded(&remaining, layout, &[], &monospace_width);
            assert!(next > consumed, "pagination stalled at {consumed}");
            consumed = next;
            pages.push(lines);
            assert!(pages.len() < 1000, "runaway pagination");
        }
        pages
    }

    #[test]
    fn a_word_longer_than_a_whole_page_is_split_across_pages_without_stalling() {
        // 10 x 3 characters per page; the word needs 4 pages.
        let word = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz0123456789";
        let text = format!("start {word} end");
        let pages = paginate_all(&text, word_wrap_layout(10, 3));
        let shown: String = pages
            .iter()
            .flatten()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // Every character shown exactly once, in order.
        assert_eq!(shown.replace(' ', ""), text.replace(' ', ""));
    }

    #[test]
    fn a_hard_broken_word_filling_the_page_is_not_repeated_on_the_next_page() {
        // The 25-character word starts on the page's second line and fills
        // it; the next page must continue right after the shown part.
        let word = "0123456789ABCDEFGHIJKLMNO";
        let text = format!("first line {word} after");
        let pages = paginate_all(&text, word_wrap_layout(10, 3));
        assert_eq!(
            pages[0]
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["first line", "0123456789", "ABCDEFGHIJ"]
        );
        assert_eq!(pages[1][0].text, "KLMNO");
        assert_eq!(pages[1][1].text, "after");
    }

    #[test]
    fn mid_word_resume_backs_off_to_a_whole_source_character() {
        // "o" "ﬁ"->"f","i" "x": the break after "f" lands inside the
        // ligature's expansion, so resume after "o" and show "fi" again.
        let ends = [1, 4, 4, 5];
        assert_eq!(mid_word_resume_offset(&ends, 2), Some(1));
        assert_eq!(mid_word_resume_offset(&ends, 3), Some(4));
        assert_eq!(mid_word_resume_offset(&ends, 0), None);
        // A group at the very start cannot back off: keep progressing.
        assert_eq!(mid_word_resume_offset(&[3, 3, 3, 4], 1), Some(3));
    }

    #[test]
    fn index_checkpoints_are_dense_early_then_sparse() {
        let checkpoints: Vec<usize> = (1..=300)
            .filter(|&pages| is_index_checkpoint(pages))
            .collect();
        assert_eq!(
            checkpoints,
            [4, 8, 12, 16, 20, 24, 28, 32, 64, 128, 192, 256]
        );
    }

    #[test]
    fn wraps_at_word_boundaries_instead_of_cutting_words() {
        // "hello world" is exactly 11 characters, so it packs onto one line;
        // "foo" does not fit alongside it and moves to its own line. Neither
        // word is ever cut mid-character.
        let decoded = decoded_from("hello world foo");
        let (lines, _) =
            paginate_decoded(&decoded, word_wrap_layout(11, 10), &[], &monospace_width);
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["hello world", "foo"]);
        for line in &lines {
            assert!(line.text.chars().count() <= 11);
        }
    }

    #[test]
    fn a_word_longer_than_one_line_hard_breaks_without_dropping_characters() {
        let decoded = decoded_from("supercalifragilistic word");
        let (lines, _) =
            paginate_decoded(&decoded, word_wrap_layout(6, 10), &[], &monospace_width);
        let rebuilt: String = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // Hard-broken pieces are stitched back with a single space each, so
        // rebuilding must exactly reproduce the run of non-space characters.
        assert_eq!(
            rebuilt.split_whitespace().collect::<String>(),
            "supercalifragilisticword"
        );
        for line in &lines {
            assert!(line.text.chars().count() <= 6);
        }
    }

    #[test]
    fn a_word_that_does_not_fit_the_page_defers_whole_to_the_next_page() {
        // chars_per_line=2 keeps "ab" and "cd" from ever sharing a line;
        // lines_per_page=1 means only "ab" fits on this page at all. "cd" and
        // "efgh" must be deferred whole rather than split across the page
        // boundary.
        let decoded = decoded_from("ab cd efgh");
        let (lines, consumed) =
            paginate_decoded(&decoded, word_wrap_layout(2, 1), &[], &monospace_width);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "ab");
        // consumed must land right after "ab " (the committed word plus its
        // trailing separator), not mid-word, so the next page re-reads "cd
        // efgh" whole instead of splitting it across the page boundary.
        assert_eq!(consumed, 3);
        let remainder = decoded_from("cd efgh");
        let (next_lines, _) =
            paginate_decoded(&remainder, word_wrap_layout(2, 1), &[], &monospace_width);
        assert_eq!(next_lines[0].text, "cd");
    }

    #[test]
    fn a_full_page_ending_on_a_clean_word_boundary_does_not_skip_trailing_text() {
        // "ab" fills the one-line page and is immediately followed by a
        // newline, so the page fills up right on a clean word boundary (the
        // word buffer is empty when the line budget is hit). "cdefgh" is
        // further content within the same decoded window/chapter that must
        // stay unread and unconsumed for the *next* page to pick up --
        // `consumed` must not jump past it just because it happened to be
        // present in this call's `decoded` slice.
        let decoded = decoded_from("ab\ncdefgh");
        let (lines, consumed) =
            paginate_decoded(&decoded, word_wrap_layout(10, 1), &[], &monospace_width);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "ab");
        // consumed must land right after "ab\n", not at the end of the whole
        // decoded slice ("cdefgh" was never rendered on this page).
        assert_eq!(consumed, 3);
    }

    #[test]
    fn reserves_slot_span_for_an_inline_image_and_continues_text_after_it() {
        let text = format!("Intro{EPUB_IMAGE_SENTINEL}Outro");
        let sentinel_offset = text.find(EPUB_IMAGE_SENTINEL).unwrap() as u64;
        let images = vec![EpubImage {
            href: "OEBPS/images/fig1.jpg".into(),
            alt: "A figure".into(),
            text_offset: sentinel_offset,
            spine_index: 0,
            width: 0,
            height: 0,
        }];
        let decoded = decoded_from_bytes(&text);
        // Generous width/lines_per_page so word-wrapping itself never
        // interferes -- this test is purely about slot reservation.
        let layout = word_wrap_layout(80, 20);
        let (lines, _) = paginate_decoded(&decoded, layout, &images, &monospace_width);

        let image_index = lines
            .iter()
            .position(|line| line.image.is_some())
            .expect("image line must be present");
        let image = lines[image_index].image.as_ref().unwrap();
        assert_eq!(image.href, "OEBPS/images/fig1.jpg");
        assert_eq!(image.alt, "A figure");
        assert_eq!(image.slot_span, READER_INLINE_IMAGE_SLOT_SPAN);

        // The image reserves exactly `slot_span` consecutive entries: itself
        // plus `slot_span - 1` blank continuation lines with no image and no
        // text, so every other line-counting invariant elsewhere keeps
        // working unmodified (see `ReaderPageLine::image`'s doc comment).
        for offset in 1..image.slot_span {
            let continuation = &lines[image_index + offset];
            assert!(continuation.image.is_none());
            assert!(continuation.text.is_empty());
        }

        // Text before and after the image is still there, on its own lines,
        // not swallowed by the image block.
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert!(texts.contains(&"Intro"));
        assert!(texts.contains(&"Outro"));
    }

    #[test]
    fn an_image_that_does_not_fit_the_current_page_defers_whole_to_the_next_page() {
        let text = format!("First line.\n{EPUB_IMAGE_SENTINEL}Rest.");
        let sentinel_offset = text.find(EPUB_IMAGE_SENTINEL).unwrap() as u64;
        let images = vec![EpubImage {
            href: "img.jpg".into(),
            alt: String::new(),
            text_offset: sentinel_offset,
            spine_index: 0,
            width: 0,
            height: 0,
        }];
        let decoded = decoded_from_bytes(&text);
        // lines_per_page=3 leaves only 2 slots free after "First line."
        // claims one -- not enough for an 8-slot image, which must defer
        // whole to the next page rather than spilling past the budget.
        let layout = word_wrap_layout(80, 3);
        let (lines, consumed) = paginate_decoded(&decoded, layout, &images, &monospace_width);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "First line.");
        assert!(lines.iter().all(|line| line.image.is_none()));
        // `consumed` must land right after "First line.\n", not swallow the
        // image -- the next page's pagination re-reads from here and sees
        // the sentinel fresh, with a full line budget to place it in.
        assert_eq!(consumed, sentinel_offset);
    }

    fn image_fixture(text: &str, width: u32, height: u32) -> Vec<EpubImage> {
        vec![EpubImage {
            href: "img.jpg".into(),
            alt: String::new(),
            text_offset: text.find(EPUB_IMAGE_SENTINEL).unwrap() as u64,
            spine_index: 0,
            width,
            height,
        }]
    }

    #[test]
    fn a_standalone_image_such_as_a_cover_page_takes_the_whole_page() {
        // A cover spine item flattens to just the sentinel: nothing before
        // it on the page and only whitespace after it in the chapter.
        let text = format!("{EPUB_IMAGE_SENTINEL}\n");
        let images = image_fixture(&text, 600, 900);
        let layout = word_wrap_layout(400, 20);
        let (lines, _) =
            paginate_decoded(&decoded_from_bytes(&text), layout, &images, &monospace_width);
        let image = lines[0].image.as_ref().expect("image on first line");
        assert_eq!(image.slot_span, 20);
        assert_eq!(i32::from(image.box_width), layout.available_width_px);
        assert_eq!(
            i32::from(image.box_height),
            20 * reader_line_step(&layout) - 2
        );
    }

    #[test]
    fn an_image_followed_by_text_is_not_standalone() {
        let text = format!("{EPUB_IMAGE_SENTINEL}Caption text.");
        let images = image_fixture(&text, 0, 0);
        let (lines, _) = paginate_decoded(
            &decoded_from_bytes(&text),
            word_wrap_layout(400, 20),
            &images,
            &monospace_width,
        );
        let image = lines[0].image.as_ref().expect("image on first line");
        assert_eq!(image.slot_span, READER_INLINE_IMAGE_SLOT_SPAN);
    }

    #[test]
    fn a_known_size_image_reserves_slots_matching_its_aspect_ratio() {
        let layout = word_wrap_layout(400, 20);
        let line_step = reader_line_step(&layout);
        // Wide, short banner: 800x100 fits the 400px column at 400x50.
        let text = format!("Intro {EPUB_IMAGE_SENTINEL}Outro");
        let wide = image_fixture(&text, 800, 100);
        let (lines, _) =
            paginate_decoded(&decoded_from_bytes(&text), layout, &wide, &monospace_width);
        let image = lines.iter().find_map(|line| line.image.as_ref()).unwrap();
        assert_eq!((image.box_width, image.box_height), (400, 50));
        assert_eq!(image.slot_span, ((50 + 2 + line_step - 1) / line_step) as usize);

        // A small ornament is upscaled at most READER_INLINE_IMAGE_MAX_UPSCALE
        // times instead of being blown up to the full column width.
        let small = image_fixture(&text, 60, 20);
        let (lines, _) =
            paginate_decoded(&decoded_from_bytes(&text), layout, &small, &monospace_width);
        let image = lines.iter().find_map(|line| line.image.as_ref()).unwrap();
        assert_eq!((image.box_width, image.box_height), (120, 40));

        // A very tall image is capped at one page height. It opens the page
        // here: behind "Intro" a page-tall image would defer to the next one.
        let text = format!("{EPUB_IMAGE_SENTINEL}Outro");
        let tall = image_fixture(&text, 100, 10_000);
        let (lines, _) =
            paginate_decoded(&decoded_from_bytes(&text), layout, &tall, &monospace_width);
        let image = lines.iter().find_map(|line| line.image.as_ref()).unwrap();
        assert!(i32::from(image.box_height) <= 20 * line_step - 2);
        assert!(image.slot_span <= 20);
    }

    #[test]
    fn invisible_and_typographic_characters_do_not_become_question_marks() {
        let source = "pa\u{00AD}ro\u{200B}la\u{FEFF} e\u{0301} \u{FB01}ne \u{2022} \u{2009}x\u{2011}y";
        let decoded: Vec<(char, u64)> = source
            .chars()
            .enumerate()
            .map(|(index, value)| (value, index as u64 + 1))
            .collect();
        let normalized: String = normalize_decoded(&decoded)
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        assert!(!normalized.contains('?'), "{normalized:?}");
        assert_eq!(normalized, "parola e fine *  x-y");
    }

    #[test]
    fn a_sentinel_with_no_matching_image_record_is_skipped_without_panicking() {
        // Behavior under test is "does not panic and does not drop
        // surrounding content" -- with a generous width/page budget and no
        // forced line break, "Before" and "After" legitimately end up
        // word-wrapped onto the same line (like any other two words
        // separated only by whitespace), so this checks the rebuilt text
        // rather than asserting a specific line split.
        let text = format!("Before {EPUB_IMAGE_SENTINEL} After");
        let decoded = decoded_from_bytes(&text);
        let layout = word_wrap_layout(80, 20);
        let (lines, _) = paginate_decoded(&decoded, layout, &[], &monospace_width);
        assert!(lines.iter().all(|line| line.image.is_none()));
        let rebuilt = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(rebuilt.contains("Before"));
        assert!(rebuilt.contains("After"));
    }

    #[test]
    fn theme_switch_keeps_layout_geometry_and_cache_fingerprint_inputs_stable() {
        let classic = ReaderPreferences::default();
        let mut contrast = classic;
        contrast.theme = ReadingTheme::HighContrast;
        assert_eq!(classic.layout(), contrast.layout());
    }

    #[test]
    fn reader_font_cycle_preserves_legacy_keys_and_adds_literata() {
        assert_eq!(
            BookFont::AtkinsonHyperlegible.marker(),
            "atkinson-hyperlegible"
        );
        assert_eq!(BookFont::Serif.marker(), "serif");
        assert_eq!(BookFont::Literata.marker(), "literata");
        assert_eq!(BookFont::Inter.next(), BookFont::AtkinsonHyperlegible);
        assert_eq!(BookFont::AtkinsonHyperlegible.next(), BookFont::Serif);
        assert_eq!(BookFont::Serif.next(), BookFont::Literata);
        assert_eq!(BookFont::Literata.next(), BookFont::Inter);
        assert_eq!(BookFont::Inter.previous(), BookFont::Literata);
        assert_eq!(BookFont::parse("literata").unwrap(), BookFont::Literata);
    }

    #[test]
    fn parses_serializes_and_cycles_reader_preferences() {
        let parsed = ReaderPreferences::parse(
            "version=1\ntheme=high-contrast\norientation=landscape\nfont_size=xlarge\nbook_font=serif\nparagraph_alignment=right\nshow_progress=false\n",
        )
        .unwrap();
        assert_eq!(parsed.theme, ReadingTheme::HighContrast);
        assert_eq!(parsed.orientation, ReaderOrientation::Landscape);
        assert_eq!(parsed.font_size, BookFontSize::XLarge);
        assert_eq!(parsed.book_font, BookFont::Serif);
        assert_eq!(parsed.paragraph_alignment, ParagraphAlignment::Right);
        assert!(!parsed.show_progress);
        assert!(parsed.serialized().contains("font_size=xlarge"));
        assert!(parsed.serialized().contains("book_font=serif"));
        assert!(parsed.serialized().contains("paragraph_alignment=right"));
        // Files written before full screen existed load with it off.
        assert!(!parsed.full_screen);
    }

    /// Full screen persists, and paginates against its own (taller) page:
    /// more lines, a distinct cache fingerprint, while turning it off keeps
    /// every pre-existing page cache's fingerprint unchanged.
    #[test]
    fn full_screen_persists_and_gets_its_own_pagination() {
        let normal = ReaderPreferences::default();
        let full = ReaderPreferences {
            full_screen: true,
            ..normal
        };
        let reparsed = ReaderPreferences::parse(&full.serialized()).unwrap();
        assert!(reparsed.full_screen);
        assert!(full.layout().lines_per_page > normal.layout().lines_per_page);
        let book = ReaderBook {
            path: "Book.txt".into(),
            title: "Book".into(),
            format: BookFormat::Text,
            size_bytes: 1000,
            modified_seconds: 0,
        };
        assert_ne!(
            book_fingerprint(&book, full.layout()),
            book_fingerprint(&book, normal.layout())
        );
    }

    /// `Small` and `Medium` were removed in favor of two tiers larger than
    /// the old `XLarge` ceiling. Preference files a pre-upgrade firmware
    /// wrote with those markers must still load -- into the new smallest
    /// tier -- instead of failing to parse and silently reverting every
    /// other saved Reader preference to its default.
    #[test]
    fn legacy_small_and_medium_font_size_markers_migrate_to_large() {
        assert_eq!(BookFontSize::parse("small").unwrap(), BookFontSize::Large);
        assert_eq!(BookFontSize::parse("medium").unwrap(), BookFontSize::Large);
        assert_eq!(
            BookFontSize::parse("xxlarge").unwrap(),
            BookFontSize::XXLarge
        );
        assert_eq!(
            BookFontSize::parse("xxxlarge").unwrap(),
            BookFontSize::XXXLarge
        );
    }

    #[test]
    fn book_font_size_cycle_covers_all_four_tiers_in_order() {
        assert_eq!(BookFontSize::Large.next(), BookFontSize::XLarge);
        assert_eq!(BookFontSize::XLarge.next(), BookFontSize::XXLarge);
        assert_eq!(BookFontSize::XXLarge.next(), BookFontSize::XXXLarge);
        assert_eq!(BookFontSize::XXXLarge.next(), BookFontSize::Large);
        assert_eq!(BookFontSize::Large.previous(), BookFontSize::XXXLarge);
        assert_eq!(BookFontSize::XXXLarge.previous(), BookFontSize::XXLarge);
    }

    #[test]
    fn layout_changes_request_first_page_first_rebuild_and_persist_preferences() {
        let root = temp_dir("prefs-books");
        let state = temp_dir("prefs-state");
        fs::write(root.join("Book.txt"), "hello world ".repeat(800)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::LoadingStageChanged);
        assert_eq!(reader.tick(), ReaderTickOutcome::FirstPageReady);
        assert!(reader.cycle_book_font_size());
        assert_eq!(
            reader.loading_stage(),
            Some(ReaderLoadingStage::UpdatingLayout)
        );
        assert!(state.join(READER_PREFS_FILE).exists());
    }

    #[test]
    fn books_and_files_reopen_from_per_book_positions_while_bookmarks_remain_explicit() {
        let root = temp_dir("positions-books");
        let state = temp_dir("positions-state");
        fs::write(root.join("A.txt"), "alpha body ".repeat(1200)).unwrap();
        fs::write(root.join("B.txt"), "beta body ".repeat(1200)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        for _ in 0..3 {
            reader.tick();
        }
        reader.next_page();
        reader.next_page();
        let saved = reader.session.as_ref().unwrap().current_location();
        assert_eq!(saved.page_index, 2);

        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        for _ in 0..4 {
            reader.tick();
        }
        assert_eq!(reader.session.as_ref().unwrap().current_absolute_page(), 2);

        let mut explicit = saved.clone();
        explicit.page_index = 1;
        explicit.byte_offset = reader.session.as_ref().unwrap().page_offsets[1];
        reader.bookmarks = vec![explicit];
        assert!(reader.request_open_bookmark(0));
        for _ in 0..4 {
            reader.tick();
        }
        assert_eq!(reader.session.as_ref().unwrap().current_absolute_page(), 1);
    }

    #[test]
    fn paragraph_alignment_defaults_to_justified_and_changes_cache_fingerprint_inputs() {
        let justified = ReaderPreferences::default();
        assert_eq!(justified.paragraph_alignment, ParagraphAlignment::Justified);
        let mut left = justified;
        left.paragraph_alignment = ParagraphAlignment::Left;
        assert_ne!(justified.layout(), left.layout());
    }

    #[test]
    fn preference_editor_browses_candidates_then_commits_or_cancels() {
        let mut reader = ReaderUiState::default();
        reader.begin_preferences_edit();
        assert_eq!(
            reader.selected_preference(),
            ReadingPreference::ReadingTheme
        );
        reader.cycle_preference_next();
        assert_eq!(reader.selected_preference(), ReadingPreference::Orientation);
        reader.cycle_preference_previous();
        assert_eq!(
            reader.selected_preference(),
            ReadingPreference::ReadingTheme
        );

        let initial_theme = reader.preferences.theme;
        reader.open_preference_editor();
        reader.cycle_preference_editor_next();
        assert_eq!(
            reader.preference_edit.unwrap().theme,
            ReadingTheme::HighContrast
        );
        // Browsing alone never touches the real preferences.
        assert_eq!(reader.preferences.theme, initial_theme);

        assert!(reader.cancel_preference_edit());
        assert!(reader.preference_edit.is_none());
        assert_eq!(reader.preferences.theme, initial_theme);
        assert!(!reader.cancel_preference_edit());

        reader.open_preference_editor();
        reader.cycle_preference_editor_next();
        assert!(!reader.commit_preference_edit());
        assert!(reader.preference_edit.is_none());
        assert_eq!(reader.preferences.theme, ReadingTheme::HighContrast);
    }

    #[test]
    fn reader_owned_runtime_filenames_are_fat83_safe() {
        for name in [
            READER_STATE_FILE,
            READER_POSITIONS_FILE,
            READER_RECENT_FILE,
            READER_BOOKMARKS_FILE,
            READER_PREFS_FILE,
            "ED9B69AF.CCH",
            "ED9B69AF.TMP",
            "ED9B69AF.BAK",
        ] {
            assert!(is_fat83_safe_file_name(Path::new(name)), "{name}");
        }
        assert!(!is_fat83_safe_file_name(Path::new(
            LEGACY_READER_POSITIONS_FILE
        )));
        assert!(!is_fat83_safe_file_name(Path::new("BED9B69AF.CCH")));
    }

    #[test]
    fn cache_filename_uses_exactly_eight_hexadecimal_characters() {
        let root = temp_dir("fat83-cache-books");
        let state = temp_dir("fat83-cache-state");
        fs::write(root.join("Book.txt"), "text body ".repeat(1000)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        let book = reader.books.first().unwrap();
        let cache = reader.cache_path_for(book, reader.preferences.layout());
        let file = cache.file_name().unwrap().to_str().unwrap();
        assert_eq!(file.len(), 12);
        assert_eq!(&file[8..], ".CCH");
        assert!(file[..8].bytes().all(|value| value.is_ascii_hexdigit()));
        assert!(is_fat83_safe_file_name(&cache));
    }

    #[test]
    fn legacy_positions_file_migrates_to_short_name_safe_primary() {
        let root = temp_dir("legacy-positions-books");
        let state = temp_dir("legacy-positions-state");
        fs::write(root.join("Book.txt"), "text body ".repeat(1000)).unwrap();
        let legacy = state.join(LEGACY_READER_POSITIONS_FILE);
        fs::write(
            &legacy,
            "version=1\nentry=Book.txt\tBook\ttxt\t1000\t0\t3\t42\n",
        )
        .unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        let report = reader.load_persistent_state();
        assert_eq!(report.position_count, 1);
        assert!(state.join(READER_POSITIONS_FILE).exists());
        assert_eq!(reader.positions[0].byte_offset, 42);
    }

    #[test]
    fn fat83_runtime_primary_temp_and_backup_paths_are_safe_without_cache_prefix() {
        let root = temp_dir("fat83-runtime-books");
        let state = temp_dir("fat83-runtime-state");
        fs::write(root.join("Book.txt"), "text body ".repeat(1000)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        let book = reader.books.first().unwrap();
        let positions = reader.positions_path();
        let cache = reader.cache_path_for(book, reader.preferences.layout());
        for path in [
            positions.clone(),
            super::with_extension(&positions, "TMP"),
            super::with_extension(&positions, "BAK"),
            cache.clone(),
            super::with_extension(&cache, "TMP"),
            super::with_extension(&cache, "BAK"),
        ] {
            assert!(is_fat83_safe_file_name(&path), "{}", path.display());
        }
        let cache_file = cache.file_name().unwrap().to_str().unwrap();
        assert_eq!(&cache_file[8..], ".CCH");
        assert!(
            !cache_file.starts_with('B')
                || cache_file[..8]
                    .bytes()
                    .all(|value| value.is_ascii_hexdigit())
        );
        assert_eq!(cache_file[..8].len(), 8);
    }

    #[test]
    fn bookmark_page_label_uses_active_layout_offsets_and_stored_fallback() {
        let book = ReaderBook {
            path: "Book.txt".into(),
            title: "Book".into(),
            format: BookFormat::Text,
            size_bytes: 1000,
            modified_seconds: 0,
        };
        let bookmark = ReaderLocation {
            path: book.path.clone(),
            title: book.title.clone(),
            format: book.format,
            size_bytes: book.size_bytes,
            modified_seconds: book.modified_seconds,
            page_index: 8,
            byte_offset: 220,
            epub_chapter: None,
            reading_percent: None,
        };
        let mut reader = ReaderUiState::default();
        assert_eq!(reader.bookmark_display_page(&bookmark), 9);
        reader.session = Some(ReaderSession {
            book,
            encoding: TextEncoding::Utf8,
            epub_document: None,
            layout: ReaderPreferences::default().layout(),
            current_page: 0,
            page_number_base: 0,
            page_offsets: vec![0, 100, 200, 300],
            indexed_through: 300,
            index_complete: false,
            cache: Vec::new(),
            epub_chapter_pages: Vec::new(),
            epub_pending_chapter: None,
            epub_document_cache_pending: false,
        });
        assert_eq!(reader.bookmark_display_page(&bookmark), 3);
    }

    #[test]
    fn epub_chapter_labels_use_chapter_relative_page_totals_and_persist() {
        let label = ReaderChapterPageLabel {
            chapter_number: 3,
            page_number: 2,
            page_count: 9,
        };
        let location = ReaderLocation {
            path: "book.epub".into(),
            title: "Book title".into(),
            format: BookFormat::Epub,
            size_bytes: 100,
            modified_seconds: 7,
            page_index: 11,
            byte_offset: 55,
            epub_chapter: Some(label.clone()),
            reading_percent: Some(55),
        };
        assert_eq!(
            parse_location_record(&serialize_location(&location)).unwrap(),
            location
        );
        assert_eq!(
            parse_location_fields(&serialize_location_fields(&location)).unwrap(),
            location
        );
        assert_eq!(label.page_text(), "2/9");
    }

    #[test]
    fn legacy_location_fields_without_chapter_metadata_remain_readable() {
        let location = parse_location_fields("book.txt\tBook\ttxt\t10\t0\t2\t5").unwrap();
        assert_eq!(location.format, BookFormat::Text);
        assert_eq!(location.epub_chapter, None);
    }

    /// Read an `EpubDocument`'s full flattened text regardless of whether it
    /// is still RAM-resident or already backed by its `.EPX` cache file.
    fn full_epub_text(document: &EpubDocument) -> String {
        let len = usize::try_from(document.text_size_bytes()).unwrap();
        String::from_utf8(document.text_window(0, len).unwrap().into_owned()).unwrap()
    }

    fn epub_document_fixture() -> EpubDocument {
        EpubDocument::from_resident_for_test(
            "Title\twith\ttabs, \\ backslash and \"quotes\"".into(),
            "Chapter one.\n\nChapter two with\ttab and \\ backslash.".into(),
            vec![EpubTocEntry {
                label: "Start\nlabel".into(),
                text_offset: 0,
                spine_index: 0,
            }],
            vec![
                EpubChapter {
                    number: 1,
                    label: "Chapter\tOne".into(),
                    text_offset: 0,
                    text_end_offset: 12,
                    spine_index: 0,
                },
                EpubChapter {
                    number: 2,
                    label: "Chapter Two".into(),
                    text_offset: 14,
                    text_end_offset: 51,
                    spine_index: 1,
                },
            ],
            vec![EpubImage {
                href: "OEBPS/images/fig\t1.jpg".into(),
                alt: "A \\ backslash and \"quotes\"".into(),
                text_offset: 6,
                spine_index: 0,
                width: 640,
                height: 480,
            }],
            2,
        )
    }

    fn epub_cache_book_fixture() -> ReaderBook {
        ReaderBook {
            path: "Sample.epub".into(),
            title: "Sample".into(),
            format: BookFormat::Epub,
            size_bytes: 4096,
            modified_seconds: 12,
        }
    }

    /// Write `content` to `dir`'s `.EPX` cache file path for `book` and parse
    /// it back exactly as [`ReaderUiState::load_epub_document_cache_best_effort`]
    /// would (bounded header read, then [`parse_epub_document_cache`] against
    /// the real file), so these unit tests exercise the same path production
    /// does instead of calling the parser on an in-memory string.
    fn write_and_load_epub_cache(
        dir: &Path,
        book: &ReaderBook,
        content: &str,
    ) -> Result<EpubDocument, String> {
        let path = dir.join("cache.EPX");
        fs::write(&path, content).unwrap();
        let header = read_epub_cache_header(&path)?;
        parse_epub_document_cache(&header, book, &path)
    }

    #[test]
    fn epub_document_cache_round_trips_through_serialize_and_parse() {
        let dir = temp_dir("epub-doc-cache-roundtrip");
        let book = epub_cache_book_fixture();
        let document = epub_document_fixture();
        let fingerprint = epub_document_fingerprint(&book);
        let (serialized, _body_offset) =
            serialize_epub_document_cache(&document, fingerprint).unwrap();
        let loaded = write_and_load_epub_cache(&dir, &book, &serialized).unwrap();
        assert_eq!(loaded.title, document.title);
        assert_eq!(loaded.toc, document.toc);
        assert_eq!(loaded.chapters, document.chapters);
        assert_eq!(loaded.spine_count, document.spine_count);
        assert_eq!(loaded.text_size_bytes(), document.text_size_bytes());
        assert_eq!(full_epub_text(&loaded), full_epub_text(&document));
    }

    #[test]
    fn epub_document_cache_rejects_fingerprint_mismatch() {
        let dir = temp_dir("epub-doc-cache-fingerprint-mismatch");
        let book = epub_cache_book_fixture();
        let mut other = book.clone();
        other.size_bytes = book.size_bytes + 1;
        let document = epub_document_fixture();
        let (serialized, _body_offset) =
            serialize_epub_document_cache(&document, epub_document_fingerprint(&book)).unwrap();
        assert!(write_and_load_epub_cache(&dir, &other, &serialized).is_err());
    }

    #[test]
    fn epub_document_cache_rejects_truncated_text_payload() {
        let dir = temp_dir("epub-doc-cache-truncated");
        let book = epub_cache_book_fixture();
        let document = epub_document_fixture();
        let (serialized, _body_offset) =
            serialize_epub_document_cache(&document, epub_document_fingerprint(&book)).unwrap();
        let truncated = &serialized[..serialized.len() - 5];
        assert!(write_and_load_epub_cache(&dir, &book, truncated).is_err());
    }

    fn stored_epub_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        fn push_u16(output: &mut Vec<u8>, value: u16) {
            output.extend(value.to_le_bytes());
        }
        fn push_u32(output: &mut Vec<u8>, value: u32) {
            output.extend(value.to_le_bytes());
        }
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

    fn write_sample_epub(path: &Path) {
        let bytes = stored_epub_zip(&[
            (
                "META-INF/container.xml",
                "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>",
            ),
            (
                "OEBPS/book.opf",
                "<package><metadata><dc:title>Cache Sample</dc:title></metadata><manifest><item id='nav' href='nav.xhtml' media-type='application/xhtml+xml' properties='nav'/><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/></spine></package>",
            ),
            (
                "OEBPS/nav.xhtml",
                "<nav><ol><li><a href='c1.xhtml'>Start</a></li></ol></nav>",
            ),
            (
                "OEBPS/c1.xhtml",
                "<html><body><h1>Start</h1><p>Cached chapter body.</p></body></html>",
            ),
        ]);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn warm_cache_is_reported_only_for_epubs_with_an_epx_on_disk() {
        let root = temp_dir("warm-open-books");
        let state = temp_dir("warm-open-state");
        write_sample_epub(&root.join("Sample.epub"));
        let open_reader = || {
            let mut reader = ReaderUiState::with_roots(
                root.to_string_lossy().into_owned(),
                state.to_string_lossy().into_owned(),
            );
            reader.refresh_library();
            reader.library_selected = 0;
            assert!(reader.apply_library_button(ButtonEvent::Select));
            reader
        };

        let mut reader = open_reader();
        assert!(!reader.pending_open_has_warm_cache(), "first open has no .EPX yet");
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        assert!(!reader.pending_open_has_warm_cache(), "nothing pending once open");
        reader.tick(); // deferred .EPX write

        let reader = open_reader();
        assert!(reader.pending_open_has_warm_cache());

        let txt_root = temp_dir("warm-open-txt-books");
        let txt_state = temp_dir("warm-open-txt-state");
        write_sequential_txt_books(&txt_root, &["Book.txt"]);
        let mut txt_reader = ReaderUiState::with_roots(
            txt_root.to_string_lossy().into_owned(),
            txt_state.to_string_lossy().into_owned(),
        );
        txt_reader.refresh_library();
        let book = txt_reader.books[0].clone();
        txt_reader.request_open_book(book, None);
        assert!(!txt_reader.pending_open_has_warm_cache());
    }

    #[test]
    fn epub_reopen_hits_flattened_text_cache_and_skips_reparsing() {
        let root = temp_dir("epub-cache-books");
        let state = temp_dir("epub-cache-state");
        write_sample_epub(&root.join("Sample.epub"));

        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        let first_text = full_epub_text(
            reader
                .session
                .as_ref()
                .unwrap()
                .epub_document
                .as_ref()
                .unwrap(),
        );
        assert!(first_text.contains("Cached chapter body."));

        // The `.EPX` write is deferred to the first background tick after
        // the page is shown (see `epub_document_cache_pending`), so it isn't
        // on disk yet right after `FirstPageReady` itself.
        assert!(reader.session.as_ref().unwrap().epub_document_cache_pending);
        reader.tick();
        assert!(!reader.session.as_ref().unwrap().epub_document_cache_pending);

        // Once persisted, the live session's document drops its RAM copy of
        // the flattened text and reads back through the `.EPX` file it just
        // wrote — freeing that RAM for the rest of the reading session while
        // still paginating correctly.
        let persisted_document = reader
            .session
            .as_ref()
            .unwrap()
            .epub_document
            .as_ref()
            .unwrap();
        assert!(persisted_document.resident_text().is_none());
        assert_eq!(full_epub_text(persisted_document), first_text);

        let cache_dir = state.join(READER_CACHE_DIRECTORY);
        let epx_files: Vec<_> = fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("EPX")
            })
            .collect();
        assert_eq!(epx_files.len(), 1);

        let mut reopened = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reopened.refresh_library();
        reopened.library_selected = 0;
        assert!(reopened.apply_library_button(ButtonEvent::Select));
        let mut guard = 0;
        loop {
            reopened.tick();
            guard += 1;
            assert!(guard < 10, "loading never reached ReadingEpubPackage");
            if reopened.loading_stage() == Some(ReaderLoadingStage::ReadingEpubPackage) {
                break;
            }
        }
        let inspect_message = reopened.loading.as_ref().unwrap().message.clone();
        assert!(
            inspect_message.contains("cached"),
            "expected cache-hit message, got: {inspect_message}"
        );
        while reopened.tick() != ReaderTickOutcome::FirstPageReady {}
        let cached_text = full_epub_text(
            reopened
                .session
                .as_ref()
                .unwrap()
                .epub_document
                .as_ref()
                .unwrap(),
        );
        assert_eq!(cached_text, first_text);

        let epx_files_after: Vec<_> = fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("EPX")
            })
            .collect();
        assert_eq!(epx_files_after.len(), 1);
    }

    fn write_multi_page_sample_epub(path: &Path) {
        let filler = "Filler paragraph text repeated many times to force multiple printed pages of pagination. ".repeat(80);
        let chapter = format!("<html><body><h1>Start</h1><p>{filler}</p></body></html>");
        let bytes = stored_epub_zip(&[
            (
                "META-INF/container.xml",
                "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>",
            ),
            (
                "OEBPS/book.opf",
                "<package><metadata><dc:title>Multi Page Sample</dc:title></metadata><manifest><item id='nav' href='nav.xhtml' media-type='application/xhtml+xml' properties='nav'/><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/></spine></package>",
            ),
            (
                "OEBPS/nav.xhtml",
                "<nav><ol><li><a href='c1.xhtml'>Start</a></li></ol></nav>",
            ),
            ("OEBPS/c1.xhtml", &chapter),
        ]);
        fs::write(path, bytes).unwrap();
    }

    /// A cache hit must surface exactly what was written to disk, not a value
    /// that happens to match a correct recomputation. This test tampers with
    /// the on-disk `.EPP` page-index cache (drops the chapter's last page
    /// anchor) after the first open, then reopens the book and asserts the
    /// wrong, tampered page count comes back. If pagination were silently
    /// recomputed instead of read from cache, this would fail: recomputation
    /// would restore the original, correct page count.
    #[test]
    fn epub_reopen_uses_persisted_page_index_cache_instead_of_recomputing() {
        let root = temp_dir("epub-page-cache-books");
        let state = temp_dir("epub-page-cache-state");
        let path = root.join("Sample.epub");
        write_multi_page_sample_epub(&path);

        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        // The fresh open only reads the first page synchronously (see
        // `open_epub_session`); drive background indexing the rest of the
        // way so `page_offsets`/the persisted `.EPP` reflect the whole book,
        // exactly like a session that's been read to completion would.
        let mut guard = 0;
        while !reader.session.as_ref().unwrap().index_complete {
            reader.tick();
            guard += 1;
            assert!(guard < 200, "background EPUB indexing never completed");
        }
        let session = reader.session.as_ref().unwrap();
        let book = session.book.clone();
        let original_page_count = session.page_offsets.len();
        assert!(
            original_page_count > 1,
            "fixture must paginate to more than one page to make tampering observable"
        );
        let layout = reader.preferences.layout();

        let epp_path = reader.epub_page_index_cache_path_for(&book, layout);
        assert!(epp_path.exists());
        let mut cached = load_epub_page_index_cache(&epp_path, &book, layout)
            .unwrap()
            .unwrap();
        let last_chapter = cached.last_mut().unwrap();
        assert!(
            last_chapter.page_offsets.len() > 1,
            "fixture chapter must have more than one page"
        );
        last_chapter.page_offsets.pop();
        let fingerprint = book_fingerprint(&book, layout);
        atomic_replace_text(
            &epp_path,
            &serialize_epub_page_index_cache(&cached, fingerprint),
        )
        .unwrap();

        let mut reopened = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reopened.refresh_library();
        reopened.library_selected = 0;
        assert!(reopened.apply_library_button(ButtonEvent::Select));
        while reopened.tick() != ReaderTickOutcome::FirstPageReady {}
        let reopened_page_count = reopened.session.as_ref().unwrap().page_offsets.len();
        assert_eq!(reopened_page_count, original_page_count - 1);
    }

    fn write_multi_chapter_sample_epub(path: &Path, chapter_labels: &[&str]) {
        let manifest_items: String = (1..=chapter_labels.len())
            .map(|index| {
                format!("<item id='c{index}' href='c{index}.xhtml' media-type='application/xhtml+xml'/>")
            })
            .collect();
        let spine_items: String = (1..=chapter_labels.len())
            .map(|index| format!("<itemref idref='c{index}'/>"))
            .collect();
        let opf = format!(
            "<package><metadata><dc:title>Multi Chapter Sample</dc:title></metadata><manifest><item id='nav' href='nav.xhtml' media-type='application/xhtml+xml' properties='nav'/>{manifest_items}</manifest><spine>{spine_items}</spine></package>"
        );
        let nav_items: String = chapter_labels
            .iter()
            .enumerate()
            .map(|(index, label)| format!("<li><a href='c{}.xhtml'>{label}</a></li>", index + 1))
            .collect();
        let nav = format!("<nav><ol>{nav_items}</ol></nav>");
        let chapters: Vec<(String, String)> = chapter_labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                (
                    format!("OEBPS/c{}.xhtml", index + 1),
                    format!(
                        "<html><body><h1>{label}</h1><p>Body text for {label}.</p></body></html>"
                    ),
                )
            })
            .collect();
        let mut entries: Vec<(&str, &str)> = vec![
            (
                "META-INF/container.xml",
                "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>",
            ),
            ("OEBPS/book.opf", &opf),
            ("OEBPS/nav.xhtml", &nav),
        ];
        entries.extend(
            chapters
                .iter()
                .map(|(name, body)| (name.as_str(), body.as_str())),
        );
        let bytes = stored_epub_zip(&entries);
        fs::write(path, bytes).unwrap();
    }

    /// The whole point of the lazy pagination: `BuildingFirstPage` must not
    /// block on paginating chapters the reader hasn't reached yet. Only the
    /// chapter containing the first page may be paginated synchronously;
    /// everything else grows one chapter per `tick()` afterwards.
    #[test]
    fn epub_fresh_open_paginates_only_the_first_chapter_and_defers_the_rest() {
        let root = temp_dir("epub-lazy-open-books");
        let state = temp_dir("epub-lazy-open-state");
        let path = root.join("Sample.epub");
        write_multi_chapter_sample_epub(&path, &["Chapter One", "Chapter Two", "Chapter Three"]);

        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}

        {
            let session = reader.session.as_ref().unwrap();
            assert_eq!(
                session.page_offsets.len(),
                1,
                "only the requested page should be read synchronously"
            );
            assert_eq!(
                session.epub_chapter_pages.len(),
                0,
                "no chapter should be finalized before it is fully paginated"
            );
            assert!(
                !session.index_complete,
                "later chapters must not be indexed before the first page is shown"
            );
            let pending = session
                .epub_pending_chapter
                .as_ref()
                .expect("the first chapter should still be indexing in the background");
            assert_eq!(pending.chapter.number, 1);
        }
        let (book, layout) = {
            let session = reader.session.as_ref().unwrap();
            (session.book.clone(), session.layout)
        };
        let epp_path = reader.epub_page_index_cache_path_for(&book, layout);
        assert!(
            !epp_path.exists(),
            "a partial index must not be persisted as the full-book cache"
        );

        let mut guard = 0;
        while !reader.session.as_ref().unwrap().index_complete {
            reader.tick();
            guard += 1;
            assert!(guard < 50, "background EPUB indexing never completed");
        }
        assert_eq!(reader.session.as_ref().unwrap().epub_chapter_pages.len(), 3);
        assert!(
            epp_path.exists(),
            "a book indexed from its true start should persist the completed cache"
        );
    }

    /// Reading percent used to require `index_complete` (forward indexing
    /// having walked all the way to the book's true end) before it would
    /// report anything but `None` ("--%"). Since background indexing only
    /// ever advances a little ahead of where the reader is, `index_complete`
    /// stays false for most of a session on anything but a short book,
    /// leaving the percentage stuck at "--%" for that whole time. It must
    /// fall back to current-byte-offset versus total-byte-size instead,
    /// which is known immediately (no extra pagination work, so this must
    /// not cost anything at load time).
    #[test]
    fn epub_reading_percent_reports_a_byte_offset_estimate_before_the_index_is_complete() {
        let root = temp_dir("epub-percent-books");
        let state = temp_dir("epub-percent-state");
        let path = root.join("Sample.epub");
        write_multi_chapter_sample_epub(&path, &["Chapter One", "Chapter Two", "Chapter Three"]);

        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}

        let session = reader.session.as_ref().unwrap();
        assert!(
            !session.index_complete,
            "this test only means something before the exact page-based percentage is available"
        );
        let percent = session
            .reading_percent()
            .expect("byte-offset fallback must report a percentage instead of '--%'");
        assert!(percent <= 100);
        assert_eq!(session.reading_percent_label(), format!("{percent}%"));
    }

    /// A fully warm reopen (both `.EPX` and `.EPP` caches present, resuming a
    /// saved position) used to cost one `tick()` — a 250ms floor plus a full
    /// e-paper redraw — per loading stage, most of which are pure bookkeeping
    /// with nothing worth showing on screen. It must now collapse to 3:
    /// `OpeningFile` (kicks off the "inspecting" message), the cached archive
    /// inspection (shows the cache-hit message), then everything else
    /// chained straight through to `FirstPageReady`.
    #[test]
    fn epub_warm_reopen_collapses_bookkeeping_stages_into_few_ticks() {
        let root = temp_dir("epub-warm-reopen-books");
        let state = temp_dir("epub-warm-reopen-state");
        let path = root.join("Sample.epub");
        write_multi_chapter_sample_epub(&path, &["Chapter One", "Chapter Two", "Chapter Three"]);

        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        // Drive background indexing to completion so both `.EPX` and `.EPP`
        // are warm on disk for the reopen below.
        let mut guard = 0;
        while !reader.session.as_ref().unwrap().index_complete {
            reader.tick();
            guard += 1;
            assert!(guard < 50, "background EPUB indexing never completed");
        }
        reader.next_page();

        let mut reopened = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reopened.load_persistent_state();
        reopened.refresh_library();
        reopened.library_selected = 0;
        assert!(reopened.request_continue());

        let mut ticks = 0;
        loop {
            let outcome = reopened.tick();
            ticks += 1;
            assert!(ticks < 10, "warm reopen took too many ticks");
            if outcome == ReaderTickOutcome::FirstPageReady {
                break;
            }
        }
        assert_eq!(
            ticks, 3,
            "a fully warm EPUB reopen should need only 3 ticks: OpeningFile, \
             the cached archive inspection, then everything else chained"
        );
    }

    /// A first cut of this fix indexed one whole chapter per background step,
    /// which was still perceptible as a stuck next-page button on a chapter
    /// with many pages (or on any single very large chapter). Every step —
    /// background `tick()` or an on-demand `next_page()` — must cost at most
    /// one page's word-wrap, matching TXT, even mid-chapter.
    #[test]
    fn epub_background_indexing_advances_at_most_one_page_per_tick() {
        let root = temp_dir("epub-page-granular-books");
        let state = temp_dir("epub-page-granular-state");
        let path = root.join("Sample.epub");
        write_multi_page_sample_epub(&path);

        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        reader.library_selected = 0;
        assert!(reader.apply_library_button(ButtonEvent::Select));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}

        assert_eq!(reader.session.as_ref().unwrap().page_offsets.len(), 1);
        let mut previous = 1;
        let mut saw_growth = false;
        for _ in 0..40 {
            if reader.session.as_ref().unwrap().index_complete {
                break;
            }
            reader.tick();
            let current = reader.session.as_ref().unwrap().page_offsets.len();
            assert!(
                current <= previous + 1,
                "a single tick must add at most one page (got {previous} -> {current})"
            );
            saw_growth |= current > previous;
            previous = current;
        }
        assert!(saw_growth, "background indexing never advanced");
        assert!(
            previous > 1,
            "fixture must paginate to more than one page to make this observable"
        );
    }

    /// A resume that lands mid-book with no matching `.EPP` cache (e.g. a
    /// font/orientation change just invalidated it) must show the target
    /// chapter immediately without indexing the chapters before it. Once
    /// background indexing then walks forward to the book's end, the
    /// resulting chapter-two-through-end run *is* persisted (each chapter
    /// carries its own offsets, so a mid-book-started run is safe to cache
    /// on disk) — this is what lets a later reopen at the same resume point
    /// skip synchronous pagination entirely (see the follow-up test below).
    #[test]
    fn epub_resume_after_cache_miss_paginates_only_the_target_chapter_then_persists_the_run_it_walks(
    ) {
        let root = temp_dir("epub-resume-cache-miss-books");
        let warm_state = temp_dir("epub-resume-cache-miss-warm-state");
        let state = temp_dir("epub-resume-cache-miss-state");
        let path = root.join("Sample.epub");
        write_multi_chapter_sample_epub(&path, &["Chapter One", "Chapter Two", "Chapter Three"]);

        // Learn the real chapter boundaries and the book's resolved identity
        // via a normal, fully-indexed open.
        let mut warm = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            warm_state.to_string_lossy().into_owned(),
        );
        warm.refresh_library();
        warm.library_selected = 0;
        assert!(warm.apply_library_button(ButtonEvent::Select));
        while warm.tick() != ReaderTickOutcome::FirstPageReady {}
        let (book, second_chapter_offset) = {
            let session = warm.session.as_ref().unwrap();
            let offset = session.epub_document.as_ref().unwrap().chapters[1].text_offset;
            (session.book.clone(), offset)
        };

        // Reopen in a state directory with no `.EPP` cache yet, resuming
        // directly into chapter two.
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        let resume = ReaderLocation {
            path: book.path.clone(),
            title: book.title.clone(),
            format: book.format,
            size_bytes: book.size_bytes,
            modified_seconds: book.modified_seconds,
            page_index: 5,
            byte_offset: second_chapter_offset,
            epub_chapter: None,
            reading_percent: None,
        };
        reader.request_open_book(book.clone(), Some(resume));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}

        let layout = {
            let session = reader.session.as_ref().unwrap();
            assert_eq!(
                session.page_offsets,
                vec![second_chapter_offset],
                "only the requested page should be read synchronously"
            );
            assert_eq!(session.epub_chapter_pages.len(), 0);
            assert!(!session.index_complete);
            let pending = session
                .epub_pending_chapter
                .as_ref()
                .expect("chapter two should still be indexing in the background");
            assert_eq!(pending.chapter.number, 2);
            session.layout
        };
        let epp_path = reader.epub_page_index_cache_path_for(&book, layout);
        assert!(!epp_path.exists());

        let mut guard = 0;
        while !reader.session.as_ref().unwrap().index_complete {
            reader.tick();
            guard += 1;
            assert!(guard < 50, "background EPUB indexing never completed");
        }
        assert!(
            epp_path.exists(),
            "a mid-book-started run that reached the book's end should still be persisted, \
             since each cached chapter is self-describing on disk"
        );
    }

    /// Once a mid-book-started run has been persisted (previous test), a
    /// later reopen resuming at the *same* offset must hit that cache and
    /// skip synchronous pagination, while a reopen at an offset the cached
    /// run never covered (a stale resume from a different part of the book)
    /// must fall back to a fresh single-chapter pagination exactly as if no
    /// `.EPP` existed, rather than misusing the unrelated cached range.
    #[test]
    fn epub_reopen_at_a_previously_persisted_mid_book_offset_hits_cache_but_other_offsets_still_miss(
    ) {
        let root = temp_dir("epub-mid-book-cache-hit-books");
        let warm_state = temp_dir("epub-mid-book-cache-hit-warm-state");
        let path = root.join("Sample.epub");
        write_multi_chapter_sample_epub(&path, &["Chapter One", "Chapter Two", "Chapter Three"]);

        let mut warm = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            warm_state.to_string_lossy().into_owned(),
        );
        warm.refresh_library();
        warm.library_selected = 0;
        assert!(warm.apply_library_button(ButtonEvent::Select));
        while warm.tick() != ReaderTickOutcome::FirstPageReady {}
        let (book, second_chapter_offset, first_chapter_offset) = {
            let session = warm.session.as_ref().unwrap();
            let document = session.epub_document.as_ref().unwrap();
            (
                session.book.clone(),
                document.chapters[1].text_offset,
                document.chapters[0].text_offset,
            )
        };

        // First reopen: no cache yet, resumes into chapter two, and walks
        // forward in the background until it persists (mirrors the previous
        // test exactly, just reusing its outcome as this test's setup).
        let mid_book_state = temp_dir("epub-mid-book-cache-hit-state");
        let mut seeding = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            mid_book_state.to_string_lossy().into_owned(),
        );
        seeding.refresh_library();
        let resume_chapter_two = ReaderLocation {
            path: book.path.clone(),
            title: book.title.clone(),
            format: book.format,
            size_bytes: book.size_bytes,
            modified_seconds: book.modified_seconds,
            page_index: 5,
            byte_offset: second_chapter_offset,
            epub_chapter: None,
            reading_percent: None,
        };
        seeding.request_open_book(book.clone(), Some(resume_chapter_two.clone()));
        while seeding.tick() != ReaderTickOutcome::FirstPageReady {}
        let mut guard = 0;
        while !seeding.session.as_ref().unwrap().index_complete {
            seeding.tick();
            guard += 1;
            assert!(guard < 50, "background EPUB indexing never completed");
        }
        drop(seeding);

        // Second reopen, same state directory, same resume offset: must be
        // a cache hit — the whole chapter-two-through-end run is available
        // synchronously, no `epub_pending_chapter` left in progress.
        let mut hit = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            mid_book_state.to_string_lossy().into_owned(),
        );
        hit.refresh_library();
        hit.request_open_book(book.clone(), Some(resume_chapter_two));
        while hit.tick() != ReaderTickOutcome::FirstPageReady {}
        {
            let session = hit.session.as_ref().unwrap();
            assert!(
                session.page_offsets.len() > 1,
                "a cache hit should seed every already-known page, not just the resume page"
            );
            assert!(session.index_complete);
            assert!(session.epub_pending_chapter.is_none());
            // Regression check: `index_complete` here only means indexing
            // reached the book's *end* — the persisted run still starts at
            // chapter two, so chapter one's pages were never counted.
            // Trusting the page-count formula would divide a near-zero
            // "current page" (counted from chapter two, not the book's
            // start) by an undercounted total, reporting close to 0% for a
            // book the reader is really about a third of the way into.
            let percent = session
                .reading_percent()
                .expect("book byte size is known once opened");
            assert!(
                percent >= 20,
                "resuming into a persisted run that starts mid-book (missing chapter \
                 one) must still report progress from the true byte offset, not from \
                 the run's own start; got {percent}%"
            );
        }
        drop(hit);

        // Third reopen, same state directory, but resuming into chapter one
        // instead — outside the persisted chapter-two-through-end run, so
        // this must fall back to a fresh single-chapter pagination rather
        // than misinterpreting the unrelated cached range.
        let mut miss = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            mid_book_state.to_string_lossy().into_owned(),
        );
        miss.refresh_library();
        let resume_chapter_one = ReaderLocation {
            path: book.path.clone(),
            title: book.title.clone(),
            format: book.format,
            size_bytes: book.size_bytes,
            modified_seconds: book.modified_seconds,
            page_index: 0,
            byte_offset: first_chapter_offset,
            epub_chapter: None,
            reading_percent: None,
        };
        miss.request_open_book(book.clone(), Some(resume_chapter_one));
        while miss.tick() != ReaderTickOutcome::FirstPageReady {}
        {
            let session = miss.session.as_ref().unwrap();
            assert_eq!(
                session.page_offsets,
                vec![first_chapter_offset],
                "an offset outside the cached run must still fall back to a one-page open"
            );
            let pending = session
                .epub_pending_chapter
                .as_ref()
                .expect("chapter one should still be indexing in the background");
            assert_eq!(pending.chapter.number, 1);
        }
    }

    fn write_two_chapter_epub_with_long_second_chapter(path: &Path) {
        let filler = "Filler paragraph text repeated many times to force multiple printed pages of pagination. ".repeat(80);
        let chapter_one =
            "<html><body><h1>Chapter One</h1><p>Short chapter one body.</p></body></html>"
                .to_string();
        let chapter_two = format!("<html><body><h1>Chapter Two</h1><p>{filler}</p></body></html>");
        let bytes = stored_epub_zip(&[
            (
                "META-INF/container.xml",
                "<container><rootfiles><rootfile full-path='OEBPS/book.opf'/></rootfiles></container>",
            ),
            (
                "OEBPS/book.opf",
                "<package><metadata><dc:title>Backward Nav Sample</dc:title></metadata><manifest><item id='nav' href='nav.xhtml' media-type='application/xhtml+xml' properties='nav'/><item id='c1' href='c1.xhtml' media-type='application/xhtml+xml'/><item id='c2' href='c2.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c1'/><itemref idref='c2'/></spine></package>",
            ),
            (
                "OEBPS/nav.xhtml",
                "<nav><ol><li><a href='c1.xhtml'>Chapter One</a></li><li><a href='c2.xhtml'>Chapter Two</a></li></ol></nav>",
            ),
            ("OEBPS/c1.xhtml", &chapter_one),
            ("OEBPS/c2.xhtml", &chapter_two),
        ]);
        fs::write(path, bytes).unwrap();
    }

    /// The exact bug this test guards against: a book cache is cleared (or a
    /// font/layout change invalidates it), the reader resumes mid-chapter as
    /// before, and pressing "previous page" from that very first page does
    /// nothing — there was no way back past the page the session was seeded
    /// with. It must now walk back through the pages already read before
    /// this session (within the resumed chapter, then across into the
    /// previous chapter too), exactly as if they had been indexed forward.
    #[test]
    fn epub_resume_mid_chapter_can_page_backward_through_already_read_pages_and_into_the_previous_chapter(
    ) {
        let root = temp_dir("epub-backward-nav-books");
        let warm_state = temp_dir("epub-backward-nav-warm-state");
        let state = temp_dir("epub-backward-nav-state");
        let path = root.join("Sample.epub");
        write_two_chapter_epub_with_long_second_chapter(&path);

        // Learn chapter two's real page offsets via a normal, fully-indexed
        // open.
        let mut warm = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            warm_state.to_string_lossy().into_owned(),
        );
        warm.refresh_library();
        warm.library_selected = 0;
        assert!(warm.apply_library_button(ButtonEvent::Select));
        while warm.tick() != ReaderTickOutcome::FirstPageReady {}
        let mut guard = 0;
        while !warm.session.as_ref().unwrap().index_complete {
            warm.tick();
            guard += 1;
            assert!(guard < 50, "background EPUB indexing never completed");
        }
        let (book, chapter_two_pages) = {
            let session = warm.session.as_ref().unwrap();
            let chapter_two = session
                .epub_chapter_pages
                .iter()
                .find(|chapter| chapter.chapter_number == 2)
                .unwrap()
                .clone();
            (session.book.clone(), chapter_two)
        };
        assert!(
            chapter_two_pages.page_offsets.len() > 2,
            "fixture must paginate chapter two to more than two pages"
        );
        let resume_offset = chapter_two_pages.page_offsets[2];

        // Reopen in a state directory with no `.EPP` cache yet, resuming
        // into the third page of chapter two — mid-chapter, not its start.
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        let resume = ReaderLocation {
            path: book.path.clone(),
            title: book.title.clone(),
            format: book.format,
            size_bytes: book.size_bytes,
            modified_seconds: book.modified_seconds,
            page_index: 20,
            byte_offset: resume_offset,
            epub_chapter: None,
            reading_percent: None,
        };
        reader.request_open_book(book.clone(), Some(resume));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}

        {
            let session = reader.session.as_ref().unwrap();
            assert_eq!(
                session.current_page, 2,
                "the resume page should already know its two preceding pages within chapter two"
            );
            assert_eq!(session.page_offsets.len(), 3);
        }

        // Step back within chapter two first: no chapter crossing needed yet.
        reader.previous_page();
        reader.previous_page();
        assert_eq!(reader.session.as_ref().unwrap().current_page, 0);

        // One more step must cross into chapter one, until now completely
        // unindexed, instead of silently doing nothing. Chapter one is short
        // enough to be a single page, so `current_page` lands back at 0 —
        // what matters is that `page_offsets` grew and now points into
        // chapter one instead of the step being a silent no-op.
        reader.previous_page();
        let session = reader.session.as_ref().unwrap();
        assert!(
            session.page_offsets.len() > 3,
            "crossing into chapter one should have prepended at least one page"
        );
        let offset = session.page_offsets[session.current_page];
        let chapter = session
            .epub_document
            .as_ref()
            .unwrap()
            .chapter_for_offset(offset)
            .unwrap();
        assert_eq!(chapter.number, 1, "should now be positioned in chapter one");
    }

    #[test]
    fn opening_another_book_releases_the_active_session_before_loading() {
        let root = temp_dir("release-session-books");
        let state = temp_dir("release-session-state");
        let first = root.join("First.txt");
        let second = root.join("Second.txt");
        fs::write(&first, "first book body ".repeat(100)).unwrap();
        fs::write(&second, "second book body ".repeat(100)).unwrap();
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        let first_path = first.to_string_lossy();
        let second_path = second.to_string_lossy();
        let first_book = reader
            .books
            .iter()
            .find(|book| book.path == first_path.as_ref())
            .unwrap()
            .clone();
        let second_book = reader
            .books
            .iter()
            .find(|book| book.path == second_path.as_ref())
            .unwrap()
            .clone();
        reader.request_open_book(first_book, None);
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        assert!(reader.session.is_some());
        reader.request_open_book(second_book, None);
        assert!(reader.session.is_none());
        assert_eq!(
            reader.loading_stage(),
            Some(ReaderLoadingStage::OpeningFile)
        );
    }

    fn write_sequential_txt_books(root: &std::path::Path, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| {
                let path = root.join(name);
                fs::write(&path, format!("{name} body ").repeat(200)).unwrap();
                path
            })
            .collect()
    }

    #[test]
    fn reselecting_a_parked_book_is_instant_with_no_reload() {
        let root = temp_dir("session-cache-reselect-books");
        let state = temp_dir("session-cache-reselect-state");
        let paths = write_sequential_txt_books(&root, &["First.txt", "Second.txt"]);
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        // Recent reorders on every open, which shifts `visible_entries()`
        // indices, so look each one up fresh right before use rather than
        // reusing an index computed earlier.
        let index_of = |reader: &ReaderUiState, path: &std::path::Path| {
            reader
                .visible_entries()
                .iter()
                .position(|entry| entry.book.path == path.to_string_lossy())
                .unwrap()
        };

        assert!(reader.request_open_visible(index_of(&reader, &paths[0])));
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        assert_eq!(
            reader.session.as_ref().unwrap().book.path,
            paths[0].to_string_lossy()
        );

        // Switching to the second book parks the first instead of dropping it.
        assert!(reader.request_open_visible(index_of(&reader, &paths[1])));
        assert!(
            reader.loading.is_some(),
            "a genuinely different, never-cached book still needs a real reload"
        );
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        assert_eq!(reader.session_cache.len(), 1);
        assert_eq!(
            reader.session_cache[0].book.path,
            paths[0].to_string_lossy()
        );

        // Reselecting the first book is a zero-I/O swap: no `loading` stage.
        assert!(reader.request_open_visible(index_of(&reader, &paths[0])));
        assert!(reader.loading.is_none());
        assert_eq!(
            reader.session.as_ref().unwrap().book.path,
            paths[0].to_string_lossy()
        );
        assert_eq!(reader.session_cache.len(), 1);
        assert_eq!(
            reader.session_cache[0].book.path,
            paths[1].to_string_lossy()
        );
    }

    #[test]
    fn parked_session_cache_evicts_the_oldest_past_the_limit() {
        let root = temp_dir("session-cache-eviction-books");
        let state = temp_dir("session-cache-eviction-state");
        let names = ["One.txt", "Two.txt", "Three.txt", "Four.txt", "Five.txt"];
        let paths = write_sequential_txt_books(&root, &names);
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        for path in &paths {
            let book = reader
                .books
                .iter()
                .find(|book| book.path == path.to_string_lossy())
                .unwrap()
                .clone();
            reader.request_open_book(book, None);
            while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        }
        assert_eq!(
            reader.session.as_ref().unwrap().book.path,
            paths[4].to_string_lossy()
        );
        assert_eq!(
            reader.session_cache.len(),
            READER_SESSION_CACHE_LIMIT,
            "cache must stay bounded at READER_SESSION_CACHE_LIMIT"
        );
        let cached_paths: Vec<_> = reader
            .session_cache
            .iter()
            .map(|session| session.book.path.clone())
            .collect();
        assert_eq!(
            cached_paths,
            vec![
                paths[3].to_string_lossy().into_owned(),
                paths[2].to_string_lossy().into_owned(),
                paths[1].to_string_lossy().into_owned(),
            ],
            "most-recently-parked first, oldest (`One.txt`) evicted"
        );
    }

    #[test]
    fn request_continue_promotes_a_parked_session_instead_of_reloading() {
        let root = temp_dir("session-cache-continue-books");
        let state = temp_dir("session-cache-continue-state");
        let paths = write_sequential_txt_books(&root, &["First.txt", "Second.txt"]);
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        let first_book = reader
            .books
            .iter()
            .find(|book| book.path == paths[0].to_string_lossy())
            .unwrap()
            .clone();
        let second_book = reader
            .books
            .iter()
            .find(|book| book.path == paths[1].to_string_lossy())
            .unwrap()
            .clone();
        reader.request_open_book(first_book.clone(), None);
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        reader.request_open_book(second_book, None);
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        assert_eq!(reader.session_cache.len(), 1);

        // `resume` normally tracks whatever was read most recently (here,
        // the second book); point it back at the parked first book to
        // exercise `request_continue`'s cache-promotion path deliberately.
        reader.resume = Some(reader.session_cache[0].current_location());

        assert!(reader.request_continue());
        assert!(
            reader.loading.is_none(),
            "a parked book must resume with no reload"
        );
        assert_eq!(reader.session.as_ref().unwrap().book.path, first_book.path);
    }

    /// Write `header` (which must end with the text marker) followed by
    /// `body_bytes` of filler, and return what `read_epub_cache_header` makes
    /// of it.
    fn read_header_from(name: &str, header: &[u8], body_bytes: usize) -> Result<String, String> {
        let dir = temp_dir(name);
        let path = dir.join("TEST.EPX");
        let mut content = header.to_vec();
        content.extend(std::iter::repeat(b'x').take(body_bytes));
        fs::write(&path, content).unwrap();
        super::read_epub_cache_header(&path)
    }

    /// A header of exactly `total_len` bytes ending in the text marker.
    fn header_of_len(total_len: usize) -> Vec<u8> {
        let marker = super::EPUB_CACHE_TEXT_MARKER;
        let mut header = b"version=1\ntitle=".to_vec();
        header.resize(total_len - marker.len() - 1, b't');
        header.push(b'\n');
        header.extend_from_slice(marker);
        header
    }

    #[test]
    fn epub_cache_header_stops_at_the_marker_without_the_body() {
        let header = header_of_len(900);
        let read = read_header_from("epx-header-small", &header, 600 * 1024).unwrap();
        assert_eq!(read.as_bytes(), header.as_slice());
    }

    #[test]
    fn epub_cache_header_finds_a_marker_straddling_two_reads() {
        let chunk = super::EPUB_CACHE_HEADER_READ_CHUNK_BYTES;
        // Marker starts 5 bytes before the first read ends.
        let header = header_of_len(chunk - 5 + super::EPUB_CACHE_TEXT_MARKER.len());
        let read = read_header_from("epx-header-straddle", &header, 10_000).unwrap();
        assert_eq!(read.as_bytes(), header.as_slice());
    }

    #[test]
    fn epub_cache_header_accepts_a_marker_at_end_of_file() {
        let header = header_of_len(3 * super::EPUB_CACHE_HEADER_READ_CHUNK_BYTES + 17);
        let read = read_header_from("epx-header-eof", &header, 0).unwrap();
        assert_eq!(read.as_bytes(), header.as_slice());
    }

    #[test]
    fn epub_cache_header_keeps_the_size_bound() {
        let max = super::EPUB_CACHE_HEADER_MAX_BYTES;
        let at_bound = header_of_len(max);
        assert!(read_header_from("epx-header-at-bound", &at_bound, 1000).is_ok());
        let past_bound = header_of_len(max + 1);
        assert!(read_header_from("epx-header-past-bound", &past_bound, 1000).is_err());
    }

    #[test]
    fn epub_cache_header_without_marker_is_an_error() {
        let dir = temp_dir("epx-header-missing");
        let path = dir.join("TEST.EPX");
        fs::write(&path, vec![b'x'; 20_000]).unwrap();
        assert!(super::read_epub_cache_header(&path).is_err());
    }

    #[test]
    fn resuming_at_the_saved_location_does_not_rewrite_state_files() {
        let root = temp_dir("resume-no-rewrite-books");
        let state = temp_dir("resume-no-rewrite-state");
        let paths = write_sequential_txt_books(&root, &["Book.txt"]);
        let roots = || {
            ReaderUiState::with_roots(
                root.to_string_lossy().into_owned(),
                state.to_string_lossy().into_owned(),
            )
        };
        let mut reader = roots();
        reader.refresh_library();
        let book = reader
            .books
            .iter()
            .find(|book| book.path == paths[0].to_string_lossy())
            .unwrap()
            .clone();
        reader.request_open_book(book, None);
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        let state_path = reader.state_path();
        let saved = fs::read_to_string(&state_path).unwrap();
        // The first save had nothing to back up; a rewrite would leave one.
        let backup = super::with_extension(&state_path, "BAK");
        assert!(!backup.exists());

        // Fresh boot: reload persisted state and resume the same book.
        let mut reader = roots();
        reader.load_persistent_state();
        assert!(reader.request_continue());
        while reader.tick() != ReaderTickOutcome::FirstPageReady {}

        assert!(!backup.exists(), "resume rewrote STATE.TXT");
        assert_eq!(fs::read_to_string(&state_path).unwrap(), saved);
    }

    #[test]
    fn background_warmup_parks_recent_books_without_touching_the_active_one() {
        let root = temp_dir("session-cache-warmup-books");
        let state = temp_dir("session-cache-warmup-state");
        // Opened in this order so `Active.txt` (opened last) ends up as the
        // foreground session; `Other.txt` is what warm-up should reach.
        let paths = write_sequential_txt_books(&root, &["Other.txt", "Active.txt"]);
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        for path in &paths {
            let book = reader
                .books
                .iter()
                .find(|book| book.path == path.to_string_lossy())
                .unwrap()
                .clone();
            reader.request_open_book(book, None);
            while reader.tick() != ReaderTickOutcome::FirstPageReady {}
        }
        // Both books are now in `recent` (Active.txt most recent); clear the
        // park that `release_active_session_for_open` already did for
        // `Other.txt` so this test exercises `tick_background_warmup`'s own
        // repopulation instead.
        reader.session_cache.clear();

        reader.seed_background_warmup(Some(paths[1].to_string_lossy().as_ref()));
        assert!(reader.tick_background_warmup());
        assert!(
            !reader.tick_background_warmup(),
            "queue should drain in one step"
        );
        assert_eq!(reader.session_cache.len(), 1);
        assert_eq!(
            reader.session_cache[0].book.path,
            paths[0].to_string_lossy()
        );
        assert_eq!(
            reader.session.as_ref().unwrap().book.path,
            paths[1].to_string_lossy(),
            "warm-up must never touch the active foreground session"
        );
    }

    #[test]
    fn repeated_degraded_persistence_events_are_suppressed_until_status_changes() {
        let mut reader = ReaderUiState::default();
        reader.finish_persistence("anchor-cache", vec!["CACHE: failed".into()]);
        assert!(reader.take_persistence_event().is_some());
        reader.finish_persistence("anchor-cache", vec!["CACHE: failed".into()]);
        assert!(reader.take_persistence_event().is_none());
        reader.finish_persistence("anchor-cache", Vec::new());
        assert_eq!(
            reader.take_persistence_event().as_deref(),
            Some("status=saved scope=anchor-cache")
        );
    }

    /// Words below 3 letters (once punctuation is trimmed) are the filter
    /// that keeps short conjunctions/articles out of dictionary-mode word
    /// selection.
    #[test]
    fn eligible_word_spans_trims_punctuation_and_drops_short_words() {
        let line = "Il gatto, corre veloce!";
        let words: Vec<&str> = eligible_word_spans(line)
            .into_iter()
            .map(|(start, end)| &line[start..end])
            .collect();
        assert_eq!(words, vec!["gatto", "corre", "veloce"]);

        assert!(eligible_word_spans("Ma no da").is_empty());
        assert!(eligible_word_spans("").is_empty());
    }

    fn dictionary_mode_test_session(lines: &[&str]) -> ReaderSession {
        ReaderSession {
            book: ReaderBook {
                path: "Book.txt".into(),
                title: "Book".into(),
                format: BookFormat::Text,
                size_bytes: 1000,
                modified_seconds: 0,
            },
            encoding: TextEncoding::Utf8,
            epub_document: None,
            layout: word_wrap_layout(200, 10),
            current_page: 0,
            page_number_base: 0,
            page_offsets: vec![0],
            indexed_through: 0,
            index_complete: true,
            cache: vec![ReaderCachedPage {
                page_index: 0,
                byte_offset: 0,
                next_byte_offset: 0,
                lines: lines
                    .iter()
                    .map(|text| ReaderPageLine {
                        text: (*text).to_string(),
                        paragraph_end: true,
                        image: None,
                    })
                    .collect(),
            }],
            epub_chapter_pages: Vec::new(),
            epub_pending_chapter: None,
            epub_document_cache_pending: false,
        }
    }

    /// Full line -> word -> definition walk, including the confirmed
    /// behaviour that a line with no eligible words is skipped while
    /// scrolling (not just rejected on confirm), and that BOOT-style
    /// step-back retraces one phase at a time.
    #[test]
    fn dictionary_mode_steps_through_line_word_and_definition_phases() {
        let mut reader = ReaderUiState::default();
        reader.session = Some(dictionary_mode_test_session(&[
            "Il gatto corre veloce",
            "Ma no da",
            "Lei arriva presto",
        ]));

        assert!(reader.toggle_dictionary_mode());
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::LineSelect { line_index: 0 }
        );

        // Down skips line 1 ("Ma no da" has no word >= 3 letters).
        reader.dictionary_move_line(1);
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::LineSelect { line_index: 2 }
        );

        // Clamped at the last eligible line: no page turn, no wraparound.
        reader.dictionary_move_line(1);
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::LineSelect { line_index: 2 }
        );

        reader.dictionary_move_line(-1);
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::LineSelect { line_index: 0 }
        );

        reader.dictionary_confirm_line();
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::WordSelect {
                line_index: 0,
                word_index: 0
            }
        );

        reader.dictionary_move_word(1);
        reader.dictionary_move_word(1);
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::WordSelect {
                line_index: 0,
                word_index: 2
            }
        );
        // Clamped at the last eligible word ("gatto", "corre", "veloce").
        reader.dictionary_move_word(1);
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::WordSelect {
                line_index: 0,
                word_index: 2
            }
        );

        reader.dictionary_confirm_word();
        match &reader.dictionary_mode {
            ReaderDictionaryMode::Definition {
                line_index,
                word_index,
                word,
                ..
            } => {
                assert_eq!(*line_index, 0);
                assert_eq!(*word_index, 2);
                assert_eq!(word, "veloce");
            }
            other => panic!("expected Definition phase, got {other:?}"),
        }

        assert!(reader.dictionary_step_back());
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::WordSelect {
                line_index: 0,
                word_index: 2
            }
        );
        assert!(reader.dictionary_step_back());
        assert_eq!(
            reader.dictionary_mode,
            ReaderDictionaryMode::LineSelect { line_index: 0 }
        );
        assert!(reader.dictionary_step_back());
        assert_eq!(reader.dictionary_mode, ReaderDictionaryMode::Off);
        assert!(!reader.dictionary_step_back());
    }

    /// A held SELECT toggles the mode off again from any sub-phase, and a
    /// page with no selectable words at all refuses to enter the mode.
    #[test]
    fn dictionary_mode_toggle_exits_from_any_phase_and_refuses_empty_pages() {
        let mut reader = ReaderUiState::default();
        reader.session = Some(dictionary_mode_test_session(&["Il gatto corre veloce"]));
        assert!(reader.toggle_dictionary_mode());
        reader.dictionary_confirm_line();
        assert!(matches!(
            reader.dictionary_mode,
            ReaderDictionaryMode::WordSelect { .. }
        ));
        assert!(reader.toggle_dictionary_mode());
        assert_eq!(reader.dictionary_mode, ReaderDictionaryMode::Off);

        reader.session = Some(dictionary_mode_test_session(&["Ma no da", "Se tu lo"]));
        assert!(!reader.toggle_dictionary_mode());
        assert_eq!(reader.dictionary_mode, ReaderDictionaryMode::Off);
    }

    #[test]
    fn cycle_book_action_wraps_between_mark_completed_and_bookmarks() {
        let mut reader = ReaderUiState::default();
        assert_eq!(
            reader.selected_book_action(),
            LibraryBookAction::MarkCompleted
        );
        reader.cycle_book_action_next();
        assert_eq!(reader.selected_book_action(), LibraryBookAction::Bookmarks);
        reader.cycle_book_action_next();
        assert_eq!(
            reader.selected_book_action(),
            LibraryBookAction::MarkCompleted
        );
        reader.cycle_book_action_previous();
        assert_eq!(reader.selected_book_action(), LibraryBookAction::Bookmarks);
    }

    #[test]
    fn book_actions_bookmarks_filters_to_only_the_target_book() {
        let mut reader = ReaderUiState::default();
        reader.bookmarks = vec![
            ReaderLocation {
                path: "a.txt".into(),
                title: "A".into(),
                format: BookFormat::Text,
                size_bytes: 10,
                modified_seconds: 0,
                page_index: 1,
                byte_offset: 5,
                epub_chapter: None,
                reading_percent: None,
            },
            ReaderLocation {
                path: "b.txt".into(),
                title: "B".into(),
                format: BookFormat::Text,
                size_bytes: 10,
                modified_seconds: 0,
                page_index: 2,
                byte_offset: 7,
                epub_chapter: None,
                reading_percent: None,
            },
        ];
        assert!(reader.book_actions_bookmarks().is_empty());

        reader.open_book_actions(ReaderBook {
            path: "b.txt".into(),
            title: "B".into(),
            format: BookFormat::Text,
            size_bytes: 10,
            modified_seconds: 0,
        });
        let filtered = reader.book_actions_bookmarks();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "b.txt");
    }

    #[test]
    fn mark_book_actions_target_completed_is_a_noop_without_a_target() {
        let mut reader = ReaderUiState::default();
        assert!(!reader.mark_book_actions_target_completed());
        assert!(reader.recent.is_empty());
        assert!(reader.positions.is_empty());
    }

    #[test]
    fn mark_book_actions_target_completed_persists_full_progress_for_a_never_opened_book() {
        let root = temp_dir("book-actions-books");
        let state_dir = temp_dir("book-actions-state");
        write_sequential_txt_books(&root, &["Fresh.txt"]);
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state_dir.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        let book = reader.books[0].clone();
        assert!(reader.saved_position_for_book(&book).is_none());

        reader.open_book_actions(book.clone());
        assert!(reader.mark_book_actions_target_completed());

        let position = reader
            .saved_position_for_book(&book)
            .expect("mark-completed should create a position");
        assert_eq!(position.reading_percent, Some(100));
        assert!(reader
            .recent
            .iter()
            .any(|entry| entry.path == book.path && entry.reading_percent == Some(100)));

        // Reload from disk into a fresh instance to confirm this was actually
        // persisted (POSITS.TXT / RECENT.TXT), not just held in memory.
        let mut reopened = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state_dir.to_string_lossy().into_owned(),
        );
        reopened.load_persistent_state();
        let reloaded_position = reopened
            .saved_position_for_book(&book)
            .expect("position should survive a reload");
        assert_eq!(reloaded_position.reading_percent, Some(100));
        assert!(reopened.recent.iter().any(|entry| entry.path == book.path));
    }

    #[test]
    fn mark_book_actions_target_completed_updates_an_existing_position_in_place() {
        let root = temp_dir("book-actions-existing-books");
        let state_dir = temp_dir("book-actions-existing-state");
        write_sequential_txt_books(&root, &["Started.txt"]);
        let mut reader = ReaderUiState::with_roots(
            root.to_string_lossy().into_owned(),
            state_dir.to_string_lossy().into_owned(),
        );
        reader.refresh_library();
        let book = reader.books[0].clone();
        let partial = ReaderLocation {
            path: book.path.clone(),
            title: book.title.clone(),
            format: book.format,
            size_bytes: book.size_bytes,
            modified_seconds: book.modified_seconds,
            page_index: 3,
            byte_offset: 42,
            epub_chapter: None,
            reading_percent: Some(17),
        };
        reader.positions.push(partial);

        reader.open_book_actions(book.clone());
        assert!(reader.mark_book_actions_target_completed());

        // Exactly one position for this book — updated, not duplicated.
        let matching: Vec<_> = reader
            .positions
            .iter()
            .filter(|entry| entry.path == book.path)
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].reading_percent, Some(100));
    }
}

#[cfg(test)]
mod backward_navigation_tests {
    use super::*;
    use crate::epub::{EpubChapter, EpubDocument, EpubImage, EPUB_IMAGE_SENTINEL};

    /// Regression: a session resumed in chapter 2 must page back into
    /// chapter 1 (a standalone cover image) and actually show it, not keep
    /// showing chapter 2's first page under the cover's colliding absolute
    /// page index.
    #[test]
    fn paging_back_from_a_resumed_chapter_shows_the_cover_page() {
        let cover = EPUB_IMAGE_SENTINEL.to_string();
        let body = "Parola ".repeat(400);
        let text = format!("{cover}

{body}");
        let cover_end = cover.len() as u64;
        let body_start = cover_end + 2;
        let document = EpubDocument::from_resident_for_test(
            "Book".into(),
            text.clone(),
            Vec::new(),
            vec![
                EpubChapter {
                    number: 1,
                    label: "Cover".into(),
                    text_offset: 0,
                    text_end_offset: cover_end,
                    spine_index: 0,
                },
                EpubChapter {
                    number: 2,
                    label: "One".into(),
                    text_offset: body_start,
                    text_end_offset: text.len() as u64,
                    spine_index: 1,
                },
            ],
            vec![EpubImage {
                href: "OEBPS/cover.jpg".into(),
                alt: "Cover".into(),
                text_offset: 0,
                spine_index: 0,
                width: 800,
                height: 1200,
            }],
            2,
        );
        let layout = ReaderPreferences::default().layout();
        let first = read_epub_page(&document, layout, body_start, 0).unwrap();
        let mut session = ReaderSession {
            book: ReaderBook {
                path: "BOOK.EPUB".into(),
                title: "Book".into(),
                format: BookFormat::Epub,
                size_bytes: 0,
                modified_seconds: 0,
            },
            encoding: TextEncoding::Utf8,
            epub_document: Some(document),
            layout,
            current_page: 0,
            page_number_base: 0,
            page_offsets: vec![body_start],
            indexed_through: body_start,
            index_complete: false,
            cache: vec![first],
            epub_chapter_pages: Vec::new(),
            epub_pending_chapter: None,
            epub_document_cache_pending: false,
        };

        session.previous_page().unwrap();

        assert_eq!(session.page_offsets[session.current_page], 0);
        let page = session.current_cached_page().expect("cover page cached");
        assert_eq!(page.byte_offset, 0);
        let image = page.lines[0].image.as_ref().expect("cover image line");
        assert_eq!(image.href, "OEBPS/cover.jpg");
        assert_eq!(image.slot_span, layout.lines_per_page);

        // And forward again lands back on chapter 2's first page.
        session.next_page().unwrap();
        let page = session.current_cached_page().unwrap();
        assert_eq!(page.byte_offset, body_start);
    }
}

#[cfg(test)]
mod pagination_perf_tests {
    use super::*;

    /// Deterministic synthetic prose: paragraphs, Italian accents, curly
    /// quotes, Gutenberg-style `_emphasis_`, CRLF line breaks and the odd
    /// word wider than one line, so every `paginate_decoded` branch runs.
    pub(super) fn synthetic_book(bytes: usize, seed: u64) -> String {
        const WORDS: &[&str] = &[
            "the",
            "reader",
            "turned",
            "città",
            "perché",
            "quickly",
            "and",
            "a",
            "\u{201C}quoted\u{201D}",
            "_emphasis_",
            "file_name",
            "of",
            "long-winded",
            "e-paper",
            "già",
            "però",
            "\u{2014}",
            "page,",
            "chapter.",
            "said:",
            "I",
            "extraordinarily",
            "you",
            "è",
            "Wave",
            "…",
            "journey",
            "night",
            "light",
        ];
        let mut state = seed;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        let mut text = String::new();
        let mut words_in_paragraph = 0;
        while text.len() < bytes {
            let roll = next() % 1000;
            if roll < 3 {
                text.push_str(&"x".repeat(60 + next() % 40));
            } else {
                text.push_str(WORDS[next() % WORDS.len()]);
            }
            words_in_paragraph += 1;
            if words_in_paragraph > 40 + next() % 120 {
                text.push_str(if next() % 2 == 0 { "\r\n\r\n" } else { "\n" });
                words_in_paragraph = 0;
            } else {
                text.push(' ');
            }
        }
        text
    }

    fn temp_book(name: &str, text: &str) -> ReaderBook {
        let path = std::env::temp_dir().join(format!(
            "rustmix-pagination-{name}-{}.txt",
            std::process::id()
        ));
        fs::write(&path, text).unwrap();
        ReaderBook {
            path: path.to_string_lossy().into_owned(),
            title: name.into(),
            format: BookFormat::Text,
            size_bytes: text.len() as u64,
            modified_seconds: 0,
        }
    }

    fn paginate_whole_txt(book: &ReaderBook, layout: ReaderLayout) -> Vec<ReaderCachedPage> {
        let mut pages = Vec::new();
        let mut offset = 0;
        while offset < book.size_bytes {
            let page =
                read_txt_page(book, TextEncoding::Utf8, layout, offset, pages.len()).unwrap();
            assert!(
                page.next_byte_offset > offset,
                "pagination stalled at {offset}"
            );
            offset = page.next_byte_offset;
            pages.push(page);
        }
        pages
    }

    /// The single-pass, full-window TXT pagination that preceded the
    /// adaptive window, kept verbatim as the reference the new path must
    /// reproduce exactly.
    fn reference_txt_page(
        book: &ReaderBook,
        encoding: TextEncoding,
        layout: ReaderLayout,
        byte_offset: u64,
        page_index: usize,
    ) -> ReaderCachedPage {
        let mut file = File::open(&book.path).unwrap();
        file.seek(SeekFrom::Start(byte_offset)).unwrap();
        let mut bytes = vec![0_u8; READER_PAGE_READ_BYTES];
        let read = file.read(&mut bytes).unwrap();
        bytes.truncate(read);
        let skip_bom = byte_offset == 0 && bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let base = byte_offset + if skip_bom { 3 } else { 0 };
        let decoded = decode_with_offsets(&bytes[if skip_bom { 3 } else { 0 }..], encoding, base);
        let normalized = normalize_decoded(&decoded);
        let width_of = reader_layout_measure(&layout);
        let (lines, consumed) = paginate_decoded(&normalized, layout, &[], &width_of);
        ReaderCachedPage {
            page_index,
            byte_offset,
            next_byte_offset: consumed.max(base).min(book.size_bytes),
            lines,
        }
    }

    /// EPUB counterpart of [`reference_txt_page`].
    fn reference_epub_page(
        document: &EpubDocument,
        layout: ReaderLayout,
        byte_offset: u64,
        page_index: usize,
        text_end_offset: u64,
    ) -> ReaderCachedPage {
        let text_len = document.text_size_bytes() as usize;
        let start = (byte_offset as usize).min(text_len);
        let bounded_end = (text_end_offset as usize).min(text_len);
        let window_end = start
            .saturating_add(READER_PAGE_READ_BYTES)
            .min(bounded_end);
        let window = document.text_window(start, window_end).unwrap();
        let decoded = decode_with_offsets(&window, TextEncoding::Utf8, start as u64);
        let normalized = normalize_decoded(&decoded);
        let width_of = reader_layout_measure(&layout);
        let (lines, consumed) = paginate_decoded(&normalized, layout, &document.images, &width_of);
        ReaderCachedPage {
            page_index,
            byte_offset: start as u64,
            next_byte_offset: consumed.max(start as u64).min(text_end_offset),
            lines,
        }
    }

    fn equivalence_layouts() -> Vec<ReaderLayout> {
        [
            (
                ReaderOrientation::Portrait,
                BookFontSize::Large,
                BookFont::Serif,
            ),
            (
                ReaderOrientation::Portrait,
                BookFontSize::XXXLarge,
                BookFont::Literata,
            ),
            (
                ReaderOrientation::Landscape,
                BookFontSize::XLarge,
                BookFont::Serif,
            ),
        ]
        .into_iter()
        .map(|(orientation, font_size, book_font)| {
            ReaderPreferences {
                orientation,
                font_size,
                book_font,
                ..ReaderPreferences::default()
            }
            .layout()
        })
        .collect()
    }

    /// Text that stresses the window boundaries: normal prose, a run of
    /// one-word lines (pages that consume little text), a whitespace run
    /// longer than the first window, and a single "word" longer than the
    /// whole 16 KB window.
    fn boundary_stress_text() -> String {
        let mut text = synthetic_book(40 * 1024, 3);
        for index in 0..400 {
            text.push_str(if index % 3 == 0 { "_a_\n" } else { "verse\n" });
        }
        text.push_str(&" ".repeat(9 * 1024));
        text.push_str(&synthetic_book(8 * 1024, 5));
        text.push_str(&"w".repeat(20 * 1024));
        text.push(' ');
        text.push_str(&synthetic_book(12 * 1024, 11));
        text
    }

    #[test]
    fn adaptive_txt_window_matches_the_full_window_page_for_page() {
        for (name, text) in [
            ("prose", synthetic_book(160 * 1024, 42)),
            ("stress", boundary_stress_text()),
            ("bom", format!("\u{FEFF}{}", synthetic_book(24 * 1024, 9))),
        ] {
            let book = temp_book(&format!("equiv-{name}"), &text);
            for encoding in [TextEncoding::Utf8, TextEncoding::Windows1252] {
                for layout in equivalence_layouts() {
                    let mut offset = 0;
                    let mut index = 0;
                    while offset < book.size_bytes {
                        let expected = reference_txt_page(&book, encoding, layout, offset, index);
                        let actual = read_txt_page(&book, encoding, layout, offset, index).unwrap();
                        assert_eq!(
                            actual, expected,
                            "{name} {encoding:?} {layout:?} offset={offset}"
                        );
                        assert!(expected.next_byte_offset > offset, "stalled at {offset}");
                        offset = expected.next_byte_offset;
                        index += 1;
                    }
                }
            }
            let _ = fs::remove_file(&book.path);
        }
    }

    #[test]
    fn adaptive_epub_window_matches_the_full_window_page_for_page() {
        // Chapter bodies, each with inline images in the positions whose
        // "standalone" lookahead depends on how far the window reaches.
        let bodies: Vec<String> = vec![
            // Image opening the chapter, then more whitespace than the first
            // window holds, then text: not standalone.
            format!(
                "{EPUB_IMAGE_SENTINEL}\n{}{}",
                " ".repeat(6 * 1024),
                synthetic_book(20 * 1024, 1)
            ),
            // Image closing the chapter, followed only by whitespace.
            format!(
                "{}\n\n{EPUB_IMAGE_SENTINEL}\n   \n",
                synthetic_book(18 * 1024, 2)
            ),
            // A chapter that is only an image: a cover.
            format!("{EPUB_IMAGE_SENTINEL}"),
            // Images scattered through ordinary text.
            {
                let mut body = String::new();
                for part in 0..6 {
                    body.push_str(&synthetic_book(3 * 1024 + part * 700, 20 + part as u64));
                    body.push(' ');
                    body.push(EPUB_IMAGE_SENTINEL);
                    body.push(' ');
                }
                body
            },
            boundary_stress_text(),
        ];
        let mut text = String::new();
        let mut chapters = Vec::new();
        let mut images = Vec::new();
        for (index, body) in bodies.iter().enumerate() {
            if index > 0 {
                text.push_str("\n\n");
            }
            let chapter_start = text.len() as u64;
            for (offset, _) in body.match_indices(EPUB_IMAGE_SENTINEL) {
                images.push(EpubImage {
                    href: format!("OEBPS/img{}.jpg", images.len()),
                    alt: String::new(),
                    text_offset: chapter_start + offset as u64,
                    spine_index: index,
                    width: if images.len() % 2 == 0 { 600 } else { 0 },
                    height: if images.len() % 2 == 0 { 900 } else { 0 },
                });
            }
            text.push_str(body);
            chapters.push(EpubChapter {
                number: index + 1,
                label: format!("Chapter {}", index + 1),
                text_offset: chapter_start,
                text_end_offset: text.len() as u64,
                spine_index: index,
            });
        }
        let document = EpubDocument::from_resident_for_test(
            "Equivalence".into(),
            text,
            Vec::new(),
            chapters.clone(),
            images,
            bodies.len(),
        );
        for layout in equivalence_layouts() {
            for chapter in &chapters {
                let mut offset = chapter.text_offset;
                let mut index = 0;
                while offset < chapter.text_end_offset {
                    let expected = reference_epub_page(
                        &document,
                        layout,
                        offset,
                        index,
                        chapter.text_end_offset,
                    );
                    let actual = read_epub_page_until(
                        &document,
                        layout,
                        offset,
                        index,
                        chapter.text_end_offset,
                    )
                    .unwrap();
                    assert_eq!(
                        actual, expected,
                        "chapter {} {layout:?} offset={offset}",
                        chapter.number
                    );
                    assert!(expected.next_byte_offset > offset, "stalled at {offset}");
                    offset = expected.next_byte_offset;
                    index += 1;
                }
            }
        }
    }

    #[test]
    #[ignore = "host timing benchmark; run with --release --ignored --nocapture"]
    fn bench_txt_pagination_whole_book() {
        let text = synthetic_book(600 * 1024, 7);
        let book = temp_book("bench", &text);
        for size in [BookFontSize::Large, BookFontSize::XXXLarge] {
            let layout = ReaderPreferences {
                font_size: size,
                ..ReaderPreferences::default()
            }
            .layout();
            let started = std::time::Instant::now();
            let pages = paginate_whole_txt(&book, layout);
            let elapsed = started.elapsed();
            println!(
                "bench-txt-pagination font={size:?} pages={} total-ms={} per-page-us={}",
                pages.len(),
                elapsed.as_millis(),
                elapsed.as_micros() / pages.len() as u128
            );
        }
        let _ = fs::remove_file(&book.path);
    }
}
