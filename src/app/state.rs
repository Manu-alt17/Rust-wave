//! Product UI state transitions independent of hardware wiring.

use crate::{
    audio::{AudioSnapshot, AudioUiRequest},
    board_services::BoardSnapshot,
    build_info::FIRMWARE_VERSION,
    buttons::ButtonEvent,
    clock_time_editor::{self, ClockEditField, ClockTimeEditor},
    network::NetworkSnapshot,
    network_saved::NetworkSavedUiState,
    orientation::DisplayOrientation,
    ota::{InstallProgress, OtaCheckState, OtaUiRequest, UpdateChannel},
    power_key_menu::{PowerKeyMenuOutcome, PowerKeyMenuUiState},
    reader::{
        LibraryBookAction, ReaderDictionaryMode, ReaderGoToOutcome, ReaderLocation, ReaderOption,
        ReaderOrientation, ReaderSession, ReaderTickOutcome, ReaderUiState,
    },
    reading_stats::ReadingStatsSnapshot,
    regional::RegionalPreferences,
    storage::StorageSnapshot,
    usb_disk::UsbDiskPhase,
    wifi_transfer::{WifiTransferSnapshot, WifiTransferState, WifiTransferUiRequest},
};

use super::{
    audiobooks::AudiobookUiState,
    display::DisplayPreferences,
    menu::{category_index, home_entries, CategoryUsage, CATEGORY_COUNT, MAIN_CATEGORY_COUNT},
    router::{ScreenRoute, ScreenRouter},
    setup::{SetupOutcome, SetupUiState},
};

/// Number of selectable rows in the playback overview screen.
pub const AUDIO_ACTION_COUNT: usize = 6;
/// Number of selectable rows in the Display settings screen.
pub const DISPLAY_ACTION_COUNT: usize = 3;
/// Set date & time or open RTC details rows on the Clock overview screen.
pub const CLOCK_ACTION_COUNT: usize = 2;
/// Configure via phone, saved networks, retry connection and details rows
/// on the Network screen.
pub const NETWORK_ACTION_COUNT: usize = 4;
/// Rows of the Software Update screen: the action the current state offers
/// (check, install, retry...) and the release channel.
pub const OTA_ACTION_COUNT: usize = 2;
/// Ways of copying files offered by the Upload screen: the Wi-Fi portal and
/// the USB cable.
pub const UPLOAD_ACTION_COUNT: usize = 2;
/// Rows of the Info screen: the next page and "Restore settings".
pub const INFO_ACTION_COUNT: usize = 2;
/// Rows of the warning shown when the microSD cannot be read: restart, or
/// go on without it.
pub const CARD_WARNING_ACTION_COUNT: usize = 2;

