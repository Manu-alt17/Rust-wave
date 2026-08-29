//! Explicitly activated Wi-Fi SD-card transfer web portal.
//!
//! The portal is intentionally dormant after boot.  The user must enter
//! Settings > Network and select Start Wi-Fi Transfer.  The target-specific
//! HTTP server owns its own ESP-IDF task while the main loop retains display,
//! routing and sleep ownership.

use std::path::{Component, Path, PathBuf};

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
/// Stop a forgotten transfer portal after ten minutes without HTTP traffic.
pub const WIFI_TRANSFER_INACTIVITY_SECONDS: u64 = 10 * 60;
/// The first slice uses the conventional LAN HTTP port.
pub const WIFI_TRANSFER_HTTP_PORT: u16 = 80;
/// One authenticated browser session code is shown on the e-paper screen.
pub const WIFI_TRANSFER_CODE_DIGITS: usize = 6;
/// Bound one directory listing to protect the HTTP task heap.
pub const WIFI_TRANSFER_MAX_DIRECTORY_ROWS: usize = 256;

/// User request handed from the hardware-independent UI into `main.rs`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTransferUiRequest {
    Start,
    Stop,
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
}

/// Small main-loop snapshot updated when the server starts, stops or handles a
/// completed filesystem request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiTransferSnapshot {
    pub state: WifiTransferState,
    pub url: Option<String>,
    pub code: Option<String>,
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
            last_action: "Starting LAN portal".into(),
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
            | "WEATHER.TXT"
            | "VOICE/META.TXT"
            | "VOICE/SETTINGS.TXT"
            | "APPS/CALENDAR/EVENTS.TMP"
            | "APPS/CALENDAR/EVENTS.BAK"
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
        time::{Duration, Instant},
    };

    use anyhow::{anyhow, bail, Context, Result};
    use embedded_svc::{http::Method, io::Write as _};
    use esp_idf_svc::{
        http::server::{Configuration, EspHttpServer},
        sys,
    };
    use log::{info, warn};

    use crate::storage::SD_MOUNT_POINT;

    use super::{
        is_protected_portal_path, query_value, resolve_portal_path, WifiTransferSnapshot,
        WifiTransferState, WIFI_TRANSFER_HTTP_PORT, WIFI_TRANSFER_INACTIVITY_SECONDS,
        WIFI_TRANSFER_MAX_DIRECTORY_ROWS, WIFI_TRANSFER_MAX_UPLOAD_BYTES, WIFI_TRANSFER_ROOT,
        WIFI_TRANSFER_SERVER_STACK_BYTES, WIFI_TRANSFER_STREAM_CHUNK_BYTES,
    };

    const PORTAL_HTML: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Rustmix-Wave Transfer</title><style>
