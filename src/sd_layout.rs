//! Where things live on the microSD card, and the move from the layout of
//! firmware up to v1.5.0-beta.5, which kept everything in one `RUSTMIX`
//! folder.
//!
//! ```text
//! Books/        books, EPUB and TXT
//! Audiobooks/   MP3 audiobooks: one file, or one folder of tracks, per title
//! Wallpapers/   sleep-screen images
//! System/       what the device manages by itself: settings, reading
//!               positions, statistics, caches, the dictionary
//! ```
//!
//! [`migrate_legacy_layout`] runs right after the card is mounted, before
//! anything reads it. It moves rather than copies: a rename on the same FAT
//! volume is quick whatever the size, and the files keep the dates the
//! reading positions and statistics recognise books by. It never overwrites
//! anything, and it picks up where it stopped if the power goes in the
//! middle: whatever is still in `RUSTMIX` moves on the next boot.

use std::{
    collections::HashMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::reading_stats::book_id_for;

/// Books, EPUB and TXT.
pub const BOOKS_FOLDER: &str = "Books";
/// MP3 audiobooks.
pub const AUDIOBOOKS_FOLDER: &str = "Audiobooks";
/// Sleep-screen images.
pub const WALLPAPERS_FOLDER: &str = "Wallpapers";
/// What the device manages by itself.
pub const SYSTEM_FOLDER: &str = "System";
/// The one folder of firmware up to v1.5.0-beta.5.
pub const LEGACY_FOLDER: &str = "RUSTMIX";

/// The legacy folders that become top-level folders of the card. Everything
/// else in `RUSTMIX` goes into [`SYSTEM_FOLDER`].
const CONTENT_MOVES: [(&str, &str); 3] = [
    ("BOOKS", BOOKS_FOLDER),
    ("AUDIO", AUDIOBOOKS_FOLDER),
    ("SLEEP", WALLPAPERS_FOLDER),
];

/// Legacy caches, keyed by the books' paths and so of no use once the books
/// moved: dropped instead of moved, and rebuilt as books are opened.
const LEGACY_CACHES: [&str; 2] = ["READER/CACHE", "CACHE"];

/// Legacy files with no place in the new layout: the "most used" history of
/// Settings, which keeps one fixed order now, and the read-me files that
/// described `RUSTMIX`.
const LEGACY_DROPPED_FILES: [&str; 3] = ["MENU.TXT", "LEGGIMI.TXT", "README.TXT"];

/// Files under [`SYSTEM_FOLDER`] that name books by their full path, each
/// with the `.BAK` the Reader keeps of it.
const PATH_FILES: [&str; 5] = [
    "READER/STATE",
    "READER/POSITS",
    "READER/POSITIONS",
    "READER/RECENT",
    "READER/MARKS",
];

/// Full paths as the legacy firmware wrote them (the card is always mounted
/// at `/sdcard`), and what they become: the content folders before the
/// folder itself.
const PATH_PREFIXES: [(&str, &str); 4] = [
    ("/sdcard/RUSTMIX/BOOKS/", "/sdcard/Books/"),
    ("/sdcard/RUSTMIX/AUDIO/", "/sdcard/Audiobooks/"),
    ("/sdcard/RUSTMIX/SLEEP/", "/sdcard/Wallpapers/"),
    ("/sdcard/RUSTMIX/", "/sdcard/System/"),
];

/// The monthly reading-statistics logs, under [`SYSTEM_FOLDER`].
const STATS_FOLDER: &str = "STATS";

/// Bytes compared at a time when a file meets one of the same name.
const COMPARE_BLOCK_BYTES: usize = 16 * 1024;

/// What a migration did, for the boot log.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Migration {
    /// Files and folders moved out of `RUSTMIX`.
    pub moved: usize,
    /// Of those, the files moved under another name (`name (2).ext`), since
    /// a different file of the same name was already where they go.
    pub renamed: usize,
    /// Entries left in `RUSTMIX`: a file where a folder of the same name
    /// is, or the other way round, or a move that failed. Tried again on the
    /// next boot.
    pub kept: usize,
    /// Files whose book paths were rewritten.
    pub path_files: usize,
    /// Statistics lines whose book was rewritten.
    pub stats_lines: usize,
    /// Whether `RUSTMIX` is gone.
    pub finished: bool,
}

