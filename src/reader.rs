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
    time::{Instant, UNIX_EPOCH},
};

use crate::{
    buttons::ButtonEvent,
    dictionary::{
        compact_error, load_dictionary_index, lookup_dictionary_exact, DictionaryIndexRow,
        DICTIONARY_ROOT,
    },
    epub::{
        open_epub_on_worker, read_epub_title_on_worker, EpubChapter, EpubDocument, EpubTocEntry,
        EPUB_REFLOW_TEXT_LIMIT, EPUB_SPINE_LIMIT, EPUB_TOC_LIMIT,
    },
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
/// Maximum library rows retained for the embedded product UI.
pub const READER_LIBRARY_LIMIT: usize = 128;
/// Maximum per-book last-position records retained on removable storage.
pub const READER_POSITION_LIMIT: usize = 64;
/// Maximum recent-book records retained on removable storage.
pub const READER_RECENT_LIMIT: usize = 16;
/// Maximum bookmark records retained on removable storage.
pub const READER_BOOKMARK_LIMIT: usize = 128;
/// Maximum page anchors accepted from one SD-backed cache file.
pub const READER_CACHE_OFFSET_LIMIT: usize = 4096;
/// Persist an anchor-cache checkpoint after this many newly indexed pages.
pub const READER_CACHE_CHECKPOINT_PAGES: usize = 4;
/// Maximum pre-indexed EPUB page anchors retained for chapter-aware labels.
pub const READER_EPUB_PAGE_ANCHOR_LIMIT: usize = 4096;

const READER_PERSISTENCE_VERSION: &str = "1";
const READER_CACHE_VERSION: &str = "3";
const READER_PREFS_VERSION: &str = "1";
/// SD-backed flattened-EPUB-text cache format version. Independent of Reader
/// layout: the cached reflowed text and TOC never change with font/orientation.
const EPUB_DOCUMENT_CACHE_VERSION: &str = "1";
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
    Small,
    #[default]
    Medium,
    Large,
    XLarge,
}

