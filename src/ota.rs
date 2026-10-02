//! GitHub-release-based OTA update check, download and flash.
//!
//! The device compares its own [`crate::build_info::FIRMWARE_VERSION`]
//! against the GitHub release its [`UpdateChannel`] points at on
//! `build_info::OTA_REPO_OWNER`/`OTA_REPO_NAME`: the latest release on the
//! stable channel, the highest version among the recent ones, pre-releases
//! included, on the beta channel. No JSON crate is used (same
//! homegrown-parser approach as `weather.rs`): only the fields actually
//! needed, `tag_name`, `draft` and the first `.bin` asset URL, are pulled
//! out of the response with a small quoted-string scanner. ESP-IDF HTTPS
//! wiring and flashing live below `cfg(target_os = "espidf")`.

use std::{cmp::Ordering, fs, io, path::Path};

/// Upper bound on the buffered `releases/latest` JSON response. A release's
/// markdown description (`body` field) counts toward this even though it is
/// never read, so keep release notes on this repository reasonably short --
/// an oversized `body` can push `tag_name`/`assets` past this cap and make a
/// check fail safely (retried on the next interval) instead of parsing.
pub const MAX_RELEASE_RESPONSE_BYTES: usize = 16 * 1024;
/// Chunk size used both for downloading the update binary and for streaming
/// it straight into the inactive OTA partition -- large enough for decent
/// TLS throughput, small enough to keep worker stack/heap use tiny. Nothing
/// ever buffers a whole firmware image at once.
pub const OTA_DOWNLOAD_CHUNK_BYTES: usize = 4096;
/// How often the main loop checks GitHub for a new release in the
/// background, while Wi-Fi is connected. The Settings > Software Update
/// screen can always trigger an immediate check regardless of this timer.
pub const OTA_CHECK_INTERVAL_SECONDS: u64 = 24 * 60 * 60;
/// Upper bound on the buffered release *list* the beta channel reads:
/// [`BETA_RELEASE_LIST_LENGTH`] releases, each about the size of one
/// `releases/latest` response.
pub const MAX_RELEASE_LIST_BYTES: usize = 96 * 1024;
/// Releases the beta channel compares, newest first. The highest version
/// among them wins, so a stable release published after a beta still
/// reaches beta devices.
pub const BETA_RELEASE_LIST_LENGTH: usize = 5;
/// Where the channel chosen on the Software Update screen is kept.
pub const UPDATE_CONFIG_PATH: &str = "/sdcard/RUSTMIX/UPDATE.TXT";

/// Which releases the device updates from. Stable follows GitHub's latest
/// release, which is never a pre-release. Beta follows the highest version
/// among the recent releases, pre-releases included: beta builds, from the
/// `beta` branch, are published as GitHub pre-releases with a version such
/// as `1.5.0-beta.1`. Switching back to stable never downgrades: the device
/// waits for a stable release newer than the firmware it runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

impl UpdateChannel {
    /// The channel a firmware version was published on: a pre-release
    /// (`1.5.0-beta.1`) came from the beta channel. Used until a choice has
    /// been saved.
    #[must_use]
    pub fn of_version(version: &str) -> Self {
        match parse_version(version) {
            Some(version) if !version.pre.is_empty() => Self::Beta,
            _ => Self::Stable,
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }

    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Stable => Self::Beta,
            Self::Beta => Self::Stable,
        }
    }

    /// GitHub API path, below the repository, of the releases this channel
    /// reads.
    #[must_use]
    pub fn releases_api_path(self) -> String {
        match self {
            Self::Stable => "releases/latest".into(),
            Self::Beta => format!("releases?per_page={BETA_RELEASE_LIST_LENGTH}"),
        }
    }

    /// Largest response the request for this channel may return.
    #[must_use]
    pub const fn response_limit_bytes(self) -> usize {
        match self {
            Self::Stable => MAX_RELEASE_RESPONSE_BYTES,
            Self::Beta => MAX_RELEASE_LIST_BYTES,
        }
    }

    /// The release this channel offers, from the response to
    /// [`Self::releases_api_path`].
    pub fn parse_response(self, json: &str) -> Result<ReleaseInfo, ReleaseCheckError> {
        match self {
            Self::Stable => parse_release_response(json),
            Self::Beta => newest_release(parse_release_list(json)).ok_or_else(|| {
                ReleaseCheckError::InvalidResponse("no published release has a .bin asset".into())
            }),
        }
    }

    /// The `channel=` value of an `UPDATE.TXT`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .filter(|(key, _)| key.trim() == "channel")
            .find_map(
                |(_, value)| match value.trim().to_ascii_lowercase().as_str() {
                    "stable" => Some(Self::Stable),
                    "beta" => Some(Self::Beta),
                    _ => None,
                },
            )
    }

    #[must_use]
    pub fn serialized(self) -> String {
        format!(
            "# RustMix Wave update channel: stable or beta\nchannel={}\n",
            self.marker()
        )
    }

    /// The saved channel, or the one `running_version` was published on
    /// when nothing readable is saved (first boot, no card).
    #[must_use]
    pub fn load_from_path(path: impl AsRef<Path>, running_version: &str) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| Self::parse(&text))
            .unwrap_or_else(|| Self::of_version(running_version))
    }

    pub fn save_to_path(self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.serialized())
    }
}

