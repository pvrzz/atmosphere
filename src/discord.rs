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

pub const BUILTIN_CLIENT_ID: &str = "1573218461658648768";
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
            (Some(n), true) if !n.title.trim().is_empty() => Some((n.artist.clone(), n.title.clone(), n.album.clone())),
            _ => None,
        };
        let cover = match wanted {
            Some((artist, title, album)) => self.cover_for(&artist, &title, &album).await,
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

    async fn cover_for(&mut self, artist: &str, title: &str, album: &str) -> Option<String> {
        let norm = crate::artistinfo::norm;
        let key = format!("{}\u{1}{}\u{1}{}", norm(artist), norm(title), norm(album));
        if let Some(hit) = self.covers.get(&key) {
            return hit.clone();
        }
        let found = timeout(Duration::from_secs(15), find_cover(&self.http, artist, title, album)).await.ok().flatten();
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

pub fn lead_artist(artist: &str) -> String {
    static SPLIT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\s*(?:,|;|\s(?:feat\.?|ft\.?|featuring|with|x)\s)").unwrap());
    SPLIT.split(artist.trim()).next().unwrap_or_default().trim().to_string()
}

pub fn clean_title(title: &str) -> String {
    static NOISE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\s*(?:[\(\[][^\)\]]*(?:feat|ft\.|with |remaster|version|edit|mix|live|explicit|clean|bonus|deluxe|official|audio|video|lyric)[^\)\]]*[\)\]]|\s-\s.*(?:remaster|version|edit|mix|live).*$|\s(?:feat\.?|ft\.?|featuring)\s.*$)").unwrap()
    });
    let t = NOISE.replace_all(title.trim(), "").trim().to_string();
    if t.is_empty() { title.trim().to_string() } else { t }
}

fn same_artist(found: &str, want: &str) -> bool {
    let (f, w) = (crate::artistinfo::norm(found), crate::artistinfo::norm(want));
    w.is_empty() || (!f.is_empty() && (f == w || f.contains(&w) || w.contains(&f)))
}

pub fn pick_album_cover(v: &Value, artist: &str, album: &str) -> Option<String> {
    let want = crate::artistinfo::norm(album);
    v["data"].as_array()?.iter().find_map(|a| {
        let title = crate::artistinfo::norm(a["title"].as_str().unwrap_or_default());
        let ok = same_artist(a["artist"]["name"].as_str().unwrap_or_default(), artist) && !want.is_empty() && (title == want || title.starts_with(&want) || want.starts_with(&title));
        let url = a["cover_xl"].as_str().or_else(|| a["cover_big"].as_str())?;
        (ok && url.starts_with("https://")).then(|| url.to_string())
    })
}

pub fn pick_itunes(v: &Value, artist: &str, title: &str, album: &str) -> Option<String> {
    let (t, al) = (crate::artistinfo::norm(title), crate::artistinfo::norm(album));
    let list = v["results"].as_array()?;
    let ok = |r: &&Value| same_artist(r["artistName"].as_str().unwrap_or_default(), artist);
    let by_song = list.iter().filter(ok).find(|r| {
        let name = crate::artistinfo::norm(&clean_title(r["trackName"].as_str().unwrap_or_default()));
        !t.is_empty() && (name == t || name.starts_with(&t) || t.starts_with(&name))
    });
    let by_album = || list.iter().filter(ok).find(|r| !al.is_empty() && crate::artistinfo::norm(r["collectionName"].as_str().unwrap_or_default()).starts_with(&al));
    let art = by_song.or_else(by_album)?["artworkUrl100"].as_str()?;
    art.starts_with("https://").then(|| art.replace("100x100bb", "1000x1000bb"))
}