impl BookFontSize {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
            Self::XLarge => "XLarge",
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
            Self::XLarge => "xlarge",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Small => Self::Medium,
            Self::Medium => Self::Large,
            Self::Large => Self::XLarge,
            Self::XLarge => Self::Small,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Small => Self::XLarge,
            Self::Medium => Self::Small,
            Self::Large => Self::Medium,
            Self::XLarge => Self::Large,
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "small" => Ok(Self::Small),
            "medium" => Ok(Self::Medium),
            "large" => Ok(Self::Large),
            "xlarge" | "extra-large" | "extra_large" => Ok(Self::XLarge),
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
}

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            theme: ReadingTheme::Classic,
            orientation: ReaderOrientation::Portrait,
            font_size: BookFontSize::Medium,
            book_font: BookFont::Serif,
            paragraph_alignment: ParagraphAlignment::Justified,
            show_progress: true,
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
        let lines_per_page = match (self.orientation, self.font_size) {
            (ReaderOrientation::Portrait, BookFontSize::Small) => 31,
            (ReaderOrientation::Portrait, BookFontSize::Medium) => 26,
            (ReaderOrientation::Portrait, BookFontSize::Large) => 23,
            (ReaderOrientation::Portrait, BookFontSize::XLarge) => 20,
            (ReaderOrientation::Landscape, BookFontSize::Small) => 17,
            (ReaderOrientation::Landscape, BookFontSize::Medium) => 14,
            (ReaderOrientation::Landscape, BookFontSize::Large) => 12,
            (ReaderOrientation::Landscape, BookFontSize::XLarge) => 10,
        };
        ReaderLayout {
            available_width_px,
            lines_per_page,
            orientation: self.orientation,
            font_size: self.font_size,
            book_font: self.book_font,
            paragraph_alignment: self.paragraph_alignment,
        }
    }

    #[must_use]
    pub fn serialized(self) -> String {
        let show_progress = if self.show_progress { "true" } else { "false" };
        format!(
            "version={}\ntheme={}\norientation={}\nfont_size={}\nbook_font={}\nparagraph_alignment={}\nshow_progress={}\n",
            READER_PREFS_VERSION,
            self.theme.marker(),
            self.orientation.marker(),
            self.font_size.marker(),
            self.book_font.marker(),
            self.paragraph_alignment.marker(),
            show_progress,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderPageLine {
    pub text: String,
    pub paragraph_end: bool,
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
        Some(ReaderChapterPageLabel {
            chapter_number: chapter.chapter_number,
            page_number,
            page_count: chapter.page_offsets.len().max(1),
        })
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
    /// cached, via `ensure_page_cached`. Bounding synchronous work to one
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
        self.page_number_base = self.page_number_base.saturating_sub(prepended_count);
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

/// Reader Options action rows. Editable values live on the separate
/// Reading Preferences editor so menu controls match the rest of the firmware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderOption {
    Bookmark,
    Bookmarks,
    TableOfContents,
    ReadingPreferences,
    ClearGhosting,
    GoToLibrary,
    GoHome,
}

impl ReaderOption {
    pub const ALL: [Self; 7] = [
        Self::Bookmark,
        Self::Bookmarks,
        Self::TableOfContents,
        Self::ReadingPreferences,
        Self::ClearGhosting,
        Self::GoToLibrary,
        Self::GoHome,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bookmark => "Add / Remove Bookmark",
            Self::Bookmarks => "Bookmarks",
            Self::TableOfContents => "Table of Contents",
            Self::ReadingPreferences => "Reading Preferences",
            Self::ClearGhosting => "Clear Ghosting",
            Self::GoToLibrary => "Go to Library",
            Self::GoHome => "Go Home",
        }
    }

    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Bookmark => "TOGGLE",
            Self::Bookmarks => "LIST",
            Self::TableOfContents => "NONE",
            Self::ReadingPreferences => ">>>",
            Self::ClearGhosting => "RUN",
            Self::GoToLibrary | Self::GoHome => ">>>",
        }
    }
}

/// Reading Preferences editor rows. UP/DOWN changes the active value and
/// SELECT advances to the next row, matching the firmware editor convention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadingPreference {
    ReadingTheme,
    Orientation,
    BookFontSize,
    BookFont,
    ParagraphAlignment,
    ShowProgress,
}

