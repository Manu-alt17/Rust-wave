//! Append-only diagnostic logs on the microSD, kept to a bounded size.
//!
//! `BOOTTIME.LOG` alone grows by several kilobytes at every boot and every
//! standby, and nothing ever trimmed it: a card that had been in use for a
//! few weeks carried hundreds of kilobytes of it. Each log is now renamed to
//! `.OLD` once it reaches [`SD_LOG_ROTATE_BYTES`], so the last two windows of
//! history stay on the card and nothing more.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

/// Size at which a log is moved to `.OLD`, replacing the previous one.
pub const SD_LOG_ROTATE_BYTES: u64 = 64 * 1024;

/// Appends `text` to the log at `path`, rotating it first when full.
pub fn append(path: &str, text: &str) -> io::Result<()> {
    append_with_limit(Path::new(path), text, SD_LOG_ROTATE_BYTES)
}

fn append_with_limit(path: &Path, text: &str, limit: u64) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() >= limit) {
        let old = path.with_extension("OLD");
        // FatFs refuses to rename over an existing file.
        let _ = fs::remove_file(&old);
        fs::rename(path, &old)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::append_with_limit;

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rustmix-sd-log-{name}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_full_log_moves_to_old_and_starts_over() {
        let dir = temp_dir("rotate");
        let log = dir.join("RUSTMIX").join("BOOTTIME.LOG");
        append_with_limit(&log, "first\n", 10).unwrap();
        append_with_limit(&log, "second\n", 10).unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "first\nsecond\n");
        // 13 bytes, over the limit: the next line starts a new file.
        append_with_limit(&log, "third\n", 10).unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "third\n");
        let old = dir.join("RUSTMIX").join("BOOTTIME.OLD");
        assert_eq!(fs::read_to_string(&old).unwrap(), "first\nsecond\n");
        // A second rotation replaces the previous .OLD.
        append_with_limit(&log, "fourth, long enough\n", 10).unwrap();
        append_with_limit(&log, "fifth\n", 10).unwrap();
        assert_eq!(fs::read_to_string(&old).unwrap(), "third\nfourth, long enough\n");
        assert_eq!(fs::read_to_string(&log).unwrap(), "fifth\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
