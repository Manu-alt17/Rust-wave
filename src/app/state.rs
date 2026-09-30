//! Product UI state transitions independent of hardware wiring.

use crate::{
    alarm::AlarmSnapshot,
    audio::{AudioSnapshot, AudioUiRequest},
    board_services::BoardSnapshot,
    buttons::ButtonEvent,
    clock_time_editor::{self, ClockEditField, ClockTimeEditor},
    dictionary::DictionaryUiState,
    imu::ImuReading,
    imu_events::{ImuControlOutcome, ImuDetectedEvent, ImuEventBridge},
    network::NetworkSnapshot,
    network_saved::NetworkSavedUiState,
    orientation::DisplayOrientation,
    ota::{OtaCheckState, OtaUiRequest},
    power_key_menu::{PowerKeyMenuOutcome, PowerKeyMenuUiState},
    reader::{
        LibraryBookAction, ReaderDictionaryMode, ReaderLocation, ReaderOption, ReaderOrientation,
        ReaderSession, ReaderTickOutcome, ReaderUiState,
    },
    reading_stats::ReadingStatsSnapshot,
    regional::RegionalPreferences,
    storage::StorageSnapshot,
    unit_converter::UnitConverterUiState,
    voice_notes::{VoiceNotesUiRequest, VoiceNotesUiState},
    wifi_transfer::{WifiTransferSnapshot, WifiTransferState, WifiTransferUiRequest},
};

use super::{
    display::DisplayPreferences,
    menu::{category_index, home_entries, CategoryUsage, CATEGORY_COUNT, MAIN_CATEGORY_COUNT},
    router::{ScreenRoute, ScreenRouter},
};

/// Number of selectable rows in the playback overview screen.
pub const AUDIO_ACTION_COUNT: usize = 6;
/// Number of selectable rows in the Display settings screen.
pub const DISPLAY_ACTION_COUNT: usize = 3;
/// Set date & time or open RTC details rows on the Clock overview screen.
pub const CLOCK_ACTION_COUNT: usize = 2;
/// Configure via phone, saved networks and provisioning-details rows on the
/// Network screen. The Wi-Fi transfer portal is reached only from the Home
/// "Upload" tile.
pub const NETWORK_ACTION_COUNT: usize = 3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppState {
    pub home_selected: usize,
    category_selected: [usize; CATEGORY_COUNT],
    /// Recently-opened Tools/Settings entries shown under "Most used";
    /// persisted by the runtime owner in main.rs whenever it changes.
    pub category_usage: CategoryUsage,
    pub display_action_selected: usize,
    pub display: DisplayPreferences,
    /// Offline X4-pack-compatible native Dictionary keyboard and lookup snapshot.
    pub dictionary: DictionaryUiState,
    /// Offline fixed-point Unit Converter cursor and editable field.
    pub unit_converter: UnitConverterUiState,
    /// TXT / reflowable EPUB Reader library, staged opening, RAM cache and options.
    pub reader: ReaderUiState,
    /// Rust-owned debounced QMI8658 event bridge and diagnostics controls.
    pub imu_events: ImuEventBridge,
    pub partial_refreshes: u8,
    pub panel_awake: bool,
    pub select_presses: u32,
    pub orientation: DisplayOrientation,
    pub regional: RegionalPreferences,
    pub router: ScreenRouter,
    pub board: BoardSnapshot,
    pub storage: StorageSnapshot,
    /// Password-free snapshot owned by the networking boundary.
    pub network: NetworkSnapshot,
    /// SD-backed alarm schedules and active-alarm UI snapshot.
    pub alarms: AlarmSnapshot,
    /// Playback-only ES8311 diagnostics snapshot.
    pub audio: AudioSnapshot,
    /// Selected Audio-overview action.
    pub audio_action_selected: usize,
    /// Selected Clock-overview action: set date & time or RTC details.
    pub clock_action_selected: usize,
    /// Runtime-only "Set date & time" editor draft, opened from the Clock
    /// overview and discarded on cancel or save.
    pub clock_time_editor: Option<ClockTimeEditor>,
    /// Committed local edit, converted to the RTC storage basis, awaiting a
    /// hardware write from the runtime owner in main.rs.
    clock_set_time_request: Option<crate::rtc::RtcDateTime>,
    /// Timezone committed from the "Set date & time" editor, awaiting
    /// best-effort persistence to `WIFI.TXT` from the runtime owner in
    /// main.rs. Applied to `regional` immediately regardless of whether
    /// persistence succeeds.
    clock_set_timezone_request: Option<crate::regional::TimeZoneProfile>,
    /// Selected Network action: configure via phone, saved networks or
    /// provisioning details.
    pub network_action_selected: usize,
    /// Unified portal lifecycle snapshot: file transfer plus (when reached
    /// via the bootstrap hotspot instead of an already-joined LAN) Wi-Fi
    /// provisioning, replacing the old on-device rotary-keyboard credential
    /// editor.
    pub wifi_transfer: WifiTransferSnapshot,
    wifi_transfer_request: Option<WifiTransferUiRequest>,
    /// Read-only saved-network list and rotary selection for the "Saved
    /// networks" screen. Adding or changing a password only happens through
    /// the phone portal.
    pub network_saved: NetworkSavedUiState,
    network_saved_forget_request: Option<String>,
    /// SD-backed PCM WAV voice-note catalog and recorder UI snapshot.
    pub voice_notes: VoiceNotesUiState,
    /// Global display-maintenance menu opened by a physical Power long press.
    pub power_key_menu: PowerKeyMenuUiState,
    power_key_menu_return_route: ScreenRoute,
    power_key_manual_refresh_requested: bool,
    /// GitHub-release OTA check/install lifecycle, shown on the Software
    /// Update screen.
    pub ota: OtaCheckState,
    ota_request: Option<OtaUiRequest>,
    /// Lazily-aggregated reading time/speed/streak snapshot, refreshed by
    /// the runtime owner in main.rs when the Reading Stats screen is opened
    /// (see [`Self::take_reading_stats_refresh_request`]).
    pub reading_stats: ReadingStatsSnapshot,
    reading_stats_refresh_requested: bool,
    /// Set whenever a Reader page turn actually moves the current position,
    /// regardless of source (every one of them funnels through
    /// [`Self::apply_reader`]). Taken by
    /// the runtime owner in main.rs, which owns the wall clock and SD
    /// access the reading-stats session tracker needs.
    reader_page_turn_event: Option<ReaderLocation>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            // Continue Reading (the trailing `HOME_ENTRIES` slot — see its
            // own doc comment in `menu.rs`) is the most likely first action
            // on a fresh boot, so it starts pre-selected rather than
            // whichever tile happens to sit at index 0.
            home_selected: MAIN_CATEGORY_COUNT - 1,
            category_selected: [0; CATEGORY_COUNT],
            category_usage: CategoryUsage::default(),
            display_action_selected: 0,
            display: DisplayPreferences::default(),
            dictionary: DictionaryUiState::default(),
            unit_converter: UnitConverterUiState::default(),
            reader: ReaderUiState::default(),
            imu_events: ImuEventBridge::default(),
            partial_refreshes: 0,
            panel_awake: true,
            select_presses: 0,
            orientation: DisplayOrientation::default(),
            regional: RegionalPreferences::default(),
            router: ScreenRouter::default(),
            board: BoardSnapshot::default(),
            storage: StorageSnapshot::default(),
            network: NetworkSnapshot::default(),
            alarms: AlarmSnapshot::default(),
            audio: AudioSnapshot::default(),
            audio_action_selected: 0,
            clock_action_selected: 0,
            clock_time_editor: None,
            clock_set_time_request: None,
            clock_set_timezone_request: None,
            network_action_selected: 0,
            wifi_transfer: WifiTransferSnapshot::default(),
            wifi_transfer_request: None,
            network_saved: NetworkSavedUiState::default(),
            network_saved_forget_request: None,
            voice_notes: VoiceNotesUiState::default(),
            power_key_menu: PowerKeyMenuUiState::default(),
            power_key_menu_return_route: ScreenRoute::Home,
            power_key_manual_refresh_requested: false,
            ota: OtaCheckState::default(),
            ota_request: None,
            reading_stats: ReadingStatsSnapshot::default(),
            reading_stats_refresh_requested: false,
            reader_page_turn_event: None,
        }
    }
}

