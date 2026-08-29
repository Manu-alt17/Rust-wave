//! Hierarchical screen router for the portrait product UI shell.

/// Product screens exposed by the RustMix Wave shell.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScreenRoute {
    #[default]
    Home,
    Reader,
    Productivity,
    Games,
    Tools,
    Settings,
    ContinueReading,
    Library,
    Bookmarks,
    ReaderBookmarks,
    ReaderLoading,
    ReaderPage,
    ReaderOptions,
    ReaderPreferences,
    ReaderToc,
    Calendar,
    CalendarAgenda,
    CalendarEventDetails,
    CalendarEventEditor,
    CalendarDeleteConfirmation,
    VoiceNotes,
    VoiceNoteDetails,
    VoiceNoteRecording,
    GamesTbd,
    LuaApps,
    LuaGame,
    LuaGameError,
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
    PowerKeyMenu,
    DeviceInfo,
    DeviceInfoBoard,
    DeviceInfoRuntime,
    Environment,
    EnvironmentDetails,
    Motion,
    MotionEvents,
    MotionDetails,
    Network,
    NetworkDetails,
    NetworkProvision,
    NetworkSaved,
    WifiTransfer,
    Weather,
    WeatherDetails,
}

impl ScreenRoute {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Reader => "Reader",
            Self::Productivity => "Productivity",
            Self::Games => "Games",
            Self::Tools => "Tools",
            Self::Settings => "Settings",
            Self::ContinueReading => "Continue Reading",
            Self::Library => "Library",
            Self::Bookmarks => "Bookmarks",
            Self::ReaderBookmarks => "Reader Bookmarks",
            Self::ReaderLoading => "Opening Book",
            Self::ReaderPage => "Reader Page",
            Self::ReaderOptions => "Reader Options",
            Self::ReaderPreferences => "Reading Preferences",
            Self::ReaderToc => "Table of Contents",
            Self::Calendar => "Calendar",
            Self::CalendarAgenda => "Daily Agenda",
            Self::CalendarEventDetails => "Calendar Event",
            Self::CalendarEventEditor => "Edit Calendar Event",
            Self::CalendarDeleteConfirmation => "Delete Calendar Event",
            Self::VoiceNotes => "Voice Notes",
            Self::VoiceNoteDetails => "Voice Note",
            Self::VoiceNoteRecording => "Record Voice Note",
            Self::GamesTbd => "TBD",
            Self::LuaApps => "SD Lua Apps",
            Self::LuaGame => "Lua App",
            Self::LuaGameError => "Lua App Error",
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
            Self::PowerKeyMenu => "Power Key Menu",
            Self::DeviceInfo => "Device Info",
            Self::DeviceInfoBoard => "Board services",
            Self::DeviceInfoRuntime => "Runtime services",
            Self::Environment => "Environment",
            Self::EnvironmentDetails => "Sensor details",
            Self::Motion => "Motion",
            Self::MotionEvents => "Motion events",
            Self::MotionDetails => "Motion details",
            Self::Network => "Network",
            Self::NetworkDetails => "Provisioning details",
            Self::NetworkProvision => "Configure via Phone",
            Self::NetworkSaved => "Saved Networks",
            Self::WifiTransfer => "Wi-Fi Transfer",
            Self::Weather => "Weather",
            Self::WeatherDetails => "Weather details",
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Reader => "reader",
            Self::Productivity => "productivity",
            Self::Games => "games",
            Self::Tools => "tools",
            Self::Settings => "settings",
            Self::ContinueReading => "continue-reading",
            Self::Library => "library",
            Self::Bookmarks => "bookmarks",
            Self::ReaderBookmarks => "reader-bookmarks",
            Self::ReaderLoading => "reader-loading",
            Self::ReaderPage => "reader-page",
            Self::ReaderOptions => "reader-options",
            Self::ReaderPreferences => "reader-preferences",
            Self::ReaderToc => "reader-toc",
            Self::Calendar => "calendar",
            Self::CalendarAgenda => "calendar-agenda",
            Self::CalendarEventDetails => "calendar-event-details",
            Self::CalendarEventEditor => "calendar-event-editor",
            Self::CalendarDeleteConfirmation => "calendar-delete-confirmation",
            Self::VoiceNotes => "voice-notes",
            Self::VoiceNoteDetails => "voice-note-details",
            Self::VoiceNoteRecording => "voice-note-recording",
            Self::GamesTbd => "games-tbd",
            Self::LuaApps => "lua-apps",
            Self::LuaGame => "lua-game",
            Self::LuaGameError => "lua-game-error",
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
            Self::PowerKeyMenu => "power-key-menu",
            Self::DeviceInfo => "device-info",
            Self::DeviceInfoBoard => "device-info-board",
            Self::DeviceInfoRuntime => "device-info-runtime",
            Self::Environment => "environment",
            Self::EnvironmentDetails => "environment-details",
            Self::Motion => "motion",
            Self::MotionEvents => "motion-events",
            Self::MotionDetails => "motion-details",
            Self::Network => "network",
            Self::NetworkDetails => "network-details",
            Self::NetworkProvision => "network-provision",
            Self::NetworkSaved => "network-saved",
            Self::WifiTransfer => "wifi-transfer",
            Self::Weather => "weather",
            Self::WeatherDetails => "weather-details",
        }
    }

    #[must_use]
    pub const fn is_category(self) -> bool {
        matches!(
            self,
            Self::Reader | Self::Productivity | Self::Games | Self::Tools | Self::Settings
        )
    }

    #[must_use]
    pub const fn is_placeholder(self) -> bool {
        matches!(self, Self::GamesTbd)
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
            Self::Reader | Self::Productivity | Self::Games | Self::Tools | Self::Settings => {
                Some(Self::Home)
            }
            Self::ContinueReading | Self::Library | Self::Bookmarks => Some(Self::Reader),
            Self::ReaderBookmarks => Some(Self::ReaderOptions),
            Self::ReaderLoading | Self::ReaderPage => Some(Self::Library),
            Self::ReaderOptions => Some(Self::ReaderPage),
            Self::ReaderPreferences => Some(Self::ReaderOptions),
            Self::ReaderToc => Some(Self::ReaderOptions),
            Self::Calendar | Self::VoiceNotes => Some(Self::Productivity),
            Self::CalendarAgenda => Some(Self::Calendar),
            Self::CalendarEventDetails => Some(Self::CalendarAgenda),
            Self::CalendarEventEditor => Some(Self::CalendarAgenda),
            Self::CalendarDeleteConfirmation => Some(Self::CalendarEventDetails),
            Self::VoiceNoteDetails | Self::VoiceNoteRecording => Some(Self::VoiceNotes),
            Self::GamesTbd | Self::LuaApps | Self::Magic => Some(Self::Games),
            Self::LuaGame | Self::LuaGameError => Some(Self::LuaApps),
            Self::MagicView => Some(Self::Magic),
            Self::Files | Self::Dictionary | Self::UnitConverter => Some(Self::Tools),
            Self::PowerKeyMenu => Some(Self::Home),
            Self::Alarms
            | Self::Audio
            | Self::Clock
            | Self::Display
            | Self::DeviceInfo
            | Self::Environment
            | Self::Motion
            | Self::Network
            | Self::Weather => Some(Self::Settings),
            Self::AudioDetails => Some(Self::Audio),
            Self::ClockSetTime | Self::ClockDetails => Some(Self::Clock),
            Self::DeviceInfoBoard => Some(Self::DeviceInfo),
            Self::DeviceInfoRuntime => Some(Self::DeviceInfoBoard),
            Self::EnvironmentDetails => Some(Self::Environment),
            Self::MotionEvents => Some(Self::Motion),
            Self::MotionDetails => Some(Self::MotionEvents),
            Self::NetworkDetails | Self::NetworkProvision | Self::NetworkSaved => {
                Some(Self::Network)
            }
            Self::WifiTransfer => Some(Self::Home),
            Self::WeatherDetails => Some(Self::Weather),
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
                | Self::NetworkProvision
                | Self::NetworkSaved
                | Self::WifiTransfer
                | Self::Alarms
                | Self::Calendar
                | Self::CalendarAgenda
                | Self::ReaderLoading
                | Self::VoiceNoteRecording
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

    #[test]
    fn router_exposes_static_parent_hierarchy() {
        assert_eq!(ScreenRoute::Files.parent(), Some(ScreenRoute::Tools));
        assert_eq!(ScreenRoute::Display.parent(), Some(ScreenRoute::Settings));
        assert_eq!(ScreenRoute::PowerKeyMenu.parent(), Some(ScreenRoute::Home));
        assert_eq!(
            ScreenRoute::Calendar.parent(),
            Some(ScreenRoute::Productivity)
        );
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
        assert!(!ScreenRoute::UnitConverter.is_placeholder());
        assert!(!ScreenRoute::Dictionary.is_placeholder());
        assert_eq!(ScreenRoute::AudioDetails.parent(), Some(ScreenRoute::Audio));
        assert_eq!(ScreenRoute::ClockSetTime.parent(), Some(ScreenRoute::Clock));
        assert_eq!(
            ScreenRoute::DeviceInfoRuntime.parent(),
            Some(ScreenRoute::DeviceInfoBoard)
        );
        assert_eq!(ScreenRoute::Reader.parent(), Some(ScreenRoute::Home));
        assert_eq!(ScreenRoute::LuaApps.parent(), Some(ScreenRoute::Games));
        assert_eq!(ScreenRoute::LuaGame.parent(), Some(ScreenRoute::LuaApps));
        assert_eq!(
            ScreenRoute::LuaGameError.parent(),
            Some(ScreenRoute::LuaApps)
        );
        assert_eq!(ScreenRoute::Magic.parent(), Some(ScreenRoute::Games));
        assert_eq!(ScreenRoute::MagicView.parent(), Some(ScreenRoute::Magic));
        assert_eq!(ScreenRoute::Home.parent(), None);
        assert_eq!(ScreenRoute::WifiTransfer.parent(), Some(ScreenRoute::Home));
        assert_eq!(
            ScreenRoute::NetworkProvision.parent(),
            Some(ScreenRoute::Network)
        );
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
        assert!(!ScreenRoute::Bookmarks.is_reader_active());
        assert!(!ScreenRoute::Home.is_reader_active());
    }
}
