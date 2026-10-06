//! Optional Wi-Fi and SNTP runtime with hardware-independent snapshots.

use crate::{
    network_config::{NetworkConfig, WIFI_CONFIG_PATH},
    regional::Locale,
    rtc::RtcDateTime,
};

/// Product-facing Wi-Fi state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WifiConnectionState {
    Disabled,
    #[default]
    ConfigurationMissing,
    Connecting,
    Connected,
    Failed,
}

impl WifiConnectionState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Disabled => "DISABLED",
            Self::ConfigurationMissing => "NO CONFIG",
            Self::Connecting => "CONNECTING",
            Self::Connected => "CONNECTED",
            Self::Failed => "FAILED",
        }
    }

    /// Locale-aware sibling of [`Self::label`]. `label` itself is left
    /// untouched because `src/main.rs`'s serial diagnostics logging depends
    /// on its English output staying stable.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::Disabled => "DISABILITATO",
                Self::ConfigurationMissing => "CONFIG MANCANTE",
                Self::Connecting => "CONNESSIONE",
                Self::Connected => "CONNESSO",
                Self::Failed => "FALLITO",
            },
        }
    }

    /// The state as a sentence for the Network screen, where
    /// [`Self::label_i18n`]'s terse upper-case tags read as error codes.
    #[must_use]
    pub const fn status_text(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => match self {
                Self::Disabled => "Wi-Fi off",
                Self::ConfigurationMissing => "No saved network",
                Self::Connecting => "Connecting...",
                Self::Connected => "Connected",
                Self::Failed => "Not connected",
            },
            Locale::Italian => match self {
                Self::Disabled => "Wi-Fi spento",
                Self::ConfigurationMissing => "Nessuna rete salvata",
                Self::Connecting => "Connessione in corso...",
                Self::Connected => "Connesso",
                Self::Failed => "Non connesso",
            },
        }
    }
}

/// Product-facing SNTP synchronization state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NtpSyncState {
    Disabled,
    #[default]
    WaitingForWifi,
    Synchronizing,
    Synchronized,
    Failed,
}

impl NtpSyncState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Disabled => "DISABLED",
            Self::WaitingForWifi => "WAIT WIFI",
            Self::Synchronizing => "SYNCING",
            Self::Synchronized => "SYNCED",
            Self::Failed => "FAILED",
        }
    }
}

/// Serial-log fingerprint. RSSI is intentionally excluded so signal-strength
/// churn is reported only by the bounded heartbeat marker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkLogFingerprint {
    pub wifi_state: WifiConnectionState,
    pub ntp_state: NtpSyncState,
    pub ssid: Option<String>,
    pub ipv4_address: Option<String>,
    pub last_sync_utc: Option<RtcDateTime>,
    pub error: Option<String>,
}

/// Rendering snapshot that never contains the Wi-Fi password.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkSnapshot {
    pub wifi_state: WifiConnectionState,
    pub ntp_state: NtpSyncState,
    pub ssid: Option<String>,
    pub ipv4_address: Option<String>,
    pub rssi_dbm: Option<i32>,
    pub timezone_name: String,
    pub ntp_server: String,
    pub last_sync_utc: Option<RtcDateTime>,
    pub error: Option<String>,
    /// Count of networks in the currently loaded `WIFI.TXT`, for the
    /// Network screen's "N saved" summary. Not part of the log fingerprint
    /// or connection state since it never affects behavior, only display.
    pub saved_network_count: usize,
}

impl Default for NetworkSnapshot {
    fn default() -> Self {
        Self {
            wifi_state: WifiConnectionState::ConfigurationMissing,
            ntp_state: NtpSyncState::WaitingForWifi,
            ssid: None,
            ipv4_address: None,
            rssi_dbm: None,
            timezone_name: crate::network_config::DEFAULT_TIMEZONE.into(),
            ntp_server: "pool.ntp.org".into(),
            last_sync_utc: None,
            error: None,
            saved_network_count: 0,
        }
    }
}

impl NetworkSnapshot {
    /// Render a provisioned-but-not-yet-connected boot state before Wi-Fi is
    /// started after the first e-paper frame. Reports the first candidate in
    /// boot fail-over order; `NetworkRuntime::tick` updates the SSID as it
    /// advances through the saved list.
    #[must_use]
    pub fn provisioned(config: &NetworkConfig) -> Self {
        Self {
            wifi_state: WifiConnectionState::Connecting,
            ntp_state: NtpSyncState::WaitingForWifi,
            ssid: config.networks.first().map(|network| network.ssid.clone()),
            timezone_name: config.timezone.clone(),
            ntp_server: config.ntp_server.clone(),
            saved_network_count: config.networks.len(),
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn home_badge(&self) -> &'static str {
        match (self.wifi_state, self.ntp_state) {
            (WifiConnectionState::Connected, NtpSyncState::Synchronized) => "NTP OK",
            (WifiConnectionState::Connected, _) => "WIFI OK",
            (WifiConnectionState::ConfigurationMissing, _) => "NO CFG",
            (WifiConnectionState::Connecting, _) => "WAIT",
            (WifiConnectionState::Disabled, _) => "OFF",
            (WifiConnectionState::Failed, _) => "FAILED",
        }
    }

    #[must_use]
    pub fn ssid_label(&self) -> &str {
        self.ssid.as_deref().unwrap_or("--")
    }

    #[must_use]
    pub fn ipv4_label(&self) -> &str {
        self.ipv4_address.as_deref().unwrap_or("--")
    }

    #[must_use]
    pub fn rssi_label(&self) -> String {
        self.rssi_dbm
            .map_or_else(|| "--".into(), |value| format!("{value} dBm"))
    }

    #[must_use]
    pub fn last_sync_label(&self) -> String {
        self.last_sync_utc.map_or_else(
            || "not synchronized".into(),
            |value| format!("{} UTC", value.date_time()),
        )
    }

    #[must_use]
    pub const fn config_path() -> &'static str {
        WIFI_CONFIG_PATH
    }

    /// Build a concise fingerprint for serial-marker rate limiting.
    #[must_use]
    pub fn log_fingerprint(&self) -> NetworkLogFingerprint {
        NetworkLogFingerprint {
            wifi_state: self.wifi_state,
            ntp_state: self.ntp_state,
            ssid: self.ssid.clone(),
            ipv4_address: self.ipv4_address.clone(),
            last_sync_utc: self.last_sync_utc,
            error: self.error.clone(),
        }
    }
}

/// The hotspot's own address: gateway, DNS server and portal in one.
///
/// Deliberately outside the private ranges. The hotspot answers every DNS
/// query with this address so that a phone's connectivity check lands on
/// the portal; Android, and Samsung's One UI in particular, does not treat
/// a network as "sign in required" when that answer is a private address
/// (192.168.x.x, as ESP-IDF's default 192.168.71.1 was): it files the
/// network under "no Internet" and never offers the page. 4.3.2.1 is the
/// address ESP32 captive portals commonly use for this reason. Nothing is
/// routed beyond the hotspot, so the address only has to look public.
pub const PROVISIONING_AP_IP: &str = "4.3.2.1";
/// The hotspot's subnet mask (a /24 around [`PROVISIONING_AP_IP`]).
pub const PROVISIONING_AP_NETMASK: &str = "255.255.255.0";

/// The device's own hotspot, brought up by [`espidf::NetworkRuntime::start_provisioning`]
/// so a phone can join it and reach the provisioning portal. Freshly generated
/// every time provisioning starts. `portal_ip` is [`PROVISIONING_AP_IP`],
/// set on the SoftAP netif before the hotspot starts, so it never needs to
/// be polled for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisioningApInfo {
    pub ap_ssid: String,
    pub ap_password: String,
    pub portal_ip: String,
}