/// Moves a card in the legacy layout to the new one. `None` when there is
/// no `RUSTMIX` folder, which is every boot but the first one after the
/// update. A single entry that cannot be moved does not stop the rest.
pub fn migrate_legacy_layout(root: impl AsRef<Path>) -> Option<Migration> {
    let root = root.as_ref();
    let legacy = root.join(LEGACY_FOLDER);
    if !legacy.is_dir() {
        return None;
    }
    let mut report = Migration::default();
    // Only the legacy copies: a run left unfinished must not drop the
    // caches rebuilt since.
    for cache in LEGACY_CACHES {
        let _ = fs::remove_dir_all(legacy.join(cache));
    }
    for file in LEGACY_DROPPED_FILES {
        let _ = fs::remove_file(legacy.join(file));
    }
    for (from, to) in CONTENT_MOVES {
        fix_folder_case(root, to);
        let from = legacy.join(from);
        if from.is_dir() {
            move_into(&from, &root.join(to), &mut report);
        }
    }
    let system = root.join(SYSTEM_FOLDER);
    let _ = fs::create_dir_all(&system);
    for name in entry_names(&legacy) {
        // A content folder still here holds what could not be moved: it
        // waits for the next boot, and never goes into System.
        let content = CONTENT_MOVES
            .iter()
            .any(|(from, _)| name.to_string_lossy().eq_ignore_ascii_case(from));
        if !content {
            move_into(&legacy.join(&name), &system.join(&name), &mut report);
        }
    }
    // Every run, not only the one that moved the files: one cut short
    // before this point finds nothing left to move.
    report.path_files = rewrite_paths(&system);
    report.stats_lines = rewrite_statistics(root);
    report.finished = fs::remove_dir(&legacy).is_ok();
    Some(report)
}

/// The names in a folder, read before anything in it moves.
fn entry_names(folder: &Path) -> Vec<std::ffi::OsString> {
    fs::read_dir(folder)
        .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
        .unwrap_or_default()
}

/// FAT keeps the case a folder was made with, and a card used by older
/// firmware can have `books` where the layout says `Books`: renamed through
/// a temporary name, since FAT takes the two for one and the same. A rename
/// cut short by the power is finished on the next boot.
fn fix_folder_case(root: &Path, wanted: &str) {
    let target = root.join(wanted);
    let temporary = root.join(format!("{wanted}.MOVING"));
    let names = entry_names(root);
    if temporary.is_dir() && !names.iter().any(|name| name.eq_ignore_ascii_case(wanted)) {
        let _ = fs::rename(&temporary, &target);
        return;
    }
    // Where names are case-sensitive, as on the host, both can exist.
    if names.iter().any(|name| name == wanted) {
        return;
    }
    let Some(actual) = names
        .iter()
        .find(|name| name.to_string_lossy().eq_ignore_ascii_case(wanted))
    else {
        return;
    };
    if fs::rename(root.join(actual), &temporary).is_ok() {
        let _ = fs::rename(&temporary, &target);
    }
}