impl AppState {
    /// Compact local time label for the persistent header status glyphs.
    #[must_use]
    pub fn status_time_label(&self) -> String {
        self.board.time_label(self.regional)
    }

    /// Compact local date label ("Thu, Aug 13") for the persistent header
    /// status glyphs.
    #[must_use]
    pub fn status_date_label(&self) -> String {
        self.board.rtc.map_or_else(
            || "Date unavailable".into(),
            |rtc| compact_local_date(self.regional.localize_rtc(rtc)),
        )
    }

    /// Whether Wi-Fi is fully connected, for the persistent header Wi-Fi icon.
    #[must_use]
    pub fn wifi_connected(&self) -> bool {
        self.network.wifi_state == crate::network::WifiConnectionState::Connected
    }

    /// Battery percentage for the persistent header battery icon, when the
    /// PMIC reports a connected battery.
    #[must_use]
    pub fn battery_percent(&self) -> Option<u8> {
        self.board
            .power
            .and_then(|snapshot| snapshot.battery_percent)
    }

    /// Whether the persistent header should show its charging glyph.
    ///
    /// Deliberately `charging || vbus_present` rather than `charging` alone:
    /// the AXP2101's charger state machine reports `charging` only while
    /// actively pushing current (trickle/pre-charge/constant-current/
    /// constant-voltage) and switches to charge-done once the battery tops
    /// off, even though USB power is still connected. Gating the icon on
    /// `charging` alone made it disappear as soon as the battery finished
    /// charging while the cable stayed plugged in, which read as a bug
    /// ("the cable is connected and it's charging") rather than the correct
    /// "fully charged, still on external power" state. `vbus_present` keeps
    /// the glyph up for as long as the cable is actually attached.
    #[must_use]
    pub fn battery_charging(&self) -> bool {
        self.board
            .power
            .is_some_and(|snapshot| snapshot.charging || snapshot.vbus_present)
    }

    /// Apply one debounced button event to routes whose behavior is fully
    /// hardware-independent. Files, Alarms and Audio remain delegated to their
    /// existing owners from main.rs.
    pub fn apply(&mut self, event: ButtonEvent) {
        let route = self.router.current();
        if route == ScreenRoute::Home {
            self.apply_home(event);
        } else if route.is_category() {
            self.apply_category(route, event);
        } else if route == ScreenRoute::Display {
            self.apply_display(event);
        } else if route == ScreenRoute::Language {
            self.apply_language(event);
        } else if route == ScreenRoute::PowerKeyMenu {
            self.apply_power_key_menu(event);
        } else if route == ScreenRoute::Dictionary {
            self.apply_dictionary(event);
        } else if route == ScreenRoute::UnitConverter {
            self.apply_unit_converter(event);
        } else if route == ScreenRoute::MotionEvents {
            self.apply_motion_events(event);
        } else if route == ScreenRoute::ClockSetTime {
            self.apply_clock_set_time(event);
        } else if matches!(
            route,
            ScreenRoute::VoiceNotes
                | ScreenRoute::VoiceNoteDetails
                | ScreenRoute::VoiceNoteRecording
        ) {
            self.apply_voice_notes(event);
        } else if matches!(
            route,
            ScreenRoute::ContinueReading
                | ScreenRoute::Library
                | ScreenRoute::LibraryBookActions
                | ScreenRoute::LibraryBookBookmarks
                | ScreenRoute::ReaderBookmarks
                | ScreenRoute::ReaderLoading
                | ScreenRoute::ReaderPage
                | ScreenRoute::ReaderOptions
                | ScreenRoute::ReaderPreferences
                | ScreenRoute::ReaderToc
        ) {
            self.apply_reader(event);
        } else {
            match (route, event) {
                (ScreenRoute::Clock, ButtonEvent::Up) => {
                    self.clock_action_selected = self
                        .clock_action_selected
                        .checked_sub(1)
                        .unwrap_or(CLOCK_ACTION_COUNT - 1);
                }
                (ScreenRoute::Clock, ButtonEvent::Down) => {
                    self.clock_action_selected =
                        (self.clock_action_selected + 1) % CLOCK_ACTION_COUNT;
                }
                (ScreenRoute::Clock, ButtonEvent::Select) => {
                    self.note_select_press();
                    if self.clock_action_selected == 0 {
                        self.open_clock_time_editor();
                        self.router.navigate_to(ScreenRoute::ClockSetTime);
                    } else {
                        self.router.navigate_to(ScreenRoute::ClockDetails);
                    }
                }
                (ScreenRoute::Environment, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.router.navigate_to(ScreenRoute::EnvironmentDetails);
                }
                (ScreenRoute::Motion, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.router.navigate_to(ScreenRoute::MotionEvents);
                }
                (ScreenRoute::Network, ButtonEvent::Up) => {
                    self.network_action_selected = self
                        .network_action_selected
                        .checked_sub(1)
                        .unwrap_or(NETWORK_ACTION_COUNT - 1);
                }
                (ScreenRoute::Network, ButtonEvent::Down) => {
                    self.network_action_selected =
                        (self.network_action_selected + 1) % NETWORK_ACTION_COUNT;
                }
                (ScreenRoute::Network, ButtonEvent::Select) => {
                    self.note_select_press();
                    match self.network_action_selected {
                        0 => {
                            self.request_wifi_transfer_start();
                            self.router.navigate_to(ScreenRoute::WifiTransfer);
                        }
                        1 => self.router.navigate_to(ScreenRoute::NetworkSaved),
                        _ => self.router.navigate_to(ScreenRoute::NetworkDetails),
                    }
                }
                (ScreenRoute::NetworkSaved, ButtonEvent::Up) => {
                    self.network_saved.move_previous();
                }
                (ScreenRoute::NetworkSaved, ButtonEvent::Down) => {
                    self.network_saved.move_next();
                }
                (ScreenRoute::NetworkSaved, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.confirm_or_arm_network_saved_forget();
                }
                (ScreenRoute::WifiTransfer, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.wifi_transfer_request = Some(WifiTransferUiRequest::Stop);
                    self.router.navigate_to(ScreenRoute::Home);
                }
                (ScreenRoute::DeviceInfo, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.router.navigate_to(ScreenRoute::DeviceInfoBoard);
                }
                (ScreenRoute::OtaUpdate, ButtonEvent::Select) => {
                    self.note_select_press();
                    match &self.ota {
                        OtaCheckState::UpdateAvailable {
                            version,
                            download_url,
                        } => {
                            self.ota_request = Some(OtaUiRequest::InstallNow {
                                version: version.clone(),
                                download_url: download_url.clone(),
                            });
                            self.ota = OtaCheckState::Installing;
                        }
                        state if state.can_check() => {
                            self.ota_request = Some(OtaUiRequest::CheckNow);
                            self.ota = OtaCheckState::Checking;
                        }
                        _ => {}
                    }
                }
                (ScreenRoute::DeviceInfoBoard, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.router.navigate_to(ScreenRoute::DeviceInfoRuntime);
                }
                (
                    ScreenRoute::Environment
                    | ScreenRoute::Motion
                    | ScreenRoute::DeviceInfo
                    | ScreenRoute::DeviceInfoBoard,
                    ButtonEvent::Up | ButtonEvent::Down,
                )
                | (
                    ScreenRoute::AudioDetails
                    | ScreenRoute::ClockDetails
                    | ScreenRoute::DeviceInfoRuntime
                    | ScreenRoute::EnvironmentDetails
                    | ScreenRoute::MotionDetails
                    | ScreenRoute::NetworkDetails
                    | ScreenRoute::WifiTransfer,
                    _,
                )
                | (ScreenRoute::Files | ScreenRoute::Alarms | ScreenRoute::Audio, _) => {}
                _ => {}
            }
        }
        self.sync_orientation_for_active_route();
    }

    fn apply_home(&mut self, event: ButtonEvent) {
        let count = home_entries().len();
        match event {
            ButtonEvent::Up => {
                self.home_selected = self.home_selected.checked_sub(1).unwrap_or(count - 1);
            }
            ButtonEvent::Down => self.home_selected = (self.home_selected + 1) % count,
            ButtonEvent::Select => {
                self.note_select_press();
                if let Some(entry) = home_entries().get(self.home_selected) {
                    if entry.route == ScreenRoute::WifiTransfer {
                        self.request_wifi_transfer_start();
                    }
                    if entry.route == ScreenRoute::ReadingStats {
                        self.reading_stats_refresh_requested = true;
                    }
                    if entry.route == ScreenRoute::Library {
                        self.reader.refresh_library();
                    }
                    if entry.route == ScreenRoute::ContinueReading {
                        self.activate_continue_reading();
                    } else {
                        self.router.navigate_to(entry.route);
                    }
                }
            }
        }
    }