#[cfg(target_os = "espidf")]
pub mod espidf {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use anyhow::{anyhow, Context, Result};
    use embedded_svc::wifi::{
        AccessPointConfiguration, AuthMethod, ClientConfiguration, Configuration,
    };
    use esp_idf_svc::{
        eventloop::EspSystemEventLoop,
        hal::modem::WifiModemPeripheral,
        handle::RawHandle,
        nvs::EspDefaultNvsPartition,
        sntp::{EspSntp, SntpConf},
        sys,
        wifi::{BlockingWifi, EspWifi},
    };
    use log::warn;

    use crate::{
        network::{NetworkSnapshot, NtpSyncState, ProvisioningApInfo, WifiConnectionState},
        network_config::{NetworkConfig, SavedNetwork},
        network_scan::WifiScanEntry,
        ntp::{utc_from_unix_seconds, MIN_VALID_SNTP_UNIX_SECONDS},
        rtc::RtcDateTime,
    };

    /// Ceiling on the non-blocking association handshake driven by
    /// [`NetworkRuntime::advance_boot_phase`], applied per saved-network
    /// candidate. Mirrors the timeout the previous blocking `wait_netif_up`
    /// implicitly enforced, so a misconfigured or unreachable access point
    /// still resolves to the next candidate (or `WifiConnectionState::Failed`
    /// once the list is exhausted) in a bounded time instead of leaving the
    /// UI stuck on "Connecting" forever.
    const WIFI_BOOT_TIMEOUT: Duration = Duration::from_secs(15);
    /// Settle window before a non-blocking scan's results are read back.
    /// `esp_wifi_scan_start` does not expose a simple non-blocking
    /// completion poll through the safe wrapper used here, so
    /// [`NetworkRuntime::poll_scan`] just waits out this bounded window
    /// (long enough for a full active scan across all 2.4 GHz channels)
    /// before fetching results, without blocking the main loop in between.
    const WIFI_SCAN_TIMEOUT: Duration = Duration::from_secs(4);
    /// Bound the scan result buffer to protect the main-task stack/heap.
    const WIFI_SCAN_MAX_RESULTS: usize = 24;
    /// Minimum gap between a candidate that failed *immediately* (a
    /// synchronous association error, not an association that timed out --
    /// see `WifiBootPhase::CandidateCooldown`) and the next saved network's
    /// `connect()` attempt, so several saved networks that are all
    /// unreachable don't hammer the radio with back-to-back connect calls.
    const CANDIDATE_RETRY_COOLDOWN: Duration = Duration::from_millis(300);
    use super::{PROVISIONING_AP_IP, PROVISIONING_AP_NETMASK};

    /// Bound how many stations may join the provisioning hotspot at once.
    const PROVISIONING_AP_MAX_CONNECTIONS: u16 = 4;

    /// Drop Wi-Fi into its most aggressive modem-sleep once associated for
    /// background use (periodic NTP sync only): ESP-IDF's own
    /// default, `WIFI_PS_MIN_MODEM`, still wakes the radio every DTIM
    /// interval, far more often than this firmware's background sync
    /// cadence needs. [`crate::wifi_transfer::espidf::WifiTransferServer`]
    /// overrides this to `WIFI_PS_NONE` for the duration of an active
    /// file-transfer session and restores it on drop.
    fn apply_background_power_save() {
        let status = unsafe { sys::esp_wifi_set_ps(sys::wifi_ps_type_t_WIFI_PS_MAX_MODEM) };
        if status != sys::ESP_OK {
            warn!("rustmix-wave=wifi-power-save status=failed mode=max-modem error-code={status}");
        }
    }

