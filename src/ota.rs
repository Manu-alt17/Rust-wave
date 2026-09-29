//! GitHub-release-based OTA update check, download and flash.
//!
//! The device compares its own [`crate::build_info::FIRMWARE_VERSION`]
//! against the latest GitHub release for
//! `build_info::OTA_REPO_OWNER`/`OTA_REPO_NAME`. No JSON crate is used: only
//! the two fields actually needed, `tag_name` and the first `.bin` asset URL, are pulled out of the
//! response with a small quoted-string scanner. ESP-IDF HTTPS wiring and
//! flashing live below `cfg(target_os = "espidf")`.

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

/// Compare the running firmware version against a release tag. Uses a
/// best-effort `major.minor.patch` parse (a leading `v`/`V` and any
/// non-numeric pre-release suffix on the patch component are ignored) when
/// both sides parse cleanly; otherwise falls back to a plain string
/// inequality check, which is good enough to flag "something changed"
/// without pretending to understand every possible tag format.
#[must_use]
pub fn is_update_available(current: &str, remote_tag: &str) -> bool {
    let remote = remote_tag.trim_start_matches(['v', 'V']);
    match (parse_semver(current), parse_semver(remote)) {
        (Some(current), Some(remote)) => remote > current,
        _ => !remote.is_empty() && remote != current,
    }
}

fn parse_semver(input: &str) -> Option<(u32, u32, u32)> {
    let mut parts = input.trim().splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch_field = parts.next().unwrap_or("0");
    let patch_digits: String = patch_field
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let patch = if patch_digits.is_empty() {
        0
    } else {
        patch_digits.parse().ok()?
    };
    Some((major, minor, patch))
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
            is_update_available, parse_release_response, OtaCheckState, ReleaseCheckError,
            ReleaseInfo, MAX_RELEASE_RESPONSE_BYTES, OTA_DOWNLOAD_CHUNK_BYTES,
        },
        runtime_worker::{poll_named_worker, spawn_named_worker_in_psram, NamedWorkerError},
    };

    /// Stack budget for the short-lived version-check worker, dominated by
    /// the TLS handshake.
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
    /// internal RAM. Safe here specifically because `fetch_latest_release`
    /// only does an HTTPS GET and a JSON scan -- it never touches
    /// flash/NVS/the OTA partition, unlike `install_update` below, which
    /// must keep the plain internal-RAM stack.
    pub fn spawn_latest_release_check(
    ) -> Result<Receiver<Result<ReleaseInfo, ReleaseCheckError>>, NamedWorkerError<ReleaseCheckError>>
    {
        spawn_named_worker_in_psram(
            "ota-check",
            OTA_CHECK_WORKER_STACK_BYTES,
            fetch_latest_release,
        )
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

    fn fetch_latest_release() -> Result<ReleaseInfo, ReleaseCheckError> {
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
            "https://api.github.com/repos/{OTA_REPO_OWNER}/{OTA_REPO_NAME}/releases/latest"
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
        let mut body = vec![0_u8; MAX_RELEASE_RESPONSE_BYTES];
        let bytes_read = io::try_read_full(&mut response, &mut body).map_err(|error| {
            ReleaseCheckError::Transport(format!("response read failed: {}", error.0))
        })?;
        if bytes_read == body.len() {
            return Err(ReleaseCheckError::InvalidResponse(format!(
                "GitHub release response reached the {MAX_RELEASE_RESPONSE_BYTES}-byte limit"
            )));
        }
        let text = core::str::from_utf8(&body[..bytes_read]).map_err(|error| {
            ReleaseCheckError::InvalidResponse(format!("response is not UTF-8: {error}"))
        })?;
        parse_release_response(text)
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
        parse_release_response, OtaCheckState, ReleaseInfo,
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
