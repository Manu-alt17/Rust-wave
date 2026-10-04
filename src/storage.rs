//! SDMMC storage-browser model.
//!
//! ESP-IDF mounts the SD card at [`SD_MOUNT_POINT`]. This module owns
//! directory scans, bounded text previews and the one change the browser
//! can make to the card: deleting a single file, after a confirmation. It
//! never creates, renames or writes files, and never deletes a folder.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use log::{info, warn};

use crate::{buttons::ButtonEvent, regional::Locale};

/// VFS mount point used by the ESP-IDF FAT filesystem wrapper.
pub const SD_MOUNT_POINT: &str = "/sdcard";
/// Maximum number of visible rows on the portrait browser screen.
pub const STORAGE_PAGE_SIZE: usize = 7;
/// Maximum number of filesystem entries retained for one directory.
pub const MAX_STORAGE_ENTRIES: usize = 128;
/// Maximum number of bytes loaded for a bounded file preview.
pub const MAX_PREVIEW_BYTES: usize = 384;
/// SDMMC clock: the standard 20 MHz of default-speed cards. It had been
/// halved to 10 MHz after timeouts in the field, which the RTC FAST memory
/// fix in `sdkconfig.defaults` (SD buffers landing where DMA cannot reach)
/// explains better: on the board, 4 MB written and read back three times,
/// and every file under RUSTMIX hashed twice, gave no error and no
/// difference at 20 MHz. It gains little on its own (reads 12% faster):
/// what slowed reads down was PSRAM destinations, see `sd_io`.
pub const SDMMC_STABLE_SPEED_KHZ: u32 = 20_000;
/// Bounded command timeout used for SDMMC operations.
pub const SDMMC_COMMAND_TIMEOUT_MS: u32 = 1_000;
/// Total attempts for read-only filesystem operations after transient SDMMC errors.
pub const STORAGE_IO_RETRY_ATTEMPTS: usize = 3;
/// Delay between read-only retry attempts.
pub const STORAGE_IO_RETRY_DELAY_MS: u64 = 120;

/// Browser-entry category. Going up a folder or back to Home is BOOT's
/// job (see [`StorageBrowser::go_back`]), not a row of the list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageEntryKind {
    /// Synthetic row that retries a failed read-only directory scan.
    RetryScan,
    /// Filesystem directory.
    Directory,
    /// Regular filesystem file.
    File,
}

impl StorageEntryKind {
    /// Compact UI badge.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::RetryScan => "RETRY",
            Self::Directory => "DIR",
            Self::File => "FILE",
        }
    }

    /// Locale-aware sibling of [`Self::badge`] for on-screen display.
    /// `badge` itself is left untouched in case other code relies on its
    /// stable English output.
    #[must_use]
    pub const fn badge_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.badge(),
            Locale::Italian => match self {
                Self::RetryScan => "RIPROVA",
                Self::Directory => "CART",
                Self::File => "FILE",
            },
        }
    }
}

/// One read-only browser row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageEntry {
    pub name: String,
    pub kind: StorageEntryKind,
    pub size_bytes: Option<u64>,
}

impl StorageEntry {
    fn retry_scan() -> Self {
        Self {
            name: "Retry SD scan".into(),
            kind: StorageEntryKind::RetryScan,
            size_bytes: None,
        }
    }

    /// Compact size label for the right side of a browser row.
    #[must_use]
    pub fn size_label(&self) -> String {
        match self.kind {
            StorageEntryKind::File => self.size_bytes.map_or_else(|| "--".into(), format_bytes),
            _ => self.kind.badge().into(),
        }
    }
}

/// Bounded, read-only regular-file preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePreview {
    pub name: String,
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
}

