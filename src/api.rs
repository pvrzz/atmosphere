// atmosphere · api.rs · Opt-in local API for other apps: loopback only, token-protected, permission-scoped
use crate::AppState;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast;

pub const ACTION_EVENT: &str = "atmosphere://api";
const MAX_FAILURES: usize = 20;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Any,
    Read,
    Control,
    Links,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenFile {
    token_hash: String,
    created: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    running: bool,
    port: u16,
    error: Option<String>,
    has_token: bool,
    token_created: Option<u64>,
    last_used: Option<u64>,
    last_client: Option<String>,
}

pub struct ApiHub {
    now: RwLock<Value>,
    tx: broadcast::Sender<Value>,
    token: RwLock<TokenFile>,
    failures: Mutex<VecDeque<Instant>>,
    server: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    status: Mutex<Status>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn token_path() -> std::path::PathBuf {
    crate::settings::path().with_file_name("api.json")
}

fn digest(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl ApiHub {
    pub fn load() -> Self {
        let token: TokenFile = std::fs::read(token_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let status = Status { running: false, port: 0, error: None, has_token: !token.token_hash.is_empty(), token_created: (token.created > 0).then_some(token.created), last_used: None, last_client: None };
        Self {
            now: RwLock::new(json!({ "playing": false, "track": null })),
            tx: broadcast::channel(32).0,
            token: RwLock::new(token),
            failures: Mutex::new(VecDeque::new()),
            server: Mutex::new(None),
            status: Mutex::new(status),
        }
    }

    pub fn new_token(&self) -> anyhow::Result<String> {
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw).map_err(|e| anyhow::anyhow!("Couldn't create a secure token ({e})"))?;
        let token = format!("atmo_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw));
        let file = TokenFile { token_hash: digest(&token), created: now_secs() };
        let path = token_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_vec(&file)?)?;
        {
            let mut s = self.status.lock().unwrap();
            s.has_token = true;
            s.token_created = Some(file.created);
            s.last_used = None;
            s.last_client = None;
        }
        *self.token.write().unwrap() = file;
        Ok(token)
    }

    fn verify(&self, token: &str) -> bool {
        let stored = self.token.read().unwrap();
        !stored.token_hash.is_empty() && same(digest(token).as_bytes(), stored.token_hash.as_bytes())
    }

    fn locked(&self) -> bool {
        let mut f = self.failures.lock().unwrap();
        while f.front().is_some_and(|t| t.elapsed() > FAILURE_WINDOW) {
            f.pop_front();
        }
        f.len() >= MAX_FAILURES
    }

    fn fail(&self) {
        self.failures.lock().unwrap().push_back(Instant::now());
    }

    fn touch(&self, client: Option<&str>) {
        let mut s = self.status.lock().unwrap();
        s.last_used = Some(now_secs());
        s.last_client = client.map(|c| c.chars().take(80).collect());
    }

    pub fn publish(&self, state: Value) {
        *self.now.write().unwrap() = state.clone();
        let _ = self.tx.send(state);
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}

#[derive(Clone)]
struct Ctx {
    app: AppHandle,
    port: u16,
}

fn deny(code: StatusCode, msg: &str) -> Response {
    (code, [(header::CACHE_CONTROL, "no-store")], Json(json!({ "error": msg }))).into_response()
}

fn guard(ctx: &Ctx, headers: &HeaderMap, scope: Scope) -> Option<Response> {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    if host != format!("127.0.0.1:{}", ctx.port) && host != format!("localhost:{}", ctx.port) {
        return Some(deny(StatusCode::MISDIRECTED_REQUEST, "Connect to 127.0.0.1 directly."));
    }
    if headers.contains_key(header::ORIGIN) || headers.contains_key("sec-fetch-site") {
        return Some(deny(StatusCode::FORBIDDEN, "Web pages can't use the Atmosphere API."));
    }
    let hub = ctx.app.state::<ApiHub>();
    if hub.locked() {
        return Some(deny(StatusCode::TOO_MANY_REQUESTS, "Too many wrong tokens. Try again in a minute."));
    }
    let token = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim).unwrap_or("");
    if token.is_empty() || !hub.verify(token) {
        hub.fail();
        return Some(deny(StatusCode::UNAUTHORIZED, "Missing or wrong access token."));
    }
    let s = ctx.app.state::<AppState>().snapshot();
    let (allowed, what) = match scope {
        Scope::Any => (true, ""),
        Scope::Read => (s.api_read, "see what's playing or browse the library"),
        Scope::Control => (s.api_control, "control playback"),
        Scope::Links => (s.api_links, "send links"),
    };
    if !allowed {
        return Some(deny(StatusCode::FORBIDDEN, &format!("Atmosphere's settings don't allow apps to {what}.")));
    }
    hub.touch(headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()));
    None
}

fn act(ctx: &Ctx, payload: Value) -> Response {
    let _ = ctx.app.emit(ACTION_EVENT, payload);
    (StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response()
}

async fn ping(State(ctx): State<Ctx>, h: HeaderMap) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Any) {
        return r;
    }
    let s = ctx.app.state::<AppState>().snapshot();
    Json(json!({ "app": "atmosphere", "version": env!("CARGO_PKG_VERSION"), "allowed": { "read": s.api_read, "control": s.api_control, "links": s.api_links } })).into_response()
}