impl ReadingPreference {
    pub const ALL: [Self; 5] = [
        Self::ReadingTheme,
        Self::Orientation,
        Self::BookFontSize,
        Self::BookFont,
        Self::ParagraphAlignment,
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
    pub toc_selected: usize,
    pub loading: Option<PendingReaderOpen>,
    pub session: Option<ReaderSession>,
    /// Decoded thumbnails for books currently visible on the Library screen,
    /// keyed by [`ReaderBook::path`]. Populated by the main loop (which owns
    /// SD access) from [`crate::cover_cache::CoverCache`]; `render_library`
    /// only ever reads this map, never touches SD itself. Cleared whenever
    /// the Library screen is left, so this stays bounded to roughly one
    /// screenful rather than growing across a full library scroll.
    pub library_thumbnails: std::collections::HashMap<String, crate::cover_cache::CachedThumbnail>,
    pub options_selected: usize,
    pub dictionary_mode: ReaderDictionaryMode,
    /// Parsed INDEX.TXT rows, loaded once and reused: the pack doesn't
    /// change mid-session, and re-reading/re-parsing it (hundreds of KB for
    /// a full pack) on every single word lookup is the dominant cost of an
    /// in-reader dictionary lookup on SD-backed storage.
    dictionary_index_cache: Option<Vec<DictionaryIndexRow>>,
    pub preferences_selected: usize,
    preferences_layout_dirty: bool,
    pub last_message: Option<String>,
    persistence_event: Option<String>,
    last_persistence_event: Option<String>,
    clear_ghost_requested: bool,
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
            toc_selected: 0,
            loading: None,
            session: None,
            library_thumbnails: std::collections::HashMap::new(),
            options_selected: 0,
            dictionary_mode: ReaderDictionaryMode::Off,
            dictionary_index_cache: None,
            preferences_selected: 0,
            preferences_layout_dirty: false,
            last_message: None,
            persistence_event: None,
            last_persistence_event: None,
            clear_ghost_requested: false,
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
        let mut warnings = Vec::new();
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
        self.resume = match load_location_record(&self.state_path()) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("STATE.TXT: {error}"));
                None
            }
        };
        self.positions = match self.load_positions_with_legacy_migration() {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("POSITS.TXT: {error}"));
                Vec::new()
            }
        };
        self.recent = match load_location_list(&self.recent_path(), READER_RECENT_LIMIT) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("RECENT.TXT: {error}"));
                Vec::new()
            }
        };
        self.bookmarks = match load_location_list(&self.bookmarks_path(), READER_BOOKMARK_LIMIT) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("MARKS.TXT: {error}"));
                Vec::new()
            }
        };
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
        let still_present = |location: &ReaderLocation| {
            self.books.iter().any(|book| location.matches_book(book))
        };
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

    pub fn request_continue(&mut self) -> bool {
        let Some(location) = self.resume.clone().or_else(|| self.recent.first().cloned()) else {
            return false;
        };
        self.request_open_book(location.as_book(), Some(location));
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

    #[must_use]
    pub fn visible_entries(&self) -> Vec<ReaderLibraryEntry> {
        let (recent, other) = self.library_sections();
        recent.into_iter().chain(other).collect()
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

    pub fn request_open_visible(&mut self, visible_index: usize) -> bool {
        let Some(entry) = self.visible_entries().get(visible_index).cloned() else {
            return false;
        };
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

    /// Persist and drop the previous session before a new book is parsed. EPUB
    /// documents retain flattened text and chapter anchors in RAM; keeping the
    /// old document alive while allocating the next parser-worker stack can
    /// exhaust the embedded heap after repeated book switches.
    fn release_active_session_for_open(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.persist_current_session_best_effort();
        self.session = None;
        log::info!("rustmix-wave=reader-session-memory-release status=completed reason=book-open");
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

    pub fn tick(&mut self) -> ReaderTickOutcome {
        if let Some(mut loading) = self.loading.take() {
            let outcome = loop {
                let stage_before = loading.stage;
                let stage_started_at = Instant::now();
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
                                    loading.message = error;
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
                                    )
                                }),
                        };
                        match session {
                            Ok(session) => {
                                self.session = Some(session);
                                self.last_message =
                                    Some("Saved position ready; caching continues lazily".into());
                                self.persist_current_session_best_effort();
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
                if let Some(document) = session.epub_document.as_ref() {
                    self.persist_epub_document_cache_best_effort(&session.book, document);
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
                    session.page_offsets.len() % READER_CACHE_CHECKPOINT_PAGES == 0
                        || session.index_complete
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
        outcome
    }

    pub fn previous_page(&mut self) {
        if let Some(session) = self.session.as_mut() {
            if let Err(error) = session.previous_page() {
                self.last_message = Some(error);
                return;
            }
            self.persist_current_session_best_effort();
        }
    }

    pub fn next_page(&mut self) {
        if let Some(session) = self.session.as_mut() {
            if let Err(error) = session.next_page() {
                self.last_message = Some(error);
                return;
            }
            self.persist_current_session_best_effort();
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

    /// Loads and parses INDEX.TXT on first use only; later lookups in the
    /// same session reuse the cached rows instead of re-reading the file.
    fn cached_dictionary_index(&mut self) -> Result<&[DictionaryIndexRow], String> {
        if self.dictionary_index_cache.is_none() {
            self.dictionary_index_cache = Some(
                load_dictionary_index(Path::new(DICTIONARY_ROOT))
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(self
            .dictionary_index_cache
            .as_deref()
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
            Ok(rows) => match lookup_dictionary_exact(Path::new(DICTIONARY_ROOT), rows, &word) {
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
        let page = {
            let Some(document) = session.epub_document.as_ref() else {
                return false;
            };
            read_epub_page(document, session.layout, entry.text_offset, 0)
                .map(|page| (page, document.text_size_bytes()))
        };
        session.page_number_base = 0;
        session.current_page = 0;
        session.page_offsets = vec![entry.text_offset];
        session.indexed_through = entry.text_offset;
        session.index_complete = false;
        session.cache.clear();
        // Stale in-progress-chapter tracking from wherever the reader was
        // before the jump would otherwise get appended onto `page_offsets`
        // by the next background tick, mixing pages from two unrelated
        // positions in the book.
        session.epub_pending_chapter = None;
        match page {
            Ok((page, source_size)) => {
                session.indexed_through = page.next_byte_offset;
                session.index_complete = session.indexed_through >= source_size;
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

    /// Apply one Settings-style SELECT action to the highlighted preference.
    /// Redraw-only settings persist immediately in place. Layout-sensitive
    /// settings persist immediately and request a staged current-page rebuild.
    #[must_use]
    pub fn activate_selected_preference(&mut self) -> bool {
        let layout_sensitive = match self.selected_preference() {
            ReadingPreference::ReadingTheme => {
                self.preferences.theme = self.preferences.theme.next();
                self.last_message =
                    Some(format!("Reading theme: {}", self.preferences.theme.label()));
                self.persist_preferences_best_effort();
                self.request_clear_ghosting();
                false
            }
            ReadingPreference::Orientation => {
                self.preferences.orientation = self.preferences.orientation.next();
                self.last_message = Some(format!(
                    "Orientation: {}",
                    self.preferences.orientation.label()
                ));
                true
            }
            ReadingPreference::BookFontSize => {
                self.preferences.font_size = self.preferences.font_size.next();
                self.last_message = Some(format!(
                    "Book font size: {}",
                    self.preferences.font_size.label()
                ));
                true
            }
            ReadingPreference::BookFont => {
                self.preferences.book_font = self.preferences.book_font.next();
                self.last_message =
                    Some(format!("Book font: {}", self.preferences.book_font.label()));
                true
            }
            ReadingPreference::ParagraphAlignment => {
                self.preferences.paragraph_alignment = self.preferences.paragraph_alignment.next();
                self.last_message = Some(format!(
                    "Paragraph alignment: {}",
                    self.preferences.paragraph_alignment.label()
                ));
                true
            }
            ReadingPreference::ShowProgress => {
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
                false
            }
        };
        if layout_sensitive {
            self.request_layout_rebuild()
        } else {
            false
        }
    }

    /// Finish the Settings-style editor. SELECT already persists changes and
    /// launches any required staged rebuild, so BOOT simply returns to options.
    pub fn finish_preferences_edit(&mut self) -> bool {
        self.preferences_layout_dirty = false;
        false
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
    /// same book can skip ZIP/DEFLATE/HTML-flatten work entirely.
    fn persist_epub_document_cache_best_effort(
        &mut self,
        book: &ReaderBook,
        document: &EpubDocument,
    ) {
        let path = self.epub_document_cache_path_for(book);
        let fingerprint = epub_document_fingerprint(book);
        match atomic_replace_cache_text(
            &path,
            &serialize_epub_document_cache(document, fingerprint),
        ) {
            Ok(()) => {
                log::info!("rustmix-wave=epub-document-cache status=saved");
            }
            Err(error) => {
                log::warn!("rustmix-wave=epub-document-cache status=save-failed error={error}");
                self.persistence_warning = Some(format!("EPUB cache not saved: {error}"));
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
        let (
            epub_chapter_pages,
            epub_pending_chapter,
            page_offsets,
            indexed_through,
            index_complete,
        ) = match self
            .load_epub_chapter_pages_cache_best_effort(book, layout)
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
            }) {
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
/// `previous` is the prior scan's results (empty on the first scan). An EPUB
/// whose path, size, and modification time still match an entry in
/// `previous` reuses that entry's title instead of reopening the archive to
/// reparse OPF metadata, so re-entering the Library screen after the first
/// scan doesn't pay the zip-parsing cost again for unchanged files.
pub fn scan_txt_library(
    root: impl AsRef<Path>,
    previous: &[ReaderBook],
) -> Result<Vec<ReaderBook>, String> {
    let root = root.as_ref();
    let mut books = Vec::new();
    let entries =
        fs::read_dir(root).map_err(|error| format!("Books folder unavailable: {error}"))?;
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
            .map(|book| book.title.clone());
        let title = if let Some(cached_title) = cached_title {
            cached_title
        } else if format == BookFormat::Epub {
            read_epub_title_on_worker(&path)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(fallback_title)
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
    Ok(books)
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
    let start = usize::try_from(byte_offset)
        .map_err(|_| "EPUB byte offset exceeds platform range".to_string())?
        .min(document.text.len());
    let bounded_end = usize::try_from(text_end_offset)
        .map_err(|_| "EPUB chapter end exceeds platform range".to_string())?
        .min(document.text.len());
    let end = start
        .saturating_add(READER_PAGE_READ_BYTES)
        .min(bounded_end);
    let bytes = document.text.as_bytes();
    let start = next_utf8_boundary(bytes, start);
    let end = previous_utf8_boundary(bytes, end).max(start);
    let decoded = decode_with_offsets(&bytes[start..end], TextEncoding::Utf8, start as u64);
    let normalized = normalize_decoded(&decoded);
    let width_of = reader_layout_measure(&layout);
    let (lines, consumed) = paginate_decoded(&normalized, layout, &width_of);
    let next_byte_offset = consumed.max(start as u64).min(text_end_offset);
    Ok(ReaderCachedPage {
        page_index,
        byte_offset: start as u64,
        next_byte_offset,
        lines,
    })
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
    let mut bytes = vec![0_u8; READER_PAGE_READ_BYTES];
    let read = file
        .read(&mut bytes)
        .map_err(|error| format!("Read failed: {error}"))?;
    bytes.truncate(read);
    let skip_bom = byte_offset == 0 && bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let base = byte_offset + if skip_bom { 3 } else { 0 };
    let decoded = decode_with_offsets(&bytes[if skip_bom { 3 } else { 0 }..], encoding, base);
    let normalized = normalize_decoded(&decoded);
    let width_of = reader_layout_measure(&layout);
    let (lines, consumed) = paginate_decoded(&normalized, layout, &width_of);
    let next_byte_offset = consumed.max(base).min(book.size_bytes);
    Ok(ReaderCachedPage {
        page_index,
        byte_offset,
        next_byte_offset,
        lines,
    })
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
                Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or(""),
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
        push_normalized_character(&mut normalized, character, next_offset);
    }
    normalized
}

fn push_normalized_character(output: &mut Vec<(char, u64)>, character: char, next_offset: u64) {
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
        value
            if value == '\n'
                || value == '\r'
                || value == '\t'
                || value.is_ascii_graphic()
                || value == ' ' =>
        {
            output.push((value, next_offset));
            return;
        }
        _ => "?",
    };
    for value in replacement.chars() {
        output.push((value, next_offset));
    }
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric()
}

/// Move `word` onto `line`, wrapping to a new line first if it would not
/// fit, and hard-breaking only when the word alone is wider than one line
/// (no narrower unit exists to wrap on). Returns `false` if the page's line
/// budget was reached before the word could be placed; the caller must then
/// stop consuming input without advancing its resume offset, so the next
/// page re-reads the word whole instead of splitting it across the page
/// boundary.
fn place_word(
    lines: &mut Vec<ReaderPageLine>,
    line: &mut String,
    word: &mut String,
    layout: &ReaderLayout,
    width_of: &impl Fn(&str) -> i32,
) -> bool {
    if word.is_empty() {
        return true;
    }
    if lines.len() >= layout.lines_per_page {
        return false;
    }
    let fits = line.is_empty()
        || width_of(line) + width_of(" ") + width_of(word) <= layout.available_width_px;
    if !line.is_empty() && !fits {
        lines.push(ReaderPageLine {
            text: core::mem::take(line),
            paragraph_end: false,
        });
        if lines.len() >= layout.lines_per_page {
            return false;
        }
    }
    while width_of(word) > layout.available_width_px {
        let split_at = pixel_split_point(word, layout.available_width_px, width_of);
        let (head, tail) = word.split_at(split_at);
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(head);
        let tail = tail.to_string();
        lines.push(ReaderPageLine {
            text: core::mem::take(line),
            paragraph_end: false,
        });
        *word = tail;
        if lines.len() >= layout.lines_per_page {
            return false;
        }
    }
    if !line.is_empty() {
        line.push(' ');
    }
    line.push_str(word);
    word.clear();
    true
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
fn paginate_decoded(
    decoded: &[(char, u64)],
    layout: ReaderLayout,
    width_of: &impl Fn(&str) -> i32,
) -> (Vec<ReaderPageLine>, u64) {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut word = String::new();
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
        let character = match character {
            '\r' => continue,
            '\n' => {
                if !place_word(&mut lines, &mut line, &mut word, &layout, width_of) {
                    page_full = true;
                    break;
                }
                lines.push(ReaderPageLine {
                    text: core::mem::take(&mut line),
                    paragraph_end: true,
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
            if !place_word(&mut lines, &mut line, &mut word, &layout, width_of) {
                page_full = true;
                break;
            }
            consumed = next_offset;
        } else {
            word.push(character);
        }
    }

    // When the page filled up, `word` is either empty (the break landed on a
    // clean word boundary) or holds a word deferred to the next page
    // (`place_word` returned `false` above without consuming it). Either way
    // `consumed` already reflects exactly what this page rendered, and must
    // not be pulled forward to the end of the decoded window here: doing so
    // silently skipped every byte between the true page end and the end of
    // the up-to-16 KB read window on every page that happened to break on a
    // clean word boundary, discarding whole chunks of chapter text
    // (sometimes mid-word once the next page resumed past the gap).
    if !page_full {
        if let Some((_, last_offset)) = decoded.last() {
            if place_word(&mut lines, &mut line, &mut word, &layout, width_of) {
                consumed = consumed.max(*last_offset);
            }
        }
    }
    if lines.len() < layout.lines_per_page && (!line.is_empty() || lines.is_empty()) {
        lines.push(ReaderPageLine {
            text: line,
            paragraph_end: true,
        });
    }
    (lines, consumed)
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
/// duplicate up to [`EPUB_REFLOW_TEXT_LIMIT`] bytes.
fn serialize_epub_document_cache(document: &EpubDocument, fingerprint: u64) -> String {
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
    output.push_str(&format!("text_bytes={}\ntext_start\n", document.text.len()));
    output.push_str(&document.text);
    output
}

fn parse_epub_document_cache(text: &str, book: &ReaderBook) -> Result<EpubDocument, String> {
    let mut version = None;
    let mut fingerprint = None;
    let mut title = None;
    let mut spine_count = None;
    let mut text_bytes = None;
    let mut toc = Vec::new();
    let mut chapters = Vec::new();
    let mut cursor = 0usize;
    let mut body_offset = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if trimmed == "text_start" {
            body_offset = Some(cursor + line.len());
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
                _ => {}
            }
        }
        cursor += line.len();
    }
    if version.as_deref() != Some(EPUB_DOCUMENT_CACHE_VERSION) {
        return Err("unsupported EPUB cache version".into());
    }
    let fingerprint = fingerprint.ok_or_else(|| "missing EPUB cache fingerprint".to_string())?;
    if fingerprint != epub_document_fingerprint(book) {
        return Err("EPUB cache fingerprint mismatch".into());
    }
    let body_offset = body_offset.ok_or_else(|| "missing EPUB cache text marker".to_string())?;
    let text_bytes: usize =
        text_bytes.ok_or_else(|| "missing EPUB cache text length".to_string())?;
    if text_bytes > EPUB_REFLOW_TEXT_LIMIT {
        return Err("EPUB cache text exceeds byte limit".into());
    }
    if body_offset.checked_add(text_bytes) != Some(text.len()) {
        return Err("EPUB cache text length mismatch".into());
    }
    let body = &text[body_offset..];
    if toc.is_empty() && chapters.is_empty() {
        return Err("EPUB cache produced no chapters".into());
    }
    let text_len = body.len() as u64;
    if chapters.iter().any(|chapter| {
        chapter.text_offset > chapter.text_end_offset || chapter.text_end_offset > text_len
    }) {
        return Err("EPUB cache chapter offset out of range".into());
    }
    if toc.iter().any(|entry| entry.text_offset > text_len) {
        return Err("EPUB cache TOC offset out of range".into());
    }
    Ok(EpubDocument {
        title: title.ok_or_else(|| "missing EPUB cache title".to_string())?,
        text: body.to_string(),
        toc,
        chapters,
        spine_count: spine_count.ok_or_else(|| "missing EPUB cache spine count".to_string())?,
    })
}

fn load_epub_document_cache(
    path: &Path,
    book: &ReaderBook,
) -> Result<Option<EpubDocument>, String> {
    load_with_backup(path, |text| parse_epub_document_cache(text, book))
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
    let mut chapters = Vec::new();
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
                let mut page_offsets = Vec::new();
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
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("version=") {
            version = Some(value);
        } else if let Some(value) = line.strip_prefix("entry=") {
            if output.len() < limit {
                output.push(parse_location_fields(value)?);
            }
        }
    }
    if version != Some(READER_PERSISTENCE_VERSION) {
        return Err("unsupported persistence version".into());
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
        match fs::read_to_string(&candidate) {
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
    let _ = fs::remove_file(&backup);
    Ok(())
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
        eligible_word_spans, epub_document_fingerprint, is_fat83_safe_file_name,
        load_epub_page_index_cache, load_location_record, normalize_decoded, paginate_decoded,
        parse_epub_document_cache, parse_location_fields, parse_location_record, scan_txt_library,
        serialize_epub_document_cache, serialize_epub_page_index_cache, serialize_location,
        serialize_location_fields, BookFont, BookFontSize, BookFormat, EpubChapter, EpubDocument,
        EpubTocEntry, ParagraphAlignment, ReaderBook, ReaderCachedPage, ReaderChapterPageLabel,
        ReaderDictionaryMode, ReaderLayout, ReaderLoadingStage, ReaderLocation, ReaderOrientation,
        ReaderPageLine, ReaderPreferences, ReaderSession, ReaderTickOutcome, ReaderUiState,
        ReadingPreference, ReadingTheme, TextEncoding, LEGACY_READER_POSITIONS_FILE,
        READER_BOOKMARKS_FILE, READER_CACHE_DIRECTORY, READER_POSITIONS_FILE, READER_PREFS_FILE,
        READER_RECENT_FILE, READER_STATE_FILE,
    };
    use crate::buttons::ButtonEvent;

    fn temp_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("rustmix-reader-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
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
            font_size: BookFontSize::Medium,
            book_font: BookFont::Serif,
            paragraph_alignment: ParagraphAlignment::Left,
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

    #[test]
    fn wraps_at_word_boundaries_instead_of_cutting_words() {
        // "hello world" is exactly 11 characters, so it packs onto one line;
        // "foo" does not fit alongside it and moves to its own line. Neither
        // word is ever cut mid-character.
        let decoded = decoded_from("hello world foo");
        let (lines, _) = paginate_decoded(&decoded, word_wrap_layout(11, 10), &monospace_width);
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["hello world", "foo"]);
        for line in &lines {
            assert!(line.text.chars().count() <= 11);
        }
    }

    #[test]
    fn a_word_longer_than_one_line_hard_breaks_without_dropping_characters() {
        let decoded = decoded_from("supercalifragilistic word");
        let (lines, _) = paginate_decoded(&decoded, word_wrap_layout(6, 10), &monospace_width);
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
            paginate_decoded(&decoded, word_wrap_layout(2, 1), &monospace_width);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "ab");
        // consumed must land right after "ab " (the committed word plus its
        // trailing separator), not mid-word, so the next page re-reads "cd
        // efgh" whole instead of splitting it across the page boundary.
        assert_eq!(consumed, 3);
        let remainder = decoded_from("cd efgh");
        let (next_lines, _) =
            paginate_decoded(&remainder, word_wrap_layout(2, 1), &monospace_width);
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
            paginate_decoded(&decoded, word_wrap_layout(10, 1), &monospace_width);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "ab");
        // consumed must land right after "ab\n", not at the end of the whole
        // decoded slice ("cdefgh" was never rendered on this page).
        assert_eq!(consumed, 3);
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
    fn preference_editor_uses_move_then_select_change_policy() {
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
        assert!(!reader.activate_selected_preference());
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

    fn epub_document_fixture() -> EpubDocument {
        EpubDocument {
            title: "Title\twith\ttabs, \\ backslash and \"quotes\"".into(),
            text: "Chapter one.\n\nChapter two with\ttab and \\ backslash.".into(),
            toc: vec![EpubTocEntry {
                label: "Start\nlabel".into(),
                text_offset: 0,
                spine_index: 0,
            }],
            chapters: vec![
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
            spine_count: 2,
        }
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

    #[test]
    fn epub_document_cache_round_trips_through_serialize_and_parse() {
        let book = epub_cache_book_fixture();
        let document = epub_document_fixture();
        let fingerprint = epub_document_fingerprint(&book);
        let serialized = serialize_epub_document_cache(&document, fingerprint);
        assert_eq!(
            parse_epub_document_cache(&serialized, &book).unwrap(),
            document
        );
    }

    #[test]
    fn epub_document_cache_rejects_fingerprint_mismatch() {
        let book = epub_cache_book_fixture();
        let mut other = book.clone();
        other.size_bytes = book.size_bytes + 1;
        let document = epub_document_fixture();
        let serialized = serialize_epub_document_cache(&document, epub_document_fingerprint(&book));
        assert!(parse_epub_document_cache(&serialized, &other).is_err());
    }

    #[test]
    fn epub_document_cache_rejects_truncated_text_payload() {
        let book = epub_cache_book_fixture();
        let document = epub_document_fixture();
        let serialized = serialize_epub_document_cache(&document, epub_document_fingerprint(&book));
        let truncated = &serialized[..serialized.len() - 5];
        assert!(parse_epub_document_cache(truncated, &book).is_err());
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
        let first_text = reader
            .session
            .as_ref()
            .unwrap()
            .epub_document
            .as_ref()
            .unwrap()
            .text
            .clone();
        assert!(first_text.contains("Cached chapter body."));

        // The `.EPX` write is deferred to the first background tick after
        // the page is shown (see `epub_document_cache_pending`), so it isn't
        // on disk yet right after `FirstPageReady` itself.
        assert!(reader.session.as_ref().unwrap().epub_document_cache_pending);
        reader.tick();
        assert!(!reader.session.as_ref().unwrap().epub_document_cache_pending);

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
        let cached_text = reopened
            .session
            .as_ref()
            .unwrap()
            .epub_document
            .as_ref()
            .unwrap()
            .text
            .clone();
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
}
