//! Explicitly activated Wi-Fi portal: file transfer plus, when no Wi-Fi is
//! joined yet, the Wi-Fi setup that used to live in a separate "Configure via
//! phone" flow and its own standalone module, since folded into this one.
//!
//! The portal is intentionally dormant after boot. The user opens it from
//! the Home "Wi-Fi Transfer" tile (or the Settings > Network shortcut, which
//! is the exact same action). One `WifiTransferServer` instance and one
//! `PORTAL_HTML` frontend serve both purposes; only where it is reachable
//! differs: `start_lan` binds on the already-connected LAN address with no
//! radio changes, while `start_ap` is used instead when no Wi-Fi is joined
//! yet, bootstrapping the device's own hotspot first (via
//! `NetworkRuntime::start_provisioning`) so the portal -- Wi-Fi setup tab
//! included -- is reachable there instead. The portal asks for no code:
//! reaching its address while it is open is enough, and only requests sent
//! by another site's page are refused (`is_same_origin_request`). The
//! target-specific HTTP server
//! owns its own ESP-IDF task while the main loop retains display, routing
//! and sleep ownership.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::regional::Locale;

/// Portal root.  Browser requests may never escape this subtree.
pub const WIFI_TRANSFER_ROOT: &str = "/sdcard/RUSTMIX";
/// The ESP-IDF HTTP server task exists only while transfer mode is enabled.
pub const WIFI_TRANSFER_SERVER_STACK_BYTES: usize = 24 * 1024;
/// Upload and download bodies stream through one bounded scratch buffer.
pub const WIFI_TRANSFER_STREAM_CHUNK_BYTES: usize = 4 * 1024;
/// Keep accidental huge uploads bounded for the first portal slice.
pub const WIFI_TRANSFER_MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
/// Bound decoded browser paths before touching removable storage.
pub const WIFI_TRANSFER_MAX_PATH_BYTES: usize = 128;
/// Stop a forgotten transfer portal after ten minutes without HTTP traffic
/// when reachable on the LAN. The hotspot-bootstrap case uses the shorter
/// [`NETWORK_PROVISION_INACTIVITY_SECONDS`] instead, so an unattended open
/// hotspot does not linger as long.
pub const WIFI_TRANSFER_INACTIVITY_SECONDS: u64 = 10 * 60;
/// The first slice uses the conventional LAN HTTP port.
pub const WIFI_TRANSFER_HTTP_PORT: u16 = 80;
/// Bound one directory listing to protect the HTTP task heap.
pub const WIFI_TRANSFER_MAX_DIRECTORY_ROWS: usize = 256;
/// Stop a forgotten hotspot-bootstrap session after five minutes without HTTP
/// traffic, so the open (to anyone who captured the QR code) hotspot does not
/// stay broadcasting indefinitely.
pub const NETWORK_PROVISION_INACTIVITY_SECONDS: u64 = 5 * 60;
/// Re-scan for nearby networks this often while the hotspot-bootstrap portal
/// is open and no join attempt is in flight, so the phone's list stays
/// reasonably fresh. The portal page's `wifiLoadScan` polling interval
/// (in the embedded JS below) is kept matched to this, so the phone isn't
/// re-fetching `/api/scan` faster than the on-device scan actually changes.
pub const NETWORK_PROVISION_RESCAN_SECONDS: u64 = 12;

/// User request handed from the hardware-independent UI into `main.rs`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTransferUiRequest {
    Start,
    Stop,
}

/// Outcome of a phone/browser-submitted network, drained by `main.rs` once
/// per main-loop iteration and applied to `NetworkRuntime`/`WIFI.TXT`. Only
/// produced while the portal is reachable via the bootstrap hotspot (see the
/// module docs); joining a *new* network while already on a LAN would
/// require dropping that LAN connection first, so the Wi-Fi tab does not
/// offer it there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingJoinRequest {
    pub ssid: String,
    pub password: String,
}

/// Live outcome of the most recent phone/browser-submitted network, as
/// reported back to the on-device screen. `Testing` is shown while
/// `NetworkRuntime` is attempting the real connection required before
/// persisting.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum JoinAttemptState {
    #[default]
    Idle,
    Testing {
        ssid: String,
    },
    Succeeded {
        ssid: String,
    },
    Failed {
        ssid: String,
        error: String,
    },
}

/// Compact UI-facing lifecycle state.  This snapshot never contains a Wi-Fi
/// password and remains safe to render or log.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WifiTransferState {
    #[default]
    Off,
    Starting,
    Ready,
    Failed,
}

impl WifiTransferState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Starting => "STARTING",
            Self::Ready => "READY",
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
                Self::Off => "SPENTO",
                Self::Starting => "AVVIO",
                Self::Ready => "PRONTO",
                Self::Failed => "FALLITO",
            },
        }
    }
}

/// Small main-loop snapshot updated when the server starts, stops or handles a
/// completed filesystem request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiTransferSnapshot {
    pub state: WifiTransferState,
    pub url: Option<String>,
    /// `Some` only while reachable via the bootstrap hotspot instead of an
    /// already-joined LAN (see the module docs); drives which card the
    /// on-device screen and the portal's Wi-Fi tab show.
    pub ap_ssid: Option<String>,
    pub ap_password: Option<String>,
    /// Phones on the hotspot right now (always 0 on the LAN path). Filled
    /// in by the main loop, which owns the Wi-Fi driver; the screen uses it
    /// to say what to do next on the phone once one has joined.
    pub hotspot_clients: u16,
    pub join: JoinAttemptState,
    pub last_action: String,
    pub last_bytes: usize,
    pub error: Option<String>,
}

impl Default for WifiTransferSnapshot {
    fn default() -> Self {
        Self {
            state: WifiTransferState::Off,
            url: None,
            ap_ssid: None,
            ap_password: None,
            hotspot_clients: 0,
            join: JoinAttemptState::Idle,
            last_action: "Portal is off".into(),
            last_bytes: 0,
            error: None,
        }
    }
}

impl WifiTransferSnapshot {
    #[must_use]
    pub fn starting() -> Self {
        Self {
            state: WifiTransferState::Starting,
            last_action: "Starting portal".into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            state: WifiTransferState::Failed,
            last_action: "Portal start failed".into(),
            error: Some(error.into()),
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(
            self.state,
            WifiTransferState::Starting | WifiTransferState::Ready
        )
    }

    #[must_use]
    pub fn url_label(&self) -> &str {
        self.url.as_deref().unwrap_or("--")
    }
}

/// Whether a request that changes something (upload, delete, rename, Wi-Fi
/// setup) may be served, going by its `Origin` and `Host` headers.
///
/// The portal asks for no code: whoever reaches its address while it is open
/// may use it. What must still be refused is a request the user never made:
/// any web page open in a browser on the same network can make that browser
/// send a POST to this address. A browser states the page such a request
/// comes from in `Origin`, so one naming another site is refused. A request
/// without `Origin` (not sent by a page's script) is served.
#[must_use]
pub fn is_same_origin_request(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|origin| !origin.is_empty()) else {
        return true;
    };
    let Some(host) = host.map(str::trim).filter(|host| !host.is_empty()) else {
        return false;
    };
    origin
        .strip_prefix("http://")
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

/// Resolve one portal path, as it arrives from [`query_value`] (already
/// percent-decoded, and decoded only once), beneath `/sdcard/RUSTMIX`.
/// Rejects traversal, absolute and overlong paths, names the SD card cannot
/// store as typed, and the protected configuration files -- judged on the
/// path actually resolved, so no other spelling of one (`./WIFI.TXT`,
/// `wifi.txt`, `WIFI.TXT/`, `WIFI.TXT.`) gets past the check, and a
/// twice-encoded name is not decoded a second time here.
pub fn resolve_portal_path(relative: &str) -> Result<PathBuf, &'static str> {
    if relative.len() > WIFI_TRANSFER_MAX_PATH_BYTES {
        return Err("path exceeds portal limit");
    }
    let mut safe = PathBuf::from(WIFI_TRANSFER_ROOT);
    let mut resolved = Vec::new();
    for component in Path::new(relative.trim_start_matches('/')).components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str().ok_or("path is not UTF-8")?;
                if !is_sd_safe_name(name) {
                    return Err("name not allowed on the SD card");
                }
                resolved.push(name);
                safe.push(name);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path traversal is blocked")
            }
        }
    }
    if is_protected_portal_path(&resolved.join("/")) {
        return Err("protected configuration file");
    }
    Ok(safe)
}

/// Configuration files stay hidden and cannot be modified by the initial LAN
/// portal.  This prevents accidental credential disclosure or live config
/// replacement while services are running. FAT names are case-insensitive,
/// and empty or `.` components name nothing, so neither counts.
#[must_use]
pub fn is_protected_portal_path(relative: &str) -> bool {
    let canonical = relative
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_ascii_uppercase)
        .collect::<Vec<_>>()
        .join("/");
    matches!(canonical.as_str(), "WIFI.TXT" | "CLOCK.TXT" | "DISPLAY.TXT")
}

/// One file or folder name the SD card's FAT filesystem (long names on)
/// stores exactly as typed: up to 255 characters, none of the ones FAT
/// forbids, and no trailing dot or space -- FAT silently drops those, so
/// `WIFI.TXT.` would open `WIFI.TXT`.
#[must_use]
pub fn is_sd_safe_name(component: &str) -> bool {
    !component.is_empty()
        && component.chars().count() <= 255
        && !component.ends_with(['.', ' '])
        && !component
            .chars()
            .any(|character| character.is_control() || "\\/:*?\"<>|".contains(character))
}

/// The sleep screen as the page asks for it, for the runtime owner in
/// main.rs to carry out: it owns the display settings and the catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SleepScreenRequest {
    /// What standby is to show.
    pub mode: crate::app::display::SleepScreenMode,
    /// With [`SleepScreenMode::Fixed`], the wallpaper of `/SLEEP` to keep;
    /// `None` keeps the one standby shows now.
    ///
    /// [`SleepScreenMode::Fixed`]: crate::app::display::SleepScreenMode::Fixed
    pub fixed: Option<String>,
}

/// Whether `name` can be a sleep wallpaper's file name: one name, not a
/// path, ending in `.bmp`.
#[must_use]
pub fn is_sleep_image_name(name: &str) -> bool {
    is_sd_safe_name(name)
        && name.rsplit_once('.').is_some_and(|(stem, extension)| {
            !stem.is_empty() && extension.eq_ignore_ascii_case("bmp")
        })
}

/// The sleep screen as `/api/status` tells it to the page: the Display
/// setting's marker and the wallpaper standby shows now, which is the fixed
/// one when the mode is `fixed`.
#[must_use]
pub fn sleep_status_json(mode: &str, fixed: Option<&str>) -> String {
    let escape = |value: &str| -> String {
        value
            .chars()
            .filter(|character| !character.is_control())
            .flat_map(|character| match character {
                '"' | '\\' => vec!['\\', character],
                other => vec![other],
            })
            .collect()
    };
    format!(
        "{{\"mode\":\"{}\",\"fixed\":\"{}\"}}",
        escape(mode),
        escape(fixed.unwrap_or(""))
    )
}

/// Media type a downloaded file is declared as, from its extension. Without
/// one the HTTP server calls everything `text/html`, and a browser then shows
/// the file as a page instead of saving it.
#[must_use]
pub fn download_content_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "epub" => "application/epub+zip",
        "txt" | "log" | "old" => "text/plain; charset=utf-8",
        "bmp" => "image/bmp",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