impl FilePreview {
    /// Render a fixed number of short lines for the portrait preview screen.
    #[must_use]
    pub fn display_lines(&self, max_lines: usize, max_chars: usize) -> Vec<String> {
        if self.binary {
            return vec!["Binary file preview is disabled.".into()];
        }

        let mut lines = Vec::new();
        for raw_line in self.text.lines() {
            let mut remainder = raw_line.trim_end();
            if remainder.is_empty() {
                lines.push(String::new());
                if lines.len() >= max_lines {
                    break;
                }
                continue;
            }
            while !remainder.is_empty() && lines.len() < max_lines {
                let (line, rest) = split_at_char_boundary(remainder, max_chars);
                lines.push(line.to_string());
                remainder = rest;
            }
            if lines.len() >= max_lines {
                break;
            }
        }
        if lines.is_empty() {
            lines.push("(empty text file)".into());
        }
        lines
    }
}

/// Diagnostics captured for the most recent read-only directory scan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectoryScanStats {
    /// Raw directory entries returned by the mounted FAT VFS, excluding `.` and `..`.
    pub raw_entries: usize,
    /// Regular files and directories retained for the product browser.
    pub retained_entries: usize,
    /// Entries classified with metadata because the directory-entry type hint was incomplete.
    pub metadata_fallbacks: usize,
    /// Non-file, non-directory or symlink entries skipped by the browser.
    pub ignored_special: usize,
}

#[derive(Debug)]
struct DirectoryScanResult {
    entries: Vec<StorageEntry>,
    stats: DirectoryScanStats,
}

/// Hardware-independent storage snapshot consumed by product screens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageSnapshot {
    pub mounted: bool,
    pub current_path: String,
    pub entries: Vec<StorageEntry>,
    pub selected: usize,
    pub page_start: usize,
    pub preview: Option<FilePreview>,
    pub error: Option<String>,
    /// Diagnostics for the most recent read-only directory scan.
    pub scan: DirectoryScanStats,
    /// Whether the list shows the card's top folder: BOOT then leaves the
    /// browser instead of going up.
    pub at_root: bool,
    /// Name of the file a held SELECT asked to delete, until SELECT
    /// confirms or anything else cancels.
    pub pending_delete: Option<String>,
    /// How the last delete went, until the list changes again.
    pub notice: Option<StorageNotice>,
}

/// Outcome of a delete, for the screen to word in the user's language.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageNotice {
    /// The named file is gone.
    Deleted(String),
    /// The named file is still there; the second field says why.
    DeleteFailed(String, String),
}

impl Default for StorageSnapshot {
    fn default() -> Self {
        Self {
            mounted: false,
            current_path: SD_MOUNT_POINT.into(),
            entries: Vec::new(),
            selected: 0,
            page_start: 0,
            preview: None,
            error: None,
            scan: DirectoryScanStats::default(),
            at_root: true,
            pending_delete: None,
            notice: None,
        }
    }
}

impl StorageSnapshot {
    /// Product-facing availability label.
    #[must_use]
    pub fn status_label(&self) -> &'static str {
        if !self.mounted {
            "NO SD"
        } else if self.error.is_some() {
            "SD RETRY"
        } else if self.scan.retained_entries == 0 {
            "SD EMPTY"
        } else {
            "SD READY"
        }
    }

    /// Current page number and total page count.
    #[must_use]
    pub fn page_label(&self) -> String {
        let pages = self.entries.len().max(1).div_ceil(STORAGE_PAGE_SIZE);
        let current = (self.page_start / STORAGE_PAGE_SIZE) + 1;
        format!("{current}/{pages}")
    }

    /// Visible rows for the current page.
    #[must_use]
    pub fn visible_entries(&self) -> &[StorageEntry] {
        let start = self.page_start.min(self.entries.len());
        let end = (start + STORAGE_PAGE_SIZE).min(self.entries.len());
        &self.entries[start..end]
    }

    /// The row under the cursor.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&StorageEntry> {
        self.entries.get(self.selected)
    }

    /// Selected row index relative to the current page.
    #[must_use]
    pub fn selected_on_page(&self) -> usize {
        self.selected.saturating_sub(self.page_start)
    }
}