/// "Restore settings" on the Info screen: it acts on a second SELECT.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsResetStage {
    #[default]
    Idle,
    /// Chosen once: the next SELECT restores, BOOT or the rocker cancel.
    Armed,
    /// Just restored; shown until the selection moves.
    Done,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppState {
    pub home_selected: usize,
    category_selected: [usize; CATEGORY_COUNT],
    /// Recently-opened Settings entries shown under "Most used";
    /// persisted by the runtime owner in main.rs whenever it changes.
    pub category_usage: CategoryUsage,
    pub display_action_selected: usize,
    pub display: DisplayPreferences,
    /// TXT / reflowable EPUB Reader library, staged opening, RAM cache and options.
    pub reader: ReaderUiState,
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
    /// SSID of a saved network the user asked to connect to, awaiting the
    /// runtime owner in main.rs.
    network_join_request: Option<String>,
    /// "Retry connection" on the Network screen, awaiting the runtime owner
    /// in main.rs: reconnect with the whole saved list.
    network_retry_request: bool,
    /// SSID of the saved network a "Connect" is trying, until the attempt
    /// settles one way or the other.
    network_join_target: Option<String>,
    /// SSID of the saved network the last "Connect" could not join, for the
    /// Network screen to say so. Cleared by the next attempt.
    pub network_join_failed: Option<String>,
    /// Where SELECT or BOOT on the transfer portal screen returns: Home when
    /// opened from the Upload tile, Network when opened from "Configure via
    /// phone".
    wifi_transfer_return_route: ScreenRoute,
    /// "Connect to PC" screen, and its request to start disk mode.
    pub usb_disk: UsbDiskPhase,
    usb_disk_request: bool,
    /// Selected way of copying files on the Upload screen: 0 the Wi-Fi
    /// portal, 1 the USB cable.
    pub upload_selected: usize,
    /// Audiobook library, player and saved listening positions.
    pub audiobooks: AudiobookUiState,
    /// Global display-maintenance menu opened by a physical Power long press.
    pub power_key_menu: PowerKeyMenuUiState,
    power_key_menu_return_route: ScreenRoute,
    power_key_manual_refresh_requested: bool,
    restart_requested: bool,
    /// Selected row of the Info screen: 0 opens the next page, 1 restores
    /// the settings.
    pub info_selected: usize,
    /// Where "Restore settings" on the Info screen stands.
    pub settings_reset: SettingsResetStage,
    /// GitHub-release OTA check/install lifecycle, shown on the Software
    /// Update screen.
    pub ota: OtaCheckState,
    /// Releases the Software Update screen checks: changed there with UP or
    /// DOWN, persisted by the runtime owner in main.rs.
    pub ota_channel: UpdateChannel,
    /// Selected row of the Software Update screen: `0` the action, `1` the
    /// channel.
    pub ota_action_selected: usize,
    /// `true` after a first SELECT on "Install", awaiting a second one to
    /// start the download. Moving the selection, leaving the screen or any
    /// new check result disarms it.
    pub ota_install_armed: bool,
    /// How far the firmware download has got while an update installs.
    pub ota_install_progress: Option<InstallProgress>,
    /// The bootloader in flash as it describes itself (`v5.5.1,
    /// 2026-10-02`), read at boot by the runtime owner in main.rs; `None`
    /// when it carries no description.
    pub installed_bootloader: Option<String>,
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
    /// The first-run pages: shown once on a new card, and from Settings.
    pub setup: SetupUiState,
    /// Selected row of the "card not readable" warning: 0 restarts, 1 goes
    /// on without the card.
    pub card_warning_selected: usize,
    /// The books folder is known to hold nothing: Home's Continue card
    /// offers to add a first book and opens Upload. Set by a scan (or by
    /// main.rs at boot) and dropped as soon as an upload is opened, since
    /// from then on only the next scan can tell.
    pub library_known_empty: bool,
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
            reader: ReaderUiState::default(),
            partial_refreshes: 0,
            panel_awake: true,
            select_presses: 0,
            orientation: DisplayOrientation::default(),
            regional: RegionalPreferences::default(),
            router: ScreenRouter::default(),
            board: BoardSnapshot::default(),
            storage: StorageSnapshot::default(),
            network: NetworkSnapshot::default(),
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
            network_join_request: None,
            network_retry_request: false,
            network_join_target: None,
            network_join_failed: None,
            wifi_transfer_return_route: ScreenRoute::Upload,
            audiobooks: AudiobookUiState::default(),
            usb_disk: UsbDiskPhase::Idle,
            usb_disk_request: false,
            upload_selected: 0,
            power_key_menu: PowerKeyMenuUiState::default(),
            power_key_menu_return_route: ScreenRoute::Home,
            power_key_manual_refresh_requested: false,
            restart_requested: false,
            info_selected: 0,
            settings_reset: SettingsResetStage::Idle,
            ota: OtaCheckState::default(),
            ota_channel: UpdateChannel::of_version(FIRMWARE_VERSION),
            ota_action_selected: 0,
            ota_install_armed: false,
            ota_install_progress: None,
            installed_bootloader: None,
            ota_request: None,
            reading_stats: ReadingStatsSnapshot::default(),
            reading_stats_refresh_requested: false,
            reader_page_turn_event: None,
            setup: SetupUiState::default(),
            card_warning_selected: 0,
            library_known_empty: false,
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
    /// hardware-independent. Files and Audio remain delegated to their
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
        } else if route == ScreenRoute::ClockSetTime {
            self.apply_clock_set_time(event);
        } else if route == ScreenRoute::AudiobookLibrary {
            if event == ButtonEvent::Select {
                self.note_select_press();
            }
            if event == ButtonEvent::Select && self.audiobooks_are_empty() {
                // Nothing to play: SELECT goes where audiobooks are added.
                self.open_upload();
            } else if self.audiobooks.apply_library(event) {
                self.router.navigate_to(ScreenRoute::AudiobookPlayer);
            }
        } else if route == ScreenRoute::AudiobookPlayer {
            if event == ButtonEvent::Select {
                self.note_select_press();
            }
            self.audiobooks.apply_player(event);
        } else if route == ScreenRoute::Upload {
            self.apply_upload(event);
        } else if route == ScreenRoute::UsbDisk {
            if event == ButtonEvent::Select && self.usb_disk == UsbDiskPhase::Idle {
                self.note_select_press();
                self.usb_disk_request = true;
            }
        } else if route == ScreenRoute::Setup {
            self.apply_setup(event);
        } else if route == ScreenRoute::CardWarning {
            self.apply_card_warning(event);
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
                | ScreenRoute::ReaderGoTo
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
                            self.wifi_transfer_return_route = ScreenRoute::Network;
                            self.router.navigate_to(ScreenRoute::WifiTransfer);
                        }
                        1 => {
                            self.network_saved.close_menu();
                            self.router.navigate_to(ScreenRoute::NetworkSaved);
                        }
                        2 => {
                            // Nothing to retry without a saved network.
                            if self.network.saved_network_count > 0 {
                                self.network_retry_request = true;
                                self.network_join_target = None;
                                self.network_join_failed = None;
                            }
                        }
                        _ => self.router.navigate_to(ScreenRoute::NetworkDetails),
                    }
                }
                (ScreenRoute::NetworkSaved, ButtonEvent::Up) => {
                    if self.network_saved.menu.is_some() {
                        self.network_saved.menu_previous();
                    } else {
                        self.network_saved.move_previous();
                    }
                }
                (ScreenRoute::NetworkSaved, ButtonEvent::Down) => {
                    if self.network_saved.menu.is_some() {
                        self.network_saved.menu_next();
                    } else {
                        self.network_saved.move_next();
                    }
                }
                (ScreenRoute::NetworkSaved, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.activate_network_saved();
                }
                (ScreenRoute::WifiTransfer, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.wifi_transfer_request = Some(WifiTransferUiRequest::Stop);
                    self.router.navigate_to(self.wifi_transfer_return_route);
                }
                (ScreenRoute::DeviceInfo, _) => self.apply_device_info(event),
                (ScreenRoute::OtaUpdate, ButtonEvent::Select) if self.ota_action_selected == 1 => {
                    self.note_select_press();
                    self.ota_install_armed = false;
                    // Not while a check or an install is under way: its
                    // result belongs to the channel it was started for.
                    if self.ota.can_check() {
                        self.ota_channel = self.ota_channel.toggled();
                        self.ota_request = Some(OtaUiRequest::CheckNow);
                        self.ota = OtaCheckState::Checking;
                        self.ota_action_selected = 0;
                    }
                }
                (ScreenRoute::OtaUpdate, ButtonEvent::Select) => {
                    self.note_select_press();
                    match &self.ota {
                        // Installing takes two presses: the first only arms
                        // it and the screen asks to confirm.
                        OtaCheckState::UpdateAvailable { .. } if !self.ota_install_armed => {
                            self.ota_install_armed = true;
                        }
                        OtaCheckState::UpdateAvailable {
                            version,
                            download_url,
                        } => {
                            self.ota_request = Some(OtaUiRequest::InstallNow {
                                version: version.clone(),
                                download_url: download_url.clone(),
                            });
                            self.ota = OtaCheckState::Installing;
                            self.ota_install_armed = false;
                        }
                        OtaCheckState::BootloaderAvailable { release, asset, .. } => {
                            self.ota_request = Some(OtaUiRequest::PrepareBootloader {
                                release: release.clone(),
                                asset: asset.clone(),
                            });
                            self.ota = OtaCheckState::PreparingBootloader;
                        }
                        OtaCheckState::BootloaderReady { .. }
                        | OtaCheckState::BootloaderDamaged(_) => {
                            // Checked here, at the press, against the battery
                            // reading the screen is showing.
                            match crate::bootloader_update::power_allows_write(
                                self.battery_percent(),
                                self.battery_charging(),
                            ) {
                                Ok(()) => {
                                    self.ota_request = Some(OtaUiRequest::InstallBootloader);
                                    self.ota = OtaCheckState::InstallingBootloader;
                                }
                                Err(battery) => {
                                    self.ota = OtaCheckState::BootloaderFailed(
                                        bootloader_power_refusal(self.regional.locale, battery),
                                    );
                                }
                            }
                        }
                        state if state.can_check() => {
                            self.ota_request = Some(OtaUiRequest::CheckNow);
                            self.ota = OtaCheckState::Checking;
                        }
                        _ => {}
                    }
                }
                (ScreenRoute::OtaUpdate, ButtonEvent::Up | ButtonEvent::Down) => {
                    // Two rows, so both directions move to the other one.
                    // The rocker only moves the selection: changing the
                    // channel takes a SELECT on its row.
                    self.ota_install_armed = false;
                    if self.ota.can_check() {
                        self.ota_action_selected =
                            (self.ota_action_selected + 1) % OTA_ACTION_COUNT;
                    }
                }
                (ScreenRoute::DeviceInfoBoard, ButtonEvent::Select) => {
                    self.note_select_press();
                    self.router.navigate_to(ScreenRoute::DeviceInfoRuntime);
                }
                (ScreenRoute::DeviceInfoBoard, ButtonEvent::Up | ButtonEvent::Down)
                | (
                    ScreenRoute::AudioDetails
                    | ScreenRoute::ClockDetails
                    | ScreenRoute::DeviceInfoRuntime
                    | ScreenRoute::NetworkDetails
                    | ScreenRoute::WifiTransfer,
                    _,
                )
                | (ScreenRoute::Files | ScreenRoute::Audio, _) => {}
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
                    if entry.route == ScreenRoute::ReadingStats {
                        self.reading_stats_refresh_requested = true;
                    }
                    if entry.route == ScreenRoute::Library {
                        self.refresh_library();
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

    /// Upload: the rocker moves between the two ways of copying files,
    /// SELECT opens the chosen one. The Wi-Fi portal starts only here, once
    /// it was chosen, and comes back to this screen when it is closed.
    fn apply_upload(&mut self, event: ButtonEvent) {
        match event {
            ButtonEvent::Up => {
                self.upload_selected = self
                    .upload_selected
                    .checked_sub(1)
                    .unwrap_or(UPLOAD_ACTION_COUNT - 1);
            }
            ButtonEvent::Down => {
                self.upload_selected = (self.upload_selected + 1) % UPLOAD_ACTION_COUNT;
            }
            ButtonEvent::Select => {
                self.note_select_press();
                if self.upload_selected == 0 {
                    self.request_wifi_transfer_start();
                    self.wifi_transfer_return_route = ScreenRoute::Upload;
                    self.router.navigate_to(ScreenRoute::WifiTransfer);
                } else {
                    self.router.navigate_to(ScreenRoute::UsbDisk);
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
            // No book to resume: the Library, or straight to Upload when
            // there is no book at all.
            self.refresh_library();
            if self.library_known_empty {
                self.open_upload();
            } else {
                self.router.navigate_to(ScreenRoute::Library);
            }
        }
    }

    /// Rescan the books folder and note whether it turned out empty.
    fn refresh_library(&mut self) {
        self.reader.refresh_library();
        self.library_known_empty =
            self.reader.books.is_empty() && self.reader.library_error.is_none();
    }

    /// Whether the audiobook list is empty because there are none, rather
    /// than because the card could not be read.
    #[must_use]
    pub fn audiobooks_are_empty(&self) -> bool {
        self.audiobooks.books.is_empty() && self.audiobooks.scan_error.is_none()
    }

    /// Whether the Library is empty because there are no books, rather than
    /// because the card could not be read.
    #[must_use]
    pub fn library_is_empty(&self) -> bool {
        self.reader.visible_entries().is_empty() && self.reader.library_error.is_none()
    }

    /// Open the Upload chooser from an empty list.
    fn open_upload(&mut self) {
        self.upload_selected = 0;
        self.router.navigate_to(ScreenRoute::Upload);
    }

    /// The first-run pages for a new card, from the page they were left at.
    /// Called by the runtime owner in main.rs at boot.
    pub fn begin_first_run_setup(&mut self, page_index: u8) {
        let wifi_ready = self.setup_wifi_ready();
        self.setup.start_first_run(page_index, wifi_ready);
        self.router.navigate_to(ScreenRoute::Setup);
    }

    /// The "card not readable" warning. Called by the runtime owner in
    /// main.rs at boot.
    pub fn show_card_warning(&mut self) {
        self.card_warning_selected = 0;
        self.router.navigate_to(ScreenRoute::CardWarning);
    }

    /// Whether the device already has a Wi-Fi network: the setup's Wi-Fi
    /// page then offers "next" rather than "skip".
    #[must_use]
    pub fn setup_wifi_ready(&self) -> bool {
        self.wifi_connected() || self.network.saved_network_count > 0
    }

    fn apply_setup(&mut self, event: ButtonEvent) {
        if event == ButtonEvent::Select {
            self.note_select_press();
        }
        let wifi_ready = self.setup_wifi_ready();
        let outcome = self.setup.apply(event, wifi_ready);
        self.run_setup_outcome(outcome);
    }

    fn run_setup_outcome(&mut self, outcome: SetupOutcome) {
        match outcome {
            SetupOutcome::None => {}
            SetupOutcome::LanguageChosen(locale) => self.regional.locale = locale,
            SetupOutcome::OpenWifiPortal => {
                self.request_wifi_transfer_start();
                self.wifi_transfer_return_route = ScreenRoute::Setup;
                self.router.navigate_to(ScreenRoute::WifiTransfer);
            }
            SetupOutcome::OpenClockEditor => {
                self.open_clock_time_editor();
                self.router.navigate_to(ScreenRoute::ClockSetTime);
            }
            SetupOutcome::OpenUsbDisk => {
                self.library_known_empty = false;
                self.router.navigate_to(ScreenRoute::UsbDisk);
            }
            SetupOutcome::Finished => {
                self.router.navigate_to(ScreenRoute::Home);
                self.reading_stats_refresh_requested = true;
            }
            SetupOutcome::LeftToSettings => self.router.navigate_to(ScreenRoute::Settings),
        }
    }

    /// The card cannot be read: restart to try again, or go on without it.
    fn apply_card_warning(&mut self, event: ButtonEvent) {
        match event {
            ButtonEvent::Up | ButtonEvent::Down => {
                self.card_warning_selected =
                    (self.card_warning_selected + 1) % CARD_WARNING_ACTION_COUNT;
            }
            ButtonEvent::Select => {
                self.note_select_press();
                if self.card_warning_selected == 0 {
                    self.restart_requested = true;
                } else {
                    self.router.navigate_to(ScreenRoute::Home);
                    self.reading_stats_refresh_requested = true;
                }
            }
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
                if target == ScreenRoute::OtaUpdate {
                    self.ota_action_selected = 0;
                    self.ota_install_armed = false;
                    self.request_ota_check_if_idle();
                }
                if target == ScreenRoute::Network {
                    self.network_action_selected = 0;
                }
                if target == ScreenRoute::Setup {
                    self.setup.start_from_settings(self.regional.locale);
                }
                self.router.navigate_to(target);
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
                    // Opened from the first-run pages, it goes back to them.
                    self.router.navigate_to(if self.setup.active {
                        ScreenRoute::Setup
                    } else {
                        ScreenRoute::Clock
                    });
                } else if let Some(editor) = self.clock_time_editor.as_mut() {
                    editor.advance_field();
                }
            }
        }
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

    /// Held SELECT on Saved networks opens the selected network's menu, as
    /// a short press does: a long press has no other meaning on this route,
    /// so it must not be silently swallowed. It never runs a menu action,
    /// which stays a deliberate short press.
    pub fn apply_network_saved_select_long_press(&mut self) -> bool {
        if self.router.current() != ScreenRoute::NetworkSaved {
            return false;
        }
        if self.network_saved.menu.is_none() {
            self.network_saved.open_menu();
        }
        true
    }

    /// SELECT on Saved networks: open the selected network's action menu,
    /// or run the action under the menu cursor.
    fn activate_network_saved(&mut self) {
        use crate::network_saved::SavedNetworkAction;

        let Some(action) = self.network_saved.selected_menu_action() else {
            self.network_saved.open_menu();
            return;
        };
        let ssid = self
            .network_saved
            .selected_entry()
            .map(|entry| entry.ssid.clone());
        self.network_saved.close_menu();
        match (action, ssid) {
            (SavedNetworkAction::Connect, Some(ssid)) => {
                self.network_join_request = Some(ssid.clone());
                self.network_join_target = Some(ssid);
                self.network_join_failed = None;
                // The Network screen shows the attempt as it goes.
                self.router.navigate_to(ScreenRoute::Network);
            }
            (SavedNetworkAction::Forget, Some(ssid)) => {
                self.network_saved_forget_request = Some(ssid);
            }
            _ => {}
        }
    }

    /// SSID of a saved network to connect to, for the runtime owner in
    /// main.rs.
    #[must_use]
    pub fn take_network_join_request(&mut self) -> Option<String> {
        self.network_join_request.take()
    }

    /// "Retry connection" was chosen: the runtime owner in main.rs should
    /// reconnect with the saved-network list.
    #[must_use]
    pub fn take_network_retry_request(&mut self) -> bool {
        core::mem::take(&mut self.network_retry_request)
    }

    /// SELECT on "Connect to PC" asks the runtime to start disk mode.
    pub fn take_usb_disk_request(&mut self) -> bool {
        std::mem::take(&mut self.usb_disk_request)
    }

    /// Held SELECT on the audiobook player opens its menu (skip, tracks,
    /// stop).
    pub fn apply_audiobook_select_long_press(&mut self) -> bool {
        if self.router.current() != ScreenRoute::AudiobookPlayer {
            return false;
        }
        self.audiobooks.open_menu();
        true
    }

    /// Held SELECT on a bookmark list deletes the selected bookmark: the
    /// Reader's own list (the open book's bookmarks) or the one reached from
    /// a Library book's actions.
    pub fn apply_bookmark_select_long_press(&mut self) -> bool {
        match self.router.current() {
            ScreenRoute::ReaderBookmarks => self.reader.delete_selected_session_bookmark(),
            ScreenRoute::LibraryBookBookmarks => self.reader.delete_selected_book_bookmark(),
            _ => false,
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
                        self.refresh_library();
                        self.router.navigate_to(ScreenRoute::Library);
                    }
                }
            }
            ScreenRoute::Library => {
                if event == ButtonEvent::Select {
                    self.note_select_press();
                }
                if event == ButtonEvent::Select && self.library_is_empty() {
                    // No book to open: SELECT goes where books are added.
                    self.open_upload();
                } else if self.reader.apply_library_button(event) {
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
                        LibraryBookAction::MarkUnread => {
                            self.reader.mark_book_actions_target_unread();
                            self.router.navigate_to(ScreenRoute::Library);
                        }
                        LibraryBookAction::Bookmarks => {
                            self.reader.book_bookmarks_selected = 0;
                            self.router.navigate_to(ScreenRoute::LibraryBookBookmarks);
                        }
                        // Deleting takes two presses: the first only arms
                        // it and the row asks to confirm.
                        LibraryBookAction::Delete if !self.reader.book_delete_armed => {
                            self.reader.book_delete_armed = true;
                        }
                        LibraryBookAction::Delete => {
                            // On failure the overlay stays, with the reason.
                            if self.reader.delete_book_actions_target() {
                                self.router.navigate_to(ScreenRoute::Library);
                            }
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
            ScreenRoute::ReaderGoTo => match event {
                ButtonEvent::Up => self.reader.adjust_goto(true),
                ButtonEvent::Down => self.reader.adjust_goto(false),
                ButtonEvent::Select => {
                    self.note_select_press();
                    match self.reader.go_to_percent() {
                        ReaderGoToOutcome::Jumped => {
                            self.router.navigate_to(ScreenRoute::ReaderPage);
                        }
                        ReaderGoToOutcome::Reopening => {
                            self.router.navigate_to(ScreenRoute::ReaderLoading);
                        }
                        ReaderGoToOutcome::Stayed => {}
                    }
                }
            },
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
                            // Open on the chapter being read.
                            self.reader.toc_selected = self.reader.current_toc_index().unwrap_or(0);
                            self.router.navigate_to(ScreenRoute::ReaderToc)
                        }
                        ReaderOption::GoTo => {
                            self.reader.begin_goto();
                            self.router.navigate_to(ScreenRoute::ReaderGoTo);
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
            PowerKeyMenuOutcome::Restart => {
                self.restart_requested = true;
                self.close_power_key_menu();
            }
            PowerKeyMenuOutcome::Cancel => self.close_power_key_menu(),
        }
    }

    /// "Restart" was chosen in the Power-key menu: the runtime owner saves
    /// what is still in memory and restarts the device.
    pub fn take_restart_request(&mut self) -> bool {
        core::mem::take(&mut self.restart_requested)
    }

    /// Info screen: the rocker moves between "next page" and "Restore
    /// settings"; the second asks for a confirming SELECT before it acts.
    fn apply_device_info(&mut self, event: ButtonEvent) {
        match event {
            ButtonEvent::Up | ButtonEvent::Down => {
                self.info_selected = (self.info_selected + 1) % INFO_ACTION_COUNT;
                self.settings_reset = SettingsResetStage::Idle;
            }
            ButtonEvent::Select => {
                self.note_select_press();
                if self.info_selected == 0 {
                    self.settings_reset = SettingsResetStage::Idle;
                    self.router.navigate_to(ScreenRoute::DeviceInfoBoard);
                } else if self.settings_reset == SettingsResetStage::Armed {
                    self.restore_default_settings();
                    self.settings_reset = SettingsResetStage::Done;
                } else {
                    self.settings_reset = SettingsResetStage::Armed;
                }
            }
        }
    }

    /// Back to the values of a first start: text size, standby and sleep
    /// screen, the "most used" settings and the update channel. Language,
    /// clock, Wi-Fi networks, books and reading preferences are the user's
    /// own and stay.
    fn restore_default_settings(&mut self) {
        self.display = DisplayPreferences::default();
        self.category_usage = CategoryUsage::default();
        let channel = UpdateChannel::of_version(FIRMWARE_VERSION);
        if self.ota_channel != channel {
            self.ota_channel = channel;
            // What the last check found was for the other channel.
            if self.ota.can_check() {
                self.ota = OtaCheckState::Idle;
            }
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
                    0 => self.display.cycle_font_size(),
                    1 => self.display.cycle_sleep_screen(),
                    _ => self.display.cycle_auto_sleep(),
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
        // BOOT closes the player menu before it leaves the player; leaving
        // does not stop the book, which keeps playing in the background.
        if self.router.current() == ScreenRoute::AudiobookPlayer && self.audiobooks.close_menu() {
            return;
        }
        // BOOT closes the saved-network action menu before it leaves the
        // list.
        if self.router.current() == ScreenRoute::NetworkSaved && self.network_saved.close_menu() {
            return;
        }
        if self.router.current() == ScreenRoute::Setup {
            let wifi_ready = self.setup_wifi_ready();
            let outcome = self.setup.back(wifi_ready);
            self.setup.select_language(self.regional.locale);
            self.run_setup_outcome(outcome);
            self.sync_orientation_for_active_route();
            return;
        }
        // "Connect to PC" opened from the first-run pages goes back to them.
        if self.router.current() == ScreenRoute::UsbDisk && self.setup.active {
            self.setup.save_current_page();
            self.router.navigate_to(ScreenRoute::Setup);
            self.sync_orientation_for_active_route();
            return;
        }
        if self.router.current() == ScreenRoute::WifiTransfer {
            self.wifi_transfer_request = Some(WifiTransferUiRequest::Stop);
            self.router.navigate_to(self.wifi_transfer_return_route);
            if self.router.current() == ScreenRoute::Home {
                self.reading_stats_refresh_requested = true;
            }
            self.sync_orientation_for_active_route();
            return;
        }
        if self.router.current() == ScreenRoute::OtaUpdate {
            self.ota_install_armed = false;
        }
        if self.router.current() == ScreenRoute::DeviceInfo {
            // BOOT on a restore waiting for its confirmation only cancels it.
            if self.settings_reset == SettingsResetStage::Armed {
                self.settings_reset = SettingsResetStage::Idle;
                return;
            }
            self.settings_reset = SettingsResetStage::Idle;
            self.info_selected = 0;
        }
        if self.router.current() == ScreenRoute::ClockSetTime {
            // BOOT steps back one field; from the first it drops the draft
            // and leaves.
            if self
                .clock_time_editor
                .as_mut()
                .is_some_and(ClockTimeEditor::retreat_field)
            {
                return;
            }
            self.clock_time_editor = None;
            // Opened from the first-run pages, it goes back to them.
            if self.setup.active {
                self.router.navigate_to(ScreenRoute::Setup);
                self.sync_orientation_for_active_route();
                return;
            }
        }
        if self.router.current() == ScreenRoute::ReaderLoading {
            self.reader.cancel_loading();
        }
        // BOOT on a delete waiting for its confirmation only cancels it: the
        // book's options stay open.
        if self.router.current() == ScreenRoute::LibraryBookActions && self.reader.book_delete_armed
        {
            self.reader.book_delete_armed = false;
            return;
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
        self.sync_saved_network_connection();
        self.settle_network_join();
    }

    /// Follow a "Connect" to a saved network to its end. A network that
    /// cannot be joined must not leave the device offline: the failure is
    /// kept for the Network screen and the saved list is tried again, which
    /// brings back the network it was on.
    fn settle_network_join(&mut self) {
        // Until main.rs has taken the request, the snapshot still describes
        // the connection the attempt is about to replace.
        if self.network_join_target.is_none() || self.network_join_request.is_some() {
            return;
        }
        match self.network.wifi_state {
            crate::network::WifiConnectionState::Connected => self.network_join_target = None,
            crate::network::WifiConnectionState::Failed => {
                self.network_join_failed = self.network_join_target.take();
                self.network_retry_request = true;
            }
            _ => {}
        }
    }

    /// Keep the Saved networks list's "connected" mark on the network the
    /// device is actually on, whatever the list was last filled with.
    fn sync_saved_network_connection(&mut self) {
        let connected = if self.network.wifi_state == crate::network::WifiConnectionState::Connected
        {
            self.network.ssid.as_deref()
        } else {
            None
        };
        self.network_saved.mark_connected(connected);
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
        // Books may arrive: only the next scan can say the folder is empty.
        self.library_known_empty = false;
        if !self.wifi_transfer.is_active() {
            self.wifi_transfer_request = Some(WifiTransferUiRequest::Start);
        }
    }

    /// Refresh the "Saved networks" screen list, as read from `WIFI.TXT` by
    /// the main loop.
    pub fn set_saved_networks(&mut self, networks: Vec<crate::network_saved::SavedNetworkEntry>) {
        self.network_saved.set_networks(networks);
        self.sync_saved_network_connection();
    }

    #[must_use]
    pub fn take_network_saved_forget_request(&mut self) -> Option<String> {
        self.network_saved_forget_request.take()
    }

    pub fn request_wifi_transfer_stop(&mut self) {
        if self.wifi_transfer.state != WifiTransferState::Off {
            self.wifi_transfer_request = Some(WifiTransferUiRequest::Stop);
        }
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
        if self.ota != state {
            self.ota_install_armed = false;
        }
        if state != OtaCheckState::Installing {
            self.ota_install_progress = None;
        }
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

/// Why a bootloader write was refused, in the user's language.
fn bootloader_power_refusal(locale: crate::regional::Locale, battery: Option<u8>) -> String {
    use crate::{bootloader_update::MIN_BATTERY_PERCENT, regional::Locale};

    match (battery, locale) {
        (Some(percent), Locale::Italian) => format!(
            "Batteria al {percent}%: caricala almeno al {MIN_BATTERY_PERCENT}% o collega il cavo USB."
        ),
        (Some(percent), Locale::English) => format!(
            "Battery at {percent}%: charge it to {MIN_BATTERY_PERCENT}% or connect the USB cable."
        ),
        (None, Locale::Italian) => "Livello della batteria sconosciuto: collega il cavo USB.".into(),
        (None, Locale::English) => "Battery level unknown: connect the USB cable.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{compact_local_date, AppState, ClockEditField, SettingsResetStage};
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
    fn home_categories_wrap_and_open() {
        let mut state = AppState::default();
        // Continue Reading starts pre-selected (see `AppState::default`).
        assert_eq!(
            state.home_selected,
            home_index(ScreenRoute::ContinueReading)
        );
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
    fn home_file_browser_returns_home() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Files);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Files);
        state.router.back();
        assert_eq!(state.active_route(), ScreenRoute::Home);
    }

    #[test]
    fn a_bootloader_is_downloaded_then_written_only_with_enough_power() {
        use crate::{
            bootloader_update::BootloaderAsset,
            ota::{OtaCheckState, OtaUiRequest},
            power::PowerSnapshot,
        };

        let asset = BootloaderAsset {
            download_url: "https://example.com/x-bootloader.img".into(),
            sha256: [7; 32],
            size: 19_008,
        };
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::OtaUpdate);
        state.ota = OtaCheckState::BootloaderAvailable {
            release: "v1.5.0-beta.2".into(),
            installed: None,
            asset: asset.clone(),
        };
        // First SELECT: download and check only.
        state.apply(ButtonEvent::Select);
        assert_eq!(state.ota, OtaCheckState::PreparingBootloader);
        assert_eq!(
            state.take_ota_request(),
            Some(OtaUiRequest::PrepareBootloader {
                release: "v1.5.0-beta.2".into(),
                asset,
            })
        );
        // No second request while it downloads.
        state.apply(ButtonEvent::Select);
        assert_eq!(state.take_ota_request(), None);

        let ready = OtaCheckState::BootloaderReady {
            release: "v1.5.0-beta.2".into(),
            new: Some("v5.5.1, 2026-10-02".into()),
        };
        let battery = |percent: u8, external: bool| PowerSnapshot {
            battery_percent: Some(percent),
            battery_voltage_mv: Some(3_900),
            vbus_present: external,
            charging: false,
        };
        // Second SELECT on a low battery: refused, nothing requested, said
        // in the user's language.
        state.ota = ready.clone();
        state.board.power = Some(battery(30, false));
        state.regional.locale = crate::regional::Locale::Italian;
        state.apply(ButtonEvent::Select);
        assert_eq!(
            state.ota,
            OtaCheckState::BootloaderFailed(
                "Batteria al 30%: caricala almeno al 50% o collega il cavo USB.".into()
            )
        );
        assert_eq!(state.take_ota_request(), None);
        // After a copy that did not read back, SELECT writes it again.
        state.ota = OtaCheckState::BootloaderDamaged("readback".into());
        state.board.power = Some(battery(80, false));
        state.apply(ButtonEvent::Select);
        assert_eq!(state.ota, OtaCheckState::InstallingBootloader);
        assert_eq!(
            state.take_ota_request(),
            Some(OtaUiRequest::InstallBootloader)
        );
        // On the USB cable it goes ahead.
        state.ota = ready;
        state.board.power = Some(battery(30, true));
        state.apply(ButtonEvent::Select);
        assert_eq!(state.ota, OtaCheckState::InstallingBootloader);
        assert_eq!(
            state.take_ota_request(),
            Some(OtaUiRequest::InstallBootloader)
        );
    }

    #[test]
    fn the_update_channel_changes_only_with_select_on_its_row() {
        use crate::ota::{OtaCheckState, OtaUiRequest, UpdateChannel};

        let mut state = AppState::default();
        // A stable firmware starts on Stable, a beta one on Beta.
        let initial = state.ota_channel;
        assert_eq!(
            initial,
            UpdateChannel::of_version(crate::build_info::FIRMWARE_VERSION)
        );
        state.router.navigate_to(ScreenRoute::OtaUpdate);
        state.ota = OtaCheckState::UpdateAvailable {
            version: "v1.4.9".into(),
            download_url: "https://example.com/s.bin".into(),
        };
        // The rocker only moves between the action and the channel rows.
        state.apply(ButtonEvent::Down);
        assert_eq!(state.ota_action_selected, 1);
        assert_eq!(state.ota_channel, initial);
        assert_eq!(state.take_ota_request(), None);
        // SELECT on the channel row switches it and checks it; the update
        // found on the other channel is not offered any more.
        state.apply(ButtonEvent::Select);
        assert_eq!(state.ota_channel, initial.toggled());
        assert_eq!(state.ota, OtaCheckState::Checking);
        assert_eq!(state.take_ota_request(), Some(OtaUiRequest::CheckNow));
        assert_eq!(state.ota_action_selected, 0);
        // A check under way belongs to its channel: no switching meanwhile.
        state.apply(ButtonEvent::Up);
        assert_eq!(state.ota_action_selected, 0);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.ota_channel, initial.toggled());
        assert_eq!(state.take_ota_request(), None);
        state.update_ota_state(OtaCheckState::UpToDate);
        state.apply(ButtonEvent::Up);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.ota_channel, initial);
        assert_eq!(state.take_ota_request(), Some(OtaUiRequest::CheckNow));
    }

    /// The book-actions overlay open on a book whose file is not there, with
    /// "Delete Book" selected.
    fn state_on_delete_book() -> AppState {
        let mut state = AppState::default();
        state.reader.open_book_actions(crate::reader::ReaderBook {
            path: "/nowhere/BOOKS/Missing.txt".into(),
            title: "Missing".into(),
            format: crate::reader::BookFormat::Text,
            size_bytes: 10,
            modified_seconds: 0,
        });
        state.router.navigate_to(ScreenRoute::Library);
        state.router.navigate_to(ScreenRoute::LibraryBookActions);
        state.reader.book_actions_selected = crate::reader::LibraryBookAction::ALL
            .iter()
            .position(|action| *action == crate::reader::LibraryBookAction::Delete)
            .unwrap();
        state
    }

    #[test]
    fn deleting_a_book_takes_a_second_select_and_boot_only_cancels_it() {
        let mut state = state_on_delete_book();
        state.apply(ButtonEvent::Select);
        assert!(state.reader.book_delete_armed);
        assert_eq!(state.active_route(), ScreenRoute::LibraryBookActions);
        // BOOT cancels the pending delete and stays on the book's options.
        state.back();
        assert!(!state.reader.book_delete_armed);
        assert_eq!(state.active_route(), ScreenRoute::LibraryBookActions);
        // Moving off the row cancels it too.
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Up);
        assert!(!state.reader.book_delete_armed);
        // With nothing pending, BOOT leaves.
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Library);
    }

    #[test]
    fn a_book_that_cannot_be_deleted_keeps_its_options_open_with_the_reason() {
        let mut state = state_on_delete_book();
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::LibraryBookActions);
        assert!(state.reader.book_actions_error.is_some());
        assert!(!state.reader.book_delete_armed);
        assert!(state.reader.book_actions_target.is_some());
    }

    #[test]
    fn installing_an_update_takes_a_second_select() {
        use crate::ota::{OtaCheckState, OtaUiRequest};

        let available = OtaCheckState::UpdateAvailable {
            version: "v1.4.9".into(),
            download_url: "https://example.com/s.bin".into(),
        };
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::OtaUpdate);
        state.ota = available.clone();
        state.apply(ButtonEvent::Select);
        assert!(state.ota_install_armed);
        assert_eq!(state.ota, available);
        assert_eq!(state.take_ota_request(), None);
        // Moving the selection disarms it.
        state.apply(ButtonEvent::Down);
        assert!(!state.ota_install_armed);
        state.apply(ButtonEvent::Up);
        state.apply(ButtonEvent::Select);
        assert!(state.ota_install_armed);
        // So does leaving the screen.
        state.back();
        assert!(!state.ota_install_armed);
        state.router.navigate_to(ScreenRoute::OtaUpdate);
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.ota, OtaCheckState::Installing);
        assert_eq!(
            state.take_ota_request(),
            Some(OtaUiRequest::InstallNow {
                version: "v1.4.9".into(),
                download_url: "https://example.com/s.bin".into(),
            })
        );
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
        assert_ne!(state.display.font_size, original.font_size);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_ne!(state.display.sleep_screen, original.sleep_screen);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_ne!(state.display.auto_sleep, original.auto_sleep);
        state.apply(ButtonEvent::Down);
        assert_eq!(state.display_action_selected, 0);
        assert_eq!(state.active_route(), ScreenRoute::Display);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Settings);
    }

    #[test]
    fn wifi_transfer_stop_and_return_goes_to_upload_not_network() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Upload);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Upload);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::WifiTransfer);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Start)
        );
        state.update_wifi_transfer_snapshot(crate::wifi_transfer::WifiTransferSnapshot {
            state: crate::wifi_transfer::WifiTransferState::Ready,
            url: Some("http://192.168.1.2/".into()),
            last_action: "Portal ready".into(),
            last_bytes: 0,
            ..Default::default()
        });
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Upload);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Stop)
        );
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Home);
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

    /// A state connected to "Home", with "Home" and "Office" saved.
    fn state_with_two_saved_networks() -> AppState {
        let mut state = AppState::default();
        state.update_network_snapshot(crate::network::NetworkSnapshot {
            wifi_state: crate::network::WifiConnectionState::Connected,
            ssid: Some("Home".into()),
            saved_network_count: 2,
            ..crate::network::NetworkSnapshot::default()
        });
        // The list arrives without the connected mark: the live connection
        // sets it.
        state.set_saved_networks(two_saved_networks());
        state
    }

    fn two_saved_networks() -> Vec<crate::network_saved::SavedNetworkEntry> {
        vec![
            crate::network_saved::SavedNetworkEntry {
                ssid: "Home".into(),
                connected: false,
            },
            crate::network_saved::SavedNetworkEntry {
                ssid: "Office".into(),
                connected: false,
            },
        ]
    }

    #[test]
    fn network_saved_select_opens_a_menu_that_connects_or_forgets() {
        use crate::network_saved::SavedNetworkAction;

        let mut state = state_with_two_saved_networks();
        state.router.navigate_to(ScreenRoute::NetworkSaved);

        // The connected network offers no "Connect": SELECT, then SELECT on
        // the first row, forgets it.
        state.apply(ButtonEvent::Select);
        assert_eq!(
            state.network_saved.selected_menu_action(),
            Some(SavedNetworkAction::Forget)
        );
        assert_eq!(state.take_network_saved_forget_request(), None);
        state.apply(ButtonEvent::Select);
        assert!(state.network_saved.menu.is_none());
        assert_eq!(
            state.take_network_saved_forget_request(),
            Some("Home".into())
        );
        assert_eq!(state.take_network_join_request(), None);

        // Another network: the first row connects, and the Network screen
        // shows the attempt.
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(
            state.network_saved.selected_menu_action(),
            Some(SavedNetworkAction::Connect)
        );
        state.apply(ButtonEvent::Select);
        assert_eq!(state.take_network_join_request(), Some("Office".into()));
        assert_eq!(state.take_network_saved_forget_request(), None);
        assert_eq!(state.active_route(), ScreenRoute::Network);
    }

    #[test]
    fn network_saved_menu_moves_with_the_rocker_and_closes_on_cancel_or_boot() {
        use crate::network_saved::SavedNetworkAction;

        let mut state = state_with_two_saved_networks();
        state.router.navigate_to(ScreenRoute::NetworkSaved);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        // With the menu open the rocker moves inside it, not in the list.
        state.apply(ButtonEvent::Up);
        assert_eq!(
            state.network_saved.selected_menu_action(),
            Some(SavedNetworkAction::Cancel)
        );
        assert_eq!(
            state
                .network_saved
                .selected_entry()
                .map(|entry| entry.ssid.as_str()),
            Some("Office")
        );
        state.apply(ButtonEvent::Select);
        assert!(state.network_saved.menu.is_none());
        assert_eq!(state.take_network_join_request(), None);
        assert_eq!(state.take_network_saved_forget_request(), None);

        // BOOT closes the menu first, then leaves the list.
        state.apply(ButtonEvent::Select);
        state.back();
        assert!(state.network_saved.menu.is_none());
        assert_eq!(state.active_route(), ScreenRoute::NetworkSaved);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Network);
    }

    #[test]
    fn the_saved_network_marked_connected_is_the_one_the_device_is_on() {
        let mut state = state_with_two_saved_networks();
        assert!(state.network_saved.networks[0].connected);
        assert!(!state.network_saved.networks[1].connected);
        state.update_network_snapshot(crate::network::NetworkSnapshot {
            wifi_state: crate::network::WifiConnectionState::Connected,
            ssid: Some("Office".into()),
            ..crate::network::NetworkSnapshot::default()
        });
        assert!(!state.network_saved.networks[0].connected);
        assert!(state.network_saved.networks[1].connected);
        // While connecting, nothing is connected yet.
        state.update_network_snapshot(crate::network::NetworkSnapshot {
            wifi_state: crate::network::WifiConnectionState::Connecting,
            ssid: Some("Home".into()),
            ..crate::network::NetworkSnapshot::default()
        });
        assert!(state
            .network_saved
            .networks
            .iter()
            .all(|entry| !entry.connected));
    }

    #[test]
    fn network_saved_held_select_opens_the_menu_but_never_runs_an_action() {
        let mut state = state_with_two_saved_networks();
        state.router.navigate_to(ScreenRoute::NetworkSaved);
        assert!(state.apply_network_saved_select_long_press());
        assert!(state.network_saved.menu.is_some());
        // Held again with the menu open: still claimed, nothing forgotten.
        assert!(state.apply_network_saved_select_long_press());
        assert!(state.network_saved.menu.is_some());
        assert_eq!(state.take_network_saved_forget_request(), None);
        assert_eq!(state.take_network_join_request(), None);
    }

    fn network_snapshot(
        wifi_state: crate::network::WifiConnectionState,
        ssid: &str,
    ) -> crate::network::NetworkSnapshot {
        crate::network::NetworkSnapshot {
            wifi_state,
            ssid: Some(ssid.into()),
            saved_network_count: 2,
            ..crate::network::NetworkSnapshot::default()
        }
    }

    /// Choose "Connect" on the saved network "Office" and hand the request
    /// to the runtime, as main.rs does.
    fn connect_to_office(state: &mut AppState) {
        state.router.navigate_to(ScreenRoute::NetworkSaved);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.take_network_join_request(), Some("Office".into()));
    }

    #[test]
    fn a_failed_connect_goes_back_to_the_saved_list_and_says_so() {
        use crate::network::WifiConnectionState;

        let mut state = state_with_two_saved_networks();
        connect_to_office(&mut state);
        state.update_network_snapshot(network_snapshot(WifiConnectionState::Connecting, "Office"));
        assert!(!state.take_network_retry_request());
        assert_eq!(state.network_join_failed, None);

        state.update_network_snapshot(network_snapshot(WifiConnectionState::Failed, "Office"));
        assert_eq!(state.network_join_failed.as_deref(), Some("Office"));
        assert!(state.take_network_retry_request());
        // One recovery only: a reconnect that fails too is not retried
        // forever.
        state.update_network_snapshot(network_snapshot(WifiConnectionState::Failed, "Home"));
        assert!(!state.take_network_retry_request());
        // The note stays until the next attempt.
        state.update_network_snapshot(network_snapshot(WifiConnectionState::Connected, "Home"));
        assert_eq!(state.network_join_failed.as_deref(), Some("Office"));
        state.router.navigate_to(ScreenRoute::Network);
        state.network_action_selected = 2;
        state.apply(ButtonEvent::Select);
        assert_eq!(state.network_join_failed, None);
    }

    #[test]
    fn a_successful_connect_leaves_nothing_to_recover() {
        use crate::network::WifiConnectionState;

        let mut state = state_with_two_saved_networks();
        connect_to_office(&mut state);
        state.update_network_snapshot(network_snapshot(WifiConnectionState::Connecting, "Office"));
        state.update_network_snapshot(network_snapshot(WifiConnectionState::Connected, "Office"));
        assert_eq!(state.network_join_failed, None);
        // A later drop of that network is an ordinary failure, not a failed
        // "Connect".
        state.update_network_snapshot(network_snapshot(WifiConnectionState::Failed, "Office"));
        assert_eq!(state.network_join_failed, None);
        assert!(!state.take_network_retry_request());
    }

    #[test]
    fn the_connection_before_a_connect_is_not_taken_for_its_result() {
        use crate::network::WifiConnectionState;

        let mut state = state_with_two_saved_networks();
        state.router.navigate_to(ScreenRoute::NetworkSaved);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Select);
        // main.rs has not taken the request yet: this snapshot is still the
        // old connection.
        state.update_network_snapshot(network_snapshot(WifiConnectionState::Connected, "Home"));
        assert_eq!(state.take_network_join_request(), Some("Office".into()));
        state.update_network_snapshot(network_snapshot(WifiConnectionState::Failed, "Office"));
        assert_eq!(state.network_join_failed.as_deref(), Some("Office"));
    }

    #[test]
    fn network_saved_held_select_is_a_no_op_off_route() {
        let mut state = state_with_two_saved_networks();
        assert!(!state.apply_network_saved_select_long_press());
        assert!(state.network_saved.menu.is_none());
    }

    #[test]
    fn network_retry_is_requested_only_with_a_saved_network() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Network);
        state.network_action_selected = 2;
        state.apply(ButtonEvent::Select);
        assert!(!state.take_network_retry_request());
        state.network.saved_network_count = 2;
        state.apply(ButtonEvent::Select);
        assert!(state.take_network_retry_request());
        assert!(!state.take_network_retry_request());
        assert_eq!(state.active_route(), ScreenRoute::Network);
        state.network_action_selected = 3;
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::NetworkDetails);
    }

    #[test]
    fn the_portal_opened_from_network_returns_to_network() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Network);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::WifiTransfer);
        let _ = state.take_wifi_transfer_request();
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Network);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Stop)
        );
        // BOOT does the same.
        state.apply(ButtonEvent::Select);
        let _ = state.take_wifi_transfer_request();
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Network);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Stop)
        );
    }

    #[test]
    fn home_upload_tile_offers_wifi_and_usb_and_starts_nothing_by_itself() {
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Upload);
        state.apply(ButtonEvent::Select);
        // Only the chooser: the portal is not started until it is chosen.
        assert_eq!(state.active_route(), ScreenRoute::Upload);
        assert_eq!(state.upload_selected, 0);
        assert_eq!(state.take_wifi_transfer_request(), None);

        // Wi-Fi: the portal starts, and BOOT comes back to the chooser.
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::WifiTransfer);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Start)
        );
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Upload);
        assert_eq!(
            state.take_wifi_transfer_request(),
            Some(crate::wifi_transfer::WifiTransferUiRequest::Stop)
        );

        // USB cable: the "Connect to PC" screen, which asks for its own
        // SELECT before the card is handed to the computer.
        state.apply(ButtonEvent::Down);
        assert_eq!(state.upload_selected, 1);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::UsbDisk);
        assert!(!state.take_usb_disk_request());
        assert_eq!(state.take_wifi_transfer_request(), None);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Upload);
        // The rocker wraps between the two.
        state.apply(ButtonEvent::Down);
        assert_eq!(state.upload_selected, 0);
        state.apply(ButtonEvent::Up);
        assert_eq!(state.upload_selected, 1);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Home);
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

        state.apply(ButtonEvent::Select); // advance to Day
        state.apply(ButtonEvent::Up); // day + 1
        state.apply(ButtonEvent::Select); // advance to Month
        state.apply(ButtonEvent::Up); // month + 1
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
            crate::regional::TimeZoneProfile::EuropeRome
        );
        state.router.navigate_to(ScreenRoute::Clock);
        state.apply(ButtonEvent::Select); // open editor on the Timezone field
        state.apply(ButtonEvent::Up); // cycle away from Europe/Rome
        assert_ne!(
            state.clock_time_editor.unwrap().timezone,
            crate::regional::TimeZoneProfile::EuropeRome
        );
        for _ in 0..(ClockEditField::COUNT - 1) {
            state.apply(ButtonEvent::Select); // advance to Save
        }
        state.apply(ButtonEvent::Select); // commit
        assert_ne!(
            state.regional.timezone,
            crate::regional::TimeZoneProfile::EuropeRome
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
    fn clock_set_time_editor_boot_returns_to_the_previous_field_first() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Clock);
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Select); // Day
        state.apply(ButtonEvent::Select); // Month
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::ClockSetTime);
        assert_eq!(
            state.clock_time_editor.unwrap().selected_field(),
            ClockEditField::Day
        );
        state.back();
        assert_eq!(
            state.clock_time_editor.unwrap().selected_field(),
            ClockEditField::Timezone
        );
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Clock);
        assert!(state.clock_time_editor.is_none());
    }

    #[test]
    fn home_continue_reading_card_starts_selected_and_routes_straight_to_library() {
        // Continue Reading starts pre-selected (see `AppState::default`) —
        // it's the most likely first action on a fresh boot. SELECT on it
        // resumes (or, with nothing saved, falls through to Library)
        // immediately instead of stopping on the old intermediate Reader
        // category / summary screen, neither of which exists any more.
        let mut state = AppState::default();
        assert_eq!(
            state.home_selected,
            home_index(ScreenRoute::ContinueReading)
        );
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
        assert_eq!(
            state.home_selected,
            home_index(ScreenRoute::ContinueReading)
        );
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
    fn power_key_menu_restart_asks_the_runtime_to_restart_once() {
        let mut state = AppState::default();
        state.open_power_key_menu();
        state.apply(ButtonEvent::Down);
        assert!(!state.take_restart_request());
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Home);
        assert!(state.take_restart_request());
        assert!(!state.take_restart_request());
        assert!(!state.take_power_key_manual_refresh_request());
    }

    #[test]
    fn info_restores_the_settings_only_after_a_confirming_select() {
        use crate::app::display::{DisplayPreferences, UiFontSize};

        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::DeviceInfo);
        state.display.font_size = UiFontSize::Large;
        state.regional.locale = crate::regional::Locale::Italian;
        let changed = state.display;
        assert_ne!(changed, DisplayPreferences::default());

        // First row: the next page, as before.
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::DeviceInfoBoard);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::DeviceInfo);

        // Second row: one SELECT only arms, and BOOT or the rocker cancel.
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.settings_reset, SettingsResetStage::Armed);
        assert_eq!(state.display, changed);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::DeviceInfo);
        assert_eq!(state.settings_reset, SettingsResetStage::Idle);
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Up);
        assert_eq!(state.settings_reset, SettingsResetStage::Idle);
        assert_eq!(state.display, changed);

        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.settings_reset, SettingsResetStage::Done);
        assert_eq!(state.display, DisplayPreferences::default());
        // The language is not a setting this restores.
        assert_eq!(state.regional.locale, crate::regional::Locale::Italian);

        state.back();
        assert_ne!(state.active_route(), ScreenRoute::DeviceInfo);
        assert_eq!(state.info_selected, 0);
        assert_eq!(state.settings_reset, SettingsResetStage::Idle);
    }

    #[test]
    fn power_key_menu_preserves_return_route_and_requests_manual_refresh() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Library);
        state.open_power_key_menu();
        assert_eq!(state.active_route(), ScreenRoute::PowerKeyMenu);
        assert_eq!(state.power_key_sleep_restore_route(), ScreenRoute::Library);
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

    /// A state at the given first-run page, as main.rs leaves it at boot.
    fn first_run_at(page: crate::app::setup::SetupPage) -> AppState {
        let mut state = AppState::default();
        state.begin_first_run_setup(page.index());
        let _ = state.setup.take_progress();
        state
    }

    #[test]
    fn first_run_asks_the_language_then_walks_to_home() {
        use crate::{app::setup::SetupPage, first_run::SetupProgress, regional::Locale};

        let mut state = AppState::default();
        state.begin_first_run_setup(0);
        assert_eq!(state.active_route(), ScreenRoute::Setup);
        assert_eq!(state.setup.take_progress(), Some(SetupProgress::Page(0)));
        // English is under the cursor; nothing changes until SELECT.
        assert_eq!(state.regional.locale, Locale::Italian);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.regional.locale, Locale::English);
        assert!(state.setup.take_guide_request());
        assert_eq!(state.setup.page, SetupPage::Keys);
        // "Skip the setup" goes to the last page, and SELECT there to Home.
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.setup.page, SetupPage::Done);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Home);
        assert!(!state.setup.active);
        assert_eq!(state.setup.take_progress(), Some(SetupProgress::Done));
    }

    #[test]
    fn screens_opened_from_the_first_run_pages_come_back_to_them() {
        use crate::app::setup::SetupPage;

        // The phone portal, closed with SELECT and with BOOT.
        for close_with_boot in [false, true] {
            let mut state = first_run_at(SetupPage::Wifi);
            state.apply(ButtonEvent::Select);
            assert_eq!(state.active_route(), ScreenRoute::WifiTransfer);
            assert_eq!(
                state.take_wifi_transfer_request(),
                Some(crate::wifi_transfer::WifiTransferUiRequest::Start)
            );
            if close_with_boot {
                state.back();
            } else {
                state.apply(ButtonEvent::Select);
            }
            assert_eq!(state.active_route(), ScreenRoute::Setup);
            assert_eq!(
                (state.setup.page, state.setup.selected),
                (SetupPage::Wifi, 1)
            );
        }

        // The date and time editor, left with BOOT from its first field.
        let mut state = first_run_at(SetupPage::Clock);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::ClockSetTime);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Setup);
        assert_eq!(state.setup.page, SetupPage::Clock);
        // And saved: every field confirmed down to "Save".
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        for _ in 0..12 {
            if state.active_route() != ScreenRoute::ClockSetTime {
                break;
            }
            state.apply(ButtonEvent::Select);
        }
        assert_eq!(state.active_route(), ScreenRoute::Setup);
        assert!(state.take_clock_set_time_request().is_some());

        // "Connect to PC", left with BOOT before it starts.
        let mut state = first_run_at(SetupPage::Book);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::UsbDisk);
        assert_eq!(
            state.setup.take_progress(),
            Some(crate::first_run::SetupProgress::Page(
                SetupPage::Done.index()
            ))
        );
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Setup);
        assert_eq!(
            state.setup.take_progress(),
            Some(crate::first_run::SetupProgress::Page(
                SetupPage::Book.index()
            ))
        );
    }

    #[test]
    fn outside_the_first_run_pages_the_same_screens_go_where_they_always_did() {
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Clock);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::ClockSetTime);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Clock);
        state.router.navigate_to(ScreenRoute::UsbDisk);
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Upload);
    }

    #[test]
    fn first_steps_reopen_from_settings_and_boot_leaves_them() {
        use crate::{app::setup::SetupPage, regional::Locale};

        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Settings);
        let index = state
            .category_usage
            .ordered_entries(ScreenRoute::Settings)
            .iter()
            .position(|entry| entry.route == ScreenRoute::Setup)
            .expect("First steps is a Settings tile");
        for _ in 0..index {
            state.apply(ButtonEvent::Down);
        }
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Setup);
        // The language in use is the one under the cursor.
        assert_eq!(
            (state.setup.page, state.setup.selected),
            (SetupPage::Language, 1)
        );
        state.apply(ButtonEvent::Select);
        assert_eq!(state.regional.locale, Locale::Italian);
        assert!(!state.setup.take_guide_request());
        assert_eq!(state.setup.take_progress(), None);
        state.back();
        assert_eq!(
            (state.setup.page, state.setup.selected),
            (SetupPage::Language, 1)
        );
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Settings);
        assert!(!state.setup.active);
    }

    #[test]
    fn the_card_warning_restarts_or_goes_on_without_the_card() {
        let mut state = AppState::default();
        state.show_card_warning();
        assert_eq!(state.active_route(), ScreenRoute::CardWarning);
        state.apply(ButtonEvent::Select);
        assert!(state.take_restart_request());

        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Home);
        assert!(!state.take_restart_request());

        state.show_card_warning();
        state.back();
        assert_eq!(state.active_route(), ScreenRoute::Home);
    }

    #[test]
    fn empty_lists_send_select_to_upload() {
        use crate::reader::ReaderUiState;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let books = std::env::temp_dir().join(format!("rustmix-empty-books-{nanos}"));
        std::fs::create_dir_all(&books).unwrap();

        // Home's Continue card, with no book to resume and none on the card.
        let mut state = AppState {
            reader: ReaderUiState::with_books_root(books.to_string_lossy()),
            ..AppState::default()
        };
        state.apply(ButtonEvent::Select);
        assert!(state.library_known_empty);
        assert_eq!(state.active_route(), ScreenRoute::Upload);
        // Opening an upload drops what was known about the folder.
        state.apply(ButtonEvent::Select);
        assert!(!state.library_known_empty);

        // The Library itself, empty.
        let mut state = AppState {
            reader: ReaderUiState::with_books_root(books.to_string_lossy()),
            ..AppState::default()
        };
        state.home_selected = home_index(ScreenRoute::Library);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Library);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Upload);

        // A folder that cannot be read is not "empty": SELECT does nothing.
        std::fs::remove_dir_all(&books).unwrap();
        let mut state = AppState {
            reader: ReaderUiState::with_books_root(books.to_string_lossy()),
            ..AppState::default()
        };
        state.home_selected = home_index(ScreenRoute::Library);
        state.apply(ButtonEvent::Select);
        assert!(!state.library_known_empty);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Library);

        // Audiobooks, empty.
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::AudiobookLibrary);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Upload);
        let mut state = AppState::default();
        state.audiobooks.set_library(Err("no card".into()));
        state.router.navigate_to(ScreenRoute::AudiobookLibrary);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::AudiobookLibrary);
    }
}