/// `Content-Disposition` value that makes a browser save a download under
/// the file's own name. The plain `filename` carries an ASCII stand-in for
/// old clients; `filename*` (RFC 6266) carries the real name, percent-encoded
/// as UTF-8, and is the one current browsers use.
#[must_use]
pub fn download_content_disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() && !matches!(character, '"' | '\\' | '%' | ';') {
                character
            } else if character == ' ' {
                ' '
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::with_capacity(name.len() * 3);
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&+^`|".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

/// Whether the page is shown in English: the device's own language, set by
/// the runtime owner in main.rs each time the portal starts.
static PORTAL_ENGLISH: AtomicBool = AtomicBool::new(false);

/// Tell the page which language the device is in.
pub fn set_portal_locale(locale: Locale) {
    PORTAL_ENGLISH.store(matches!(locale, Locale::English), Ordering::Relaxed);
}

/// The page's language as `/api/status` reports it.
#[must_use]
pub fn portal_locale_code() -> &'static str {
    if PORTAL_ENGLISH.load(Ordering::Relaxed) {
        "en"
    } else {
        "it"
    }
}

/// Whether `path` is the card's own top folder, as [`resolve_portal_path`]
/// returns it for `/`. Deleting a folder with what it holds stops here: the
/// top folder holds the settings and everything else.
#[must_use]
pub fn is_portal_root(path: &Path) -> bool {
    path == Path::new(WIFI_TRANSFER_ROOT)
}

/// Tiny query parser used by the portal API.  The firmware intentionally avoids
/// allocating a generic web framework.
#[must_use]
pub fn query_value(uri: &str, key: &str) -> Option<String> {
    let query = uri.split_once('?')?.1;
    query.split('&').find_map(|part| {
        let (candidate, value) = part.split_once('=')?;
        (candidate == key)
            .then(|| percent_decode(value).ok())
            .flatten()
    })
}

fn percent_decode(value: &str) -> Result<String, &'static str> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let high = hex(bytes[index + 1]).ok_or("invalid percent escape")?;
                let low = hex(bytes[index + 2]).ok_or("invalid percent escape")?;
                output.push((high << 4) | low);
                index += 3;
            }
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).map_err(|_| "path is not UTF-8")
}

const fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(target_os = "espidf")]
pub mod espidf {
    use std::{
        ffi::CString,
        fs::{self, File},
        io::{Read as StdRead, Write as StdWrite},
        path::Path,
        sync::{Arc, Mutex},
        time::{Duration, Instant, UNIX_EPOCH},
    };

    use anyhow::{anyhow, bail, Context, Result};
    use embedded_svc::{http::Method, io::Write as _};
    use esp_idf_svc::{
        http::server::{Configuration, EspHttpServer},
        sys,
    };
    use log::{info, warn};

    use crate::app::display::SleepScreenMode;
    use crate::cover_cache::{CachedThumbnail, CoverCache};
    use crate::dns_captive_portal::espidf::CaptivePortalDns;
    use crate::network_scan::WifiScanEntry;
    use crate::reader::{
        book_format_from_path, scan_txt_library, ReaderBook, READER_BOOKS_DIRECTORY,
    };
    use crate::storage::SD_MOUNT_POINT;

    use super::{
        download_content_disposition, download_content_type, is_portal_root,
        is_protected_portal_path, is_same_origin_request, is_sleep_image_name, portal_locale_code,
        query_value, resolve_portal_path, sleep_status_json, JoinAttemptState, PendingJoinRequest,
        SleepScreenRequest, WifiTransferSnapshot, WifiTransferState,
        NETWORK_PROVISION_INACTIVITY_SECONDS, WIFI_TRANSFER_HTTP_PORT,
        WIFI_TRANSFER_INACTIVITY_SECONDS, WIFI_TRANSFER_MAX_DIRECTORY_ROWS,
        WIFI_TRANSFER_MAX_UPLOAD_BYTES, WIFI_TRANSFER_ROOT, WIFI_TRANSFER_SERVER_STACK_BYTES,
        WIFI_TRANSFER_STREAM_CHUNK_BYTES,
    };

    const PORTAL_HTML: &str = r##"<!doctype html>
<html lang="it"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Rustmix Wave</title><style>
*{box-sizing:border-box}
:root{--bg:#f5f6f8;--card:#fff;--text:#1a1d23;--muted:#6b7280;--border:#e2e5ea;--accent:#2563eb;--accent-dark:#1d4ed8;--accent-soft:#e8effd;--danger:#dc2626;--ok:#16a34a;--radius:10px}
@media (prefers-color-scheme:dark){:root{--bg:#14161a;--card:#1c1f26;--text:#e8eaed;--muted:#9aa3b2;--border:#2a2e37;--accent:#3b82f6;--accent-dark:#60a5fa;--accent-soft:#1e2a44;--danger:#f87171;--ok:#4ade80}}
html{-webkit-text-size-adjust:100%}
body{margin:0;background:var(--bg);color:var(--text);font-family:-apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;font-size:16px}
[hidden]{display:none!important}
button,input{font:inherit;color:inherit}
svg{width:22px;height:22px;flex:none;stroke:currentColor;fill:none;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round}
h2{font-size:1rem;margin:0 0 .5rem}
a{color:var(--accent)}
.top{display:flex;align-items:center;gap:.5rem;padding:.6rem 1rem;background:var(--card);border-bottom:1px solid var(--border)}
.top h1{font-size:1.05rem;margin:0;flex:1;white-space:nowrap;min-width:0;overflow:hidden;text-overflow:ellipsis}
.pill{font-size:.75rem;color:var(--muted);border:1px solid var(--border);border-radius:999px;padding:.25rem .6rem;white-space:nowrap}
.pill b{font-weight:600}
.only-wide{display:none}
@media (min-width:480px){.only-wide{display:inline}}
.pill.on b{color:var(--ok)}
.pill.off{border-color:var(--danger);color:var(--danger)}
.banner{background:var(--card);border-bottom:2px solid var(--danger);padding:.7rem 1rem;display:flex;gap:.8rem;align-items:center;font-size:.9rem}
.banner span{flex:1}
.tabs{display:flex;background:var(--card);border-bottom:1px solid var(--border);overflow-x:auto}
.tab{flex:1;min-width:max-content;border:0;background:none;padding:.8rem .55rem;min-height:48px;color:var(--muted);border-bottom:3px solid transparent;cursor:pointer}
.tab.active{color:var(--accent);border-bottom-color:var(--accent);font-weight:600}
.wrap{padding:.8rem;max-width:1180px;margin:0 auto}
.card{background:var(--card);border:1px solid var(--border);border-radius:var(--radius);padding:.9rem;margin-bottom:.8rem}
.card.flush{padding:0;overflow:hidden}
.btn{border:1px solid var(--border);background:var(--card);border-radius:8px;padding:0 .9rem;min-height:44px;cursor:pointer;display:inline-flex;align-items:center;justify-content:center;gap:.4rem;text-decoration:none;color:var(--text);white-space:nowrap}
.btn:hover{border-color:var(--accent)}
.btn.primary{background:var(--accent);border-color:var(--accent);color:#fff;font-weight:600}
.btn.primary:hover{background:var(--accent-dark)}
.btn.danger{color:var(--danger);border-color:var(--danger)}
.btn.icon{width:44px;padding:0}
.btn.small{min-height:40px;padding:0 .7rem;font-size:.85rem}
.btn[disabled]{opacity:.4;cursor:default}
input[type=text],input[type=search],input[type=password]{background:transparent;border:1px solid var(--border);border-radius:8px;padding:0 .8rem;min-height:44px;min-width:0}
.hint{font-size:.8rem;color:var(--muted);margin:.4rem 0}
.row-flex{display:flex;gap:.5rem;align-items:center;flex-wrap:wrap}
.grow{flex:1;min-width:0}
.drop{border:2px dashed var(--border);border-radius:var(--radius);padding:1.2rem;text-align:center;color:var(--muted);cursor:pointer}
.drop.drag,.dragover{border-color:var(--accent)!important;color:var(--accent)}
.queue{display:flex;flex-direction:column;gap:.5rem}
.queue:not(:empty){margin-top:.8rem}
.qitem{display:flex;align-items:center;gap:.6rem;font-size:.85rem}
.qitem .name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.qitem .bar{flex:1;height:6px;background:var(--border);border-radius:3px;overflow:hidden}
.qitem .bar>i{display:block;height:100%;background:var(--accent);width:0%}
.qitem.done .bar>i{background:var(--ok)}
.qitem.error{flex-wrap:wrap}
.qitem.error .bar{display:none}
.qitem.error .state{flex-basis:100%;color:var(--danger)}
.qitem .state{color:var(--muted);font-size:.8rem}
.book-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(140px,1fr));gap:.8rem}
.book-card{border:1px solid var(--border);border-radius:var(--radius);padding:.6rem;display:flex;flex-direction:column;gap:.4rem;min-width:0}
.book-cover{position:relative;aspect-ratio:208/252;background:var(--bg);border-radius:6px;overflow:hidden;display:flex;align-items:center;justify-content:center;border:1px solid var(--border)}
.book-cover img{width:100%;height:100%;object-fit:cover;image-rendering:pixelated}
.book-cover.empty img{display:none}
.book-cover.empty::after{content:attr(data-empty);color:var(--muted);font-size:.7rem;padding:.5rem;text-align:center}
.book-title{font-size:.85rem;font-weight:600;overflow:hidden;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical}
.card-actions{margin-top:auto;display:flex;gap:.4rem;flex-wrap:wrap}
.card-actions .btn{flex:1;min-height:40px;padding:0 .4rem;font-size:.8rem}
.card-actions .btn.wide{flex-basis:100%}
.book-card.fixed{border-color:var(--accent);box-shadow:0 0 0 1px var(--accent)}
.fixed-badge{font-size:.8rem;font-weight:600;color:var(--accent)}
.choice{display:flex;align-items:center;gap:.6rem;min-height:44px;cursor:pointer}
.choice input{width:20px;height:20px;margin:0;flex:none;accent-color:var(--accent)}
/* lists: files, audiobooks, wifi */
.rows{border-top:1px solid var(--border)}
.row{display:flex;align-items:center;gap:.7rem;padding:0 .4rem 0 .8rem;min-height:56px;border-bottom:1px solid var(--border);cursor:pointer;user-select:none;-webkit-user-select:none}
.row:last-child{border-bottom:0}
.row.sel{background:var(--accent-soft)}
.row.sys{opacity:.6}
.row:focus-visible{outline:2px solid var(--accent);outline-offset:-2px}
.row .ic{color:var(--accent);display:flex}
.row .ic.file{color:var(--muted)}
.row .nm{flex:1;min-width:0}
.row .nm div{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.row .meta{font-size:.8rem;color:var(--muted)}
.row .col{display:none;font-size:.85rem;color:var(--muted);white-space:nowrap}
.more{width:44px;height:44px;border:0;background:none;border-radius:8px;display:flex;align-items:center;justify-content:center;color:var(--muted);cursor:pointer;flex:none}
.more:hover{background:var(--bg)}
.check{width:22px;height:22px;border:2px solid var(--border);border-radius:6px;flex:none;display:none;align-items:center;justify-content:center;color:#fff}
.check svg{width:16px;height:16px;stroke-width:3}
.row.sel .check{background:var(--accent);border-color:var(--accent)}
.selmode .check{display:flex;border-color:var(--accent)}
.empty-note{padding:1.4rem .9rem;color:var(--muted);text-align:center;font-size:.9rem}
.bar{display:flex;align-items:center;gap:.5rem;padding:.5rem}
.bar+.bar{padding-top:0}
.path{flex:1;display:flex;align-items:center;gap:.25rem;min-height:44px;border:1px solid var(--border);border-radius:8px;padding:0 .7rem;overflow:hidden;white-space:nowrap;min-width:0}
.path span{color:var(--muted)}
.path button{border:0;background:none;padding:.4rem .1rem;cursor:pointer;color:var(--text)}
.path button:last-child{font-weight:600}
.sys-line{display:flex;align-items:center;gap:.5rem;padding:.6rem .8rem;color:var(--muted);font-size:.85rem;border-top:1px solid var(--border)}
.sys-line span{flex:1}
.sys-line button{border:0;background:none;color:var(--accent);min-height:40px;cursor:pointer}
.side,.thead,.statusbar{display:none}
.selbar{position:fixed;left:.6rem;right:.6rem;bottom:.6rem;max-width:560px;margin:0 auto;background:#1a1d23;color:#fff;border-radius:12px;padding:.4rem .5rem .4rem .9rem;display:flex;align-items:center;gap:.2rem;box-shadow:0 6px 24px rgba(0,0,0,.3);z-index:20}
.selbar span{flex:1;font-size:.9rem}
.selbar button{background:none;border:0;color:#fff;min-height:48px;min-width:52px;padding:0 .5rem;border-radius:8px;display:flex;flex-direction:column;align-items:center;justify-content:center;font-size:.7rem;gap:2px;cursor:pointer}
.selbar button.del{color:#fca5a5}
.menu{position:fixed;background:var(--card);border:1px solid var(--border);border-radius:10px;box-shadow:0 8px 28px rgba(0,0,0,.22);padding:.3rem;min-width:200px;z-index:40}
.menu button{display:flex;align-items:center;gap:.6rem;min-height:44px;padding:0 .7rem;border-radius:7px;border:0;background:none;width:100%;text-align:left;cursor:pointer}
.menu button:hover{background:var(--bg)}
.menu button.del{color:var(--danger)}
.menu hr{border:0;border-top:1px solid var(--border);margin:.3rem 0}
.menu .lab{font-size:.7rem;text-transform:uppercase;letter-spacing:.04em;color:var(--muted);padding:.4rem .7rem .1rem}
#toast{position:fixed;left:.6rem;right:.6rem;margin:0 auto;width:fit-content;max-width:min(560px,calc(100% - 1.2rem));bottom:4.6rem;background:#1a1d23;color:#fff;border-radius:10px;padding:.7rem .9rem;display:flex;gap:.8rem;align-items:center;font-size:.9rem;z-index:50;box-shadow:0 6px 24px rgba(0,0,0,.3)}
#toast.error{background:#7f1d1d}
#toast button{background:none;border:0;color:#93c5fd;font-weight:600;min-height:40px;padding:0 .4rem;cursor:pointer;white-space:nowrap}
#shade{position:fixed;inset:0;background:rgba(0,0,0,.45);z-index:30;display:flex;align-items:flex-end;justify-content:center}
.sheet{background:var(--card);border-radius:14px 14px 0 0;padding:1rem;width:100%;max-width:520px;max-height:88vh;overflow:auto}
.sheet h2{font-size:1.05rem}
.sheet input[type=text],.sheet input[type=password]{width:100%;margin:.3rem 0}
.sheet .actions{display:flex;gap:.5rem;margin-top:.9rem}
.sheet .actions .btn{flex:1}
.sheet pre{white-space:pre-wrap;word-break:break-word;font-size:.8rem;background:var(--bg);border-radius:8px;padding:.7rem;max-height:50vh;overflow:auto;margin:.4rem 0}
.sheet .preview{max-width:100%;max-height:55vh;display:block;margin:.4rem auto;border:1px solid var(--border)}
.pick{border:1px solid var(--border);border-radius:8px;max-height:45vh;overflow:auto;margin:.5rem 0}
.pick .row{min-height:48px}
/* wallpapers */
.stage{display:none}
.stage.show{display:block}
.device-frame{width:min(100%,250px);margin:.8rem auto .4rem;border:2px solid var(--text);border-radius:14px;padding:9px;background:var(--bg)}
.crop-frame{position:relative;overflow:hidden;width:100%;aspect-ratio:3/5;background:#000;border:1px solid var(--muted);touch-action:none;cursor:grab}
.crop-frame:active{cursor:grabbing}
.crop-frame img{position:absolute;top:0;left:0;transform-origin:top left;user-select:none;-webkit-user-drag:none;max-width:none}
.crop-frame canvas{position:absolute;top:0;left:0;width:100%;height:100%}
.slider-row{display:flex;align-items:center;gap:.6rem;margin:.5rem 0;font-size:.85rem}
.slider-row input[type=range]{flex:1}
.seg{display:flex;justify-content:center;margin:.6rem 0}
.seg button{border:1px solid var(--border);background:var(--card);min-height:40px;padding:0 .9rem;cursor:pointer}
.seg button:first-child{border-radius:8px 0 0 8px}
.seg button:last-child{border-radius:0 8px 8px 0;border-left:none}
.seg button.on{background:var(--accent);border-color:var(--accent);color:#fff;font-weight:600}
.stage-actions{display:flex;gap:.5rem}
.stage-actions .btn{flex:1}
.sleep-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(140px,1fr));gap:.8rem}
.sleep-thumb{aspect-ratio:3/5;background:var(--bg);border-radius:6px;overflow:hidden;border:1px solid var(--border);display:flex;align-items:center;justify-content:center}
.sleep-thumb canvas{width:100%;height:100%;display:block}
.sleep-thumb.empty::after{content:attr(data-empty);color:var(--muted);font-size:.7rem;padding:.5rem;text-align:center}
/* wifi */
.net{display:flex;flex-wrap:wrap;align-items:center;gap:.5rem;padding:.6rem 0;border-bottom:1px solid var(--border)}
.net:last-child{border-bottom:0}
.net .name{flex:1 1 60%;min-width:0;overflow-wrap:anywhere;font-weight:600;display:flex;align-items:center;gap:.5rem}
.bars{display:inline-flex;align-items:flex-end;gap:2px;height:16px}
.bars i{width:4px;background:var(--border);border-radius:1px}
.bars i.on{background:var(--text)}
.pw{display:flex;gap:.5rem}
.pw input{flex:1}
@media (min-width:900px){
  .wrap{padding:1rem}
  #shade{align-items:center}
  .sheet{border-radius:14px}
  .explorer{display:grid;grid-template-columns:230px minmax(0,1fr);border-top:1px solid var(--border);min-height:420px}
  .side{display:block;border-right:1px solid var(--border);padding:.5rem}
  .side button{display:flex;align-items:center;gap:.6rem;padding:.45rem .6rem;border-radius:7px;font-size:.92rem;border:0;background:none;width:100%;text-align:left;cursor:pointer;min-height:36px}
  .side button.on{background:var(--accent-soft);color:var(--accent);font-weight:600}
  .side .lab{font-size:.7rem;text-transform:uppercase;letter-spacing:.04em;color:var(--muted);padding:.7rem .6rem .25rem}
  .side .sub{padding-left:1.6rem;color:var(--muted)}
  .rows{border-top:0}
  .thead{display:flex;align-items:center;gap:.7rem;padding:0 .4rem 0 .8rem;border-bottom:1px solid var(--border);min-height:36px}
  .thead button{border:0;background:none;font-size:.75rem;font-weight:600;color:var(--muted);cursor:pointer;padding:.4rem 0;text-align:left}
  .thead .nm{flex:1}
  .row{min-height:42px;cursor:default}
  .row .meta{display:none}
  .row .col,.thead .col{display:block}
  .c-date{width:150px}.c-type{width:120px}.c-size{width:80px}
  .check{display:flex}
  .row .more{width:36px;height:36px}
  .thead .sp-check{width:22px;flex:none}.thead .sp-ic{width:22px;flex:none}.thead .sp-more{width:36px;flex:none}
  .statusbar{display:flex;justify-content:space-between;gap:1rem;padding:.5rem .8rem;border-top:1px solid var(--border);font-size:.8rem;color:var(--muted)}
  .sys-line{display:none}
  .only-phone{display:none!important}
  .dropzone-desktop{display:block}
}
.dropzone-desktop{display:none;margin:1rem;border:2px dashed var(--border);border-radius:10px;padding:1.2rem;text-align:center;color:var(--muted)}
@media (max-width:899px){.only-desktop{display:none!important}}
</style></head><body>
<header class="top">
<h1>Rustmix Wave</h1>
<span class="pill" id="linkPill"><b>&#9679;</b> <span id="linkText">...</span></span>
<span class="pill" id="space"><span id="spaceNum">--</span><span class="only-wide" id="spaceWord"></span></span>
<button class="btn icon small" id="reloadBtn" onclick="refreshActiveTab()" aria-label="Ricarica" data-en-label="Reload"></button>
</header>
<div class="banner" id="offline" hidden><span data-en="The device does not answer: usually because its Upload, Wi-Fi screen was closed. This page works only while that screen is open. Open it again on the device, then tap Retry.">Il dispositivo non risponde: di solito &egrave; perch&eacute; la schermata Carica, Wi-Fi &egrave; stata chiusa. Questa pagina funziona solo mentre quella schermata &egrave; aperta. Riaprila sul dispositivo, poi tocca Riprova.</span><button class="btn small" onclick="retryLink()" data-en="Retry">Riprova</button></div>
<nav class="tabs" id="tabs">
<button class="tab" data-tab="books" onclick="showTab('books')" data-en="Books">Libri</button>
<button class="tab" data-tab="audio" onclick="showTab('audio')" data-en="Audiobooks">Audiolibri</button>
<button class="tab" data-tab="wallpaper" onclick="showTab('wallpaper')" data-en="Wallpapers">Sfondi</button>
<button class="tab" data-tab="files" onclick="showTab('files')" data-en="Files">File</button>
<button class="tab" data-tab="wifi" onclick="showTab('wifi')">Wi-Fi</button>
</nav>
<div class="wrap" id="app">

<section id="tab-books" class="tabpanel" hidden>
<div class="card">
<div class="drop" id="dropBooks" onclick="document.getElementById('fileBooks').click()" data-en="Tap to choose your books (EPUB or TXT), or drag them here">Tocca per scegliere i tuoi libri (EPUB o TXT), oppure trascinali qui</div>
<input id="fileBooks" type="file" multiple accept=".epub,.txt" hidden>
<div class="queue" id="queueBooks"></div>
</div>
<div class="card">
<div class="row-flex" style="margin-bottom:.8rem"><input class="grow" id="bookSearch" type="search" placeholder="Cerca nei tuoi libri..." data-en-ph="Search your books..." oninput="renderBooks()"><span class="hint" id="bookCount"></span></div>
<div class="book-grid" id="bookGrid"></div>
</div>
</section>

<section id="tab-audio" class="tabpanel" hidden>
<div class="card">
<div class="drop" id="dropAudio" onclick="document.getElementById('fileAudio').click()" data-en="Tap to choose the MP3 files of an audiobook, or drag them here">Tocca per scegliere i file MP3 di un audiolibro, oppure trascinali qui</div>
<input id="fileAudio" type="file" multiple accept=".mp3,audio/mpeg" hidden>
<p class="hint" data-en="One file is one audiobook. Several files together become one title, and its tracks play in name order. Files over 64 MB go by USB cable.">Un file solo &egrave; un audiolibro. Pi&ugrave; file insieme diventano un solo titolo, con le tracce in ordine di nome. I file oltre 64 MB si copiano con il cavo USB.</p>
<div class="queue" id="queueAudio"></div>
</div>
<div class="card flush">
<h2 style="padding:.9rem .9rem 0" data-en="Audiobooks on the device">Audiolibri sul dispositivo</h2>
<div class="rows" id="audioRows" style="border-top:0"></div>
</div>
</section>

<section id="tab-wallpaper" class="tabpanel" hidden>
<div class="card" id="wallpaper">
<h2 data-en="New wallpaper">Nuovo sfondo</h2>
<div id="bgPick">
<p class="hint" data-en="The picture that stays on the screen while the device is in standby. It is saved in black and white.">&Egrave; l&apos;immagine che resta sullo schermo quando il dispositivo &egrave; in standby. Viene salvata in bianco e nero.</p>
<div class="row-flex" style="margin-top:.6rem">
<button class="btn grow" id="bgChoose" onclick="document.getElementById('fileBg').click()"></button>
<button class="btn grow only-desktop" onclick="toast(L('Copia un\'immagine in un altro sito, poi premi Ctrl+V su questa pagina.','Copy a picture on another site, then press Ctrl+V on this page.'))" id="bgPaste"></button>
</div>
<input id="fileBg" type="file" accept="image/*" hidden>
<p class="hint" id="bgGoogle"><span data-en="No picture at hand? ">Non hai un&apos;immagine pronta? </span><a href="https://www.google.com/imghp" target="_blank" rel="noopener" data-en="Look for one on Google">Cercala su Google</a><span data-en=": it opens in another tab. Save the picture you like, then choose it here.">: si apre in un&apos;altra scheda. Salva l&apos;immagine che ti piace, poi sceglila qui.</span></p>
</div>
<div class="stage" id="cropStage">
<p class="hint" style="text-align:center;margin:.4rem 0 0" data-en="Drag to move. Pinch or use the slider to zoom.">Trascina per spostare. Pizzica o usa il cursore per ingrandire.</p>
<div class="device-frame"><div class="crop-frame" id="cropFrame"><img id="cropImg" alt=""><canvas id="cropPreview" width="480" height="800" hidden></canvas></div></div>
<div class="slider-row"><span>Zoom</span><input id="zoomRange" type="range" min="1" max="4" step="0.01" value="1" aria-label="Zoom"></div>
<div class="seg"><button id="viewPhoto" class="on" onclick="setBgView(false)" data-en="Photo">Foto</button><button id="viewFinal" onclick="setBgView(true)" data-en="As it will look">Come si vedr&agrave;</button></div>
<div class="stage-actions"><button class="btn" onclick="cancelCrop()" data-en="Cancel">Annulla</button><button class="btn primary" id="bgSave" onclick="uploadBackground()" data-en="Save wallpaper">Salva sfondo</button></div>
</div>
<p id="bgStatus" class="hint" role="status"></p>
</div>
<div class="card">
<h2 data-en="In standby, show">In standby mostra</h2>
<div id="sleepModes"></div>
<p class="hint" id="sleepModeNote"></p>
</div>
<div class="card">
<h2 data-en="Wallpapers on the device">Sfondi sul dispositivo</h2>
<div class="sleep-grid" id="sleepGallery"></div>
</div>
</section>

<section id="tab-files" class="tabpanel" hidden>
<div class="card flush" id="filesCard">
<div class="bar">
<button class="btn icon" id="fxUp" onclick="fxGoUp()" aria-label="Cartella superiore" data-en-label="Parent folder"></button>
<div class="path" id="fxPath"></div>
<input class="only-desktop" id="fxSearch" type="search" style="width:240px" oninput="fxRender()">
<button class="btn icon only-phone" id="fxSearchBtn" onclick="fxToggleSearch()" aria-label="Cerca" data-en-label="Search"></button>
<button class="btn primary only-desktop" id="fxUploadD" onclick="document.getElementById('fileFx').click()"></button>
<button class="btn only-desktop" id="fxNewD" onclick="fxNewFolder()"></button>
</div>
<div class="bar only-phone" id="fxSearchBar" hidden><input class="grow" id="fxSearchP" type="search" oninput="fxRender()"></div>
<div class="bar only-phone">
<button class="btn primary" id="fxUploadP" onclick="document.getElementById('fileFx').click()"></button>
<button class="btn" id="fxNewP" onclick="fxNewFolder()"></button>
<span class="grow"></span>
<button class="btn icon" id="fxMoreBtn" onclick="fxToolbarMenu(this)" aria-label="Altro" data-en-label="More"></button>
</div>
<input id="fileFx" type="file" multiple hidden>
<div class="queue" id="queueFx" style="padding:0 .6rem"></div>
<div class="explorer">
<div class="side" id="fxSide"></div>
<div>
<div class="thead" id="fxHead"></div>
<div class="rows" id="fxRows"></div>
<div class="sys-line" id="fxSys" hidden></div>
<div class="dropzone-desktop" id="fxDropHint"></div>
</div>
</div>
<div class="statusbar"><span id="fxStatus"></span><span data-en="Double click opens · F2 renames · Del deletes · right click for the menu">Doppio clic apre &middot; F2 rinomina &middot; Canc elimina &middot; tasto destro per il menu</span></div>
</div>
</section>

<section id="tab-wifi" class="tabpanel" hidden>
<div class="card">
<h2 data-en="Saved networks">Reti salvate</h2>
<div id="wifiSaved"></div>
<button class="btn" style="margin-top:.6rem" onclick="wifiOpenJoin('',false)" data-en="Add a network by name">Aggiungi una rete scrivendo il nome</button>
</div>
<div class="card">
<h2 data-en="Networks nearby">Reti vicine</h2>
<div id="wifiScan"></div>
<p class="hint" id="wifiLanNote" hidden data-en="Nearby networks show only while you are on the device's own hotspot. From here you can still add one by typing its name: the device leaves this network to try it, and comes back by itself if it fails.">Le reti vicine si vedono solo quando sei collegato all&apos;hotspot del dispositivo. Da qui puoi comunque aggiungerne una scrivendo il nome: il dispositivo lascia questa rete per provarla, e ci torna da solo se non riesce.</p>
</div>
</section>
</div>
<div id="toast" hidden role="status"></div>
<script>
// --- base ---
// The page is written in Italian and carries its English beside it: static
// text in `data-en` attributes, everything built here through L(it, en).
// The language is the device's own, read from /api/status.
let LANG='it',activeTab='books',portalHotspot=false,linkUp=true;
function L(it,en){return LANG==='en'?en:it}
function $(id){return document.getElementById(id)}
function enc(s){return encodeURIComponent(s)}
function escapeHtml(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;')}
function formatBytes(n){
  if(n===undefined||n===null)return '--';
  const u=['B','KB','MB','GB'];let i=0,v=n;
  while(v>=1024&&i<u.length-1){v/=1024;i++}
  return (i===0?String(v):v.toLocaleString(LANG,{minimumFractionDigits:1,maximumFractionDigits:1}))+' '+u[i];
}
function formatDate(seconds,short){
  if(!seconds)return '';
  let d=new Date(seconds*1000);
  try{
    return short?d.toLocaleDateString(LANG,{day:'numeric',month:'short'}):d.toLocaleString(LANG,{day:'numeric',month:'short',year:'numeric',hour:'2-digit',minute:'2-digit'});
  }catch(e){return d.toISOString().slice(0,10)}
}
function parentOf(p){let i=p.lastIndexOf('/');return i<=0?'/':p.slice(0,i)}
function baseName(p){return p.slice(p.lastIndexOf('/')+1)}
function joinPath(dir,name){return (dir==='/'?'/':dir+'/')+name}
const ICON={
 folder:'<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
 file:'<svg viewBox="0 0 24 24"><path d="M7 3h7l5 5v11a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M14 3v5h5"/></svg>',
 book:'<svg viewBox="0 0 24 24"><path d="M5 4h11a3 3 0 0 1 3 3v13H8a3 3 0 0 1-3-3z"/><path d="M5 17a3 3 0 0 1 3-3h11"/></svg>',
 img:'<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 8"/></svg>',
 music:'<svg viewBox="0 0 24 24"><path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/></svg>',
 more:'<svg viewBox="0 0 24 24"><circle cx="12" cy="5" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="12" cy="19" r="1.2"/></svg>',
 up:'<svg viewBox="0 0 24 24"><path d="M12 19V5M5 12l7-7 7 7"/></svg>',
 plus:'<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>',
 upload:'<svg viewBox="0 0 24 24"><path d="M12 16V4M6 10l6-6 6 6M4 20h16"/></svg>',
 down:'<svg viewBox="0 0 24 24"><path d="M12 4v12M6 10l6 6 6-6M4 20h16"/></svg>',
 move:'<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M10 13h6M13 10l3 3-3 3"/></svg>',
 edit:'<svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16z"/></svg>',
 trash:'<svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/></svg>',
 check:'<svg viewBox="0 0 24 24"><path d="M5 12l5 5 9-10"/></svg>',
 search:'<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="6"/><path d="M20 20l-4.5-4.5"/></svg>',
 paste:'<svg viewBox="0 0 24 24"><rect x="6" y="5" width="12" height="16" rx="2"/><path d="M9 5V3h6v2"/></svg>',
 x:'<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>',
 reload:'<svg viewBox="0 0 24 24"><path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5"/></svg>',
 open:'<svg viewBox="0 0 24 24"><path d="M5 12h14M13 6l6 6-6 6"/></svg>',
 select:'<svg viewBox="0 0 24 24"><rect x="4" y="4" width="16" height="16" rx="3"/><path d="M8 12l3 3 5-6"/></svg>',
 eye:'<svg viewBox="0 0 24 24"><path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>'};
async function api(url,opt){
  let r;
  try{r=await fetch(url,opt)}catch(e){noteLinkDown();throw e}
  let t=await r.text();
  if(!r.ok)throw new Error(t||('HTTP '+r.status));
  return t;
}
async function ensureDir(path){try{await api('/api/list?path='+enc(path))}catch(e){try{await api('/api/mkdir?path='+enc(path),{method:'POST'})}catch(e2){}}}

// What the device or the browser answered, in the user's words. The device
// answers in English and with the system's own messages; anything not known
// here is shown as it came.
const MAX_UPLOAD_BYTES=64*1024*1024;
function tooBigText(){return L('Il file supera i 64 MB che si possono caricare da qui. Usa il cavo USB: sul dispositivo, Carica, poi Cavo USB.','The file is over the 64 MB that can be uploaded from here. Use the USB cable: on the device, Upload, then USB cable.')}
function explainError(e){
  let raw=String(e&&e.message!==undefined?e.message:e);
  const known=[
    [/failed to fetch|networkerror|load failed|network error/i,L('Il dispositivo non risponde: di solito è perché la schermata Carica, Wi-Fi è stata chiusa. Riaprila sul dispositivo.','The device does not answer: usually because its Upload, Wi-Fi screen was closed. Open it again on the device.')],
    [/upload exceeds/i,tooBigText()],
    [/protected configuration file/i,L('È un file di impostazioni del dispositivo: da qui non si può toccare.','This is one of the device\'s settings files: it cannot be changed from here.')],
    [/name not allowed/i,L('Questo nome non si può usare sulla scheda: niente / : * ? " < > | né barre rovesciate, e niente punti o spazi alla fine.','This name cannot be used on the card: no / : * ? " < > | or backslashes, and no dots or spaces at the end.')],
    [/path exceeds/i,L('Il percorso è troppo lungo: accorcia il nome del file o della cartella.','The path is too long: shorten the name of the file or folder.')],
    [/path traversal|not UTF-8|top folder/i,L('Percorso non valido.','Invalid path.')],
    [/another site/i,L("Richiesta rifiutata: apri la pagina dall'indirizzo mostrato sul dispositivo.",'Request refused: open the page from the address shown on the device.')],
    [/cover not cached/i,L('La copertina non è ancora pronta.','The cover is not ready yet.')],
    [/not empty|os error (39|66|90)\b/i,L('La cartella non è vuota: elimina prima quello che contiene.','The folder is not empty: delete what it holds first.')],
    [/no such file|os error 2\b/i,L('File o cartella non trovati: forse sono già stati spostati o eliminati.','File or folder not found: it may already have been moved or deleted.')],
    [/already exists|file exists|os error 17\b/i,L('Esiste già un elemento con questo nome.','Something with this name is already there.')],
    [/no space|os error 28\b/i,L('La scheda di memoria è piena.','The memory card is full.')]
  ];
  for(const [pattern,text] of known){if(pattern.test(raw))return text}
  return L('Errore: ','Error: ')+raw;
}

// --- messaggi: una riga in basso, vicino al pollice, che sparisce da sola ---
let toastTimer=null;
function toast(text,opts){
  opts=opts||{};
  let el=$('toast');
  el.className=opts.error?'error':'';
  el.innerHTML='<span>'+escapeHtml(text)+'</span>'+(opts.action?'<button id="toastAction">'+escapeHtml(opts.actionLabel)+'</button>':'');
  el.hidden=false;
  if(opts.action)$('toastAction').onclick=function(){hideToast();opts.action()};
  clearTimeout(toastTimer);
  toastTimer=setTimeout(hideToast,opts.ms||(opts.error?8000:4000));
}
function hideToast(){clearTimeout(toastTimer);$('toast').hidden=true}
function status(text){toast(text)}
// With the device gone the banner at the top already says so, once.
function fail(e){if(linkUp)toast(explainError(e),{error:true})}

// --- collegamento: si vede se il dispositivo risponde, e la pagina aperta e
// usata tiene viva la sessione (`alive=1`); una pagina dimenticata no, cosi
// il dispositivo la chiude da solo come prima.
let lastTouch=Date.now();
['pointerdown','keydown','wheel'].forEach(ev=>document.addEventListener(ev,()=>{lastTouch=Date.now()},{passive:true}));
function showLink(up){
  linkUp=up;
  $('linkPill').className='pill '+(up?'on':'off');
  $('linkText').textContent=up?L('Collegato','Connected'):L('Non collegato','Not connected');
  $('offline').hidden=up;
  $('space').hidden=!up;
}
function noteLinkDown(){showLink(false)}
function showSpace(bytes){$('spaceNum').textContent=formatBytes(bytes);$('spaceWord').textContent=L(' liberi',' free');$('space').title=L('Spazio libero sulla scheda','Free space on the card')}
async function readStatus(alive){
  let r=await fetch('/api/status'+(alive?'?alive=1':''));
  if(!r.ok)throw new Error('HTTP '+r.status);
  let s=JSON.parse(await r.text());
  portalHotspot=!!s.hotspot;
  if(s.sleep)sleepScreen=s.sleep;
  showSpace(s.free_bytes);
  showLink(true);
  return s;
}
async function pollLink(){
  if(document.visibilityState!=='visible')return;
  try{await readStatus(Date.now()-lastTouch<10*60*1000)}catch(e){showLink(false)}
}
function fetchSpace(){readStatus(false).catch(()=>{})}
async function retryLink(){try{await readStatus(true);refreshActiveTab()}catch(e){showLink(false)}}

// --- finestre: testo da chiedere, cartella da scegliere, anteprima ---
let sheetClose=null;
function openSheet(html,onClose){
  closeSheet();
  let shade=document.createElement('div');
  shade.id='shade';
  shade.innerHTML='<div class="sheet" role="dialog" aria-modal="true">'+html+'</div>';
  shade.addEventListener('pointerdown',e=>{if(e.target===shade)closeSheet()});
  document.body.appendChild(shade);
  sheetClose=onClose||null;
  return shade.firstChild;
}
function closeSheet(){
  let shade=$('shade');
  if(!shade)return;
  shade.remove();
  let done=sheetClose;sheetClose=null;
  if(done)done();
}
function askText(title,value,okLabel,note){
  return new Promise(resolve=>{
    let answered=false;
    let sheet=openSheet('<h2>'+escapeHtml(title)+'</h2>'+(note?'<p class="hint">'+escapeHtml(note)+'</p>':'')+'<input type="text" id="askInput" autocomplete="off"><div class="actions"><button class="btn" id="askNo">'+L('Annulla','Cancel')+'</button><button class="btn primary" id="askOk">'+escapeHtml(okLabel)+'</button></div>',()=>{if(!answered)resolve(null)});
    let input=sheet.querySelector('#askInput');
    input.value=value||'';
    let ok=()=>{let v=input.value.trim();if(!v)return;answered=true;closeSheet();resolve(v)};
    sheet.querySelector('#askOk').onclick=ok;
    sheet.querySelector('#askNo').onclick=closeSheet;
    input.addEventListener('keydown',e=>{if(e.key==='Enter')ok()});
    input.focus();
    let dot=input.value.lastIndexOf('.');
    input.setSelectionRange(0,dot>0?dot:input.value.length);
  });
}
function askConfirm(title,text,okLabel){
  return new Promise(resolve=>{
    let answered=false;
    let sheet=openSheet('<h2>'+escapeHtml(title)+'</h2><p class="hint">'+escapeHtml(text)+'</p><div class="actions"><button class="btn" id="askNo">'+L('Annulla','Cancel')+'</button><button class="btn danger" id="askOk">'+escapeHtml(okLabel)+'</button></div>',()=>{if(!answered)resolve(false)});
    sheet.querySelector('#askOk').onclick=()=>{answered=true;closeSheet();resolve(true)};
    sheet.querySelector('#askNo').onclick=closeSheet;
  });
}

// --- menu: le azioni di una riga, sotto il dito o sotto il puntatore ---
function closeMenu(){let m=document.querySelector('.menu');if(m)m.remove()}
function openMenu(at,items){
  closeMenu();
  let menu=document.createElement('div');
  menu.className='menu';
  menu.setAttribute('role','menu');
  for(const item of items){
    if(item==='-'){menu.appendChild(document.createElement('hr'));continue}
    if(item.label&&!item.run){let lab=document.createElement('div');lab.className='lab';lab.textContent=item.label;menu.appendChild(lab);continue}
    let b=document.createElement('button');
    b.setAttribute('role','menuitem');
    if(item.danger)b.className='del';
    b.innerHTML=(item.icon?ICON[item.icon]:'<svg viewBox="0 0 24 24"></svg>')+'<span>'+escapeHtml(item.label)+'</span>';
    b.onclick=function(){closeMenu();item.run()};
    menu.appendChild(b);
  }
  document.body.appendChild(menu);
  let x,y;
  if(at&&at.getBoundingClientRect){let r=at.getBoundingClientRect();x=r.right-menu.offsetWidth;y=r.bottom+4}else{x=at.x;y=at.y}
  x=Math.max(8,Math.min(x,innerWidth-menu.offsetWidth-8));
  if(y+menu.offsetHeight>innerHeight-8)y=Math.max(8,innerHeight-menu.offsetHeight-8);
  menu.style.left=x+'px';menu.style.top=y+'px';
}
document.addEventListener('pointerdown',e=>{if(!e.target.closest('.menu'))closeMenu()},true);
document.addEventListener('keydown',e=>{if(e.key==='Escape'){closeMenu();closeSheet()}});
window.addEventListener('scroll',closeMenu,{passive:true});

// --- elimina con «Annulla»: l'elemento sparisce subito dall'elenco, ma viene
// eliminato davvero solo dopo qualche secondo, o quando si lascia la pagina.
let pendingDeletes=new Map(),pendingTimer=null,pendingAfter=null;
function isPendingDelete(path){return pendingDeletes.has(path)}
function deleteWithUndo(items,after){
  // items: [{path, folder}]
  flushDeletes();
  for(const item of items)pendingDeletes.set(item.path,item);
  pendingAfter=after;
  if(after)after();
  let label=items.length===1?L('«'+baseName(items[0].path)+'» eliminato','"'+baseName(items[0].path)+'" deleted'):L(items.length+' elementi eliminati',items.length+' items deleted');
  toast(label,{ms:6000,actionLabel:L('Annulla','Undo'),action:function(){clearTimeout(pendingTimer);pendingDeletes.clear();if(after)after()}});
  pendingTimer=setTimeout(flushDeletes,6000);
}
function deleteUrl(item){return '/api/delete?path='+enc(item.path)+(item.folder?'&recursive=1':'')}
async function flushDeletes(){
  clearTimeout(pendingTimer);
  if(pendingDeletes.size===0)return;
  let items=Array.from(pendingDeletes.values()),after=pendingAfter,failed=null;
  for(const item of items){
    try{await api(deleteUrl(item),{method:'POST'})}catch(e){failed=e}
    pendingDeletes.delete(item.path);
  }
  if(failed)fail(failed);
  fetchSpace();
  if(after)after(true);
}
// Leaving the page keeps the user's word: what was deleted goes.
window.addEventListener('pagehide',()=>{for(const item of pendingDeletes.values()){try{fetch(deleteUrl(item),{method:'POST',keepalive:true})}catch(e){}}pendingDeletes.clear()});

// --- caricamento file: una fabbrica condivisa, ciascuna scheda con la sua
// coda e la sua cartella di destinazione.
// Long file names are kept: only the characters FAT cannot store are
// replaced, and the trailing dots and spaces it would silently drop.
function safeName(name){let s=String(name).replace(/[\\\/:*?"<>|\u0000-\u001f]/g,'_').replace(/[. ]+$/,'').trim();return s.slice(0,200)||'file'}
function queueStateText(state){return {queued:L('in coda','queued'),uploading:L('caricamento','uploading'),done:L('caricato','uploaded')}[state]}
function createUploader(opts){
  let queue=[],busy=false;
  function render(){
    let html='';
    for(const item of queue){
      html+='<div class="qitem '+item.status+'"><span class="name">'+escapeHtml(item.name)+'</span><div class="bar"><i style="width:'+item.progress+'%"></i></div><span class="state">'+escapeHtml(item.status==='error'?item.error:queueStateText(item.status))+'</span></div>';
    }
    $(opts.containerId).innerHTML=html;
  }
  // `dir` overrides the uploader's own folder for this batch, `existing`
  // the names already in it.
  function handleFiles(fileList,dir,existing){
    let target=dir||opts.getDir();
    let used=existing||opts.getExisting(target);
    queue=queue.filter(q=>q.status==='queued'||q.status==='uploading');
    for(const file of Array.from(fileList)){
      let name=safeName(file.name),base=name,i=1;
      while(used.has(name.toLowerCase())){let dot=base.lastIndexOf('.');let stem=dot<=0?base:base.slice(0,dot);let ext=dot<=0?'':base.slice(dot);name=stem+' ('+i+')'+ext;i++}
      used.add(name.toLowerCase());
      // Refused here rather than by the device after 64 MB went through.
      // So is a path longer than the device accepts (128 bytes).
      let refused=file.size>MAX_UPLOAD_BYTES?tooBigText():(new TextEncoder().encode(joinPath(target,name)).length>128?explainError('path exceeds'):'');
      queue.push({file:file,name:name,dir:target,status:refused?'error':'queued',progress:0,error:refused});
    }
    render();
    process();
  }
  function uploadOne(item){
    return new Promise((resolve,reject)=>{
      let xhr=new XMLHttpRequest();
      xhr.open('POST','/api/upload?path='+enc(joinPath(item.dir,item.name)));
      xhr.upload.onprogress=function(e){if(e.lengthComputable){item.progress=Math.round(e.loaded/e.total*100);render()}};
      xhr.onload=function(){if(xhr.status>=200&&xhr.status<300)resolve();else reject(new Error(xhr.responseText||('HTTP '+xhr.status)))};
      xhr.onerror=function(){reject(new Error('Failed to fetch'))};
      xhr.send(item.file);
    });
  }
  async function process(){
    if(busy)return;busy=true;let any=false;
    while(true){
      let item=queue.find(q=>q.status==='queued');
      if(!item)break;
      any=true;item.status='uploading';render();lastTouch=Date.now();
      try{await uploadOne(item);item.status='done';item.progress=100}
      catch(e){item.status='error';item.error=explainError(e)}
      if(item.status==='done'&&opts.onItemDone){
        try{await opts.onItemDone(item)}catch(e){/* best-effort, never fails the upload itself */}
      }
      render();
    }
    busy=false;
    // What went through leaves the queue after a moment; what did not stays,
    // with its reason, until the next batch.
    setTimeout(()=>{if(!busy){queue=queue.filter(q=>q.status!=='done');render()}},4000);
    if(any&&opts.onDone)opts.onDone();
  }
  return {handleFiles:handleFiles};
}
function wireDropZone(zoneId,inputId,uploader){
  let zone=$(zoneId);
  ['dragover','dragenter'].forEach(ev=>zone.addEventListener(ev,e=>{e.preventDefault();zone.classList.add('drag')}));
  ['dragleave','drop'].forEach(ev=>zone.addEventListener(ev,e=>{e.preventDefault();zone.classList.remove('drag')}));
  zone.addEventListener('drop',e=>{if(e.dataTransfer.files.length)uploader.handleFiles(e.dataTransfer.files)});
  $(inputId).addEventListener('change',e=>{if(e.target.files.length)uploader.handleFiles(e.target.files);e.target.value=''});
}
function downloadPaths(paths){
  // One after the other: a browser asked for several files at once stops
  // after the first.
  paths.forEach((p,i)=>setTimeout(()=>{let a=document.createElement('a');a.href='/api/download?path='+enc(p);a.download=baseName(p);document.body.appendChild(a);a.click();a.remove()},i*700));
}

// --- copertina libro generata nel browser: lo stesso principio gia usato
// per gli sfondi (il decoder JPEG/PNG del browser fa il lavoro pesante, non
// l'ESP32) applicato agli EPUB. Un tentativo lato
// dispositivo di fare questo durante l'upload (parsing ZIP + decodifica
// immagine + paginazione) ha saturato la memoria del firmware causando
// errori "not enough space" alla lettura successiva: generare qui, e
// caricare solo il file binario gia pronto (.THB, stesso formato che
// CoverCache scrive su SD), evita del tutto quel rischio.
const COVER_THUMB_W=208,COVER_THUMB_H=252,COVER_CACHE_DIR='/READER/CACHE';
function fnv1a64(bytes){
  let hash=0xcbf29ce484222325n;const prime=0x100000001b3n,mask=0xffffffffffffffffn;
  for(let i=0;i<bytes.length;i++){hash^=BigInt(bytes[i]);hash=(hash*prime)&mask}
  return hash;
}
function coverFingerprint(absPath,sizeBytes,modifiedSeconds){
  let buf=[];
  const str=s=>{for(const b of new TextEncoder().encode(s))buf.push(b)};
  const u64=n=>{let v=BigInt(Math.trunc(n));for(let i=0;i<8;i++){buf.push(Number(v&0xffn));v>>=8n}};
  const u16=n=>{buf.push(n&0xff);buf.push((n>>8)&0xff)};
  // The last string is the firmware's COVER_CACHE_FORMAT_VERSION: the two must match.
  str(absPath);u64(sizeBytes);u64(modifiedSeconds);str('epub');u16(COVER_THUMB_W);u16(COVER_THUMB_H);str('5');
  return fnv1a64(Uint8Array.from(buf));
}
function zipParseCentralDirectory(bytes){
  let view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
  let minScan=Math.max(0,bytes.length-22-65535),eocd=-1;
  for(let i=bytes.length-22;i>=minScan;i--){if(view.getUint32(i,true)===0x06054b50){eocd=i;break}}
  if(eocd<0)throw new Error('EOCD non trovato');
  let entryCount=view.getUint16(eocd+10,true),cdOffset=view.getUint32(eocd+16,true);
  let entries=new Map(),offset=cdOffset,decoder=new TextDecoder('utf-8');
  for(let i=0;i<entryCount;i++){
    if(view.getUint32(offset,true)!==0x02014b50)break;
    let method=view.getUint16(offset+10,true),compSize=view.getUint32(offset+20,true);
    let nameLen=view.getUint16(offset+28,true),extraLen=view.getUint16(offset+30,true),commentLen=view.getUint16(offset+32,true);
    let localOffset=view.getUint32(offset+42,true);
    let name=decoder.decode(bytes.subarray(offset+46,offset+46+nameLen));
    entries.set(name,{method:method,compSize:compSize,localOffset:localOffset});
    offset+=46+nameLen+extraLen+commentLen;
  }
  return entries;
}
async function zipReadEntry(bytes,entry){
  let view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
  let nameLen=view.getUint16(entry.localOffset+26,true),extraLen=view.getUint16(entry.localOffset+28,true);
  let dataStart=entry.localOffset+30+nameLen+extraLen;
  let raw=bytes.subarray(dataStart,dataStart+entry.compSize);
  if(entry.method===0)return raw;
  if(entry.method===8){
    let stream=new Blob([raw]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
    return new Uint8Array(await new Response(stream).arrayBuffer());
  }
  throw new Error('metodo di compressione ZIP non supportato: '+entry.method);
}
function xmlAttr(tag,name){let m=tag.match(new RegExp(name+'\\s*=\\s*(?:"([^"]*)"|\'([^\']*)\')'));return m?(m[1]!==undefined?m[1]:m[2]):null}
async function extractEpubCoverBytes(fileBytes){
  let entries=zipParseCentralDirectory(fileBytes);
  let containerEntry=entries.get('META-INF/container.xml');
  if(!containerEntry)return null;
  let containerXml=new TextDecoder('utf-8').decode(await zipReadEntry(fileBytes,containerEntry));
  let rootfileTag=(containerXml.match(/<rootfile\b[^>]*>/)||[])[0];
  let opfPath=rootfileTag&&xmlAttr(rootfileTag,'full-path');
  if(!opfPath)return null;
  let opfEntry=entries.get(opfPath);
  if(!opfEntry)return null;
  let opfXml=new TextDecoder('utf-8').decode(await zipReadEntry(fileBytes,opfEntry));
  let packageDir=opfPath.includes('/')?opfPath.slice(0,opfPath.lastIndexOf('/')+1):'';
  let items=Array.from(opfXml.matchAll(/<item\b[^>]*>/g)).map(m=>m[0]);
  let coverHref=null,coverMediaType=null;
  let coverImageItem=items.find(tag=>(xmlAttr(tag,'properties')||'').split(/\s+/).includes('cover-image'));
  if(coverImageItem){coverHref=xmlAttr(coverImageItem,'href');coverMediaType=xmlAttr(coverImageItem,'media-type')}
  else{
    let metaTags=Array.from(opfXml.matchAll(/<meta\b[^>]*>/g)).map(m=>m[0]);
    let coverMeta=metaTags.find(tag=>xmlAttr(tag,'name')==='cover');
    let coverId=coverMeta&&xmlAttr(coverMeta,'content');
    if(coverId){
      let item=items.find(tag=>xmlAttr(tag,'id')===coverId);
      if(item){coverHref=xmlAttr(item,'href');coverMediaType=xmlAttr(item,'media-type')}
    }
    if(!coverHref){
      let item=items.find(tag=>xmlAttr(tag,'id')==='cover');
      if(item){coverHref=xmlAttr(item,'href');coverMediaType=xmlAttr(item,'media-type')}
    }
  }
  if(!coverHref)return null;
  let coverEntry=entries.get(packageDir+coverHref);
  if(!coverEntry)return null;
  return {bytes:await zipReadEntry(fileBytes,coverEntry),mediaType:coverMediaType||''};
}
async function buildCoverBits(imageBytes,mediaType){
  let bitmap=await createImageBitmap(new Blob([imageBytes],{type:mediaType||'image/jpeg'}));
  let canvas=document.createElement('canvas');
  canvas.width=COVER_THUMB_W;canvas.height=COVER_THUMB_H;
  let ctx=canvas.getContext('2d');
  // Transparency on white, as the firmware draws it: a bare canvas reads
  // back transparent pixels as black.
  ctx.fillStyle='#fff';ctx.fillRect(0,0,COVER_THUMB_W,COVER_THUMB_H);
  // The whole cover stretched to the cell's shape, as the firmware does:
  // the centre crop cut the edges off.
  ctx.drawImage(bitmap,0,0,COVER_THUMB_W,COVER_THUMB_H);
  let imageData=ctx.getImageData(0,0,COVER_THUMB_W,COVER_THUMB_H);
  let lum=computeLuminance(imageData.data,COVER_THUMB_W,COVER_THUMB_H,0,0);
  let dithered=ditherFloydSteinberg(lum,COVER_THUMB_W,COVER_THUMB_H);
  return packInkBits(dithered,COVER_THUMB_W,COVER_THUMB_H);
}
function packInkBits(dithered,width,height){
  // Ink (drawn/black) = bit 1, opposite of the sleep-wallpaper BMP's raw
  // panel convention above, matching the 1bpp `ImageRaw<BinaryColor>`
  // layout CoverCache reads back from a `.THB`.
  let rowBytes=Math.ceil(width/8),out=new Uint8Array(rowBytes*height);
  for(let y=0;y<height;y++){
    for(let x=0;x<width;x++){
      if(dithered[y*width+x]<128)out[y*rowBytes+(x>>3)]|=(0x80>>(x&7));
    }
  }
  return out;
}
function buildThbFile(bits,fingerprint){
  let header=new Uint8Array(18),dv=new DataView(header.buffer);
  header[0]=0x52;header[1]=0x57;header[2]=0x54;header[3]=0x48; // "RWTH"
  header[4]=1;header[5]=0;
  dv.setUint16(6,COVER_THUMB_W,true);dv.setUint16(8,COVER_THUMB_H,true);
  let v=fingerprint;for(let i=0;i<8;i++){header[10+i]=Number(v&0xffn);v>>=8n}
  let out=new Uint8Array(header.length+bits.length);
  out.set(header,0);out.set(bits,header.length);
  return out;
}
async function pregenerateBookCoverClientSide(item){
  if(typeof DecompressionStream==='undefined'||typeof createImageBitmap==='undefined')return;
  if(!/\.epu[b]?$/i.test(item.name))return;
  let fileBytes=new Uint8Array(await item.file.arrayBuffer());
  let cover=await extractEpubCoverBytes(fileBytes);
  if(!cover)return;
  let bookPath='/BOOKS/'+item.name;
  // A plain directory stat, not /api/books: that endpoint re-derives every
  // EPUB's title from its OPF (one ZIP-parsing worker thread per book) on
  // every call, which is wasted work here — only this one file's size/mtime
  // are needed to compute the fingerprint.
  let dirEntries=JSON.parse(await api('/api/list?path=/BOOKS'));
  let stat=dirEntries.find(e=>e.name===item.name);
  if(!stat)return;
  let bits=await buildCoverBits(cover.bytes,cover.mediaType);
  let fingerprint=coverFingerprint('/sdcard/RUSTMIX'+bookPath,stat.size,stat.modified);
  let fingerprintHex=(fingerprint&0xffffffffn).toString(16).toUpperCase().padStart(8,'0');
  let thb=buildThbFile(bits,fingerprint);
  // /api/mkdir only creates one level at a time, so ensure the parent
  // exists first: a device that has never opened the Reader yet may not
  // have /READER on the card at all.
  await ensureDir('/READER');
  await ensureDir(COVER_CACHE_DIR);
  await api('/api/upload?path='+enc(COVER_CACHE_DIR+'/'+fingerprintHex+'.THB'),{method:'POST',body:new Blob([thb])});
}

// --- libri: le copertine arrivano gia pronte dal browser (vedi
// pregenerateBookCoverClientSide sopra), quindi sia questa pagina che la
// schermata Libreria del dispositivo le trovano gia in cache.
let books=[];
const booksUploader=createUploader({containerId:'queueBooks',getDir:()=>'/BOOKS',getExisting:()=>new Set(books.map(b=>baseName(b.path).toLowerCase())),onItemDone:pregenerateBookCoverClientSide,onDone:()=>{status(L('Libri caricati','Books uploaded'));refreshBooks();fetchSpace()}});
wireDropZone('dropBooks','fileBooks',booksUploader);
async function refreshBooks(){
  try{
    await ensureDir('/BOOKS');
    books=JSON.parse(await api('/api/books'));
    renderBooks();
  }catch(e){$('bookGrid').innerHTML='<p class="hint">'+escapeHtml(explainError(e))+'</p>'}
}
function renderBooks(){
  let q=$('bookSearch').value.trim().toLowerCase();
  let present=books.filter(b=>!isPendingDelete(b.path));
  let visible=present.filter(b=>!q||b.title.toLowerCase().includes(q));
  let html='';
  for(const b of visible){
    let p=escapeHtml(b.path);
    html+='<div class="book-card"><div class="book-cover" data-empty="'+L('Nessuna copertina','No cover')+'"><img src="/api/cover?path='+enc(b.path)+'" alt="" loading="lazy" onerror="this.parentElement.classList.add(\'empty\')"></div><div class="book-title">'+escapeHtml(b.title)+'</div><div class="hint" style="margin:0">'+escapeHtml(b.format)+' &middot; '+formatBytes(b.size)+'</div><div class="card-actions"><button class="btn" data-p="'+p+'" onclick="downloadPaths([this.dataset.p])">'+L('Scarica','Download')+'</button><button class="btn danger" data-p="'+p+'" onclick="deleteBook(this.dataset.p)">'+L('Elimina','Delete')+'</button></div></div>';
  }
  $('bookGrid').innerHTML=html||'<p class="hint">'+(present.length?L('Nessun libro con questo titolo.','No book with this title.'):L('Nessun libro. Tocca il riquadro qui sopra per caricare i tuoi EPUB o TXT.','No books yet. Tap the box above to upload your EPUB or TXT files.'))+'</p>';
  $('bookCount').textContent=present.length+' '+(present.length===1?L('libro','book'):L('libri','books'));
}
function deleteBook(path){deleteWithUndo([{path:path,folder:false}],done=>{if(done)refreshBooks();else renderBooks()})}

// --- audiolibri: un file MP3 e un titolo; una cartella di MP3 e un titolo
// con le sue tracce (come li legge il dispositivo, vedi `audiobook.rs`).
let audioEntries=[],audioInfo=new Map();
const audioUploader=createUploader({containerId:'queueAudio',getDir:()=>'/AUDIO',getExisting:()=>new Set(),onDone:()=>{status(L('Audiolibro caricato','Audiobook uploaded'));refreshAudio();fetchSpace()}});
function commonTitle(files){
  let names=files.map(f=>f.name.replace(/\.mp3$/i,''));
  let prefix=names[0];
  for(const n of names){while(prefix&&!n.startsWith(prefix))prefix=prefix.slice(0,-1)}
  prefix=prefix.replace(/[\s._\-–(\[#]*\d*$/,'').replace(/[\s._\-–(\[#]+$/,'').trim();
  return prefix.length>=3?prefix:L('Audiolibro','Audiobook');
}
async function audioFilesChosen(list){
  let files=Array.from(list).filter(f=>/\.mp3$/i.test(f.name));
  if(!files.length){toast(L('Servono file MP3.','MP3 files are needed.'),{error:true});return}
  if(files.length===1){
    audioUploader.handleFiles(files,'/AUDIO',new Set(audioEntries.map(e=>e.name.toLowerCase())));
    return;
  }
  let title=await askText(L('Titolo dell\'audiolibro','Title of the audiobook'),commonTitle(files),L('Carica','Upload'),L(files.length+' tracce, in una cartella con questo nome.',files.length+' tracks, in a folder of this name.'));
  if(!title)return;
  let dir='/AUDIO/'+safeName(title);
  await ensureDir('/AUDIO');
  await ensureDir(dir);
  let existing=new Set();
  try{existing=new Set(JSON.parse(await api('/api/list?path='+enc(dir))).map(e=>e.name.toLowerCase()))}catch(e){fail(e);return}
  files.sort((a,b)=>a.name.localeCompare(b.name,undefined,{numeric:true}));
  audioUploader.handleFiles(files,dir,existing);
}
(function(){
  let zone=$('dropAudio');
  ['dragover','dragenter'].forEach(ev=>zone.addEventListener(ev,e=>{e.preventDefault();zone.classList.add('drag')}));
  ['dragleave','drop'].forEach(ev=>zone.addEventListener(ev,e=>{e.preventDefault();zone.classList.remove('drag')}));
  zone.addEventListener('drop',e=>{if(e.dataTransfer.files.length)audioFilesChosen(e.dataTransfer.files)});
  $('fileAudio').addEventListener('change',e=>{if(e.target.files.length)audioFilesChosen(e.target.files);e.target.value=''});
})();
async function refreshAudio(){
  try{
    await ensureDir('/AUDIO');
    audioEntries=JSON.parse(await api('/api/list?path=/AUDIO')).filter(e=>e.kind==='folder'||/\.mp3$/i.test(e.name));
    audioEntries.sort((a,b)=>a.name.localeCompare(b.name,undefined,{numeric:true,sensitivity:'base'}));
    renderAudio();
    // One folder at a time: the device answers a single request comfortably.
    for(const e of audioEntries){
      if(e.kind!=='folder'||activeTab!=='audio')continue;
      try{
        let tracks=JSON.parse(await api('/api/list?path='+enc('/AUDIO/'+e.name))).filter(t=>t.kind==='file'&&/\.mp3$/i.test(t.name));
        audioInfo.set(e.name,{count:tracks.length,size:tracks.reduce((sum,t)=>sum+t.size,0)});
        renderAudio();
      }catch(err){}
    }
  }catch(e){$('audioRows').innerHTML='<div class="empty-note">'+escapeHtml(explainError(e))+'</div>'}
}
function renderAudio(){
  let html='';
  for(const e of audioEntries){
    let path='/AUDIO/'+e.name;
    if(isPendingDelete(path))continue;
    let folder=e.kind==='folder',info=audioInfo.get(e.name);
    let meta=folder?(info?info.count+' '+(info.count===1?L('traccia','track'):L('tracce','tracks'))+' · '+formatBytes(info.size):'...'):'1 file · '+formatBytes(e.size);
    html+='<div class="row" tabindex="0" data-p="'+escapeHtml(path)+'" data-f="'+(folder?1:0)+'" onclick="audioMenu(this,this.querySelector(\'.more\'))"><span class="ic">'+ICON.music+'</span><div class="nm"><div>'+escapeHtml(folder?e.name:e.name.replace(/\.mp3$/i,''))+'</div><div class="meta" style="display:block">'+escapeHtml(meta)+'</div></div><button class="more" aria-label="'+L('Azioni','Actions')+'">'+ICON.more+'</button></div>';
  }
  $('audioRows').innerHTML=html||'<div class="empty-note">'+L('Nessun audiolibro. Tocca il riquadro qui sopra per caricare i file MP3.','No audiobooks yet. Tap the box above to upload MP3 files.')+'</div>';
}
function audioMenu(row,anchor){
  let path=row.dataset.p,folder=row.dataset.f==='1';
  let items=[];
  if(folder)items.push({icon:'open',label:L('Apri in File','Open in Files'),run:()=>{showTab('files');fxOpen(path)}});
  else items.push({icon:'down',label:L('Scarica','Download'),run:()=>downloadPaths([path])});
  items.push({icon:'edit',label:L('Rinomina','Rename'),run:()=>renameEntry(path,refreshAudio)});
  items.push('-');
  items.push({icon:'trash',label:L('Elimina','Delete'),danger:true,run:()=>deleteWithUndo([{path:path,folder:folder}],done=>{if(done)refreshAudio();else renderAudio()})});
  openMenu(anchor,items);
}
// Rename, shared by every list. The extension stays unless the user types
// another one.
async function renameEntry(path,after){
  let old=baseName(path);
  let name=await askText(L('Rinomina','Rename'),old,L('Rinomina','Rename'));
  if(!name)return;
  let safe=safeName(name);
  if(safe===old)return;
  try{await api('/api/rename?from='+enc(path)+'&to='+enc(joinPath(parentOf(path),safe)),{method:'POST'});after()}catch(e){fail(e)}
}

// --- bianco e nero: condiviso tra copertine e sfondi ---
function computeLuminance(data,width,height,brightness,contrast){
  let n=width*height,lum=new Float32Array(n),cf=1+contrast/100;
  for(let i=0;i<n;i++){let o=i*4;let v=0.299*data[o]+0.587*data[o+1]+0.114*data[o+2];lum[i]=(v-128)*cf+128+brightness}
  return lum;
}
function ditherFloydSteinberg(lum,width,height){
  let w=width,h=height,out=new Uint8Array(w*h);
  for(let y=0;y<h;y++){
    for(let x=0;x<w;x++){
      let i=y*w+x,old=lum[i],val=old<128?0:255;
      out[i]=val;
      let err=old-val;
      if(x+1<w)lum[i+1]+=err*7/16;
      if(y+1<h){
        if(x>0)lum[i+w-1]+=err*3/16;
        lum[i+w]+=err*5/16;
        if(x+1<w)lum[i+w+1]+=err*1/16;
      }
    }
  }
  return out;
}

// --- sfondi ---
// The device is held upright, 480 wide and 800 tall, so that is how the
// image is framed here. The sleep file itself is the panel's native 800x480
// (see `sleep_images.rs`): the turn is made when saving, never by the user.
const BG_W=800,BG_H=480,BG_PW=480,BG_PH=800,BG_ZOOM_MAX=4;
let bgObjectUrl=null,bgFinalView=false,bgPreviewTimer=null,bgBusy=false;
let bgCrop={tx:0,ty:0,zoom:1,baseScale:1,frameW:240,frameH:400,natW:0,natH:0};
function bgStatus(t){document.getElementById('bgStatus').textContent=t}
function hideStages(){document.querySelectorAll('.stage').forEach(el=>el.classList.remove('show'));document.getElementById('bgPick').hidden=false}
function showStage(id){hideStages();document.getElementById(id).classList.add('show');document.getElementById('bgPick').hidden=true}
function cancelCrop(){hideStages();bgStatus('')}
function onImageReady(){
  let img=document.getElementById('cropImg');
  bgCrop.natW=img.naturalWidth;bgCrop.natH=img.naturalHeight;
  showStage('cropStage');
  let frame=document.getElementById('cropFrame');
  bgCrop.frameW=frame.clientWidth;bgCrop.frameH=frame.clientHeight;
  // At zoom 1 the image covers the frame: no empty band can be left.
  bgCrop.baseScale=Math.max(bgCrop.frameW/bgCrop.natW,bgCrop.frameH/bgCrop.natH);
  bgCrop.zoom=1;
  document.getElementById('zoomRange').value=1;
  centerCrop();
  setBgView(false);
  applyCropTransform();
  bgStatus('');
  lastTouch=Date.now();
  document.getElementById('wallpaper').scrollIntoView({behavior:'smooth',block:'start'});
}
function loadBackgroundFile(file){
  if(!file)return;
  if(bgObjectUrl)URL.revokeObjectURL(bgObjectUrl);
  bgObjectUrl=URL.createObjectURL(file);
  let img=document.getElementById('cropImg');
  img.onload=onImageReady;
  img.onerror=function(){hideStages();bgStatus(L('Questo file non è un\'immagine che il browser sa aprire. Prova con un JPEG o un PNG.','The browser cannot open this file as a picture. Try a JPEG or a PNG.'))};
  img.src=bgObjectUrl;
}
function centerCrop(){
  let dispW=bgCrop.natW*bgCrop.baseScale*bgCrop.zoom,dispH=bgCrop.natH*bgCrop.baseScale*bgCrop.zoom;
  bgCrop.tx=(bgCrop.frameW-dispW)/2;bgCrop.ty=(bgCrop.frameH-dispH)/2;
}
function clampCrop(){
  let dispW=bgCrop.natW*bgCrop.baseScale*bgCrop.zoom,dispH=bgCrop.natH*bgCrop.baseScale*bgCrop.zoom;
  bgCrop.tx=Math.min(0,Math.max(bgCrop.frameW-dispW,bgCrop.tx));
  bgCrop.ty=Math.min(0,Math.max(bgCrop.frameH-dispH,bgCrop.ty));
}
function applyCropTransform(){
  let scale=bgCrop.baseScale*bgCrop.zoom;
  let img=document.getElementById('cropImg');
  img.style.width=(bgCrop.natW*scale)+'px';
  img.style.height=(bgCrop.natH*scale)+'px';
  img.style.transform='translate('+bgCrop.tx+'px,'+bgCrop.ty+'px)';
}
// Zoom keeping the point (fx, fy) of the frame under the finger or pointer.
function setBgZoom(zoom,fx,fy){
  zoom=Math.min(BG_ZOOM_MAX,Math.max(1,zoom));
  let old=bgCrop.baseScale*bgCrop.zoom,next=bgCrop.baseScale*zoom;
  bgCrop.tx=fx-(fx-bgCrop.tx)*next/old;bgCrop.ty=fy-(fy-bgCrop.ty)*next/old;
  bgCrop.zoom=zoom;
  document.getElementById('zoomRange').value=zoom;
  clampCrop();applyCropTransform();bgCropChanged();
}
const cropFrameEl=document.getElementById('cropFrame');
let bgPointers=new Map(),bgDragStart=null,bgPinchStart=null;
function bgFramePoint(e){let r=cropFrameEl.getBoundingClientRect();return {x:e.clientX-r.left-cropFrameEl.clientLeft,y:e.clientY-r.top-cropFrameEl.clientTop}}
function bgGestureStart(){
  let pts=Array.from(bgPointers.values());
  if(pts.length===1){bgDragStart={x:pts[0].x,y:pts[0].y,tx:bgCrop.tx,ty:bgCrop.ty};bgPinchStart=null}
  else if(pts.length>=2){bgDragStart=null;bgPinchStart={d:Math.hypot(pts[0].x-pts[1].x,pts[0].y-pts[1].y)||1,zoom:bgCrop.zoom}}
}
cropFrameEl.addEventListener('pointerdown',e=>{bgPointers.set(e.pointerId,bgFramePoint(e));cropFrameEl.setPointerCapture(e.pointerId);bgGestureStart();showBgPhotoWhileMoving()});
cropFrameEl.addEventListener('pointermove',e=>{
  if(!bgPointers.has(e.pointerId))return;
  bgPointers.set(e.pointerId,bgFramePoint(e));
  let pts=Array.from(bgPointers.values());
  if(bgPinchStart&&pts.length>=2){
    let d=Math.hypot(pts[0].x-pts[1].x,pts[0].y-pts[1].y)||1;
    setBgZoom(bgPinchStart.zoom*d/bgPinchStart.d,(pts[0].x+pts[1].x)/2,(pts[0].y+pts[1].y)/2);
  }else if(bgDragStart){
    bgCrop.tx=bgDragStart.tx+(pts[0].x-bgDragStart.x);bgCrop.ty=bgDragStart.ty+(pts[0].y-bgDragStart.y);
    clampCrop();applyCropTransform();
  }
});
function bgPointerEnd(e){bgPointers.delete(e.pointerId);bgGestureStart();if(bgPointers.size===0)bgCropChanged()}
cropFrameEl.addEventListener('pointerup',bgPointerEnd);
cropFrameEl.addEventListener('pointercancel',bgPointerEnd);
cropFrameEl.addEventListener('wheel',e=>{e.preventDefault();let p=bgFramePoint(e);showBgPhotoWhileMoving();setBgZoom(bgCrop.zoom*(e.deltaY<0?1.08:1/1.08),p.x,p.y)},{passive:false});
document.getElementById('zoomRange').addEventListener('input',e=>{showBgPhotoWhileMoving();setBgZoom(parseFloat(e.target.value),bgCrop.frameW/2,bgCrop.frameH/2)});
$('fileBg').addEventListener('change',e=>{if(e.target.files.length)loadBackgroundFile(e.target.files[0]);e.target.value=''});
// The framed part of the image, 480x800, upright as the user sees it.
function renderPortraitDither(){
  let scale=bgCrop.baseScale*bgCrop.zoom;
  let sx=-bgCrop.tx/scale,sy=-bgCrop.ty/scale,sw=bgCrop.frameW/scale,sh=bgCrop.frameH/scale;
  let canvas=document.createElement('canvas');
  canvas.width=BG_PW;canvas.height=BG_PH;
  let ctx=canvas.getContext('2d');
  // Transparency on white, as on paper.
  ctx.fillStyle='#fff';ctx.fillRect(0,0,BG_PW,BG_PH);
  ctx.drawImage(document.getElementById('cropImg'),sx,sy,sw,sh,0,0,BG_PW,BG_PH);
  let lum=computeLuminance(ctx.getImageData(0,0,BG_PW,BG_PH).data,BG_PW,BG_PH,0,0);
  autoLevels(lum);
  return ditherFloydSteinberg(lum,BG_PW,BG_PH);
}
// Stretches the tones between black and white, leaving out the darkest and
// lightest 1%: a dull photo would otherwise dither into an even grey. A
// picture that is nearly one tone already is left alone.
function autoLevels(lum){
  let hist=new Uint32Array(256),n=lum.length;
  for(let i=0;i<n;i++)hist[Math.max(0,Math.min(255,Math.round(lum[i])))]++;
  let cut=n*0.01,lo=0,hi=255,acc=0;
  for(;lo<255;lo++){acc+=hist[lo];if(acc>cut)break}
  acc=0;
  for(;hi>0;hi--){acc+=hist[hi];if(acc>cut)break}
  if(hi-lo<32)return;
  let k=255/(hi-lo);
  for(let i=0;i<n;i++)lum[i]=(lum[i]-lo)*k;
}
function drawBgPreview(){
  let dithered=renderPortraitDither();
  let canvas=document.getElementById('cropPreview'),ctx=canvas.getContext('2d');
  let out=ctx.createImageData(BG_PW,BG_PH);
  for(let i=0;i<dithered.length;i++){let v=dithered[i],o=i*4;out.data[o]=v;out.data[o+1]=v;out.data[o+2]=v;out.data[o+3]=255}
  ctx.putImageData(out,0,0);
  canvas.hidden=false;
}
function setBgView(finalView){
  bgFinalView=finalView;
  document.getElementById('viewPhoto').classList.toggle('on',!finalView);
  document.getElementById('viewFinal').classList.toggle('on',finalView);
  if(finalView)drawBgPreview();else document.getElementById('cropPreview').hidden=true;
}
// While the image moves, the photo is shown: the black and white version is
// redrawn once it has stopped.
function showBgPhotoWhileMoving(){if(bgFinalView)document.getElementById('cropPreview').hidden=true}
function bgCropChanged(){
  if(!bgFinalView)return;
  clearTimeout(bgPreviewTimer);
  bgPreviewTimer=setTimeout(()=>{if(bgFinalView&&bgPointers.size===0)drawBgPreview()},180);
}
// Upright 480x800 to the panel's native 800x480. The firmware's portrait
// layer maps logical (x, y) to native (y, 479 - x) (`orientation.rs`), so
// native (nx, ny) is upright (479 - ny, nx).
function portraitToNative(portrait){
  let out=new Uint8Array(BG_W*BG_H);
  for(let ny=0;ny<BG_H;ny++){
    let lx=BG_PW-1-ny;
    for(let nx=0;nx<BG_W;nx++)out[ny*BG_W+nx]=portrait[nx*BG_PW+lx];
  }
  return out;
}
function buildSleepBmp(dithered){
  const rowBytes=BG_W/8,pixelBytes=rowBytes*BG_H,headerSize=62,total=headerSize+pixelBytes;
  let buf=new Uint8Array(total),dv=new DataView(buf.buffer);
  buf[0]=0x42;buf[1]=0x4D;
  dv.setUint32(2,total,true);
  dv.setUint32(10,headerSize,true);
  dv.setUint32(14,40,true);
  dv.setInt32(18,BG_W,true);
  dv.setInt32(22,BG_H,true);
  dv.setUint16(26,1,true);
  dv.setUint16(28,1,true);
  dv.setUint32(30,0,true);
  dv.setUint32(34,pixelBytes,true);
  dv.setUint32(46,2,true);
  buf[54]=0;buf[55]=0;buf[56]=0;buf[57]=0;
  buf[58]=255;buf[59]=255;buf[60]=255;buf[61]=0;
  let offset=headerSize;
  for(let canvasRow=BG_H-1;canvasRow>=0;canvasRow--){
    for(let byteIndex=0;byteIndex<rowBytes;byteIndex++){
      let b=0;
      for(let bit=0;bit<8;bit++){
        let x=byteIndex*8+bit;
        if(dithered[canvasRow*BG_W+x]>=128)b|=(1<<(7-bit));
      }
      buf[offset]=b;offset++;
    }
  }
  return buf;
}
// The file gets the first free SLEEPnnn.BMP: a name is nothing the user has
// to think about.
async function nextSleepName(){
  await ensureDir('/SLEEP');
  let existing=new Set(JSON.parse(await api('/api/list?path=/SLEEP')).map(e=>e.name.toUpperCase()));
  for(let i=1;i<=999;i++){
    let candidate='SLEEP'+String(i).padStart(3,'0')+'.BMP';
    if(!existing.has(candidate))return candidate;
  }
  throw new Error(L('troppi sfondi: eliminane qualcuno','too many wallpapers: delete some'));
}
async function uploadBackground(){
  if(bgBusy||!document.getElementById('cropImg').naturalWidth)return;
  bgBusy=true;document.getElementById('bgSave').disabled=true;
  bgStatus(L('Preparazione...','Preparing...'));
  try{
    let bytes=buildSleepBmp(portraitToNative(renderPortraitDither()));
    let name=await nextSleepName();
    let blob=new Blob([bytes],{type:'application/octet-stream'});
    await new Promise((resolve,reject)=>{
      let xhr=new XMLHttpRequest();
      xhr.open('POST','/api/upload?path='+enc('/SLEEP/'+name));
      xhr.upload.onprogress=function(e){if(e.lengthComputable)bgStatus(L('Salvataggio ','Saving ')+Math.round(e.loaded/e.total*100)+'%')};
      xhr.onload=function(){if(xhr.status>=200&&xhr.status<300)resolve();else reject(new Error(xhr.responseText||('HTTP '+xhr.status)))};
      xhr.onerror=function(){reject(new Error('Failed to fetch'))};
      xhr.send(blob);
    });
    hideStages();
    bgStatus(L('Sfondo salvato. Lo trovi qui sotto.','Wallpaper saved. You find it below.'));
    refreshSleepGallery();
    fetchSpace();
  }catch(e){bgStatus(explainError(e))}
  bgBusy=false;document.getElementById('bgSave').disabled=false;
}

// --- sfondi: da dove arriva l'immagine ---
// The page has no picture search of its own: the free collections a page
// may query hold photographs, not the artwork people want as a wallpaper,
// and Google's results cannot be shown inside another page. A picture comes
// from the phone or the computer, pasted or dragged in; Google is a plain
// link to another tab.
// A picture copied on any other site, pasted here. On this page (not served
// encrypted) a button may not read the clipboard, so the paste itself is
// what is listened for.
document.addEventListener('paste',e=>{
  if(activeTab!=='wallpaper'||!e.clipboardData)return;
  let file=Array.from(e.clipboardData.files||[]).find(f=>f.type.startsWith('image/'));
  if(file){e.preventDefault();loadBackgroundFile(file)}
});
// A picture dragged here: a file from the computer, or straight from another
// tab when that site lets its pictures be read.
(function(){
  let card=$('wallpaper');
  ['dragover','dragenter'].forEach(ev=>card.addEventListener(ev,e=>{e.preventDefault();card.classList.add('dragover')}));
  ['dragleave','drop'].forEach(ev=>card.addEventListener(ev,e=>{e.preventDefault();card.classList.remove('dragover')}));
  card.addEventListener('drop',async e=>{
    let file=Array.from(e.dataTransfer.files||[]).find(f=>f.type.startsWith('image/'));
    if(file){loadBackgroundFile(file);return}
    let url=(e.dataTransfer.getData('text/uri-list')||e.dataTransfer.getData('text/plain')||'').split('\n')[0].trim();
    if(!/^https?:/.test(url))return;
    try{
      let r=await fetch(url);
      let blob=await r.blob();
      if(!blob.type.startsWith('image/'))throw new Error('not an image');
      loadBackgroundFile(blob);
    }catch(err){
      bgStatus(L('Quel sito non lascia leggere l\'immagine da qui. Copiala e incollala (Ctrl+V), oppure salvala e sceglila.','That site does not let the picture be read from here. Copy and paste it (Ctrl+V), or save it and choose it.'));
    }
  });
})();
function showWallpaperSources(){
  // On the device's own hotspot the phone has no Internet to search with.
  $('bgGoogle').hidden=portalHotspot;
  let phone=matchMedia('(pointer:coarse)').matches;
  $('bgChoose').innerHTML=ICON.img+(phone?L('Dal telefono','From the phone'):L('Dal computer','From the computer'));
  $('bgPaste').innerHTML=ICON.paste+L('Incolla','Paste');
}

// --- sfondi sul dispositivo ---
// A sleep file is stored turned, as the panel wants it: each thumbnail is
// turned back so it shows the way it will on the device. Decoded pictures
// are kept, so a list drawn again does not ask the device for them twice.
let sleepEntries=[],sleepBitmaps=new Map();
async function drawSleepThumb(canvas,url,key){
  try{
    let bitmap=sleepBitmaps.get(key);
    if(!bitmap){
      let r=await fetch(url);
      if(!r.ok)throw new Error('HTTP '+r.status);
      bitmap=await createImageBitmap(await r.blob());
      sleepBitmaps.set(key,bitmap);
    }
    let ctx=canvas.getContext('2d'),k=canvas.width/bitmap.height;
    ctx.translate(canvas.width,0);ctx.rotate(Math.PI/2);ctx.scale(k,k);
    ctx.drawImage(bitmap,0,0);
  }catch(e){if(canvas.parentElement)canvas.parentElement.classList.add('empty');canvas.remove()}
}
// What the device shows in standby: the Display setting ("sequential",
// "random", "fixed", "book-cover") and, when fixed, which wallpaper. All of
// it is set here as well as on the device, so it is read again with the
// list.
let sleepScreen={mode:'sequential',fixed:''};
const SLEEP_MODES=[
  ['sequential','Gli sfondi, uno dopo l\'altro','The wallpapers, one after the other'],
  ['random','Gli sfondi, a caso','The wallpapers, at random'],
  ['fixed','Sempre lo stesso sfondo','Always the same wallpaper'],
  ['book-cover','La copertina del libro che stai leggendo','The cover of the book you are reading']
];
// The wallpaper standby shows now, when it is still on the card.
function fixedName(){
  let wanted=(sleepScreen.fixed||'').toUpperCase();
  let entry=sleepEntries.find(e=>e.name.toUpperCase()===wanted);
  return entry?entry.name:'';
}
function isFixedWallpaper(name){return sleepScreen.mode==='fixed'&&fixedName()===name}
function sleepModeHint(){
  if(sleepScreen.mode==='fixed')return fixedName()?L('Resta quello segnato «Sfondo fisso» qui sotto. Per cambiarlo tocca «Usa come fisso» su un altro.','The one marked "Fixed wallpaper" below stays. To change it, tap "Keep this one" on another.'):L('Carica uno sfondo, poi sceglilo con «Usa come fisso».','Add a wallpaper, then choose it with "Keep this one".');
  if(sleepScreen.mode==='book-cover')return L('Quando lo standby arriva mentre leggi. Altrimenti gli sfondi, uno dopo l\'altro.','When standby comes while you are reading. Otherwise the wallpapers, one after the other.');
  return L('Per tenerne sempre uno tocca «Usa come fisso» sullo sfondo che vuoi.','To keep one all the time, tap "Keep this one" on the wallpaper you want.');
}
function renderSleepModes(){
  $('sleepModes').innerHTML=SLEEP_MODES.map(m=>'<label class="choice"><input type="radio" name="sleepMode" value="'+m[0]+'"'+(sleepScreen.mode===m[0]?' checked':'')+' onchange="setSleepMode(this.value)"><span>'+L(m[1],m[2])+'</span></label>').join('');
  $('sleepModeNote').textContent=sleepModeHint();
}
// Set the sleep screen and, with `name`, the wallpaper that stays. "Always
// the same" with none on show yet takes the first by name, as the device
// would by itself, so the page can mark it.
async function setSleepMode(mode,name){
  if(mode==='fixed'&&!name){
    name=fixedName();
    if(!name&&sleepEntries.length)name=sleepEntries.map(e=>e.name).sort((a,b)=>a.toUpperCase()<b.toUpperCase()?-1:1)[0];
  }
  try{
    await api('/api/sleep?mode='+enc(mode)+(name?'&name='+enc(name):''),{method:'POST'});
    sleepScreen={mode:mode,fixed:name||sleepScreen.fixed};
    toast(mode==='fixed'&&name?L('Questo sfondo resta fisso in standby.','This wallpaper now stays in standby.'):L('Schermata di standby salvata.','Sleep screen saved.'));
  }catch(e){fail(e)}
  // Also after a failure: the choice goes back to what the device has.
  renderSleepGallery();
}
async function refreshSleepGallery(){
  try{
    await ensureDir('/SLEEP');
    try{await readStatus(false)}catch(e){}
    sleepEntries=JSON.parse(await api('/api/list?path=/SLEEP')).filter(e=>e.kind==='file');
    renderSleepGallery();
  }catch(e){$('sleepGallery').innerHTML='<p class="hint">'+escapeHtml(explainError(e))+'</p>'}
}
async function renderSleepGallery(){
  let gallery=$('sleepGallery'),html='';
  for(const e of sleepEntries){
    let path='/SLEEP/'+e.name;
    if(isPendingDelete(path))continue;
    let p=escapeHtml(path);
    let fixed=isFixedWallpaper(e.name),n=escapeHtml(e.name);
    // The one that stays is marked, any other can take its place.
    html+='<div class="book-card'+(fixed?' fixed':'')+'">'+(fixed?'<div class="fixed-badge">'+L('Sfondo fisso','Fixed wallpaper')+'</div>':'')+'<div class="sleep-thumb" data-empty="'+L('Anteprima non disponibile','No preview')+'"><canvas width="240" height="400" data-p="'+p+'" data-k="'+escapeHtml(e.name+':'+e.size+':'+e.modified)+'"></canvas></div><div class="card-actions">'+(fixed?'':'<button class="btn wide" data-n="'+n+'" onclick="setSleepMode(\'fixed\',this.dataset.n)">'+L('Usa come fisso','Keep this one')+'</button>')+'<button class="btn" data-p="'+p+'" onclick="downloadPaths([this.dataset.p])">'+L('Scarica','Download')+'</button><button class="btn danger" data-p="'+p+'" onclick="deleteSleepImage(this.dataset.p)">'+L('Elimina','Delete')+'</button></div></div>';
  }
  gallery.innerHTML=html||'<p class="hint">'+L('Nessuno sfondo caricato.','No wallpapers yet.')+'</p>';
  renderSleepModes();
  // One at a time: the device serves a single file comfortably.
  for(const canvas of Array.from(gallery.querySelectorAll('canvas[data-p]'))){
    if(!canvas.isConnected)return;
    await drawSleepThumb(canvas,'/api/download?path='+enc(canvas.dataset.p),canvas.dataset.k);
  }
}
function deleteSleepImage(path){deleteWithUndo([{path:path,folder:false}],done=>{if(done)refreshSleepGallery();else renderSleepGallery()})}

// --- file: la scheda di memoria come in un esplora file ---
// On a phone: a tap opens, the three dots hold a row's actions, a long press
// starts a selection. With a mouse: a click selects, a double click opens,
// the right button opens the menu, F2 renames, Del deletes, and a row can be
// dragged onto a folder. The device only ever sees list, upload, rename
// (which also moves), mkdir and delete.
const FRIENDLY={BOOKS:['Libri','Books'],AUDIO:['Audiolibri','Audiobooks'],SLEEP:['Sfondi','Wallpapers'],APPS:['App e dizionari','Apps and dictionaries']};
let fx={path:'/',entries:[],sel:new Set(),selMode:false,sort:'name',asc:true,showSys:false,anchor:null,loaded:false};
let fxCounts=new Map(),fxDragging=null,fxLongPress=null,fxSkipClick=false;
const fxMouse=matchMedia('(hover:hover) and (pointer:fine)');
function fxFriendly(dir,name){return dir==='/'&&FRIENDLY[name]?L(FRIENDLY[name][0],FRIENDLY[name][1]):name}
// What the device keeps for itself: at the top of the card, every file but
// the two "read me" ones, and the folders of reading positions and
// statistics. Hidden until asked for, so they are not deleted by mistake.
function fxIsSystem(dir,e){
  if(e.name.startsWith('.'))return true;
  if(dir!=='/')return false;
  if(e.kind==='folder')return e.name==='READER'||e.name==='STATS';
  return !/^(LEGGIMI|README)\.TXT$/i.test(e.name);
}
function fxExt(name){let i=name.lastIndexOf('.');return i<0?'':name.slice(i+1).toLowerCase()}
function fxKind(dir,e){
  if(e.kind==='folder')return 'folder';
  let ext=fxExt(e.name);
  if(ext==='epub'||(ext==='txt'&&dir.startsWith('/BOOKS')))return 'book';
  if(ext==='mp3')return 'music';
  if(ext==='bmp'||ext==='png'||ext==='jpg'||ext==='jpeg')return 'img';
  return 'file';
}
function fxTypeLabel(dir,e){
  if(e.kind==='folder')return L('Cartella','Folder');
  let ext=fxExt(e.name);
  if(ext==='epub')return L('Libro (EPUB)','Book (EPUB)');
  if(ext==='txt')return dir.startsWith('/BOOKS')?L('Libro (testo)','Book (text)'):L('Testo','Text');
  if(ext==='mp3')return 'Audio MP3';
  if(ext==='bmp'||ext==='png'||ext==='jpg'||ext==='jpeg')return L('Immagine','Picture');
  return ext?'File '+ext.toUpperCase():'File';
}
function fxPreviewable(e){let ext=fxExt(e.name);return e.kind==='file'&&(['bmp','png','jpg','jpeg'].includes(ext)||(['txt','log','old'].includes(ext)&&e.size<=300*1024))}
function fxQuery(){return (fxMouse.matches&&innerWidth>=900?$('fxSearch'):$('fxSearchP')).value.trim().toLowerCase()}
function fxVisible(){
  let q=fxQuery();
  let list=fx.entries.filter(e=>!isPendingDelete(joinPath(fx.path,e.name))&&(fx.showSys||!fxIsSystem(fx.path,e))&&(!q||e.name.toLowerCase().includes(q)||fxFriendly(fx.path,e.name).toLowerCase().includes(q)));
  let dir=fx.asc?1:-1;
  list.sort((a,b)=>{
    // Folders first, whatever the order asked for.
    if((a.kind==='folder')!==(b.kind==='folder'))return a.kind==='folder'?-1:1;
    let r=0;
    if(fx.sort==='date')r=a.modified-b.modified;
    else if(fx.sort==='size')r=a.size-b.size;
    else if(fx.sort==='type')r=fxTypeLabel(fx.path,a).localeCompare(fxTypeLabel(fx.path,b));
    if(r===0)r=fxFriendly(fx.path,a.name).localeCompare(fxFriendly(fx.path,b.name),undefined,{numeric:true,sensitivity:'base'});
    return r*dir;
  });
  return list;
}
function fxEntry(name){return fx.entries.find(e=>e.name===name)}
function fxSelItems(){return Array.from(fx.sel).map(fxEntry).filter(Boolean).map(e=>({path:joinPath(fx.path,e.name),folder:e.kind==='folder',size:e.size}))}
async function fxOpen(path,keep){
  try{
    let entries=JSON.parse(await api('/api/list?path='+enc(path)));
    let same=fx.path===path;
    fx.path=path;fx.entries=entries;fx.loaded=true;
    if(!(keep&&same)){fx.sel.clear();fx.selMode=false;fx.anchor=null}
    else{for(const n of Array.from(fx.sel)){if(!fxEntry(n))fx.sel.delete(n)}}
    fxRender();
    if(path==='/')fxCountRoot();
  }catch(e){fail(e)}
}
function fxGoUp(){if(fx.path!=='/')fxOpen(parentOf(fx.path))}
// How many things each folder at the top holds, asked one folder at a time.
async function fxCountRoot(){
  for(const e of fx.entries){
    if(e.kind!=='folder'||fxCounts.has(e.name)||activeTab!=='files'||fx.path!=='/')continue;
    try{fxCounts.set(e.name,JSON.parse(await api('/api/list?path='+enc('/'+e.name))).length);if(fx.path==='/')fxRender()}catch(err){return}
  }
}
function fxRender(){
  let list=fxVisible(),dir=fx.path;
  // path
  let crumbs='<button data-p="/">'+(innerWidth>=900?L('Scheda di memoria','Memory card'):L('Scheda','Card'))+'</button>',acc='';
  dir.split('/').filter(Boolean).forEach((part,i)=>{let parent=acc||'/';acc+='/'+part;crumbs+='<span>›</span><button data-p="'+escapeHtml(acc)+'">'+escapeHtml(i===0?fxFriendly(parent,part):part)+'</button>'});
  $('fxPath').innerHTML=crumbs;
  $('fxUp').disabled=dir==='/';
  $('fxSearch').placeholder=$('fxSearchP').placeholder=L('Cerca in ','Search in ')+(dir==='/'?L('Scheda','Card'):fxFriendly(parentOf(dir),baseName(dir)));
  // column heads (mouse)
  let arrow=key=>fx.sort===key?(fx.asc?' ▲':' ▼'):'';
  $('fxHead').innerHTML='<span class="sp-check"></span><span class="sp-ic"></span><button class="nm" data-s="name">'+L('Nome','Name')+arrow('name')+'</button><button class="col c-date" data-s="date">'+L('Ultima modifica','Modified')+arrow('date')+'</button><button class="col c-type" data-s="type">'+L('Tipo','Type')+arrow('type')+'</button><button class="col c-size" data-s="size">'+L('Dimensione','Size')+arrow('size')+'</button><span class="sp-more"></span>';
  // rows
  let html='';
  for(const e of list){
    let folder=e.kind==='folder',kind=fxKind(dir,e),shown=fxFriendly(dir,e.name);
    let meta;
    if(folder){
      let count=dir==='/'?fxCounts.get(e.name):undefined;
      let parts=[];
      if(shown!==e.name)parts.push(e.name);
      if(count!==undefined)parts.push(count===0?L('vuota','empty'):count+' '+(count===1?L('elemento','item'):L('elementi','items')));
      meta=parts.join(' · ')||L('Cartella','Folder');
    }else meta=formatBytes(e.size)+(e.modified?' · '+formatDate(e.modified,true):'');
    html+='<div class="row'+(fx.sel.has(e.name)?' sel':'')+(fxIsSystem(dir,e)?' sys':'')+'" tabindex="0" data-n="'+escapeHtml(e.name)+'"'+(fxMouse.matches?' draggable="true"':'')+'><span class="check">'+ICON.check+'</span><span class="ic'+(folder?'':' file')+'">'+ICON[kind]+'</span><div class="nm"><div>'+escapeHtml(shown)+'</div><div class="meta">'+escapeHtml(meta)+'</div></div><span class="col c-date">'+escapeHtml(formatDate(e.modified,false))+'</span><span class="col c-type">'+escapeHtml(fxTypeLabel(dir,e))+'</span><span class="col c-size">'+(folder?'':formatBytes(e.size))+'</span><button class="more" aria-label="'+L('Azioni','Actions')+'">'+ICON.more+'</button></div>';
  }
  let hidden=fx.entries.filter(e=>fxIsSystem(dir,e)).length;
  $('fxRows').innerHTML=html||'<div class="empty-note">'+(fxQuery()?L('Niente con questo nome in questa cartella.','Nothing with this name in this folder.'):L('Cartella vuota. Usa «Carica qui» per aggiungere file.','Empty folder. Use "Upload here" to add files.'))+'</div>';
  $('filesCard').classList.toggle('selmode',fx.selMode);
  // system files line (phone) and side panel (mouse)
  $('fxSys').hidden=hidden===0;
  $('fxSys').innerHTML=ICON.folder+'<span>'+(fx.showSys?L('File di sistema mostrati','System files shown'):L('File di sistema nascosti','System files hidden'))+' ('+hidden+')</span><button onclick="fxToggleSys()">'+(fx.showSys?L('Nascondi','Hide'):L('Mostra','Show'))+'</button>';
  let side=(p,icon,label,cls)=>'<button data-p="'+p+'" class="'+(cls||'')+(dir===p||(p!=='/'&&dir.startsWith(p+'/'))?' on':'')+'">'+ICON[icon]+escapeHtml(label)+'</button>';
  $('fxSide').innerHTML='<div class="lab">'+L('Raccolte','Collections')+'</div>'+side('/BOOKS','book',L('Libri','Books'))+side('/AUDIO','music',L('Audiolibri','Audiobooks'))+side('/SLEEP','img',L('Sfondi','Wallpapers'))+'<div class="lab">'+L('Scheda di memoria','Memory card')+'</div><button data-p="/" class="'+(dir==='/'?'on':'')+'">'+ICON.folder+L('Tutti i file','All files')+'</button><button class="sub'+(fx.showSys?' on':'')+'" onclick="fxToggleSys()">'+ICON.eye+L('File di sistema','System files')+'</button>';
  $('fxDropHint').textContent=L('Trascina qui i file per caricarli in «','Drag files here to upload them to "')+(dir==='/'?L('Scheda di memoria','Memory card'):fxFriendly(parentOf(dir),baseName(dir)))+L('»','"');
  fxRenderSelection(list);
}
// The buttons that carry an icon, labelled once the language is known.
function fxLabels(){
  $('fxUp').innerHTML=ICON.up;$('fxSearchBtn').innerHTML=ICON.search;$('fxMoreBtn').innerHTML=ICON.more;
  $('fxUploadP').innerHTML=ICON.upload+L('Carica qui','Upload here');$('fxUploadD').innerHTML=ICON.upload+L('Carica','Upload');
  $('fxNewP').innerHTML=$('fxNewD').innerHTML=ICON.plus+L('Nuova cartella','New folder');
}
function fxRenderSelection(list){
  let items=fxSelItems();
  let size=items.reduce((sum,i)=>sum+(i.folder?0:i.size),0);
  $('fxStatus').textContent=(list?list.length:fxVisible().length)+' '+L('elementi','items')+(items.length?' · '+items.length+' '+(items.length===1?L('selezionato','selected'):L('selezionati','selected'))+(size?' ('+formatBytes(size)+')':''):'');
  let bar=$('selbar');
  if(items.length===0||activeTab!=='files'){if(bar)bar.remove();return}
  if(!bar){bar=document.createElement('div');bar.id='selbar';bar.className='selbar';document.body.appendChild(bar)}
  bar.innerHTML='<span>'+items.length+' '+(items.length===1?L('selezionato','selected'):L('selezionati','selected'))+'</span><button onclick="fxBulk(\'download\')">'+ICON.down+L('Scarica','Download')+'</button><button onclick="fxBulk(\'move\')">'+ICON.move+L('Sposta','Move')+'</button><button class="del" onclick="fxBulk(\'delete\')">'+ICON.trash+L('Elimina','Delete')+'</button><button onclick="fxClearSel()" aria-label="'+L('Chiudi','Close')+'">'+ICON.x+'</button>';
}
// The selection changed and nothing else: the rows stay the same elements
// (a double click needs its two clicks to land on one), only their mark moves.
function fxPaintSel(){
  document.querySelectorAll('#fxRows .row').forEach(row=>row.classList.toggle('sel',fx.sel.has(row.dataset.n)));
  fxRenderSelection();
}
function fxClearSel(){fx.sel.clear();fx.selMode=false;fx.anchor=null;fxRender()}
function fxToggle(name){if(fx.sel.has(name))fx.sel.delete(name);else fx.sel.add(name);fx.anchor=name;if(fx.sel.size===0)fx.selMode=false;fxRender()}
function fxToggleSys(){fx.showSys=!fx.showSys;fxRender()}
function fxToggleSearch(){let bar=$('fxSearchBar');bar.hidden=!bar.hidden;if(bar.hidden){$('fxSearchP').value='';fxRender()}else $('fxSearchP').focus()}
function fxSortBy(key){if(fx.sort===key)fx.asc=!fx.asc;else{fx.sort=key;fx.asc=true}fxRender()}
function fxToolbarMenu(anchor){
  let mark=key=>fx.sort===key?'check':null;
  openMenu(anchor,[
    {icon:'select',label:fx.selMode?L('Fine selezione','Done selecting'):L('Seleziona','Select'),run:()=>{if(fx.selMode)fxClearSel();else{fx.selMode=true;fxRender()}}},
    '-',{label:L('Ordina per','Sort by')},
    {icon:mark('name'),label:L('Nome','Name'),run:()=>fxSortBy('name')},
    {icon:mark('date'),label:L('Data','Date'),run:()=>fxSortBy('date')},
    {icon:mark('size'),label:L('Dimensione','Size'),run:()=>fxSortBy('size')},
    '-',{icon:'eye',label:fx.showSys?L('Nascondi i file di sistema','Hide system files'):L('Mostra i file di sistema','Show system files'),run:fxToggleSys}
  ]);
}
function fxActivate(name,row){
  let e=fxEntry(name);
  if(!e)return;
  let path=joinPath(fx.path,name);
  if(e.kind==='folder')fxOpen(path);
  else if(fxPreviewable(e))fxPreview(path,e);
  else fxRowMenu(name,row?row.querySelector('.more'):null);
}
function fxRowMenu(name,at){
  let e=fxEntry(name);
  if(!e)return;
  let path=joinPath(fx.path,name),folder=e.kind==='folder',items=[];
  if(folder)items.push({icon:'open',label:L('Apri','Open'),run:()=>fxOpen(path)});
  else{
    if(fxPreviewable(e))items.push({icon:'eye',label:L('Anteprima','Preview'),run:()=>fxPreview(path,e)});
    items.push({icon:'down',label:L('Scarica','Download'),run:()=>downloadPaths([path])});
  }
  items.push({icon:'edit',label:L('Rinomina','Rename'),run:()=>renameEntry(path,()=>fxOpen(fx.path))});
  items.push({icon:'move',label:L('Sposta in...','Move to...'),run:()=>fxMove([path])});
  items.push('-');
  items.push({icon:'trash',label:L('Elimina','Delete'),danger:true,run:()=>fxDelete([{path:path,folder:folder}])});
  openMenu(at||{x:innerWidth/2-100,y:innerHeight/2-120},items);
}
function fxBulkMenu(at){
  let n=fx.sel.size;
  openMenu(at,[{label:n+' '+L('selezionati','selected')},
    {icon:'down',label:L('Scarica','Download'),run:()=>fxBulk('download')},
    {icon:'move',label:L('Sposta in...','Move to...'),run:()=>fxBulk('move')},
    '-',{icon:'trash',label:L('Elimina','Delete'),danger:true,run:()=>fxBulk('delete')}]);
}
function fxBulk(action){
  let items=fxSelItems();
  if(!items.length)return;
  if(action==='download'){
    let files=items.filter(i=>!i.folder).map(i=>i.path);
    if(files.length<items.length)toast(L('Le cartelle non si scaricano: scarico solo i file.','Folders cannot be downloaded: only the files are.'));
    downloadPaths(files);
  }else if(action==='move')fxMove(items.map(i=>i.path));
  else fxDelete(items);
}
function fxDelete(items){
  fx.sel.clear();fx.selMode=false;
  deleteWithUndo(items,done=>{if(done){fxCounts.clear();if(activeTab==='files')fxOpen(fx.path,true)}else fxRender()});
}
async function fxNewFolder(){
  let name=await askText(L('Nuova cartella','New folder'),'',L('Crea','Create'));
  if(!name)return;
  try{await api('/api/mkdir?path='+enc(joinPath(fx.path,safeName(name))),{method:'POST'});fxCounts.clear();fxOpen(fx.path)}catch(e){fail(e)}
}
async function fxMoveTo(paths,dest){
  let moved=0,error=null;
  for(const p of paths){
    if(parentOf(p)===dest)continue;
    if(dest===p||dest.startsWith(p+'/')){error=new Error(L('Una cartella non può essere spostata dentro sé stessa.','A folder cannot be moved into itself.'));continue}
    try{await api('/api/rename?from='+enc(p)+'&to='+enc(joinPath(dest,baseName(p))),{method:'POST'});moved++}catch(e){error=e}
  }
  fxCounts.clear();
  await fxOpen(fx.path);
  if(error)toast(error.message&&/sé stessa|itself/.test(error.message)?error.message:explainError(error),{error:true});
  else if(moved)toast(L(moved===1?'Spostato in «':moved+' elementi spostati in «',moved===1?'Moved to "':moved+' items moved to "')+(dest==='/'?L('Scheda','Card'):fxFriendly(parentOf(dest),baseName(dest)))+L('»','"'));
}
async function fxMove(paths){
  let dest=await pickFolder(L('Sposta in...','Move to...'),fx.path,L('Sposta qui','Move here'));
  if(dest!==null)fxMoveTo(paths,dest);
}
// A folder chosen by walking the card, as in any "move to" window.
function pickFolder(title,start,okLabel){
  return new Promise(resolve=>{
    let here=start,answered=false;
    let sheet=openSheet('<h2>'+escapeHtml(title)+'</h2><div class="path" id="pickPath"></div><div class="pick" id="pickRows"></div><div class="actions"><button class="btn" id="pickNo">'+L('Annulla','Cancel')+'</button><button class="btn primary" id="pickOk">'+escapeHtml(okLabel)+'</button></div>',()=>{if(!answered)resolve(null)});
    async function show(path){
      try{
        let folders=JSON.parse(await api('/api/list?path='+enc(path))).filter(e=>e.kind==='folder'&&(fx.showSys||!fxIsSystem(path,e)));
        folders.sort((a,b)=>fxFriendly(path,a.name).localeCompare(fxFriendly(path,b.name),undefined,{numeric:true,sensitivity:'base'}));
        here=path;
        let crumbs='<button data-p="/">'+L('Scheda','Card')+'</button>',acc='';
        path.split('/').filter(Boolean).forEach((part,i)=>{let parent=acc||'/';acc+='/'+part;crumbs+='<span>›</span><button data-p="'+escapeHtml(acc)+'">'+escapeHtml(i===0?fxFriendly(parent,part):part)+'</button>'});
        sheet.querySelector('#pickPath').innerHTML=crumbs;
        sheet.querySelector('#pickRows').innerHTML=folders.map(e=>'<div class="row" tabindex="0" data-p="'+escapeHtml(joinPath(path,e.name))+'"><span class="ic">'+ICON.folder+'</span><div class="nm"><div>'+escapeHtml(fxFriendly(path,e.name))+'</div></div></div>').join('')||'<div class="empty-note">'+L('Nessuna cartella qui dentro.','No folders in here.')+'</div>';
      }catch(e){fail(e)}
    }
    sheet.addEventListener('click',e=>{let el=e.target.closest('[data-p]');if(el)show(el.dataset.p)});
    sheet.querySelector('#pickOk').onclick=()=>{answered=true;closeSheet();resolve(here)};
    sheet.querySelector('#pickNo').onclick=closeSheet;
    show(start);
  });
}
async function fxPreview(path,e){
  let ext=fxExt(e.name),body;
  if(['bmp','png','jpg','jpeg'].includes(ext)){
    // A sleep image is stored turned: shown the way the device shows it.
    body=path.startsWith('/SLEEP/')&&ext==='bmp'?'<div class="sleep-thumb" style="max-width:240px;margin:.4rem auto" data-empty="'+L('Anteprima non disponibile','No preview')+'"><canvas id="previewCanvas" width="480" height="800"></canvas></div>':'<img class="preview" src="/api/download?path='+enc(path)+'" alt="">';
  }else{
    try{let text=await api('/api/download?path='+enc(path));body='<pre>'+escapeHtml(text.slice(0,20000))+(text.length>20000?'\n...':'')+'</pre>'}catch(err){fail(err);return}
  }
  let sheet=openSheet('<h2 style="overflow-wrap:anywhere">'+escapeHtml(e.name)+'</h2><p class="hint">'+escapeHtml(fxTypeLabel(fx.path,e)+' · '+formatBytes(e.size)+(e.modified?' · '+formatDate(e.modified,false):''))+'</p>'+body+'<div class="actions"><button class="btn" id="pvClose">'+L('Chiudi','Close')+'</button><button class="btn primary" id="pvDown">'+ICON.down+L('Scarica','Download')+'</button></div>');
  sheet.querySelector('#pvClose').onclick=closeSheet;
  sheet.querySelector('#pvDown').onclick=()=>downloadPaths([path]);
  let canvas=sheet.querySelector('#previewCanvas');
  if(canvas)drawSleepThumb(canvas,'/api/download?path='+enc(path),e.name+':'+e.size+':'+e.modified);
}
const fxUploader=createUploader({containerId:'queueFx',getDir:()=>fx.path,getExisting:dir=>dir===fx.path?new Set(fx.entries.map(e=>e.name.toLowerCase())):new Set(),onDone:()=>{status(L('Caricamento completato','Upload complete'));fxCounts.clear();fxOpen(fx.path,true);fetchSpace()}});
$('fileFx').addEventListener('change',e=>{if(e.target.files.length)fxUploader.handleFiles(e.target.files);e.target.value=''});
// --- gesti sulle righe ---
(function(){
  let rows=$('fxRows');
  rows.addEventListener('click',e=>{
    if(fxSkipClick){fxSkipClick=false;return}
    let row=e.target.closest('.row');
    if(!row)return;
    let name=row.dataset.n,more=e.target.closest('.more');
    if(more){fxRowMenu(name,more);return}
    if(e.target.closest('.check')){fxToggle(name);return}
    if(fxMouse.matches){
      if(e.detail>=2){fxActivate(name,row);return}
      if(e.shiftKey&&fx.anchor){
        let names=fxVisible().map(x=>x.name),a=names.indexOf(fx.anchor),b=names.indexOf(name);
        if(a>=0&&b>=0){fx.sel=new Set(names.slice(Math.min(a,b),Math.max(a,b)+1));fxPaintSel();return}
      }
      if(e.ctrlKey||e.metaKey){if(fx.sel.has(name))fx.sel.delete(name);else fx.sel.add(name);fx.anchor=name;fxPaintSel();return}
      fx.sel=new Set([name]);fx.anchor=name;fxPaintSel();
    }else if(fx.selMode)fxToggle(name);
    else fxActivate(name,row);
  });
  rows.addEventListener('contextmenu',e=>{
    let row=e.target.closest('.row');
    if(!row)return;
    e.preventDefault();
    let name=row.dataset.n;
    if(!fx.sel.has(name)){fx.sel=new Set([name]);fx.anchor=name;fxPaintSel()}
    if(fx.sel.size>1)fxBulkMenu({x:e.clientX,y:e.clientY});else fxRowMenu(name,{x:e.clientX,y:e.clientY});
  });
  // A long press on a touch screen starts a selection.
  rows.addEventListener('pointerdown',e=>{
    let row=e.target.closest('.row');
    if(!row||e.pointerType==='mouse'||e.target.closest('.more'))return;
    let name=row.dataset.n;
    clearTimeout(fxLongPress);
    fxLongPress=setTimeout(()=>{fxSkipClick=true;fx.selMode=true;fx.sel.add(name);fx.anchor=name;fxRender();if(navigator.vibrate)navigator.vibrate(15)},550);
  });
  ['pointerup','pointercancel','pointermove','scroll'].forEach(ev=>rows.addEventListener(ev,e=>{if(ev!=='pointermove'||Math.abs(e.movementX)+Math.abs(e.movementY)>4)clearTimeout(fxLongPress)},{passive:true}));
  rows.addEventListener('keydown',e=>{let row=e.target.closest('.row');if(row&&e.key==='Enter'){e.preventDefault();fxActivate(row.dataset.n,row)}});
  // Drag: rows onto a folder to move them, files from the computer to upload.
  rows.addEventListener('dragstart',e=>{
    let row=e.target.closest('.row');
    if(!row)return;
    let name=row.dataset.n;
    if(!fx.sel.has(name)){fx.sel=new Set([name]);fx.anchor=name;fxPaintSel()}
    fxDragging=fxSelItems().map(i=>i.path);
    e.dataTransfer.effectAllowed='move';
    try{e.dataTransfer.setData('text/plain',name)}catch(err){}
  });
  rows.addEventListener('dragend',()=>{fxDragging=null;document.querySelectorAll('.dragover').forEach(el=>el.classList.remove('dragover'))});
  function target(e){
    // A folder row, a side-panel entry, or the list itself (the open folder).
    let row=e.target.closest('#fxRows .row');
    if(row){let entry=fxEntry(row.dataset.n);if(entry&&entry.kind==='folder')return {el:row,path:joinPath(fx.path,entry.name)}}
    let side=e.target.closest('#fxSide button[data-p]');
    if(side)return {el:side,path:side.dataset.p};
    return {el:$('fxDropHint'),path:fx.path};
  }
  let zone=$('filesCard');
  zone.addEventListener('dragover',e=>{
    let t=target(e);
    if(fxDragging&&t.path===fx.path)return;
    e.preventDefault();
    document.querySelectorAll('.dragover').forEach(el=>el.classList.remove('dragover'));
    t.el.classList.add('dragover');
  });
  zone.addEventListener('dragleave',e=>{if(!zone.contains(e.relatedTarget))document.querySelectorAll('.dragover').forEach(el=>el.classList.remove('dragover'))});
  zone.addEventListener('drop',e=>{
    let t=target(e);
    document.querySelectorAll('.dragover').forEach(el=>el.classList.remove('dragover'));
    if(fxDragging){e.preventDefault();let paths=fxDragging;fxDragging=null;if(t.path!==fx.path)fxMoveTo(paths,t.path);return}
    if(e.dataTransfer.files&&e.dataTransfer.files.length){e.preventDefault();fxUploader.handleFiles(e.dataTransfer.files,t.path)}
  });
  $('fxPath').addEventListener('click',e=>{let b=e.target.closest('button[data-p]');if(b)fxOpen(b.dataset.p)});
  $('fxSide').addEventListener('click',e=>{let b=e.target.closest('button[data-p]');if(b)fxOpen(b.dataset.p)});
  $('fxHead').addEventListener('click',e=>{let b=e.target.closest('button[data-s]');if(b)fxSortBy(b.dataset.s)});
  document.addEventListener('keydown',e=>{
    if(activeTab!=='files'||$('shade')||/^(INPUT|TEXTAREA)$/.test(e.target.tagName))return;
    let one=fx.sel.size===1?Array.from(fx.sel)[0]:null;
    if(e.key==='F2'&&one){e.preventDefault();renameEntry(joinPath(fx.path,one),()=>fxOpen(fx.path))}
    else if(e.key==='Delete'&&fx.sel.size){e.preventDefault();fxDelete(fxSelItems())}
    else if(e.key==='Enter'&&one&&!e.target.closest('.row')){e.preventDefault();fxActivate(one)}
    else if(e.key==='Backspace'){e.preventDefault();fxGoUp()}
    else if(e.key==='Escape'&&fx.sel.size)fxClearSel();
    else if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='a'){e.preventDefault();fx.sel=new Set(fxVisible().map(x=>x.name));fxRender()}
  });
  addEventListener('resize',()=>{if(activeTab==='files'&&fx.loaded)fxRender()});
})();

// --- Wi-Fi: le reti salvate si vedono e si dimenticano sempre (e solo una
// modifica a WIFI.TXT); la ricerca delle reti vicine funziona solo
// dall'hotspot del dispositivo, e unirsi a una rete nuova viene prima
// provato con una connessione vera (vedi NetworkRuntime::try_join_candidate
// lato firmware).
function wifiBars(rssi){
  let n=rssi>=-55?4:rssi>=-67?3:rssi>=-75?2:1;
  let label=[L('segnale debole','weak signal'),L('segnale discreto','fair signal'),L('segnale buono','good signal'),L('segnale ottimo','excellent signal')][n-1];
  return '<span class="bars" role="img" title="'+label+'" aria-label="'+label+'">'+[1,2,3,4].map(i=>'<i class="'+(i<=n?'on':'')+'" style="height:'+(i*4)+'px"></i>').join('')+'</span>';
}
async function wifiLoadSaved(){
  try{
    let list=JSON.parse(await api('/api/networks'));
    let html='';
    for(const net of list){
      let s=escapeHtml(net.ssid);
      html+='<div class="net"><div class="name">'+s+'</div><button class="btn small" data-s="'+s+'" onclick="wifiOpenJoin(this.dataset.s,true)">'+L('Cambia password','Change password')+'</button><button class="btn small danger" data-s="'+s+'" onclick="wifiForget(this.dataset.s)">'+L('Dimentica','Forget')+'</button></div>';
    }
    $('wifiSaved').innerHTML=html||'<p class="hint">'+L('Nessuna rete salvata.','No saved networks.')+'</p>';
  }catch(e){$('wifiSaved').innerHTML='<p class="hint">'+escapeHtml(explainError(e))+'</p>'}
}
async function wifiLoadScan(){
  $('wifiLanNote').hidden=portalHotspot;
  if(!portalHotspot){$('wifiScan').innerHTML='';return}
  try{
    let list=JSON.parse(await api('/api/scan'));
    let html='';
    for(const net of list){
      let s=escapeHtml(net.ssid);
      html+='<div class="net"><div class="name">'+wifiBars(net.rssi)+'<span>'+s+'</span></div><button class="btn small primary" data-s="'+s+'" onclick="wifiOpenJoin(this.dataset.s,true)">'+L('Collega','Connect')+'</button></div>';
    }
    $('wifiScan').innerHTML=html||'<p class="hint">'+L('Nessuna rete trovata per ora. La ricerca si ripete da sola ogni pochi secondi.','No networks found yet. The search repeats by itself every few seconds.')+'</p>';
  }catch(e){}
}
function wifiShow(){wifiLoadSaved();wifiLoadScan()}
function wifiOpenJoin(ssid,locked){
  let sheet=openSheet('<h2 style="overflow-wrap:anywhere">'+(ssid?L('Collega a «','Connect to "')+escapeHtml(ssid)+L('»','"'):L('Aggiungi una rete','Add a network'))+'</h2>'
    +'<input type="text" id="wifiSsid" autocomplete="off" autocapitalize="none" placeholder="'+L('Nome della rete','Network name')+'"'+(locked?' hidden':'')+'>'
    +'<div class="pw"><input type="password" id="wifiPw" autocomplete="off" autocapitalize="none" placeholder="'+L('Password (vuota se la rete è aperta)','Password (empty for an open network)')+'"><button class="btn" id="wifiPwShow">'+L('Mostra','Show')+'</button></div>'
    +(portalHotspot?'':'<p class="hint">'+L('Il dispositivo lascia la rete su cui è adesso per provare questa. Se riesce, lo ritrovi al nuovo indirizzo mostrato sul suo schermo; se non riesce, torna qui da solo.','The device leaves the network it is on to try this one. If it works, you find it at the new address shown on its screen; if it fails, it comes back here by itself.')+'</p>')
    +'<p class="hint" id="wifiJoinStatus" role="status"></p>'
    +'<div class="actions"><button class="btn" id="wifiNo">'+L('Annulla','Cancel')+'</button><button class="btn primary" id="wifiOk">'+L('Collega e salva','Connect and save')+'</button></div>');
  let ssidInput=sheet.querySelector('#wifiSsid'),pw=sheet.querySelector('#wifiPw');
  ssidInput.value=ssid||'';
  sheet.querySelector('#wifiPwShow').onclick=function(){let hide=pw.type==='text';pw.type=hide?'password':'text';this.textContent=hide?L('Mostra','Show'):L('Nascondi','Hide')};
  sheet.querySelector('#wifiNo').onclick=closeSheet;
  sheet.querySelector('#wifiOk').onclick=()=>wifiSubmitJoin(ssidInput.value.trim(),pw.value);
  (locked?pw:ssidInput).focus();
}
function wifiJoinNote(text){let el=$('wifiJoinStatus');if(el)el.textContent=text}
async function wifiSubmitJoin(ssid,password){
  if(!ssid){wifiJoinNote(L('Scrivi il nome della rete.','Type the network name.'));return}
  wifiJoinNote(L('Provo a collegarmi...','Trying to connect...'));
  try{await api('/api/networks?ssid='+enc(ssid)+'&password='+enc(password),{method:'POST'})}catch(e){wifiJoinNote(explainError(e));return}
  wifiPollJoin(0);
}
async function wifiPollJoin(misses){
  if(!$('wifiJoinStatus'))return;
  try{
    let s=await readStatus(true);
    let j=s.wifi_join||{state:'idle'};
    if(j.state==='connected'){wifiJoinNote(L('Collegata e salvata.','Connected and saved.'));wifiLoadSaved();setTimeout(closeSheet,1500);return}
    if(j.state==='failed'){wifiJoinNote(L('Non sono riuscito a collegarmi: ','Could not connect: ')+(j.error||L('controlla la password.','check the password.')));return}
    setTimeout(()=>wifiPollJoin(0),1000);
  }catch(e){
    // The device left this network to try the new one.
    if(misses>=3){wifiJoinNote(L('Il dispositivo ha lasciato questa rete per provare quella nuova. Guarda il suo schermo: se è riuscito mostra il nuovo indirizzo, altrimenti torna qui tra poco.','The device left this network to try the new one. Look at its screen: if it worked it shows the new address, otherwise it is back here shortly.'));setTimeout(()=>wifiPollJoin(misses),3000);return}
    setTimeout(()=>wifiPollJoin(misses+1),1500);
  }
}
async function wifiForget(ssid){
  if(!await askConfirm(L('Dimenticare la rete?','Forget this network?'),L('«'+ssid+'» verrà tolta dalle reti salvate. Per riaverla servirà di nuovo la password.','"'+ssid+'" will be removed from the saved networks. Getting it back needs the password again.'),L('Dimentica','Forget')))return;
  try{await api('/api/networks/delete?ssid='+enc(ssid),{method:'POST'});setTimeout(wifiLoadSaved,400)}catch(e){fail(e)}
}

// --- schede e avvio ---
function applyLang(){
  document.documentElement.lang=LANG;
  if(LANG!=='en')return;
  document.querySelectorAll('[data-en]').forEach(el=>{el.textContent=el.dataset.en});
  document.querySelectorAll('[data-en-ph]').forEach(el=>{el.placeholder=el.dataset.enPh});
  document.querySelectorAll('[data-en-label]').forEach(el=>{el.setAttribute('aria-label',el.dataset.enLabel)});
}
function showTab(name){
  closeMenu();
  document.querySelectorAll('.tabpanel').forEach(el=>{el.hidden=true});
  document.querySelectorAll('.tab').forEach(el=>el.classList.toggle('active',el.dataset.tab===name));
  $('tab-'+name).hidden=false;
  activeTab=name;
  fxRenderSelection();
  if(name==='books')refreshBooks();
  else if(name==='audio')refreshAudio();
  else if(name==='wallpaper'){showWallpaperSources();refreshSleepGallery()}
  else if(name==='files')fxOpen(fx.path,true);
  else if(name==='wifi')wifiShow();
}
function refreshActiveTab(){fxCounts.clear();showTab(activeTab);fetchSpace()}
async function initApp(){
  $('reloadBtn').innerHTML=ICON.reload;
  let s=null;
  try{s=await readStatus(true)}catch(e){showLink(false)}
  if(s&&s.lang==='en'){LANG='en';showSpace(s.free_bytes)}
  applyLang();
  fxLabels();
  showLink(linkUp);
  // On the device's own hotspot the reason to be here is the Wi-Fi.
  showTab(portalHotspot?'wifi':'books');
  // Often enough that leaving the device's screen shows here within a few
  // seconds, before a tap finds the page disconnected.
  setInterval(pollLink,5000);
  setInterval(()=>{if(activeTab==='wifi'&&portalHotspot&&document.visibilityState==='visible')wifiLoadScan()},12000);
}

initApp();
</script></body></html>"##;

    #[derive(Debug)]
    struct SharedStatus {
        snapshot: WifiTransferSnapshot,
        last_activity: Instant,
        /// Previous `/api/books` scan, reused as `scan_txt_library`'s
        /// `previous` argument so a book whose path/size/mtime is unchanged
        /// keeps its already-known title instead of paying a fresh EPUB
        /// ZIP-parsing worker spawn on every single poll of this endpoint
        /// (the Libri tab's `refreshBooks()` calls it on every tab switch
        /// and once per uploaded file). Empty is always a safe start: it
        /// only ever means the first call after the portal starts re-derives
        /// every title once, exactly like today.
        cached_books: Vec<ReaderBook>,
        /// Latest Wi-Fi scan, only ever populated while reachable via the
        /// bootstrap hotspot (see the module docs).
        scan: Vec<WifiScanEntry>,
        saved_ssids: Vec<String>,
        pending_join: Option<PendingJoinRequest>,
        pending_delete: Option<String>,
        /// The Display setting's sleep screen and the wallpaper standby
        /// shows now, as the runtime owner last told them (see
        /// [`WifiTransferServer::set_sleep_screen`]); the page reads them
        /// from `/api/status`.
        sleep_mode: SleepScreenMode,
        sleep_fixed: Option<String>,
        pending_sleep_screen: Option<SleepScreenRequest>,
    }

    impl SharedStatus {
        fn new(url: String, ap_ssid: Option<String>, ap_password: Option<String>) -> Self {
            Self {
                snapshot: WifiTransferSnapshot {
                    state: WifiTransferState::Ready,
                    url: Some(url),
                    ap_ssid,
                    ap_password,
                    hotspot_clients: 0,
                    join: JoinAttemptState::Idle,
                    last_action: "Portal ready".into(),
                    last_bytes: 0,
                    error: None,
                },
                last_activity: Instant::now(),
                cached_books: Vec::new(),
                scan: Vec::new(),
                saved_ssids: Vec::new(),
                pending_join: None,
                pending_delete: None,
                sleep_mode: SleepScreenMode::default(),
                sleep_fixed: None,
                pending_sleep_screen: None,
            }
        }

        fn touch(&mut self, action: impl Into<String>, bytes: usize) {
            self.snapshot.last_action = action.into();
            self.snapshot.last_bytes = bytes;
            self.snapshot.error = None;
            self.last_activity = Instant::now();
        }

        /// Lighter-weight sibling of [`Self::touch`] for the Wi-Fi endpoints:
        /// resets the inactivity clock without touching `last_action`/
        /// `last_bytes`/`error`, which describe file-transfer activity, not
        /// Wi-Fi scan/join/forget requests.
        fn touch_activity(&mut self) {
            self.last_activity = Instant::now();
        }
    }

    /// One line per request the hotspot's catch-all redirects: the address
    /// a phone asked for while deciding whether this network needs a
    /// sign-in page. `warn` (the level release builds print) only while
    /// [`crate::dns_captive_portal::DIAGNOSTIC_LOG`] is on.
    fn log_captive_probe(method: &str, uri: &str, host: Option<&str>) {
        let host = host.unwrap_or("-");
        if crate::dns_captive_portal::DIAGNOSTIC_LOG {
            warn!("rustmix-wave=wifi-transfer-server status=captive-probe method={method} host={host} uri={uri}");
        } else {
            info!("rustmix-wave=wifi-transfer-server status=captive-probe method={method} host={host} uri={uri}");
        }
    }

    /// RAII wrapper.  Constructing starts ESP-IDF's dedicated HTTP task;
    /// dropping stops that task and frees its resources (and, when started
    /// via [`Self::start_ap`], the DNS hijack alongside it -- the caller is
    /// expected to call `NetworkRuntime::stop_provisioning` at the same
    /// time).
    pub struct WifiTransferServer {
        _server: EspHttpServer<'static>,
        _dns: Option<CaptivePortalDns>,
        shared: Arc<Mutex<SharedStatus>>,
    }

    impl WifiTransferServer {
        /// Start reachable on the LAN address the device already has (no
        /// radio changes): the common case once Wi-Fi is configured.
        pub fn start_lan(ipv4: &str) -> Result<Self> {
            let url = format!("http://{ipv4}/");
            Self::start_inner(url, None)
        }

        /// Start reachable via the device's own bootstrap hotspot instead,
        /// for when no Wi-Fi is joined yet. `ap_ssid`/`ap_password` come from
        /// the `NetworkRuntime::start_provisioning` call the caller is
        /// expected to have just made.
        pub fn start_ap(portal_ip: &str, ap_ssid: String, ap_password: String) -> Result<Self> {
            let answer_ip = portal_ip
                .parse::<std::net::Ipv4Addr>()
                .with_context(|| format!("portal IP is not a valid IPv4 address: {portal_ip}"))?
                .octets();
            // Answer every DNS query on the hotspot with our own address, so
            // the phone's captive-portal probe (whatever hostname it picks)
            // lands on this HTTP server and the OS offers to open it.
            let dns = CaptivePortalDns::start(answer_ip)?;
            let url = format!("http://{portal_ip}/");
            Self::start_inner(url, Some((ap_ssid, ap_password, dns)))
        }

        fn start_inner(
            url: String,
            ap: Option<(String, String, CaptivePortalDns)>,
        ) -> Result<Self> {
            // Suspend Wi-Fi modem-sleep for the life of the portal session:
            // the background default (`WIFI_PS_MAX_MODEM`, applied once
            // associated -- see network.rs) trades latency for battery, which
            // would otherwise throttle this server's upload/download
            // throughput. Restored to the background default on `Drop`.
            let status = unsafe { sys::esp_wifi_set_ps(sys::wifi_ps_type_t_WIFI_PS_NONE) };
            if status != sys::ESP_OK {
                warn!("rustmix-wave=wifi-power-save status=failed mode=none error-code={status}");
            }
            let (ap_ssid, ap_password, dns) = match ap {
                Some((ssid, password, dns)) => (Some(ssid), Some(password), Some(dns)),
                None => (None, None, None),
            };
            let shared = Arc::new(Mutex::new(SharedStatus::new(
                url.clone(),
                ap_ssid,
                ap_password,
            )));
            let mut server = EspHttpServer::new(&Configuration {
                http_port: WIFI_TRANSFER_HTTP_PORT,
                stack_size: WIFI_TRANSFER_SERVER_STACK_BYTES,
                // Task stack in PSRAM instead of internal RAM (ESP-IDF's
                // default): the hotspot portal ran internal RAM out right
                // after start (pthread_mutex_init ENOMEM, then a panic).
                // Safe here because no handler writes the internal flash
                // (NVS/OTA), which would disable the cache a PSRAM stack
                // lives behind; the same holds for the firmware's PSRAM
                // worker threads (`runtime_worker`).
                task_caps: esp_idf_svc::sys::MALLOC_CAP_SPIRAM | esp_idf_svc::sys::MALLOC_CAP_8BIT,
                max_open_sockets: 4,
                max_sessions: 4,
                max_uri_handlers: 20,
                // esp-idf-svc's default, spelled out because the hotspot
                // depends on it: when all sockets are taken, the least
                // recently used one makes room for a new connection.
                lru_purge_enable: true,
                session_timeout: Duration::from_secs(60),
                // Lets the catch-all handler registered last, only when
                // started via `start_ap`, use the glob pattern "*" instead of
                // requiring an exact URI match. Harmless when no such handler
                // is registered (the `start_lan` case).
                uri_match_wildcard: true,
                ..Default::default()
            })?;

            server.fn_handler("/", Method::Get, move |request| {
                request
                    .into_ok_response()?
                    .write_all(PORTAL_HTML.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let list_shared = Arc::clone(&shared);
            server.fn_handler("/api/list", Method::Get, move |request| {
                let relative = query_value(request.uri(), "path").unwrap_or_else(|| "/".into());
                let body = list_directory_json(&relative)?;
                lock(&list_shared).touch(format!("Listed {relative}"), body.len());
                request.into_ok_response()?.write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let download_shared = Arc::clone(&shared);
            server.fn_handler("/api/download", Method::Get, move |request| {
                let relative = required_query(request.uri(), "path")?;
                let path = resolve_portal_path(&relative).map_err(|error| anyhow!(error))?;
                let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
                // Declared as a file to save, under its own name and with
                // its own type: left to the server's default (`text/html`)
                // the browser opened every download as a page. An `<img>`
                // pointed here still shows the picture.
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file");
                let disposition = download_content_disposition(name);
                let mut response = request.into_response(
                    200,
                    Some("OK"),
                    &[
                        ("Content-Type", download_content_type(name)),
                        ("Content-Disposition", disposition.as_str()),
                        ("Cache-Control", "no-store"),
                    ],
                )?;
                let mut buffer = [0_u8; WIFI_TRANSFER_STREAM_CHUNK_BYTES];
                let mut total = 0;
                loop {
                    let read = StdRead::read(&mut file, &mut buffer)?;
                    if read == 0 { break; }
                    response.write_all(&buffer[..read])?;
                    total += read;
                }
                lock(&download_shared).touch(format!("Downloaded {relative}"), total);
                info!("rustmix-wave=wifi-transfer-request method=GET route=download path={relative} bytes={total} status=completed");
                Ok::<(), anyhow::Error>(())
            })?;

            let upload_shared = Arc::clone(&shared);
            server.fn_handler("/api/upload", Method::Post, move |mut request| {
                ensure_same_origin(request.header("Origin"), request.header("Host"))?;
                let relative = required_query(request.uri(), "path")?;
                let path = resolve_portal_path(&relative).map_err(|error| anyhow!(error))?;
                let temporary = temporary_path(&path)?;
                if temporary.exists() {
                    fs::remove_file(&temporary)?;
                }
                let transfer_result = (|| -> Result<usize> {
                    let mut file = File::create(&temporary)
                        .with_context(|| format!("create {}", temporary.display()))?;
                    let mut buffer = [0_u8; WIFI_TRANSFER_STREAM_CHUNK_BYTES];
                    let mut total = 0;
                    loop {
                        let read = request.read(&mut buffer)?;
                        if read == 0 { break; }
                        total += read;
                        if total > WIFI_TRANSFER_MAX_UPLOAD_BYTES {
                            bail!("upload exceeds 64 MiB limit");
                        }
                        StdWrite::write_all(&mut file, &buffer[..read])?;
                    }
                    StdWrite::flush(&mut file)?;
                    Ok(total)
                })();
                let total = match transfer_result {
                    Ok(total) => total,
                    Err(error) => {
                        let _ = fs::remove_file(&temporary);
                        return Err(error);
                    }
                };
                commit_atomic_upload(&temporary, &path)?;
                lock(&upload_shared).touch(format!("Uploaded {relative}"), total);
                info!("rustmix-wave=wifi-transfer-request method=POST route=upload path={relative} bytes={total} status=completed");
                request.into_ok_response()?.write_all(b"uploaded")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let delete_shared = Arc::clone(&shared);
            server.fn_handler("/api/delete", Method::Post, move |request| {
                ensure_same_origin(request.header("Origin"), request.header("Host"))?;
                let relative = required_query(request.uri(), "path")?;
                let path = resolve_portal_path(&relative).map_err(|error| anyhow!(error))?;
                // A folder goes with what it holds only when the page asks
                // for that (`recursive=1`, after the user's own delete), and
                // never the card's top folder.
                let recursive = query_value(request.uri(), "recursive").is_some();
                if path.is_dir() {
                    if is_portal_root(&path) {
                        bail!("the card's top folder cannot be deleted");
                    }
                    if recursive {
                        fs::remove_dir_all(&path)?;
                    } else {
                        fs::remove_dir(&path)?;
                    }
                } else {
                    fs::remove_file(&path)?;
                }
                lock(&delete_shared).touch(format!("Deleted {relative}"), 0);
                info!("rustmix-wave=wifi-transfer-request method=POST route=delete path={relative} status=completed");
                request.into_ok_response()?.write_all(b"deleted")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let mkdir_shared = Arc::clone(&shared);
            server.fn_handler("/api/mkdir", Method::Post, move |request| {
                ensure_same_origin(request.header("Origin"), request.header("Host"))?;
                let relative = required_query(request.uri(), "path")?;
                let path = resolve_portal_path(&relative).map_err(|error| anyhow!(error))?;
                fs::create_dir(&path)?;
                lock(&mkdir_shared).touch(format!("Created {relative}"), 0);
                info!("rustmix-wave=wifi-transfer-request method=POST route=mkdir path={relative} status=completed");
                request.into_ok_response()?.write_all(b"created")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let rename_shared = Arc::clone(&shared);
            server.fn_handler("/api/rename", Method::Post, move |request| {
                ensure_same_origin(request.header("Origin"), request.header("Host"))?;
                let from = required_query(request.uri(), "from")?;
                let to = required_query(request.uri(), "to")?;
                let source = resolve_portal_path(&from).map_err(|error| anyhow!(error))?;
                let destination = resolve_portal_path(&to).map_err(|error| anyhow!(error))?;
                fs::rename(&source, &destination)?;
                lock(&rename_shared).touch(format!("Renamed {from}"), 0);
                info!("rustmix-wave=wifi-transfer-request method=POST route=rename from={from} to={to} status=completed");
                request.into_ok_response()?.write_all(b"renamed")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let status_shared = Arc::clone(&shared);
            server.fn_handler("/api/status", Method::Get, move |request| {
                // A page that is open and in use says so (`alive=1`), which
                // counts as activity: framing a wallpaper, say, sends nothing
                // else for minutes. A plain status read does not, so a page
                // left open and forgotten still lets the portal close.
                let alive = query_value(request.uri(), "alive").is_some();
                let (snapshot, sleep) = {
                    let mut guard = lock(&status_shared);
                    if alive {
                        guard.touch_activity();
                    }
                    (
                        guard.snapshot.clone(),
                        sleep_status_json(
                            guard.sleep_mode.marker(),
                            guard.sleep_fixed.as_deref(),
                        ),
                    )
                };
                let (total_bytes, free_bytes) = sd_space_bytes().unwrap_or((0, 0));
                let body = format!(
                    "{{\"state\":\"{}\",\"last_action\":\"{}\",\"last_bytes\":{},\"total_bytes\":{total_bytes},\"free_bytes\":{free_bytes},\"hotspot\":{},\"lang\":\"{}\",\"wifi_join\":{},\"sleep\":{sleep}}}",
                    snapshot.state.label(),
                    json_escape(&snapshot.last_action),
                    snapshot.last_bytes,
                    snapshot.ap_ssid.is_some(),
                    portal_locale_code(),
                    wifi_join_json(&snapshot.join),
                );
                request.into_ok_response()?.write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            // Wi-Fi tab endpoints. Reused from the retired standalone
            // provisioning portal: nearby-network scan results and saved
            // networks (list + forget) work in either mode, but a scan is
            // only ever non-empty and a join only ever processed while
            // reachable via `start_ap` -- see `main.rs`'s
            // `maintain_portal_server`, which gates that on the same
            // `via_hotspot` flag this snapshot's `ap_ssid` reflects.
            let scan_shared = Arc::clone(&shared);
            server.fn_handler("/api/scan", Method::Get, move |request| {
                let body = {
                    let mut guard = lock(&scan_shared);
                    guard.touch_activity();
                    scan_json(&guard.scan)
                };
                request
                    .into_response(200, Some("OK"), &[("Cache-Control", "no-store")])?
                    .write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let networks_shared = Arc::clone(&shared);
            server.fn_handler("/api/networks", Method::Get, move |request| {
                let body = {
                    let mut guard = lock(&networks_shared);
                    guard.touch_activity();
                    saved_json(&guard.saved_ssids)
                };
                request
                    .into_response(200, Some("OK"), &[("Cache-Control", "no-store")])?
                    .write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let join_shared = Arc::clone(&shared);
            server.fn_handler("/api/networks", Method::Post, move |mut request| {
                ensure_same_origin(request.header("Origin"), request.header("Host"))?;
                // Drain any request body (none expected; credentials travel
                // as query parameters like the rest of this tiny API) so the
                // connection can be reused.
                let mut discard = [0_u8; 64];
                while request.read(&mut discard)? > 0 {}
                let ssid = required_query(request.uri(), "ssid")?;
                let password = query_value(request.uri(), "password").unwrap_or_default();
                {
                    let mut guard = lock(&join_shared);
                    guard.touch_activity();
                    guard.pending_join = Some(PendingJoinRequest {
                        ssid: ssid.clone(),
                        password,
                    });
                    guard.snapshot.join = JoinAttemptState::Testing { ssid };
                }
                request.into_ok_response()?.write_all(b"testing")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let delete_net_shared = Arc::clone(&shared);
            server.fn_handler("/api/networks/delete", Method::Post, move |request| {
                ensure_same_origin(request.header("Origin"), request.header("Host"))?;
                let ssid = required_query(request.uri(), "ssid")?;
                {
                    let mut guard = lock(&delete_net_shared);
                    guard.touch_activity();
                    guard.pending_delete = Some(ssid);
                }
                request.into_ok_response()?.write_all(b"deleted")?;
                Ok::<(), anyhow::Error>(())
            })?;

            // The sleep screen, set on the page: what standby shows and,
            // when it is always the same wallpaper, which one. Only asked
            // for here: the display settings and the wallpaper catalog
            // belong to the main loop, which carries it out on its next turn
            // (see `main.rs`'s `maintain_portal_sleep_screen`). What
            // `/api/status` says changes at once, so the page that asked
            // sees it done.
            let sleep_shared = Arc::clone(&shared);
            server.fn_handler("/api/sleep", Method::Post, move |request| {
                ensure_same_origin(request.header("Origin"), request.header("Host"))?;
                let marker = required_query(request.uri(), "mode")?;
                let Some(mode) = SleepScreenMode::from_marker(&marker) else {
                    bail!("unknown sleep screen");
                };
                let name = query_value(request.uri(), "name").unwrap_or_default();
                let fixed = if mode == SleepScreenMode::Fixed && !name.is_empty() {
                    if !is_sleep_image_name(&name) {
                        bail!("not a wallpaper name");
                    }
                    let path = resolve_portal_path(&format!("/SLEEP/{name}"))
                        .map_err(|error| anyhow!(error))?;
                    if !path.is_file() {
                        bail!("no such wallpaper");
                    }
                    Some(name)
                } else {
                    None
                };
                {
                    let mut guard = lock(&sleep_shared);
                    guard.touch_activity();
                    guard.sleep_mode = mode;
                    if let Some(name) = fixed.as_ref() {
                        guard.sleep_fixed = Some(name.clone());
                    }
                    guard.pending_sleep_screen = Some(SleepScreenRequest { mode, fixed });
                }
                info!(
                    "rustmix-wave=wifi-transfer-request method=POST route=sleep mode={} status=accepted",
                    mode.marker()
                );
                request.into_ok_response()?.write_all(b"ok")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let books_shared = Arc::clone(&shared);
            server.fn_handler("/api/books", Method::Get, move |request| {
                let previous = lock(&books_shared).cached_books.clone();
                let (body, scanned) = books_json(&previous)?;
                {
                    let mut guard = lock(&books_shared);
                    guard.cached_books = scanned;
                    guard.touch("Listed books", body.len());
                }
                request
                    .into_response(200, Some("OK"), &[("Cache-Control", "no-store")])?
                    .write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let cover_shared = Arc::clone(&shared);
            server.fn_handler("/api/cover", Method::Get, move |request| {
                let relative = required_query(request.uri(), "path")?;
                let path = resolve_portal_path(&relative).map_err(|error| anyhow!(error))?;
                let book = book_from_path(&path)?;
                // Read-only against the same on-device thumbnail cache the
                // Library screen uses (see `reader_cover_cache_directory`):
                // never decodes a JPEG/PNG here. That work only ever runs
                // either in the browser (see the Wi-Fi transfer portal's
                // upload flow) or on-device from the Library screen's own
                // paced, one-thumbnail-per-frame loop — both proven safe.
                // Chaining a decode onto this HTTP request too was tried and
                // reliably pushed this hardware into "not enough space"
                // failures, so a cache miss here is just reported as one
                // rather than generated on demand.
                let thumbnail = CoverCache::new(reader_cover_cache_directory())
                    .load_cached_thumbnail(&book)
                    .ok_or_else(|| anyhow!("cover not cached yet"))?;
                let png_bytes = thumbnail_to_png(&thumbnail)?;
                lock(&cover_shared).touch(format!("Cover {relative}"), png_bytes.len());
                request
                    .into_response(
                        200,
                        Some("OK"),
                        &[("Content-Type", "image/png"), ("Cache-Control", "no-store")],
                    )?
                    .write_all(&png_bytes)?;
                Ok::<(), anyhow::Error>(())
            })?;

            // Catch-all, registered last (and thus tried last, after the
            // exact-path handlers above) via `uri_match_wildcard`, only when
            // started via `start_ap`. Any path a phone's OS probes to decide
            // whether this hotspot is a captive portal (Android
            // `/generate_204`, Apple `/hotspot-detect.html`, Windows
            // `/connecttest.txt` and `/redirect`, ...) lands here and gets
            // redirected to the portal itself, which both fails those
            // probes' "internet already works" check and, on Windows, is the
            // literal 3xx signal it looks for to auto-open the sign-in
            // browser. A body is included because iOS's captive-portal
            // detector specifically requires one -- a redirect with an empty
            // body is not enough for it to recognize a portal.
            //
            // `Connection: close` because the hotspot answers every DNS name
            // with its own address: the phone's background apps knock here
            // too, and the server has four sockets. (It also drops the least
            // recently used one for a newcomer, `lru_purge_enable`.) HEAD
            // gets the same answer without a body.
            let dns_guard = if let Some(dns) = dns {
                let redirect_url = url.clone();
                server.fn_handler("*", Method::Get, move |request| {
                    log_captive_probe("GET", request.uri(), request.header("Host"));
                    request
                        .into_response(
                            302,
                            Some("Found"),
                            &[
                                ("Location", redirect_url.as_str()),
                                ("Cache-Control", "no-store"),
                                ("Connection", "close"),
                            ],
                        )?
                        .write_all(b"Redirecting to the Rustmix-Wave portal.")?;
                    Ok::<(), anyhow::Error>(())
                })?;
                let redirect_url = url.clone();
                server.fn_handler("*", Method::Head, move |request| {
                    log_captive_probe("HEAD", request.uri(), request.header("Host"));
                    request.into_response(
                        302,
                        Some("Found"),
                        &[
                            ("Location", redirect_url.as_str()),
                            ("Cache-Control", "no-store"),
                            ("Connection", "close"),
                        ],
                    )?;
                    Ok::<(), anyhow::Error>(())
                })?;
                Some(dns)
            } else {
                None
            };

            info!("rustmix-wave=wifi-transfer-server status=ready mode={} url={url} root={WIFI_TRANSFER_ROOT} stack-bytes={WIFI_TRANSFER_SERVER_STACK_BYTES}", if dns_guard.is_some() { "hotspot" } else { "lan" });
            Ok(Self {
                _server: server,
                _dns: dns_guard,
                shared,
            })
        }

        #[must_use]
        pub fn snapshot(&self) -> WifiTransferSnapshot {
            lock(&self.shared).snapshot.clone()
        }

        pub fn set_scan_results(&self, entries: Vec<WifiScanEntry>) {
            lock(&self.shared).scan = entries;
        }

        pub fn set_saved_networks(&self, ssids: Vec<String>) {
            lock(&self.shared).saved_ssids = ssids;
        }

        pub fn take_pending_join(&self) -> Option<PendingJoinRequest> {
            lock(&self.shared).pending_join.take()
        }

        pub fn take_pending_delete(&self) -> Option<String> {
            lock(&self.shared).pending_delete.take()
        }

        /// The sleep screen the page asked for, once.
        pub fn take_pending_sleep_screen(&self) -> Option<SleepScreenRequest> {
            lock(&self.shared).pending_sleep_screen.take()
        }

        /// Tell the page the sleep screen as it is on the device: the
        /// Display setting and the wallpaper standby shows now. Left alone
        /// while a request from the page is still waiting, whose outcome
        /// `/api/status` already tells.
        pub fn set_sleep_screen(&self, mode: SleepScreenMode, fixed: Option<String>) {
            let mut guard = lock(&self.shared);
            if guard.pending_sleep_screen.is_some() {
                return;
            }
            guard.sleep_mode = mode;
            if guard.sleep_fixed != fixed {
                guard.sleep_fixed = fixed;
            }
        }

        pub fn record_join_succeeded(&self, ssid: String) {
            lock(&self.shared).snapshot.join = JoinAttemptState::Succeeded { ssid };
        }

        pub fn record_join_failed(&self, ssid: String, error: String) {
            lock(&self.shared).snapshot.join = JoinAttemptState::Failed { ssid, error };
        }

        /// `via_hotspot` selects which inactivity budget applies: the
        /// shorter [`NETWORK_PROVISION_INACTIVITY_SECONDS`] while reachable
        /// only via the open bootstrap hotspot, the longer
        /// [`WIFI_TRANSFER_INACTIVITY_SECONDS`] once on a private LAN.
        #[must_use]
        pub fn is_expired(&self, via_hotspot: bool) -> bool {
            let threshold = if via_hotspot {
                NETWORK_PROVISION_INACTIVITY_SECONDS
            } else {
                WIFI_TRANSFER_INACTIVITY_SECONDS
            };
            lock(&self.shared).last_activity.elapsed() >= Duration::from_secs(threshold)
        }
    }

    impl Drop for WifiTransferServer {
        fn drop(&mut self) {
            let status = unsafe { sys::esp_wifi_set_ps(sys::wifi_ps_type_t_WIFI_PS_MAX_MODEM) };
            if status != sys::ESP_OK {
                warn!(
                    "rustmix-wave=wifi-power-save status=failed mode=max-modem error-code={status}"
                );
            }
            info!("rustmix-wave=wifi-transfer-server status=stopped");
        }
    }

    fn lock(shared: &Arc<Mutex<SharedStatus>>) -> std::sync::MutexGuard<'_, SharedStatus> {
        shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Refuse a change another web page made the browser send (see
    /// [`is_same_origin_request`]).
    fn ensure_same_origin(origin: Option<&str>, host: Option<&str>) -> Result<()> {
        if is_same_origin_request(origin, host) {
            Ok(())
        } else {
            bail!("request from another site refused")
        }
    }

    fn required_query(uri: &str, key: &str) -> Result<String> {
        query_value(uri, key).ok_or_else(|| anyhow!("missing query parameter: {key}"))
    }

    fn sibling_path(path: &Path, extension: &str) -> Result<std::path::PathBuf> {
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("filename required"))?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("parent folder required"))?;
        Ok(parent.join(format!("{stem}.{extension}")))
    }

    fn temporary_path(path: &Path) -> Result<std::path::PathBuf> {
        sibling_path(path, "TMP")
    }

    fn commit_atomic_upload(temporary: &Path, destination: &Path) -> Result<()> {
        let backup = sibling_path(destination, "BAK")?;
        if backup.exists() {
            fs::remove_file(&backup)?;
        }
        let had_destination = destination.exists();
        if had_destination {
            fs::rename(destination, &backup)?;
        }
        if let Err(error) = fs::rename(temporary, destination) {
            if had_destination {
                let _ = fs::rename(&backup, destination);
            }
            let _ = fs::remove_file(temporary);
            return Err(error.into());
        }
        if had_destination {
            fs::remove_file(&backup)?;
        }
        Ok(())
    }

    fn list_directory_json(relative: &str) -> Result<String> {
        let path = resolve_portal_path(relative).map_err(|error| anyhow!(error))?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)?.take(WIFI_TRANSFER_MAX_DIRECTORY_ROWS) {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let child_relative = format!("{}/{}", relative.trim_end_matches('/'), name);
            if is_protected_portal_path(&child_relative) || !super::is_sd_safe_name(&name) {
                continue;
            }
            let metadata = entry.metadata()?;
            let modified_seconds = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs());
            entries.push((name, metadata.is_dir(), metadata.len(), modified_seconds));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut json = String::from("[");
        for (index, (name, directory, size, modified_seconds)) in entries.into_iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            let kind = if directory { "folder" } else { "file" };
            json.push_str(&format!(
                r#"{{"name":"{}","kind":"{kind}","size":{size},"modified":{modified_seconds}}}"#,
                json_escape(&name)
            ));
        }
        json.push(']');
        Ok(json)
    }

    fn json_escape(value: &str) -> String {
        value.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn scan_json(entries: &[WifiScanEntry]) -> String {
        let mut json = String::from("[");
        for (index, entry) in entries.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                r#"{{"ssid":"{}","rssi":{}}}"#,
                json_escape(&entry.ssid),
                entry.rssi_dbm
            ));
        }
        json.push(']');
        json
    }

    fn saved_json(ssids: &[String]) -> String {
        let mut json = String::from("[");
        for (index, ssid) in ssids.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str(&format!(r#"{{"ssid":"{}"}}"#, json_escape(ssid)));
        }
        json.push(']');
        json
    }

    fn wifi_join_json(join: &JoinAttemptState) -> String {
        match join {
            JoinAttemptState::Idle => r#"{"state":"idle"}"#.into(),
            JoinAttemptState::Testing { .. } => r#"{"state":"testing"}"#.into(),
            JoinAttemptState::Succeeded { .. } => r#"{"state":"connected"}"#.into(),
            JoinAttemptState::Failed { error, .. } => {
                format!(r#"{{"state":"failed","error":"{}"}}"#, json_escape(error))
            }
        }
    }

    /// Builds a [`ReaderBook`] straight from filesystem metadata for a path
    /// already resolved under the portal root. Title is left blank: the
    /// cover cache's fingerprint (path + size + mtime + format) never hashes
    /// it, so the portal has no reason to pay for EPUB title parsing here.
    fn book_from_path(path: &Path) -> Result<ReaderBook> {
        let format = book_format_from_path(path).ok_or_else(|| anyhow!("unsupported file type"))?;
        let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        let modified_seconds = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_secs());
        Ok(ReaderBook {
            path: path.to_string_lossy().into_owned(),
            title: String::new(),
            format,
            size_bytes: metadata.len(),
            modified_seconds,
        })
    }

    /// The on-device Library screen's own cover cache directory
    /// (`<READER_STATE_DIRECTORY>/CACHE`, shared with the `.EPX`/`.EPP`/
    /// `.CCH` Reader sidecars — see [`crate::cover_cache`]'s module docs).
    /// The portal must read from this exact directory, not
    /// [`CoverCache::default`]'s standalone fallback root, or a thumbnail it
    /// serves (or a browser-pregenerated one it stores) would sit somewhere
    /// the on-device Library screen never looks at.
    fn reader_cover_cache_directory() -> std::path::PathBuf {
        Path::new(crate::reader::READER_STATE_DIRECTORY).join(crate::reader::READER_CACHE_DIRECTORY)
    }

    /// JSON array of the Reader library, reusing the same scan the on-device
    /// Library screen runs so the portal's book list never drifts from what
    /// the device itself will show.
    fn books_json(previous: &[ReaderBook]) -> Result<(String, Vec<ReaderBook>)> {
        let books =
            scan_txt_library(READER_BOOKS_DIRECTORY, previous).map_err(|error| anyhow!(error))?;
        let mut json = String::from("[");
        for (index, book) in books.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            let relative = book
                .path
                .strip_prefix(WIFI_TRANSFER_ROOT)
                .unwrap_or(&book.path);
            json.push_str(&format!(
                r#"{{"path":"{}","title":"{}","format":"{}","size":{},"modified":{}}}"#,
                json_escape(relative),
                json_escape(&book.title),
                book.format.badge(),
                book.size_bytes,
                book.modified_seconds,
            ));
        }
        json.push(']');
        Ok((json, books))
    }

    /// Encodes a cached 1bpp thumbnail (bit 1 = ink/black) as an 8-bit
    /// grayscale PNG so any browser can render it directly in an `<img>`
    /// without a bespoke decoder.
    fn thumbnail_to_png(thumbnail: &CachedThumbnail) -> Result<Vec<u8>> {
        let width = u32::from(thumbnail.width);
        let height = u32::from(thumbnail.height);
        let row_bytes = (thumbnail.width as usize).div_ceil(8);
        let mut pixels = vec![0u8; (width * height) as usize];
        for y in 0..thumbnail.height as usize {
            for x in 0..thumbnail.width as usize {
                let ink = thumbnail.bits[y * row_bytes + x / 8] & (0x80 >> (x % 8)) != 0;
                pixels[y * thumbnail.width as usize + x] = if ink { 0 } else { 255 };
            }
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|error| anyhow!("PNG header failed: {error}"))?;
            writer
                .write_image_data(&pixels)
                .map_err(|error| anyhow!("PNG data failed: {error}"))?;
        }
        Ok(bytes)
    }

    /// Total and free byte counts for the mounted SD card, or `None` when the
    /// underlying FAT volume cannot be queried.
    fn sd_space_bytes() -> Option<(u64, u64)> {
        let path = CString::new(SD_MOUNT_POINT).ok()?;
        let mut total_bytes = 0_u64;
        let mut free_bytes = 0_u64;
        if unsafe { sys::esp_vfs_fat_info(path.as_ptr(), &mut total_bytes, &mut free_bytes) }
            != sys::ESP_OK
        {
            return None;
        }
        Some((total_bytes, free_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_protected_portal_path, is_sd_safe_name, is_sleep_image_name, query_value,
        resolve_portal_path, sleep_status_json, WifiTransferSnapshot, WifiTransferState,
    };

    #[test]
    fn portal_is_off_until_the_user_explicitly_starts_it() {
        let snapshot = WifiTransferSnapshot::default();
        assert_eq!(snapshot.state, WifiTransferState::Off);
        assert!(!snapshot.is_active());
    }

    #[test]
    fn portal_paths_are_confined_and_sd_safe() {
        assert!(resolve_portal_path("/BOOKS/POIROT01.EPU").is_ok());
        assert!(resolve_portal_path("/BOOKS/L'amica geniale - Elena Ferrante.epub").is_ok());
        assert!(resolve_portal_path("../WIFI.TXT").is_err());
        assert!(resolve_portal_path("/BOOKS/what?.txt").is_err());
        assert!(is_sd_safe_name("MAIN.LUA"));
        assert!(is_sd_safe_name("Perché «così» (2021).epub"));
        assert!(!is_sd_safe_name("a:b.txt"));
        assert!(!is_sd_safe_name("trailing dot."));
        assert!(!is_sd_safe_name("trailing space "));
        assert!(!is_sd_safe_name(""));
        assert!(!is_sd_safe_name(&"x".repeat(256)));
    }

    #[test]
    fn configuration_files_are_protected() {
        assert!(is_protected_portal_path("/WIFI.TXT"));
        assert!(is_protected_portal_path("CLOCK.TXT"));
        assert!(!is_protected_portal_path("/BOOKS/NOTES001.TXT"));
    }

    /// Every spelling that resolves to a protected file is refused by the
    /// resolver itself, which every file endpoint goes through.
    #[test]
    fn no_spelling_of_a_protected_file_resolves() {
        let from_query = |raw: &str| query_value(&format!("/api/download?path={raw}"), "path");
        for raw in [
            "WIFI.TXT",
            "/WIFI.TXT",
            "./WIFI.TXT",
            "/./WIFI.TXT",
            "WIFI.TXT/",
            "//WIFI.TXT",
            "wifi.txt",
            "WiFi.Txt",
            "%2E%2FWIFI.TXT",
            "%57IFI.TXT",
            "WIFI.TXT.",
            "WIFI.TXT%20",
            "WIFI.TXT+",
            "BOOKS/../WIFI.TXT",
            "display.txt",
        ] {
            let decoded = from_query(raw).unwrap();
            assert!(resolve_portal_path(&decoded).is_err(), "{raw} -> {decoded}");
        }
        // Encoded twice, the name is decoded once: a literal "%57IFI.TXT",
        // which is some other file, never WIFI.TXT.
        let literal = resolve_portal_path(&from_query("%2557IFI.TXT").unwrap()).unwrap();
        assert_eq!(literal.file_name().unwrap(), "%57IFI.TXT");
        // Ordinary files next to them still resolve.
        assert!(resolve_portal_path("./BOOKS/WIFI.TXT").is_ok());
        assert!(resolve_portal_path("MENU.TXT").is_ok());
    }

    #[test]
    fn changes_are_refused_only_when_another_site_sent_them() {
        use super::is_same_origin_request as same;
        // The portal's own page, on the LAN address or the hotspot's.
        assert!(same(Some("http://192.168.1.10"), Some("192.168.1.10")));
        assert!(same(Some("http://4.3.2.1"), Some("4.3.2.1")));
        assert!(same(Some("http://Rustmix.local"), Some("rustmix.local")));
        // Not a page's script: a tool, or a browser that sends no Origin.
        assert!(same(None, Some("192.168.1.10")));
        assert!(same(Some(""), Some("192.168.1.10")));
        // A page of another site making the browser post here.
        assert!(!same(Some("https://example.com"), Some("192.168.1.10")));
        assert!(!same(Some("http://example.com"), Some("192.168.1.10")));
        assert!(!same(
            Some("http://192.168.1.10:8080"),
            Some("192.168.1.10")
        ));
        assert!(!same(Some("null"), Some("192.168.1.10")));
        assert!(!same(Some("http://192.168.1.10"), None));
    }

    #[test]
    fn a_download_is_declared_as_a_file_to_save_under_its_own_name() {
        use super::{download_content_disposition, download_content_type};

        assert_eq!(download_content_type("Libro.EPUB"), "application/epub+zip");
        assert_eq!(download_content_type("SLEEP001.BMP"), "image/bmp");
        assert_eq!(
            download_content_type("LEGGIMI.TXT"),
            "text/plain; charset=utf-8"
        );
        assert_eq!(download_content_type("traccia 01.mp3"), "audio/mpeg");
        assert_eq!(
            download_content_type("senza estensione"),
            "application/octet-stream"
        );

        assert_eq!(
            download_content_disposition("Mondo Emerso.epub"),
            "attachment; filename=\"Mondo Emerso.epub\"; filename*=UTF-8''Mondo%20Emerso.epub"
        );
        // Accents and quotes cannot ride in the plain name: it gets a
        // stand-in, and the real name goes percent-encoded beside it.
        assert_eq!(
            download_content_disposition("L'età \"d'oro\"; 100%.txt"),
            "attachment; filename=\"L'et_ _d'oro__ 100_.txt\"; \
             filename*=UTF-8''L%27et%C3%A0%20%22d%27oro%22%3B%20100%25.txt"
        );
    }

    fn portal_page() -> String {
        // A Windows checkout has CRLF line endings, which `include_str!`
        // keeps (string literals themselves always get LF).
        let source = include_str!("wifi_transfer.rs").replace("\r\n", "\n");
        let page_start = source.find("const PORTAL_HTML").unwrap();
        let page_end = page_start + source[page_start..].find("\"##;").unwrap();
        source[page_start..page_end].to_string()
    }

    #[test]
    fn the_wallpaper_editor_is_upright_and_has_no_tone_sliders() {
        let page = portal_page();
        // Framed as the device is held, 3 wide by 5 tall.
        assert!(page.contains(
            ".crop-frame{position:relative;overflow:hidden;width:100%;aspect-ratio:3/5;"
        ));
        assert!(page.contains("function portraitToNative("));
        // Position and zoom only.
        assert!(!page.contains("brightRange"));
        assert!(!page.contains("contrastRange"));
        assert!(!page.contains("rotateImage"));
        assert!(!page.contains("id=\"bgName\""));
        // No picture search inside the page: a picture is chosen, pasted or
        // dragged in, and Google is a plain link to another tab.
        assert!(!page.contains("commons.wikimedia.org"));
        assert!(!page.contains("searchImages"));
        assert!(!page.contains("id=\"imgQuery\""));
        assert!(page.contains(
            "<a href=\"https://www.google.com/imghp\" target=\"_blank\" rel=\"noopener\""
        ));
    }

    #[test]
    fn a_fixed_wallpaper_is_named_by_a_bmp_file_name_and_told_as_json() {
        assert!(is_sleep_image_name("SLEEP003.BMP"));
        assert!(is_sleep_image_name("mare.bmp"));
        for bad in [
            "",
            ".bmp",
            "SLEEP003.TXT",
            "../WIFI.TXT",
            "a/b.bmp",
            "SLEEP003.BMP.",
        ] {
            assert!(!is_sleep_image_name(bad), "{bad:?} accepted");
        }
        assert_eq!(
            sleep_status_json("fixed", Some("SLEEP003.BMP")),
            r#"{"mode":"fixed","fixed":"SLEEP003.BMP"}"#
        );
        assert_eq!(
            sleep_status_json("sequential", None),
            r#"{"mode":"sequential","fixed":""}"#
        );
        // Whatever the cursor file holds cannot break the answer.
        assert_eq!(
            sleep_status_json("fixed", Some("a\"b\\c\nd")),
            r#"{"mode":"fixed","fixed":"a\"b\\cd"}"#
        );
    }

    #[test]
    fn the_page_sets_the_whole_sleep_screen() {
        use crate::app::display::SleepScreenMode;

        let page = portal_page();
        assert!(page.contains("if(s.sleep)sleepScreen=s.sleep;"));
        // Every sleep screen of the device can be chosen here, by the same
        // names the device saves.
        for mode in [
            SleepScreenMode::Sequential,
            SleepScreenMode::Random,
            SleepScreenMode::Fixed,
            SleepScreenMode::BookCover,
        ] {
            assert!(
                page.contains(&format!("  ['{}','", mode.marker())),
                "{} cannot be chosen on the page",
                mode.marker()
            );
            assert_eq!(SleepScreenMode::from_marker(mode.marker()), Some(mode));
        }
        assert!(page.contains("'/api/sleep?mode='+enc(mode)+(name?'&name='+enc(name):'')"));
        assert!(page.contains("onclick=\"setSleepMode(\\'fixed\\',this.dataset.n)\""));
        assert!(page.contains("<div id=\"sleepModes\"></div>"));
        // The handler exists under the same name the page calls.
        let source = include_str!("wifi_transfer.rs");
        assert!(source.contains("server.fn_handler(\"/api/sleep\", Method::Post"));
        assert!(source.contains("\\\"sleep\\\":{sleep}"));
    }

    #[test]
    fn the_page_saves_downloads_and_deletes_folders_only_on_request() {
        let page = portal_page();
        // One place makes every download, and names the file to save.
        assert!(page.contains("a.download=baseName(p)"));
        assert!(!page.contains("href=\"/api/download"));
        // A folder goes with its contents only through the user's delete.
        assert_eq!(page.matches("&recursive=1").count(), 1);
        // Both languages are in the page; the device says which to use.
        assert!(page.contains("function L(it,en){return LANG==='en'?en:it}"));
        assert!(page.contains("s.lang==='en'"));
    }

    #[test]
    fn the_page_language_follows_the_device_and_the_top_folder_is_never_deleted() {
        use super::{is_portal_root, portal_locale_code, resolve_portal_path, set_portal_locale};
        use crate::regional::Locale;

        set_portal_locale(Locale::English);
        assert_eq!(portal_locale_code(), "en");
        set_portal_locale(Locale::Italian);
        assert_eq!(portal_locale_code(), "it");

        for spelling in ["/", "", "//", "/."] {
            let path = resolve_portal_path(spelling).unwrap();
            assert!(is_portal_root(&path), "{spelling:?}");
        }
        assert!(!is_portal_root(&resolve_portal_path("/BOOKS").unwrap()));
        assert!(!is_portal_root(
            &resolve_portal_path("/AUDIO/Titolo").unwrap()
        ));
    }

    #[test]
    fn the_portal_page_asks_for_no_code() {
        // A Windows checkout has CRLF line endings, which `include_str!`
        // keeps (string literals themselves always get LF).
        let source = include_str!("wifi_transfer.rs").replace("\r\n", "\n");
        let page_start = source.find("const PORTAL_HTML").unwrap();
        let page_end = page_start + source[page_start..].find("\"##;").unwrap();
        let page = &source[page_start..page_end];
        assert!(!page.contains("code="));
        assert!(!page.contains("lockCode"));
        // The app is shown at once and loads its data without unlocking.
        assert!(page.contains("<div class=\"wrap\" id=\"app\">"));
        assert!(page.contains("\ninitApp();\n</script>"));
    }

    #[test]
    fn query_parser_decodes_portal_paths() {
        assert_eq!(
            query_value("/api/list?sort=name&path=%2FBOOKS", "path").as_deref(),
            Some("/BOOKS")
        );
    }
}