async fn find_cover(http: &reqwest::Client, artist: &str, title: &str, album: &str) -> Option<String> {
    use crate::artistinfo::{enc, get_json};
    let lead = lead_artist(artist);
    let song = clean_title(title);
    let album = album.trim();
    let field = |k: &str, v: &str| if v.is_empty() { String::new() } else { format!("{k}:\"{v}\" ") };
    let deezer = |q: String| format!("https://api.deezer.com/search{q}");
    if let Some(v) = get_json(http, &deezer(format!("?limit=10&q={}", enc(&format!("{}{}", field("artist", &lead), field("track", &song)))))).await {
        if let Some(u) = pick_cover(&v, &lead) {
            return Some(u);
        }
    }
    if !album.is_empty() {
        if let Some(v) = get_json(http, &deezer(format!("/album?limit=10&q={}", enc(&format!("{}{}", field("artist", &lead), field("album", &clean_title(album))))))).await {
            if let Some(u) = pick_album_cover(&v, &lead, &clean_title(album)) {
                return Some(u);
            }
        }
    }
    let term = enc(&format!("{lead} {song}"));
    if let Some(v) = get_json(http, &format!("https://itunes.apple.com/search?media=music&entity=song&limit=15&term={term}")).await {
        if let Some(u) = pick_itunes(&v, &lead, &song, album) {
            return Some(u);
        }
    }
    let loose = get_json(http, &deezer(format!("?limit=10&q={}", enc(&format!("{lead} {song}"))))).await?;
    pick_cover(&loose, &lead)
}

