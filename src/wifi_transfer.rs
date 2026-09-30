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
//! included -- is reachable there instead. The target-specific HTTP server
//! owns its own ESP-IDF task while the main loop retains display, routing
//! and sleep ownership.

use std::path::{Component, Path, PathBuf};

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
/// One authenticated browser session code is shown on the e-paper screen.
pub const WIFI_TRANSFER_CODE_DIGITS: usize = 6;
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
    pub code: Option<String>,
    /// `Some` only while reachable via the bootstrap hotspot instead of an
    /// already-joined LAN (see the module docs); drives which card the
    /// on-device screen and the portal's Wi-Fi tab show.
    pub ap_ssid: Option<String>,
    pub ap_password: Option<String>,
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
            code: None,
            ap_ssid: None,
            ap_password: None,
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

    #[must_use]
    pub fn code_label(&self) -> &str {
        self.code.as_deref().unwrap_or("------")
    }
}

/// Reject empty, traversal, absolute and overlong relative portal paths.
/// Returned paths always stay beneath `/sdcard/RUSTMIX`.
pub fn resolve_portal_path(relative: &str) -> Result<PathBuf, &'static str> {
    let decoded = percent_decode(relative)?;
    if decoded.len() > WIFI_TRANSFER_MAX_PATH_BYTES {
        return Err("path exceeds portal limit");
    }
    let trimmed = decoded.trim_start_matches('/');
    let relative_path = Path::new(trimmed);
    let mut safe = PathBuf::from(WIFI_TRANSFER_ROOT);
    for component in relative_path.components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str().ok_or("path is not UTF-8")?;
                if !is_fat83_component(name) {
                    return Err("use FAT 8.3-safe names");
                }
                safe.push(name);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path traversal is blocked")
            }
        }
    }
    Ok(safe)
}

/// Configuration files stay hidden and cannot be modified by the initial LAN
/// portal.  This prevents accidental credential disclosure or live config
/// replacement while services are running.
#[must_use]
pub fn is_protected_portal_path(relative: &str) -> bool {
    let upper = relative.trim_start_matches('/').to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "WIFI.TXT"
            | "CLOCK.TXT"
            | "ALARMS.TXT"
            | "DISPLAY.TXT"
            | "VOICE/META.TXT"
            | "VOICE/SETTINGS.TXT"
    )
}

/// Folder names are at most eight uppercase-safe characters.  Files are 8.3.
#[must_use]
pub fn is_fat83_component(component: &str) -> bool {
    if component.is_empty() || component == "." || component == ".." {
        return false;
    }
    let mut parts = component.split('.');
    let stem = parts.next().unwrap_or_default();
    let extension = parts.next();
    if parts.next().is_some() || stem.is_empty() || stem.len() > 8 {
        return false;
    }
    if extension.is_some_and(|value| value.is_empty() || value.len() > 3) {
        return false;
    }
    component
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'~'))
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

    use crate::cover_cache::{CachedThumbnail, CoverCache};
    use crate::dns_captive_portal::espidf::CaptivePortalDns;
    use crate::network_scan::WifiScanEntry;
    use crate::reader::{
        book_format_from_path, scan_txt_library, ReaderBook, READER_BOOKS_DIRECTORY,
    };
    use crate::storage::SD_MOUNT_POINT;

    use super::{
        is_protected_portal_path, query_value, resolve_portal_path, JoinAttemptState,
        PendingJoinRequest, WifiTransferSnapshot, WifiTransferState,
        NETWORK_PROVISION_INACTIVITY_SECONDS, WIFI_TRANSFER_HTTP_PORT,
        WIFI_TRANSFER_INACTIVITY_SECONDS, WIFI_TRANSFER_MAX_DIRECTORY_ROWS,
        WIFI_TRANSFER_MAX_UPLOAD_BYTES, WIFI_TRANSFER_ROOT, WIFI_TRANSFER_SERVER_STACK_BYTES,
        WIFI_TRANSFER_STREAM_CHUNK_BYTES,
    };

    const PORTAL_HTML: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Rustmix-Wave</title><style>