/// Moves `from` to `to`, and nothing is lost on the way. Onto a folder that
/// is already there, a folder moves what it holds one entry at a time. Onto
/// a file of the same name: when the two hold the same bytes, the moving
/// one takes its place, as it is the copy the reading positions and the
/// statistics know (they recognise a book by its date too); when they
/// differ, both stay, the moving one under the first free `name (2).ext`.
fn move_into(from: &Path, to: &Path, report: &mut Migration) {
    let outcome = match fs::metadata(to) {
        Err(error) if error.kind() == ErrorKind::NotFound => fs::rename(from, to),
        Ok(existing) if existing.is_dir() && from.is_dir() => {
            for name in entry_names(from) {
                move_into(&from.join(&name), &to.join(&name), report);
            }
            let _ = fs::remove_dir(from);
            return;
        }
        Ok(existing) if existing.is_file() && from.is_file() => {
            if same_contents(from, to) {
                // FAT renames never replace: the other copy goes first.
                fs::remove_file(to).and_then(|()| fs::rename(from, to))
            } else if let Some(free) = free_name(to) {
                report.renamed += 1;
                fs::rename(from, free)
            } else {
                Err(ErrorKind::AlreadyExists.into())
            }
        }
        _ => Err(ErrorKind::AlreadyExists.into()),
    };
    match outcome {
        Ok(()) => report.moved += 1,
        Err(_) => report.kept += 1,
    }
}

/// Whether two files hold the same bytes, read side by side a block at a
/// time.
fn same_contents(left: &Path, right: &Path) -> bool {
    let (Ok(left_meta), Ok(right_meta)) = (fs::metadata(left), fs::metadata(right)) else {
        return false;
    };
    if left_meta.len() != right_meta.len() {
        return false;
    }
    let (Ok(mut left), Ok(mut right)) = (fs::File::open(left), fs::File::open(right)) else {
        return false;
    };
    let mut left_block = vec![0_u8; COMPARE_BLOCK_BYTES];
    let mut right_block = vec![0_u8; COMPARE_BLOCK_BYTES];
    loop {
        let (Ok(read), Ok(other)) = (
            crate::sd_io::read_full(&mut left, &mut left_block),
            crate::sd_io::read_full(&mut right, &mut right_block),
        ) else {
            return false;
        };
        if read != other || left_block[..read] != right_block[..read] {
            return false;
        }
        if read < COMPARE_BLOCK_BYTES {
            return true;
        }
    }
}

/// `name (2).ext`, `name (3).ext`... beside `path`: the first one free.
fn free_name(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    (2..100)
        .map(|number| path.with_file_name(format!("{stem} ({number}){extension}")))
        .find(|candidate| {
            fs::metadata(candidate).is_err_and(|error| error.kind() == ErrorKind::NotFound)
        })
}

/// Rewrites the book paths in the Reader's files, and in their backups.
/// Returns how many files changed.
fn rewrite_paths(system: &Path) -> usize {
    let mut changed = 0;
    for file in PATH_FILES {
        for extension in ["TXT", "BAK"] {
            let path = system.join(format!("{file}.{extension}"));
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let rewritten = PATH_PREFIXES
                .iter()
                .fold(text.clone(), |text, (old, new)| text.replace(old, new));
            if rewritten == text {
                continue;
            }
            // The file itself goes through the Reader's power-safe replace,
            // which keeps the old one as `.BAK`, rewritten next in place.
            // The legacy `POSITIONS.TXT` is no 8.3 name, which that replace
            // refuses: it is written in place too.
            let written = (extension == "TXT"
                && crate::reader::atomic_replace_text(&path, &rewritten).is_ok())
                || fs::write(&path, &rewritten).is_ok();
            if written {
                changed += 1;
            }
        }
    }
    changed
}

