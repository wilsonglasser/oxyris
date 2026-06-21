//! Mobile-takeover companion server.
//!
//! When the user enables it, this stands up a LAN-bound HTTP + WebSocket server
//! that mirrors a **pure** session's `claude` PTY to a phone. The phone takes
//! over the PTY: the desktop view freezes (see `PtySupervisor::acquire_takeover`)
//! while the phone drives input/resize, and control returns to the desktop when
//! the phone releases or disconnects.
//!
//! Scope is deliberately narrow — pure (PTY) sessions only. Structured sessions
//! have a different (event-sourced) shape and are out of scope for this first
//! cut.
//!
//! Security: the server binds the LAN, so every API/WS request must carry an
//! unguessable per-run token (the QR encodes it in the URL fragment). The token
//! is ephemeral — a fresh one is minted on each `start` and dies on `stop`.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_server::tls_rustls::RustlsConfig;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use oxyris_core::{AggregateId, Environment};
use oxyris_ipc::ops::op_name;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tower_http::cors::CorsLayer;

use crate::domain::session::SessionKind;
use crate::infra::agent_pool::AgentPool;
use crate::infra::projections::Projections;
use crate::infra::pty::PtySupervisor;

/// Cap on a single mobile upload (mirrors the desktop attachment cap).
const MAX_UPLOAD: usize = 10 * 1024 * 1024;

/// Public metadata about a running (or stopped) mobile server. Returned by the
/// toggle Tauri commands so the desktop can render the pairing QR + URL.
#[derive(Debug, Clone, Serialize)]
pub struct MobileInfo {
    pub running: bool,
    pub url: String,
    pub token: String,
    pub port: u16,
    /// Inline SVG of the pairing QR (encodes `url`). Empty if QR generation
    /// failed — the URL is still shown as a fallback.
    pub qr_svg: String,
}

impl MobileInfo {
    /// The "not running" sentinel returned by `status`/`stop`.
    pub fn stopped() -> Self {
        Self {
            running: false,
            url: String::new(),
            token: String::new(),
            port: 0,
            qr_svg: String::new(),
        }
    }
}

/// A live mobile server: the axum task plus the info needed to pair a phone.
/// Held by `AppState`; dropping or [`MobileServer::stop`] tears the server down.
pub struct MobileServer {
    info: MobileInfo,
    handle: JoinHandle<()>,
}

impl MobileServer {
    pub fn info(&self) -> MobileInfo {
        self.info.clone()
    }

    pub fn stop(self) {
        self.handle.abort();
    }
}

#[derive(Clone)]
struct AppCtx {
    pty: Arc<PtySupervisor>,
    projections: Arc<Projections>,
    agent_pool: Arc<AgentPool>,
    data_dir: PathBuf,
    token: Arc<str>,
}

/// Spawn the LAN server over HTTPS. Binds an OS-chosen port on `0.0.0.0`, mints
/// a fresh pairing token, generates a self-signed cert (a secure context is
/// required for the phone's mic), and returns the URL/QR a phone scans.
pub async fn start(
    pty: Arc<PtySupervisor>,
    projections: Arc<Projections>,
    agent_pool: Arc<AgentPool>,
    data_dir: PathBuf,
) -> anyhow::Result<MobileServer> {
    // rustls 0.23 needs a process-wide crypto provider. Idempotent — ignore the
    // error if another subsystem already installed one.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let token = gen_token();
    // std listener bound to :0 so we can read back the OS-chosen port, then hand
    // it to axum-server for the TLS accept loop.
    let std_listener = std::net::TcpListener::bind("0.0.0.0:0")?;
    std_listener.set_nonblocking(true)?;
    let port = std_listener.local_addr()?.port();
    let ip = local_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let url = format!("https://{ip}:{port}/#t={token}");
    let qr_svg = render_qr(&url);

    // Self-signed cert valid for the LAN IP + localhost. rcgen turns IP-shaped
    // SAN entries into IP SANs, so the browser matches `https://<ip>`.
    let cert = rcgen::generate_simple_self_signed(vec![ip.to_string(), "localhost".to_owned()])
        .map_err(|e| anyhow::anyhow!("self-signed cert: {e}"))?;
    let config = RustlsConfig::from_pem(
        cert.cert.pem().into_bytes(),
        cert.key_pair.serialize_pem().into_bytes(),
    )
    .await?;

    let ctx = AppCtx {
        pty,
        projections,
        agent_pool,
        data_dir,
        token: Arc::from(token.as_str()),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/sessions", get(list_sessions))
        .route("/api/upload", post(upload))
        .route("/ws", get(ws_handler))
        // The phone fetches from a different origin (its own page URL) than it
        // posts to — permissive CORS keeps the static-shell-from-CDN model
        // working. Auth is by bearer token, not origin.
        .layer(CorsLayer::permissive())
        .layer(DefaultBodyLimit::max(MAX_UPLOAD))
        .with_state(ctx);

    let handle = tokio::spawn(async move {
        if let Err(e) = axum_server::from_tcp_rustls(std_listener, config)
            .serve(app.into_make_service())
            .await
        {
            tracing::warn!(error = %e, "mobile: server exited");
        }
    });

    tracing::info!(%url, port, "mobile: takeover server ready (https)");
    Ok(MobileServer {
        info: MobileInfo {
            running: true,
            url,
            token,
            port,
            qr_svg,
        },
        handle,
    })
}

// ----- handlers -------------------------------------------------------------

/// Bearer-token check for REST routes. The token rides `Authorization: Bearer`.
fn authed(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|got| got == token)
        .unwrap_or(false)
}

