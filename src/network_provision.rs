//! Phone-based Wi-Fi provisioning portal.
//!
//! Explicitly activated from Settings > Network > "Configure via phone" (see
//! `src/app/screens/network.rs`). While active, `NetworkRuntime` runs
//! `Configuration::Mixed`: the device broadcasts its own hotspot (joinable by
//! scanning a single on-screen QR code) while still scanning nearby networks
//! on the STA side. Three complementary signals make the phone's OS treat
//! the hotspot as a captive network, so it opens the portal automatically
//! after joining instead of requiring a second "open this URL" QR code:
//! `NetworkRuntime::start_provisioning` advertises the portal via DHCP
//! Option 114 (RFC 8910, read directly off the DHCP lease by clients that
//! support it), and, for clients that don't, the captive-portal DNS hijack
//! in `crate::dns_captive_portal` plus this module's own wildcard HTTP
//! redirect land any of the OS's connectivity probes (or a manually opened
//! site) on the portal instead. The portal lists
//! nearby networks plus the networks already saved to `WIFI.TXT`, lets the
//! phone submit a new SSID/password (validated by a real connection attempt
//! before it is persisted) and forget a saved network. This replaces the
//! old on-device rotary-keyboard credential editor, which required typing a
//! Wi-Fi password one character at a time with Up/Down/Select.

/// The ESP-IDF HTTP server task exists only while the portal is enabled.
pub const NETWORK_PROVISION_SERVER_STACK_BYTES: usize = 24 * 1024;
/// Stop a forgotten provisioning session after five minutes without HTTP
/// traffic, so the hotspot and its open (to anyone who captured the QR code)
/// portal do not stay broadcasting indefinitely.
pub const NETWORK_PROVISION_INACTIVITY_SECONDS: u64 = 5 * 60;
/// Re-scan for nearby networks this often while the portal is open and no
/// join attempt is in flight, so the phone's list stays reasonably fresh.
pub const NETWORK_PROVISION_RESCAN_SECONDS: u64 = 12;

/// User request handed from the hardware-independent UI into `main.rs`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkProvisionUiRequest {
    Start,
    Stop,
}

/// Compact UI-facing lifecycle state. Never contains a Wi-Fi password.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NetworkProvisionState {
    #[default]
    Off,
    Starting,
    Ready,
    Failed,
}

impl NetworkProvisionState {
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

/// Outcome of the most recent phone-submitted network, as reported back to
/// the on-device screen. `Testing` is shown while `NetworkRuntime` is
/// attempting the real connection the portal requires before persisting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinAttemptState {
    Idle,
    Testing { ssid: String },
    Succeeded { ssid: String },
    Failed { ssid: String, error: String },
}

impl Default for JoinAttemptState {
    fn default() -> Self {
        Self::Idle
    }
}

/// Small main-loop snapshot updated as the portal starts, stops, or a phone
/// request completes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkProvisionSnapshot {
    pub state: NetworkProvisionState,
    pub ap_ssid: Option<String>,
    pub ap_password: Option<String>,
    pub portal_url: Option<String>,
    pub join: JoinAttemptState,
    pub saved_network_count: usize,
    pub error: Option<String>,
}

impl Default for NetworkProvisionSnapshot {
    fn default() -> Self {
        Self {
            state: NetworkProvisionState::Off,
            ap_ssid: None,
            ap_password: None,
            portal_url: None,
            join: JoinAttemptState::Idle,
            saved_network_count: 0,
            error: None,
        }
    }
}

impl NetworkProvisionSnapshot {
    #[must_use]
    pub fn starting() -> Self {
        Self {
            state: NetworkProvisionState::Starting,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            state: NetworkProvisionState::Failed,
            error: Some(error.into()),
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(
            self.state,
            NetworkProvisionState::Starting | NetworkProvisionState::Ready
        )
    }
}

#[cfg(target_os = "espidf")]
pub mod espidf {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use anyhow::{anyhow, Context, Result};
    use embedded_svc::{http::Method, io::Write as _};
    use esp_idf_svc::http::server::{Configuration, EspHttpServer};
    use log::info;

