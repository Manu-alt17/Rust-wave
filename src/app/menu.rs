//! Data-driven category menu definitions for the main product shell.
//!
//! Category rows contain applications only. Hierarchical navigation uses the
//! dedicated GPIO0 Boot-button long press instead of synthetic Back rows.

use crate::regional::Locale;

use super::router::ScreenRoute;

pub const MAIN_CATEGORY_COUNT: usize = 7;
pub const CATEGORY_COUNT: usize = 3;
pub const CATEGORY_PAGE_SIZE: usize = 6;
pub const SETTINGS_ENTRY_COUNT: usize = 10;
/// Leading entries of [`SETTINGS_ENTRIES`] shown under the "Most used" /
/// "Più usate" heading on the Settings grid (see
/// `screens::category::render_tile_grid`), ahead of the "Other" /
/// "Altro" section holding the rest. Picked from what the user told us they
/// reach for most: Network (Wi-Fi status) and Software Update.
pub const SETTINGS_PRIMARY_COUNT: usize = 2;

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
        label_en: "Statistics",
        label_it: "Stats",
        subtitle_en: "Reading time, speed and streak",
        subtitle_it: "Tempo di lettura, velocità e streak",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::ReadingStats,
    },
    MenuEntry {
        label_en: "Games",
        label_it: "Giochi",
        subtitle_en: "E-paper friendly games",
        subtitle_it: "Giochi adatti allo schermo e-paper",
        badge_en: "SD",
        badge_it: "SD",
        route: ScreenRoute::Games,
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
        label_en: "Tools",
        label_it: "Strumenti",
        subtitle_en: "Files, dictionary and conversion",
        subtitle_it: "File, dizionario e conversioni",
        badge_en: "3",
        badge_it: "3",
        route: ScreenRoute::Tools,
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
    // Kept last rather than first so every existing `home_selected` index
    // above (0..=5, the grid tiles) keeps its meaning unchanged; the default
    // `home_selected` (see `AppState::default`) still lands on this entry
    // first, since resuming the current book is the most likely first
    // action. The Home dashboard draws this one as the full-width Continue
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

const GAMES_ENTRIES: [MenuEntry; 2] = [
    MenuEntry {
        label_en: "SD Lua Apps",
        label_it: "App Lua da SD",
        subtitle_en: "SD-loaded apps with native canvas",
        subtitle_it: "App caricate da SD con canvas nativo",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::LuaApps,
    },
    MenuEntry {
        label_en: "Magic Tokens",
        label_it: "Token Magic",
        subtitle_en: "MTG token art curated from your phone",
        subtitle_it: "Illustrazioni token MTG curate dal telefono",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Magic,
    },
];

const TOOLS_ENTRIES: [MenuEntry; 5] = [
    MenuEntry {
        label_en: "File Browser",
        label_it: "Esplora file",
        subtitle_en: "Read-only SDMMC browser",
        subtitle_it: "Esplora SDMMC in sola lettura",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Files,
    },
    MenuEntry {
        label_en: "Dictionary",
        label_it: "Dizionario",
        subtitle_en: "Offline prefix lookup",
        subtitle_it: "Ricerca per prefisso offline",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Dictionary,
    },
    MenuEntry {
        // Short tile caption in both locales, matching "Update"/"Info"
        // above.
        label_en: "Conv",
        label_it: "Conv",
        subtitle_en: "Offline fixed-point conversions",
        subtitle_it: "Conversioni offline a virgola fissa",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::UnitConverter,
    },
    MenuEntry {
        label_en: "Calendar",
        label_it: "Calendario",
        subtitle_en: "US agenda and personal editor",
        subtitle_it: "Agenda e editor personale",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Calendar,
    },
    MenuEntry {
        label_en: "Voice Notes",
        label_it: "Note vocali",
        subtitle_en: "Record PCM WAV notes to SD",
        subtitle_it: "Registra note PCM WAV su SD",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::VoiceNotes,
    },
];

const SETTINGS_ENTRIES: [MenuEntry; SETTINGS_ENTRY_COUNT] = [
    // First `SETTINGS_PRIMARY_COUNT` entries are the "Most used" / "Più
    // usate" section — keep that many in sync if this leading group changes.
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
        subtitle_en: "ES8311 playback and alarm chime",
        subtitle_it: "Riproduzione ES8311 e suoneria sveglia",
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
    MenuEntry {
        label_en: "Environment",
        label_it: "Ambiente",
        subtitle_en: "SHTC3 temperature and humidity",
        subtitle_it: "Temperatura e umidità SHTC3",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Environment,
    },
    MenuEntry {
        label_en: "Motion",
        label_it: "Movimento",
        subtitle_en: "QMI8658 accelerometer and gyroscope",
        subtitle_it: "Accelerometro e giroscopio QMI8658",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Motion,
    },
    MenuEntry {
        label_en: "Weather",
        label_it: "Meteo",
        subtitle_en: "Open-Meteo conditions and forecast",
        subtitle_it: "Condizioni e previsioni Open-Meteo",
        badge_en: "",
        badge_it: "",
        route: ScreenRoute::Weather,
    },
];

#[must_use]
pub const fn home_entries() -> &'static [MenuEntry] {
    &HOME_ENTRIES
}

#[must_use]
pub const fn category_entries(route: ScreenRoute) -> &'static [MenuEntry] {
    match route {
        ScreenRoute::Games => &GAMES_ENTRIES,
        ScreenRoute::Tools => &TOOLS_ENTRIES,
        ScreenRoute::Settings => &SETTINGS_ENTRIES,
        _ => &[],
    }
}

#[must_use]
pub const fn category_index(route: ScreenRoute) -> Option<usize> {
    match route {
        ScreenRoute::Games => Some(0),
        ScreenRoute::Tools => Some(1),
        ScreenRoute::Settings => Some(2),
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
        assert_eq!(category_entries(ScreenRoute::Games).len(), 2);
        assert_eq!(category_entries(ScreenRoute::Tools).len(), 5);
        assert_eq!(
            category_entries(ScreenRoute::Settings).len(),
            SETTINGS_ENTRY_COUNT
        );
        for route in [
            ScreenRoute::Games,
            ScreenRoute::Tools,
            ScreenRoute::Settings,
        ] {
            assert!(category_entries(route)
                .iter()
                .all(|entry| entry.route != ScreenRoute::Home));
        }
    }

    #[test]
    fn statistics_replaces_productivity_as_a_direct_home_destination() {
        assert!(home_entries()
            .iter()
            .any(|entry| entry.route == ScreenRoute::ReadingStats));
        assert!(category_entries(ScreenRoute::Tools)
            .iter()
            .any(|entry| entry.route == ScreenRoute::Calendar));
        assert!(category_entries(ScreenRoute::Tools)
            .iter()
            .any(|entry| entry.route == ScreenRoute::VoiceNotes));
    }

    #[test]
    fn settings_contains_display_and_language_before_existing_diagnostics() {
        let settings = category_entries(ScreenRoute::Settings);
        assert!(settings
            .iter()
            .any(|entry| entry.route == ScreenRoute::Display));
        assert!(settings
            .iter()
            .any(|entry| entry.route == ScreenRoute::Language));
        assert!(settings
            .iter()
            .any(|entry| entry.route == ScreenRoute::Weather));
    }

    #[test]
    fn menu_entry_label_resolves_by_locale() {
        let library = home_entries()[0];
        assert_eq!(library.label(Locale::English), "Library");
        assert_eq!(library.label(Locale::Italian), "Libreria");
    }
}