/// Result of one browser-level button event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageUiOutcome {
    None,
    SelectionChanged,
    DirectoryChanged,
    PreviewOpened,
    PreviewClosed,
    RetryRequested,
    /// A file was deleted, or the attempt failed: see the snapshot's notice.
    DeleteFinished,
    /// A delete waiting for its confirmation was dropped.
    DeleteCancelled,
}

/// Stateful directory browser.
#[derive(Debug)]
pub struct StorageBrowser {
    root: PathBuf,
    current: PathBuf,
    mounted: bool,
    entries: Vec<StorageEntry>,
    selected: usize,
    page_start: usize,
    preview: Option<FilePreview>,
    error: Option<String>,
    scan: DirectoryScanStats,
    pending_delete: Option<String>,
    notice: Option<StorageNotice>,
}

impl StorageBrowser {
    /// Create a browser rooted at the mounted SD-card path.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, mounted: bool) -> Self {
        let root = root.into();
        let mut browser = Self {
            current: root.clone(),
            root,
            mounted,
            entries: Vec::new(),
            selected: 0,
            page_start: 0,
            preview: None,
            error: None,
            scan: DirectoryScanStats::default(),
            pending_delete: None,
            notice: None,
        };
        browser.refresh();
        browser
    }

    /// Return a cloneable UI snapshot without leaking filesystem handles.
    #[must_use]
    pub fn snapshot(&self) -> StorageSnapshot {
        StorageSnapshot {
            mounted: self.mounted,
            current_path: self.display_path(),
            entries: self.entries.clone(),
            selected: self.selected,
            page_start: self.page_start,
            preview: self.preview.clone(),
            error: self.error.clone(),
            scan: self.scan,
            at_root: self.current == self.root,
            pending_delete: self.pending_delete.clone(),
            notice: self.notice.clone(),
        }
    }

    /// Rescan the current directory. The shell remains navigable when no card
    /// is inserted or a directory cannot be read.
    pub fn refresh(&mut self) {
        self.preview = None;
        self.error = None;
        self.pending_delete = None;
        self.notice = None;
        self.scan = DirectoryScanStats::default();
        self.entries.clear();

        if !self.mounted {
            self.error = Some("Insert a FAT-formatted SD card and reboot.".into());
            self.normalize_selection();
            return;
        }

        match read_directory_entries_with_retry(&self.current) {
            Ok(mut scan) => {
                self.scan = scan.stats;
                info!(
                    "rustmix-wave=storage-directory-scan path={} raw-entries={} retained-entries={} metadata-fallbacks={} ignored-special={}",
                    self.display_path(),
                    self.scan.raw_entries,
                    self.scan.retained_entries,
                    self.scan.metadata_fallbacks,
                    self.scan.ignored_special
                );
                if self.scan.raw_entries == 0 {
                    info!(
                        "rustmix-wave=storage-directory-empty path={} reason=no-fat-entries",
                        self.display_path()
                    );
                }
                self.entries.append(&mut scan.entries);
            }
            Err(error) => {
                self.error = Some(format!(
                    "Directory scan failed after {STORAGE_IO_RETRY_ATTEMPTS} attempts: {error}"
                ));
                self.entries.push(StorageEntry::retry_scan());
            }
        }
        self.normalize_selection();
    }

    /// Apply one debounced app button while the Files route is active.
    pub fn apply_button(&mut self, event: ButtonEvent) -> StorageUiOutcome {
        // A delete waiting for its confirmation: SELECT deletes, the rocker
        // cancels without moving.
        if let Some(name) = self.pending_delete.take() {
            return if event == ButtonEvent::Select {
                self.delete_file(&name)
            } else {
                StorageUiOutcome::DeleteCancelled
            };
        }
        if self.preview.is_some() {
            if event == ButtonEvent::Select {
                self.preview = None;
                return StorageUiOutcome::PreviewClosed;
            }
            return StorageUiOutcome::None;
        }

        match event {
            ButtonEvent::Up => {
                if self.entries.is_empty() {
                    return StorageUiOutcome::None;
                }
                self.notice = None;
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(self.entries.len() - 1);
                self.update_page_start();
                StorageUiOutcome::SelectionChanged
            }
            ButtonEvent::Down => {
                if self.entries.is_empty() {
                    return StorageUiOutcome::None;
                }
                self.notice = None;
                self.selected = (self.selected + 1) % self.entries.len();
                self.update_page_start();
                StorageUiOutcome::SelectionChanged
            }
            ButtonEvent::Select => self.activate_selected(),
        }
    }

    fn activate_selected(&mut self) -> StorageUiOutcome {
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            return StorageUiOutcome::None;
        };
        match entry.kind {
            StorageEntryKind::RetryScan => {
                self.refresh();
                StorageUiOutcome::RetryRequested
            }
            StorageEntryKind::Directory => {
                let candidate = self.current.join(&entry.name);
                if candidate.starts_with(&self.root) {
                    self.current = candidate;
                    self.selected = 0;
                    self.page_start = 0;
                    self.refresh();
                    StorageUiOutcome::DirectoryChanged
                } else {
                    self.error = Some("Blocked directory traversal outside SD root.".into());
                    StorageUiOutcome::None
                }
            }
            StorageEntryKind::File => {
                let candidate = self.current.join(&entry.name);
                match read_preview_with_retry(&self.root, &candidate) {
                    Ok(preview) => {
                        self.preview = Some(preview);
                        StorageUiOutcome::PreviewOpened
                    }
                    Err(error) => {
                        self.error = Some(format!("Preview unavailable: {error}"));
                        StorageUiOutcome::None
                    }
                }
            }
        }
    }

    /// BOOT: drop a delete waiting for its confirmation, else close the
    /// preview, else go up one folder with the cursor on the folder just
    /// left. `false` at the top folder, where BOOT leaves the browser.
    pub fn go_back(&mut self) -> bool {
        if self.pending_delete.take().is_some() {
            return true;
        }
        if self.preview.take().is_some() {
            return true;
        }
        if self.current == self.root {
            return false;
        }
        let left = self
            .current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        match self.current.parent() {
            Some(parent) if parent.starts_with(&self.root) => {
                self.current = parent.to_path_buf();
            }
            _ => self.current = self.root.clone(),
        }
        self.selected = 0;
        self.page_start = 0;
        self.refresh();
        if let Some(index) = left.and_then(|left| {
            self.entries
                .iter()
                .position(|entry| entry.kind == StorageEntryKind::Directory && entry.name == left)
        }) {
            self.selected = index;
            self.update_page_start();
        }
        true
    }

    /// Held SELECT on a file: ask to delete it. `true` when the request was
    /// taken; folders and the preview do not take it.
    pub fn request_delete(&mut self) -> bool {
        if self.preview.is_some() {
            return false;
        }
        match self.entries.get(self.selected) {
            Some(entry) if entry.kind == StorageEntryKind::File => {
                self.notice = None;
                self.pending_delete = Some(entry.name.clone());
                true
            }
            _ => false,
        }
    }

    /// Delete the file `name` of the current folder and read the folder
    /// again; the cursor stays where the file was.
    fn delete_file(&mut self, name: &str) -> StorageUiOutcome {
        let candidate = self.current.join(name);
        let result = if candidate.starts_with(&self.root) && candidate.is_file() {
            fs::remove_file(&candidate)
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "not a file"))
        };
        let selected = self.selected;
        self.refresh();
        self.selected = selected;
        self.normalize_selection();
        self.notice = Some(match result {
            Ok(()) => {
                info!("rustmix-wave=storage-delete status=deleted name={name:?}");
                StorageNotice::Deleted(name.to_string())
            }
            Err(error) => {
                warn!("rustmix-wave=storage-delete status=failed name={name:?} error={error}");
                StorageNotice::DeleteFailed(name.to_string(), error.to_string())
            }
        });
        StorageUiOutcome::DeleteFinished
    }

    fn normalize_selection(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
            self.page_start = 0;
        } else {
            self.selected = self.selected.min(self.entries.len() - 1);
            self.update_page_start();
        }
    }

    fn update_page_start(&mut self) {
        self.page_start = (self.selected / STORAGE_PAGE_SIZE) * STORAGE_PAGE_SIZE;
    }

    fn display_path(&self) -> String {
        if self.current == self.root {
            return SD_MOUNT_POINT.into();
        }
        self.current.strip_prefix(&self.root).map_or_else(
            |_| SD_MOUNT_POINT.into(),
            |relative| format!("{SD_MOUNT_POINT}/{}", relative.display()),
        )
    }
}