/// One parsed GitHub "latest release" response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub download_url: String,
}

/// Release-check failure classified for logging.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseCheckError {
    Transport(String),
    HttpStatus(u16),
    InvalidResponse(String),
}

impl core::fmt::Display for ReleaseCheckError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport(message) => write!(formatter, "{message}"),
            Self::HttpStatus(status) => {
                write!(formatter, "GitHub API returned HTTP status {status}")
            }
            Self::InvalidResponse(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for ReleaseCheckError {}

/// Product-facing OTA lifecycle, driven by the Settings > Software Update
/// screen and by the main loop's periodic background check.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum OtaCheckState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    UpdateAvailable {
        version: String,
        download_url: String,
    },
    CheckFailed(String),
    Installing,
    InstallFailed(String),
}

impl OtaCheckState {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Checking => "CHECKING",
            Self::UpToDate => "UP TO DATE",
            Self::UpdateAvailable { .. } => "UPDATE AVAILABLE",
            Self::CheckFailed(_) => "CHECK FAILED",
            Self::Installing => "INSTALLING",
            Self::InstallFailed(_) => "INSTALL FAILED",
        }
    }

    #[must_use]
    pub const fn is_update_available(&self) -> bool {
        matches!(self, Self::UpdateAvailable { .. })
    }

    /// Whether a check (manual or periodic) may currently be started.
    #[must_use]
    pub const fn can_check(&self) -> bool {
        !matches!(self, Self::Checking | Self::Installing)
    }
}

/// One-shot UI action, drained by the main loop from `AppState`.
///
/// `InstallNow` carries its own `version`/`download_url` rather than making
/// the runtime owner re-read them from `AppState::ota` at drain time: the
/// button handler that queues this request also immediately flips `ota` to
/// `Installing` (for instant screen feedback), which would otherwise erase
/// the very data the install needs before the main loop ever gets to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OtaUiRequest {
    CheckNow,
    InstallNow {
        version: String,
        download_url: String,
    },
}

/// Compare the running firmware version against a release tag, with
/// SemVer precedence when both sides parse: a leading `v`/`V` is ignored,
/// and a pre-release such as `1.5.0-beta.2` comes after `1.5.0-beta.1` and
/// before `1.5.0`. Otherwise falls back to a plain string inequality check,
/// which is good enough to flag "something changed" without pretending to
/// understand every possible tag format.
#[must_use]
pub fn is_update_available(current: &str, remote_tag: &str) -> bool {
    match (parse_version(current), parse_version(remote_tag)) {
        (Some(current), Some(remote)) => remote > current,
        _ => {
            let remote = remote_tag.trim_start_matches(['v', 'V']);
            !remote.is_empty() && remote != current
        }
    }
}

/// A release version: `major.minor.patch` and the dot-separated
/// pre-release identifiers after a `-` (`1.5.0-beta.2` has `beta`, `2`).
#[derive(Clone, Debug, Eq, PartialEq)]
struct Version {
    core: (u32, u32, u32),
    pre: Vec<String>,
}