    /// Tracks the non-blocking Wi-Fi association handshake kicked off by
    /// [`NetworkRuntime::connect`] and [`NetworkRuntime::try_join_candidate`].
    /// The first e-paper frame and the button-polling main loop must never
    /// wait on association or DHCP, so both callers only request the driver
    /// start/connect and return immediately; `tick` then advances this phase
    /// by one driver-state check per main-loop iteration instead of blocking
    /// on it.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum WifiBootPhase {
        /// Waiting for the boot-time scan kicked off by [`NetworkRuntime::connect`]
        /// to settle, so `candidates` can be reordered to try any saved
        /// network the scan actually saw in range before ones it didn't,
        /// strongest (closest) signal first.
        Scanning,
        /// Waiting for `esp_wifi_start()` to finish bringing the driver up.
        Starting,
        /// Driver started and `esp_wifi_connect()` requested; waiting for
        /// association and DHCP to complete.
        Connecting,
        /// A `Connecting`-phase candidate failed *immediately* (a synchronous
        /// association error rather than a timeout) and another saved
        /// network is queued; waiting out `CANDIDATE_RETRY_COOLDOWN` before
        /// issuing its `connect()` instead of doing so on the same tick.
        /// Never blocks: `tick` just no-ops here until the cooldown elapses,
        /// same as every other phase. An association that times out instead
        /// already has a natural multi-second gap from `WIFI_BOOT_TIMEOUT`
        /// and moves to the next candidate immediately, skipping this phase.
        CandidateCooldown,
    }

    /// Own Wi-Fi and SNTP services for as long as the firmware is running.
    pub struct NetworkRuntime {
        wifi: Option<BlockingWifi<EspWifi<'static>>>,
        sntp: Option<EspSntp<'static>>,
        snapshot: NetworkSnapshot,
        ntp_reported: bool,
        suspended: bool,
        boot_phase: Option<WifiBootPhase>,
        boot_started_at: Option<Instant>,
        /// Set while `boot_phase` is `CandidateCooldown`, consumed by
        /// `advance_boot_phase` once `CANDIDATE_RETRY_COOLDOWN` elapses.
        candidate_cooldown_started_at: Option<Instant>,
        /// Whether `esp_wifi_start()` has been requested on `wifi` yet. Set
        /// eagerly by [`Self::connect`]/[`Self::start_provisioning`]; left
        /// `false` by [`Self::provision`] so a driver held only for
        /// scanning/first-time setup does not draw radio power until the
        /// user actually opens Wi-Fi setup.
        driver_started: bool,
        /// Set while a non-blocking scan requested by [`Self::start_scan`] is
        /// settling, consumed by [`Self::poll_scan`].
        scan_started_at: Option<Instant>,
        /// Saved networks being attempted, in boot/resume fail-over order,
        /// or the single candidate under test by
        /// [`Self::try_join_candidate`]. `candidate_index` is the one
        /// currently in flight; `advance_boot_phase` moves to the next entry
        /// on failure instead of giving up immediately.
        candidates: Vec<SavedNetwork>,
        candidate_index: usize,
        /// SSID of the last saved network that actually associated (got an
        /// IP), as opposed to `candidates[candidate_index]`, which is merely
        /// the last one *attempted* -- possibly one that was failing or out
        /// of range when Wi-Fi was suspended. [`Self::resume`] prefers this.
        connected_ssid: Option<String>,
        /// `true` while `Configuration::Mixed` (AP + STA) is active for the
        /// phone provisioning portal.
        provisioning: bool,
        /// DHCP Option 114 (RFC 8910) captive-portal URI advertised by
        /// [`Self::start_provisioning`]. `esp_netif_dhcps_option` stores the
        /// raw pointer it is given rather than copying the string, reading
        /// it fresh via `strlen` for every DHCP offer/ack for as long as the
        /// hotspot runs -- so this must outlive the DHCP server, which this
        /// field, kept for the life of the runtime, guarantees.
        captive_portal_uri: Option<std::ffi::CString>,
    }

    impl NetworkRuntime {
        #[must_use]
        pub fn configuration_missing() -> Self {
            Self {
                wifi: None,
                sntp: None,
                snapshot: NetworkSnapshot::default(),
                ntp_reported: false,
                suspended: false,
                boot_phase: None,
                boot_started_at: None,
                candidate_cooldown_started_at: None,
                driver_started: false,
                scan_started_at: None,
                candidates: Vec::new(),
                candidate_index: 0,
                connected_ssid: None,
                provisioning: false,
                captive_portal_uri: None,
            }
        }

        /// Bring up a Wi-Fi driver without any saved credentials and without
        /// starting the radio, so Wi-Fi setup can scan and provision on a
        /// first-ever boot (no `WIFI.TXT` yet) without requiring a reboot.
        /// The radio itself only powers up once [`Self::start_scan`] or
        /// [`Self::start_provisioning`] is actually called.
        pub fn provision<M>(modem: M) -> Result<Self>
        where
            M: WifiModemPeripheral + 'static,
        {
            let sys_loop = EspSystemEventLoop::take()?;
            let nvs = EspDefaultNvsPartition::take()?;
            let wifi =
                BlockingWifi::wrap(EspWifi::new(modem, sys_loop.clone(), Some(nvs))?, sys_loop)?;
            Ok(Self {
                wifi: Some(wifi),
                sntp: None,
                snapshot: NetworkSnapshot::default(),
                ntp_reported: false,
                suspended: false,
                boot_phase: None,
                boot_started_at: None,
                candidate_cooldown_started_at: None,
                driver_started: false,
                scan_started_at: None,
                candidates: Vec::new(),
                candidate_index: 0,
                connected_ssid: None,
                provisioning: false,
                captive_portal_uri: None,
            })
        }

        #[must_use]
        pub fn failed(config: &NetworkConfig, error: impl Into<String>) -> Self {
            Self {
                wifi: None,
                sntp: None,
                snapshot: NetworkSnapshot {
                    wifi_state: WifiConnectionState::Failed,
                    ntp_state: NtpSyncState::Failed,
                    ssid: config.networks.first().map(|network| network.ssid.clone()),
                    timezone_name: config.timezone.clone(),
                    ntp_server: config.ntp_server.clone(),
                    error: Some(error.into()),
                    saved_network_count: config.networks.len(),
                    ..NetworkSnapshot::default()
                },
                ntp_reported: false,
                suspended: false,
                boot_phase: None,
                boot_started_at: None,
                candidate_cooldown_started_at: None,
                driver_started: false,
                scan_started_at: None,
                candidates: Vec::new(),
                candidate_index: 0,
                connected_ssid: None,
                provisioning: false,
                captive_portal_uri: None,
            }
        }

        /// Request Wi-Fi start after the initial e-paper frame is already
        /// visible, without waiting for association or DHCP. Returns as soon
        /// as the start command is queued so the caller can enter the
        /// button-polling main loop immediately; `tick` completes the
        /// handshake in the background, failing over through
        /// `config.networks` in order until one succeeds or the list is
        /// exhausted.
        pub fn connect<M>(modem: M, config: &NetworkConfig) -> Result<Self>
        where
            M: WifiModemPeripheral + 'static,
        {
            let nvs_span = crate::boot_profile::span("wifi-event-loop-nvs-take");
            let sys_loop = EspSystemEventLoop::take()?;
            let nvs = EspDefaultNvsPartition::take()?;
            nvs_span.end();
            let driver_span = crate::boot_profile::span("wifi-driver-new");
            let mut wifi =
                BlockingWifi::wrap(EspWifi::new(modem, sys_loop.clone(), Some(nvs))?, sys_loop)?;
            driver_span.end();
            let first = config
                .networks
                .first()
                .context("at least one saved network is required")?;
            // Non-blocking: `EspWifi::start` (reached through `wifi_mut`,
            // bypassing `BlockingWifi`'s waiting wrapper) only queues
            // `esp_wifi_start()` and returns. `tick`/`advance_boot_phase`
            // observes the driver reaching "started" and then requests
            // `esp_wifi_connect()` itself.
            let start_span = crate::boot_profile::span("wifi-start");
            wifi.wifi_mut().start()?;
            start_span.end();
            // Scan before committing to a candidate: with more than one
            // saved network, blindly trying `WIFI.TXT`'s order wastes the
            // full `WIFI_BOOT_TIMEOUT` on an out-of-range network before
            // ever reaching one that would have associated immediately.
            // `advance_boot_phase`'s `Scanning` arm reorders `candidates`
            // once the scan settles, trying any saved SSID actually seen in
            // range first, and only then requests the real connection. With
            // a single saved network, or if the scan fails to start, connect
            // in saved order, starting from `first` (see
            // `start_candidate_scan`).
            let scan_started = start_candidate_scan(config.networks.len(), "wifi-boot-scan");
            if !scan_started {
                wifi.set_configuration(&Configuration::Client(client_configuration(first)?))?;
            }

            Ok(Self {
                wifi: Some(wifi),
                sntp: None,
                snapshot: NetworkSnapshot {
                    wifi_state: WifiConnectionState::Connecting,
                    ntp_state: NtpSyncState::WaitingForWifi,
                    ssid: Some(first.ssid.clone()),
                    ipv4_address: None,
                    rssi_dbm: None,
                    timezone_name: config.timezone.clone(),
                    ntp_server: config.ntp_server.clone(),
                    last_sync_utc: None,
                    error: None,
                    saved_network_count: config.networks.len(),
                },
                ntp_reported: false,
                suspended: false,
                boot_phase: Some(if scan_started {
                    WifiBootPhase::Scanning
                } else {
                    WifiBootPhase::Starting
                }),
                boot_started_at: Some(Instant::now()),
                candidate_cooldown_started_at: None,
                driver_started: true,
                scan_started_at: scan_started.then(Instant::now),
                candidates: config.networks.clone(),
                candidate_index: 0,
                connected_ssid: None,
                provisioning: false,
                captive_portal_uri: None,
            })
        }

        #[must_use]
        pub fn snapshot(&self) -> NetworkSnapshot {
            self.snapshot.clone()
        }

        #[must_use]
        pub const fn is_suspended(&self) -> bool {
            self.suspended
        }

        #[must_use]
        pub const fn is_provisioning(&self) -> bool {
            self.provisioning
        }

        /// Stop optional network services while retaining station ownership so
        /// a later power-key wake can reconnect without rebuilding
        /// the complete application shell.
        pub fn suspend(&mut self) -> Result<()> {
            let _ = self.sntp.take();
            if let Some(wifi) = self.wifi.as_mut() {
                let _ = wifi.disconnect();
                wifi.stop()?;
            }
            self.snapshot.wifi_state = WifiConnectionState::Disabled;
            self.snapshot.ntp_state = NtpSyncState::Disabled;
            self.snapshot.ipv4_address = None;
            self.snapshot.rssi_dbm = None;
            self.snapshot.error = None;
            self.ntp_reported = false;
            self.suspended = true;
            self.boot_phase = None;
            self.boot_started_at = None;
            self.candidate_cooldown_started_at = None;
            Ok(())
        }

        /// Restart Wi-Fi association and SNTP after the wake frame is already
        /// visible. Non-blocking, same shape as [`Self::connect`]/
        /// [`Self::stop_provisioning`]: this only requests
        /// `esp_wifi_start()`, applies the first candidate's configuration
        /// and arms a `WifiBootPhase::Starting` handshake for
        /// `tick`/`advance_boot_phase` to carry to completion across
        /// subsequent main-loop iterations. An earlier version blocked here
        /// on `wifi.connect()` and DHCP directly, freezing button/panel
        /// handling for however long association and DHCP took -- several
        /// seconds even to an in-range AP, more than twenty if a saved
        /// network ahead of it in `config.networks` was out of range -- since
        /// this runs inline in the main loop, unlike `connect`'s boot-time
        /// callers which are naturally idle. `advance_boot_phase`'s existing
        /// fail-over already tries the rest of `config.networks` in order if
        /// the first candidate fails, so failure here stays non-fatal and
        /// visible in the product-facing network snapshot exactly as before.
        ///
        /// Like [`Self::connect`], this scans first and lets the `Scanning`
        /// phase try saved networks actually seen in range, strongest first,
        /// instead of blindly re-trying whatever was last attempted: that
        /// could be a saved network that is switched off or out of range
        /// (a phone hotspot, say), costing the full `WIFI_BOOT_TIMEOUT`
        /// before an in-range one got its turn. The network last actually
        /// connected (`connected_ssid`) is placed first in the saved order,
        /// which only decides the order among networks the scan did *not*
        /// see, and the fallback order if the scan cannot start.
        pub fn resume(&mut self, config: &NetworkConfig) -> Result<()> {
            self.suspended = false;
            self.boot_phase = None;
            self.boot_started_at = None;
            self.candidate_cooldown_started_at = None;
            self.sntp = None;
            let preferred_ssid = self.connected_ssid.clone();
            self.candidates = config.networks.clone();
            self.candidate_index = 0;
            if let Some(ssid) = preferred_ssid {
                if let Some(position) = self
                    .candidates
                    .iter()
                    .position(|network| network.ssid == ssid)
                {
                    let preferred = self.candidates.remove(position);
                    self.candidates.insert(0, preferred);
                }
            }
            let first = self
                .candidates
                .first()
                .cloned()
                .context("at least one saved network is required")?;
            let wifi = self.wifi.as_mut().context("Wi-Fi runtime is unavailable")?;
            wifi.wifi_mut().start()?;
            let scan_started = start_candidate_scan(self.candidates.len(), "wifi-resume-scan");
            if !scan_started {
                wifi.set_configuration(&Configuration::Client(client_configuration(&first)?))?;
            }
            self.snapshot.wifi_state = WifiConnectionState::Connecting;
            self.snapshot.ntp_state = NtpSyncState::WaitingForWifi;
            self.snapshot.ssid = Some(first.ssid);
            self.snapshot.ipv4_address = None;
            self.snapshot.rssi_dbm = None;
            self.snapshot.timezone_name = config.timezone.clone();
            self.snapshot.ntp_server = config.ntp_server.clone();
            self.snapshot.saved_network_count = config.networks.len();
            self.snapshot.error = None;
            self.ntp_reported = false;
            self.boot_phase = Some(if scan_started {
                WifiBootPhase::Scanning
            } else {
                WifiBootPhase::Starting
            });
            self.boot_started_at = Some(Instant::now());
            Ok(())
        }

        /// Number of phones currently associated to the provisioning
        /// hotspot. AP+STA `Configuration::Mixed` shares a single radio, so
        /// an active scan (used to refresh the "nearby networks" list)
        /// briefly leaves the AP's operating channel to visit others; any
        /// phone already joined loses the link for that window, which can
        /// reset its in-flight HTTP requests, including the very
        /// captive-portal probe this portal depends on to auto-open.
        /// Callers should skip periodic rescans while this is non-zero, so a
        /// phone that already joined gets a clean shot at that probe instead
        /// of the connection being pulled out from under it. Returns 0 (and
        /// so never blocks a caller from proceeding) if the driver call
        /// fails; the operating error, if any, was already reported when the
        /// hotspot started.
        #[must_use]
        pub fn provisioning_client_count(&self) -> u16 {
            let mut sta_list = unsafe { core::mem::zeroed::<sys::wifi_sta_list_t>() };
            let status = unsafe { sys::esp_wifi_ap_get_sta_list(&mut sta_list) };
            if status == sys::ESP_OK {
                sta_list.num as u16
            } else {
                0
            }
        }

        /// Kick off a non-blocking Wi-Fi scan for the phone provisioning
        /// portal's "nearby networks" list. Starts the radio first if it was
        /// only ever `provision()`-ed (never connected). Results are read
        /// back by [`Self::poll_scan`] once the settle window elapses.
        pub fn start_scan(&mut self) -> Result<()> {
            let wifi = self.wifi.as_mut().context("Wi-Fi driver is unavailable")?;
            if !self.driver_started {
                wifi.wifi_mut().start()?;
                self.driver_started = true;
            }
            let status = unsafe { sys::esp_wifi_scan_start(core::ptr::null(), false) };
            if status != sys::ESP_OK {
                return Err(anyhow!("esp_wifi_scan_start failed: {status}"));
            }
            self.scan_started_at = Some(Instant::now());
            Ok(())
        }

        /// Poll once per main-loop iteration for a settled scan. Returns
        /// `None` while no scan is in flight or its settle window has not
        /// yet elapsed; never blocks.
        pub fn poll_scan(&mut self) -> Option<std::result::Result<Vec<WifiScanEntry>, String>> {
            let started_at = self.scan_started_at?;
            if started_at.elapsed() < WIFI_SCAN_TIMEOUT {
                return None;
            }
            self.scan_started_at = None;
            Some(fetch_scan_results())
        }

        /// Switch the driver into `Configuration::Mixed` (AP + STA), so the
        /// phone provisioning portal can broadcast a joinable hotspot while
        /// still scanning nearby networks over the STA side. The STA side
        /// starts idle (no target configured) until
        /// [`Self::try_join_candidate`] is called. Non-blocking: the AP
        /// gateway address is ESP-IDF's fixed SoftAP default, so it is
        /// reported immediately rather than polled for.
        pub fn start_provisioning(&mut self) -> Result<ProvisioningApInfo> {
            self.sntp = None;
            self.boot_phase = None;
            self.boot_started_at = None;
            self.candidate_cooldown_started_at = None;
            self.candidates.clear();
            self.candidate_index = 0;
            let ap_ssid = format!("RUSTMIX-{:04}", unsafe { sys::esp_random() } % 10_000);
            let ap_password = generate_ap_password();
            let wifi = self.wifi.as_mut().context("Wi-Fi driver is unavailable")?;
            let _ = wifi.disconnect();

            // Three things about the hotspot's own netif are not ESP-IDF's
            // SoftAP defaults, and all three are set here on the existing AP
            // netif rather than on a replacement: one built from
            // `NetifConfiguration::wifi_default_router()` would carry the
            // same fixed "WIFI_AP_DEF" key already registered by the AP
            // netif `EspWifi` set up at startup, and `esp_netif_new` rejects
            // a duplicate key with `ESP_ERR_INVALID_ARG`.
            //
            // 1. Its address: see `PROVISIONING_AP_IP` for why it is not the
            //    default 192.168.71.1.
            // 2. The DNS server its DHCP leases name. embedded-svc's default
            //    router config hands clients Google's public DNS (8.8.8.8),
            //    which this isolated hotspot has no route to: every query
            //    of a joining phone -- including the captive-portal probe
            //    the single-QR-code join flow depends on -- would time out
            //    instead of reaching `crate::dns_captive_portal`'s wildcard
            //    responder. The hotspot names itself instead.
            // 3. DHCP Option 114 (RFC 8910), the captive-portal signal
            //    ESP-IDF's own `captive_portal` example sets alongside the
            //    DNS/HTTP trick: a client that supports it reads the portal
            //    URI from its lease.
            let ap_ip: std::net::Ipv4Addr = PROVISIONING_AP_IP
                .parse()
                .context("provisioning AP IP is not a valid IPv4 address")?;
            let ap_netmask: std::net::Ipv4Addr = PROVISIONING_AP_NETMASK
                .parse()
                .context("provisioning AP netmask is not a valid IPv4 address")?;
            let as_esp_ip = |ip: std::net::Ipv4Addr| sys::esp_ip4_addr_t {
                addr: u32::to_be(u32::from_be_bytes(ip.octets())),
            };
            let captive_portal_uri = std::ffi::CString::new(format!("http://{PROVISIONING_AP_IP}"))
                .context("provisioning captive-portal URI contains an interior NUL byte")?;
            let ap_netif = wifi.wifi_mut().ap_netif().handle();

            // ESP-IDF takes a new address only while the netif's DHCP server
            // is *stopped* -- not while it runs, and not in the "never
            // started" state it has before the hotspot first comes up
            // (`ESP_ERR_ESP_NETIF_DHCP_NOT_STOPPED` in both) -- and DHCP
            // options only while it is not running. So: stop, set, start,
            // whatever state it was in. Started again before the hotspot is
            // up, the server just waits for it; it then leases addresses
            // from the new /24.
            let status = unsafe { sys::esp_netif_dhcps_stop(ap_netif) };
            if status != sys::ESP_OK
                && status != sys::ESP_ERR_ESP_NETIF_DHCP_ALREADY_STOPPED as sys::esp_err_t
            {
                return Err(anyhow!(
                    "esp_netif_dhcps_stop before configuring the provisioning hotspot failed: {status}"
                ));
            }

            let ip_info = sys::esp_netif_ip_info_t {
                ip: as_esp_ip(ap_ip),
                netmask: as_esp_ip(ap_netmask),
                gw: as_esp_ip(ap_ip),
            };
            let ip_status = unsafe { sys::esp_netif_set_ip_info(ap_netif, &ip_info) };

            let mut dns_info = unsafe { core::mem::zeroed::<sys::esp_netif_dns_info_t>() };
            let dns_status = unsafe {
                dns_info.ip.u_addr.ip4 = as_esp_ip(ap_ip);
                sys::esp_netif_set_dns_info(
                    ap_netif,
                    sys::esp_netif_dns_type_t_ESP_NETIF_DNS_MAIN,
                    &mut dns_info,
                )
            };

            let option_status = unsafe {
                sys::esp_netif_dhcps_option(
                    ap_netif,
                    sys::esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_SET,
                    sys::esp_netif_dhcp_option_id_t_ESP_NETIF_CAPTIVEPORTAL_URI,
                    captive_portal_uri.as_ptr() as *mut core::ffi::c_void,
                    captive_portal_uri.as_bytes().len() as u32,
                )
            };

            // The server goes back on whatever happened above: a hotspot
            // that hands out no addresses is worse than any of the three
            // settings missing.
            let restart_status = unsafe { sys::esp_netif_dhcps_start(ap_netif) };
            if restart_status != sys::ESP_OK
                && restart_status != sys::ESP_ERR_ESP_NETIF_DHCP_ALREADY_STARTED as sys::esp_err_t
            {
                return Err(anyhow!(
                    "esp_netif_dhcps_start after configuring the provisioning hotspot failed: {restart_status}"
                ));
            }
            if ip_status != sys::ESP_OK {
                return Err(anyhow!(
                    "esp_netif_set_ip_info failed to give the provisioning hotspot its address: {ip_status}"
                ));
            }
            if dns_status != sys::ESP_OK {
                return Err(anyhow!(
                    "esp_netif_set_dns_info failed to point the provisioning hotspot's DNS at itself: {dns_status}"
                ));
            }
            // Option 114 is an extra signal on top of the DNS/HTTP redirect,
            // not a requirement: a failure here must not take the portal down.
            if option_status == sys::ESP_OK {
                // `esp_netif_dhcps_option` stored the raw pointer above, not
                // a copy; keep the backing `CString` alive for as long as the
                // hotspot might still be up.
                self.captive_portal_uri = Some(captive_portal_uri);
            } else {
                warn!(
                    "rustmix-wave=provisioning-captive-uri status=failed error-code={option_status}"
                );
            }
            if crate::dns_captive_portal::DIAGNOSTIC_LOG {
                warn!(
                    "rustmix-wave=provisioning-hotspot status=configured ip={PROVISIONING_AP_IP} netmask={PROVISIONING_AP_NETMASK} ip-status={ip_status} dns-status={dns_status} option114-status={option_status} dhcps-start={restart_status}"
                );
            }

            wifi.set_configuration(&Configuration::Mixed(
                ClientConfiguration::default(),
                AccessPointConfiguration {
                    ssid: ap_ssid
                        .as_str()
                        .try_into()
                        .context("AP SSID exceeds embedded Wi-Fi capacity")?,
                    password: ap_password
                        .as_str()
                        .try_into()
                        .context("AP password exceeds embedded Wi-Fi capacity")?,
                    auth_method: AuthMethod::WPA2Personal,
                    max_connections: PROVISIONING_AP_MAX_CONNECTIONS,
                    ..Default::default()
                },
            ))?;
            if !self.driver_started {
                wifi.wifi_mut().start()?;
                self.driver_started = true;
            }
            self.provisioning = true;
            self.snapshot.wifi_state = WifiConnectionState::Disabled;
            self.snapshot.ntp_state = NtpSyncState::Disabled;
            self.snapshot.ipv4_address = None;
            self.snapshot.rssi_dbm = None;
            self.snapshot.error = None;
            Ok(ProvisioningApInfo {
                ap_ssid,
                ap_password,
                portal_ip: PROVISIONING_AP_IP.into(),
            })
        }

        /// Attempt one candidate network. While the AP+STA pair started by
        /// [`Self::start_provisioning`] is up, switches to `Mixed` so the
        /// hotspot keeps broadcasting (used by the portal's hotspot-bootstrap
        /// "validate before save" step). Otherwise -- a device already
        /// joined to a LAN, trying a *different* saved network from the
        /// portal's Wi-Fi tab -- there is no AP to protect, so this switches
        /// straight to the candidate, dropping the current connection
        /// immediately; the caller is responsible for reconnecting to the
        /// existing saved-network list (e.g. via [`Self::stop_provisioning`])
        /// if the candidate then fails, so the device is not left stranded.
        /// Non-blocking either way; poll [`Self::snapshot`] for the outcome
        /// (`Connected`/`Failed`) the way the boot sequence does. Only a
        /// confirmed `Connected` snapshot should be persisted.
        pub fn try_join_candidate(&mut self, ssid: String, password: String) -> Result<()> {
            let wifi = self
                .wifi
                .as_mut()
                .context("Wi-Fi driver was never started")?;
            let candidate = SavedNetwork { ssid, password };
            let _ = wifi.disconnect();
            let client_conf = client_configuration(&candidate)?;
            let configuration = match ap_configuration_in_use(wifi) {
                Ok(ap_conf) => Configuration::Mixed(client_conf, ap_conf),
                Err(_) => Configuration::Client(client_conf),
            };
            wifi.set_configuration(&configuration)?;
            wifi.wifi_mut().connect()?;
            self.snapshot.wifi_state = WifiConnectionState::Connecting;
            self.snapshot.ssid = Some(candidate.ssid.clone());
            self.snapshot.ipv4_address = None;
            self.snapshot.rssi_dbm = None;
            self.snapshot.error = None;
            self.candidates = vec![candidate];
            self.candidate_index = 0;
            self.boot_phase = Some(WifiBootPhase::Connecting);
            self.boot_started_at = Some(Instant::now());
            Ok(())
        }

        /// Leave provisioning mode, dropping the hotspot, and reconnect
        /// using the (possibly just-updated) saved-network list in boot
        /// fail-over order. Non-blocking, same shape as [`Self::connect`].
        /// `config` is `None` when provisioning ends without ever saving a
        /// network (nothing to connect to yet): the driver still leaves
        /// `Configuration::Mixed`, but stays otherwise idle.
        pub fn stop_provisioning(&mut self, config: Option<&NetworkConfig>) -> Result<()> {
            self.provisioning = false;
            self.sntp = None;
            self.boot_phase = None;
            self.boot_started_at = None;
            self.candidate_cooldown_started_at = None;
            self.candidates.clear();
            self.candidate_index = 0;
            let wifi = self
                .wifi
                .as_mut()
                .context("Wi-Fi driver was never started")?;
            let _ = wifi.disconnect();
            let Some(config) = config else {
                wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
                self.snapshot = NetworkSnapshot::default();
                self.ntp_reported = false;
                return Ok(());
            };
            // Same scan-first order as `connect`/`resume`: connecting straight
            // to `WIFI.TXT`'s first entry cost the full `WIFI_BOOT_TIMEOUT`
            // whenever that one was switched off (a phone hotspot) before an
            // in-range saved network got its turn. The network last actually
            // connected leads the saved order the scan falls back on.
            let mut candidates = config.networks.clone();
            if let Some(position) = self
                .connected_ssid
                .as_ref()
                .and_then(|ssid| candidates.iter().position(|network| &network.ssid == ssid))
            {
                let preferred = candidates.remove(position);
                candidates.insert(0, preferred);
            }
            let first = candidates
                .first()
                .cloned()
                .context("at least one saved network is required")?;
            wifi.set_configuration(&Configuration::Client(client_configuration(&first)?))?;
            let scan_started = start_candidate_scan(candidates.len(), "wifi-reconnect-scan");
            if !scan_started {
                wifi.wifi_mut().connect()?;
            }
            self.candidates = candidates;
            self.snapshot.wifi_state = WifiConnectionState::Connecting;
            self.snapshot.ntp_state = NtpSyncState::WaitingForWifi;
            self.snapshot.ssid = Some(first.ssid.clone());
            self.snapshot.ipv4_address = None;
            self.snapshot.rssi_dbm = None;
            self.snapshot.timezone_name = config.timezone.clone();
            self.snapshot.ntp_server = config.ntp_server.clone();
            self.snapshot.saved_network_count = config.networks.len();
            self.snapshot.error = None;
            self.ntp_reported = false;
            self.boot_phase = Some(if scan_started {
                WifiBootPhase::Scanning
            } else {
                WifiBootPhase::Connecting
            });
            self.boot_started_at = Some(Instant::now());
            Ok(())
        }

        pub fn record_resume_failure(&mut self, error: impl Into<String>) {
            self.snapshot.wifi_state = WifiConnectionState::Failed;
            self.snapshot.ntp_state = NtpSyncState::Failed;
            self.snapshot.ipv4_address = None;
            self.snapshot.rssi_dbm = None;
            self.snapshot.error = Some(error.into());
            self.suspended = false;
        }

        pub fn record_configuration_missing(&mut self) {
            self.snapshot = NetworkSnapshot::default();
            self.ntp_reported = false;
            self.suspended = false;
        }

        /// Poll for an SNTP-populated system clock. The official wrapper keeps
        /// the SNTP service alive and updates `SystemTime` in the background.
        pub fn tick(&mut self) -> Option<RtcDateTime> {
            if self.suspended {
                return None;
            }
            let was_connected = matches!(self.snapshot.wifi_state, WifiConnectionState::Connected);
            self.advance_boot_phase();
            // Only meaningful -- and only safe to call -- while actually
            // associated to an AP. Calling `esp_wifi_sta_get_ap_info` with no
            // active STA association (e.g. every tick during phone
            // provisioning, where `Configuration::Mixed` leaves the STA side
            // idle) is a known trigger for the Wi-Fi driver's
            // "Haven't to connect to a suitable AP now!" log spam, which
            // also disrupts the AP side sharing its radio -- resetting a
            // joined phone's in-flight requests, including the
            // captive-portal probe the single-QR-code join flow depends on.
            //
            // Gated on `was_connected` (state on *entry*, before
            // `advance_boot_phase` ran) so the tick that freshly transitions
            // into `Connected` doesn't immediately re-read RSSI a second
            // time: `advance_boot_phase`'s `Connecting` arm already sampled
            // it once for that same transition.
            if was_connected && matches!(self.snapshot.wifi_state, WifiConnectionState::Connected) {
                self.snapshot.rssi_dbm = read_rssi_dbm();
            }
            if self.ntp_reported || self.sntp.is_none() {
                return None;
            }
            let seconds = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
            if seconds < MIN_VALID_SNTP_UNIX_SECONDS {
                return None;
            }
            let utc = utc_from_unix_seconds(seconds);
            self.snapshot.ntp_state = NtpSyncState::Synchronized;
            self.snapshot.last_sync_utc = Some(utc);
            self.ntp_reported = true;
            Some(utc)
        }

        /// Advance the non-blocking Wi-Fi boot handshake started by
        /// [`Self::connect`], [`Self::stop_provisioning`] or
        /// [`Self::try_join_candidate`] by one driver-state check. Called
        /// once per `tick`, i.e. once per main-loop iteration; never blocks,
        /// so button polling in the same loop is never held up by it. On a
        /// `Connecting`-phase failure, advances to the next saved-network
        /// candidate instead of failing immediately, unless the current
        /// candidate is the last (or only, for a provisioning test) one.
        fn advance_boot_phase(&mut self) {
            let Some(phase) = self.boot_phase else {
                return;
            };

            // Handled before `wifi` is borrowed below: `try_next_candidate`
            // re-borrows `self.wifi` itself (see its doc comment), so this
            // must run with no outstanding borrow of `self.wifi` in scope.
            if matches!(phase, WifiBootPhase::CandidateCooldown) {
                let elapsed = self
                    .candidate_cooldown_started_at
                    .is_some_and(|started| started.elapsed() >= CANDIDATE_RETRY_COOLDOWN);
                if !elapsed {
                    return;
                }
                self.candidate_cooldown_started_at = None;
                if !self.try_next_candidate() {
                    self.snapshot.wifi_state = WifiConnectionState::Failed;
                    self.snapshot.ntp_state = NtpSyncState::Failed;
                    self.snapshot.error = Some("no further saved network candidates".into());
                    self.boot_phase = None;
                    self.boot_started_at = None;
                }
                return;
            }

            let Some(wifi) = self.wifi.as_mut() else {
                self.boot_phase = None;
                self.boot_started_at = None;
                self.candidate_cooldown_started_at = None;
                return;
            };

            let mut failure: Option<String> = None;
            let mut advanced = false;

            match phase {
                WifiBootPhase::Scanning => {
                    let settled = self
                        .boot_started_at
                        .is_some_and(|started| started.elapsed() >= WIFI_SCAN_TIMEOUT);
                    if settled {
                        self.scan_started_at = None;
                        match fetch_scan_results() {
                            Ok(entries) => {
                                self.candidates = reorder_candidates_by_scan(
                                    std::mem::take(&mut self.candidates),
                                    &entries,
                                );
                            }
                            Err(error) => {
                                warn!("rustmix-wave=wifi-boot-scan status=failed error={error}");
                            }
                        }
                        match self.candidates.first().cloned() {
                            Some(winner) => {
                                let outcome = client_configuration(&winner).and_then(|conf| {
                                    wifi.set_configuration(&Configuration::Client(conf))?;
                                    wifi.wifi_mut().connect()?;
                                    Ok(())
                                });
                                match outcome {
                                    Ok(()) => {
                                        self.snapshot.ssid = Some(winner.ssid);
                                        self.boot_phase = Some(WifiBootPhase::Connecting);
                                        self.boot_started_at = Some(Instant::now());
                                        advanced = true;
                                        // OTA-worker internal-RAM fragmentation
                                        // investigation: heap right as
                                        // association is requested, before
                                        // DHCP/SNTP traffic -- see the matching
                                        // trace at Wi-Fi-connected-and-synced.
                                        crate::runtime_memory::log_runtime_memory(
                                            "wifi-association-requested",
                                        );
                                    }
                                    Err(error) => failure = Some(format!("{error:?}")),
                                }
                            }
                            None => failure = Some("no saved network is available".into()),
                        }
                    }
                }
                WifiBootPhase::Starting => match wifi.is_started() {
                    Ok(true) => match wifi.wifi_mut().connect() {
                        Ok(()) => {
                            self.boot_phase = Some(WifiBootPhase::Connecting);
                            advanced = true;
                            crate::runtime_memory::log_runtime_memory("wifi-association-requested");
                        }
                        Err(error) => failure = Some(format!("{error:?}")),
                    },
                    Ok(false) => {}
                    Err(error) => failure = Some(format!("{error:?}")),
                },
                WifiBootPhase::Connecting => match sta_ready(wifi) {
                    Ok(true) => match wifi.wifi().sta_netif().get_ip_info() {
                        Ok(ip_info) => {
                            // Drop any SNTP session still held from an
                            // earlier successful association (e.g. testing a
                            // second candidate from the phone provisioning
                            // portal after an earlier one already
                            // succeeded) before requesting a new one:
                            // `EspSntp::new` fails with
                            // `ESP_ERR_INVALID_STATE` if the previous
                            // instance was not dropped first, since the
                            // underlying SNTP client is a process-wide
                            // singleton, even though the STA association and
                            // DHCP lease above genuinely succeeded.
                            self.sntp = None;
                            let mut conf = SntpConf::default();
                            conf.servers[0] = self.snapshot.ntp_server.as_str();
                            match EspSntp::new(&conf) {
                                Ok(sntp) => {
                                    self.sntp = Some(sntp);
                                    apply_background_power_save();
                                    self.snapshot.wifi_state = WifiConnectionState::Connected;
                                    self.connected_ssid = self
                                        .candidates
                                        .get(self.candidate_index)
                                        .map(|network| network.ssid.clone());
                                    self.snapshot.ntp_state = NtpSyncState::Synchronizing;
                                    self.snapshot.ipv4_address = Some(format!("{}", ip_info.ip));
                                    self.snapshot.rssi_dbm = read_rssi_dbm();
                                    self.boot_phase = None;
                                    advanced = true;
                                }
                                Err(error) => failure = Some(format!("{error:?}")),
                            }
                        }
                        Err(error) => failure = Some(format!("{error:?}")),
                    },
                    Ok(false) => {}
                    Err(error) => failure = Some(format!("{error:?}")),
                },
                WifiBootPhase::CandidateCooldown => {
                    unreachable!("handled and returned above before `wifi` was borrowed")
                }
            }

            if let Some(error) = failure {
                warn!("rustmix-wave=wifi-boot status=failed phase={phase:?} error={error}");
                // An immediate (synchronous) failure, as opposed to an
                // association that timed out below: defer the next
                // candidate's `connect()` by `CANDIDATE_RETRY_COOLDOWN`
                // instead of issuing it on this same tick, so several saved
                // networks that are all immediately unreachable don't
                // hammer the radio back-to-back.
                if matches!(phase, WifiBootPhase::Connecting)
                    && self.candidate_index + 1 < self.candidates.len()
                {
                    self.boot_phase = Some(WifiBootPhase::CandidateCooldown);
                    self.candidate_cooldown_started_at = Some(Instant::now());
                    return;
                }
                self.snapshot.wifi_state = WifiConnectionState::Failed;
                self.snapshot.ntp_state = NtpSyncState::Failed;
                self.snapshot.error = Some(error);
                self.boot_phase = None;
                self.boot_started_at = None;
                return;
            }

            if advanced {
                if self.boot_phase.is_none() {
                    self.boot_started_at = None;
                }
                return;
            }

            if self
                .boot_started_at
                .is_some_and(|started| started.elapsed() >= WIFI_BOOT_TIMEOUT)
            {
                warn!(
                    "rustmix-wave=wifi-boot status=timed-out phase={phase:?} timeout-secs={}",
                    WIFI_BOOT_TIMEOUT.as_secs()
                );
                if matches!(phase, WifiBootPhase::Connecting) && self.try_next_candidate() {
                    return;
                }
                self.snapshot.wifi_state = WifiConnectionState::Failed;
                self.snapshot.ntp_state = NtpSyncState::Failed;
                self.snapshot.error = Some("Wi-Fi association timed out".into());
                self.boot_phase = None;
                self.boot_started_at = None;
            }
        }

        /// Move to the next saved-network candidate after the current one
        /// failed to associate. Returns `false` (leaving `self.boot_phase`
        /// untouched for the caller to resolve into `Failed`) once the list
        /// is exhausted. Re-borrows `self.wifi` itself (rather than taking it
        /// as a parameter) so callers already holding a `self.wifi` borrow
        /// can drop it before calling this, avoiding a double mutable borrow
        /// of `self`.
        fn try_next_candidate(&mut self) -> bool {
            let Some(next) = self.candidates.get(self.candidate_index + 1).cloned() else {
                return false;
            };
            let Some(wifi) = self.wifi.as_mut() else {
                return false;
            };
            self.candidate_index += 1;
            let _ = wifi.disconnect();
            let configuration = if self.provisioning {
                ap_configuration_in_use(wifi)
                    .ok()
                    .map(|ap_conf| Configuration::Mixed(ClientConfiguration::default(), ap_conf))
            } else {
                None
            };
            let client_conf = match client_configuration(&next) {
                Ok(conf) => conf,
                Err(_) => return false,
            };
            let set_result = match configuration {
                Some(Configuration::Mixed(_, ap_conf)) => {
                    wifi.set_configuration(&Configuration::Mixed(client_conf, ap_conf))
                }
                _ => wifi.set_configuration(&Configuration::Client(client_conf)),
            };
            if set_result.is_err() || wifi.wifi_mut().connect().is_err() {
                return false;
            }
            self.snapshot.ssid = Some(next.ssid);
            self.snapshot.wifi_state = WifiConnectionState::Connecting;
            self.snapshot.error = None;
            self.boot_phase = Some(WifiBootPhase::Connecting);
            self.boot_started_at = Some(Instant::now());
            true
        }
    }

    /// STA-only readiness check. Unlike `EspWifi::is_up()`, this never ANDs
    /// in the AP interface's state, so it stays correct while
    /// `Configuration::Mixed` is active for provisioning (the AP side is
    /// always up once started; only the STA side reflects an in-progress
    /// candidate attempt).
    fn sta_ready(
        wifi: &BlockingWifi<EspWifi<'static>>,
    ) -> Result<bool, esp_idf_svc::sys::EspError> {
        Ok(wifi.is_connected()? && wifi.wifi().sta_netif().is_up()?)
    }

    /// Re-read the AP configuration currently applied to the driver, so a
    /// candidate fail-over during provisioning can restate it inside a fresh
    /// `Configuration::Mixed` without hard-coding the AP's SSID/password a
    /// second time.
    fn ap_configuration_in_use(
        wifi: &BlockingWifi<EspWifi<'static>>,
    ) -> Result<AccessPointConfiguration> {
        match wifi.get_configuration()? {
            Configuration::Mixed(_, ap_conf) | Configuration::AccessPoint(ap_conf) => Ok(ap_conf),
            _ => Err(anyhow!("access point is not active")),
        }
    }

    fn client_configuration(network: &SavedNetwork) -> Result<ClientConfiguration> {
        let auth_method = if network.password.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        };
        Ok(ClientConfiguration {
            ssid: network
                .ssid
                .as_str()
                .try_into()
                .context("SSID exceeds embedded Wi-Fi capacity")?,
            password: network
                .password
                .as_str()
                .try_into()
                .context("password exceeds embedded Wi-Fi capacity")?,
            auth_method,
            ..Default::default()
        })
    }

    /// A fresh random 12-character alphanumeric WPA2 password for the
    /// provisioning hotspot, generated each session from the hardware RNG.
    fn generate_ap_password() -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        (0..12)
            .map(|_| {
                let index = (unsafe { sys::esp_random() } as usize) % ALPHABET.len();
                ALPHABET[index] as char
            })
            .collect()
    }

    /// Start the scan that decides which saved network to try first, when
    /// there is a choice to make. With a single saved network there is none,
    /// and `esp_wifi_connect` looks for it on every channel by itself: a scan
    /// before that only delayed the connection by one more full sweep.
    /// `false` (a single network, or a scan that failed to start) means
    /// connecting in saved order.
    fn start_candidate_scan(saved_networks: usize, marker: &str) -> bool {
        if saved_networks < 2 {
            return false;
        }
        let started = unsafe { sys::esp_wifi_scan_start(core::ptr::null(), false) } == sys::ESP_OK;
        if !started {
            warn!("rustmix-wave={marker} status=failed-to-start falling-back=saved-order");
        }
        started
    }

    fn read_rssi_dbm() -> Option<i32> {
        let mut record = unsafe { core::mem::zeroed::<sys::wifi_ap_record_t>() };
        let status = unsafe { sys::esp_wifi_sta_get_ap_info(&mut record) };
        (status == sys::ESP_OK).then_some(i32::from(record.rssi))
    }

    /// Read back results from a scan already known to have settled (started
    /// by [`NetworkRuntime::start_scan`] or the boot-time scan in
    /// [`NetworkRuntime::connect`]). Shared so both call sites report
    /// failures the same way.
    fn fetch_scan_results() -> std::result::Result<Vec<WifiScanEntry>, String> {
        let mut count: u16 = 0;
        let status = unsafe { sys::esp_wifi_scan_get_ap_num(&mut count) };
        if status != sys::ESP_OK {
            return Err(format!("esp_wifi_scan_get_ap_num failed: {status}"));
        }

        let capped = (count as usize).min(WIFI_SCAN_MAX_RESULTS);
        let mut records = vec![unsafe { core::mem::zeroed::<sys::wifi_ap_record_t>() }; capped];
        let mut fetched = capped as u16;
        let status =
            unsafe { sys::esp_wifi_scan_get_ap_records(&mut fetched, records.as_mut_ptr()) };
        if status != sys::ESP_OK {
            return Err(format!("esp_wifi_scan_get_ap_records failed: {status}"));
        }
        records.truncate(fetched as usize);

        Ok(records
            .iter()
            .map(|record| WifiScanEntry {
                ssid: ap_record_ssid(record),
                rssi_dbm: i32::from(record.rssi),
            })
            .collect())
    }

    /// Reorder boot candidates so any saved SSID the just-completed scan
    /// actually saw in range is tried before ones it didn't, strongest
    /// signal (closest access point) first, without disturbing the
    /// out-of-range group's original saved-priority order. This is what
    /// stops a saved-but-out-of-range network listed first in `WIFI.TXT`
    /// from eating the full `WIFI_BOOT_TIMEOUT` before an in-range saved
    /// network further down the list ever gets a turn, and what prefers the
    /// nearest of several in-range saved networks (e.g. an AP advertising on
    /// multiple channels, or two saved networks both in range) instead of
    /// just the one listed first in `WIFI.TXT`.
    fn reorder_candidates_by_scan(
        candidates: Vec<SavedNetwork>,
        scan: &[WifiScanEntry],
    ) -> Vec<SavedNetwork> {
        let strongest_rssi = |ssid: &str| -> Option<i32> {
            scan.iter()
                .filter(|entry| entry.ssid == ssid)
                .map(|entry| entry.rssi_dbm)
                .max()
        };
        let (mut in_range, out_of_range): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .partition(|network| strongest_rssi(&network.ssid).is_some());
        in_range.sort_by(|a, b| strongest_rssi(&b.ssid).cmp(&strongest_rssi(&a.ssid)));
        in_range.into_iter().chain(out_of_range).collect()
    }

    /// Decode a scan record's null-terminated SSID byte array.
    fn ap_record_ssid(record: &sys::wifi_ap_record_t) -> String {
        let bytes = &record.ssid[..];
        let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..len]).into_owned()
    }

    #[cfg(test)]
    mod tests {
        use super::{reorder_candidates_by_scan, WifiScanEntry};
        use crate::network_config::SavedNetwork;

        fn network(ssid: &str) -> SavedNetwork {
            SavedNetwork {
                ssid: ssid.into(),
                password: String::new(),
            }
        }

        fn seen(ssid: &str, rssi_dbm: i32) -> WifiScanEntry {
            WifiScanEntry {
                ssid: ssid.into(),
                rssi_dbm,
            }
        }

        #[test]
        fn in_range_candidates_are_tried_strongest_signal_first() {
            let candidates = vec![network("Far"), network("Near"), network("Medium")];
            // Saved order lists "Far" first, but the scan sees "Near" much
            // closer; it must be tried before "Far" and "Medium" despite
            // being listed second in `WIFI.TXT`.
            let scan = vec![seen("Far", -80), seen("Near", -30), seen("Medium", -55)];
            let reordered = reorder_candidates_by_scan(candidates, &scan);
            assert_eq!(
                reordered
                    .iter()
                    .map(|n| n.ssid.as_str())
                    .collect::<Vec<_>>(),
                vec!["Near", "Medium", "Far"]
            );
        }

        #[test]
        fn out_of_range_candidates_keep_saved_order_after_in_range_ones() {
            let candidates = vec![network("Unseen"), network("Home"), network("AlsoUnseen")];
            let scan = vec![seen("Home", -50)];
            let reordered = reorder_candidates_by_scan(candidates, &scan);
            assert_eq!(
                reordered
                    .iter()
                    .map(|n| n.ssid.as_str())
                    .collect::<Vec<_>>(),
                vec!["Home", "Unseen", "AlsoUnseen"]
            );
        }

        #[test]
        fn a_saved_network_seen_on_multiple_channels_uses_its_strongest_reading() {
            let candidates = vec![network("Weak"), network("MultiChannel")];
            let scan = vec![
                seen("Weak", -60),
                seen("MultiChannel", -70),
                seen("MultiChannel", -40),
            ];
            let reordered = reorder_candidates_by_scan(candidates, &scan);
            assert_eq!(
                reordered
                    .iter()
                    .map(|n| n.ssid.as_str())
                    .collect::<Vec<_>>(),
                vec!["MultiChannel", "Weak"]
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NetworkSnapshot, NtpSyncState, WifiConnectionState, PROVISIONING_AP_IP,
        PROVISIONING_AP_NETMASK,
    };

    #[test]
    fn the_hotspot_address_is_not_one_a_phone_takes_for_a_home_network() {
        // Android does not offer the sign-in page when the hotspot's DNS
        // answers with a private address.
        let ip: std::net::Ipv4Addr = PROVISIONING_AP_IP.parse().unwrap();
        assert!(!ip.is_private(), "{ip}");
        assert!(!ip.is_loopback() && !ip.is_link_local() && !ip.is_multicast());
        assert!(!ip.is_unspecified() && !ip.is_broadcast() && !ip.is_documentation());
        // The gateway is the first host of its own /24.
        assert_eq!(ip.octets()[3], 1);
        let mask: std::net::Ipv4Addr = PROVISIONING_AP_NETMASK.parse().unwrap();
        assert_eq!(mask.octets(), [255, 255, 255, 0]);
    }

    #[test]
    fn configuration_missing_snapshot_is_safe_for_home() {
        let snapshot = NetworkSnapshot::default();
        assert_eq!(
            snapshot.wifi_state,
            WifiConnectionState::ConfigurationMissing
        );
        assert_eq!(snapshot.ntp_state, NtpSyncState::WaitingForWifi);
        assert_eq!(snapshot.home_badge(), "NO CFG");
        assert_eq!(snapshot.ssid_label(), "--");
    }

    #[test]
    fn connected_and_synchronized_snapshot_has_ntp_badge() {
        let snapshot = NetworkSnapshot {
            wifi_state: WifiConnectionState::Connected,
            ntp_state: NtpSyncState::Synchronized,
            ..NetworkSnapshot::default()
        };
        assert_eq!(snapshot.home_badge(), "NTP OK");
    }
    #[test]
    fn log_fingerprint_ignores_rssi_churn() {
        let mut snapshot = NetworkSnapshot::default();
        snapshot.rssi_dbm = Some(-34);
        let first = snapshot.log_fingerprint();
        snapshot.rssi_dbm = Some(-61);
        assert_eq!(first, snapshot.log_fingerprint());
    }

    #[test]
    fn log_fingerprint_still_changes_for_ipv4_state() {
        let snapshot = NetworkSnapshot::default();
        let first = snapshot.log_fingerprint();
        let mut changed = snapshot;
        changed.ipv4_address = Some("192.0.2.10".into());
        assert_ne!(first, changed.log_fingerprint());
    }

    #[test]
    fn provisioned_snapshot_reports_first_candidate_and_saved_count() {
        let config = crate::network_config::NetworkConfig::validated(
            vec![
                crate::network_config::SavedNetwork {
                    ssid: "Home".into(),
                    password: "correct-horse".into(),
                },
                crate::network_config::SavedNetwork {
                    ssid: "Travel".into(),
                    password: "second-pass".into(),
                },
            ],
            "UTC".into(),
            "pool.ntp.org".into(),
        )
        .unwrap();
        let snapshot = NetworkSnapshot::provisioned(&config);
        assert_eq!(snapshot.ssid_label(), "Home");
        assert_eq!(snapshot.saved_network_count, 2);
    }
}