#[derive(Serialize)]
struct MobileSession {
    id: String,
    title: String,
    project: String,
    model: String,
    status: String,
    /// Whether a live `claude` PTY exists for this session right now — only live
    /// ones are connectable.
    live: bool,
}

/// List pure (PTY) sessions across every project. Structured sessions are
/// filtered out — they're not mirrorable through this terminal bridge.
async fn list_sessions(State(ctx): State<AppCtx>, headers: HeaderMap) -> Response {
    if !authed(&headers, &ctx.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut out: Vec<MobileSession> = Vec::new();
    let projects = ctx.projections.list_projects().unwrap_or_default();
    for p in &projects {
        let sessions = ctx.projections.list_sessions(p.id).unwrap_or_default();
        for s in sessions {
            let Ok(Some(snap)) = ctx.projections.get_session(s.id) else {
                continue;
            };
            if snap.data.kind != SessionKind::Pure {
                continue;
            }
            out.push(MobileSession {
                id: s.id.to_string(),
                title: snap.data.title.clone().unwrap_or_default(),
                project: p.name.clone(),
                model: snap.data.model.clone(),
                status: format!("{:?}", snap.data.status).to_lowercase(),
                live: ctx.pty.claude_terminal_for_session(s.id).is_some(),
            });
        }
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
struct UploadQuery {
    /// Session id — doubles as the attachment bucket, matching the desktop.
    session: String,
    /// Original filename from the phone, used to derive the extension / a
    /// readable name. Sanitized server-side.
    name: String,
    t: String,
}

#[derive(Serialize)]
struct UploadResult {
    /// Path as `claude` sees it — a Windows path for local projects, a POSIX
    /// path inside the distro for WSL ones. The phone injects `@<path>`.
    path: String,
    is_image: bool,
}

/// Accept a file uploaded from the phone, store it where the session's `claude`
/// can read it, and return the path to `@`-reference. Body is the raw bytes.
/// Mirrors `attachment_save`'s routing (local store vs WSL agent) but accepts any
/// file type — the phone has no local disk the desktop can reach, so unlike the
/// desktop file-picker (which references files in place) it must upload bytes.
async fn upload(State(ctx): State<AppCtx>, Query(q): Query<UploadQuery>, body: Bytes) -> Response {
    if q.t.as_str() != &*ctx.token {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if body.len() > MAX_UPLOAD {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let Some(bucket) = sanitize_bucket(&q.session) else {
        return (StatusCode::BAD_REQUEST, "bad session id").into_response();
    };
    let name = sanitize_name(&q.name);
    let is_image = is_image_name(&name);
    // Unique on-disk name so two uploads of `photo.jpg` don't collide.
    let stored = format!("{}-{}", uuid::Uuid::now_v7(), name);

    match resolve_env(&ctx.projections, &bucket) {
        Some(Environment::Wsl { distro }) => {
            let Some(home) = agent_home(&ctx.agent_pool, &distro).await else {
                return (StatusCode::BAD_GATEWAY, "agent unreachable").into_response();
            };
            let path = format!("{home}/.oxyris/attachments/{bucket}/{stored}");
            let b64 = base64::engine::general_purpose::STANDARD.encode(&body);
            if ctx
                .agent_pool
                .call(
                    &distro,
                    op_name::FS_WRITE_BYTES,
                    serde_json::json!({ "path": path, "bytes_b64": b64 }),
                )
                .await
                .is_err()
            {
                return (StatusCode::BAD_GATEWAY, "agent write failed").into_response();
            }
            Json(UploadResult { path, is_image }).into_response()
        }
        _ => {
            let dir = ctx.data_dir.join("attachments").join(&bucket);
            if std::fs::create_dir_all(&dir).is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, "mkdir failed").into_response();
            }
            let path = dir.join(&stored);
            if std::fs::write(&path, &body).is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, "write failed").into_response();
            }
            Json(UploadResult {
                path: path.display().to_string(),
                is_image,
            })
            .into_response()
        }
    }
}

#[derive(Deserialize)]
struct WsQuery {
    session: String,
    t: String,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(ctx): State<AppCtx>,
) -> Response {
    if q.t.as_str() != &*ctx.token {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(uuid) = uuid::Uuid::parse_str(&q.session) else {
        return (StatusCode::BAD_REQUEST, "bad session id").into_response();
    };
    let session_id = AggregateId(uuid);
    let Some(term_id) = ctx.pty.claude_terminal_for_session(session_id) else {
        return (StatusCode::NOT_FOUND, "no live pure terminal for session").into_response();
    };
    ws.on_upgrade(move |socket| handle_socket(socket, ctx, term_id))
}

/// Inbound control frames from the phone.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    /// Raw keystroke bytes to write to the PTY.
    Data { data: String },
    /// The phone's terminal dimensions — it owns the PTY size during takeover.
    Resize { cols: u16, rows: u16 },
    /// Explicit hand-back of control to the desktop.
    Release,
}

async fn handle_socket(socket: WebSocket, ctx: AppCtx, term_id: String) {
    // Take control. If someone else already holds it, tell the phone and bail
    // without disturbing the existing takeover.
    if ctx.pty.acquire_takeover(&term_id).is_err() {
        let mut s = socket;
        let _ = s
            .send(Message::Text(
                r#"{"type":"error","message":"terminal busy"}"#.into(),
            ))
            .await;
        let _ = s.close().await;
        return;
    }

    let (mut tx, mut rx) = socket.split();

    // Backfill: replay everything the PTY has emitted so far, then dedup live
    // chunks against this sequence — same replay-then-live dance the desktop
    // does on attach.
    let mut last_seq = match ctx.pty.attach_snapshot(&term_id) {
        Ok(snap) => {
            let _ = tx
                .send(Message::Text(snapshot_json(&snap.data, snap.last_seq)))
                .await;
            snap.last_seq
        }
        Err(_) => 0,
    };

    let mut sub = ctx.pty.subscribe_output();

    loop {
        tokio::select! {
            // Prefer draining inbound control frames promptly so a release/close
            // isn't starved by a heavy output flood.
            biased;
            inbound = rx.next() => {
                match inbound {
                    Some(Ok(Message::Text(txt))) => {
                        match serde_json::from_str::<ClientMsg>(&txt) {
                            Ok(ClientMsg::Data { data }) => {
                                let _ = ctx.pty.write_mobile(&term_id, &data);
                            }
                            Ok(ClientMsg::Resize { cols, rows }) => {
                                let _ = ctx.pty.resize_mobile(&term_id, cols, rows);
                            }
                            Ok(ClientMsg::Release) => break,
                            Err(_) => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            out = sub.recv() => {
                match out {
                    Ok(o) if o.terminal_id == term_id => {
                        if o.seq <= last_seq { continue; }
                        last_seq = o.seq;
                        if tx.send(Message::Text(output_json(o.seq, &o.data))).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {} // chunk for a different terminal
                    Err(RecvError::Lagged(_)) => {
                        // The phone fell behind the broadcast. Resync from the
                        // replay buffer instead of dropping bytes silently.
                        if let Ok(snap) = ctx.pty.attach_snapshot(&term_id) {
                            last_seq = snap.last_seq;
                            if tx.send(Message::Text(snapshot_json(&snap.data, snap.last_seq))).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }

    // Hand control back to the desktop no matter how we exited.
    ctx.pty.release_takeover(&term_id);
}

fn snapshot_json(data: &str, last_seq: u64) -> String {
    serde_json::json!({ "type": "snapshot", "data": data, "lastSeq": last_seq }).to_string()
}

fn output_json(seq: u64, data: &str) -> String {
    serde_json::json!({ "type": "output", "seq": seq, "data": data }).to_string()
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

// ----- helpers --------------------------------------------------------------

/// Bucket id must be filesystem-safe (it's a session UUID in practice).
fn sanitize_bucket(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.len() > 64 {
        return None;
    }
    raw.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        .then(|| raw.to_owned())
}

/// Strip an uploaded filename down to a safe base name (no path components, no
/// shell-hostile chars), capped in length. Falls back to `file` when empty.
fn sanitize_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(80)
        .collect();
    if cleaned.is_empty() {
        "file".to_owned()
    } else {
        cleaned
    }
}

fn is_image_name(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg")
    )
}

/// Resolve the project environment for a bucket (session id). Mirrors
/// `attachments::resolve_environment` but takes the projections directly so the
/// mobile module needs no `AppState`.
fn resolve_env(projections: &Projections, bucket: &str) -> Option<Environment> {
    let session_id = AggregateId(uuid::Uuid::parse_str(bucket).ok()?);
    let snap = projections.get_session(session_id).ok()??;
    let projects = projections.list_projects().ok()?;
    projects
        .into_iter()
        .find(|p| p.id == snap.data.project_id)
        .map(|p| p.environment)
}

/// The distro's home dir, so uploads land where the agent can write and `claude`
/// can read (`<home>/.oxyris/...`). `None` on any agent failure.
async fn agent_home(agent: &AgentPool, distro: &str) -> Option<String> {
    let info = agent
        .call(distro, op_name::SYSTEM_INFO, serde_json::json!({}))
        .await
        .ok()?;
    info.get("home")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty())
}

/// 24 bytes of OS randomness, URL-safe base64. ~192 bits — not guessable on a
/// LAN within a session's lifetime.
fn gen_token() -> String {
    let mut buf = [0u8; 24];
    getrandom::getrandom(&mut buf).expect("os rng");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Best-effort LAN IP discovery: open a UDP socket "to" a public address and
/// read back the local address the OS would route through. No packets are sent
/// (UDP connect just sets the default peer), so it works offline too.
fn local_ip() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

fn render_qr(data: &str) -> String {
    use qrcode::QrCode;
    use qrcode::render::svg;
    match QrCode::new(data.as_bytes()) {
        Ok(code) => code
            .render::<svg::Color>()
            .min_dimensions(240, 240)
            .quiet_zone(true)
            .build(),
        Err(_) => String::new(),
    }
}

/// Self-contained mobile UI. Reads the pairing token from the URL fragment
/// (`#t=...`), lists pure sessions, and opens a takeover terminal over the WS.
/// xterm.js is pulled from a CDN — the phone is typically on a wifi with
/// internet; vendoring the bundle locally is a follow-up.
const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no" />
<title>Oxyris Mobile</title>
<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.min.css" />
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  html, body { margin: 0; height: 100%; background: #0b0d12; color: #e6e8ee;
    font-family: ui-sans-serif, system-ui, -apple-system, sans-serif; }
  #app { display: flex; flex-direction: column; height: 100%; }
  header { display: flex; align-items: center; gap: 8px; padding: 10px 12px;
    background: #11141b; border-bottom: 1px solid #1d212b; }
  header .title { font-weight: 600; font-size: 15px; }
  header .spacer { flex: 1; }
  button { background: #2a2f3a; color: #e6e8ee; border: 1px solid #3a4150;
    border-radius: 8px; padding: 8px 12px; font-size: 14px; }
  button:active { background: #353c49; }
  button.danger { background: #3a1f24; border-color: #5a2a31; color: #ffb4b4; }
  #list { padding: 12px; overflow: auto; }
  .session { background: #11141b; border: 1px solid #1d212b; border-radius: 12px;
    padding: 14px; margin-bottom: 10px; }
  .session h3 { margin: 0 0 4px; font-size: 15px; }
  .session .meta { font-size: 12px; color: #8b93a7; }
  .session.dead { opacity: .45; }
  .dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%;
    background: #3ddc84; margin-right: 6px; vertical-align: middle; }
  .dot.off { background: #5a6273; }
  #term { flex: 1; min-height: 0; padding: 4px; }
  #bar { display: flex; gap: 8px; align-items: center; padding: 8px 10px;
    background: #11141b; border-top: 1px solid #1d212b; }
  #bar input.msg { flex: 1; min-width: 0; background: #0b0d12; color: #e6e8ee;
    border: 1px solid #2a2f3a; border-radius: 8px; padding: 9px 10px; font-size: 14px; }
  #bar .iconbtn { width: 40px; height: 40px; display: flex; align-items: center;
    justify-content: center; padding: 0; }
  #bar .iconbtn.rec { background: #3a1f24; border-color: #5a2a31; color: #ff8a8a; }
  #status { font-size: 11px; color: #8b93a7; padding: 2px 10px; min-height: 16px; }
  .hidden { display: none !important; }
  .empty { color: #8b93a7; text-align: center; margin-top: 40px; }
</style>
</head>
<body>
<div id="app">
  <header>
    <span class="title">Oxyris</span>
    <span id="sub" class="meta" style="font-size:12px;color:#8b93a7"></span>
    <span class="spacer"></span>
    <button id="refresh" class="hidden">↻</button>
    <button id="leave" class="danger hidden">Devolver</button>
  </header>
  <div id="list"></div>
  <div id="term" class="hidden"></div>
  <div id="status" class="hidden"></div>
  <div id="bar" class="hidden">
    <button id="mic" class="iconbtn" title="Falar">🎤</button>
    <button id="attach" class="iconbtn" title="Anexar arquivo">📎</button>
    <input id="msg" class="msg" type="text" placeholder="Mensagem (ou digite no terminal)…" />
    <button id="send" class="iconbtn" title="Enviar">➤</button>
    <input id="file" type="file" class="hidden" />
  </div>
</div>
<script src="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.min.js"></script>
<script src="https://cdn.jsdelivr.net/npm/@xterm/addon-fit@0.10.0/lib/addon-fit.min.js"></script>
<script>
const token = new URLSearchParams(location.hash.slice(1)).get('t') || '';
const listEl = document.getElementById('list');
const termEl = document.getElementById('term');
const leaveBtn = document.getElementById('leave');
const refreshBtn = document.getElementById('refresh');
const subEl = document.getElementById('sub');
const barEl = document.getElementById('bar');
const statusEl = document.getElementById('status');
const micBtn = document.getElementById('mic');
const attachBtn = document.getElementById('attach');
const sendBtn = document.getElementById('send');
const msgEl = document.getElementById('msg');
const fileEl = document.getElementById('file');
let ws = null, term = null, fit = null, resizeTimer = null;
let curSessionId = null, recog = null, recording = false;

function setStatus(s) { statusEl.textContent = s || ''; }
// Write bytes to the PTY over the WS.
function wsData(data) { if (ws && ws.readyState === 1) ws.send(JSON.stringify({ type: 'data', data })); }
// Submit a message: send the text, then a SEPARATE Enter after a short pause —
// claude's TUI reads text+\r in one write as a paste and won't submit (same
// paste-burst workaround the desktop uses).
function submitText(value) {
  if (!value) return;
  wsData(value);
  setTimeout(() => wsData('\r'), 70);
}

async function loadSessions() {
  listEl.innerHTML = '<div class="empty">Carregando…</div>';
  try {
    const r = await fetch('/api/sessions', { headers: { Authorization: 'Bearer ' + token } });
    if (r.status === 401) { listEl.innerHTML = '<div class="empty">Token inválido. Escaneie o QR de novo.</div>'; return; }
    const sessions = await r.json();
    if (!sessions.length) { listEl.innerHTML = '<div class="empty">Nenhuma sessão Claude puro.</div>'; return; }
    listEl.innerHTML = '';
    for (const s of sessions) {
      const div = document.createElement('div');
      div.className = 'session' + (s.live ? '' : ' dead');
      div.innerHTML = `<h3><span class="dot ${s.live?'':'off'}"></span>${escapeHtml(s.title || '(sem título)')}</h3>
        <div class="meta">${escapeHtml(s.project)} · ${escapeHtml(s.model || '')} · ${s.status}</div>`;
      if (s.live) div.onclick = () => connect(s);
      listEl.appendChild(div);
    }
  } catch (e) { listEl.innerHTML = '<div class="empty">Erro: ' + escapeHtml(String(e)) + '</div>'; }
}

function connect(s) {
  curSessionId = s.id;
  listEl.classList.add('hidden');
  termEl.classList.remove('hidden');
  barEl.classList.remove('hidden');
  statusEl.classList.remove('hidden');
  leaveBtn.classList.remove('hidden');
  refreshBtn.classList.add('hidden');
  subEl.textContent = s.title || s.project;

  term = new Terminal({ fontSize: 13, scrollback: 5000, cursorBlink: true,
    theme: { background: '#0b0d12' } });
  fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(termEl);
  fit.fit();

  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  ws = new WebSocket(`${proto}://${location.host}/ws?session=${encodeURIComponent(s.id)}&t=${encodeURIComponent(token)}`);
  ws.onopen = () => sendResize();
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.type === 'snapshot') { term.reset(); term.write(m.data); }
    else if (m.type === 'output') { term.write(m.data); }
    else if (m.type === 'error') { term.write('\r\n[' + m.message + ']\r\n'); }
  };
  ws.onclose = () => { term.write('\r\n[conexão encerrada]\r\n'); };

  term.onData(d => { if (ws && ws.readyState === 1) ws.send(JSON.stringify({ type: 'data', data: d })); });
  term.onResize(() => sendResize());
  window.addEventListener('resize', onWindowResize);
}

function sendResize() {
  if (!fit || !ws || ws.readyState !== 1) return;
  fit.fit();
  ws.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }));
}
function onWindowResize() {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(sendResize, 120);
}

function leave() {
  if (recording) stopMic();
  if (ws && ws.readyState === 1) { ws.send(JSON.stringify({ type: 'release' })); ws.close(); }
  window.removeEventListener('resize', onWindowResize);
  if (term) { term.dispose(); term = null; }
  termEl.innerHTML = '';
  termEl.classList.add('hidden');
  barEl.classList.add('hidden');
  statusEl.classList.add('hidden');
  leaveBtn.classList.add('hidden');
  refreshBtn.classList.remove('hidden');
  listEl.classList.remove('hidden');
  subEl.textContent = '';
  setStatus('');
  curSessionId = null;
  loadSessions();
}

// ── Voice: browser Web Speech API (needs the HTTPS secure context) ───────────
const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
function startMic() {
  if (!SR) { setStatus('Reconhecimento de voz indisponível neste navegador.'); return; }
  recog = new SR();
  recog.lang = navigator.language || 'pt-BR';
  recog.interimResults = true;
  recog.continuous = false;
  let finalText = '';
  recog.onresult = (e) => {
    let interim = '';
    for (let i = e.resultIndex; i < e.results.length; i++) {
      const r = e.results[i];
      if (r.isFinal) finalText += r[0].transcript;
      else interim += r[0].transcript;
    }
    setStatus('🎤 ' + (finalText + interim).trim());
  };
  recog.onerror = (e) => { setStatus('Erro de voz: ' + e.error); };
  recog.onend = () => {
    recording = false;
    micBtn.classList.remove('rec');
    const t = finalText.trim();
    if (t) { submitText(t); setStatus('Enviado: ' + t); }
    else setStatus('');
  };
  recording = true;
  micBtn.classList.add('rec');
  setStatus('🎤 ouvindo…');
  recog.start();
}
function stopMic() { if (recog) recog.stop(); }
function toggleMic() { if (recording) stopMic(); else startMic(); }

// ── File attach: upload bytes to the desktop, inject @path into the prompt ───
async function uploadFile(file) {
  if (!curSessionId) return;
  setStatus('Enviando ' + file.name + '…');
  try {
    const buf = await file.arrayBuffer();
    const r = await fetch('/api/upload?session=' + encodeURIComponent(curSessionId)
      + '&name=' + encodeURIComponent(file.name) + '&t=' + encodeURIComponent(token), {
      method: 'POST',
      headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/octet-stream' },
      body: buf,
    });
    if (!r.ok) { setStatus('Falha no upload (' + r.status + ')'); return; }
    const info = await r.json();
    // Inject `@path ` (trailing space accepts claude's autocomplete). No submit —
    // user can add text then send.
    wsData('@' + info.path + ' ');
    setStatus('Anexado: ' + file.name);
  } catch (e) { setStatus('Erro: ' + String(e)); }
}

function escapeHtml(s) { return s.replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])); }

leaveBtn.onclick = leave;
refreshBtn.onclick = loadSessions;
micBtn.onclick = toggleMic;
attachBtn.onclick = () => fileEl.click();
fileEl.onchange = () => { const f = fileEl.files && fileEl.files[0]; if (f) uploadFile(f); fileEl.value = ''; };
sendBtn.onclick = () => { const v = msgEl.value.trim(); if (v) { submitText(v); msgEl.value = ''; } };
msgEl.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); sendBtn.onclick(); } });
if (!SR) micBtn.classList.add('hidden');
refreshBtn.classList.remove('hidden');
if (!token) listEl.innerHTML = '<div class="empty">Faltando token. Abra pelo QR do desktop.</div>';
else loadSessions();
</script>
</body>
</html>"#;