impl Ord for Version {
    /// SemVer precedence: a pre-release comes before its release, and
    /// pre-release identifiers compare as numbers when both are numbers
    /// (`beta.2` before `beta.10`).
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre_release(&self.pre, &other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_pre_release(left: &[String], right: &[String]) -> Ordering {
    for (left_id, right_id) in left.iter().zip(right) {
        let order = match (left_id.parse::<u64>(), right_id.parse::<u64>()) {
            (Ok(left_number), Ok(right_number)) => left_number.cmp(&right_number),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => left_id.cmp(right_id),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// Best-effort `major.minor.patch[-pre.release][+build]` parse: a missing
/// minor or patch reads as 0, build metadata is ignored, and a suffix glued
/// to the patch number (`1.2.4rc1`) counts as a pre-release.
fn parse_version(input: &str) -> Option<Version> {
    let input = input.trim().trim_start_matches(['v', 'V']);
    let input = input.split('+').next().unwrap_or_default();
    let (core, pre) = input.split_once('-').unwrap_or((input, ""));
    let mut parts = core.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch_field = parts.next().unwrap_or("0");
    let digits = patch_field.bytes().take_while(u8::is_ascii_digit).count();
    let patch = if digits == 0 {
        0
    } else {
        patch_field[..digits].parse().ok()?
    };
    let mut pre_release = Vec::new();
    if digits < patch_field.len() {
        pre_release.push(patch_field[digits..].to_string());
    }
    pre_release.extend(
        pre.split('.')
            .filter(|identifier| !identifier.is_empty())
            .map(str::to_string),
    );
    Some(Version {
        core: (major, minor, patch),
        pre: pre_release,
    })
}

/// Find `"key":"value"` in a JSON document and return `value`, decoding the
/// handful of escapes GitHub's API actually emits. Not a general JSON
/// parser: it trusts the document is well-formed and only looks for one
/// specific string-valued key.
fn extract_string_field(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\"");
    let key_pos = json.find(&pattern)?;
    let after_key = &json[key_pos + pattern.len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = after_key[colon_pos + 1..].trim_start();
    let after_quote = after_colon.strip_prefix('"')?;
    extract_until_unescaped_quote(after_quote)
}

/// Return the first `assets[].browser_download_url` whose value ends in
/// `.bin`, scanning past any earlier assets (checksums, source archives).
fn extract_first_bin_asset_url(json: &str) -> Option<String> {
    const KEY: &str = "\"browser_download_url\"";
    let mut search_from = 0;
    while let Some(relative) = json[search_from..].find(KEY) {
        let key_pos = search_from + relative;
        let after_key = &json[key_pos + KEY.len()..];
        let Some(colon_pos) = after_key.find(':') else {
            break;
        };
        let after_colon = after_key[colon_pos + 1..].trim_start();
        let Some(after_quote) = after_colon.strip_prefix('"') else {
            break;
        };
        let Some(value) = extract_until_unescaped_quote(after_quote) else {
            break;
        };
        if value.to_ascii_lowercase().ends_with(".bin") {
            return Some(value);
        }
        search_from = key_pos + KEY.len();
    }
    None
}

fn extract_until_unescaped_quote(input: &str) -> Option<String> {
    let mut result = String::new();
    let mut chars = input.chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(result),
            '\\' => match chars.next()? {
                'n' => result.push('\n'),
                't' => result.push('\t'),
                other => result.push(other),
            },
            other => result.push(other),
        }
    }
    None
}

/// Parse a full GitHub "get latest release" response body into the two
/// fields the update flow needs.
fn parse_release_response(json: &str) -> Result<ReleaseInfo, ReleaseCheckError> {
    let tag_name = extract_string_field(json, "tag_name")
        .ok_or_else(|| ReleaseCheckError::InvalidResponse("response has no tag_name".into()))?;
    let download_url = extract_first_bin_asset_url(json)
        .ok_or_else(|| ReleaseCheckError::InvalidResponse("release has no .bin asset".into()))?;
    Ok(ReleaseInfo {
        tag_name,
        download_url,
    })
}

/// `"key": true|false` in a JSON document, the first such key.
fn extract_bool_field(json: &str, key: &str) -> Option<bool> {
    let pattern = format!("\"{key}\"");
    let after_key = &json[json.find(&pattern)? + pattern.len()..];
    let after_colon = after_key[after_key.find(':')? + 1..].trim_start();
    if after_colon.starts_with("true") {
        Some(true)
    } else if after_colon.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

/// The objects of a top-level JSON array (`[{..}, {..}]`), as slices:
/// braces inside strings do not count.
fn top_level_objects(json: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let (mut depth, mut start) = (0_usize, 0_usize);
    let (mut in_string, mut escaped) = (false, false);
    for (index, byte) in json.bytes().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = index;
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    objects.push(&json[start..=index]);
                }
            }
            _ => {}
        }
    }
    objects
}

/// One release in GitHub's release list, as far as the update flow cares.
struct ReleaseEntry {
    tag_name: String,
    draft: bool,
    download_url: Option<String>,
}

