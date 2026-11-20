// atmosphere · discord.rs · Optional Discord Rich Presence over Discord's local IPC socket, plus the art assets for your own Discord app
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};

pub const BUILTIN_CLIENT_ID: &str = "";
pub const ASSET_LOGO: &str = "atmosphere";
pub const ASSET_PAUSED: &str = "paused";
const LOGO_PNG: &[u8] = include_bytes!("../icons/icon.png");
const SEND_GAP: Duration = Duration::from_secs(4);
const RETRY: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub position_ms: u64,
    pub playing: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub enabled: bool,
    pub client_id: String,
    pub cover: bool,
    pub paused: bool,
    pub status: String,
}

impl Options {
    pub fn from(s: &Settings) -> Self {
        let own = s.discord_client_id.trim();
        Self {
            enabled: s.discord_enabled,
            client_id: if own.is_empty() { BUILTIN_CLIENT_ID.to_string() } else { own.to_string() },
            cover: s.discord_cover,
            paused: s.discord_paused,
            status: s.discord_status.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String,
    pub user: Option<String>,
    pub error: Option<String>,
    pub builtin_id: bool,
}

enum Msg {
    Now(Option<NowPlaying>),
    Opts(Options),
}

pub struct DiscordHub {
    tx: mpsc::UnboundedSender<Msg>,
    status: Arc<Mutex<Status>>,
}

impl DiscordHub {
    pub fn start(app: AppHandle, http: reqwest::Client, opts: Options) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let status = Arc::new(Mutex::new(Status { state: "off".into(), builtin_id: !BUILTIN_CLIENT_ID.is_empty(), ..Default::default() }));
        let worker = Worker { app, http, status: status.clone(), opts, now: None, conn: None, covers: HashMap::new(), nonce: 0 };
        tauri::async_runtime::spawn(worker.run(rx));
        Self { tx, status }
    }

    pub fn now_playing(&self, now: Option<NowPlaying>) {
        let _ = self.tx.send(Msg::Now(now));
    }

    pub fn configure(&self, opts: Options) {
        let _ = self.tx.send(Msg::Opts(opts));
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

struct Conn {
    io: Box<dyn Io>,
    client_id: String,
}

enum Fail {
    NotRunning,
    Rejected(String),
    Broken,
}

struct Worker {
    app: AppHandle,
    http: reqwest::Client,
    status: Arc<Mutex<Status>>,
    opts: Options,
    now: Option<NowPlaying>,
    conn: Option<Conn>,
    covers: HashMap<String, Option<String>>,
    nonce: u64,
}

impl Worker {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        let mut dirty = true;
        let mut ready = Instant::now();
        let mut last = Instant::now().checked_sub(SEND_GAP).unwrap_or_else(Instant::now);
        loop {
            let msg = if dirty {
                match timeout_at(ready, rx.recv()).await {
                    Ok(None) => break,
                    Ok(Some(m)) => Some(m),
                    Err(_) => None,
                }
            } else {
                match rx.recv().await {
                    None => break,
                    m => m,
                }
            };
            match msg {
                Some(Msg::Now(n)) => {
                    if n == self.now {
                        continue;
                    }
                    self.now = n;
                    dirty = true;
                    ready = ready.max(last + SEND_GAP).max(Instant::now());
                }
                Some(Msg::Opts(o)) => {
                    if o == self.opts {
                        continue;
                    }
                    if o.client_id != self.opts.client_id || !o.enabled {
                        self.conn = None;
                    }
                    self.opts = o;
                    dirty = true;
                    ready = Instant::now();
                }
                None => match self.flush().await {
                    Ok(()) => {
                        dirty = false;
                        last = Instant::now();
                    }
                    Err(wait) => ready = Instant::now() + wait,
                },
            }
        }
    }

    fn set_status(&self, state: &str, user: Option<String>, error: Option<String>) {
        let s = Status { state: state.into(), user, error, builtin_id: !BUILTIN_CLIENT_ID.is_empty() };
        let changed = {
            let mut cur = self.status.lock().unwrap();
            let changed = (cur.state.as_str(), &cur.user, &cur.error) != (s.state.as_str(), &s.user, &s.error);
            *cur = s.clone();
            changed
        };
        if changed {
            let _ = self.app.emit("atmosphere://discord", s);
        }
    }

    async fn flush(&mut self) -> Result<(), Duration> {
        if !self.opts.enabled {
            self.conn = None;
            self.set_status("off", None, None);
            return Ok(());
        }
        if self.opts.client_id.is_empty() {
            self.conn = None;
            self.set_status("noid", None, None);
            return Ok(());
        }
        if self.conn.as_ref().is_some_and(|c| c.client_id != self.opts.client_id) {
            self.conn = None;
        }
        if self.conn.is_none() {
            self.set_status("connecting", None, None);
            match connect(&self.opts.client_id).await {
                Ok((conn, user)) => {
                    self.conn = Some(conn);
                    self.set_status("connected", user, None);
                }
                Err(Fail::NotRunning) => {
                    self.set_status("waiting", None, None);
                    return Err(RETRY);
                }
                Err(Fail::Rejected(e)) => {
                    self.set_status("error", None, Some(e));
                    return Err(RETRY * 4);
                }
                Err(Fail::Broken) => {
                    self.set_status("waiting", None, None);
                    return Err(RETRY);
                }
            }
        }
        let wanted = match (&self.now, self.opts.cover) {
            (Some(n), true) if !n.title.trim().is_empty() => Some((n.artist.clone(), n.title.clone())),
            _ => None,
        };
        let cover = match wanted {
            Some((artist, title)) => self.cover_for(&artist, &title).await,
            None => None,
        };
        let act = self.now.as_ref().and_then(|n| activity(&self.opts, n, cover.as_deref(), now_ms()));
        self.nonce += 1;
        let frame = json!({ "cmd": "SET_ACTIVITY", "args": { "pid": std::process::id(), "activity": act }, "nonce": self.nonce.to_string() });
        let conn = self.conn.as_mut().expect("connected");
        let sent = async {
            write_frame(&mut conn.io, 1, &frame).await.ok()?;
            read_frame(&mut conn.io).await.ok()
        };
        match timeout(Duration::from_secs(5), sent).await {
            Ok(Some((_, reply))) if reply["evt"] == "ERROR" => {
                let msg = reply["data"]["message"].as_str().unwrap_or("Discord didn't accept the status").to_string();
                let user = self.status.lock().unwrap().user.clone();
                self.set_status("connected", user, Some(msg));
                Ok(())
            }
            Ok(Some(_)) => {
                let user = self.status.lock().unwrap().user.clone();
                self.set_status("connected", user, None);
                Ok(())
            }
            _ => {
                self.conn = None;
                self.set_status("waiting", None, None);
                Err(Duration::from_secs(2))
            }
        }
    }

    async fn cover_for(&mut self, artist: &str, title: &str) -> Option<String> {
        let key = format!("{}\u{1}{}", crate::artistinfo::norm(artist), crate::artistinfo::norm(title));
        if let Some(hit) = self.covers.get(&key) {
            return hit.clone();
        }
        let first = artist.split([',', '&']).next().unwrap_or(artist).trim();
        let q = if first.is_empty() { format!("track:\"{title}\"") } else { format!("artist:\"{first}\" track:\"{title}\"") };
        let url = format!("https://api.deezer.com/search?limit=5&q={}", crate::artistinfo::enc(&q));
        let found = match timeout(Duration::from_secs(6), crate::artistinfo::get_json(&self.http, &url)).await {
            Ok(Some(v)) => pick_cover(&v, first),
            Ok(None) => None,
            Err(_) => return None,
        };
        self.covers.insert(key, found.clone());
        found
    }
}

async fn timeout_at<F: std::future::Future>(at: Instant, f: F) -> Result<F::Output, tokio::time::error::Elapsed> {
    tokio::time::timeout_at(at, f).await
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn pick_cover(v: &Value, artist: &str) -> Option<String> {
    let want = crate::artistinfo::norm(artist);
    v["data"].as_array()?.iter().find_map(|t| {
        let name = crate::artistinfo::norm(t["artist"]["name"].as_str().unwrap_or_default());
        let ok = want.is_empty() || name == want || name.contains(&want) || want.contains(&name);
        let url = t["album"]["cover_xl"].as_str().or_else(|| t["album"]["cover_big"].as_str())?;
        (ok && url.starts_with("https://")).then(|| url.to_string())
    })
}

fn fit(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut out: String = s.chars().take(128).collect();
    if s.chars().count() > 128 {
        out = out.chars().take(127).collect::<String>() + "…";
    }
    while out.chars().count() < 2 {
        out.push('\u{2800}');
    }
    Some(out)
}

pub fn activity(opts: &Options, now: &NowPlaying, cover: Option<&str>, at_ms: u64) -> Option<Value> {
    if !now.playing && !opts.paused {
        return None;
    }
    let title = fit(&now.title).unwrap_or_else(|| "Unknown song".into());
    let mut a = json!({
        "type": 2,
        "details": title,
        "status_display_type": match opts.status.as_str() { "artist" => 1, "song" => 2, _ => 0 },
    });
    if let Some(artist) = fit(&now.artist) {
        a["state"] = json!(artist);
    }
    let mut assets = json!({ "large_image": cover.unwrap_or(ASSET_LOGO) });
    if let Some(album) = fit(&now.album) {
        assets["large_text"] = json!(album);
    }
    if !now.playing {
        assets["small_image"] = json!(ASSET_PAUSED);
        assets["small_text"] = json!("Paused");
    } else if cover.is_some() {
        assets["small_image"] = json!(ASSET_LOGO);
        assets["small_text"] = json!("Atmosphere");
    }
    a["assets"] = assets;
    if now.playing {
        let start = at_ms.saturating_sub(now.position_ms);
        let mut ts = json!({ "start": start });
        if now.duration_ms > now.position_ms {
            ts["end"] = json!(start + now.duration_ms);
        }
        a["timestamps"] = ts;
    }
    Some(a)
}

async fn write_frame(io: &mut Box<dyn Io>, op: u32, v: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(v)?;
    let mut buf = Vec::with_capacity(body.len() + 8);
    buf.extend(op.to_le_bytes());
    buf.extend((body.len() as u32).to_le_bytes());
    buf.extend(body);
    io.write_all(&buf).await?;
    io.flush().await
}

async fn read_frame(io: &mut Box<dyn Io>) -> std::io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    io.read_exact(&mut head).await?;
    let op = u32::from_le_bytes(head[..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; len];
    io.read_exact(&mut body).await?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

async fn open_socket() -> Option<Box<dyn Io>> {
    #[cfg(windows)]
    for i in 0..10 {
        if let Ok(p) = tokio::net::windows::named_pipe::ClientOptions::new().open(format!(r"\\.\pipe\discord-ipc-{i}")) {
            return Some(Box::new(p));
        }
    }
    #[cfg(unix)]
    {
        let mut dirs: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(|k| std::env::var_os(k).map(Into::into)).collect();
        dirs.push("/tmp".into());
        for dir in dirs {
            for sub in ["", "app/com.discordapp.Discord", "snap.discord", ".flatpak/dev.vencord.Vesktop/xdg-run"] {
                for i in 0..10 {
                    let path = dir.join(sub).join(format!("discord-ipc-{i}"));
                    if let Ok(s) = tokio::net::UnixStream::connect(&path).await {
                        return Some(Box::new(s));
                    }
                }
            }
        }
    }
    None
}

async fn connect(client_id: &str) -> Result<(Conn, Option<String>), Fail> {
    let mut io = open_socket().await.ok_or(Fail::NotRunning)?;
    let hello = async {
        write_frame(&mut io, 0, &json!({ "v": 1, "client_id": client_id })).await.ok()?;
        read_frame(&mut io).await.ok()
    };
    let (op, reply) = timeout(Duration::from_secs(5), hello).await.ok().flatten().ok_or(Fail::Broken)?;
    if op == 2 || reply["evt"] == "ERROR" {
        let msg = reply["message"].as_str().or_else(|| reply["data"]["message"].as_str()).unwrap_or("Discord refused the connection");
        let friendly = if msg.to_lowercase().contains("client id") { "Discord doesn't recognise that application ID. Check it in the Discord Developer Portal." } else { msg };
        return Err(Fail::Rejected(friendly.to_string()));
    }
    let u = &reply["data"]["user"];
    let user = u["global_name"].as_str().filter(|s| !s.is_empty()).or_else(|| u["username"].as_str()).map(str::to_string);
    Ok((Conn { io, client_id: client_id.to_string() }, user))
}

fn badge_png(size: u32) -> Vec<u8> {
    let (bg, fg) = ([0x4bu8, 0x52, 0xd6], [255u8, 255, 255]);
    let s = size as f32;
    let (cx, r) = (s / 2.0, s / 2.0);
    let (bar_w, bar_h, gap) = (s * 0.11, s * 0.40, s * 0.10);
    let in_bar = |x: f32, y: f32| {
        let top = cx - bar_h / 2.0;
        let left = cx - gap / 2.0 - bar_w;
        let right = cx + gap / 2.0;
        (top..top + bar_h).contains(&y) && ((left..left + bar_w).contains(&x) || (right..right + bar_w).contains(&x))
    };
    let mut img = image::RgbaImage::new(size, size);
    const N: u32 = 4;
    for (px, py, p) in img.enumerate_pixels_mut() {
        let (mut circle, mut glyph) = (0u32, 0u32);
        for sy in 0..N {
            for sx in 0..N {
                let x = px as f32 + (sx as f32 + 0.5) / N as f32;
                let y = py as f32 + (sy as f32 + 0.5) / N as f32;
                if (x - cx).powi(2) + (y - cx).powi(2) <= r * r {
                    circle += 1;
                    if in_bar(x, y) {
                        glyph += 1;
                    }
                }
            }
        }
        if circle == 0 {
            continue;
        }
        let t = glyph as f32 / circle as f32;
        let mix = |i: usize| (bg[i] as f32 * (1.0 - t) + fg[i] as f32 * t).round() as u8;
        *p = image::Rgba([mix(0), mix(1), mix(2), (circle * 255 / (N * N)) as u8]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

const SETUP_NOTES: &str = "Atmosphere on Discord: using your own Discord application

1. Open https://discord.com/developers/applications and choose New Application.
   The name you give it is what Discord shows as \"Listening to <name>\".
2. On the application's General Information page, copy the Application ID
   and paste it into Atmosphere > Settings > Discord > Your own Discord app.
3. Open Rich Presence > Art Assets and upload both images from this folder.
   Keep their names exactly as they are (atmosphere and paused), because
   Atmosphere asks Discord for them by name.
4. New art can take a few minutes to show up on Discord.
";

pub fn write_assets(dir: &Path) -> anyhow::Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let files = [(format!("{ASSET_LOGO}.png"), LOGO_PNG.to_vec()), (format!("{ASSET_PAUSED}.png"), badge_png(1024)), ("How to set up.txt".to_string(), SETUP_NOTES.as_bytes().to_vec())];
    let mut written = Vec::new();
    for (name, bytes) in files {
        let path = dir.join(&name);
        std::fs::write(&path, bytes)?;
        written.push(path.to_string_lossy().into_owned());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options { enabled: true, client_id: "1".into(), cover: true, paused: true, status: "app".into() }
    }

    #[test]
    fn builds_a_listening_activity_with_a_progress_bar() {
        let now = NowPlaying { title: "Get Lucky".into(), artist: "Daft Punk".into(), album: "Random Access Memories".into(), duration_ms: 248_000, position_ms: 8_000, playing: true };
        let a = activity(&opts(), &now, Some("https://cdn/x.jpg"), 1_000_000).unwrap();
        assert_eq!(a["type"], 2);
        assert_eq!(a["details"], "Get Lucky");
        assert_eq!(a["state"], "Daft Punk");
        assert_eq!(a["timestamps"]["start"], 992_000);
        assert_eq!(a["timestamps"]["end"], 1_240_000);
        assert_eq!(a["assets"]["large_image"], "https://cdn/x.jpg");
        assert_eq!(a["assets"]["small_image"], ASSET_LOGO);
        assert_eq!(a["status_display_type"], 0);

        let paused = NowPlaying { playing: false, ..now.clone() };
        let p = activity(&Options { status: "artist".into(), ..opts() }, &paused, None, 1_000_000).unwrap();
        assert!(p.get("timestamps").is_none());
        assert_eq!(p["assets"]["large_image"], ASSET_LOGO);
        assert_eq!(p["assets"]["small_image"], ASSET_PAUSED);
        assert_eq!(p["status_display_type"], 1);
        assert!(activity(&Options { paused: false, ..opts() }, &paused, None, 0).is_none());

        let short = NowPlaying { title: "X".into(), artist: String::new(), album: "A".repeat(300), ..Default::default() };
        let s = activity(&opts(), &short, None, 0).unwrap();
        assert_eq!(s["details"].as_str().unwrap().chars().count(), 2);
        assert!(s.get("state").is_none());
        assert_eq!(s["assets"]["large_text"].as_str().unwrap().chars().count(), 128);
    }

    #[test]
    fn picks_a_cover_only_from_the_same_artist() {
        let v = json!({ "data": [
            { "artist": { "name": "Someone Else" }, "album": { "cover_xl": "https://e/1.jpg" } },
            { "artist": { "name": "Daft Punk" }, "album": { "cover_xl": "https://e/2.jpg" } }
        ]});
        assert_eq!(pick_cover(&v, "Daft Punk").as_deref(), Some("https://e/2.jpg"));
        assert_eq!(pick_cover(&json!({ "data": [v["data"][0].clone()] }), "Daft Punk"), None);
    }

    #[tokio::test]
    #[ignore]
    async fn live_discord_refuses_an_unknown_app() {
        match connect("100000000000000001").await {
            Err(Fail::Rejected(msg)) => eprintln!("refused: {msg}"),
            Err(Fail::NotRunning) => panic!("Discord isn't running"),
            Err(Fail::Broken) => panic!("no reply"),
            Ok(_) => panic!("accepted a made-up app"),
        }
    }

    #[test]
    fn writes_the_art_assets() {
        let dir = std::env::temp_dir().join(format!("atmo-discord-{}", std::process::id()));
        let files = write_assets(&dir).unwrap();
        assert_eq!(files.len(), 3);
        let badge = image::open(dir.join("paused.png")).unwrap();
        assert_eq!((badge.width(), badge.height()), (1024, 1024));
        let logo = image::open(dir.join("atmosphere.png")).unwrap();
        assert!(logo.width() >= 512);
        let _ = std::fs::remove_dir_all(dir);
    }
}
