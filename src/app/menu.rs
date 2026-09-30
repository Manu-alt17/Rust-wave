//! Data-driven category menu definitions for the main product shell.
//!
//! Category rows contain applications only. Hierarchical navigation uses the
//! dedicated GPIO0 Boot-button long press instead of synthetic Back rows.

use std::{fs, path::Path};

use anyhow::{Context, Result};

use crate::regional::Locale;

use super::router::ScreenRoute;

pub const MAIN_CATEGORY_COUNT: usize = 7;
pub const CATEGORY_COUNT: usize = 1;
pub const SETTINGS_ENTRY_COUNT: usize = 7;
/// Maximum number of tiles shown under the "Most used" / "Più usate"
/// heading of the Settings grid (see
/// `screens::category::render_tile_grid`). The section holds the entries
/// opened most recently, newest first — see [`CategoryUsage`].
pub const MOST_USED_MAX: usize = 3;
/// Persisted "Most used" history for the Settings grid.
pub const MENU_USAGE_CONFIG_PATH: &str = "/sdcard/RUSTMIX/MENU.TXT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuEntry {
    pub label_en: &'static str,
    pub label_it: &'static str,
    pub subtitle_en: &'static str,
    pub subtitle_it: &'static str,
    pub badge_en: &'static str,
    pub badge_it: &'static str,
    pub route: ScreenRoute,
}

impl MenuEntry {
    #[must_use]
    pub const fn label(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label_en,
            Locale::Italian => self.label_it,
        }
    }

    #[must_use]
    pub const fn subtitle(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.subtitle_en,
            Locale::Italian => self.subtitle_it,
        }
    }

    #[must_use]
    pub const fn badge(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.badge_en,
            Locale::Italian => self.badge_it,
        }
    }
}

const HOME_ENTRIES: [MenuEntry; MAIN_CATEGORY_COUNT] = [
    MenuEntry {
        label_en: "Library",
        label_it: "Libreria",
        subtitle_en: "TXT and EPUB book library",
        subtitle_it: "Libreria di libri TXT ed EPUB",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Library,
    },
    MenuEntry {
        label_en: "Audiobooks",
        label_it: "Audiolibri",
        subtitle_en: "MP3 audiobooks from the SD card",
        subtitle_it: "Audiolibri MP3 dalla scheda SD",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::AudiobookLibrary,
    },
    MenuEntry {
        label_en: "Statistics",
        label_it: "Stats",
        subtitle_en: "Reading time, speed and streak",
        subtitle_it: "Tempo di lettura, velocità e streak",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::ReadingStats,
    },
    MenuEntry {
        label_en: "Upload",
        label_it: "Carica",
        subtitle_en: "Wi-Fi transfer, or Wi-Fi setup if not connected yet",
        subtitle_it: "Trasferimento Wi-Fi, o configurazione Wi-Fi se non ancora connesso",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::WifiTransfer,
    },
    MenuEntry {
        label_en: "Files",
        label_it: "File",
        subtitle_en: "Read-only SD card browser",
        subtitle_it: "Esplora la scheda SD in sola lettura",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Files,
    },
    MenuEntry {
        label_en: "Settings",
        label_it: "Opzioni",
        subtitle_en: "Device services and display",
        subtitle_it: "Servizi del dispositivo e schermo",
        badge_en: "7",
        badge_it: "7",
        route: ScreenRoute::Settings,
    },
    // Kept last rather than first so the grid tiles above keep the leading
    // `home_selected` indices; the default `home_selected` (see
    // `AppState::default`) still lands on this entry first, since resuming
    // the current book is the most likely first action. The Home dashboard
    // draws this one as the full-width Continue
    // Reading card instead of a grid tile (see `screens::home::render_home`)
    // and `AppState::apply_home` resumes the saved book directly on SELECT
    // instead of routing through the old intermediate summary screen.
    MenuEntry {
        label_en: "Continue Reading",
        label_it: "Continua a leggere",
        subtitle_en: "Resume the last saved book",
        subtitle_it: "Riprendi l'ultimo libro salvato",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::ContinueReading,
    },
];