/// The statistics know a book by a fingerprint of its path, size and date
/// (`reading_stats::book_id_for`): the books moved, so the fingerprint of
/// every book in the library is redone and replaced in the monthly logs.
/// Size and date did not change with the move. Returns the lines changed.
fn rewrite_statistics(root: &Path) -> usize {
    let mut ids = HashMap::new();
    if let Ok(entries) = fs::read_dir(root.join(BOOKS_FOLDER)) {
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let modified = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs());
            let size = metadata.len();
            ids.insert(
                book_id_for(&format!("/sdcard/RUSTMIX/BOOKS/{name}"), size, modified),
                book_id_for(&format!("/sdcard/Books/{name}"), size, modified),
            );
        }
    }
    if ids.is_empty() {
        return 0;
    }
    let stats = root.join(SYSTEM_FOLDER).join(STATS_FOLDER);
    let mut changed = 0;
    for name in entry_names(&stats) {
        let path: PathBuf = stats.join(&name);
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("LOG"))
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let mut lines_changed = 0;
        let mut rewritten = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let replaced = line.split_once('|').and_then(|(id, rest)| {
                let new = ids.get(&id.trim().parse::<u32>().ok()?)?;
                Some(format!("{new}|{rest}"))
            });
            match replaced {
                Some(line) => {
                    lines_changed += 1;
                    rewritten.push_str(&line);
                }
                None => rewritten.push_str(line),
            }
        }
        if lines_changed > 0 && crate::reader::atomic_replace_text(&path, &rewritten).is_ok() {
            // The log keeps no backup of its own.
            let _ = fs::remove_file(path.with_extension("BAK"));
            changed += lines_changed;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{migrate_legacy_layout, Migration};
    use crate::reading_stats::book_id_for;

    fn card(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rustmix-layout-{name}-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn modified_seconds(path: &Path) -> u64 {
        fs::metadata(path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn a_card_in_the_new_layout_is_left_alone() {
        let root = card("new");
        write(&root.join("System/WIFI.TXT"), "networks");
        assert_eq!(migrate_legacy_layout(&root), None);
        assert!(root.join("System/WIFI.TXT").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_legacy_card_moves_with_its_positions_and_statistics() {
        let root = card("legacy");
        let legacy = root.join("RUSTMIX");
        write(&legacy.join("BOOKS/Verity.epub"), "a book");
        write(&legacy.join("AUDIO/Tale/01.mp3"), "a track");
        write(&legacy.join("SLEEP/SLEEP001.BMP"), "a picture");
        write(&legacy.join("WIFI.TXT"), "networks");
        write(&legacy.join("MENU.TXT"), "settings=Audio\n");
        write(&legacy.join("README.TXT"), "what is in RUSTMIX");
        write(&legacy.join("APPS/DICT/INDEX.TXT"), "dictionary");
        write(&legacy.join("READER/CACHE/1234ABCD.EPX"), "stale cache");
        write(&legacy.join("CACHE/5678ABCD.THB"), "stale cover");
        let state = "version=1\npath=/sdcard/RUSTMIX/BOOKS/Verity.epub\ntitle=Verity\n";
        write(&legacy.join("READER/STATE.TXT"), state);
        write(&legacy.join("READER/STATE.BAK"), state);
        write(
            &legacy.join("READER/POSITS.TXT"),
            "version=1\nentry=/sdcard/RUSTMIX/BOOKS/Verity.epub\tVerity\tepub\n",
        );
        write(&legacy.join("READER/PREFS.TXT"), "font=serif\n");

        // A session logged against the book where it was.
        let book = legacy.join("BOOKS/Verity.epub");
        let (size, modified) = (fs::metadata(&book).unwrap().len(), modified_seconds(&book));
        let old_id = book_id_for("/sdcard/RUSTMIX/BOOKS/Verity.epub", size, modified);
        let new_id = book_id_for("/sdcard/Books/Verity.epub", size, modified);
        write(
            &legacy.join("STATS/202610.LOG"),
            &format!("{old_id}|100|200|0|500|3\n77|300|400|0|10|1\n"),
        );

        let report = migrate_legacy_layout(&root).unwrap();
        assert!(report.finished, "{report:?}");
        assert_eq!(report.kept, 0);
        assert!(!legacy.exists());

        assert_eq!(
            fs::read_to_string(root.join("Books/Verity.epub")).unwrap(),
            "a book"
        );
        assert!(root.join("Audiobooks/Tale/01.mp3").is_file());
        assert!(root.join("Wallpapers/SLEEP001.BMP").is_file());
        assert!(root.join("System/WIFI.TXT").is_file());
        assert!(root.join("System/APPS/DICT/INDEX.TXT").is_file());
        assert!(root.join("System/READER/PREFS.TXT").is_file());
        // Gone: the menu history, the old read-me, the caches keyed by path.
        assert!(!root.join("System/MENU.TXT").exists());
        assert!(!root.join("System/README.TXT").exists());
        assert!(!root.join("System/READER/CACHE").exists());
        assert!(!root.join("System/CACHE").exists());

        let expected_state = "version=1\npath=/sdcard/Books/Verity.epub\ntitle=Verity\n";
        assert_eq!(
            fs::read_to_string(root.join("System/READER/STATE.TXT")).unwrap(),
            expected_state
        );
        assert_eq!(
            fs::read_to_string(root.join("System/READER/STATE.BAK")).unwrap(),
            expected_state
        );
        assert!(fs::read_to_string(root.join("System/READER/POSITS.TXT"))
            .unwrap()
            .contains("entry=/sdcard/Books/Verity.epub\t"));
        // The book's sessions follow it; another book's line is untouched.
        assert_eq!(
            fs::read_to_string(root.join("System/STATS/202610.LOG")).unwrap(),
            format!("{new_id}|100|200|0|500|3\n77|300|400|0|10|1\n")
        );
        assert!(!root.join("System/STATS/202610.BAK").exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// Seen on a real card: the top-level `books` of an older firmware
    /// already held the same books as `RUSTMIX/BOOKS`.
    #[test]
    fn a_name_already_taken_loses_nothing() {
        let root = card("conflict");
        let legacy = root.join("RUSTMIX");
        // The same book in both places: the legacy copy, whose date the
        // reading positions know, is the one kept.
        write(&legacy.join("BOOKS/Same.epub"), "same book");
        write(&root.join("books/Same.epub"), "same book");
        let old_date = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        fs::File::options()
            .write(true)
            .open(legacy.join("BOOKS/Same.epub"))
            .unwrap()
            .set_modified(old_date)
            .unwrap();
        // Two different files of one name: both stay.
        write(&legacy.join("BOOKS/Other.epub"), "legacy other");
        write(&root.join("books/Other.epub"), "new other");
        write(&legacy.join("CLOCK.TXT"), "legacy clock");
        write(&root.join("System/CLOCK.TXT"), "new clock");
        // A file where a folder of the same name is: left where it was.
        write(&legacy.join("BOOKS/Comic"), "a file");
        write(&root.join("books/Comic/001.jpg"), "a page");

        let report = migrate_legacy_layout(&root).unwrap();
        assert_eq!(
            (report.moved, report.renamed, report.kept, report.finished),
            (3, 2, 1, false),
            "{report:?}"
        );
        // `books` is `Books` now, as the layout names it.
        let names: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"Books".to_string()), "{names:?}");
        assert!(!names.contains(&"books".to_string()), "{names:?}");

        let same = root.join("Books/Same.epub");
        assert_eq!(fs::read_to_string(&same).unwrap(), "same book");
        assert_eq!(modified_seconds(&same), 1_700_000_000);
        assert_eq!(
            fs::read_to_string(root.join("Books/Other.epub")).unwrap(),
            "new other"
        );
        assert_eq!(
            fs::read_to_string(root.join("Books/Other (2).epub")).unwrap(),
            "legacy other"
        );
        assert_eq!(
            fs::read_to_string(root.join("System/CLOCK (2).TXT")).unwrap(),
            "legacy clock"
        );
        assert!(legacy.join("BOOKS/Comic").is_file());

        // Once the folder is out of the way, the next boot finishes.
        fs::remove_dir_all(root.join("Books/Comic")).unwrap();
        let report = migrate_legacy_layout(&root).unwrap();
        assert_eq!(
            report,
            Migration {
                moved: 1,
                finished: true,
                ..Migration::default()
            }
        );
        assert!(root.join("Books/Comic").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
