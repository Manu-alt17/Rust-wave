//! Data-driven category menu definitions for the main product shell.
//!
//! Category rows contain applications only. Hierarchical navigation uses the
//! dedicated GPIO0 Boot-button long press instead of synthetic Back rows.
//! Entries are always shown in the order of these tables, so each one stays
//! where the user last found it.

use crate::regional::Locale;

use super::router::ScreenRoute;

pub const MAIN_CATEGORY_COUNT: usize = 6;
pub const CATEGORY_COUNT: usize = 1;
pub const SETTINGS_ENTRY_COUNT: usize = 9;

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
        subtitle_it: "Tempo di lettura, velocità e serie",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::ReadingStats,
    },
    MenuEntry {
        label_en: "Upload",
        label_it: "Carica",
        subtitle_en: "Copy files over Wi-Fi or with the USB cable",
        subtitle_it: "Copia i file via Wi-Fi o con il cavo USB",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Upload,
    },
    MenuEntry {
        label_en: "Settings",
        label_it: "Opzioni",
        subtitle_en: "Device services and display",
        subtitle_it: "Servizi del dispositivo e schermo",
        badge_en: "9",
        badge_it: "9",
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
        // too long for the compact Settings tile's width. The screen's own
        // header says the same word.
        label_en: "Update",
        label_it: "Aggiorna",
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
        subtitle_en: "UI size, sleep screen and standby",
        subtitle_it: "Dimensione interfaccia, sfondo e standby",
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
        label_en: "Files",
        label_it: "File",
        subtitle_en: "Browse the memory card",
        subtitle_it: "Esplora la scheda di memoria",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Files,
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
    MenuEntry {
        // The first-run pages, to go through again: shown by themselves
        // only once, on a new card.
        label_en: "First steps",
        label_it: "Primi passi",
        subtitle_en: "Keys, Wi-Fi, clock and first book",
        subtitle_it: "Tasti, Wi-Fi, orologio e primo libro",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Setup,
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

#[cfg(test)]
mod tests {
    use super::{category_entries, home_entries, MAIN_CATEGORY_COUNT, SETTINGS_ENTRY_COUNT};
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
    fn statistics_is_on_home_and_files_in_settings() {
        assert!(home_entries()
            .iter()
            .any(|entry| entry.route == ScreenRoute::ReadingStats));
        assert!(home_entries()
            .iter()
            .all(|entry| entry.route != ScreenRoute::Files));
        assert!(category_entries(ScreenRoute::Settings)
            .iter()
            .any(|entry| entry.route == ScreenRoute::Files));
    }

    #[test]
    fn menu_entry_label_resolves_by_locale() {
        let library = home_entries()[0];
        assert_eq!(library.label(Locale::English), "Library");
        assert_eq!(library.label(Locale::Italian), "Libreria");
    }
}