const SETTINGS_ENTRIES: [MenuEntry; SETTINGS_ENTRY_COUNT] = [
    MenuEntry {
        label_en: "Network",
        label_it: "Rete",
        subtitle_en: "SD config, Wi-Fi and SNTP status",
        subtitle_it: "Configurazione SD, stato Wi-Fi e SNTP",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Network,
    },
    MenuEntry {
        // Short, single-word tile caption in both locales rather than
        // "Software Update"/"Aggiornamento software" -- the full phrase runs
        // too long for the compact Settings tile's width.
        label_en: "Update",
        label_it: "Update",
        subtitle_en: "Check GitHub releases and install",
        subtitle_it: "Controlla le release GitHub e installa",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::OtaUpdate,
    },
    MenuEntry {
        label_en: "Audio",
        label_it: "Audio",
        subtitle_en: "ES8311 playback and volume",
        subtitle_it: "Riproduzione ES8311 e volume",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Audio,
    },
    MenuEntry {
        label_en: "Clock",
        label_it: "Orologio",
        subtitle_en: "RTC, power and localized time",
        subtitle_it: "RTC, alimentazione e ora locale",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Clock,
    },
    MenuEntry {
        label_en: "Display",
        label_it: "Schermo",
        subtitle_en: "Global UI font and size",
        subtitle_it: "Carattere e dimensione dell'interfaccia",
        badge_en: "NEW",
        badge_it: "NUOVO",
        route: ScreenRoute::Display,
    },
    MenuEntry {
        label_en: "Language",
        label_it: "Lingua",
        subtitle_en: "Switch between English and Italian",
        subtitle_it: "Passa tra inglese e italiano",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Language,
    },
    MenuEntry {
        // Short tile caption in both locales, matching the "Update" entry's
        // same reasoning above.
        label_en: "Info",
        label_it: "Info",
        subtitle_en: "Firmware and board ownership",
        subtitle_it: "Firmware e proprietà della scheda",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::DeviceInfo,
    },
];

#[must_use]
pub const fn home_entries() -> &'static [MenuEntry] {
    &HOME_ENTRIES
}

#[must_use]
pub const fn category_entries(route: ScreenRoute) -> &'static [MenuEntry] {
    match route {
        ScreenRoute::Settings => &SETTINGS_ENTRIES,
        _ => &[],
    }
}

#[must_use]
pub const fn category_index(route: ScreenRoute) -> Option<usize> {
    match route {
        ScreenRoute::Settings => Some(0),
        _ => None,
    }
}

/// Recently-opened history behind the "Most used" / "Più usate" section of
/// the Settings grid: up to [`MOST_USED_MAX`] routes, newest first. Every
/// other entry keeps its static order below the "Other" / "Altro" heading.
/// Persisted to [`MENU_USAGE_CONFIG_PATH`] by entry `label_en`, so
/// reordering or adding entries in the static tables above never remaps a
/// saved history onto the wrong tile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CategoryUsage {
    settings: Vec<ScreenRoute>,
}

impl Default for CategoryUsage {
    fn default() -> Self {
        Self {
            // Seeded with what Settings used to pin permanently (Wi-Fi status
            // and Software Update) until the user's own history replaces it.
            settings: vec![ScreenRoute::Network, ScreenRoute::OtaUpdate],
        }
    }
}

impl CategoryUsage {
    fn recent(&self, category: ScreenRoute) -> &[ScreenRoute] {
        match category {
            ScreenRoute::Settings => &self.settings,
            _ => &[],
        }
    }

    fn recent_mut(&mut self, category: ScreenRoute) -> Option<&mut Vec<ScreenRoute>> {
        match category {
            ScreenRoute::Settings => Some(&mut self.settings),
            _ => None,
        }
    }

    /// Moves `target` to the front of `category`'s history, dropping the
    /// oldest entry past [`MOST_USED_MAX`]. Returns whether anything changed
    /// (so the caller only rewrites the SD file when it has to).
    pub fn record(&mut self, category: ScreenRoute, target: ScreenRoute) -> bool {
        if !category_entries(category)
            .iter()
            .any(|entry| entry.route == target)
        {
            return false;
        }
        let Some(recent) = self.recent_mut(category) else {
            return false;
        };
        if recent.first() == Some(&target) {
            return false;
        }
        recent.retain(|route| *route != target);
        recent.insert(0, target);
        recent.truncate(MOST_USED_MAX);
        true
    }

    /// Size of `category`'s "Most used" section; `0` means the grid draws a
    /// single uncaptioned run of tiles.
    #[must_use]
    pub fn most_used_count(&self, category: ScreenRoute) -> usize {
        self.recent(category).len()
    }

    /// `category`'s entries in display order: the "Most used" history
    /// first, then every remaining entry in its static order. Selection
    /// indices on the Settings grid index into this list.
    #[must_use]
    pub fn ordered_entries(&self, category: ScreenRoute) -> Vec<MenuEntry> {
        let entries = category_entries(category);
        let recent = self.recent(category);
        let mut ordered: Vec<MenuEntry> = recent
            .iter()
            .filter_map(|route| entries.iter().copied().find(|entry| entry.route == *route))
            .collect();
        ordered.extend(
            entries
                .iter()
                .copied()
                .filter(|entry| !recent.contains(&entry.route)),
        );
        ordered
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .with_context(|| format!("read menu usage {}", path.display()))?;
        Ok(Self::parse(&text))
    }

