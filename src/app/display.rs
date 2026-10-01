//! Persistent global UI display preferences.
//!
//! Preferences are loaded from `/sdcard/RUSTMIX/DISPLAY.TXT` at boot. The UI
//! remains usable when the SD card or file is unavailable: Standard is
//! always the safe default. Changes are persisted best-effort by the runtime.

use std::{fs, path::Path};

use anyhow::{bail, Context, Result};

use crate::regional::Locale;

/// SD-backed global UI typography preference file.
pub const DISPLAY_CONFIG_PATH: &str = "/sdcard/RUSTMIX/DISPLAY.TXT";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UiFontSize {
    Compact,
    #[default]
    Standard,
    Large,
}

impl UiFontSize {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Standard => "Standard",
            Self::Large => "Large",
        }
    }

    /// Locale-aware sibling of [`Self::label`]. `label` itself is left
    /// untouched so any English-only diagnostics that depend on it stay
    /// stable.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::Compact => "Compatta",
                Self::Standard => "Standard",
                Self::Large => "Grande",
            },
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Standard => "standard",
            Self::Large => "large",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Compact => Self::Standard,
            Self::Standard => Self::Large,
            Self::Large => Self::Compact,
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "compact" => Ok(Self::Compact),
            "standard" => Ok(Self::Standard),
            "large" => Ok(Self::Large),
            other => bail!("unsupported font_size value {other:?}"),
        }
    }
}

/// What the panel shows while the device is in deep sleep.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SleepScreenMode {
    /// `/sdcard/RUSTMIX/SLEEP` images in file-name order, one step per sleep,
    /// wrapping back to the first.
    #[default]
    Sequential,
    /// A random `/sdcard/RUSTMIX/SLEEP` image, never the same one twice in a
    /// row.
    Random,
    /// Full-screen cover of the book being read, with a progress tab. Falls
    /// back to [`Self::Sequential`] when there is no book or no usable cover.
    BookCover,
}

impl SleepScreenMode {
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => match self {
                Self::Sequential => "In order",
                Self::Random => "Random",
                Self::BookCover => "Book cover",
            },
            Locale::Italian => match self {
                Self::Sequential => "In sequenza",
                Self::Random => "Casuale",
                Self::BookCover => "Copertina",
            },
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::Random => "random",
            Self::BookCover => "book-cover",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Sequential => Self::Random,
            Self::Random => Self::BookCover,
            Self::BookCover => Self::Sequential,
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sequential" => Ok(Self::Sequential),
            "random" => Ok(Self::Random),
            "book-cover" | "book_cover" | "cover" => Ok(Self::BookCover),
            other => bail!("unsupported sleep_screen value {other:?}"),
        }
    }
}

/// How long without a key press before the device goes to standby on its
/// own. The Power key sleeps it at any time regardless.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AutoSleep {
    Minutes5,
    #[default]
    Minutes10,
    Minutes15,
    Minutes30,
    Minutes60,
    Never,
}