/// Parse a GitHub "list releases" response: a JSON array of releases, each
/// read on its own, so the order of their fields does not matter.
fn parse_release_list(json: &str) -> Vec<ReleaseEntry> {
    top_level_objects(json)
        .into_iter()
        .filter_map(|release| {
            Some(ReleaseEntry {
                tag_name: extract_string_field(release, "tag_name")?,
                draft: extract_bool_field(release, "draft").unwrap_or(false),
                download_url: extract_first_bin_asset_url(release),
            })
        })
        .collect()
}

/// The highest version among the published releases with a `.bin` asset.
/// Between equal versions, or tags that do not parse, the first listed
/// wins: GitHub lists the newest first.
fn newest_release(releases: Vec<ReleaseEntry>) -> Option<ReleaseInfo> {
    let mut newest: Option<(Option<Version>, ReleaseInfo)> = None;
    for release in releases {
        if release.draft {
            continue;
        }
        let Some(download_url) = release.download_url else {
            continue;
        };
        let version = parse_version(&release.tag_name);
        if newest.as_ref().map_or(true, |(best, _)| version > *best) {
            newest = Some((
                version,
                ReleaseInfo {
                    tag_name: release.tag_name,
                    download_url,
                },
            ));
        }
    }
    newest.map(|(_, release)| release)
}

#[cfg(target_os = "espidf")]
pub mod espidf {
    use std::{sync::mpsc::Receiver, time::Duration};

    use embedded_svc::{
        http::{client::Client as HttpClient, Method},
        utils::io,
    };
    use esp_idf_svc::{
        http::client::{Configuration as HttpConfiguration, EspHttpConnection},
        ota::EspOta,
        sys,
    };
    use log::{info, warn};

    use crate::{
        build_info::{FIRMWARE_VERSION, OTA_REPO_NAME, OTA_REPO_OWNER},
        ota::{
            is_update_available, OtaCheckState, ReleaseCheckError, ReleaseInfo, UpdateChannel,
            OTA_DOWNLOAD_CHUNK_BYTES,
        },
        runtime_worker::{poll_named_worker, spawn_named_worker_in_psram, NamedWorkerError},
    };

    /// Stack budget for the short-lived version-check worker. Matches the
    /// weather-fetch worker's budget: same TLS-handshake-dominated cost.
    pub const OTA_CHECK_WORKER_STACK_BYTES: usize = 64 * 1024;
    const GITHUB_HTTP_TIMEOUT_SECONDS: u64 = 15;
    /// Generous: the firmware binary is a few MB over Wi-Fi, not a small
    /// JSON payload.
    const OTA_INSTALL_HTTP_TIMEOUT_SECONDS: u64 = 60;
    const OTA_USER_AGENT: &str = "rustmix-wave-ota";

    /// Starts a GitHub release check on a short-lived PSRAM-backed worker and
    /// returns immediately with a [`Receiver`] instead of waiting for the
    /// result. Poll it with [`poll_latest_release_check`] from the main
    /// hardware loop so that loop keeps draining input and redrawing while
    /// the request (DNS + TLS handshake + HTTP GET) is in flight -- a
    /// synchronous wait here was observed to freeze button handling for a
    /// couple of seconds every time Wi-Fi finished connecting, since the
    /// automatic background check fires as soon as the main loop sees Wi-Fi
    /// go connected.
    ///
    /// PSRAM-backed, not the plain internal-stack spawn: on this hardware,
    /// being Wi-Fi-connected alone has been observed to fragment internal
    /// SRAM below one 64 KB worker stack's size (confirmed in the field:
    /// this 64 KB spawn failed with ENOMEM against a ~31 KB largest free
    /// block while connected), so this check could never succeed on
    /// internal RAM. Safe here specifically because `fetch_release` only
    /// does an HTTPS GET and a JSON scan -- it never touches flash/NVS/the
    /// OTA partition, unlike `install_update` below, which must keep the
    /// plain internal-RAM stack.
    pub fn spawn_latest_release_check(
        channel: UpdateChannel,
    ) -> Result<Receiver<Result<ReleaseInfo, ReleaseCheckError>>, NamedWorkerError<ReleaseCheckError>>
    {
        info!(
            "rustmix-wave=ota-check status=starting channel={}",
            channel.marker()
        );
        spawn_named_worker_in_psram("ota-check", OTA_CHECK_WORKER_STACK_BYTES, move || {
            fetch_release(channel)
        })
        .map_err(NamedWorkerError::Start)
    }