    use crate::{
        dns_captive_portal::espidf::CaptivePortalDns, network_scan::WifiScanEntry,
        wifi_transfer::query_value,
    };

    use super::{
        JoinAttemptState, NetworkProvisionSnapshot, NetworkProvisionState,
        NETWORK_PROVISION_INACTIVITY_SECONDS, NETWORK_PROVISION_SERVER_STACK_BYTES,
    };

    const PORTAL_HTML: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Rustmix-Wave Wi-Fi</title><style>
*{box-sizing:border-box}
:root{--bg:#f5f6f8;--card:#fff;--text:#1a1d23;--muted:#6b7280;--border:#e2e5ea;--accent:#2563eb;--accent-dark:#1d4ed8;--danger:#dc2626;--radius:10px}
@media (prefers-color-scheme:dark){:root{--bg:#14161a;--card:#1c1f26;--text:#e8eaed;--muted:#9aa3b2;--border:#2a2e37;--accent:#3b82f6;--accent-dark:#60a5fa;--danger:#f87171}}
body{margin:0;background:var(--bg);color:var(--text);font-family:-apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif}
.wrap{max-width:640px;margin:0 auto;padding:1rem}
h1{font-size:1.2rem;margin:0 0 1rem}
.card{background:var(--card);border:1px solid var(--border);border-radius:var(--radius);padding:1rem;margin-bottom:1rem}
h2{margin:0 0 .6rem;font-size:1rem}
input,button{font:inherit}
input[type=password],input[type=text]{width:100%;background:transparent;border:1px solid var(--border);border-radius:8px;padding:.5rem .7rem;color:var(--text);margin:.3rem 0}
button{border:1px solid var(--border);background:var(--card);color:var(--text);border-radius:8px;padding:.5rem .9rem;cursor:pointer}
button.primary{background:var(--accent);border-color:var(--accent);color:#fff}
button.danger{color:var(--danger);border-color:var(--danger)}
ul{list-style:none;margin:0;padding:0}
li{display:flex;align-items:center;gap:.6rem;padding:.5rem 0;border-bottom:1px solid var(--border)}
li:last-child{border-bottom:none}
.name{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rssi{color:var(--muted);font-size:.85rem}
.hint{font-size:.85rem;color:var(--muted)}
#status{font-size:.9rem;white-space:pre-wrap}
</style></head><body><div class="wrap">
<h1>Rustmix-Wave &middot; Wi-Fi setup</h1>
<div class="card">
<h2>Nearby networks</h2>
<ul id="scan"><li class="hint">Scanning...</li></ul>
</div>
<div class="card">
<h2>Saved networks</h2>
<ul id="saved"><li class="hint">Loading...</li></ul>
</div>
<div class="card" id="joinCard" style="display:none">
<h2 id="joinTitle">Add network</h2>
<input id="joinSsid" type="text" placeholder="Network name">
<input id="joinPassword" type="password" placeholder="Password (leave blank if open)">
<p><button class="primary" onclick="submitJoin()">Connect and save</button> <button onclick="closeJoin()">Cancel</button></p>
<pre id="status"></pre>
</div>
<div class="card" id="forgetCard" style="display:none">
<h2>Forget network?</h2>
<p id="forgetText"></p>
<p><button class="danger" onclick="confirmForget()">Yes, forget it</button> <button onclick="cancelForget()">Cancel</button></p>
</div>
</div>
<script>
function openJoin(ssid,locked){
  cancelForget();
  var card=document.getElementById('joinCard');
  card.style.display='block';
  var field=document.getElementById('joinSsid');
  field.value=ssid||'';
  field.readOnly=!!locked;
  document.getElementById('joinPassword').value='';
  document.getElementById('joinTitle').textContent=ssid?('Connect to '+ssid):'Add network';
  document.getElementById('status').textContent='';
  // The join form can land below the fold once the scan/saved lists above
  // it are long enough, especially inside the small captive-portal
  // mini-browser phones auto-open; without this a tap on "Add" looks like
  // it did nothing because the newly revealed form is off-screen.
  card.scrollIntoView({behavior:'smooth',block:'start'});
}
function closeJoin(){document.getElementById('joinCard').style.display='none'}
async function loadScan(){
  try{
    var r=await fetch('/api/scan');var list=await r.json();
    var html='';
    for(var i=0;i<list.length;i++){
      html+='<li><span class="name">'+escapeHtml(list[i].ssid)+'</span><span class="rssi">'+list[i].rssi+' dBm</span><button onclick="openJoin('+JSON.stringify(list[i].ssid)+',true)">Add</button></li>';
    }
    document.getElementById('scan').innerHTML=html||'<li class="hint">No networks found yet</li>';
  }catch(e){}
}
async function loadSaved(){
  try{
    var r=await fetch('/api/networks');var list=await r.json();
    var html='';
    for(var i=0;i<list.length;i++){
      var ssid=list[i].ssid;
      html+='<li><span class="name">'+escapeHtml(ssid)+'</span><button onclick="openJoin('+JSON.stringify(ssid)+',true)">Change password</button><button class="danger" onclick="forget('+JSON.stringify(ssid)+')">Forget</button></li>';
    }
    html+='<li><button onclick="openJoin(\'\',false)">+ Add network manually</button></li>';
    document.getElementById('saved').innerHTML=html;
  }catch(e){}
}
var forgetSsid=null;
function forget(ssid){
  // Native confirm()/alert() dialogs are unreliable (often suppressed
  // outright) inside the mini-browser phones use to open a captive-portal
  // page, so this is its own always-visible confirmation card instead of a
  // popup or a subtle same-button re-tap that is easy to miss.
  closeJoin();
  forgetSsid=ssid;
  document.getElementById('forgetText').textContent='Forget "'+ssid+'"? This cannot be undone.';
  var card=document.getElementById('forgetCard');
  card.style.display='block';
  card.scrollIntoView({behavior:'smooth',block:'center'});
}
function cancelForget(){
  forgetSsid=null;
  document.getElementById('forgetCard').style.display='none';
}
async function confirmForget(){
  var ssid=forgetSsid;
  if(!ssid)return;
  cancelForget();
  await fetch('/api/networks/delete?ssid='+encodeURIComponent(ssid),{method:'POST'});
  loadSaved();
}
async function submitJoin(){
  var ssid=document.getElementById('joinSsid').value.trim();
  var password=document.getElementById('joinPassword').value;
  if(!ssid){document.getElementById('status').textContent='Network name is required.';return}
  document.getElementById('status').textContent='Connecting...';
  await fetch('/api/networks?ssid='+encodeURIComponent(ssid)+'&password='+encodeURIComponent(password),{method:'POST'});
  pollStatus();
}
async function pollStatus(){
  try{
    var r=await fetch('/api/status');var s=await r.json();
    if(s.state==='testing'){document.getElementById('status').textContent='Connecting...';setTimeout(pollStatus,1000);return}
    if(s.state==='connected'){document.getElementById('status').textContent='Connected and saved.';loadSaved();setTimeout(closeJoin,1500);return}
    if(s.state==='failed'){document.getElementById('status').textContent='Failed: '+(s.error||'could not connect');return}
  }catch(e){document.getElementById('status').textContent='Network error.'}
}
function escapeHtml(s){return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')}
loadScan();loadSaved();
setInterval(loadScan,5000);
</script></body></html>"##;

    /// Outcome of a phone-submitted network, drained by `main.rs` once per
    /// main-loop iteration and applied to `NetworkRuntime`/`WIFI.TXT`.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct PendingJoinRequest {
        pub ssid: String,
        pub password: String,
    }

    #[derive(Debug)]
    struct SharedState {
        scan: Vec<WifiScanEntry>,
        saved_ssids: Vec<String>,
        pending_join: Option<PendingJoinRequest>,
        pending_delete: Option<String>,
        join: JoinAttemptState,
        last_activity: Instant,
    }

    impl SharedState {
        fn new() -> Self {
            Self {
                scan: Vec::new(),
                saved_ssids: Vec::new(),
                pending_join: None,
                pending_delete: None,
                join: JoinAttemptState::Idle,
                last_activity: Instant::now(),
            }
        }
    }

    /// RAII wrapper. Constructing starts ESP-IDF's dedicated HTTP task;
    /// dropping stops that task and frees its resources, and drops the
    /// hotspot along with it (the caller is expected to call
    /// `NetworkRuntime::stop_provisioning` at the same time).
    pub struct NetworkProvisionServer {
        _server: EspHttpServer<'static>,
        _dns: CaptivePortalDns,
        shared: Arc<Mutex<SharedState>>,
        ap_ssid: String,
        ap_password: String,
        portal_url: String,
    }

    impl NetworkProvisionServer {
        pub fn start(portal_ip: &str) -> Result<Self> {
            let answer_ip = portal_ip
                .parse::<std::net::Ipv4Addr>()
                .with_context(|| format!("portal IP is not a valid IPv4 address: {portal_ip}"))?
                .octets();
            // Answer every DNS query on the hotspot with our own address, so
            // the phone's captive-portal probe (whatever hostname it picks)
            // lands on this HTTP server and the OS offers to open it.
            let dns = CaptivePortalDns::start(answer_ip)?;

            let shared = Arc::new(Mutex::new(SharedState::new()));
            let portal_url = format!("http://{portal_ip}/");
            let mut server = EspHttpServer::new(&Configuration {
                http_port: 80,
                stack_size: NETWORK_PROVISION_SERVER_STACK_BYTES,
                max_open_sockets: 4,
                max_sessions: 4,
                max_uri_handlers: 8,
                session_timeout: Duration::from_secs(60),
                // Lets the catch-all handler below use the glob pattern "*",
                // matched only after the specific handlers registered ahead
                // of it, instead of requiring an exact URI match.
                uri_match_wildcard: true,
                ..Default::default()
            })?;

            server.fn_handler("/", Method::Get, move |request| {
                info!("rustmix-wave=network-provision-server status=portal-page-served");
                request
                    .into_response(200, Some("OK"), &[("Cache-Control", "no-store")])?
                    .write_all(PORTAL_HTML.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let scan_shared = Arc::clone(&shared);
            server.fn_handler("/api/scan", Method::Get, move |request| {
                touch(&scan_shared);
                let body = scan_json(&lock(&scan_shared).scan);
                request
                    .into_response(200, Some("OK"), &[("Cache-Control", "no-store")])?
                    .write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let networks_shared = Arc::clone(&shared);
            server.fn_handler("/api/networks", Method::Get, move |request| {
                touch(&networks_shared);
                let body = saved_json(&lock(&networks_shared).saved_ssids);
                request
                    .into_response(200, Some("OK"), &[("Cache-Control", "no-store")])?
                    .write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            let join_shared = Arc::clone(&shared);
            server.fn_handler("/api/networks", Method::Post, move |mut request| {
                touch(&join_shared);
                // Drain any request body (none expected; credentials travel
                // as query parameters like the rest of this tiny API) so the
                // connection can be reused.
                let mut discard = [0_u8; 64];
                while request.read(&mut discard)? > 0 {}
                let ssid = required_query(request.uri(), "ssid")?;
                let password = query_value(request.uri(), "password").unwrap_or_default();
                {
                    let mut state = lock(&join_shared);
                    state.pending_join = Some(PendingJoinRequest {
                        ssid: ssid.clone(),
                        password,
                    });
                    state.join = JoinAttemptState::Testing { ssid };
                }
                request.into_ok_response()?.write_all(b"testing")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let delete_shared = Arc::clone(&shared);
            server.fn_handler("/api/networks/delete", Method::Post, move |request| {
                touch(&delete_shared);
                let ssid = required_query(request.uri(), "ssid")?;
                lock(&delete_shared).pending_delete = Some(ssid);
                request.into_ok_response()?.write_all(b"deleted")?;
                Ok::<(), anyhow::Error>(())
            })?;

            let status_shared = Arc::clone(&shared);
            server.fn_handler("/api/status", Method::Get, move |request| {
                let join = lock(&status_shared).join.clone();
                let body = status_json(&join);
                request
                    .into_response(200, Some("OK"), &[("Cache-Control", "no-store")])?
                    .write_all(body.as_bytes())?;
                Ok::<(), anyhow::Error>(())
            })?;

            // Catch-all: registered last (and thus tried last, after the
            // exact-path handlers above) via `uri_match_wildcard`. Any path a
            // phone's OS probes to decide whether this hotspot is a captive
            // portal (Android `/generate_204`, Apple `/hotspot-detect.html`,
            // Windows `/connecttest.txt` and `/redirect`, ...) lands here and
            // gets redirected to the portal itself, which both fails those
            // probes' "internet already works" check and, on Windows, is the
            // literal 3xx signal it looks for to auto-open the sign-in
            // browser. A body is included because iOS's captive-portal
            // detector specifically requires one -- a redirect with an empty
            // body is not enough for it to recognize a portal.
            let redirect_url = portal_url.clone();
            server.fn_handler("*", Method::Get, move |request| {
                info!(
                    "rustmix-wave=network-provision-server status=captive-probe uri={}",
                    request.uri()
                );
                request
                    .into_response(
                        302,
                        Some("Found"),
                        &[("Location", redirect_url.as_str()), ("Cache-Control", "no-store")],
                    )?
                    .write_all(b"Redirecting to the Wi-Fi setup portal.")?;
                Ok::<(), anyhow::Error>(())
            })?;

            info!(
                "rustmix-wave=network-provision-server status=ready ip={portal_ip} stack-bytes={NETWORK_PROVISION_SERVER_STACK_BYTES}"
            );
            Ok(Self {
                _server: server,
                _dns: dns,
                shared,
                ap_ssid: String::new(),
                ap_password: String::new(),
                portal_url,
            })
        }

        /// Attach the freshly generated AP credentials so `snapshot()` can
        /// report them for the on-screen QR codes. Split from `start` since
        /// the AP credentials come from `NetworkRuntime::start_provisioning`,
        /// which the caller invokes around the same time.
        pub fn set_ap_credentials(&mut self, ap_ssid: String, ap_password: String) {
            self.ap_ssid = ap_ssid;
            self.ap_password = ap_password;
        }

        #[must_use]
        pub fn snapshot(&self, saved_network_count: usize) -> NetworkProvisionSnapshot {
            let join = lock(&self.shared).join.clone();
            NetworkProvisionSnapshot {
                state: NetworkProvisionState::Ready,
                ap_ssid: Some(self.ap_ssid.clone()),
                ap_password: Some(self.ap_password.clone()),
                portal_url: Some(self.portal_url.clone()),
                join,
                saved_network_count,
                error: None,
            }
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
            lock(&self.shared).join = JoinAttemptState::Succeeded { ssid };
        }

        pub fn record_join_failed(&self, ssid: String, error: String) {
            lock(&self.shared).join = JoinAttemptState::Failed { ssid, error };
        }

        #[must_use]
        pub fn is_expired(&self) -> bool {
            lock(&self.shared).last_activity.elapsed()
                >= Duration::from_secs(NETWORK_PROVISION_INACTIVITY_SECONDS)
        }
    }

    impl Drop for NetworkProvisionServer {
        fn drop(&mut self) {
            info!("rustmix-wave=network-provision-server status=stopped");
        }
    }

    fn lock(shared: &Arc<Mutex<SharedState>>) -> std::sync::MutexGuard<'_, SharedState> {
        shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn touch(shared: &Arc<Mutex<SharedState>>) {
        lock(shared).last_activity = Instant::now();
    }

    fn required_query(uri: &str, key: &str) -> Result<String> {
        query_value(uri, key).ok_or_else(|| anyhow!("missing query parameter: {key}"))
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

    fn status_json(join: &JoinAttemptState) -> String {
        match join {
            JoinAttemptState::Idle => r#"{"state":"idle"}"#.into(),
            JoinAttemptState::Testing { .. } => r#"{"state":"testing"}"#.into(),
            JoinAttemptState::Succeeded { .. } => r#"{"state":"connected"}"#.into(),
            JoinAttemptState::Failed { error, .. } => {
                format!(r#"{{"state":"failed","error":"{}"}}"#, json_escape(error))
            }
        }
    }

    fn json_escape(value: &str) -> String {
        value.replace('\\', "\\\\").replace('"', "\\\"")
    }
}