    /// Resume the saved book directly instead of routing through the old
    /// Continue Reading summary screen (`ScreenRoute::ContinueReading`
    /// itself) — used by the Home dashboard's card (`apply_home`).
    fn activate_continue_reading(&mut self) {
        if self.reader.session.is_some() {
            self.router.navigate_to(ScreenRoute::ReaderPage);
        } else if self.reader.request_continue() {
            // A cache hit (`promote_cached_session`) leaves `loading` at
            // `None` and lands the book straight in `session`, same as the
            // already-open case above; only an actual reload routes through
            // the loading screen.
            let target = if self.reader.loading.is_some() {
                ScreenRoute::ReaderLoading
            } else {
                ScreenRoute::ReaderPage
            };
            self.router.navigate_to(target);
        } else {
            self.reader.refresh_library();
            self.router.navigate_to(ScreenRoute::Library);
        }
    }

    fn apply_category(&mut self, route: ScreenRoute, event: ButtonEvent) {
        let entries = self.category_usage.ordered_entries(route);
        match event {
            ButtonEvent::Up => {
                let selected = self.category_selection_mut(route);
                *selected = selected.checked_sub(1).unwrap_or(entries.len() - 1);
            }
            ButtonEvent::Down => {
                let selected = self.category_selection_mut(route);
                *selected = (*selected + 1) % entries.len();
            }
            ButtonEvent::Select => {
                let target = entries[self.category_selection(route)].route;
                self.note_select_press();
                // The opened entry moves to the front of "Most used", so keep
                // the cursor on it for when BOOT brings the user back here.
                if self.category_usage.record(route, target) {
                    *self.category_selection_mut(route) = 0;
                }
                if target == ScreenRoute::Audio {
                    self.audio_action_selected = 0;
                }
                if target == ScreenRoute::Display {
                    self.display_action_selected = 0;
                }
                if target == ScreenRoute::VoiceNotes {
                    self.voice_notes.refresh_catalog();
                }
                if target == ScreenRoute::Dictionary {
                    self.dictionary.refresh_pack_status();
                }
                if target == ScreenRoute::OtaUpdate {
                    self.request_ota_check_if_idle();
                }
                self.router.navigate_to(target);
            }
        }
    }

    fn apply_dictionary(&mut self, event: ButtonEvent) {
        if event == ButtonEvent::Select {
            self.note_select_press();
        }
        self.dictionary.apply_button(event);
    }