    /// Non-blocking poll for a check started with
    /// [`spawn_latest_release_check`]. Returns `None` while the request is
    /// still in flight -- call again on a later main-loop iteration. Never
    /// panics: every failure path resolves to `CheckFailed` instead.
    pub fn poll_latest_release_check(
        receiver: &Receiver<Result<ReleaseInfo, ReleaseCheckError>>,
    ) -> Option<OtaCheckState> {
        let result = poll_named_worker("ota-check", receiver)?;
        Some(match result {
            Ok(release) => {
                if is_update_available(FIRMWARE_VERSION, &release.tag_name) {
                    info!(
                        "rustmix-wave=ota-check status=update-available current={FIRMWARE_VERSION} latest={}",
                        release.tag_name
                    );
                    OtaCheckState::UpdateAvailable {
                        version: release.tag_name,
                        download_url: release.download_url,
                    }
                } else {
                    info!(
                        "rustmix-wave=ota-check status=up-to-date current={FIRMWARE_VERSION} latest={}",
                        release.tag_name
                    );
                    OtaCheckState::UpToDate
                }
            }
            Err(NamedWorkerError::Operation(error)) => {
                warn!("rustmix-wave=ota-check status=failed error={error}");
                OtaCheckState::CheckFailed(error.to_string())
            }
            Err(error) => {
                let message = format!("update-check worker {error}");
                warn!("rustmix-wave=ota-check status=boundary-failed error={message}");
                OtaCheckState::CheckFailed(message)
            }
        })
    }

    fn fetch_release(channel: UpdateChannel) -> Result<ReleaseInfo, ReleaseCheckError> {
        let http_config = HttpConfiguration {
            crt_bundle_attach: Some(sys::esp_crt_bundle_attach),
            timeout: Some(Duration::from_secs(GITHUB_HTTP_TIMEOUT_SECONDS)),
            ..Default::default()
        };
        let connection = EspHttpConnection::new(&http_config).map_err(|error| {
            ReleaseCheckError::Transport(format!("HTTP connection init failed: {error}"))
        })?;
        let mut client = HttpClient::wrap(connection);
        let url = format!(
            "https://api.github.com/repos/{OTA_REPO_OWNER}/{OTA_REPO_NAME}/{}",
            channel.releases_api_path()
        );
        let headers = [
            ("accept", "application/vnd.github+json"),
            ("user-agent", OTA_USER_AGENT),
        ];
        let request = client
            .request(Method::Get, &url, &headers)
            .map_err(|error| {
                ReleaseCheckError::Transport(format!("request setup failed: {error}"))
            })?;
        let mut response = request
            .submit()
            .map_err(|error| ReleaseCheckError::Transport(format!("request failed: {error}")))?;
        let status = response.status();
        if status != 200 {
            return Err(ReleaseCheckError::HttpStatus(status));
        }
        let limit = channel.response_limit_bytes();
        let mut body = vec![0_u8; limit];
        let bytes_read = io::try_read_full(&mut response, &mut body).map_err(|error| {
            ReleaseCheckError::Transport(format!("response read failed: {}", error.0))
        })?;
        if bytes_read == body.len() {
            return Err(ReleaseCheckError::InvalidResponse(format!(
                "GitHub release response reached the {limit}-byte limit"
            )));
        }
        let text = core::str::from_utf8(&body[..bytes_read]).map_err(|error| {
            ReleaseCheckError::InvalidResponse(format!("response is not UTF-8: {error}"))
        })?;
        channel.parse_response(text)
    }

    /// Download one firmware binary and stream it straight into the
    /// inactive OTA slot. Never buffers the whole
    /// image: each chunk read from the HTTPS response is written directly
    /// to flash. Returns a display-ready error string on failure; the
    /// caller is responsible for staying on the previous slot, which is
    /// exactly what happens automatically since `EspOtaUpdate::complete()`
    /// (and therefore `esp_ota_set_boot_partition`) is only ever called
    /// after every byte has been written and accepted.
    ///
    /// Runs directly on the main task's own stack instead of spawning a
    /// short-lived worker thread, unlike every other operation in this
    /// module: this one writes flash, so (per `run_named_worker_in_psram`'s
    /// own doc comment) its stack cannot live in PSRAM, and by the time an
    /// install is actually requested, Wi-Fi, reader background warm-up and
    /// the OTA check's own TLS handshake have already fragmented internal
    /// SRAM below the 64 KiB a fresh worker thread's stack needs --
    /// confirmed in the field, spawning one here reliably failed with
    /// ENOMEM. `CONFIG_ESP_MAIN_TASK_STACK_SIZE` in `sdkconfig.defaults` is
    /// sized to already include this operation's budget, reserved once at
    /// boot before anything else can fragment it, instead of trying to
    /// find a fresh contiguous block for it under pressure later.
    pub fn install_update_on_main_task(download_url: String) -> Result<(), String> {
        install_update(&download_url)
    }