async fn now_playing(State(ctx): State<Ctx>, h: HeaderMap) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Read) {
        return r;
    }
    Json(ctx.app.state::<ApiHub>().now.read().unwrap().clone()).into_response()
}

async fn events(State(ctx): State<Ctx>, h: HeaderMap) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Read) {
        return r;
    }
    let hub = ctx.app.state::<ApiHub>();
    let first = hub.now.read().unwrap().clone();
    let rx = hub.tx.subscribe();
    let ev = |v: Value| Ok::<Event, Infallible>(Event::default().event("now-playing").data(v.to_string()));
    let initial = futures_util::stream::once(async move { ev(first) });
    let updates = futures_util::stream::unfold(rx, move |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(v) => return Some((ev(v), rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    });
    Sse::new(initial.chain(updates)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))).into_response()
}

#[derive(Deserialize)]
struct TrackQuery {
    q: Option<String>,
    limit: Option<usize>,
}

async fn tracks(State(ctx): State<Ctx>, h: HeaderMap, Query(q): Query<TrackQuery>) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Read) {
        return r;
    }
    let state = ctx.app.state::<AppState>();
    let pl = std::path::PathBuf::from(state.snapshot().playlists_dir);
    let snap = state.library.snapshot(&pl);
    let needle = q.q.unwrap_or_default().to_lowercase();
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let list: Vec<Value> = snap
        .tracks
        .iter()
        .filter(|t| needle.is_empty() || [&t.title, &t.artist, &t.album].iter().any(|f| f.to_lowercase().contains(&needle)))
        .take(limit)
        .map(|t| json!({ "id": t.id, "title": t.title, "artist": t.artist, "album": t.album, "albumArtist": t.album_artist, "durationMs": t.duration_ms, "year": t.year, "playlist": t.playlist, "liked": snap.liked.contains(&t.id) }))
        .collect();
    Json(json!({ "tracks": list })).into_response()
}

async fn playlists(State(ctx): State<Ctx>, h: HeaderMap) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Read) {
        return r;
    }
    let state = ctx.app.state::<AppState>();
    let pl = std::path::PathBuf::from(state.snapshot().playlists_dir);
    let list: Vec<Value> = state.library.snapshot(&pl).playlists.iter().map(|p| json!({ "name": p.name, "trackIds": p.order, "linkedFolder": p.linked, "missing": p.missing })).collect();
    Json(json!({ "playlists": list })).into_response()
}

async fn artwork(State(ctx): State<Ctx>, h: HeaderMap, UrlPath(id): UrlPath<String>) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Read) {
        return r;
    }
    let lib = &ctx.app.state::<AppState>().library;
    let Some(path) = lib.track(&id).and_then(|t| t.art).and_then(|k| lib.art_path(&k)) else {
        return deny(StatusCode::NOT_FOUND, "No artwork for that track.");
    };
    match tokio::fs::read(path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "private, max-age=3600")], bytes).into_response(),
        Err(_) => deny(StatusCode::NOT_FOUND, "No artwork for that track."),
    }
}

async fn player(State(ctx): State<Ctx>, h: HeaderMap, UrlPath(action): UrlPath<String>) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Control) {
        return r;
    }
    if !["play", "pause", "toggle", "next", "previous"].contains(&action.as_str()) {
        return deny(StatusCode::NOT_FOUND, "Unknown player action.");
    }
    act(&ctx, json!({ "action": action }))
}

#[derive(Deserialize)]
struct Seek {
    position: f64,
}

async fn seek(State(ctx): State<Ctx>, h: HeaderMap, Json(b): Json<Seek>) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Control) {
        return r;
    }
    if !b.position.is_finite() || b.position < 0.0 {
        return deny(StatusCode::BAD_REQUEST, "position must be a number of seconds, 0 or more.");
    }
    act(&ctx, json!({ "action": "seek", "position": b.position }))
}

