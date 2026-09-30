//! Hierarchical screen router for the portrait product UI shell.

use crate::regional::Locale;

/// Product screens exposed by the RustMix Wave shell.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScreenRoute {
    #[default]
    Home,
    Tools,
    Settings,
    ContinueReading,
    Library,
    LibraryBookActions,
    LibraryBookBookmarks,
    ReaderBookmarks,
    ReaderLoading,
    ReaderPage,
    ReaderOptions,
    ReaderPreferences,
    ReaderToc,
    ReadingStats,
    Files,
    Audio,
    AudioDetails,
    Clock,
    ClockSetTime,
    ClockDetails,
    Display,
    Language,
    PowerKeyMenu,
    DeviceInfo,
    DeviceInfoBoard,
    DeviceInfoRuntime,
    OtaUpdate,
    Network,
    NetworkDetails,
    NetworkSaved,
    WifiTransfer,
}

impl ScreenRoute {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Tools => "Tools",
            Self::Settings => "Settings",
            Self::ContinueReading => "Continue Reading",
            Self::Library => "Library",
            Self::LibraryBookActions => "Book Options",
            Self::LibraryBookBookmarks => "Book Bookmarks",
            Self::ReaderBookmarks => "Reader Bookmarks",
            Self::ReaderLoading => "Opening Book",
            Self::ReaderPage => "Reader Page",
            Self::ReaderOptions => "Reader Options",
            Self::ReaderPreferences => "Reading Preferences",
            Self::ReaderToc => "Table of Contents",
            Self::ReadingStats => "Reading Stats",
            Self::Files => "File Browser",
            Self::Audio => "Audio",
            Self::AudioDetails => "Audio details",
            Self::Clock => "Clock",
            Self::ClockSetTime => "Set Date & Time",
            Self::ClockDetails => "RTC details",
            Self::Display => "Display",
            Self::Language => "Language",
            Self::PowerKeyMenu => "Power Key Menu",
            Self::DeviceInfo => "Device Info",
            Self::DeviceInfoBoard => "Board services",
            Self::DeviceInfoRuntime => "Runtime services",
            Self::OtaUpdate => "Software Update",
            Self::Network => "Network",
            Self::NetworkDetails => "Provisioning details",
            Self::NetworkSaved => "Saved Networks",
            Self::WifiTransfer => "Wi-Fi Transfer",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for on-screen headers. `label`
    /// itself is left untouched because `src/main.rs`'s serial diagnostics
    /// logging depends on its English output staying stable.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            // `OtaUpdate`/`DeviceInfo` diverge from `label()`'s stable
            // diagnostic strings: their on-screen names are the shorter
            // "Update"/"Info" (see the matching tiles in `menu.rs`), in both
            // locales.
            Locale::English => match self {
                Self::OtaUpdate => "Update",
                Self::DeviceInfo => "Info",
                other => other.label(),
            },
            Locale::Italian => match self {
                Self::Home => "Home",
                Self::Tools => "Strumenti",
                Self::Settings => "Impostazioni",
                Self::ContinueReading => "Continua a leggere",
                Self::Library => "Libreria",
                Self::LibraryBookActions => "Opzioni libro",
                Self::LibraryBookBookmarks => "Segnalibri libro",
                Self::ReaderBookmarks => "Segnalibri lettore",
                Self::ReaderLoading => "Apertura libro",
                Self::ReaderPage => "Pagina lettore",
                Self::ReaderOptions => "Opzioni lettore",
                Self::ReaderPreferences => "Preferenze di lettura",
                Self::ReaderToc => "Indice",
                Self::ReadingStats => "Statistiche di lettura",
                Self::Files => "Esplora file",
                Self::Audio => "Audio",
                Self::AudioDetails => "Dettagli audio",
                Self::Clock => "Orologio",
                Self::ClockSetTime => "Imposta data e ora",
                Self::ClockDetails => "Dettagli RTC",
                Self::Display => "Schermo",
                Self::Language => "Lingua",
                Self::PowerKeyMenu => "Menu tasto accensione",
                Self::DeviceInfo => "Info",
                Self::DeviceInfoBoard => "Servizi scheda",
                Self::DeviceInfoRuntime => "Servizi runtime",
                Self::OtaUpdate => "Update",
                Self::Network => "Rete",
                Self::NetworkDetails => "Dettagli configurazione",
                Self::NetworkSaved => "Reti salvate",
                Self::WifiTransfer => "Trasferimento Wi-Fi",
            },
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Tools => "tools",
            Self::Settings => "settings",
            Self::ContinueReading => "continue-reading",
            Self::Library => "library",
            Self::LibraryBookActions => "library-book-actions",
            Self::LibraryBookBookmarks => "library-book-bookmarks",
            Self::ReaderBookmarks => "reader-bookmarks",
            Self::ReaderLoading => "reader-loading",
            Self::ReaderPage => "reader-page",
            Self::ReaderOptions => "reader-options",
            Self::ReaderPreferences => "reader-preferences",
            Self::ReaderToc => "reader-toc",
            Self::ReadingStats => "reading-stats",
            Self::Files => "file-browser",
            Self::Audio => "audio",
            Self::AudioDetails => "audio-details",
            Self::Clock => "clock",
            Self::ClockSetTime => "clock-set-time",
            Self::ClockDetails => "rtc-details",
            Self::Display => "display",
            Self::Language => "language",
            Self::PowerKeyMenu => "power-key-menu",
            Self::DeviceInfo => "device-info",
            Self::DeviceInfoBoard => "device-info-board",
            Self::DeviceInfoRuntime => "device-info-runtime",
            Self::OtaUpdate => "ota-update",
            Self::Network => "network",
            Self::NetworkDetails => "network-details",
            Self::NetworkSaved => "network-saved",
            Self::WifiTransfer => "wifi-transfer",
        }
    }

    #[must_use]
    pub const fn is_category(self) -> bool {
        matches!(self, Self::Tools | Self::Settings)
    }

    /// Whether this route only exists while a book session is open. Used to
    /// decide, at the moment hardware deep sleep is entered, whether the
    /// device should auto-resume the last book on the next boot instead of
    /// landing on Home (a real deep-sleep wake is a full reboot, so nothing
    /// in RAM, including the router's current route, survives it).
    #[must_use]
    pub const fn is_reader_active(self) -> bool {
        matches!(
            self,
            Self::ReaderPage
                | Self::ReaderOptions
                | Self::ReaderToc
                | Self::ReaderBookmarks
                | Self::ReaderPreferences
        )
    }

    #[must_use]
    pub const fn parent(self) -> Option<Self> {
        match self {
            Self::Home => None,
            Self::Tools | Self::Settings => Some(Self::Home),
            Self::ReadingStats => Some(Self::Home),
            Self::ContinueReading | Self::Library => Some(Self::Home),
            Self::LibraryBookActions => Some(Self::Library),
            Self::LibraryBookBookmarks => Some(Self::LibraryBookActions),
            Self::ReaderBookmarks => Some(Self::ReaderOptions),
            Self::ReaderLoading | Self::ReaderPage => Some(Self::Home),
            Self::ReaderOptions => Some(Self::ReaderPage),
            Self::ReaderPreferences => Some(Self::ReaderOptions),
            Self::ReaderToc => Some(Self::ReaderOptions),
            Self::Files => Some(Self::Tools),
            Self::PowerKeyMenu => Some(Self::Home),
            Self::Audio
            | Self::Clock
            | Self::Display
            | Self::Language
            | Self::DeviceInfo
            | Self::OtaUpdate
            | Self::Network => Some(Self::Settings),
            Self::AudioDetails => Some(Self::Audio),
            Self::ClockSetTime | Self::ClockDetails => Some(Self::Clock),
            Self::DeviceInfoBoard => Some(Self::DeviceInfo),
            Self::DeviceInfoRuntime => Some(Self::DeviceInfoBoard),
            Self::NetworkDetails | Self::NetworkSaved => Some(Self::Network),
            Self::WifiTransfer => Some(Self::Home),
        }
    }

    #[must_use]
    pub const fn uses_live_status(self) -> bool {
        matches!(
            self,
            Self::Clock
                | Self::ClockDetails
                | Self::Network
                | Self::NetworkDetails
                | Self::NetworkSaved
                | Self::WifiTransfer
                | Self::ReaderLoading
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScreenRouter {
    current: ScreenRoute,
}

impl ScreenRouter {
    #[must_use]
    pub const fn current(self) -> ScreenRoute {
        self.current
    }

    pub fn navigate_to(&mut self, route: ScreenRoute) {
        self.current = route;
    }

    pub fn back(&mut self) {
        self.current = self.current.parent().unwrap_or(ScreenRoute::Home);
    }

    pub fn back_home(&mut self) {
        self.current = ScreenRoute::Home;
    }
}

#[cfg(test)]
mod tests {
    use super::{ScreenRoute, ScreenRouter};
    use crate::regional::Locale;

    #[test]
    fn router_exposes_static_parent_hierarchy() {
        assert_eq!(ScreenRoute::Files.parent(), Some(ScreenRoute::Tools));
        assert_eq!(ScreenRoute::Display.parent(), Some(ScreenRoute::Settings));
        assert_eq!(ScreenRoute::Language.parent(), Some(ScreenRoute::Settings));
        assert_eq!(ScreenRoute::PowerKeyMenu.parent(), Some(ScreenRoute::Home));
        assert_eq!(ScreenRoute::ReadingStats.parent(), Some(ScreenRoute::Home));
        assert_eq!(ScreenRoute::AudioDetails.parent(), Some(ScreenRoute::Audio));
        assert_eq!(ScreenRoute::ClockSetTime.parent(), Some(ScreenRoute::Clock));
        assert_eq!(
            ScreenRoute::DeviceInfoRuntime.parent(),
            Some(ScreenRoute::DeviceInfoBoard)
        );
        assert_eq!(ScreenRoute::OtaUpdate.parent(), Some(ScreenRoute::Settings));
        assert_eq!(ScreenRoute::Library.parent(), Some(ScreenRoute::Home));
        assert_eq!(ScreenRoute::Home.parent(), None);
        assert_eq!(ScreenRoute::WifiTransfer.parent(), Some(ScreenRoute::Home));
        assert_eq!(
            ScreenRoute::NetworkSaved.parent(),
            Some(ScreenRoute::Network)
        );
        assert_eq!(
            ScreenRoute::NetworkDetails.parent(),
            Some(ScreenRoute::Network)
        );
    }

    #[test]
    fn back_returns_details_to_overview_then_category_then_home() {
        let mut router = ScreenRouter::default();
        router.navigate_to(ScreenRoute::Settings);
        router.navigate_to(ScreenRoute::Audio);
        router.navigate_to(ScreenRoute::AudioDetails);
        router.back();
        assert_eq!(router.current(), ScreenRoute::Audio);
        router.back();
        assert_eq!(router.current(), ScreenRoute::Settings);
        router.back();
        assert_eq!(router.current(), ScreenRoute::Home);
    }

    #[test]
    fn only_in_session_reader_routes_are_reader_active() {
        assert!(ScreenRoute::ReaderPage.is_reader_active());
        assert!(ScreenRoute::ReaderOptions.is_reader_active());
        assert!(ScreenRoute::ReaderToc.is_reader_active());
        assert!(ScreenRoute::ReaderBookmarks.is_reader_active());
        assert!(ScreenRoute::ReaderPreferences.is_reader_active());
        assert!(!ScreenRoute::ReaderLoading.is_reader_active());
        assert!(!ScreenRoute::ContinueReading.is_reader_active());
        assert!(!ScreenRoute::Library.is_reader_active());
        assert!(!ScreenRoute::LibraryBookActions.is_reader_active());
        assert!(!ScreenRoute::LibraryBookBookmarks.is_reader_active());
        assert!(!ScreenRoute::Home.is_reader_active());
    }

    #[test]
    fn label_i18n_translates_into_italian_and_keeps_english_label_stable() {
        assert_eq!(ScreenRoute::Clock.label_i18n(Locale::English), "Clock");
        assert_eq!(ScreenRoute::Clock.label_i18n(Locale::Italian), "Orologio");
        assert_eq!(ScreenRoute::Clock.label(), "Clock");
    }
}
