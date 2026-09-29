//! Hierarchical screen router for the portrait product UI shell.

use crate::regional::Locale;

/// Product screens exposed by the RustMix Wave shell.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScreenRoute {
    #[default]
    Home,
    Games,
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
    Calendar,
    CalendarAgenda,
    CalendarEventDetails,
    CalendarEventEditor,
    CalendarDeleteConfirmation,
    VoiceNotes,
    VoiceNoteDetails,
    VoiceNoteRecording,
    Magic,
    MagicView,
    Files,
    Dictionary,
    UnitConverter,
    Alarms,
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
    Environment,
    EnvironmentDetails,
    Motion,
    MotionEvents,
    MotionDetails,
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
            Self::Games => "Games",
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
            Self::Calendar => "Calendar",
            Self::CalendarAgenda => "Daily Agenda",
            Self::CalendarEventDetails => "Calendar Event",
            Self::CalendarEventEditor => "Edit Calendar Event",
            Self::CalendarDeleteConfirmation => "Delete Calendar Event",
            Self::VoiceNotes => "Voice Notes",
            Self::VoiceNoteDetails => "Voice Note",
            Self::VoiceNoteRecording => "Record Voice Note",
            Self::Magic => "Magic Tokens",
            Self::MagicView => "Token View",
            Self::Files => "File Browser",
            Self::Dictionary => "Dictionary",
            Self::UnitConverter => "Unit Converter",
            Self::Alarms => "Alarms",
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
            Self::Environment => "Environment",
            Self::EnvironmentDetails => "Sensor details",
            Self::Motion => "Motion",
            Self::MotionEvents => "Motion events",
            Self::MotionDetails => "Motion details",
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
            // `OtaUpdate`/`DeviceInfo`/`UnitConverter` diverge from
            // `label()`'s stable diagnostic strings: their on-screen names
            // are the shorter "Update"/"Info"/"Conv" (see the matching tiles
            // in `menu.rs`), in both locales.
            Locale::English => match self {
                Self::OtaUpdate => "Update",
                Self::DeviceInfo => "Info",
                Self::UnitConverter => "Conv",
                other => other.label(),
            },
            Locale::Italian => match self {
                Self::Home => "Home",
                Self::Games => "Giochi",
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
                Self::Calendar => "Calendario",
                Self::CalendarAgenda => "Agenda giornaliera",
                Self::CalendarEventDetails => "Evento del calendario",
                Self::CalendarEventEditor => "Modifica evento",
                Self::CalendarDeleteConfirmation => "Elimina evento",
                Self::VoiceNotes => "Note vocali",
                Self::VoiceNoteDetails => "Nota vocale",
                Self::VoiceNoteRecording => "Registra nota vocale",
                Self::Magic => "Token Magic",
                Self::MagicView => "Visualizza token",
                Self::Files => "Esplora file",
                Self::Dictionary => "Dizionario",
                Self::UnitConverter => "Conv",
                Self::Alarms => "Sveglie",
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
                Self::Environment => "Ambiente",
                Self::EnvironmentDetails => "Dettagli sensore",
                Self::Motion => "Movimento",
                Self::MotionEvents => "Eventi di movimento",
                Self::MotionDetails => "Dettagli movimento",
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
            Self::Games => "games",
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
            Self::Calendar => "calendar",
            Self::CalendarAgenda => "calendar-agenda",
            Self::CalendarEventDetails => "calendar-event-details",
            Self::CalendarEventEditor => "calendar-event-editor",
            Self::CalendarDeleteConfirmation => "calendar-delete-confirmation",
            Self::VoiceNotes => "voice-notes",
            Self::VoiceNoteDetails => "voice-note-details",
            Self::VoiceNoteRecording => "voice-note-recording",
            Self::Magic => "magic",
            Self::MagicView => "magic-view",
            Self::Files => "file-browser",
            Self::Dictionary => "dictionary",
            Self::UnitConverter => "unit-converter",
            Self::Alarms => "alarms",
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
            Self::Environment => "environment",
            Self::EnvironmentDetails => "environment-details",
            Self::Motion => "motion",
            Self::MotionEvents => "motion-events",
            Self::MotionDetails => "motion-details",
            Self::Network => "network",
            Self::NetworkDetails => "network-details",
            Self::NetworkSaved => "network-saved",
            Self::WifiTransfer => "wifi-transfer",
        }
    }

    #[must_use]
    pub const fn is_category(self) -> bool {
        matches!(self, Self::Games | Self::Tools | Self::Settings)
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
            Self::Games | Self::Tools | Self::Settings => Some(Self::Home),
            Self::ReadingStats => Some(Self::Home),
            Self::ContinueReading | Self::Library => Some(Self::Home),
            Self::LibraryBookActions => Some(Self::Library),
            Self::LibraryBookBookmarks => Some(Self::LibraryBookActions),
            Self::ReaderBookmarks => Some(Self::ReaderOptions),
            Self::ReaderLoading | Self::ReaderPage => Some(Self::Home),
            Self::ReaderOptions => Some(Self::ReaderPage),
            Self::ReaderPreferences => Some(Self::ReaderOptions),
            Self::ReaderToc => Some(Self::ReaderOptions),
            Self::Calendar | Self::VoiceNotes => Some(Self::Tools),
            Self::CalendarAgenda => Some(Self::Calendar),
            Self::CalendarEventDetails => Some(Self::CalendarAgenda),
            Self::CalendarEventEditor => Some(Self::CalendarAgenda),
            Self::CalendarDeleteConfirmation => Some(Self::CalendarEventDetails),
            Self::VoiceNoteDetails | Self::VoiceNoteRecording => Some(Self::VoiceNotes),
            Self::Magic => Some(Self::Games),
            Self::MagicView => Some(Self::Magic),
            Self::Files | Self::Dictionary | Self::UnitConverter => Some(Self::Tools),
            Self::PowerKeyMenu => Some(Self::Home),
            Self::Alarms
            | Self::Audio
            | Self::Clock
            | Self::Display
            | Self::Language
            | Self::DeviceInfo
            | Self::OtaUpdate
            | Self::Network => Some(Self::Settings),
            Self::Environment | Self::Motion => Some(Self::Tools),
            Self::AudioDetails => Some(Self::Audio),
            Self::ClockSetTime | Self::ClockDetails => Some(Self::Clock),
            Self::DeviceInfoBoard => Some(Self::DeviceInfo),
            Self::DeviceInfoRuntime => Some(Self::DeviceInfoBoard),
            Self::EnvironmentDetails => Some(Self::Environment),
            Self::MotionEvents => Some(Self::Motion),
            Self::MotionDetails => Some(Self::MotionEvents),
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
                | Self::Environment
                | Self::EnvironmentDetails
                | Self::Motion
                | Self::MotionDetails
                | Self::Network
                | Self::NetworkDetails
                | Self::NetworkSaved
                | Self::WifiTransfer
                | Self::Alarms
                | Self::Calendar
                | Self::CalendarAgenda
                | Self::ReaderLoading
                | Self::VoiceNoteRecording
        )
    }

    /// Routes that render the SHTC3 temperature/humidity reading. A strict
    /// subset of [`Self::uses_live_status`]: used to decide whether a fresh
    /// environment sample (a mandatory ~20ms blocking read) is worth taking
    /// after an interaction that navigates here, versus skipping it and
    /// relying on the periodic live-status refresh for routes that don't
    /// display it at all.
    #[must_use]
    pub const fn uses_environment_sample(self) -> bool {
        matches!(self, Self::Clock | Self::Environment)
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
        assert_eq!(ScreenRoute::Calendar.parent(), Some(ScreenRoute::Tools));
        assert_eq!(ScreenRoute::ReadingStats.parent(), Some(ScreenRoute::Home));
        assert_eq!(
            ScreenRoute::CalendarAgenda.parent(),
            Some(ScreenRoute::Calendar)
        );
        assert_eq!(
            ScreenRoute::CalendarEventDetails.parent(),
            Some(ScreenRoute::CalendarAgenda)
        );
        assert_eq!(
            ScreenRoute::CalendarEventEditor.parent(),
            Some(ScreenRoute::CalendarAgenda)
        );
        assert_eq!(
            ScreenRoute::CalendarDeleteConfirmation.parent(),
            Some(ScreenRoute::CalendarEventDetails)
        );
        assert_eq!(
            ScreenRoute::UnitConverter.parent(),
            Some(ScreenRoute::Tools)
        );
        assert_eq!(ScreenRoute::AudioDetails.parent(), Some(ScreenRoute::Audio));
        assert_eq!(ScreenRoute::ClockSetTime.parent(), Some(ScreenRoute::Clock));
        assert_eq!(
            ScreenRoute::DeviceInfoRuntime.parent(),
            Some(ScreenRoute::DeviceInfoBoard)
        );
        assert_eq!(ScreenRoute::OtaUpdate.parent(), Some(ScreenRoute::Settings));
        assert_eq!(ScreenRoute::Library.parent(), Some(ScreenRoute::Home));
        assert_eq!(ScreenRoute::Magic.parent(), Some(ScreenRoute::Games));
        assert_eq!(ScreenRoute::MagicView.parent(), Some(ScreenRoute::Magic));
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