#[derive(Deserialize)]
struct Volume {
    volume: f64,
}

async fn volume(State(ctx): State<Ctx>, h: HeaderMap, Json(b): Json<Volume>) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Control) {
        return r;
    }
    if !b.volume.is_finite() || !(0.0..=1.0).contains(&b.volume) {
        return deny(StatusCode::BAD_REQUEST, "volume must be between 0 and 1.");
    }
    act(&ctx, json!({ "action": "volume", "volume": b.volume }))
}

#[derive(Deserialize)]
struct QueueBody {
    ids: Vec<String>,
    mode: Option<String>,
}

async fn queue(State(ctx): State<Ctx>, h: HeaderMap, Json(b): Json<QueueBody>) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Control) {
        return r;
    }
    let mode = b.mode.unwrap_or_else(|| "end".into());
    if !["now", "next", "end"].contains(&mode.as_str()) {
        return deny(StatusCode::BAD_REQUEST, "mode must be now, next or end.");
    }
    if b.ids.is_empty() || b.ids.len() > 500 {
        return deny(StatusCode::BAD_REQUEST, "Send between 1 and 500 track ids.");
    }
    let lib = &ctx.app.state::<AppState>().library;
    let known: Vec<String> = b.ids.into_iter().filter(|id| lib.track(id).is_some()).collect();
    if known.is_empty() {
        return deny(StatusCode::NOT_FOUND, "None of those track ids are in the library.");
    }
    act(&ctx, json!({ "action": "queue", "ids": known, "mode": mode }))
}

#[derive(Deserialize)]
struct LinkBody {
    url: String,
}

async fn links(State(ctx): State<Ctx>, h: HeaderMap, Json(b): Json<LinkBody>) -> Response {
    if let Some(r) = guard(&ctx, &h, Scope::Links) {
        return r;
    }
    let ok = b.url.len() <= 2048 && url::Url::parse(b.url.trim()).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some());
    if !ok {
        return deny(StatusCode::BAD_REQUEST, "url must be a full http or https link.");
    }
    act(&ctx, json!({ "action": "link", "url": b.url.trim() }))
}

async fn fallback() -> Response {
    deny(StatusCode::NOT_FOUND, "Unknown endpoint. See docs/api.md.")
}

fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/v1/ping", get(ping))
        .route("/v1/now-playing", get(now_playing))
        .route("/v1/events", get(events))
        .route("/v1/library/tracks", get(tracks))
        .route("/v1/library/playlists", get(playlists))
        .route("/v1/artwork/{id}", get(artwork))
        .route("/v1/player/seek", post(seek))
        .route("/v1/player/volume", post(volume))
        .route("/v1/player/{action}", post(player))
        .route("/v1/queue", post(queue))
        .route("/v1/links", post(links))
        .fallback(fallback)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(ctx)
}

pub fn apply(app: &AppHandle) {
    let hub = app.state::<ApiHub>();
    if let Some(old) = hub.server.lock().unwrap().take() {
        old.abort();
    }
    let s = app.state::<AppState>().snapshot();
    {
        let mut st = hub.status.lock().unwrap();
        st.running = false;
        st.port = s.api_port;
        st.error = None;
    }
    if !s.api_enabled {
        return;
    }
    if hub.token.read().unwrap().token_hash.is_empty() {
        hub.status.lock().unwrap().error = Some("Create an access token to start the API.".into());
        return;
    }
    let ctx = Ctx { app: app.clone(), port: s.api_port };
    let handle = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let hub = handle.state::<ApiHub>();
        match tokio::net::TcpListener::bind(("127.0.0.1", ctx.port)).await {
            Ok(listener) => {
                hub.status.lock().unwrap().running = true;
                let _ = handle.emit(ACTION_EVENT, json!({ "action": "status" }));
                if let Err(e) = axum::serve(listener, router(ctx)).await {
                    hub.status.lock().unwrap().error = Some(format!("The API stopped ({e})."));
                }
                hub.status.lock().unwrap().running = false;
            }
            Err(e) => {
                hub.status.lock().unwrap().error = Some(format!("Port {} is unavailable ({e}). Choose another port.", ctx.port));
            }
        }
        let _ = handle.emit(ACTION_EVENT, json!({ "action": "status" }));
    });
    *hub.server.lock().unwrap() = Some(task);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_in_constant_time_and_by_digest() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
        assert_eq!(digest("x").len(), 64);
        assert_ne!(digest("atmo_a"), digest("atmo_b"));
    }
}