impl AutoSleep {
    /// Idle seconds before standby, or `None` for [`Self::Never`].
    #[must_use]
    pub const fn idle_seconds(self) -> Option<u64> {
        match self {
            Self::Minutes5 => Some(5 * 60),
            Self::Minutes10 => Some(10 * 60),
            Self::Minutes15 => Some(15 * 60),
            Self::Minutes30 => Some(30 * 60),
            Self::Minutes60 => Some(60 * 60),
            Self::Never => None,
        }
    }

    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match self {
            Self::Minutes5 => "5 min",
            Self::Minutes10 => "10 min",
            Self::Minutes15 => "15 min",
            Self::Minutes30 => "30 min",
            Self::Minutes60 => "60 min",
            Self::Never => match locale {
                Locale::English => "Never",
                Locale::Italian => "Mai",
            },
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Minutes5 => "5",
            Self::Minutes10 => "10",
            Self::Minutes15 => "15",
            Self::Minutes30 => "30",
            Self::Minutes60 => "60",
            Self::Never => "never",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Minutes5 => Self::Minutes10,
            Self::Minutes10 => Self::Minutes15,
            Self::Minutes15 => Self::Minutes30,
            Self::Minutes30 => Self::Minutes60,
            Self::Minutes60 => Self::Never,
            Self::Never => Self::Minutes5,
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "5" => Ok(Self::Minutes5),
            "10" => Ok(Self::Minutes10),
            "15" => Ok(Self::Minutes15),
            "30" => Ok(Self::Minutes30),
            "60" => Ok(Self::Minutes60),
            "never" | "0" => Ok(Self::Never),
            other => bail!("unsupported auto_sleep value {other:?}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DisplayPreferences {
    pub font_size: UiFontSize,
    pub sleep_screen: SleepScreenMode,
    pub auto_sleep: AutoSleep,
}

impl DisplayPreferences {
    pub fn cycle_font_size(&mut self) {
        self.font_size = self.font_size.next();
    }

    pub fn cycle_sleep_screen(&mut self) {
        self.sleep_screen = self.sleep_screen.next();
    }

    pub fn cycle_auto_sleep(&mut self) {
        self.auto_sleep = self.auto_sleep.next();
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .with_context(|| format!("read display config {}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut preferences = Self::default();
        let mut saw_size = false;
        let mut saw_sleep_screen = false;
        let mut saw_auto_sleep = false;
        for (line_number, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("line {} must contain '='", line_number + 1))?;
            match key.trim() {
                // Written by firmware that offered a second UI font; the UI
                // is Inter only now, so the saved choice is ignored.
                "font_family" => {}
                "font_size" => {
                    if saw_size {
                        bail!("duplicate font_size entry");
                    }
                    preferences.font_size = UiFontSize::parse(value)?;
                    saw_size = true;
                }
                "sleep_screen" => {
                    if saw_sleep_screen {
                        bail!("duplicate sleep_screen entry");
                    }
                    preferences.sleep_screen = SleepScreenMode::parse(value)?;
                    saw_sleep_screen = true;
                }
                "auto_sleep" => {
                    if saw_auto_sleep {
                        bail!("duplicate auto_sleep entry");
                    }
                    preferences.auto_sleep = AutoSleep::parse(value)?;
                    saw_auto_sleep = true;
                }
                other => bail!("unsupported display config key {other:?}"),
            }
        }
        Ok(preferences)
    }

    pub fn save_to_path(self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        fs::write(path, self.serialized())
            .with_context(|| format!("write display config {}", path.display()))
    }

    #[must_use]
    pub fn serialized(self) -> String {
        format!(
            "# RustMix Wave display and standby\nfont_size={}\nsleep_screen={}\nauto_sleep={}\n",
            self.font_size.marker(),
            self.sleep_screen.marker(),
            self.auto_sleep.marker()
        )
    }

    #[must_use]
    pub const fn persistence_label(self) -> &'static str {
        "SD FILE"
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{AutoSleep, DisplayPreferences, SleepScreenMode, UiFontSize};

    #[test]
    fn defaults_to_standard() {
        let preferences = DisplayPreferences::default();
        assert_eq!(preferences.font_size, UiFontSize::Standard);
        assert_eq!(preferences.persistence_label(), "SD FILE");
    }

    #[test]
    fn parses_and_serializes_supported_preferences() {
        // `font_family` comes from firmware with a second UI font.
        let parsed =
            DisplayPreferences::parse("font_family=atkinson-hyperlegible\nfont_size=large\n")
                .unwrap();
        assert_eq!(parsed.font_size, UiFontSize::Large);
        assert!(!parsed.serialized().contains("font_family"));
        assert!(parsed.serialized().contains("font_size=large"));
    }

    #[test]
    fn saves_and_loads_display_file() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rustmix-display-{nanos}.txt"));
        let preferences = DisplayPreferences {
            font_size: UiFontSize::Compact,
            sleep_screen: SleepScreenMode::BookCover,
            auto_sleep: AutoSleep::Never,
        };
        preferences.save_to_path(&path).unwrap();
        assert_eq!(
            DisplayPreferences::load_from_path(&path).unwrap(),
            preferences
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_unknown_keys_and_values() {
        assert!(DisplayPreferences::parse("font_size=huge\n").is_err());
        assert!(DisplayPreferences::parse("other=value\n").is_err());
        assert!(DisplayPreferences::parse("sleep_screen=slideshow\n").is_err());
    }

    #[test]
    fn sleep_screen_defaults_to_sequential_and_round_trips() {
        assert_eq!(
            DisplayPreferences::parse("font_size=large\n")
                .unwrap()
                .sleep_screen,
            SleepScreenMode::Sequential
        );
        for mode in [
            SleepScreenMode::Sequential,
            SleepScreenMode::Random,
            SleepScreenMode::BookCover,
        ] {
            let preferences = DisplayPreferences {
                sleep_screen: mode,
                ..DisplayPreferences::default()
            };
            assert_eq!(
                DisplayPreferences::parse(&preferences.serialized()).unwrap(),
                preferences
            );
        }
        assert_eq!(
            SleepScreenMode::BookCover.next(),
            SleepScreenMode::Sequential
        );
    }

    #[test]
    fn auto_sleep_defaults_to_ten_minutes_and_round_trips() {
        // Files written before the setting existed keep the old timeout.
        let parsed = DisplayPreferences::parse("font_size=large\n").unwrap();
        assert_eq!(parsed.auto_sleep, AutoSleep::Minutes10);
        assert_eq!(parsed.auto_sleep.idle_seconds(), Some(600));
        let mut preferences = DisplayPreferences::default();
        for _ in 0..6 {
            preferences.cycle_auto_sleep();
            assert_eq!(
                DisplayPreferences::parse(&preferences.serialized()).unwrap(),
                preferences
            );
        }
        assert_eq!(preferences.auto_sleep, AutoSleep::Minutes10);
        assert_eq!(AutoSleep::Never.idle_seconds(), None);
        assert!(DisplayPreferences::parse("auto_sleep=7\n").is_err());
    }

    #[test]
    fn the_sd_card_example_parses() {
        let example = include_str!("../../examples/sd-card/RUSTMIX/DISPLAY.TXT.example");
        let parsed = DisplayPreferences::parse(example).unwrap();
        assert_eq!(parsed, DisplayPreferences::default());
    }
}