*{box-sizing:border-box}
:root{--bg:#f5f6f8;--card:#fff;--text:#1a1d23;--muted:#6b7280;--border:#e2e5ea;--accent:#2563eb;--accent-dark:#1d4ed8;--danger:#dc2626;--radius:10px}
@media (prefers-color-scheme:dark){:root{--bg:#14161a;--card:#1c1f26;--text:#e8eaed;--muted:#9aa3b2;--border:#2a2e37;--accent:#3b82f6;--accent-dark:#60a5fa;--danger:#f87171}}
body{margin:0;background:var(--bg);color:var(--text);font-family:-apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif}
[hidden]{display:none!important}
.wrap{max-width:960px;margin:0 auto;padding:1rem}
header.top{display:flex;flex-wrap:wrap;gap:.6rem;align-items:center;justify-content:space-between;margin-bottom:1rem}
h1{font-size:1.2rem;margin:0}
.badge{background:var(--card);border:1px solid var(--border);border-radius:999px;padding:.3rem .8rem;font-size:.8rem;color:var(--muted)}
.card{background:var(--card);border:1px solid var(--border);border-radius:var(--radius);padding:1rem;margin-bottom:1rem}
input,button{font:inherit}
input[type=text],input[type=search],input[type=password]{background:transparent;border:1px solid var(--border);border-radius:8px;padding:.5rem .7rem;color:var(--text)}
ul{list-style:none;margin:0;padding:0}
li{display:flex;align-items:center;gap:.6rem;padding:.5rem 0;border-bottom:1px solid var(--border)}
li:last-child{border-bottom:none}
li .name{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rssi{color:var(--muted);font-size:.85rem}
#wifiJoinCard input{width:100%;margin:.3rem 0}
button{border:1px solid var(--border);background:var(--card);color:var(--text);border-radius:8px;padding:.5rem .9rem;cursor:pointer}
button:hover{border-color:var(--accent)}
button.primary{background:var(--accent);border-color:var(--accent);color:#fff}
button.primary:hover{background:var(--accent-dark)}
button.danger{color:var(--danger);border-color:var(--danger)}
button.danger:hover{background:var(--danger);color:#fff}
.crumbs{display:flex;flex-wrap:wrap;gap:.25rem;font-size:.9rem;margin-bottom:.6rem}
.crumbs a{color:var(--accent);cursor:pointer;text-decoration:none}
.crumbs span{color:var(--muted)}
.drop{border:2px dashed var(--border);border-radius:var(--radius);padding:1.5rem;text-align:center;color:var(--muted);cursor:pointer;transition:border-color .15s}
.drop.drag{border-color:var(--accent);color:var(--accent)}
.queue{margin-top:.8rem;display:flex;flex-direction:column;gap:.5rem}
.qitem{display:flex;align-items:center;gap:.6rem;font-size:.85rem}
.qitem .name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.qitem input[type=text]{width:8rem}
.bar{flex:1;height:6px;background:var(--border);border-radius:3px;overflow:hidden}
.bar>i{display:block;height:100%;background:var(--accent);width:0%}
.qitem.done .bar>i{background:#16a34a}
.qitem.error .bar>i{background:var(--danger)}
table{width:100%;border-collapse:collapse;font-size:.9rem}
th,td{padding:.5rem .4rem;border-bottom:1px solid var(--border);text-align:left}
th{color:var(--muted);font-weight:600;font-size:.75rem;text-transform:uppercase;letter-spacing:.02em}
.actions{display:flex;gap:.4rem;flex-wrap:wrap}
.actions button{padding:.3rem .6rem;font-size:.8rem}
.toolbar{display:flex;flex-wrap:wrap;gap:.5rem;align-items:center;margin-bottom:.8rem}
.toolbar input[type=search]{flex:1;min-width:10rem}
.bulk{display:none;align-items:center;gap:.6rem;background:var(--card);border:1px solid var(--accent);border-radius:8px;padding:.5rem .8rem;margin-bottom:.8rem;font-size:.85rem}
.bulk.show{display:flex}
#status{font-size:.85rem;color:var(--muted);white-space:pre-wrap;margin:0 0 .8rem}
.hint{font-size:.8rem;color:var(--muted)}
.kind-folder{color:var(--accent)}
.crop-frame{position:relative;overflow:hidden;width:100%;max-width:480px;aspect-ratio:5/3;background:#000;border-radius:8px;margin:.6rem auto;touch-action:none;cursor:grab}
.crop-frame:active{cursor:grabbing}
.crop-frame img{position:absolute;top:0;left:0;transform-origin:top left;user-select:none;-webkit-user-drag:none}
.stage{display:none}
.stage.show{display:block}
.slider-row{display:flex;align-items:center;gap:.6rem;margin:.5rem 0;font-size:.85rem}
.slider-row input[type=range]{flex:1}
.bg-preview{width:100%;max-width:480px;height:auto;display:block;margin:.6rem auto;border-radius:8px;image-rendering:pixelated;border:1px solid var(--border)}
.search-row{display:flex;gap:.5rem;flex-wrap:wrap;margin-bottom:.4rem}
.search-row input[type=text]{flex:1;min-width:10rem}
.lock{position:fixed;inset:0;background:var(--bg);display:flex;align-items:center;justify-content:center;padding:1rem;z-index:50}
.lock-card{background:var(--card);border:1px solid var(--border);border-radius:var(--radius);padding:1.6rem;max-width:320px;width:100%;text-align:center}
.lock-card h1{margin:0 0 .4rem;font-size:1.25rem}
.lock-card input{width:100%;text-align:center;letter-spacing:.4em;font-size:1.4rem;margin:1rem 0 .6rem;padding:.6rem}
.lock-card button{width:100%;padding:.6rem;margin-top:.3rem}
.lock-error{color:var(--danger);font-size:.85rem;min-height:1.2em;margin:.6rem 0 0}
.tabs{display:flex;gap:.4rem;flex-wrap:wrap;margin-bottom:.8rem}
.tab{border:1px solid var(--border);background:var(--card);color:var(--muted);border-radius:999px;padding:.55rem 1.1rem;cursor:pointer;font-size:.9rem}
.tab.active{background:var(--accent);border-color:var(--accent);color:#fff;font-weight:600}
.book-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(140px,1fr));gap:1rem;margin-top:.6rem}
.book-card{border:1px solid var(--border);border-radius:var(--radius);padding:.6rem;display:flex;flex-direction:column;gap:.4rem}
.book-cover{position:relative;aspect-ratio:208/252;background:var(--bg);border-radius:6px;overflow:hidden;display:flex;align-items:center;justify-content:center;border:1px solid var(--border)}
.book-cover img{width:100%;height:100%;object-fit:cover;image-rendering:pixelated}
.book-cover.empty img{display:none}
.book-cover.empty::after{content:'Nessuna copertina';color:var(--muted);font-size:.7rem;padding:.5rem;text-align:center}
.book-title{font-size:.85rem;font-weight:600;overflow:hidden;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical}
.book-card .actions{margin-top:auto;display:flex;gap:.4rem}
.book-card .actions a,.book-card .actions button{padding:.3rem .5rem;font-size:.75rem;flex:1;text-align:center;text-decoration:none}
</style></head><body>
<div class="lock" id="lock">
<div class="lock-card">
<h1>Rustmix-Wave</h1>
<p class="hint">Inserisci il codice a sei cifre mostrato sul dispositivo per accedere.</p>
<input id="lockCode" type="text" inputmode="numeric" maxlength="6" placeholder="000000" autocomplete="off" onkeydown="if(event.key==='Enter')unlock()">
<button class="primary" onclick="unlock()">Sblocca</button>
<p class="lock-error" id="lockError"></p>
</div>
</div>
<div class="wrap" id="app" hidden>
<header class="top">
<div>
<h1>Rustmix-Wave</h1>
<p class="hint" style="margin:.15rem 0 0">Gestisci libri, sfondi e token dal browser</p>
</div>
<div style="display:flex;gap:.5rem;align-items:center;flex-wrap:wrap">
<span class="badge" id="space">Spazio: --</span>
<button onclick="refreshActiveTab()">Aggiorna</button>
<button onclick="lockOut()">Blocca</button>
</div>
</header>
<nav class="tabs">
<button class="tab" data-tab="books" onclick="showTab('books')">Libri</button>
<button class="tab" data-tab="wallpaper" onclick="showTab('wallpaper')">Sfondi</button>
<button class="tab" data-tab="files" onclick="showTab('files')">File</button>
<button class="tab" data-tab="wifi" onclick="showTab('wifi')">Wi-Fi</button>
</nav>
<pre id="status"></pre>

<section id="tab-books" class="tabpanel" hidden>
<div class="card">
<div class="drop" id="dropBooks" onclick="document.getElementById('fileBooks').click()">Trascina qui i tuoi eBook (EPUB o TXT) oppure tocca per selezionarli<div class="hint">I nomi vengono adattati al formato FAT 8.3 (es. LIBRO0001.EPU)</div></div>
<input id="fileBooks" type="file" multiple accept=".epub,.txt" style="display:none">
<div class="queue" id="queueBooks"></div>
</div>
<div class="card">
<div class="toolbar"><input id="bookSearch" type="search" placeholder="Cerca nei tuoi libri..." oninput="renderBooks()"><span class="hint" id="bookCount"></span></div>
<div class="book-grid" id="bookGrid"><p class="hint">Caricamento...</p></div>
</div>
</section>

<section id="tab-wallpaper" class="tabpanel" hidden>
<div class="card" id="wallpaper">
<h2 style="margin:0 0 .5rem;font-size:1.05rem">Sfondo schermata di sospensione</h2>
<p class="hint">Crea un&apos;immagine 800&times;480 in bianco e nero (dithering) per la cartella SLEEP. Cerca un&apos;immagine, salvala sul PC, poi trascinala qui per ritagliarla.</p>
<div class="search-row">
<input id="gquery" type="text" placeholder="Cerca immagini su Google...">
<button onclick="searchGoogleImages()">Cerca su Google Immagini</button>
</div>
<div class="drop" id="dropBg" onclick="document.getElementById('fileBg').click()">Trascina un&apos;immagine JPEG/PNG qui oppure tocca per selezionarla</div>
<input id="fileBg" type="file" accept="image/*" style="display:none">
<div class="stage" id="cropStage">
<div class="crop-frame" id="cropFrame"><img id="cropImg" alt=""></div>
<div class="slider-row"><span>Zoom</span><input id="zoomRange" type="range" min="1" max="3" step="0.01" value="1"><button onclick="rotateImage(-90)" title="Ruota antiorario">&#8634;</button><button onclick="rotateImage(90)" title="Ruota orario">&#8635;</button></div>
<p><button class="primary" onclick="confirmCrop()">Continua</button> <button onclick="cancelCrop()">Annulla</button></p>
</div>
<div class="stage" id="adjustStage">
<canvas class="bg-preview" id="bgPreview" width="800" height="480"></canvas>
<div class="slider-row"><span>Luminosit&agrave;</span><input id="brightRange" type="range" min="-100" max="100" step="1" value="0"></div>
<div class="slider-row"><span>Contrasto</span><input id="contrastRange" type="range" min="-100" max="100" step="1" value="0"></div>
<label>Nome file <input id="bgName" type="text" maxlength="12" value="SLEEP001.BMP"></label>
<p><button class="primary" onclick="uploadBackground()">Carica come sfondo</button> <button onclick="backToCrop()">Indietro</button></p>
</div>
<pre id="bgStatus" class="hint"></pre>
</div>
<div class="card">
<h2 style="margin:0 0 .5rem;font-size:1.05rem">Sfondi sul dispositivo</h2>
<div class="book-grid" id="sleepGallery"><p class="hint">Caricamento...</p></div>
</div>
</section>

<section id="tab-files" class="tabpanel" hidden>
<div class="card">
<div class="crumbs" id="crumbs"></div>
<div class="toolbar">
<input id="search" type="search" placeholder="Cerca nella cartella..." oninput="renderTable()">
<button onclick="newFolder()">+ Cartella</button>
</div>
<div class="bulk" id="bulk"><span id="bulkCount">0 selezionati</span><button class="danger" onclick="bulkDelete()">Elimina selezionati</button><button onclick="clearSelection()">Annulla</button></div>
<table><thead><tr><th style="width:2rem"><input type="checkbox" id="selectAll" onchange="toggleSelectAll(this.checked)"></th><th>Nome</th><th>Tipo</th><th>Dimensione</th><th>Azioni</th></tr></thead><tbody id="rows"></tbody></table>
</div>
<div class="card">
<div class="drop" id="drop" onclick="document.getElementById('file').click()">Trascina i file qui oppure tocca per selezionarli<div class="hint">I nomi vengono adattati al formato FAT 8.3 (es. LIBRO0001.TXT)</div></div>
<input id="file" type="file" multiple style="display:none">
<div class="queue" id="queue"></div>
</div>
</section>

<section id="tab-wifi" class="tabpanel" hidden>
<div class="card" id="wifiScanCard">
<h2 style="margin:0 0 .5rem;font-size:1.05rem">Reti vicine</h2>
<ul id="wifiScan"><li class="hint">Scansione...</li></ul>
</div>
<div class="card">
<h2 style="margin:0 0 .5rem;font-size:1.05rem">Reti salvate</h2>
<ul id="wifiSaved"><li class="hint">Caricamento...</li></ul>
<p class="hint" id="wifiHint" hidden>Sei collegato via LAN: aggiungere o cambiare una rete disconnette temporaneamente il dispositivo per provarla. Se la nuova rete funziona, il dispositivo passa a quella (raggiungibile a un indirizzo diverso); se fallisce, torna automaticamente alla rete attuale.</p>
</div>
<div class="card" id="wifiJoinCard" style="display:none">
<h2 id="wifiJoinTitle" style="margin:0 0 .5rem;font-size:1.05rem">Aggiungi rete</h2>
<input id="wifiJoinSsid" type="text" placeholder="Nome rete">
<input id="wifiJoinPassword" type="password" placeholder="Password (vuota se aperta)">
<p class="hint" id="wifiJoinWarning" hidden>Il dispositivo si disconnettera&apos; da questa rete per provare quella nuova: se riesce dovrai raggiungerlo a un indirizzo diverso, se fallisce torna qui da solo.</p>
<p><button class="primary" onclick="wifiSubmitJoin()">Connetti e salva</button> <button onclick="wifiCloseJoin()">Annulla</button></p>
<pre id="wifiJoinStatus" class="hint"></pre>
</div>
<div class="card" id="wifiForgetCard" style="display:none">
<h2 style="margin:0 0 .5rem;font-size:1.05rem">Dimenticare la rete?</h2>
<p id="wifiForgetText" class="hint"></p>
<p><button class="danger" onclick="wifiConfirmForget()">S&igrave;, dimentica</button> <button onclick="wifiCancelForget()">Annulla</button></p>
</div>
</section>
</div>
<script>
let current='/',entries=[],selected=new Set(),activeTab='books',books=[];
function status(t){document.getElementById('status').textContent=t}
function enc(s){return encodeURIComponent(s)}
function join(n){return (current==='/'?'/':current+'/')+n}
function getCode(){return localStorage.rustmixCode||''}
function escapeHtml(s){return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')}
function formatBytes(n){if(n===undefined||n===null)return '--';const u=['B','KB','MB','GB'];let i=0,v=n;while(v>=1024&&i<u.length-1){v/=1024;i++}return (i===0?v:v.toFixed(1))+' '+u[i]}
async function api(url,opt){let sep=url.includes('?')?'&':'?';let r=await fetch(url+sep+'code='+enc(getCode()),opt);let t=await r.text();if(!r.ok)throw new Error(t||('HTTP '+r.status));return t}
async function fetchSpace(){try{let s=JSON.parse(await api('/api/status'));document.getElementById('space').textContent='Spazio libero: '+formatBytes(s.free_bytes)+' / '+formatBytes(s.total_bytes)}catch(e){}}
async function ensureDir(path){try{await api('/api/list?path='+enc(path))}catch(e){try{await api('/api/mkdir?path='+enc(path),{method:'POST'})}catch(e2){}}}

// --- accesso: nessuna chiamata verso i dati del dispositivo avviene prima
// che il codice a sei cifre mostrato sul pannello sia stato verificato.
function lockError(t){document.getElementById('lockError').textContent=t}
async function unlock(){
  let code=document.getElementById('lockCode').value.trim();
  if(!/^\d{6}$/.test(code)){lockError('Inserisci le sei cifre mostrate sul dispositivo.');return}
  lockError('Verifica in corso...');
  try{
    let r=await fetch('/api/status?code='+enc(code));
    if(!r.ok)throw new Error('invalid');
    localStorage.rustmixCode=code;
    enterApp();
  }catch(e){lockError('Codice non valido, riprova.')}
}
function lockOut(){
  delete localStorage.rustmixCode;
  document.getElementById('app').hidden=true;
  document.getElementById('lock').hidden=false;
  document.getElementById('lockCode').value='';
  lockError('');
  document.getElementById('lockCode').focus();
}
function enterApp(){
  document.getElementById('lock').hidden=true;
  document.getElementById('app').hidden=false;
  initApp();
}
async function tryAutoUnlock(){
  let saved=localStorage.rustmixCode;
  if(!saved){document.getElementById('lockCode').focus();return}
  try{
    let r=await fetch('/api/status?code='+enc(saved));
    if(!r.ok)throw new Error('invalid');
    enterApp();
  }catch(e){document.getElementById('lockCode').focus()}
}

// --- schede ---
function showTab(name){
  document.querySelectorAll('.tabpanel').forEach(el=>{el.hidden=true});
  document.querySelectorAll('.tab').forEach(el=>el.classList.toggle('active',el.dataset.tab===name));
  document.getElementById('tab-'+name).hidden=false;
  activeTab=name;
  if(name==='books')refreshBooks();
  else if(name==='wallpaper')refreshSleepGallery();
  else if(name==='files')loadList(current);
  else if(name==='wifi')wifiRefreshStatus().then(()=>{wifiLoadSaved();wifiLoadScan()});
}
function refreshActiveTab(){showTab(activeTab);fetchSpace()}
function initApp(){
  fetchSpace();
  ensureDir('/BOOKS');
  showTab('books');
}

// --- caricamento file: una fabbrica condivisa tra la scheda Libri e la
// Gestione file, ciascuna con la propria coda e cartella di destinazione.
function fatSafeName(name,isFolder){let dot=name.lastIndexOf('.');let stem=(isFolder||dot<0)?name:name.slice(0,dot);let ext=(!isFolder&&dot>=0)?name.slice(dot+1):'';stem=stem.toUpperCase().replace(/[^A-Z0-9_\-~]/g,'').slice(0,8)||'FILE';ext=ext.toUpperCase().replace(/[^A-Z0-9_\-~]/g,'').slice(0,3);return ext?stem+'.'+ext:stem}
function createUploader(opts){
  let queue=[],busy=false;
  function render(){
    let html='';
    for(const item of queue){
      html+='<div class="qitem '+item.status+'" data-id="'+item.id+'"><span class="name">'+escapeHtml(item.file.name)+'</span>'+(item.status==='queued'?'<input type="text" value="'+escapeHtml(item.name)+'" onchange="'+opts.varName+'.rename(\''+item.id+'\',this.value)">':'<span class="name">'+escapeHtml(item.name)+'</span>')+'<div class="bar"><i style="width:'+item.progress+'%"></i></div><span class="hint">'+(item.status==='error'?item.error:item.status)+'</span>'+(item.status==='queued'?'<button onclick="'+opts.varName+'.remove(\''+item.id+'\')">x</button>':'')+'</div>';
    }
    document.getElementById(opts.containerId).innerHTML=html;
  }
  function handleFiles(fileList){
    let used=opts.getExisting();
    for(const file of Array.from(fileList)){
      let name=fatSafeName(file.name,false),base=name,i=1;
      while(used.has(name)){let dot=base.lastIndexOf('.');let stem=dot<0?base:base.slice(0,dot);let ext=dot<0?'':base.slice(dot);stem=stem.slice(0,8-String(i).length)+i;name=stem+ext;i++}
      used.add(name);
      queue.push({file:file,name:name,status:'queued',progress:0,error:'',id:Math.random().toString(36).slice(2)});
    }
    render();
    process();
  }
  function rename(id,value){let item=queue.find(q=>q.id===id);if(item)item.name=fatSafeName(value,false)}
  function remove(id){queue=queue.filter(q=>!(q.id===id&&q.status==='queued'));render()}
  function uploadOne(item){
    return new Promise((resolve,reject)=>{
      let xhr=new XMLHttpRequest();
      let dir=opts.getDir();
      let path=(dir==='/'?'/':dir+'/')+item.name;
      let url='/api/upload?path='+enc(path)+'&code='+enc(getCode());
      xhr.open('POST',url);
      xhr.upload.onprogress=function(e){if(e.lengthComputable){item.progress=Math.round(e.loaded/e.total*100);render()}};
      xhr.onload=function(){if(xhr.status>=200&&xhr.status<300)resolve();else reject(new Error(xhr.responseText||('HTTP '+xhr.status)))};
      xhr.onerror=function(){reject(new Error('errore di rete'))};
      xhr.send(item.file);
    });
  }
  async function process(){
    if(busy)return;busy=true;let any=false;
    while(true){
      let item=queue.find(q=>q.status==='queued');
      if(!item)break;
      any=true;item.status='uploading';render();
      try{await uploadOne(item);item.status='done';item.progress=100}
      catch(e){item.status='error';item.error=e.message||'errore'}
      if(item.status==='done'&&opts.onItemDone){
        try{await opts.onItemDone(item)}catch(e){/* best-effort, never fails the upload itself */}
      }
      render();
    }
    busy=false;
    if(any&&opts.onDone)opts.onDone();
  }
  return {handleFiles:handleFiles,rename:rename,remove:remove};
}
function wireDropZone(zoneId,inputId,uploader){
  let zone=document.getElementById(zoneId);
  ['dragover','dragenter'].forEach(ev=>zone.addEventListener(ev,e=>{e.preventDefault();zone.classList.add('drag')}));
  ['dragleave','drop'].forEach(ev=>zone.addEventListener(ev,e=>{e.preventDefault();zone.classList.remove('drag')}));
  zone.addEventListener('drop',e=>{if(e.dataTransfer.files.length)uploader.handleFiles(e.dataTransfer.files)});
  document.getElementById(inputId).addEventListener('change',e=>{if(e.target.files.length)uploader.handleFiles(e.target.files);e.target.value=''});
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
  str(absPath);u64(sizeBytes);u64(modifiedSeconds);str('epub');u16(COVER_THUMB_W);u16(COVER_THUMB_H);str('1');
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
const queueUploader=createUploader({containerId:'queue',varName:'queueUploader',getDir:()=>current,getExisting:()=>new Set(entries.map(e=>e.name)),onDone:()=>{status('Caricamento completato');loadList(current)}});
const booksUploader=createUploader({containerId:'queueBooks',varName:'booksUploader',getDir:()=>'/BOOKS',getExisting:()=>new Set(books.map(b=>b.path.split('/').pop())),onItemDone:pregenerateBookCoverClientSide,onDone:()=>{status('Libri caricati');refreshBooks();fetchSpace()}});
wireDropZone('drop','file',queueUploader);
wireDropZone('dropBooks','fileBooks',booksUploader);

// --- libreria libri: le copertine arrivano gia pronte dal browser (vedi
// pregenerateBookCoverClientSide sopra), quindi sia questa pagina che la
// schermata Library del dispositivo le trovano gia in cache.
async function refreshBooks(){
  try{
    let t=await api('/api/books');
    books=JSON.parse(t);
    renderBooks();
    document.getElementById('bookCount').textContent=books.length+(books.length===1?' libro':' libri');
  }catch(e){document.getElementById('bookGrid').innerHTML='<p class="hint">Errore: '+escapeHtml(e.message)+'</p>'}
}
function renderBooks(){
  let q=document.getElementById('bookSearch').value.trim().toLowerCase();
  let visible=books.filter(b=>!q||b.title.toLowerCase().includes(q));
  let html='';
  for(const b of visible){
    let coverUrl='/api/cover?path='+enc(b.path)+'&code='+enc(getCode());
    html+='<div class="book-card"><div class="book-cover"><img src="'+coverUrl+'" alt="" loading="lazy" onerror="this.parentElement.classList.add(\'empty\')"></div><div class="book-title">'+escapeHtml(b.title)+'</div><div class="hint">'+b.format+' &middot; '+formatBytes(b.size)+'</div><div class="actions"><a href="/api/download?code='+enc(getCode())+'&path='+enc(b.path)+'">Scarica</a><button class="danger" onclick="deleteBook(\''+b.path.replace(/'/g,"\\'")+'\')">Elimina</button></div></div>';
  }
  document.getElementById('bookGrid').innerHTML=html||'<p class="hint">Nessun libro caricato. Trascina un file EPUB o TXT qui sopra per iniziare.</p>';
}
async function deleteBook(path){
  if(!confirm('Eliminare questo libro dal dispositivo?'))return;
  try{await api('/api/delete?path='+enc(path),{method:'POST'});status('Libro eliminato');refreshBooks();fetchSpace()}catch(e){status('Errore: '+e.message)}
}

// --- gestione file (avanzata) ---
function crumbsHtml(path){let parts=path.split('/').filter(Boolean);let html='<a onclick="loadList(\'/\')">RUSTMIX</a>';let acc='';for(const p of parts){acc+='/'+p;html+='<span>/</span><a onclick="loadList(\''+acc+'\')">'+p+'</a>'}return html}
async function loadList(path){try{current=path;let t=await api('/api/list?path='+enc(path));entries=JSON.parse(t);selected.clear();document.getElementById('crumbs').innerHTML=crumbsHtml(path);renderTable();status('Pronto - '+entries.length+' elementi');fetchSpace()}catch(e){status('Errore: '+e.message)}}
function matchesSearch(name){let q=document.getElementById('search').value.trim().toLowerCase();return !q||name.toLowerCase().includes(q)}
function renderTable(){let rows='';if(current!=='/')rows+='<tr><td></td><td colspan="3"><button onclick="up()">.. Su</button></td></tr>';let visible=entries.filter(e=>matchesSearch(e.name));for(const e of visible){let p=join(e.name);let checked=selected.has(e.name)?'checked':'';let kindLabel=e.kind==='folder'?'<span class="kind-folder">cartella</span>':'file';let openOrDownload=e.kind==='folder'?'<button onclick="loadList(\''+p+'\')">Apri</button>':'<a href="/api/download?code='+enc(getCode())+'&path='+enc(p)+'">Scarica</a>';rows+='<tr><td><input type="checkbox" '+checked+' onchange="toggleSelect(\''+e.name+'\',this.checked)"></td><td>'+escapeHtml(e.name)+'</td><td>'+kindLabel+'</td><td>'+(e.kind==='folder'?'':formatBytes(e.size))+'</td><td class="actions">'+openOrDownload+' <button onclick="renamePath(\''+p+'\')">Rinomina</button> <button class="danger" onclick="deletePath(\''+p+'\')">Elimina</button></td></tr>'}document.getElementById('rows').innerHTML=rows||'<tr><td colspan="5" class="hint">Nessun elemento</td></tr>';document.getElementById('selectAll').checked=visible.length>0&&visible.every(e=>selected.has(e.name));updateBulkBar()}
function up(){let p=current.split('/').filter(Boolean);p.pop();loadList('/'+p.join('/'))}
function toggleSelect(name,checked){if(checked)selected.add(name);else selected.delete(name);renderTable()}
function toggleSelectAll(checked){let visible=entries.filter(e=>matchesSearch(e.name));for(const e of visible){if(checked)selected.add(e.name);else selected.delete(e.name)}renderTable()}
function clearSelection(){selected.clear();renderTable()}
function updateBulkBar(){let bar=document.getElementById('bulk');if(selected.size>0){bar.classList.add('show');document.getElementById('bulkCount').textContent=selected.size+' selezionati'}else{bar.classList.remove('show')}}
async function bulkDelete(){if(selected.size===0)return;if(!confirm('Eliminare '+selected.size+' elementi selezionati?'))return;let names=Array.from(selected);for(const name of names){try{await api('/api/delete?path='+enc(join(name)),{method:'POST'})}catch(e){status('Errore eliminando '+name+': '+e.message)}}status('Eliminati '+names.length+' elementi');loadList(current)}
async function newFolder(){let name=prompt('Nome cartella (FAT 8.3, es. LIBRI)');if(!name)return;let safe=fatSafeName(name,true);try{await api('/api/mkdir?path='+enc(join(safe)),{method:'POST'});status('Cartella creata: '+safe);loadList(current)}catch(e){status('Errore: '+e.message)}}
async function renamePath(p){let name=prompt('Nuovo nome (FAT 8.3-safe)');if(!name)return;let parent=p.substring(0,p.lastIndexOf('/'))||'/';let safe=fatSafeName(name,false);let to=(parent==='/'?'/':parent+'/')+safe;try{await api('/api/rename?from='+enc(p)+'&to='+enc(to),{method:'POST'});status('Rinominato in '+safe);loadList(current)}catch(e){status('Errore: '+e.message)}}
async function deletePath(p){if(!confirm('Eliminare '+p+'?'))return;try{await api('/api/delete?path='+enc(p),{method:'POST'});status('Eliminato');loadList(current)}catch(e){status('Errore: '+e.message)}}

// --- sfondi ---
function searchGoogleImages(){let q=document.getElementById('gquery').value.trim();if(!q)return;window.open('https://www.google.com/search?tbm=isch&q='+encodeURIComponent(q),'_blank')}
const BG_W=800,BG_H=480;
let bgObjectUrl=null,bgColorImageData=null,bgDithered=null;
let bgCrop={tx:0,ty:0,zoom:1,baseScale:1,frameW:480,frameH:288,natW:0,natH:0};
function bgStatus(t){document.getElementById('bgStatus').textContent=t}
function hideStages(){document.querySelectorAll('.stage').forEach(el=>el.classList.remove('show'))}
function showStage(id){hideStages();document.getElementById(id).classList.add('show')}
function cancelCrop(){hideStages()}
function backToCrop(){showStage('cropStage')}
function onImageReady(resetZoom){
  let img=document.getElementById('cropImg');
  bgCrop.natW=img.naturalWidth;bgCrop.natH=img.naturalHeight;
  showStage('cropStage');
  let frame=document.getElementById('cropFrame');
  bgCrop.frameW=frame.clientWidth;bgCrop.frameH=frame.clientHeight;
  bgCrop.baseScale=Math.max(bgCrop.frameW/bgCrop.natW,bgCrop.frameH/bgCrop.natH);
  if(resetZoom){bgCrop.zoom=1}else{bgCrop.zoom=Math.max(bgCrop.zoom,1)}
  document.getElementById('zoomRange').value=bgCrop.zoom;
  centerCrop();
  applyCropTransform();
  bgStatus('');
}
function loadBackgroundFile(file){
  if(bgObjectUrl)URL.revokeObjectURL(bgObjectUrl);
  bgObjectUrl=URL.createObjectURL(file);
  let img=document.getElementById('cropImg');
  img.onload=function(){onImageReady(true)};
  img.src=bgObjectUrl;
}
function rotateImage(delta){
  let img=document.getElementById('cropImg');
  if(!img.naturalWidth)return;
  let srcW=img.naturalWidth,srcH=img.naturalHeight;
  let canvas=document.createElement('canvas');
  canvas.width=srcH;canvas.height=srcW;
  let ctx=canvas.getContext('2d');
  ctx.translate(canvas.width/2,canvas.height/2);
  ctx.rotate(delta*Math.PI/180);
  ctx.drawImage(img,-srcW/2,-srcH/2);
  canvas.toBlob(function(blob){
    if(bgObjectUrl)URL.revokeObjectURL(bgObjectUrl);
    bgObjectUrl=URL.createObjectURL(blob);
    img.onload=function(){onImageReady(false)};
    img.src=bgObjectUrl;
  });
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
const cropFrameEl=document.getElementById('cropFrame');
let bgDragging=false,bgDragStartX=0,bgDragStartY=0,bgDragStartTx=0,bgDragStartTy=0;
cropFrameEl.addEventListener('pointerdown',e=>{bgDragging=true;bgDragStartX=e.clientX;bgDragStartY=e.clientY;bgDragStartTx=bgCrop.tx;bgDragStartTy=bgCrop.ty;cropFrameEl.setPointerCapture(e.pointerId)});
cropFrameEl.addEventListener('pointermove',e=>{if(!bgDragging)return;bgCrop.tx=bgDragStartTx+(e.clientX-bgDragStartX);bgCrop.ty=bgDragStartTy+(e.clientY-bgDragStartY);clampCrop();applyCropTransform()});
cropFrameEl.addEventListener('pointerup',()=>{bgDragging=false});
cropFrameEl.addEventListener('pointercancel',()=>{bgDragging=false});
document.getElementById('zoomRange').addEventListener('input',e=>{bgCrop.zoom=parseFloat(e.target.value);clampCrop();applyCropTransform()});
wireDropZone('dropBg','fileBg',{handleFiles:list=>loadBackgroundFile(list[0])});
function confirmCrop(){
  let scale=bgCrop.baseScale*bgCrop.zoom;
  let sx=-bgCrop.tx/scale,sy=-bgCrop.ty/scale,sw=bgCrop.frameW/scale,sh=bgCrop.frameH/scale;
  let canvas=document.createElement('canvas');
  canvas.width=BG_W;canvas.height=BG_H;
  let ctx=canvas.getContext('2d');
  ctx.drawImage(document.getElementById('cropImg'),sx,sy,sw,sh,0,0,BG_W,BG_H);
  bgColorImageData=ctx.getImageData(0,0,BG_W,BG_H);
  document.getElementById('brightRange').value=0;
  document.getElementById('contrastRange').value=0;
  renderDitheredPreview();
  showStage('adjustStage');
  suggestBgName();
}
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
function renderDitheredPreview(){
  if(!bgColorImageData)return;
  let brightness=parseInt(document.getElementById('brightRange').value,10);
  let contrast=parseInt(document.getElementById('contrastRange').value,10);
  let lum=computeLuminance(bgColorImageData.data,BG_W,BG_H,brightness,contrast);
  bgDithered=ditherFloydSteinberg(lum,BG_W,BG_H);
  let canvas=document.getElementById('bgPreview');
  let ctx=canvas.getContext('2d');
  let out=ctx.createImageData(BG_W,BG_H);
  for(let i=0;i<bgDithered.length;i++){let v=bgDithered[i],o=i*4;out.data[o]=v;out.data[o+1]=v;out.data[o+2]=v;out.data[o+3]=255}
  ctx.putImageData(out,0,0);
}
document.getElementById('brightRange').addEventListener('input',renderDitheredPreview);
document.getElementById('contrastRange').addEventListener('input',renderDitheredPreview);
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
function bgFileName(raw){
  let dot=raw.lastIndexOf('.');
  let stem=dot<0?raw:raw.slice(0,dot);
  stem=stem.toUpperCase().replace(/[^A-Z0-9_\-~]/g,'').slice(0,8)||'SLEEP';
  return stem+'.BMP';
}
async function suggestBgName(){
  try{
    await ensureDir('/SLEEP');
    let t=await api('/api/list?path=/SLEEP');
    let existing=new Set(JSON.parse(t).map(e=>e.name.toUpperCase()));
    for(let i=1;i<=999;i++){
      let candidate='SLEEP'+String(i).padStart(3,'0')+'.BMP';
      if(!existing.has(candidate)){document.getElementById('bgName').value=candidate;return}
    }
  }catch(e){}
}
async function uploadBackground(){
  if(!bgDithered){bgStatus('Nessuna immagine pronta');return}
  let name=bgFileName(document.getElementById('bgName').value||'SLEEP001');
  document.getElementById('bgName').value=name;
  bgStatus('Preparazione BMP...');
  try{
    await ensureDir('/SLEEP');
    let bytes=buildSleepBmp(bgDithered);
    let blob=new Blob([bytes],{type:'application/octet-stream'});
    await new Promise((resolve,reject)=>{
      let xhr=new XMLHttpRequest();
      let url='/api/upload?path='+enc('/SLEEP/'+name)+'&code='+enc(getCode());
      xhr.open('POST',url);
      xhr.upload.onprogress=function(e){if(e.lengthComputable)bgStatus('Caricamento '+Math.round(e.loaded/e.total*100)+'%')};
      xhr.onload=function(){if(xhr.status>=200&&xhr.status<300)resolve();else reject(new Error(xhr.responseText||('HTTP '+xhr.status)))};
      xhr.onerror=function(){reject(new Error('errore di rete'))};
      xhr.send(blob);
    });
    bgStatus('Sfondo caricato: /SLEEP/'+name);
    refreshSleepGallery();
    fetchSpace();
  }catch(e){bgStatus('Errore: '+e.message)}
}
async function refreshSleepGallery(){
  try{
    await ensureDir('/SLEEP');
    let t=await api('/api/list?path=/SLEEP');
    let list=JSON.parse(t).filter(e=>e.kind==='file');
    let html='';
    for(const e of list){
      let url='/api/download?code='+enc(getCode())+'&path='+enc('/SLEEP/'+e.name);
      html+='<div class="book-card"><div class="book-cover"><img src="'+url+'" alt="" loading="lazy" onerror="this.parentElement.classList.add(\'empty\')"></div><div class="book-title">'+escapeHtml(e.name)+'</div><div class="actions"><a href="'+url+'">Scarica</a><button class="danger" onclick="deleteSleepImage(\''+e.name+'\')">Elimina</button></div></div>';
    }
    document.getElementById('sleepGallery').innerHTML=html||'<p class="hint">Nessuno sfondo caricato.</p>';
  }catch(e){document.getElementById('sleepGallery').innerHTML='<p class="hint">Errore: '+escapeHtml(e.message)+'</p>'}
}
async function deleteSleepImage(name){
  if(!confirm('Eliminare questo sfondo?'))return;
  try{await api('/api/delete?path='+enc('/SLEEP/'+name),{method:'POST'});status('Sfondo eliminato');refreshSleepGallery();fetchSpace()}catch(e){status('Errore: '+e.message)}
}

// --- Wi-Fi: sempre disponibile la lista reti salvate (dimentica funziona
// sia via hotspot che gia' in LAN, e' solo una modifica a WIFI.TXT); la
// scansione e l'aggiunta di una nuova rete richiedono invece di essere sul
// hotspot di bootstrap del dispositivo, perche' unirsi a una rete diversa
// da quella gia' in uso va prima validato con una connessione reale (vedi
// NetworkRuntime::try_join_candidate lato firmware).
let wifiHotspot=false;
// onclick="..." attributes below are double-quoted, so an embedded argument
// must be single-quoted JS (matching e.g. deleteBook's own pattern above) --
// JSON.stringify's double-quoted output would close the HTML attribute at
// its very first character and silently break the handler.
function jsq(s){return "'"+String(s).replace(/\\/g,'\\\\').replace(/'/g,"\\'")+"'"}
async function wifiRefreshStatus(){
  try{
    let s=JSON.parse(await api('/api/status'));
    wifiHotspot=!!s.hotspot;
    document.getElementById('wifiHint').hidden=wifiHotspot;
  }catch(e){}
}
async function wifiLoadScan(){
  try{
    let list=JSON.parse(await api('/api/scan'));
    let html='';
    for(let i=0;i<list.length;i++){
      html+='<li><span class="name">'+escapeHtml(list[i].ssid)+'</span><span class="rssi">'+list[i].rssi+' dBm</span><button onclick="wifiOpenJoin('+jsq(list[i].ssid)+',true)">Aggiungi</button></li>';
    }
    document.getElementById('wifiScan').innerHTML=html||'<li class="hint">Nessuna rete trovata</li>';
  }catch(e){}
}
async function wifiLoadSaved(){
  try{
    let list=JSON.parse(await api('/api/networks'));
    let html=list.length?'':'<li class="hint">Nessuna rete salvata</li>';
    for(let i=0;i<list.length;i++){
      let ssid=list[i].ssid;
      html+='<li><span class="name">'+escapeHtml(ssid)+'</span><button onclick="wifiOpenJoin('+jsq(ssid)+',true)">Cambia password</button><button class="danger" onclick="wifiForget('+jsq(ssid)+')">Dimentica</button></li>';
    }
    html+='<li><button onclick="wifiOpenJoin(\'\',false)">+ Aggiungi rete manualmente</button></li>';
    document.getElementById('wifiSaved').innerHTML=html;
  }catch(e){}
}
function wifiOpenJoin(ssid,locked){
  wifiCancelForget();
  let card=document.getElementById('wifiJoinCard');
  card.style.display='block';
  let field=document.getElementById('wifiJoinSsid');
  field.value=ssid||'';
  field.readOnly=!!locked;
  document.getElementById('wifiJoinPassword').value='';
  document.getElementById('wifiJoinTitle').textContent=ssid?('Connetti a '+ssid):'Aggiungi rete';
  document.getElementById('wifiJoinStatus').textContent='';
  document.getElementById('wifiJoinWarning').hidden=wifiHotspot;
  card.scrollIntoView({behavior:'smooth',block:'start'});
}
function wifiCloseJoin(){document.getElementById('wifiJoinCard').style.display='none'}
let wifiForgetSsid=null;
function wifiForget(ssid){
  wifiCloseJoin();
  wifiForgetSsid=ssid;
  document.getElementById('wifiForgetText').textContent='Dimenticare "'+ssid+'"? Non si puo\' annullare.';
  let card=document.getElementById('wifiForgetCard');
  card.style.display='block';
  card.scrollIntoView({behavior:'smooth',block:'center'});
}
function wifiCancelForget(){
  wifiForgetSsid=null;
  document.getElementById('wifiForgetCard').style.display='none';
}
async function wifiConfirmForget(){
  let ssid=wifiForgetSsid;
  if(!ssid)return;
  wifiCancelForget();
  await api('/api/networks/delete?ssid='+enc(ssid),{method:'POST'});
  wifiLoadSaved();
}
async function wifiSubmitJoin(){
  let ssid=document.getElementById('wifiJoinSsid').value.trim();
  let password=document.getElementById('wifiJoinPassword').value;
  if(!ssid){document.getElementById('wifiJoinStatus').textContent='Il nome della rete e\' obbligatorio.';return}
  document.getElementById('wifiJoinStatus').textContent='Connessione in corso...';
  await api('/api/networks?ssid='+enc(ssid)+'&password='+enc(password),{method:'POST'});
  wifiPollJoinStatus();
}
async function wifiPollJoinStatus(){
  try{
    let s=JSON.parse(await api('/api/status'));
    let j=s.wifi_join||{state:'idle'};
    if(j.state==='testing'){document.getElementById('wifiJoinStatus').textContent='Connessione in corso...';setTimeout(wifiPollJoinStatus,1000);return}
    if(j.state==='connected'){document.getElementById('wifiJoinStatus').textContent='Connessa e salvata.';wifiLoadSaved();setTimeout(wifiCloseJoin,1500);return}
    if(j.state==='failed'){document.getElementById('wifiJoinStatus').textContent='Errore: '+(j.error||'connessione non riuscita');return}
  }catch(e){document.getElementById('wifiJoinStatus').textContent='Errore di rete.'}
}
setInterval(()=>{if(activeTab==='wifi')wifiLoadScan()},12000);

tryAutoUnlock();
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
    }

    impl SharedStatus {
        fn new(url: String, code: String, ap_ssid: Option<String>, ap_password: Option<String>) -> Self {
            Self {
                snapshot: WifiTransferSnapshot {
                    state: WifiTransferState::Ready,
                    url: Some(url),
                    code: Some(code),
                    ap_ssid,
                    ap_password,
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
        pub fn start_lan(ipv4: &str, code: String) -> Result<Self> {
            let url = format!("http://{ipv4}/");
            Self::start_inner(url, code, None)
        }

        /// Start reachable via the device's own bootstrap hotspot instead,
        /// for when no Wi-Fi is joined yet. `ap_ssid`/`ap_password` come from
        /// the `NetworkRuntime::start_provisioning` call the caller is
        /// expected to have just made.
        pub fn start_ap(
            portal_ip: &str,
            ap_ssid: String,
            ap_password: String,
            code: String,
        ) -> Result<Self> {
            let answer_ip = portal_ip
                .parse::<std::net::Ipv4Addr>()
                .with_context(|| format!("portal IP is not a valid IPv4 address: {portal_ip}"))?
                .octets();
            // Answer every DNS query on the hotspot with our own address, so
            // the phone's captive-portal probe (whatever hostname it picks)
            // lands on this HTTP server and the OS offers to open it.
            let dns = CaptivePortalDns::start(answer_ip)?;
            let url = format!("http://{portal_ip}/");
            Self::start_inner(url, code, Some((ap_ssid, ap_password, dns)))
        }

        fn start_inner(
            url: String,
            code: String,
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
                code.clone(),
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
                max_uri_handlers: 16,
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
            let list_code = code.clone();
            server.fn_handler("/api/list", Method::Get, move |request| {
                authenticate(request.uri(), &list_code)?;
                let relative = query_value(request.uri(), "path").unwrap_or_else(|| "/".into());
                let body = list_directory_json(&relative)?;
                lock(&list_shared).touch(format!("Listed {relative}"), body.len());
                request.into_ok_response()?.write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let download_shared = Arc::clone(&shared);
            let download_code = code.clone();
            server.fn_handler("/api/download", Method::Get, move |request| {
                authenticate(request.uri(), &download_code)?;
                let relative = required_query(request.uri(), "path")?;
                reject_protected(&relative)?;
                let path = resolve_portal_path(&relative).map_err(|error| anyhow!(error))?;
                let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
                let mut response = request.into_ok_response()?;
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
            let upload_code = code.clone();
            server.fn_handler("/api/upload", Method::Post, move |mut request| {
                authenticate(request.uri(), &upload_code)?;
                let relative = required_query(request.uri(), "path")?;
                reject_protected(&relative)?;
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
            let delete_code = code.clone();
            server.fn_handler("/api/delete", Method::Post, move |request| {
                authenticate(request.uri(), &delete_code)?;
                let relative = required_query(request.uri(), "path")?;
                reject_protected(&relative)?;
                let path = resolve_portal_path(&relative).map_err(|error| anyhow!(error))?;
                if path.is_dir() { fs::remove_dir(&path)?; } else { fs::remove_file(&path)?; }
                lock(&delete_shared).touch(format!("Deleted {relative}"), 0);
                info!("rustmix-wave=wifi-transfer-request method=POST route=delete path={relative} status=completed");
                request.into_ok_response()?.write_all(b"deleted")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let mkdir_shared = Arc::clone(&shared);
            let mkdir_code = code.clone();
            server.fn_handler("/api/mkdir", Method::Post, move |request| {
                authenticate(request.uri(), &mkdir_code)?;
                let relative = required_query(request.uri(), "path")?;
                reject_protected(&relative)?;
                let path = resolve_portal_path(&relative).map_err(|error| anyhow!(error))?;
                fs::create_dir(&path)?;
                lock(&mkdir_shared).touch(format!("Created {relative}"), 0);
                info!("rustmix-wave=wifi-transfer-request method=POST route=mkdir path={relative} status=completed");
                request.into_ok_response()?.write_all(b"created")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let rename_shared = Arc::clone(&shared);
            let rename_code = code.clone();
            server.fn_handler("/api/rename", Method::Post, move |request| {
                authenticate(request.uri(), &rename_code)?;
                let from = required_query(request.uri(), "from")?;
                let to = required_query(request.uri(), "to")?;
                reject_protected(&from)?;
                reject_protected(&to)?;
                let source = resolve_portal_path(&from).map_err(|error| anyhow!(error))?;
                let destination = resolve_portal_path(&to).map_err(|error| anyhow!(error))?;
                fs::rename(&source, &destination)?;
                lock(&rename_shared).touch(format!("Renamed {from}"), 0);
                info!("rustmix-wave=wifi-transfer-request method=POST route=rename from={from} to={to} status=completed");
                request.into_ok_response()?.write_all(b"renamed")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let status_shared = Arc::clone(&shared);
            let status_code = code.clone();
            server.fn_handler("/api/status", Method::Get, move |request| {
                authenticate(request.uri(), &status_code)?;
                let snapshot = lock(&status_shared).snapshot.clone();
                let (total_bytes, free_bytes) = sd_space_bytes().unwrap_or((0, 0));
                let body = format!(
                    "{{\"state\":\"{}\",\"last_action\":\"{}\",\"last_bytes\":{},\"total_bytes\":{total_bytes},\"free_bytes\":{free_bytes},\"hotspot\":{},\"wifi_join\":{}}}",
                    snapshot.state.label(),
                    json_escape(&snapshot.last_action),
                    snapshot.last_bytes,
                    snapshot.ap_ssid.is_some(),
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
            let scan_code = code.clone();
            server.fn_handler("/api/scan", Method::Get, move |request| {
                authenticate(request.uri(), &scan_code)?;
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
            let networks_code = code.clone();
            server.fn_handler("/api/networks", Method::Get, move |request| {
                authenticate(request.uri(), &networks_code)?;
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
            let join_code = code.clone();
            server.fn_handler("/api/networks", Method::Post, move |mut request| {
                authenticate(request.uri(), &join_code)?;
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
            let delete_net_code = code.clone();
            server.fn_handler("/api/networks/delete", Method::Post, move |request| {
                authenticate(request.uri(), &delete_net_code)?;
                let ssid = required_query(request.uri(), "ssid")?;
                {
                    let mut guard = lock(&delete_net_shared);
                    guard.touch_activity();
                    guard.pending_delete = Some(ssid);
                }
                request.into_ok_response()?.write_all(b"deleted")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let books_shared = Arc::clone(&shared);
            let books_code = code.clone();
            server.fn_handler("/api/books", Method::Get, move |request| {
                authenticate(request.uri(), &books_code)?;
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
            let cover_code = code.clone();
            server.fn_handler("/api/cover", Method::Get, move |request| {
                authenticate(request.uri(), &cover_code)?;
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
            let dns_guard = if let Some(dns) = dns {
                let redirect_url = url.clone();
                server.fn_handler("*", Method::Get, move |request| {
                    info!(
                        "rustmix-wave=wifi-transfer-server status=captive-probe uri={}",
                        request.uri()
                    );
                    request
                        .into_response(
                            302,
                            Some("Found"),
                            &[
                                ("Location", redirect_url.as_str()),
                                ("Cache-Control", "no-store"),
                            ],
                        )?
                        .write_all(b"Redirecting to the Rustmix-Wave portal.")?;
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

    fn authenticate(uri: &str, expected: &str) -> Result<()> {
        if query_value(uri, "code").as_deref() == Some(expected) {
            Ok(())
        } else {
            bail!("session code required")
        }
    }

    fn required_query(uri: &str, key: &str) -> Result<String> {
        query_value(uri, key).ok_or_else(|| anyhow!("missing query parameter: {key}"))
    }

    fn reject_protected(relative: &str) -> Result<()> {
        if is_protected_portal_path(relative) {
            bail!("protected configuration file")
        }
        Ok(())
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
            if is_protected_portal_path(&child_relative) || !super::is_fat83_component(&name) {
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
        is_fat83_component, is_protected_portal_path, query_value, resolve_portal_path,
        WifiTransferSnapshot, WifiTransferState,
    };

    #[test]
    fn portal_is_off_until_the_user_explicitly_starts_it() {
        let snapshot = WifiTransferSnapshot::default();
        assert_eq!(snapshot.state, WifiTransferState::Off);
        assert!(!snapshot.is_active());
    }

    #[test]
    fn portal_paths_are_confined_and_fat83_safe() {
        assert!(resolve_portal_path("/BOOKS/POIROT01.EPU").is_ok());
        assert!(resolve_portal_path("../WIFI.TXT").is_err());
        assert!(resolve_portal_path("/BOOKS/long-file-name.txt").is_err());
        assert!(is_fat83_component("MAIN.LUA"));
        assert!(is_fat83_component("SUDOKU"));
        assert!(!is_fat83_component("NOT FAT SAFE.TXT"));
    }

    #[test]
    fn configuration_files_are_protected() {
        assert!(is_protected_portal_path("/WIFI.TXT"));
        assert!(is_protected_portal_path("CLOCK.TXT"));
        assert!(is_protected_portal_path("ALARMS.TXT"));
        assert!(is_protected_portal_path("/VOICE/META.TXT"));
        assert!(is_protected_portal_path("VOICE/SETTINGS.TXT"));
        assert!(!is_protected_portal_path("/VOICE/VOICE001.WAV"));
        assert!(!is_protected_portal_path("/BOOKS/NOTES001.TXT"));
    }

    #[test]
    fn query_parser_decodes_portal_paths() {
        assert_eq!(
            query_value("/api/list?code=123456&path=%2FBOOKS", "path").as_deref(),
            Some("/BOOKS")
        );
    }
}