fn read_directory_entries_with_retry(path: &Path) -> io::Result<DirectoryScanResult> {
    retry_readonly_io("directory-scan", || read_directory_entries(path))
}

fn read_directory_entries(path: &Path) -> io::Result<DirectoryScanResult> {
    let mut entries = Vec::new();
    let mut stats = DirectoryScanStats::default();
    for entry in fs::read_dir(path)? {
        if entries.len() >= MAX_STORAGE_ENTRIES {
            break;
        }
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "." || name == ".." {
            continue;
        }
        stats.raw_entries += 1;

        let hinted_type = entry.file_type()?;
        if hinted_type.is_symlink() {
            stats.ignored_special += 1;
            continue;
        }

        // ESP-IDF FAT VFS caches the FILINFO collected by readdir so the
        // immediately following stat call is inexpensive. Prefer metadata for
        // final classification because some VFS implementations expose an
        // incomplete d_type hint even when the filesystem entry is valid.
        let metadata = entry.metadata()?;
        let metadata_type = metadata.file_type();
        let hint_is_incomplete = !hinted_type.is_dir() && !hinted_type.is_file();
        if hint_is_incomplete {
            stats.metadata_fallbacks += 1;
        }

        let (kind, size_bytes) = if metadata_type.is_dir() {
            (StorageEntryKind::Directory, None)
        } else if metadata_type.is_file() {
            (StorageEntryKind::File, Some(metadata.len()))
        } else {
            stats.ignored_special += 1;
            continue;
        };
        entries.push(StorageEntry {
            name,
            kind,
            size_bytes,
        });
    }
    entries.sort_by(|left, right| {
        storage_sort_rank(left.kind)
            .cmp(&storage_sort_rank(right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    stats.retained_entries = entries.len();
    Ok(DirectoryScanResult { entries, stats })
}

fn read_preview_with_retry(root: &Path, path: &Path) -> io::Result<FilePreview> {
    retry_readonly_io("preview-read", || read_preview(root, path))
}

fn retry_readonly_io<T>(
    operation: &str,
    mut action: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let mut last_error = None;
    for attempt in 1..=STORAGE_IO_RETRY_ATTEMPTS {
        match action() {
            Ok(value) => {
                if attempt > 1 {
                    warn!(
                        "rustmix-wave=storage-io-recovered operation={operation} attempt={attempt}/{STORAGE_IO_RETRY_ATTEMPTS}"
                    );
                }
                return Ok(value);
            }
            Err(error) => {
                warn!(
                    "rustmix-wave=storage-io-retry operation={operation} attempt={attempt}/{STORAGE_IO_RETRY_ATTEMPTS} error={error}"
                );
                last_error = Some(error);
                if attempt < STORAGE_IO_RETRY_ATTEMPTS {
                    thread::sleep(Duration::from_millis(STORAGE_IO_RETRY_DELAY_MS));
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::other("read-only storage retry policy executed without an attempt")
    }))
}

fn read_preview(root: &Path, path: &Path) -> io::Result<FilePreview> {
    if !path.starts_with(root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path is outside the SD-card root",
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "preview requires a regular file",
        ));
    }

    let mut file = File::open(path)?;
    let mut bytes = vec![0; MAX_PREVIEW_BYTES + 1];
    let read = file.read(&mut bytes)?;
    bytes.truncate(read);
    let truncated = bytes.len() > MAX_PREVIEW_BYTES;
    if truncated {
        bytes.truncate(MAX_PREVIEW_BYTES);
    }
    let binary = bytes
        .iter()
        .any(|byte| byte.is_ascii_control() && !matches!(*byte, b'\n' | b'\r' | b'\t'));
    let text = if binary {
        String::new()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Ok(FilePreview {
        name: path.file_name().map_or_else(
            || "(unnamed)".into(),
            |name| name.to_string_lossy().into_owned(),
        ),
        text,
        truncated,
        binary,
    })
}

const fn storage_sort_rank(kind: StorageEntryKind) -> u8 {
    match kind {
        StorageEntryKind::RetryScan => 1,
        StorageEntryKind::Directory => 2,
        StorageEntryKind::File => 3,
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{} MB", bytes.div_ceil(1024 * 1024))
    }
}

fn split_at_char_boundary(value: &str, max_chars: usize) -> (&str, &str) {
    if value.chars().count() <= max_chars {
        return (value, "");
    }
    let byte_index = value
        .char_indices()
        .nth(max_chars)
        .map_or(value.len(), |(index, _)| index);
    (&value[..byte_index], value[byte_index..].trim_start())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        retry_readonly_io, StorageBrowser, StorageEntryKind, StorageNotice, StorageUiOutcome,
        MAX_PREVIEW_BYTES,
    };
    use crate::buttons::ButtonEvent;

    fn fixture_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("epd397-{label}-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn unavailable_card_remains_navigable() {
        let browser = StorageBrowser::new("/missing-sdcard", false);
        let snapshot = browser.snapshot();
        assert!(!snapshot.mounted);
        assert!(snapshot.entries.is_empty());
        assert!(snapshot.error.is_some());
        assert!(snapshot.at_root);
        assert!(snapshot.visible_entries().is_empty());
    }

    #[test]
    fn mounted_empty_directory_reports_sd_empty() {
        let root = fixture_root("empty");
        let browser = StorageBrowser::new(&root, true);
        let snapshot = browser.snapshot();
        assert_eq!(snapshot.status_label(), "SD EMPTY");
        assert_eq!(snapshot.scan.raw_entries, 0);
        assert_eq!(snapshot.scan.retained_entries, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_scan_sorts_directories_before_files() {
        let root = fixture_root("sort");
        fs::create_dir(root.join("z-dir")).unwrap();
        fs::write(root.join("a-file.txt"), b"hello").unwrap();
        let browser = StorageBrowser::new(&root, true);
        let snapshot = browser.snapshot();
        assert_eq!(snapshot.entries[0].kind, StorageEntryKind::Directory);
        assert_eq!(snapshot.entries[1].kind, StorageEntryKind::File);
        assert_eq!(snapshot.scan.raw_entries, 2);
        assert_eq!(snapshot.scan.retained_entries, 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn boot_goes_up_one_folder_and_leaves_only_from_the_top() {
        let root = fixture_root("navigate");
        fs::create_dir(root.join("audio")).unwrap();
        fs::create_dir_all(root.join("books").join("inner")).unwrap();
        let mut browser = StorageBrowser::new(&root, true);
        assert!(browser.snapshot().at_root);
        // "audio" sorts first: move to "books" and enter it, then "inner".
        browser.apply_button(ButtonEvent::Down);
        assert_eq!(
            browser.apply_button(ButtonEvent::Select),
            StorageUiOutcome::DirectoryChanged
        );
        assert_eq!(
            browser.apply_button(ButtonEvent::Select),
            StorageUiOutcome::DirectoryChanged
        );
        // The separator inside the card follows the host the test runs on.
        let shown = |browser: &StorageBrowser| browser.snapshot().current_path.replace('\\', "/");
        assert!(shown(&browser).ends_with("/books/inner"));
        assert!(!browser.snapshot().at_root);
        assert!(browser.snapshot().entries.is_empty());

        assert!(browser.go_back());
        assert!(shown(&browser).ends_with("/books"));
        assert!(browser.go_back());
        let snapshot = browser.snapshot();
        assert!(snapshot.at_root);
        // The cursor is on the folder just left, not back at the top.
        assert_eq!(snapshot.selected_entry().unwrap().name, "books");
        assert!(!browser.go_back());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_is_deleted_only_after_the_confirming_select() {
        let root = fixture_root("delete");
        fs::create_dir(root.join("keep")).unwrap();
        fs::write(root.join("a.txt"), b"one").unwrap();
        fs::write(root.join("b.txt"), b"two").unwrap();
        let mut browser = StorageBrowser::new(&root, true);

        // A folder does not take the request.
        assert!(!browser.request_delete());
        browser.apply_button(ButtonEvent::Down);
        assert!(browser.request_delete());
        assert_eq!(browser.snapshot().pending_delete.as_deref(), Some("a.txt"));

        // The rocker and BOOT cancel; the file stays.
        assert_eq!(
            browser.apply_button(ButtonEvent::Down),
            StorageUiOutcome::DeleteCancelled
        );
        assert_eq!(browser.snapshot().selected, 1);
        assert!(browser.request_delete());
        assert!(browser.go_back());
        assert!(browser.snapshot().pending_delete.is_none());
        assert!(root.join("a.txt").exists());

        assert!(browser.request_delete());
        assert_eq!(
            browser.apply_button(ButtonEvent::Select),
            StorageUiOutcome::DeleteFinished
        );
        assert!(!root.join("a.txt").exists());
        assert!(root.join("b.txt").exists());
        let snapshot = browser.snapshot();
        assert_eq!(
            snapshot.notice,
            Some(StorageNotice::Deleted("a.txt".into()))
        );
        // The cursor stays in place, now on the next file.
        assert_eq!(snapshot.selected_entry().unwrap().name, "b.txt");
        assert_eq!(snapshot.entries.len(), 2);

        // The preview does not take the request either.
        browser.apply_button(ButtonEvent::Select);
        assert!(browser.snapshot().preview.is_some());
        assert!(!browser.request_delete());
        assert!(browser.go_back());
        assert!(browser.snapshot().preview.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opens_bounded_text_preview_and_closes_it() {
        let root = fixture_root("preview");
        fs::write(root.join("notes.txt"), vec![b'a'; MAX_PREVIEW_BYTES + 20]).unwrap();
        let mut browser = StorageBrowser::new(&root, true);
        assert_eq!(
            browser.apply_button(ButtonEvent::Select),
            StorageUiOutcome::PreviewOpened
        );
        let preview = browser.snapshot().preview.unwrap();
        assert!(preview.truncated);
        assert!(!preview.binary);
        assert_eq!(preview.text.len(), MAX_PREVIEW_BYTES);
        assert_eq!(
            browser.apply_button(ButtonEvent::Select),
            StorageUiOutcome::PreviewClosed
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_preview_is_reported_without_rendering_bytes() {
        let root = fixture_root("binary");
        fs::write(root.join("image.bin"), [0, 1, 2, 3]).unwrap();
        let mut browser = StorageBrowser::new(&root, true);
        browser.apply_button(ButtonEvent::Select);
        let preview = browser.snapshot().preview.unwrap();
        assert!(preview.binary);
        assert!(preview.text.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retry_helper_recovers_after_transient_failures() {
        let mut attempts = 0;
        let value = retry_readonly_io("fixture", || {
            attempts += 1;
            if attempts < 3 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "transient",
                ))
            } else {
                Ok("ready")
            }
        })
        .unwrap();
        assert_eq!(value, "ready");
        assert_eq!(attempts, 3);
    }

    #[test]
    fn failed_scan_exposes_retry_row_and_warning_status() {
        let mut browser = StorageBrowser::new("/definitely-missing-sdcard-root", true);
        let snapshot = browser.snapshot();
        assert_eq!(snapshot.status_label(), "SD RETRY");
        assert_eq!(snapshot.entries[0].kind, StorageEntryKind::RetryScan);
        assert_eq!(
            browser.apply_button(ButtonEvent::Select),
            StorageUiOutcome::RetryRequested
        );
    }
}