    fn apply_voice_notes(&mut self, event: ButtonEvent) {
        match self.router.current() {
            ScreenRoute::VoiceNotes => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                }
                let start = self.voice_notes.apply_list_button(event);
                if start {
                    self.router.navigate_to(ScreenRoute::VoiceNoteRecording);
                } else if event == ButtonEvent::Select && self.voice_notes.selected >= 2 {
                    self.voice_notes.clear_transient_details();
                    self.router.navigate_to(ScreenRoute::VoiceNoteDetails);
                }
            }
            ScreenRoute::VoiceNoteDetails => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                }
                let was_title_editing = self.voice_notes.title_editing;
                let was_delete_confirmation = self.voice_notes.delete_confirmation;
                self.voice_notes.apply_detail_button(event);
                if event == ButtonEvent::Select
                    && !was_title_editing
                    && !was_delete_confirmation
                    && self.voice_notes.detail_selected == 4
                {
                    self.voice_notes.request_stop_playback();
                    self.voice_notes.clear_transient_details();
                    self.router.navigate_to(ScreenRoute::VoiceNotes);
                }
            }
            ScreenRoute::VoiceNoteRecording => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                }
                self.voice_notes.apply_recording_button(event);
            }
            _ => {}
        }
    }

    fn apply_motion_events(&mut self, event: ButtonEvent) {
        match event {
            ButtonEvent::Up => self.imu_events.select_previous_control(),
            ButtonEvent::Down => self.imu_events.select_next_control(),
            ButtonEvent::Select => {
                self.note_select_press();
                if self.imu_events.apply_selected_control() == ImuControlOutcome::OpenDetails {
                    self.router.navigate_to(ScreenRoute::MotionDetails);
                }
            }
        }
    }

    /// Seed the runtime "Set date & time" editor from the current UTC
    /// instant (derived from the RTC reading, or a sane fallback while the
    /// RTC is unavailable) and the currently active regional timezone.
    fn open_clock_time_editor(&mut self) {
        let anchor_utc = self
            .board
            .rtc
            .map_or_else(clock_time_editor::fallback_local, |rtc| {
                self.regional.rtc_to_utc(rtc)
            });
        self.clock_time_editor = Some(ClockTimeEditor::new(anchor_utc, self.regional));
    }

    fn apply_clock_set_time(&mut self, event: ButtonEvent) {
        match event {
            ButtonEvent::Up => {
                if let Some(editor) = self.clock_time_editor.as_mut() {
                    editor.adjust(1);
                }
            }
            ButtonEvent::Down => {
                if let Some(editor) = self.clock_time_editor.as_mut() {
                    editor.adjust(-1);
                }
            }
            ButtonEvent::Select => {
                self.note_select_press();
                let save = self
                    .clock_time_editor
                    .as_ref()
                    .is_some_and(|editor| editor.selected_field() == ClockEditField::Save);
                if save {
                    if let Some(editor) = self.clock_time_editor.take() {
                        let local = editor.draft.as_local_rtc();
                        self.clock_set_time_request =
                            Some(self.regional.local_to_rtc_for_zone(editor.timezone, local));
                        self.regional.timezone = editor.timezone;
                        self.clock_set_timezone_request = Some(editor.timezone);
                    }
                    self.router.navigate_to(ScreenRoute::Clock);
                } else if let Some(editor) = self.clock_time_editor.as_mut() {
                    editor.advance_field();
                }
            }
        }
    }

    /// Feed one native QMI8658 reading into the debounced event bridge while
    /// preserving the latest raw sample for diagnostics screens.
    pub fn update_imu_event_sample(
        &mut self,
        reading: ImuReading,
        now_ms: u64,
    ) -> Option<ImuDetectedEvent> {
        self.board.imu = Some(reading);
        self.imu_events.process(reading, now_ms)
    }

    /// Record a Reader page turn that actually moved the current position,
    /// for the reading-stats session tracker in main.rs to pick up (see
    /// [`Self::take_reader_page_turn_event`]). `before` is the location
    /// captured immediately prior to the `next_page`/`previous_page` call;
    /// a turn that hits a book boundary and doesn't move is not recorded.
    fn note_reader_page_turn(&mut self, before: Option<ReaderLocation>) {
        let Some(session) = self.reader.session.as_ref() else {
            return;
        };
        let after = session.current_location();
        if before.map(|location| location.byte_offset) == Some(after.byte_offset) {
            return;
        }
        self.reader_page_turn_event = Some(after);
    }

    /// Taken by the runtime owner in main.rs after every `apply()` call, so
    /// it can feed the reading-stats session tracker (which owns the wall
    /// clock and SD access `AppState` deliberately does not).
    #[must_use]
    pub fn take_reader_page_turn_event(&mut self) -> Option<ReaderLocation> {
        self.reader_page_turn_event.take()
    }

    /// The Reading Stats screen was just opened; the runtime owner in
    /// main.rs should recompute [`Self::reading_stats`] from SD.
    #[must_use]
    pub fn take_reading_stats_refresh_request(&mut self) -> bool {
        core::mem::take(&mut self.reading_stats_refresh_requested)
    }

    pub fn update_reading_stats_snapshot(&mut self, snapshot: ReadingStatsSnapshot) {
        self.reading_stats = snapshot;
    }

    fn apply_unit_converter(&mut self, event: ButtonEvent) {
        match event {
            ButtonEvent::Up => self.unit_converter.increase_active(),
            ButtonEvent::Down => self.unit_converter.decrease_active(),
            ButtonEvent::Select => {
                self.note_select_press();
                self.unit_converter.select_next_field();
            }
        }
    }

    /// Route a held SELECT into keyboard-style screens before the other
    /// contextual handlers. Future text-entry apps should compose the shared
    /// KeyboardGridNavigation helper and join this routing boundary.
    pub fn apply_keyboard_select_long_press(&mut self) -> bool {
        if self.router.current() == ScreenRoute::VoiceNoteDetails
            && self.voice_notes.title_editing
        {
            self.voice_notes.toggle_title_editor_navigation_axis()
        } else if self.router.current() == ScreenRoute::Dictionary {
            self.dictionary.toggle_navigation_axis();
            true
        } else {
            false
        }
    }

    /// A held SELECT on the reader page opens Reader Options from normal
    /// reading, or exits the in-page dictionary mode immediately from any of
    /// its sub-phases back to normal reading. (A quick SELECT is what enters
    /// the dictionary mode.)
    pub fn apply_reader_dictionary_select_long_press(&mut self) -> bool {
        if self.router.current() != ScreenRoute::ReaderPage {
            return false;
        }
        if matches!(self.reader.dictionary_mode, ReaderDictionaryMode::Off) {
            self.reader.options_selected = 0;
            self.router.navigate_to(ScreenRoute::ReaderOptions);
            return true;
        }
        self.reader.toggle_dictionary_mode()
    }

    /// Held SELECT on Saved networks arms or confirms the "forget?" step
    /// exactly like a short press: a two-step confirmation naturally invites
    /// holding the button a beat too long on the second press, and a plain
    /// long press has no other meaning on this route, so it must not be
    /// silently swallowed here.
    pub fn apply_network_saved_select_long_press(&mut self) -> bool {
        if self.router.current() != ScreenRoute::NetworkSaved {
            return false;
        }
        self.confirm_or_arm_network_saved_forget();
        true
    }

    fn confirm_or_arm_network_saved_forget(&mut self) {
        if self.network_saved.confirming_forget {
            if let Some(entry) = self.network_saved.selected_entry() {
                self.network_saved_forget_request = Some(entry.ssid.clone());
            }
            self.network_saved.confirming_forget = false;
        } else {
            self.network_saved.begin_forget_confirmation();
        }
    }

    /// Held SELECT on the Library grid opens the "book actions" overlay
    /// (mark as completed / bookmarks) for whichever cover is currently
    /// selected.
    pub fn apply_library_select_long_press(&mut self) -> bool {
        if self.router.current() != ScreenRoute::Library {
            return false;
        }
        let Some(entry) = self
            .reader
            .visible_entries()
            .get(self.reader.library_selected)
            .cloned()
        else {
            return false;
        };
        self.reader.open_book_actions(entry.book);
        self.router.navigate_to(ScreenRoute::LibraryBookActions);
        true
    }

    fn apply_reader(&mut self, event: ButtonEvent) {
        match self.router.current() {
            ScreenRoute::ContinueReading => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                    if self.reader.session.is_some() {
                        self.router.navigate_to(ScreenRoute::ReaderPage);
                    } else if self.reader.request_continue() {
                        let target = if self.reader.loading.is_some() {
                            ScreenRoute::ReaderLoading
                        } else {
                            ScreenRoute::ReaderPage
                        };
                        self.router.navigate_to(target);
                    } else {
                        self.reader.refresh_library();
                        self.router.navigate_to(ScreenRoute::Library);
                    }
                }
            }
            ScreenRoute::Library => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                }
                if self.reader.apply_library_button(event) {
                    // Reopening the book already in `self.reader.session`
                    // (see `request_open_visible`) leaves `loading` at
                    // `None`, so go straight to `ReaderPage` instead of
                    // routing through the reload screen for no reason.
                    let target = if self.reader.loading.is_some() {
                        ScreenRoute::ReaderLoading
                    } else {
                        ScreenRoute::ReaderPage
                    };
                    self.router.navigate_to(target);
                }
            }
            ScreenRoute::ReaderBookmarks => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                }
                if self.reader.apply_bookmarks_button(event) {
                    self.router.navigate_to(ScreenRoute::ReaderLoading);
                }
            }
            ScreenRoute::LibraryBookActions => match event {
                ButtonEvent::Up => self.reader.cycle_book_action_previous(),
                ButtonEvent::Down => self.reader.cycle_book_action_next(),
                ButtonEvent::Select => {
                    self.note_select_press();
                    match self.reader.selected_book_action() {
                        LibraryBookAction::MarkCompleted => {
                            self.reader.mark_book_actions_target_completed();
                            self.router.navigate_to(ScreenRoute::Library);
                        }
                        LibraryBookAction::Bookmarks => {
                            self.reader.book_bookmarks_selected = 0;
                            self.router.navigate_to(ScreenRoute::LibraryBookBookmarks);
                        }
                    }
                }
            },
            ScreenRoute::LibraryBookBookmarks => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                }
                if self.reader.apply_book_bookmarks_button(event) {
                    self.router.navigate_to(ScreenRoute::ReaderLoading);
                }
            }
            ScreenRoute::ReaderToc => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                }
                if self.reader.apply_toc_button(event) {
                    self.router.navigate_to(ScreenRoute::ReaderPage);
                }
            }
            ScreenRoute::ReaderLoading => {}
            ScreenRoute::ReaderPage => match (self.reader.dictionary_mode.clone(), event) {
                (ReaderDictionaryMode::Off, ButtonEvent::Up) => {
                    let before = self
                        .reader
                        .session
                        .as_ref()
                        .map(ReaderSession::current_location);
                    self.reader.previous_page();
                    self.note_reader_page_turn(before);
                }
                (ReaderDictionaryMode::Off, ButtonEvent::Down) => {
                    let before = self
                        .reader
                        .session
                        .as_ref()
                        .map(ReaderSession::current_location);
                    self.reader.next_page();
                    self.note_reader_page_turn(before);
                }
                (ReaderDictionaryMode::Off, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.reader.toggle_dictionary_mode();
                }
                (ReaderDictionaryMode::LineSelect { .. }, ButtonEvent::Up) => {
                    self.reader.dictionary_move_line(-1);
                }
                (ReaderDictionaryMode::LineSelect { .. }, ButtonEvent::Down) => {
                    self.reader.dictionary_move_line(1);
                }
                (ReaderDictionaryMode::LineSelect { .. }, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.reader.dictionary_confirm_line();
                }
                (ReaderDictionaryMode::WordSelect { .. }, ButtonEvent::Up) => {
                    self.reader.dictionary_move_word(-1);
                }
                (ReaderDictionaryMode::WordSelect { .. }, ButtonEvent::Down) => {
                    self.reader.dictionary_move_word(1);
                }
                (ReaderDictionaryMode::WordSelect { .. }, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.reader.dictionary_confirm_word();
                }
                (ReaderDictionaryMode::Definition { .. }, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.reader.dictionary_step_back();
                }
                (ReaderDictionaryMode::Definition { .. }, ButtonEvent::Up | ButtonEvent::Down) => {}
            },
            ScreenRoute::ReaderOptions => match event {
                ButtonEvent::Up => self.reader.cycle_option_previous(),
                ButtonEvent::Down => self.reader.cycle_option_next(),
                ButtonEvent::Select => {
                    self.note_select_press();
                    match self.reader.selected_option() {
                        ReaderOption::Bookmark => self.reader.toggle_current_bookmark(),
                        ReaderOption::Bookmarks => {
                            self.reader.bookmarks_selected = 0;
                            self.router.navigate_to(ScreenRoute::ReaderBookmarks);
                        }
                        ReaderOption::TableOfContents => {
                            self.reader.toc_selected = 0;
                            self.router.navigate_to(ScreenRoute::ReaderToc)
                        }
                        ReaderOption::ReadingPreferences => {
                            self.reader.begin_preferences_edit();
                            self.router.navigate_to(ScreenRoute::ReaderPreferences);
                        }
                    }
                }
            },
            ScreenRoute::ReaderPreferences if self.reader.preference_edit.is_some() => {
                match event {
                    ButtonEvent::Up => self.reader.cycle_preference_editor_previous(),
                    ButtonEvent::Down => self.reader.cycle_preference_editor_next(),
                    ButtonEvent::Select => {
                        self.note_select_press();
                        if self.reader.commit_preference_edit() {
                            self.router.navigate_to(ScreenRoute::ReaderLoading);
                        }
                    }
                }
            }
            ScreenRoute::ReaderPreferences => match event {
                ButtonEvent::Up => self.reader.cycle_preference_previous(),
                ButtonEvent::Down => self.reader.cycle_preference_next(),
                ButtonEvent::Select => {
                    self.note_select_press();
                    self.reader.open_preference_editor();
                }
            },
            _ => {}
        }
    }

    /// Advance one bounded Reader loading or nearby-cache stage. main.rs calls
    /// this from the event loop so the loading screen is visible before reads.
    pub fn tick_reader(&mut self) -> ReaderTickOutcome {
        let outcome = self.reader.tick();
        if outcome == ReaderTickOutcome::FirstPageReady {
            self.router.navigate_to(ScreenRoute::ReaderPage);
        }
        self.sync_orientation_for_active_route();
        outcome
    }

    #[must_use]
    pub fn take_reader_clear_ghost_request(&mut self) -> bool {
        self.reader.take_clear_ghost_request()
    }

    /// Open the global display-maintenance menu from any awake product route.
    pub fn open_power_key_menu(&mut self) {
        if self.router.current() != ScreenRoute::PowerKeyMenu {
            self.power_key_menu_return_route = self.router.current();
        }
        self.power_key_menu.reset();
        self.router.navigate_to(ScreenRoute::PowerKeyMenu);
    }

    /// Return the route that long Power sleep should restore after wake. This
    /// unwraps the maintenance menu so a hold from that screen sleeps the
    /// underlying product route rather than restoring the transient menu.
    #[must_use]
    pub fn power_key_sleep_restore_route(&self) -> ScreenRoute {
        if self.router.current() == ScreenRoute::PowerKeyMenu {
            self.power_key_menu_return_route
        } else {
            self.router.current()
        }
    }

    #[must_use]
    pub fn take_power_key_manual_refresh_request(&mut self) -> bool {
        core::mem::take(&mut self.power_key_manual_refresh_requested)
    }

    fn close_power_key_menu(&mut self) {
        self.router.navigate_to(self.power_key_menu_return_route);
        self.sync_orientation_for_active_route();
    }

    fn apply_power_key_menu(&mut self, event: ButtonEvent) {
        if event == ButtonEvent::Select {
            self.note_select_press();
        }
        match self.power_key_menu.apply_button(event) {
            PowerKeyMenuOutcome::None => {}
            PowerKeyMenuOutcome::ClearGhosting => {
                self.power_key_manual_refresh_requested = true;
                self.close_power_key_menu();
            }
            PowerKeyMenuOutcome::Cancel => self.close_power_key_menu(),
        }
    }

    fn apply_display(&mut self, event: ButtonEvent) {
        match event {
            ButtonEvent::Up => {
                self.display_action_selected = self
                    .display_action_selected
                    .checked_sub(1)
                    .unwrap_or(DISPLAY_ACTION_COUNT - 1);
            }
            ButtonEvent::Down => {
                self.display_action_selected =
                    (self.display_action_selected + 1) % DISPLAY_ACTION_COUNT;
            }
            ButtonEvent::Select => {
                self.note_select_press();
                match self.display_action_selected {
                    0 => self.display.cycle_font_family(),
                    1 => self.display.cycle_font_size(),
                    _ => self.display.cycle_sleep_screen(),
                }
            }
        }
    }

    /// Apply one Language-settings event: Select cycles between the
    /// supported on-device languages. A single row needs no selection cursor.
    fn apply_language(&mut self, event: ButtonEvent) {
        if event == ButtonEvent::Select {
            self.note_select_press();
            self.regional.locale = self.regional.locale.next();
        }
    }

    /// Apply one Audio-overview event. Hardware requests are returned to
    /// main.rs so this product state remains independent of ESP-IDF handles.
    pub fn apply_audio_button(&mut self, event: ButtonEvent) -> Option<AudioUiRequest> {
        match event {
            ButtonEvent::Up => {
                self.audio_action_selected = self
                    .audio_action_selected
                    .checked_sub(1)
                    .unwrap_or(AUDIO_ACTION_COUNT - 1);
                None
            }
            ButtonEvent::Down => {
                self.audio_action_selected = (self.audio_action_selected + 1) % AUDIO_ACTION_COUNT;
                None
            }
            ButtonEvent::Select => {
                self.note_select_press();
                match self.audio_action_selected {
                    0 => Some(AudioUiRequest::PlayTestChime),
                    1 => Some(AudioUiRequest::StopPlayback),
                    2 => Some(AudioUiRequest::VolumeUp),
                    3 => Some(AudioUiRequest::VolumeDown),
                    4 => Some(AudioUiRequest::ToggleMute),
                    _ => {
                        self.router.navigate_to(ScreenRoute::AudioDetails);
                        None
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn category_selection(&self, route: ScreenRoute) -> usize {
        category_index(route)
            .map(|index| self.category_selected[index])
            .unwrap_or(0)
    }

    fn category_selection_mut(&mut self, route: ScreenRoute) -> &mut usize {
        let index = category_index(route).expect("category route required");
        &mut self.category_selected[index]
    }

    pub fn note_select_press(&mut self) {
        self.select_presses = self.select_presses.saturating_add(1);
    }

    /// Navigate one level toward Home. The hardware runtime calls this after a
    /// validated GPIO0 BOOT-button press.
    pub fn back(&mut self) {
        if self.router.current() == ScreenRoute::PowerKeyMenu {
            self.close_power_key_menu();
            return;
        }
        if self.router.current() == ScreenRoute::ReaderPage && self.reader.dictionary_step_back() {
            return;
        }
        if self.router.current() == ScreenRoute::WifiTransfer {
            self.wifi_transfer_request = Some(WifiTransferUiRequest::Stop);
        }
        if self.router.current() == ScreenRoute::VoiceNoteRecording {
            self.voice_notes.request_cancel_recording();
        }
        if self.router.current() == ScreenRoute::VoiceNoteDetails {
            if self.voice_notes.title_editing {
                self.voice_notes.cancel_title_edit();
                return;
            }
            if self.voice_notes.delete_confirmation {
                self.voice_notes.clear_transient_details();
                return;
            }
            self.voice_notes.request_stop_playback();
            self.voice_notes.clear_transient_details();
        }
        if self.router.current() == ScreenRoute::ClockSetTime {
            self.clock_time_editor = None;
        }
        if self.router.current() == ScreenRoute::ReaderLoading {
            self.reader.cancel_loading();
        }
        if self.router.current() == ScreenRoute::ReaderPreferences {
            // BACK steps out one level at a time: from an open row editor it
            // discards the candidate and returns to the flat list; only from
            // the flat list itself does it leave for Reader Options.
            if !self.reader.cancel_preference_edit() {
                self.router.navigate_to(ScreenRoute::ReaderOptions);
            }
        } else {
            self.router.back();
        }
        if self.router.current() == ScreenRoute::Home {
            // Home shows the Continue Reading card, remaining-time clause
            // included (see `screens::category::draw_continue_reading_tile`,
            // drawn from `screens::home`). Forward navigation into it
            // already requests a refresh (see `apply_home` above); stepping
            // BACK into it — e.g. out of an active reading session — needs
            // the same trigger, or the remaining-time line keeps showing
            // whatever was true before that session, lagging behind the
            // title/cover/percent the card already reads straight from
            // `state.reader`.
            self.reading_stats_refresh_requested = true;
        }
        self.sync_orientation_for_active_route();
    }

    /// Reader's landscape reading preference is the only route that ever
    /// leaves Portrait; every other route forces it back on entry.
    fn sync_orientation_for_active_route(&mut self) {
        self.orientation = if self.router.current() == ScreenRoute::ReaderPage {
            match self.reader.preferences.orientation {
                ReaderOrientation::Portrait => DisplayOrientation::Portrait,
                ReaderOrientation::Landscape => DisplayOrientation::Landscape,
            }
        } else {
            DisplayOrientation::Portrait
        };
    }

    #[must_use]
    pub const fn active_route(&self) -> ScreenRoute {
        self.router.current()
    }

    pub fn update_board_snapshot(&mut self, board: BoardSnapshot) {
        self.board = board;
    }

    pub fn update_storage_snapshot(&mut self, storage: StorageSnapshot) {
        self.storage = storage;
    }

    pub fn update_network_snapshot(&mut self, network: NetworkSnapshot) {
        self.network = network;
    }

    pub fn update_wifi_transfer_snapshot(&mut self, snapshot: WifiTransferSnapshot) {
        self.wifi_transfer = snapshot;
    }

    #[must_use]
    pub fn take_wifi_transfer_request(&mut self) -> Option<WifiTransferUiRequest> {
        self.wifi_transfer_request.take()
    }

    /// Start the existing LAN portal from a feature shortcut without exposing
    /// HTTP-server ownership outside the main-loop dispatcher.
    pub fn request_wifi_transfer_start(&mut self) {
        if !self.wifi_transfer.is_active() {
            self.wifi_transfer_request = Some(WifiTransferUiRequest::Start);
        }
    }

    /// Refresh the "Saved networks" screen list, as read from `WIFI.TXT` by
    /// the main loop.
    pub fn set_saved_networks(&mut self, networks: Vec<crate::network_saved::SavedNetworkEntry>) {
        self.network_saved.set_networks(networks);
    }

    #[must_use]
    pub fn take_network_saved_forget_request(&mut self) -> Option<String> {
        self.network_saved_forget_request.take()
    }

    pub fn refresh_voice_notes_catalog(&mut self) {
        self.voice_notes.refresh_catalog();
    }

    #[must_use]
    pub fn take_voice_notes_request(&mut self) -> Option<VoiceNotesUiRequest> {
        self.voice_notes.take_request()
    }

    pub fn request_wifi_transfer_stop(&mut self) {
        if self.wifi_transfer.state != WifiTransferState::Off {
            self.wifi_transfer_request = Some(WifiTransferUiRequest::Stop);
        }
    }

    pub fn update_alarm_snapshot(&mut self, alarms: AlarmSnapshot) {
        self.alarms = alarms;
    }

    pub fn update_audio_snapshot(&mut self, audio: AudioSnapshot) {
        self.audio = audio;
    }

    #[must_use]
    pub fn take_ota_request(&mut self) -> Option<OtaUiRequest> {
        self.ota_request.take()
    }

    /// Runtime owner in main.rs reports a completed check or install here.
    pub fn update_ota_state(&mut self, state: OtaCheckState) {
        self.ota = state;
    }

    /// Kick off a background check the first time the Software Update
    /// screen is opened this boot. Subsequent visits just show the last
    /// result; press SELECT to check again explicitly.
    fn request_ota_check_if_idle(&mut self) {
        if self.ota == OtaCheckState::Idle {
            self.ota_request = Some(OtaUiRequest::CheckNow);
            self.ota = OtaCheckState::Checking;
        }
    }

    /// Background periodic check from the main loop, distinct from the
    /// button-driven request above: only starts one when nothing is already
    /// in flight, and never overwrites an update the user hasn't acted on
    /// yet or an install already underway.
    pub fn request_ota_background_check(&mut self) {
        if matches!(
            self.ota,
            OtaCheckState::Idle | OtaCheckState::UpToDate | OtaCheckState::CheckFailed(_)
        ) {
            self.ota_request = Some(OtaUiRequest::CheckNow);
            self.ota = OtaCheckState::Checking;
        }
    }

    /// Take a manually edited local wall-clock value, already converted into
    /// the RTC storage basis, for the runtime owner in main.rs to write.
    #[must_use]
    pub fn take_clock_set_time_request(&mut self) -> Option<crate::rtc::RtcDateTime> {
        self.clock_set_time_request.take()
    }

    /// Take a timezone committed from the "Set date & time" editor for the
    /// runtime owner in main.rs to persist to `WIFI.TXT`.
    #[must_use]
    pub fn take_clock_set_timezone_request(&mut self) -> Option<crate::regional::TimeZoneProfile> {
        self.clock_set_timezone_request.take()
    }

    pub fn set_orientation(&mut self, orientation: DisplayOrientation) {
        self.orientation = orientation;
    }
}

fn compact_local_date(local: crate::rtc::RtcDateTime) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday = WEEKDAYS
        .get(usize::from(local.weekday))
        .copied()
        .unwrap_or("---");
    let month = local
        .month
        .checked_sub(1)
        .and_then(|index| MONTHS.get(usize::from(index)))
        .copied()
        .unwrap_or("---");
    format!("{weekday}, {month} {}", local.day)
}

#[cfg(test)]
mod tests {
    use super::{compact_local_date, AppState, ClockEditField};
    use crate::{
        app::{menu::home_entries, router::ScreenRoute},
        buttons::ButtonEvent,
        reader::{
            BookFormat, ReaderBook, ReaderCachedPage, ReaderDictionaryMode, ReaderPageLine,
            ReaderPreferences, ReaderSession, TextEncoding,
        },
        rtc::RtcDateTime,
    };

    /// Position of `route` on the Home dashboard, so these tests keep
    /// working as tiles are added or removed.
    fn home_index(route: ScreenRoute) -> usize {
        home_entries()
            .iter()
            .position(|entry| entry.route == route)
            .expect("route is on the Home dashboard")
    }

    #[test]
    fn renders_compact_status_bar_date() {
        assert_eq!(
            compact_local_date(RtcDateTime {
                year: 2026,
                month: 6,
                day: 4,
                weekday: 4,
                hour: 8,
                minute: 13,
                second: 0,
            }),
            "Thu, Jun 4"
        );
    }

    #[test]
    fn motion_event_screen_cycles_thresholds_and_opens_sensor_details() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Motion);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::MotionEvents);
        let original = state.imu_events.thresholds.tilt_enter_mg;
        state.apply(ButtonEvent::Select);
        assert_ne!(state.imu_events.thresholds.tilt_enter_mg, original);
        for _ in 0..6 {
            state.apply(ButtonEvent::Down);
        }
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::MotionDetails);
    }

    #[test]
    fn home_categories_wrap_and_open() {
        let mut state = AppState::default();
        // Continue Reading starts pre-selected (see `AppState::default`).
        assert_eq!(state.home_selected, home_index(ScreenRoute::ContinueReading));
        state.apply(ButtonEvent::Down);
        assert_eq!(state.home_selected, 0); // wraps to the first grid tile (Library).
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Library);
    }

    fn reader_session_with_lines(lines: &[&str]) -> ReaderSession {
        ReaderSession {
            book: ReaderBook {
                path: "Book.txt".into(),
                title: "Book".into(),
                format: BookFormat::Text,
                size_bytes: 1000,
                modified_seconds: 0,
            },
            encoding: TextEncoding::Utf8,
            epub_document: None,
            layout: ReaderPreferences::default().layout(),
            current_page: 0,
            page_number_base: 0,
            page_offsets: vec![0],
            indexed_through: 0,
            index_complete: true,
            cache: vec![ReaderCachedPage {
                page_index: 0,
                byte_offset: 0,
                next_byte_offset: 0,
                lines: lines
                    .iter()
                    .map(|text| ReaderPageLine {
                        text: (*text).to_string(),
                        paragraph_end: true,
                        image: None,
                    })
                    .collect(),
            }],
            epub_chapter_pages: Vec::new(),
            epub_pending_chapter: None,
            epub_document_cache_pending: false,
        }
    }

    /// A quick SELECT on the reader page opens the in-page dictionary lookup
    /// mode; BOOT then retraces it one phase at a time instead of leaving
    /// the book, and only falls through to ordinary Back once the mode is
    /// off again. A held SELECT opens Reader Options from normal reading.
    #[test]
    fn reader_select_opens_dictionary_and_back_steps_out_one_level() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::ReaderPage);
        state.reader.session = Some(reader_session_with_lines(&["Il gatto corre veloce"]));

        state.apply(ButtonEvent::Select);
        assert_eq!(
            state.reader.dictionary_mode,
            ReaderDictionaryMode::LineSelect { line_index: 0 }
        );
        assert_eq!(state.active_route(), ScreenRoute::ReaderPage);

        state.apply(ButtonEvent::Select);
        assert_eq!(
            state.reader.dictionary_mode,
            ReaderDictionaryMode::WordSelect {
                line_index: 0,
                word_index: 0
            }
        );

        // BOOT/Back steps back one dictionary-mode level at a time and never
        // leaves ReaderPage while the mode is still active.
        state.back();
        assert_eq!(
            state.reader.dictionary_mode,
            ReaderDictionaryMode::LineSelect { line_index: 0 }
        );
        assert_eq!(state.active_route(), ScreenRoute::ReaderPage);

        state.back();
        assert_eq!(state.reader.dictionary_mode, ReaderDictionaryMode::Off);
        assert_eq!(state.active_route(), ScreenRoute::ReaderPage);

        // A held SELECT exits immediately from any sub-phase.
        state.apply(ButtonEvent::Select);
        state.reader.dictionary_confirm_line();
        assert!(state.apply_reader_dictionary_select_long_press());
        assert_eq!(state.reader.dictionary_mode, ReaderDictionaryMode::Off);
        assert_eq!(state.active_route(), ScreenRoute::ReaderPage);

        // From normal reading, a held SELECT opens Reader Options.
        assert!(state.apply_reader_dictionary_select_long_press());
        assert_eq!(state.active_route(), ScreenRoute::ReaderOptions);
    }

    #[test]
    fn tools_file_browser_returns_to_tools() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Tools);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Tools);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Files);
        state.router.back();
        assert_eq!(state.active_route(), ScreenRoute::Tools);
    }

    #[test]
    fn settings_display_changes_persistent_preferences_without_a_back_row() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Settings);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Settings);
        for _ in 0..4 {
            state.apply(ButtonEvent::Down);
        }
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Display);
        let original = state.display;
        state.apply(ButtonEvent::Select);
        assert_ne!(state.display.font_family, original.font_family);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_ne!(state.display.font_size, original.font_size);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_ne!(state.display.sleep_screen, original.sleep_screen);
        state.apply(ButtonEvent::Down);
        assert_eq!(state.display_action_selected, 0);
        assert_eq!(state.active_route(), ScreenRoute::Display);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Settings);
    }

    #[test]
    fn wifi_transfer_stop_and_return_goes_to_home_not_network() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::WifiTransfer);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::WifiTransfer);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Start)
        );
        state.update_wifi_transfer_snapshot(crate::wifi_transfer::WifiTransferSnapshot {
            state: crate::wifi_transfer::WifiTransferState::Ready,
            url: Some("http://192.168.1.2/".into()),
            code: Some("123456".into()),
            last_action: "Portal ready".into(),
            last_bytes: 0,
            ..Default::default()
        });
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Home);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Stop)
        );
    }

    #[test]
    fn network_configure_via_phone_action_requests_the_same_portal_as_the_home_tile() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Network);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::WifiTransfer);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Start)
        );
        // A second request while already active/starting is suppressed.
        state.wifi_transfer = crate::wifi_transfer::WifiTransferSnapshot::starting();
        state.request_wifi_transfer_start();
        assert_eq!(state.take_wifi_transfer_request(), None);
    }

    #[test]
    fn network_saved_action_opens_the_saved_network_list() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Network);
        state.network_action_selected = 1;
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::NetworkSaved);
    }

    #[test]
    fn network_saved_select_requires_a_second_confirmation_before_forgetting() {
        let mut state = AppState::default();
        state.set_saved_networks(vec![
            crate::network_saved::SavedNetworkEntry {
                ssid: "Home".into(),
                connected: true,
            },
            crate::network_saved::SavedNetworkEntry {
                ssid: "Office".into(),
                connected: false,
            },
        ]);
        state.router.navigate_to(ScreenRoute::NetworkSaved);

        state.apply(ButtonEvent::Select);
        assert!(state.network_saved.confirming_forget);
        assert_eq!(state.take_network_saved_forget_request(), None);

        state.apply(ButtonEvent::Select);
        assert!(!state.network_saved.confirming_forget);
        assert_eq!(
            state.take_network_saved_forget_request(),
            Some("Home".into())
        );
    }

    #[test]
    fn network_saved_held_select_also_arms_and_confirms_forget() {
        // A held SELECT is classified as a long press once it crosses
        // SELECT_LONG_PRESS_MS; on this route that must behave exactly like
        // a short press instead of being silently swallowed, since the
        // two-step forget confirmation naturally invites holding the button
        // a beat too long on the second tap.
        let mut state = AppState::default();
        state.set_saved_networks(vec![crate::network_saved::SavedNetworkEntry {
            ssid: "Home".into(),
            connected: true,
        }]);
        state.router.navigate_to(ScreenRoute::NetworkSaved);

        assert!(state.apply_network_saved_select_long_press());
        assert!(state.network_saved.confirming_forget);
        assert_eq!(state.take_network_saved_forget_request(), None);

        assert!(state.apply_network_saved_select_long_press());
        assert!(!state.network_saved.confirming_forget);
        assert_eq!(
            state.take_network_saved_forget_request(),
            Some("Home".into())
        );
    }

    #[test]
    fn network_saved_held_select_is_a_no_op_off_route() {
        let mut state = AppState::default();
        state.set_saved_networks(vec![crate::network_saved::SavedNetworkEntry {
            ssid: "Home".into(),
            connected: true,
        }]);
        assert!(!state.apply_network_saved_select_long_press());
        assert!(!state.network_saved.confirming_forget);
    }

    #[test]
    fn library_held_select_opens_book_actions_for_the_selected_book() {
        let mut state = AppState::default();
        state.reader.books = vec![ReaderBook {
            path: "a.txt".into(),
            title: "A".into(),
            format: BookFormat::Text,
            size_bytes: 1,
            modified_seconds: 0,
        }];
        state.router.navigate_to(ScreenRoute::Library);
        state.reader.library_selected = 0;

        assert!(state.apply_library_select_long_press());
        assert_eq!(state.active_route(), ScreenRoute::LibraryBookActions);
        assert_eq!(
            state
                .reader
                .book_actions_target
                .as_ref()
                .map(|book| book.path.as_str()),
            Some("a.txt")
        );
    }

    #[test]
    fn library_held_select_is_a_no_op_off_route() {
        let mut state = AppState::default();
        state.reader.books = vec![ReaderBook {
            path: "a.txt".into(),
            title: "A".into(),
            format: BookFormat::Text,
            size_bytes: 1,
            modified_seconds: 0,
        }];
        assert!(!state.apply_library_select_long_press());
        assert!(state.reader.book_actions_target.is_none());
        assert_eq!(state.active_route(), ScreenRoute::Home);
    }

    #[test]
    fn network_saved_moving_selection_cancels_a_pending_confirmation() {
        let mut state = AppState::default();
        state.set_saved_networks(vec![
            crate::network_saved::SavedNetworkEntry {
                ssid: "Home".into(),
                connected: true,
            },
            crate::network_saved::SavedNetworkEntry {
                ssid: "Office".into(),
                connected: false,
            },
        ]);
        state.router.navigate_to(ScreenRoute::NetworkSaved);
        state.apply(ButtonEvent::Select);
        assert!(state.network_saved.confirming_forget);
        state.apply(ButtonEvent::Down);
        assert!(!state.network_saved.confirming_forget);
    }

    #[test]
    fn home_upload_tile_starts_wifi_transfer_directly() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::WifiTransfer);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::WifiTransfer);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Start)
        );
    }

    #[test]
    fn clock_details_use_down_select_then_hierarchical_back() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Clock);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::ClockDetails);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Clock);
    }

    #[test]
    fn clock_set_time_editor_adjusts_fields_and_requests_hardware_write_on_save() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Clock);
        state.apply(ButtonEvent::Select); // action 0: open the set-time editor
        assert_eq!(state.active_route(), ScreenRoute::ClockSetTime);
        let editor = state.clock_time_editor.expect("editor opened");
        assert_eq!(editor.selected_field(), ClockEditField::Timezone);

        state.apply(ButtonEvent::Select); // advance to Hour
        state.apply(ButtonEvent::Up); // hour + 1
        state.apply(ButtonEvent::Select); // advance to Minute
        state.apply(ButtonEvent::Up); // minute + 1
        for _ in 0..(ClockEditField::COUNT - 3) {
            state.apply(ButtonEvent::Select); // advance to Save
        }
        assert_eq!(
            state.clock_time_editor.unwrap().selected_field(),
            ClockEditField::Save
        );
        state.apply(ButtonEvent::Select); // commit
        assert_eq!(state.active_route(), ScreenRoute::Clock);
        assert!(state.clock_time_editor.is_none());
        assert!(state.take_clock_set_time_request().is_some());
        assert!(state.take_clock_set_timezone_request().is_some());
    }

    #[test]
    fn clock_set_time_editor_timezone_field_changes_the_live_regional_zone_on_save() {
        let mut state = AppState::default();
        assert_eq!(
            state.regional.timezone,
            crate::regional::TimeZoneProfile::AmericaNewYork
        );
        state.router.navigate_to(ScreenRoute::Clock);
        state.apply(ButtonEvent::Select); // open editor on the Timezone field
        state.apply(ButtonEvent::Up); // cycle away from America/New_York
        assert_ne!(
            state.clock_time_editor.unwrap().timezone,
            crate::regional::TimeZoneProfile::AmericaNewYork
        );
        for _ in 0..(ClockEditField::COUNT - 1) {
            state.apply(ButtonEvent::Select); // advance to Save
        }
        state.apply(ButtonEvent::Select); // commit
        assert_ne!(
            state.regional.timezone,
            crate::regional::TimeZoneProfile::AmericaNewYork
        );
    }

    #[test]
    fn clock_set_time_editor_discards_draft_on_cancel() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Clock);
        state.apply(ButtonEvent::Select);
        assert!(state.clock_time_editor.is_some());
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Clock);
        assert!(state.clock_time_editor.is_none());
        assert!(state.take_clock_set_time_request().is_none());
    }

    #[test]
    fn tools_dictionary_opens_native_screen_without_sd_pack() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Tools);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Tools);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Dictionary);
        assert!(!state.dictionary.pack_ready);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.dictionary.query, "B");
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Tools);
    }

    #[test]
    fn voice_note_title_editor_select_long_toggles_axis_and_boot_back_cancels() {
        let mut state = AppState::default();
        state
            .voice_notes
            .notes
            .push(crate::voice_notes::VoiceNoteEntry {
                file_name: "VOICE001.WAV".into(),
                title: "VOICE NOTE 001".into(),
                recorded_at: "2026-06-06  11:43:24".into(),
                wav_bytes: 44,
                pcm_bytes: 0,
                duration_seconds: 0,
            });
        state.voice_notes.selected = 2;
        state.voice_notes.begin_title_edit();
        state.router.navigate_to(ScreenRoute::VoiceNoteDetails);
        assert_eq!(
            state.voice_notes.title_editor_navigation_mode_label(),
            "NAV H"
        );
        assert!(state.apply_keyboard_select_long_press());
        assert_eq!(
            state.voice_notes.title_editor_navigation_mode_label(),
            "NAV V"
        );
        state.back();
        assert!(!state.voice_notes.title_editing);
        assert_eq!(state.active_route(), ScreenRoute::VoiceNoteDetails);
    }

    #[test]
    fn dictionary_keyboard_select_long_toggles_axis_preserves_key_and_boot_back_route() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Dictionary);
        state.apply(ButtonEvent::Down);
        assert_eq!(state.dictionary.selected_key_label(), "B");
        assert!(state.apply_keyboard_select_long_press());
        assert_eq!(state.dictionary.navigation_mode_label(), "NAV V");
        assert_eq!(state.dictionary.selected_key_label(), "B");
        state.apply(ButtonEvent::Down);
        assert_eq!(state.dictionary.selected_key_label(), "H");
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Tools);
    }

    #[test]
    fn tools_unit_converter_opens_and_edits_without_hardware() {
        use crate::unit_converter::{ConverterField, UnitCategory};

        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Tools);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Tools);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::UnitConverter);
        assert_eq!(state.unit_converter.active_field, ConverterField::Category);
        state.apply(ButtonEvent::Up);
        assert_eq!(state.unit_converter.category, UnitCategory::Mass);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.unit_converter.active_field, ConverterField::FromUnit);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Tools);
    }

    #[test]
    fn home_continue_reading_card_starts_selected_and_routes_straight_to_library() {
        // Continue Reading starts pre-selected (see `AppState::default`) —
        // it's the most likely first action on a fresh boot. SELECT on it
        // resumes (or, with nothing saved, falls through to Library)
        // immediately instead of stopping on the old intermediate Reader
        // category / summary screen, neither of which exists any more.
        let mut state = AppState::default();
        assert_eq!(state.home_selected, home_index(ScreenRoute::ContinueReading));
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Library);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Home);
    }

    #[test]
    fn resuming_from_home_continue_reading_card_returns_home_on_back() {
        // Exiting the reader always lands on Home (`ScreenRoute::ReaderPage`'s
        // static `parent()`), whatever screen the book was opened from.
        let mut state = AppState::default();
        state.reader.session = Some(reader_session_with_lines(&["Line"]));
        assert_eq!(state.home_selected, home_index(ScreenRoute::ContinueReading));
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::ReaderPage);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Home);
    }

    #[test]
    fn opening_a_book_from_library_returns_home_on_back() {
        // The counterpart to the Home card above: opening a book from the
        // Library screen also sends BACK to Home, never back to Library.
        let mut state = AppState::default();
        let session = reader_session_with_lines(&["Line"]);
        state.reader.books = vec![session.book.clone()];
        state.reader.session = Some(session);
        state.router.navigate_to(ScreenRoute::Library);
        state.reader.library_selected = 0;
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::ReaderPage);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Home);
    }

    #[test]
    fn stepping_back_into_home_requests_a_reading_stats_refresh() {
        // The remaining-time clause on Home's Continue Reading card (see
        // `screens::category::draw_continue_reading_tile`) comes from
        // `reading_stats`, refreshed only on request rather than every tick
        // (it costs an SD read). Forward navigation into Statistics already
        // requests one; stepping BACK into Home -- e.g. out of an active
        // reading session -- must too, or the clause lags behind the
        // title/cover/percent the card reads straight from `state.reader`.
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::ReadingStats);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::ReadingStats);
        assert!(state.take_reading_stats_refresh_request());

        state.back(); // ReadingStats -> Home.
        assert_eq!(state.active_route(), ScreenRoute::Home);
        assert!(state.take_reading_stats_refresh_request());
    }

    #[test]
    fn reader_preferences_open_editor_preview_then_commit_or_cancel() {
        use crate::reader::{ReadingPreference, ReadingTheme};

        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::ReaderPreferences);
        assert_eq!(
            state.reader.selected_preference(),
            ReadingPreference::ReadingTheme
        );
        let initial_theme = state.reader.preferences.theme;

        // Moving the flat-list selection never opens an editor or touches
        // the real preferences.
        state.apply(ButtonEvent::Down);
        assert_eq!(
            state.reader.selected_preference(),
            ReadingPreference::Orientation
        );
        assert!(state.reader.preference_edit.is_none());
        state.apply(ButtonEvent::Up);
        assert_eq!(
            state.reader.selected_preference(),
            ReadingPreference::ReadingTheme
        );
        assert_eq!(state.reader.preferences.theme, initial_theme);

        // SELECT opens the row's editor; browsing candidates previews but
        // does not commit.
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::ReaderPreferences);
        assert_eq!(state.reader.preference_edit, Some(state.reader.preferences));
        state.apply(ButtonEvent::Down);
        assert_eq!(
            state.reader.preference_edit.unwrap().theme,
            ReadingTheme::HighContrast
        );
        assert_eq!(state.reader.preferences.theme, initial_theme);

        // BACK from an open editor discards the previewed candidate and
        // steps back to the flat list — it does not leave the screen.
        state.back();
        assert!(state.reader.preference_edit.is_none());
        assert_eq!(state.reader.preferences.theme, initial_theme);
        assert_eq!(state.active_route(), ScreenRoute::ReaderPreferences);

        // Re-opening and committing with SELECT applies the browsed value.
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert!(state.reader.preference_edit.is_none());
        assert_eq!(state.reader.preferences.theme, ReadingTheme::HighContrast);
        assert_eq!(state.active_route(), ScreenRoute::ReaderPreferences);

        // BACK on the flat list (no editor open) leaves for Reader Options.
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::ReaderOptions);
    }
    #[test]
    fn productivity_voice_notes_opens_recording_route_and_queues_start() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Tools);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Tools);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::VoiceNotes);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::VoiceNoteRecording);
        assert_eq!(
            state.take_voice_notes_request(),
            Some(crate::voice_notes::VoiceNotesUiRequest::StartRecording)
        );
    }

    #[test]
    fn power_key_menu_preserves_return_route_and_requests_manual_refresh() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Library);
        state.open_power_key_menu();
        assert_eq!(state.active_route(), ScreenRoute::PowerKeyMenu);
        assert_eq!(
            state.power_key_sleep_restore_route(),
            ScreenRoute::Library
        );
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Library);
        assert!(state.take_power_key_manual_refresh_request());
        assert!(!state.take_power_key_manual_refresh_request());
    }

    #[test]
    fn power_key_menu_cancel_and_back_return_without_refresh() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Clock);
        state.open_power_key_menu();
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Clock);
        assert!(!state.take_power_key_manual_refresh_request());
        state.open_power_key_menu();
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Clock);
    }
}