    fn install_update(download_url: &str) -> Result<(), String> {
        let http_config = HttpConfiguration {
            crt_bundle_attach: Some(sys::esp_crt_bundle_attach),
            timeout: Some(Duration::from_secs(OTA_INSTALL_HTTP_TIMEOUT_SECONDS)),
            // GitHub's `browser_download_url` redirects to a signed
            // Azure Blob Storage URL carrying a SAS-token query string
            // (observed ~1KB long: sp=/sv=/sr=/se=/sig=/jwt=... params).
            // esp_http_client's default 512-byte header/request buffer is
            // nowhere near enough for that once the redirect is followed,
            // and fails with "HTTP_CLIENT: Out of buffer" (ESP_FAIL) before
            // a single byte of the actual firmware image is read.
            buffer_size: Some(4096),
            buffer_size_tx: Some(4096),
            ..Default::default()
        };
        let connection = EspHttpConnection::new(&http_config)
            .map_err(|error| format!("HTTP connection init failed: {error}"))?;
        let mut client = HttpClient::wrap(connection);
        let headers = [("user-agent", OTA_USER_AGENT)];
        let request = client
            .request(Method::Get, download_url, &headers)
            .map_err(|error| format!("download request setup failed: {error}"))?;
        let mut response = request
            .submit()
            .map_err(|error| format!("download request failed: {error}"))?;
        let status = response.status();
        if status != 200 {
            return Err(format!("download HTTP status {status}"));
        }

        let mut ota = EspOta::new().map_err(|error| format!("OTA handle unavailable: {error}"))?;
        // Dropping `update` on any early return below aborts the write via
        // `EspOtaUpdate`'s own `Drop` (calls `esp_ota_abort`), so the
        // previous slot is never at risk from a failed download.
        let mut update = ota
            .initiate_update()
            .map_err(|error| format!("failed to open update slot: {error}"))?;

        let mut chunk = [0_u8; OTA_DOWNLOAD_CHUNK_BYTES];
        let mut total_bytes = 0_usize;
        loop {
            let read = io::try_read_full(&mut response, &mut chunk)
                .map_err(|error| format!("download read failed: {}", error.0))?;
            if read > 0 {
                update
                    .write(&chunk[..read])
                    .map_err(|error| format!("flash write failed: {error}"))?;
                total_bytes += read;
            }
            if read < chunk.len() {
                break;
            }
        }
        if total_bytes == 0 {
            return Err("download produced no data".into());
        }
        update
            .complete()
            .map_err(|error| format!("failed to finalize update: {error}"))?;
        info!("rustmix-wave=ota-install status=completed bytes={total_bytes}");
        Ok(())
    }