*{box-sizing:border-box}
:root{--bg:#f5f6f8;--card:#fff;--text:#1a1d23;--muted:#6b7280;--border:#e2e5ea;--accent:#2563eb;--accent-dark:#1d4ed8;--danger:#dc2626;--radius:10px}
@media (prefers-color-scheme:dark){:root{--bg:#14161a;--card:#1c1f26;--text:#e8eaed;--muted:#9aa3b2;--border:#2a2e37;--accent:#3b82f6;--accent-dark:#60a5fa;--danger:#f87171}}
body{margin:0;background:var(--bg);color:var(--text);font-family:-apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif}
.wrap{max-width:960px;margin:0 auto;padding:1rem}
header.top{display:flex;flex-wrap:wrap;gap:.6rem;align-items:center;justify-content:space-between;margin-bottom:1rem}
h1{font-size:1.2rem;margin:0}
.badge{background:var(--card);border:1px solid var(--border);border-radius:999px;padding:.3rem .8rem;font-size:.8rem;color:var(--muted)}
.card{background:var(--card);border:1px solid var(--border);border-radius:var(--radius);padding:1rem;margin-bottom:1rem}
input,button{font:inherit}
input[type=text],input[type=search]{background:transparent;border:1px solid var(--border);border-radius:8px;padding:.5rem .7rem;color:var(--text)}
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
#status{font-size:.85rem;color:var(--muted);white-space:pre-wrap}
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
.mt-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(110px,1fr));gap:.6rem;margin:.6rem 0}
.mt-item{border:1px solid var(--border);border-radius:8px;padding:.4rem;text-align:center;font-size:.75rem;cursor:pointer}
.mt-item.sel{border-color:var(--accent);background:rgba(37,99,235,.12)}
.mt-item img{width:100%;border-radius:4px;display:block;margin-bottom:.3rem}
.mt-lib-row{display:flex;align-items:center;gap:.5rem;font-size:.85rem;padding:.35rem 0;border-bottom:1px solid var(--border)}
.mt-lib-row .name{flex:1}
</style></head><body><div class="wrap">
<header class="top">
<h1>Rustmix-Wave &middot; Wi-Fi Transfer</h1>
<div style="display:flex;gap:.5rem;align-items:center;flex-wrap:wrap">
<span class="badge" id="space">Spazio: --</span>
<input id="code" type="text" inputmode="numeric" maxlength="6" placeholder="Codice" onkeydown="if(event.key==='Enter')saveCode()">
<button class="primary" onclick="saveCode()">Sblocca</button>
<button onclick="loadList(current)">Aggiorna</button>
</div>
</header>
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
<div class="card" id="magic">
<h2 style="margin:0 0 .5rem;font-size:1.05rem">Magic: The Gathering &middot; Token</h2>
<p class="hint">Cerca i token su Scryfall (es. drago, zombie, umano), seleziona quelli che vuoi avere sul dispositivo e caricali: l&apos;immagine viene convertita in bianco e nero nel browser e salvata in MAGIC. Sul dispositivo puoi mostrarne uno grande, oppure due affiancati: in quel caso lo schermo ruota di 90&deg;, quindi gira anche il dispositivo per vederli dritti.</p>
<div class="search-row">
<input id="mtQuery" type="text" placeholder="Es. drago, zombie, umano..." onkeydown="if(event.key==='Enter')mtSearch()">
<button class="primary" onclick="mtSearch()">Cerca</button>
</div>
<div class="mt-grid" id="mtResults"></div>
<p><button class="primary" onclick="mtUploadSelected()">Carica selezionati</button> <span class="hint" id="mtSelectedCount"></span></p>
<pre id="mtStatus" class="hint">Cerca un token per iniziare.</pre>
<h2 style="margin:1rem 0 .5rem;font-size:1.05rem">Token sul dispositivo</h2>
<div id="mtLibrary" class="hint">Caricamento...</div>
</div>
<pre id="status">Inserisci il codice a sei cifre mostrato sul dispositivo.</pre>
</div>
<script>
let current='/',entries=[],selected=new Set(),queue=[],busy=false;
function status(t){document.getElementById('status').textContent=t}
function enc(s){return encodeURIComponent(s)}
function join(n){return (current==='/'?'/':current+'/')+n}
function getCode(){return localStorage.rustmixCode||document.getElementById('code').value}
function saveCode(){localStorage.rustmixCode=document.getElementById('code').value;loadList(current)}
function escapeHtml(s){return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')}
function formatBytes(n){if(n===undefined||n===null)return '--';const u=['B','KB','MB','GB'];let i=0,v=n;while(v>=1024&&i<u.length-1){v/=1024;i++}return (i===0?v:v.toFixed(1))+' '+u[i]}
async function api(url,opt){let sep=url.includes('?')?'&':'?';let r=await fetch(url+sep+'code='+enc(getCode()),opt);let t=await r.text();if(!r.ok)throw new Error(t||('HTTP '+r.status));return t}
async function fetchSpace(){try{let s=JSON.parse(await api('/api/status'));document.getElementById('space').textContent='Spazio libero: '+formatBytes(s.free_bytes)+' / '+formatBytes(s.total_bytes)}catch(e){}}
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
function fatSafeName(name,isFolder){let dot=name.lastIndexOf('.');let stem=(isFolder||dot<0)?name:name.slice(0,dot);let ext=(!isFolder&&dot>=0)?name.slice(dot+1):'';stem=stem.toUpperCase().replace(/[^A-Z0-9_\-~]/g,'').slice(0,8)||'FILE';ext=ext.toUpperCase().replace(/[^A-Z0-9_\-~]/g,'').slice(0,3);return ext?stem+'.'+ext:stem}
async function newFolder(){let name=prompt('Nome cartella (FAT 8.3, es. LIBRI)');if(!name)return;let safe=fatSafeName(name,true);try{await api('/api/mkdir?path='+enc(join(safe)),{method:'POST'});status('Cartella creata: '+safe);loadList(current)}catch(e){status('Errore: '+e.message)}}
async function renamePath(p){let name=prompt('Nuovo nome (FAT 8.3-safe)');if(!name)return;let parent=p.substring(0,p.lastIndexOf('/'))||'/';let safe=fatSafeName(name,false);let to=(parent==='/'?'/':parent+'/')+safe;try{await api('/api/rename?from='+enc(p)+'&to='+enc(to),{method:'POST'});status('Rinominato in '+safe);loadList(current)}catch(e){status('Errore: '+e.message)}}
async function deletePath(p){if(!confirm('Eliminare '+p+'?'))return;try{await api('/api/delete?path='+enc(p),{method:'POST'});status('Eliminato');loadList(current)}catch(e){status('Errore: '+e.message)}}
function handleFiles(fileList){let used=new Set(entries.map(e=>e.name));for(const file of Array.from(fileList)){let name=fatSafeName(file.name,false);let base=name,i=1;while(used.has(name)){let dot=base.lastIndexOf('.');let stem=dot<0?base:base.slice(0,dot);let ext=dot<0?'':base.slice(dot);stem=stem.slice(0,8-String(i).length)+i;name=stem+ext;i++}used.add(name);queue.push({file:file,name:name,status:'queued',progress:0,error:'',id:Math.random().toString(36).slice(2)})}renderQueue();processQueue()}
function renderQueue(){let html='';for(const item of queue){html+='<div class="qitem '+item.status+'" data-id="'+item.id+'"><span class="name">'+escapeHtml(item.file.name)+'</span>'+(item.status==='queued'?'<input type="text" value="'+escapeHtml(item.name)+'" onchange="renameQueueItem(\''+item.id+'\',this.value)">':'<span class="name">'+escapeHtml(item.name)+'</span>')+'<div class="bar"><i style="width:'+item.progress+'%"></i></div><span class="hint">'+(item.status==='error'?item.error:item.status)+'</span>'+(item.status==='queued'?'<button onclick="removeQueueItem(\''+item.id+'\')">x</button>':'')+'</div>'}document.getElementById('queue').innerHTML=html}
function renameQueueItem(id,value){let item=queue.find(q=>q.id===id);if(item)item.name=fatSafeName(value,false)}
function removeQueueItem(id){queue=queue.filter(q=>!(q.id===id&&q.status==='queued'));renderQueue()}
async function processQueue(){if(busy)return;busy=true;let any=false;while(true){let item=queue.find(q=>q.status==='queued');if(!item)break;any=true;item.status='uploading';renderQueue();try{await uploadOne(item);item.status='done';item.progress=100}catch(e){item.status='error';item.error=e.message||'errore'}renderQueue()}busy=false;if(any){status('Caricamento completato');loadList(current)}}
function uploadOne(item){return new Promise((resolve,reject)=>{let xhr=new XMLHttpRequest();let url='/api/upload?path='+enc(join(item.name))+'&code='+enc(getCode());xhr.open('POST',url);xhr.upload.onprogress=function(e){if(e.lengthComputable){item.progress=Math.round(e.loaded/e.total*100);renderQueue()}};xhr.onload=function(){if(xhr.status>=200&&xhr.status<300)resolve();else reject(new Error(xhr.responseText||('HTTP '+xhr.status)))};xhr.onerror=function(){reject(new Error('errore di rete'))};xhr.send(item.file)})}
const dropZone=document.getElementById('drop');
['dragover','dragenter'].forEach(ev=>dropZone.addEventListener(ev,e=>{e.preventDefault();dropZone.classList.add('drag')}));
['dragleave','drop'].forEach(ev=>dropZone.addEventListener(ev,e=>{e.preventDefault();dropZone.classList.remove('drag')}));
dropZone.addEventListener('drop',e=>{if(e.dataTransfer.files.length)handleFiles(e.dataTransfer.files)});
document.getElementById('file').addEventListener('change',e=>{if(e.target.files.length)handleFiles(e.target.files);e.target.value=''});
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
const dropBg=document.getElementById('dropBg');
['dragover','dragenter'].forEach(ev=>dropBg.addEventListener(ev,e=>{e.preventDefault();dropBg.classList.add('drag')}));
['dragleave','drop'].forEach(ev=>dropBg.addEventListener(ev,e=>{e.preventDefault();dropBg.classList.remove('drag')}));
dropBg.addEventListener('drop',e=>{if(e.dataTransfer.files.length)loadBackgroundFile(e.dataTransfer.files[0])});
document.getElementById('fileBg').addEventListener('change',e=>{if(e.target.files.length)loadBackgroundFile(e.target.files[0]);e.target.value=''});
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
async function ensureSleepDir(){
  try{await api('/api/list?path=/SLEEP')}
  catch(e){try{await api('/api/mkdir?path=/SLEEP',{method:'POST'})}catch(e2){}}
}
async function suggestBgName(){
  try{
    await ensureSleepDir();
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
    await ensureSleepDir();
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
    if(current==='/SLEEP')loadList(current);
    fetchSpace();
  }catch(e){bgStatus('Errore: '+e.message)}
}
// Full: one token shown big in Portrait, matching the MTG card's own
// aspect ratio. Half: the pair tile shown two-up once the device is
// rotated to Landscape (see magic_tokens::HALF_TILE_* and
// AppState::sync_orientation_for_active_route) — exactly half the 800x480
// Landscape canvas, so the pair fills the screen edge to edge.
const MT_FULL_W=420,MT_FULL_H=588,MT_HALF_W=400,MT_HALF_H=480;
let mtResults=[],mtSelected=new Set();
function mtStatus(t){document.getElementById('mtStatus').textContent=t}
function mtSanitize(s){return (s||'').replace(/[|\r\n]/g,' ').trim()}
function mtImageUris(card){return card.image_uris||(card.card_faces&&card.card_faces[0]&&card.card_faces[0].image_uris)||null}
function mtImageUrl(card){let u=mtImageUris(card);return u&&(u.png||u.large||u.normal||u.small)}
function mtThumbUrl(card){let u=mtImageUris(card);return u&&u.small}
function mtPowerToughness(card){return (card.power!=null&&card.toughness!=null)?(card.power+'/'+card.toughness):''}
async function mtSearch(){
  let q=document.getElementById('mtQuery').value.trim();
  let query='type:token'+(q?(' '+q):'');
  mtStatus('Ricerca in corso...');
  try{
    let r=await fetch('https://api.scryfall.com/cards/search?unique=prints&q='+encodeURIComponent(query));
    let data=await r.json();
    if(!r.ok)throw new Error((data&&data.details)||('HTTP '+r.status));
    mtResults=(data.data||[]).filter(c=>mtImageUrl(c));
    mtSelected.clear();
    mtRenderResults();
    mtStatus(mtResults.length+' risultati'+(data.has_more?' (prima pagina)':''));
  }catch(e){
    mtResults=[];
    mtRenderResults();
    mtStatus(q?('Nessun token trovato per "'+q+'"'):'Errore ricerca: '+e.message);
  }
}
function mtRenderResults(){
  let html='';
  for(let i=0;i<mtResults.length;i++){
    let c=mtResults[i];
    let sel=mtSelected.has(i)?' sel':'';
    html+='<div class="mt-item'+sel+'" onclick="mtToggleSelect('+i+')"><img src="'+mtThumbUrl(c)+'" alt="" loading="lazy"><div>'+escapeHtml(c.name)+'</div><div class="hint">'+escapeHtml(mtPowerToughness(c))+'</div></div>';
  }
  document.getElementById('mtResults').innerHTML=html||'<p class="hint">Nessun risultato</p>';
  document.getElementById('mtSelectedCount').textContent=mtSelected.size+' selezionati';
}
function mtToggleSelect(i){if(mtSelected.has(i))mtSelected.delete(i);else mtSelected.add(i);mtRenderResults()}
function mtHashId(str){let h=0x811c9dc5;for(let i=0;i<str.length;i++){h^=str.charCodeAt(i);h=Math.imul(h,0x01000193)}return (h>>>0).toString(16).toUpperCase().padStart(8,'0').slice(0,6)}
function mtLoadImage(url){return new Promise((resolve,reject)=>{let img=new Image();img.crossOrigin='anonymous';img.onload=()=>resolve(img);img.onerror=()=>reject(new Error('immagine non disponibile'));img.src=url})}
function mtDrawToCanvas(img,width,height){
  let canvas=document.createElement('canvas');
  canvas.width=width;canvas.height=height;
  let ctx=canvas.getContext('2d');
  let scale=Math.max(width/img.naturalWidth,height/img.naturalHeight);
  let sw=width/scale,sh=height/scale;
  let sx=(img.naturalWidth-sw)/2,sy=(img.naturalHeight-sh)/2;
  ctx.drawImage(img,sx,sy,sw,sh,0,0,width,height);
  return ctx.getImageData(0,0,width,height);
}
function mtPackBits(dithered,width,height){
  // Ink (drawn/black) = bit 1, opposite of the sleep-wallpaper BMP's raw
  // panel convention above, matching the `.TOK` / `ImageRaw<BinaryColor>`
  // format the device blits directly (see `magic_tokens::parse_token_bytes`).
  let rowBytes=Math.ceil(width/8),out=new Uint8Array(rowBytes*height);
  for(let y=0;y<height;y++){
    for(let x=0;x<width;x++){
      if(dithered[y*width+x]<128)out[y*rowBytes+(x>>3)]|=(0x80>>(x&7));
    }
  }
  return out;
}
function mtBuildTokenFile(width,height,bits){
  let header=9,buf=new Uint8Array(header+bits.length),dv=new DataView(buf.buffer);
  buf[0]=0x52;buf[1]=0x57;buf[2]=0x4D;buf[3]=0x54; // "RWMT"
  buf[4]=1;
  dv.setUint16(5,width,true);
  dv.setUint16(7,height,true);
  buf.set(bits,header);
  return buf;
}
function mtBuildTile(img,width,height){
  let imageData=mtDrawToCanvas(img,width,height);
  let lum=computeLuminance(imageData.data,width,height,0,0);
  let dithered=ditherFloydSteinberg(lum,width,height);
  return mtBuildTokenFile(width,height,mtPackBits(dithered,width,height));
}
async function mtEnsureDir(){try{await api('/api/list?path=/MAGIC')}catch(e){try{await api('/api/mkdir?path=/MAGIC',{method:'POST'})}catch(e2){}}}
async function mtReadIndex(){
  try{
    let t=await api('/api/download?path='+enc('/MAGIC/INDEX.TXT'));
    return t.split('\n').map(l=>l.trim()).filter(Boolean).map(l=>{let p=l.split('|');return {id:p[0]||'',name:p[1]||'',pt:p[2]||''}});
  }catch(e){return []}
}
function mtWriteIndex(index){
  let text=index.map(e=>e.id+'|'+e.name+'|'+e.pt).join('\n')+(index.length?'\n':'');
  return api('/api/upload?path='+enc('/MAGIC/INDEX.TXT'),{method:'POST',body:new Blob([text])});
}
async function mtUploadSelected(){
  if(mtSelected.size===0){mtStatus('Nessun token selezionato');return}
  try{
    await mtEnsureDir();
    let index=await mtReadIndex();
    let count=0;
    for(const i of mtSelected){
      let card=mtResults[i];
      let id=mtHashId(card.id);
      mtStatus('Elaborazione '+card.name+'...');
      let img=await mtLoadImage(mtImageUrl(card));
      let full=mtBuildTile(img,MT_FULL_W,MT_FULL_H);
      let half=mtBuildTile(img,MT_HALF_W,MT_HALF_H);
      await api('/api/upload?path='+enc('/MAGIC/'+id+'_F.TOK'),{method:'POST',body:new Blob([full])});
      await api('/api/upload?path='+enc('/MAGIC/'+id+'_H.TOK'),{method:'POST',body:new Blob([half])});
      let entry={id:id,name:mtSanitize(card.name),pt:mtSanitize(mtPowerToughness(card))};
      let existing=index.findIndex(e=>e.id===id);
      if(existing>=0)index[existing]=entry;else index.push(entry);
      count++;
    }
    await mtWriteIndex(index);
    mtSelected.clear();
    mtRenderResults();
    mtStatus('Caricati '+count+' token');
    mtRefreshLibrary();
    fetchSpace();
  }catch(e){mtStatus('Errore: '+e.message)}
}
async function mtRefreshLibrary(){
  let index=await mtReadIndex();
  let html='';
  for(const e of index){
    html+='<div class="mt-lib-row"><span class="name">'+escapeHtml(e.name)+' '+escapeHtml(e.pt)+'</span><button class="danger" onclick="mtRemoveToken(\''+e.id+'\')">Rimuovi</button></div>';
  }
  document.getElementById('mtLibrary').innerHTML=html||'<p class="hint">Nessun token sul dispositivo</p>';
}
async function mtRemoveToken(id){
  if(!confirm('Rimuovere questo token dal dispositivo?'))return;
  try{
    try{await api('/api/delete?path='+enc('/MAGIC/'+id+'_F.TOK'),{method:'POST'})}catch(e){}
    try{await api('/api/delete?path='+enc('/MAGIC/'+id+'_H.TOK'),{method:'POST'})}catch(e){}
    await mtWriteIndex((await mtReadIndex()).filter(e=>e.id!==id));
    mtStatus('Token rimosso');
    mtRefreshLibrary();
    fetchSpace();
  }catch(e){mtStatus('Errore rimozione: '+e.message)}
}
loadList('/');
mtRefreshLibrary();
</script></body></html>"##;

    #[derive(Debug)]
    struct SharedStatus {
        snapshot: WifiTransferSnapshot,
        last_activity: Instant,
    }

    impl SharedStatus {
        fn new(url: String, code: String) -> Self {
            Self {
                snapshot: WifiTransferSnapshot {
                    state: WifiTransferState::Ready,
                    url: Some(url),
                    code: Some(code),
                    last_action: "Portal ready".into(),
                    last_bytes: 0,
                    error: None,
                },
                last_activity: Instant::now(),
            }
        }

        fn touch(&mut self, action: impl Into<String>, bytes: usize) {
            self.snapshot.last_action = action.into();
            self.snapshot.last_bytes = bytes;
            self.snapshot.error = None;
            self.last_activity = Instant::now();
        }
    }

    /// RAII wrapper.  Constructing starts ESP-IDF's dedicated HTTP task;
    /// dropping stops that task and frees its resources.
    pub struct WifiTransferServer {
        _server: EspHttpServer<'static>,
        shared: Arc<Mutex<SharedStatus>>,
    }

    impl WifiTransferServer {
        pub fn start(ipv4: &str, code: String) -> Result<Self> {
            // Suspend Wi-Fi modem-sleep for the life of the transfer session:
            // the background default (`WIFI_PS_MAX_MODEM`, applied once
            // associated -- see network.rs) trades latency for battery, which
            // would otherwise throttle this server's upload/download
            // throughput. Restored to the background default on `Drop`.
            let status = unsafe { sys::esp_wifi_set_ps(sys::wifi_ps_type_t_WIFI_PS_NONE) };
            if status != sys::ESP_OK {
                warn!("rustmix-wave=wifi-power-save status=failed mode=none error-code={status}");
            }
            let url = format!("http://{ipv4}/");
            let shared = Arc::new(Mutex::new(SharedStatus::new(url.clone(), code.clone())));
            let mut server = EspHttpServer::new(&Configuration {
                http_port: WIFI_TRANSFER_HTTP_PORT,
                stack_size: WIFI_TRANSFER_SERVER_STACK_BYTES,
                max_open_sockets: 2,
                max_sessions: 2,
                max_uri_handlers: 10,
                session_timeout: Duration::from_secs(60),
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
                    "{{\"state\":\"{}\",\"last_action\":\"{}\",\"last_bytes\":{},\"total_bytes\":{total_bytes},\"free_bytes\":{free_bytes}}}",
                    snapshot.state.label(),
                    json_escape(&snapshot.last_action),
                    snapshot.last_bytes
                );
                request.into_ok_response()?.write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            info!("rustmix-wave=wifi-transfer-server status=ready url={url} root={WIFI_TRANSFER_ROOT} stack-bytes={WIFI_TRANSFER_SERVER_STACK_BYTES}");
            Ok(Self {
                _server: server,
                shared,
            })
        }

        #[must_use]
        pub fn snapshot(&self) -> WifiTransferSnapshot {
            lock(&self.shared).snapshot.clone()
        }

        #[must_use]
        pub fn is_expired(&self) -> bool {
            lock(&self.shared).last_activity.elapsed()
                >= Duration::from_secs(WIFI_TRANSFER_INACTIVITY_SECONDS)
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
            entries.push((name, metadata.is_dir(), metadata.len()));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut json = String::from("[");
        for (index, (name, directory, size)) in entries.into_iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            let kind = if directory { "folder" } else { "file" };
            json.push_str(&format!(
                r#"{{"name":"{}","kind":"{kind}","size":{size}}}"#,
                json_escape(&name)
            ));
        }
        json.push(']');
        Ok(json)
    }

    fn json_escape(value: &str) -> String {
        value.replace('\\', "\\\\").replace('"', "\\\"")
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
        assert!(is_protected_portal_path("APPS/CALENDAR/EVENTS.TMP"));
        assert!(is_protected_portal_path("APPS/CALENDAR/EVENTS.BAK"));
        assert!(!is_protected_portal_path("APPS/CALENDAR/EVENTS.TXT"));
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