pub fn pick_cover(v: &Value, artist: &str) -> Option<String> {
    let want = crate::artistinfo::norm(artist);
    v["data"].as_array()?.iter().find_map(|t| {
        let _ = &want;
        let ok = same_artist(t["artist"]["name"].as_str().unwrap_or_default(), artist);
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
    for i in 0..10 {
        if let Ok(p) = tokio::net::windows::named_pipe::ClientOptions::new().open(format!(r"\\.\pipe\discord-ipc-{i}")) {
            return Some(Box::new(p));
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
3. Open Rich Presence > Art Assets. Under Rich Presence Assets, choose
   Add Image(s) and upload atmosphere.png and paused.png. Keep their names
   exactly as they are, because Atmosphere asks Discord for them by name.
4. Optional: under Rich Presence Invite Image, upload invite-cover.png as
   the Cover Image (1024x576).
5. New art can take a few minutes to show up on Discord.
";

fn cover_png() -> Vec<u8> {
    let (w, h) = (1024u32, 576u32);
    let (from, to, glow): ([f32; 3], [f32; 3], [f32; 3]) = ([13.0, 15.0, 31.0], [42.0, 47.0, 143.0], [154.0, 164.0, 255.0]);
    let mut img = image::RgbaImage::from_fn(w, h, |x, y| {
        let t = (x as f32 / w as f32 * 0.6 + y as f32 / h as f32 * 0.4).clamp(0.0, 1.0);
        let (dx, dy) = (x as f32 - w as f32 / 2.0, (y as f32 - h as f32 / 2.0) * 1.4);
        let g = (1.0 - (dx * dx + dy * dy).sqrt() / 420.0).clamp(0.0, 1.0).powi(2) * 0.28;
        let c = |i: usize| (from[i] + (to[i] - from[i]) * t) * (1.0 - g) + glow[i] * g;
        image::Rgba([c(0) as u8, c(1) as u8, c(2) as u8, 255])
    });
    if let Ok(logo) = image::load_from_memory(LOGO_PNG) {
        let size = 300;
        let logo = image::imageops::resize(&logo.to_rgba8(), size, size, image::imageops::FilterType::Lanczos3);
        image::imageops::overlay(&mut img, &logo, ((w - size) / 2) as i64, ((h - size) / 2) as i64);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

pub fn write_assets(dir: &Path) -> anyhow::Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let files = [
        (format!("{ASSET_LOGO}.png"), LOGO_PNG.to_vec()),
        (format!("{ASSET_PAUSED}.png"), badge_png(1024)),
        ("invite-cover.png".to_string(), cover_png()),
        ("How to set up.txt".to_string(), SETUP_NOTES.as_bytes().to_vec()),
    ];
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
    fn cleans_titles_and_finds_covers_from_apple_music_too() {
        assert_eq!(lead_artist("Rihanna, The Weeknd feat. Drake"), "Rihanna");
        assert_eq!(lead_artist("Mumford & Sons"), "Mumford & Sons");
        assert_eq!(lead_artist("Calvin Harris x Dua Lipa"), "Calvin Harris");
        assert_eq!(clean_title("Get Lucky (feat. Pharrell Williams) [Radio Edit]"), "Get Lucky");
        assert_eq!(clean_title("Bohemian Rhapsody - Remastered 2011"), "Bohemian Rhapsody");
        assert_eq!(clean_title("Love Story (Taylor's Version)"), "Love Story");
        assert_eq!(clean_title("(Untitled)"), "(Untitled)");
        let it = json!({ "results": [
            { "artistName": "Someone", "trackName": "Woo", "collectionName": "X", "artworkUrl100": "https://is1-ssl.mzstatic.com/a/100x100bb.jpg" },
            { "artistName": "Rihanna", "trackName": "Woo", "collectionName": "ANTI", "artworkUrl100": "https://is1-ssl.mzstatic.com/b/100x100bb.jpg" }
        ]});
        assert_eq!(pick_itunes(&it, "Rihanna", "Woo", "").as_deref(), Some("https://is1-ssl.mzstatic.com/b/1000x1000bb.jpg"));
        assert_eq!(pick_itunes(&it, "Nobody", "Woo", ""), None);
        let al = json!({ "data": [ { "title": "ANTI (Deluxe)", "artist": { "name": "Rihanna" }, "cover_xl": "https://e/anti.jpg" } ] });
        assert_eq!(pick_album_cover(&al, "Rihanna", "ANTI").as_deref(), Some("https://e/anti.jpg"));
        assert_eq!(pick_album_cover(&al, "Drake", "ANTI"), None);
    }

    #[tokio::test]
    #[ignore]
    async fn live_finds_covers() {
        let http = reqwest::Client::builder().user_agent("atmosphere/test").build().unwrap();
        for (a, t, al) in [("Rihanna, The Weeknd, Travis Scott, Jeremih", "Woo", "ANTI"), ("Daft Punk", "Get Lucky (feat. Pharrell Williams) [Radio Edit]", ""), ("Lana Del Rey", "Video Games - Remastered", "Born To Die")] {
            let c = find_cover(&http, a, t, al).await;
            eprintln!("{t}: {c:?}");
            assert!(c.is_some_and(|u| u.starts_with("https://")), "no cover for {t}");
        }
    }

    #[tokio::test]
    #[ignore]
    async fn live_discord_echo() {
        let id = std::env::var("ATMO_DISCORD_ID").expect("set ATMO_DISCORD_ID");
        let Ok((mut conn, user)) = connect(&id).await else { panic!("couldn't connect") };
        eprintln!("user: {user:?}");
        let now = NowPlaying { title: "Woo".into(), artist: "Rihanna".into(), album: "ANTI".into(), duration_ms: 236_000, position_ms: 1_000, playing: true };
        let act = activity(&Options { enabled: true, client_id: id.clone(), cover: true, paused: true, status: "artist".into() }, &now, None, now_ms());
        write_frame(&mut conn.io, 1, &json!({ "cmd": "SET_ACTIVITY", "args": { "pid": std::process::id(), "activity": act }, "nonce": "t1" })).await.unwrap();
        let (_, reply) = read_frame(&mut conn.io).await.unwrap();
        eprintln!("echo: {}", serde_json::to_string(&reply["data"]).unwrap());
        write_frame(&mut conn.io, 1, &json!({ "cmd": "SET_ACTIVITY", "args": { "pid": std::process::id(), "activity": null }, "nonce": "t2" })).await.unwrap();
        let _ = read_frame(&mut conn.io).await;
    }

    #[test]
    fn writes_the_art_assets() {
        let dir = std::env::temp_dir().join(format!("atmo-discord-{}", std::process::id()));
        let files = write_assets(&dir).unwrap();
        assert_eq!(files.len(), 4);
        let cover = image::open(dir.join("invite-cover.png")).unwrap();
        assert_eq!((cover.width(), cover.height()), (1024, 576));
        let badge = image::open(dir.join("paused.png")).unwrap();
        assert_eq!((badge.width(), badge.height()), (1024, 1024));
        let logo = image::open(dir.join("atmosphere.png")).unwrap();
        assert!(logo.width() >= 512);
        let _ = std::fs::remove_dir_all(dir);
    }
}