    /// Lenient on purpose: this is a convenience history, so unknown keys or
    /// labels (say, an entry renamed by a firmware update, or the `tools=`
    /// line older firmware wrote for its Tools grid) are skipped rather
    /// than failing the whole file.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut usage = Self::default();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let category = match key.trim() {
                "settings" => ScreenRoute::Settings,
                _ => continue,
            };
            let entries = category_entries(category);
            let mut routes: Vec<ScreenRoute> = Vec::new();
            for label in value.split(',').map(str::trim) {
                if let Some(entry) = entries.iter().find(|entry| entry.label_en == label) {
                    if !routes.contains(&entry.route) && routes.len() < MOST_USED_MAX {
                        routes.push(entry.route);
                    }
                }
            }
            if let Some(recent) = usage.recent_mut(category) {
                *recent = routes;
            }
        }
        usage
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        fs::write(path, self.serialized())
            .with_context(|| format!("write menu usage {}", path.display()))
    }

    #[must_use]
    pub fn serialized(&self) -> String {
        let labels = |category: ScreenRoute| {
            self.ordered_entries(category)
                .iter()
                .take(self.most_used_count(category))
                .map(|entry| entry.label_en)
                .collect::<Vec<_>>()
                .join(",")
        };
        format!(
            "# RustMix Wave most-used menu entries, newest first\nsettings={}\n",
            labels(ScreenRoute::Settings)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        category_entries, home_entries, CategoryUsage, MAIN_CATEGORY_COUNT, MOST_USED_MAX,
        SETTINGS_ENTRY_COUNT,
    };
    use crate::{app::router::ScreenRoute, regional::Locale};

    #[test]
    fn exposes_requested_main_category_counts_without_synthetic_back_rows() {
        assert_eq!(home_entries().len(), MAIN_CATEGORY_COUNT);
        assert_eq!(
            category_entries(ScreenRoute::Settings).len(),
            SETTINGS_ENTRY_COUNT
        );
        assert!(category_entries(ScreenRoute::Settings)
            .iter()
            .all(|entry| entry.route != ScreenRoute::Home));
    }

    #[test]
    fn statistics_and_files_are_direct_home_destinations() {
        for route in [ScreenRoute::ReadingStats, ScreenRoute::Files] {
            assert!(home_entries().iter().any(|entry| entry.route == route));
        }
    }

    #[test]
    fn most_used_follows_recently_opened_entries() {
        let mut usage = CategoryUsage::default();
        // Settings starts seeded with Network and Update (see `Default`).
        assert_eq!(usage.most_used_count(ScreenRoute::Settings), 2);
        assert!(usage.record(ScreenRoute::Settings, ScreenRoute::Audio));
        assert!(usage.record(ScreenRoute::Settings, ScreenRoute::Display));
        assert!(usage.record(ScreenRoute::Settings, ScreenRoute::Clock));
        assert_eq!(
            usage.most_used_count(ScreenRoute::Settings),
            MOST_USED_MAX
        );
        // Re-opening the newest entry is a no-op; re-opening an older one
        // moves it back to the front.
        assert!(!usage.record(ScreenRoute::Settings, ScreenRoute::Clock));
        assert!(usage.record(ScreenRoute::Settings, ScreenRoute::Audio));
        // Routes outside the category are ignored.
        assert!(!usage.record(ScreenRoute::Settings, ScreenRoute::Files));

        let ordered = usage.ordered_entries(ScreenRoute::Settings);
        assert_eq!(ordered.len(), SETTINGS_ENTRY_COUNT);
        let head: Vec<ScreenRoute> = ordered.iter().take(3).map(|entry| entry.route).collect();
        assert_eq!(
            head,
            [ScreenRoute::Audio, ScreenRoute::Clock, ScreenRoute::Display]
        );
        assert!(ordered
            .iter()
            .any(|entry| entry.route == ScreenRoute::Network));
    }

    #[test]
    fn menu_usage_round_trips_and_skips_unknown_labels() {
        let mut usage = CategoryUsage::default();
        usage.record(ScreenRoute::Settings, ScreenRoute::Audio);
        assert_eq!(CategoryUsage::parse(&usage.serialized()), usage);

        // `tools=` is what firmware with a Tools grid wrote.
        let parsed =
            CategoryUsage::parse("settings=Gone,Audio,Audio\nbogus=1\ntools=File Browser\n");
        assert_eq!(
            parsed.ordered_entries(ScreenRoute::Settings)[0].route,
            ScreenRoute::Audio
        );
        assert_eq!(parsed.most_used_count(ScreenRoute::Settings), 1);
    }

    #[test]
    fn menu_entry_label_resolves_by_locale() {
        let library = home_entries()[0];
        assert_eq!(library.label(Locale::English), "Library");
        assert_eq!(library.label(Locale::Italian), "Libreria");
    }
}