    /// Confirm the currently running app image is good, canceling any
    /// pending automatic rollback armed by
    /// `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`. Idempotent and cheap: safe
    /// to call once every boot, OTA or not (a factory/already-valid image
    /// simply gets a harmless no-op from ESP-IDF's own state machine).
    pub fn mark_running_slot_valid() {
        match EspOta::new().and_then(|mut ota| ota.mark_running_slot_valid()) {
            Ok(()) => info!("rustmix-wave=ota-self-test status=confirmed"),
            Err(error) => {
                warn!("rustmix-wave=ota-self-test status=mark-valid-failed error={error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        extract_first_bin_asset_url, extract_string_field, is_update_available,
        parse_release_response, OtaCheckState, ReleaseInfo, UpdateChannel,
    };

    #[test]
    fn detects_newer_semver_tag_with_leading_v() {
        assert!(is_update_available("1.2.0", "v1.3.0"));
        assert!(is_update_available("1.2.0", "1.2.1"));
        assert!(!is_update_available("1.2.0", "v1.2.0"));
        assert!(!is_update_available("1.2.0", "1.1.9"));
    }

    #[test]
    fn falls_back_to_string_comparison_for_non_semver_tags() {
        assert!(is_update_available("1.2.0", "nightly-2026-08-29"));
        assert!(!is_update_available(
            "nightly-2026-08-29",
            "nightly-2026-08-29"
        ));
    }

    #[test]
    fn ignores_prerelease_suffix_on_patch_component() {
        assert!(is_update_available("1.2.0", "v1.2.1-beta"));
        assert!(!is_update_available("1.2.1", "v1.2.1-beta"));
    }

    #[test]
    fn extracts_tag_name_field() {
        let json = "{\"tag_name\":\"v1.3.0\",\"name\":\"Wave v1.3.0\"}";
        assert_eq!(
            extract_string_field(json, "tag_name").as_deref(),
            Some("v1.3.0")
        );
    }

    #[test]
    fn extracts_first_bin_asset_skipping_checksum_assets() {
        let json = concat!(
            "{\"assets\":[",
            "{\"name\":\"SHA256SUMS\",\"browser_download_url\":\"https://example.com/SHA256SUMS\"},",
            "{\"name\":\"firmware.bin\",\"browser_download_url\":\"https://example.com/firmware.bin\"},",
            "{\"name\":\"firmware.elf\",\"browser_download_url\":\"https://example.com/firmware.elf\"}",
            "]}",
        );
        assert_eq!(
            extract_first_bin_asset_url(json).as_deref(),
            Some("https://example.com/firmware.bin")
        );
    }

    #[test]
    fn parses_full_release_response() {
        let json = concat!(
            "{",
            "\"tag_name\": \"v1.3.0\",",
            "\"assets\": [",
            "{\"browser_download_url\": \"https://example.com/rustmix-wave-v1.3.0.bin\"}",
            "]",
            "}",
        );
        assert_eq!(
            parse_release_response(json).unwrap(),
            ReleaseInfo {
                tag_name: "v1.3.0".into(),
                download_url: "https://example.com/rustmix-wave-v1.3.0.bin".into(),
            }
        );
    }

    #[test]
    fn parse_release_response_fails_without_bin_asset() {
        let json = "{\"tag_name\":\"v1.3.0\",\"assets\":[]}";
        assert!(parse_release_response(json).is_err());
    }

    #[test]
    fn pre_releases_follow_semver_precedence() {
        assert!(is_update_available("1.4.8", "v1.5.0-beta.1"));
        assert!(is_update_available("1.5.0-beta.1", "v1.5.0-beta.2"));
        assert!(is_update_available("1.5.0-beta.2", "v1.5.0-beta.10"));
        assert!(is_update_available("1.5.0-alpha.3", "v1.5.0-beta.1"));
        assert!(is_update_available("1.5.0-beta.3", "v1.5.0"));
        assert!(!is_update_available("1.5.0", "v1.5.0-beta.3"));
        assert!(!is_update_available("1.5.0-beta.2", "v1.5.0-beta.2"));
        // Back on stable, a beta device waits for a newer stable release.
        assert!(!is_update_available("1.5.0-beta.1", "v1.4.9"));
    }

    #[test]
    fn a_pre_release_version_belongs_to_the_beta_channel() {
        assert_eq!(UpdateChannel::of_version("1.4.8"), UpdateChannel::Stable);
        assert_eq!(
            UpdateChannel::of_version("1.5.0-beta.1"),
            UpdateChannel::Beta
        );
        assert_eq!(UpdateChannel::of_version("nightly"), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::Stable.toggled(), UpdateChannel::Beta);
        assert_eq!(UpdateChannel::Beta.toggled(), UpdateChannel::Stable);
    }

    #[test]
    fn the_channel_survives_a_save_and_a_load() {
        let dir =
            std::env::temp_dir().join(format!("rustmix-update-channel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("UPDATE.TXT");
        // Nothing saved yet: the channel of the running firmware.
        assert_eq!(
            UpdateChannel::load_from_path(&path, "1.4.8"),
            UpdateChannel::Stable
        );
        assert_eq!(
            UpdateChannel::load_from_path(&path, "1.5.0-beta.1"),
            UpdateChannel::Beta
        );
        UpdateChannel::Beta.save_to_path(&path).unwrap();
        assert_eq!(
            UpdateChannel::load_from_path(&path, "1.4.8"),
            UpdateChannel::Beta
        );
        UpdateChannel::Stable.save_to_path(&path).unwrap();
        assert_eq!(
            UpdateChannel::load_from_path(&path, "1.5.0-beta.1"),
            UpdateChannel::Stable
        );
        std::fs::write(&path, "channel=nightly\n").unwrap();
        assert_eq!(
            UpdateChannel::load_from_path(&path, "1.4.8"),
            UpdateChannel::Stable
        );
        assert_eq!(
            UpdateChannel::parse(" channel = Beta \n"),
            Some(UpdateChannel::Beta)
        );
        assert_eq!(
            UpdateChannel::parse("# comment\nchannel= BETA\n"),
            Some(UpdateChannel::Beta)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Shaped like GitHub's list: nested author and uploader objects, a body
    /// with braces and escaped quotes, a draft, a release without a `.bin`,
    /// and one release with its fields in a different order.
    const RELEASE_LIST: &str = concat!(
        "[{\"url\":\"https://api.github.com/repos/o/r/releases/3\",",
        "\"author\":{\"login\":\"o\",\"type\":\"User\"},",
        "\"tag_name\":\"v1.5.0-beta.2\",\"draft\":false,\"prerelease\":true,",
        "\"assets\":[",
        "{\"name\":\"x.sha256\",\"uploader\":{\"login\":\"o\"},",
        "\"browser_download_url\":\"https://example.com/b2.sha256\"},",
        "{\"name\":\"x.bin\",\"uploader\":{\"login\":\"o\"},",
        "\"browser_download_url\":\"https://example.com/b2.bin\"}],",
        "\"body\":\"Fixes {braces}, \\\"quotes\\\" and \\\"tag_name\\\": \\\"v9.9.9\\\"\"},",
        "{\"prerelease\":false,\"draft\":false,\"tag_name\":\"v1.4.9\",",
        "\"assets\":[{\"browser_download_url\":\"https://example.com/s149.bin\"}],\"body\":\"\"},",
        "{\"tag_name\":\"v1.6.0-beta.1\",\"draft\":true,\"prerelease\":true,",
        "\"assets\":[{\"browser_download_url\":\"https://example.com/draft.bin\"}]},",
        "{\"tag_name\":\"v1.5.0-beta.10\",\"draft\":false,\"prerelease\":true,\"assets\":[]}]",
    );

    #[test]
    fn the_beta_channel_takes_the_highest_published_release_with_a_bin() {
        let release = UpdateChannel::Beta.parse_response(RELEASE_LIST).unwrap();
        // v1.6.0-beta.1 is a draft and v1.5.0-beta.10 has no .bin.
        assert_eq!(
            release,
            ReleaseInfo {
                tag_name: "v1.5.0-beta.2".into(),
                download_url: "https://example.com/b2.bin".into(),
            }
        );
        let with_newer_stable = RELEASE_LIST.replace("v1.4.9", "v1.5.0");
        let release = UpdateChannel::Beta
            .parse_response(&with_newer_stable)
            .unwrap();
        assert_eq!(release.tag_name, "v1.5.0");
        assert!(UpdateChannel::Beta.parse_response("[]").is_err());
    }

    #[test]
    fn each_channel_reads_its_own_endpoint() {
        assert_eq!(UpdateChannel::Stable.releases_api_path(), "releases/latest");
        assert_eq!(
            UpdateChannel::Beta.releases_api_path(),
            "releases?per_page=5"
        );
        // The stable channel keeps reading the single-release response.
        let latest = "{\"tag_name\":\"v1.4.9\",\"assets\":[{\"browser_download_url\":\"https://example.com/s.bin\"}]}";
        assert_eq!(
            UpdateChannel::Stable
                .parse_response(latest)
                .unwrap()
                .tag_name,
            "v1.4.9"
        );
    }

    /// The live release list of the OTA repository, saved with
    /// `curl -s 'https://api.github.com/repos/<owner>/<repo>/releases?per_page=5'`.
    #[test]
    #[ignore = "needs RUSTMIX_RELEASES_JSON pointing at a saved GitHub release list"]
    fn real_release_list_smoke() {
        let path = std::env::var("RUSTMIX_RELEASES_JSON").unwrap();
        let json = std::fs::read_to_string(path).unwrap();
        let release = UpdateChannel::Beta.parse_response(&json).unwrap();
        println!("beta channel: {release:?}");
    }

    #[test]
    fn state_reports_update_availability() {
        let state = OtaCheckState::UpdateAvailable {
            version: "v1.3.0".into(),
            download_url: "https://example.com/x.bin".into(),
        };
        assert!(state.is_update_available());
        assert!(!OtaCheckState::UpToDate.is_update_available());
        assert!(!OtaCheckState::Checking.can_check());
        assert!(OtaCheckState::UpToDate.can_check());
    }
}
