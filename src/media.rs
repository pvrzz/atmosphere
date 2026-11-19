// atmosphere · media.rs · Turns links and searches into collections via yt-dlp: metadata, available formats and release-type detection.
use crate::engine::{self, Tools};
use crate::settings::Settings;
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;

pub const PROBE_EVENT: &str = "atmosphere://probe";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub has_video: bool,
    pub heights: Vec<u32>,
    pub best_abr: Option<u32>,
    pub best_codec: Option<String>,
    pub lossless: bool,
    pub original: bool,
    pub original_lossless: bool,
    pub original_ext: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    pub index: u32,
    pub id: String,
    pub url: String,
    pub title: String,
    pub artist: String,
    pub uploader: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub release_date: Option<String>,
    pub year: Option<u16>,
    pub duration_ms: Option<u64>,
    pub thumbnail: Option<String>,
    pub license: Option<String>,
    pub description: Option<String>,
    pub lyrics: Option<String>,
    pub platform: String,
    pub availability: String,
    pub media: Option<MediaInfo>,
    pub detailed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub single: bool,
    pub release_type: String,
    pub release_type_source: String,
    pub title: String,
    pub artist: String,
    pub url: String,
    pub thumbnail: Option<String>,
    pub platform: String,
    pub year: Option<u16>,
    pub entries: Vec<Entry>,
    pub enriching: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ProbeEvent {
    #[serde(rename_all = "camelCase")]
    Entry { collection: String, entry: Box<Entry> },
    #[serde(rename_all = "camelCase")]
    Done { collection: String, release_type: String, release_type_source: String, artist: String, year: Option<u16>, failed: u32 },
}

#[derive(Default)]
pub struct Prober {
    running: Mutex<HashMap<String, Arc<Notify>>>,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|x| !x.is_empty()).map(String::from)
}

fn joined(v: &Value) -> Option<String> {
    let items: Vec<String> = v.as_array()?.iter().filter_map(s).collect();
    (!items.is_empty()).then(|| items.join(", "))
}

fn ymd(raw: &str) -> Option<String> {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    match d.len() {
        8 => Some(format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..])),
        4 => Some(d),
        _ => None,
    }
}

pub fn platform_name(extractor: &str) -> String {
    let e = extractor.to_ascii_lowercase();
    let known = [
        ("soundcloud", "SoundCloud"),
        ("youtube", "YouTube"),
        ("bandcamp", "Bandcamp"),
        ("vimeo", "Vimeo"),
        ("twitch", "Twitch"),
        ("mixcloud", "Mixcloud"),
        ("audiomack", "Audiomack"),
        ("tiktok", "TikTok"),
        ("twitter", "X"),
        ("instagram", "Instagram"),
        ("dailymotion", "Dailymotion"),
        ("archiveorg", "Internet Archive"),
        ("generic", "Web"),
    ];
    known
        .iter()
        .find(|(k, _)| e.starts_with(k))
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| extractor.trim_end_matches("IE").to_string())
}

fn summarize_formats(v: &Value) -> (MediaInfo, String) {
    let mut info = MediaInfo::default();
    let formats = v["formats"].as_array().cloned().unwrap_or_default();
    let usable: Vec<&Value> = formats
        .iter()
        .filter(|f| !f["has_drm"].as_bool().unwrap_or(false))
        .filter(|f| !s(&f["format_id"]).unwrap_or_default().contains("preview"))
        .collect();
    if formats.is_empty() {
        let has_v = s(&v["vcodec"]).is_some_and(|c| c != "none");
        info.has_video = has_v;
        info.best_abr = v["abr"].as_f64().map(|a| a.round() as u32);
        return (info, "ok".into());
    }
    if usable.is_empty() {
        let drm = formats.iter().any(|f| f["has_drm"].as_bool().unwrap_or(false));
        return (info, if drm { "drm" } else { "preview" }.into());
    }
    let lossless_codec = |c: &str| ["flac", "alac", "pcm", "wav", "aiff"].iter().any(|l| c.starts_with(l));
    let mut heights: Vec<u32> = Vec::new();
    let mut best: Option<(u32, String)> = None;
    for f in &usable {
        let vcodec = s(&f["vcodec"]).unwrap_or_else(|| "none".into());
        let acodec = s(&f["acodec"]).unwrap_or_else(|| "none".into());
        let id = s(&f["format_id"]).unwrap_or_default();
        if id == "download" {
            info.original = true;
            info.original_ext = s(&f["ext"]);
            if f["ext"].as_str().is_some_and(|e| ["wav", "flac", "aiff", "aif"].contains(&e)) {
                info.original_lossless = true;
            }
            continue;
        }
        if vcodec != "none" {
            info.has_video = true;
            if let Some(h) = f["height"].as_u64() {
                if !heights.contains(&(h as u32)) {
                    heights.push(h as u32);
                }
            }
        }
        if acodec != "none" {
            if lossless_codec(&acodec) {
                info.lossless = true;
            }
            let abr = f["abr"].as_f64().or_else(|| (vcodec == "none").then(|| f["tbr"].as_f64()).flatten());
            if let Some(a) = abr.map(|a| a.round() as u32) {
                if best.as_ref().is_none_or(|(b, _)| a > *b) {
                    best = Some((a, acodec.split('.').next().unwrap_or(&acodec).to_string()));
                }
            }
        }
    }
    heights.sort_unstable_by(|a, b| b.cmp(a));
    info.heights = heights;
    if let Some((abr, codec)) = best {
        info.best_abr = Some(abr);
        info.best_codec = Some(codec);
    }
    (info, "ok".into())
}

fn best_thumbnail(v: &Value) -> Option<String> {
    if let Some(list) = v["thumbnails"].as_array() {
        let pick = list
            .iter()
            .filter(|t| s(&t["url"]).is_some())
            .max_by_key(|t| (t["preference"].as_i64().unwrap_or(0), t["width"].as_u64().unwrap_or(0)));
        if let Some(t) = pick {
            return s(&t["url"]);
        }
    }
    s(&v["thumbnail"])
}

pub fn entry_from_info(v: &Value, index: u32) -> Entry {
    let extractor = s(&v["extractor_key"]).or_else(|| s(&v["ie_key"])).unwrap_or_default();
    let uploader = s(&v["uploader"]).or_else(|| s(&v["channel"])).unwrap_or_default();
    let has_formats = v["formats"].is_array() || v["vcodec"].is_string() || v["acodec"].is_string();
    let (media, mut availability) = if has_formats {
        let (m, a) = summarize_formats(v);
        (Some(m), a)
    } else {
        (None, "ok".into())
    };
    if let Some(a) = s(&v["availability"]) {
        if matches!(a.as_str(), "needs_auth" | "premium_only" | "subscriber_only") && availability == "ok" {
            availability = "restricted".into();
        }
    }
    let (desc_lyrics, desc_rest) = s(&v["description"]).map(|d| split_lyrics(&d)).unwrap_or((None, None));
    let release_date = s(&v["release_date"]).and_then(|d| ymd(&d)).or_else(|| s(&v["upload_date"]).and_then(|d| ymd(&d)));
    let year = v["release_year"]
        .as_u64()
        .map(|y| y as u16)
        .or_else(|| release_date.as_deref().and_then(|d| d.get(..4)?.parse().ok()));
    Entry {
        index,
        id: s(&v["id"]).unwrap_or_default(),
        url: s(&v["webpage_url"]).or_else(|| s(&v["original_url"])).or_else(|| s(&v["url"])).unwrap_or_default(),
        title: s(&v["track"]).or_else(|| s(&v["title"])).unwrap_or_else(|| "Untitled".into()),
        artist: s(&v["artist"])
            .or_else(|| joined(&v["artists"]))
            .or_else(|| s(&v["creator"]))
            .unwrap_or_else(|| uploader.clone()),
        uploader,
        album: s(&v["album"]),
        album_artist: s(&v["album_artist"]).or_else(|| joined(&v["album_artists"])),
        track_number: v["track_number"].as_u64().map(|n| n as u32),
        disc_number: v["disc_number"].as_u64().map(|n| n as u32),
        genre: s(&v["genre"]).or_else(|| joined(&v["genres"])),
        composer: s(&v["composer"]).or_else(|| joined(&v["composers"])),
        release_date,
        year,
        duration_ms: v["duration"].as_f64().map(|d| (d * 1000.0) as u64),
        thumbnail: best_thumbnail(v),
        license: s(&v["license"]),
        description: desc_rest,
        lyrics: desc_lyrics,
        platform: platform_name(&extractor),
        availability,
        detailed: has_formats,
        media,
    }
}

static LYRIC_HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*[\[\(]?\s*(lyrics|lyric|letra|letras|paroles|songtext|testo|歌词|歌詞|가사)\s*[\]\)]?\s*[:：\-–—]?\s*(.*)$").unwrap());
static SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*[\[\(]\s*(verse|chorus|hook|bridge|intro|outro|pre-chorus|refrain|interlude|breakdown|drop)[^\]\)]*[\]\)]\s*$").unwrap());
static END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(prod\.?|produced|producer|mixed|mix(ing)?\s*(&|and)?\s*master|mastered|mastering|engineered|artwork|cover art|art by|written by|composed|credits?|follow|stream|download|free download|buy|out now|available|booking|contact|management|label|instagram|insta|ig\b|twitter|tiktok|spotify|apple music|youtube|bandcamp|soundcloud|support|thanks|©|℗|copyright|\(c\)|https?://|www\.|@\w)").unwrap());
static LRC_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\[\d{1,3}:\d{1,2}(?:[.:]\d{1,3})?\]").unwrap());

fn tidy(lines: &[&str]) -> Option<String> {
    let text = lines.join("\n").trim_matches(|c: char| c == '\n' || c.is_whitespace()).to_string();
    let words = text.split_whitespace().count();
    (words >= 12).then_some(text)
}

pub fn split_lyrics(desc: &str) -> (Option<String>, Option<String>) {
    let lines: Vec<&str> = desc.lines().collect();
    let rest_of = |skip: std::ops::Range<usize>| {
        let r: Vec<&str> = lines.iter().enumerate().filter(|(i, _)| !skip.contains(i)).map(|(_, l)| *l).collect();
        let t = r.join("\n").trim().to_string();
        (!t.is_empty()).then_some(t)
    };
    let timed: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| LRC_LINE.is_match(l)).map(|(i, _)| i).collect();
    if timed.len() >= 4 {
        let (a, b) = (timed[0], timed[timed.len() - 1] + 1);
        if let Some(l) = tidy(&lines[a..b]) {
            return (Some(l), rest_of(a..b));
        }
    }
    if let Some(start) = lines.iter().position(|l| LYRIC_HEAD.is_match(l)) {
        let inline = LYRIC_HEAD.captures(lines[start]).and_then(|c| c.get(2)).map(|m| m.as_str().trim()).filter(|s| !s.is_empty());
        let mut end = start + 1;
        let mut blank_run = 0;
        while end < lines.len() {
            let l = lines[end].trim();
            if END.is_match(l) {
                break;
            }
            blank_run = if l.is_empty() { blank_run + 1 } else { 0 };
            if blank_run >= 3 {
                break;
            }
            end += 1;
        }
        let mut body: Vec<&str> = Vec::new();
        if let Some(i) = inline {
            body.push(i);
        }
        body.extend_from_slice(&lines[start + 1..end]);
        if let Some(l) = tidy(&body) {
            return (Some(l), rest_of(start..end));
        }
    }
    let sections = lines.iter().filter(|l| SECTION.is_match(l)).count();
    if sections >= 2 {
        let first = lines.iter().position(|l| SECTION.is_match(l)).unwrap_or(0);
        let mut end = first;
        while end < lines.len() && !END.is_match(lines[end].trim()) {
            end += 1;
        }
        if let Some(l) = tidy(&lines[first..end]) {
            return (Some(l), rest_of(first..end));
        }
    }
    let t = desc.trim();
    (None, (!t.is_empty()).then(|| t.chars().take(4000).collect()))
}

fn common<T: PartialEq + Clone>(entries: &[Entry], f: impl Fn(&Entry) -> Option<T>) -> Option<T> {
    let first = f(entries.first()?)?;
    entries.iter().all(|e| f(e).as_ref() == Some(&first)).then_some(first)
}

fn by_length(entries: &[Entry]) -> &'static str {
    let minutes = entries.iter().filter_map(|e| e.duration_ms).sum::<u64>() as f64 / 60000.0;
    match entries.len() {
        n if n <= 3 && minutes < 30.0 => "single",
        n if n <= 6 && minutes < 30.0 => "ep",
        _ => "album",
    }
}

static TITLE_RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?i)\b(compilation|various artists|v\.?a\.?)\b", "compilation"),
        (r"(?i)\bmix ?tape\b", "mixtape"),
        (r"(?i)\be\.?p\.?\b", "ep"),
        (r"(?i)\bl\.?p\.?\b", "lp"),
        (r"(?i)\bsingle\b", "single"),
        (r"(?i)\b(deluxe|album|remaster(ed)?|anniversary edition)\b", "album"),
    ]
    .into_iter()
    .map(|(p, k)| (Regex::new(p).unwrap(), k))
    .collect()
});

pub fn classify(extractor: &str, playlist_id: &str, url: &str, title: &str, entries: &[Entry]) -> (String, String) {
    let ex = extractor.to_ascii_lowercase();
    if ex.contains("soundclouduser") || ex.contains("soundcloud:user") {
        return ("profile".into(), "platform".into());
    }
    if ex.starts_with("youtubetab") && (playlist_id.starts_with("UC") || url.contains("/@") || url.contains("/channel/")) {
        return ("channel".into(), "platform".into());
    }
    for (re, kind) in TITLE_RULES.iter() {
        if re.is_match(title) {
            return ((*kind).into(), "title".into());
        }
    }
    if ex.starts_with("bandcampalbum") || ex == "bandcamp:album" || playlist_id.starts_with("OLAK5uy_") {
        return (by_length(entries).into(), "platform".into());
    }
    let norm = |x: &str| x.trim().to_lowercase();
    let shared_album = common(entries, |e| e.album.as_deref().map(norm));
    let one_artist = common(entries, |e| Some(e.artist.to_lowercase())).is_some();
    let numbered = entries.len() > 1 && entries.iter().enumerate().all(|(i, e)| e.track_number == Some(i as u32 + 1));
    if shared_album.as_deref().is_some_and(|a| a == norm(title)) || (shared_album.is_some() && one_artist) || (numbered && one_artist) {
        return (by_length(entries).into(), "metadata".into());
    }
    ("playlist".into(), "none".into())
}

fn release_artist(kind: &str, fallback: &str, entries: &[Entry]) -> String {
    if let Some(a) = common(entries, |e| e.album_artist.clone()) {
        return a;
    }
    if let Some(a) = common(entries, |e| Some(e.artist.clone())) {
        return a;
    }
    if kind == "compilation" {
        return "Various Artists".into();
    }
    fallback.to_string()
}

fn browser_label(id: &str) -> String {
    match id.to_ascii_lowercase().as_str() {
        "chrome" => "Chrome".into(),
        "chromium" => "Chromium".into(),
        "edge" => "Edge".into(),
        "firefox" => "Firefox".into(),
        "brave" => "Brave".into(),
        "opera" => "Opera".into(),
        "vivaldi" => "Vivaldi".into(),
        "safari" => "Safari".into(),
        "whale" => "Whale".into(),
        _ => "your browser".into(),
    }
}

pub fn cookie_error(stderr: &str) -> Option<String> {
    let lower = stderr.to_lowercase();
    if !lower.contains("cookie") {
        return None;
    }
    static FROM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)extracting cookies from (\w+)").unwrap());
    let b = FROM.captures(stderr).map(|c| browser_label(&c[1])).unwrap_or_else(|| "your browser".into());
    if lower.contains("could not copy") || lower.contains("permission denied") || lower.contains("database is locked") {
        Some(format!("{b} keeps its sign-in locked while it's running. Quit {b} completely (check the system tray too), then try again. Firefox can stay open, or use a cookies.txt file instead."))
    } else if lower.contains("could not find") && lower.contains("cookies database") {
        Some(format!("Couldn't find a {b} profile on this computer. Choose the browser you actually sign in with."))
    } else if lower.contains("decrypt") || lower.contains("dpapi") || lower.contains("app-bound") || lower.contains("keyring") {
        Some(format!("{b} encrypts its sign-in so other apps can't read it. Firefox works, or export a cookies.txt file from {b} and choose it here."))
    } else if lower.contains("netscape") || lower.contains("cookiefile") || (lower.contains("cookies") && lower.contains("no such file")) {
        Some("That cookies file couldn't be read. It needs to be a cookies.txt export in Netscape format.".into())
    } else {
        None
    }
}

pub fn humanize(stderr: &str) -> String {
    if let Some(c) = cookie_error(stderr) {
        return c;
    }
    let err = stderr
        .lines()
        .rev()
        .find(|l| l.contains("ERROR"))
        .map(|l| l.trim().trim_start_matches("ERROR:").trim().to_string())
        .unwrap_or_else(|| stderr.lines().last().unwrap_or("Unknown error").trim().to_string());
    let lower = err.to_lowercase();
    if lower.contains("unsupported url") {
        "Atmosphere can't read that link. The site isn't supported.".into()
    } else if lower.contains("drm") {
        "This stream is DRM-encrypted, so Atmosphere can't read it. If it's your own track, turn on downloads for it in the platform's track settings (or enable “Use my browser sign-in”), then choose Original file to get your uploaded master.".into()
    } else if lower.contains("sign in") || lower.contains("private") || lower.contains("login") || lower.contains("members-only") {
        format!("{err} — if this is your own content, enable “Use my browser sign-in” in Settings.")
    } else if lower.contains("404") || lower.contains("not found") || lower.contains("does not exist") {
        "Not found — the link may be private, removed or mistyped.".into()
    } else if lower.contains("429") || lower.contains("too many requests") {
        "The site is rate-limiting requests. Wait a bit and try again.".into()
    } else {
        static PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[[^\]]+\]\s*(\S+:\s+)?").unwrap());
        let cleaned = PREFIX.replace(&err, "").trim().chars().take(300).collect::<String>();
        if cleaned.is_empty() { err } else { cleaned }
    }
}

pub fn base_args(settings: &Settings, tools: &Tools) -> Vec<String> {
    let mut args: Vec<String> = ["--ignore-config", "--no-warnings", "--encoding", "utf-8", "--no-color"].map(String::from).into();
    if let Some(ff) = &tools.ffmpeg {
        args.push("--ffmpeg-location".into());
        args.push(ff.to_string_lossy().into_owned());
    }
    match settings.cookies_browser.trim() {
        "" => {}
        "file" => {
            if !settings.cookies_file.trim().is_empty() {
                args.push("--cookies".into());
                args.push(settings.cookies_file.trim().into());
            }
        }
        browser => {
            args.push("--cookies-from-browser".into());
            args.push(browser.into());
        }
    }
    if !settings.proxy.trim().is_empty() {
        args.push("--proxy".into());
        args.push(settings.proxy.trim().into());
    }
    args
}

async fn run_json(tools: &Tools, args: Vec<String>) -> Result<Value> {
    let out = engine::command(&tools.ytdlp).args(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).output().await?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!(humanize(&String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| anyhow!("Unexpected response from yt-dlp ({e})"))
}

fn collection_from(v: &Value, id: String, link: &str) -> Collection {
    let extractor = s(&v["extractor_key"]).unwrap_or_default();
    let is_list = matches!(v["_type"].as_str(), Some("playlist") | Some("multi_video"));
    if !is_list {
        let entry = entry_from_info(v, 1);
        return Collection {
            id,
            single: true,
            release_type: "track".into(),
            release_type_source: "none".into(),
            title: entry.title.clone(),
            artist: entry.artist.clone(),
            url: entry.url.clone(),
            thumbnail: entry.thumbnail.clone(),
            platform: entry.platform.clone(),
            year: entry.year,
            entries: vec![entry],
            enriching: false,
        };
    }
    let entries: Vec<Entry> = v["entries"]
        .as_array()
        .map(|list| {
            list.iter()
                .enumerate()
                .filter(|(_, e)| !e.is_null())
                .map(|(i, e)| {
                    let mut entry = entry_from_info(e, i as u32 + 1);
                    if entry.platform.is_empty() {
                        entry.platform = platform_name(&extractor);
                    }
                    entry
                })
                .collect()
        })
        .unwrap_or_default();
    let title = s(&v["title"]).unwrap_or_else(|| "Untitled".into());
    let pid = s(&v["id"]).unwrap_or_default();
    let url = s(&v["webpage_url"]).unwrap_or_else(|| link.to_string());
    let (release_type, source) = classify(&extractor, &pid, &url, &title, &entries);
    let owner = s(&v["uploader"]).or_else(|| s(&v["channel"])).unwrap_or_default();
    let artist = release_artist(&release_type, &owner, &entries);
    let thumbnail = best_thumbnail(v).or_else(|| entries.iter().find_map(|e| e.thumbnail.clone()));
    let needs_detail = entries.iter().any(|e| !e.detailed);
    Collection {
        id,
        single: false,
        release_type,
        release_type_source: source,
        title,
        artist,
        url,
        thumbnail,
        platform: platform_name(&extractor),
        year: v["release_year"].as_u64().map(|y| y as u16).or_else(|| entries.iter().find_map(|e| e.year)),
        entries,
        enriching: needs_detail,
    }
}

impl Prober {
    pub async fn probe(self: &Arc<Self>, app: &AppHandle, settings: &Settings, link: &str) -> Result<Collection> {
        let tools = engine::locate(&settings.ytdlp_path, &settings.ffmpeg_path)?;
        let mut args = base_args(settings, &tools);
        args.extend(["-J", "--flat-playlist", "--", link].map(String::from));
        let v = run_json(&tools, args).await?;
        let id = format!("c{}", NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        let mut collection = collection_from(&v, id.clone(), link);
        if crate::direct::is_direct(&v) {
            if let Some(entry) = collection.entries.first_mut() {
                crate::direct::enrich(&tools, settings, &v, entry).await?;
                collection.title = entry.title.clone();
                collection.artist = entry.artist.clone();
                collection.thumbnail = entry.thumbnail.clone();
                collection.platform = entry.platform.clone();
                collection.year = entry.year;
            }
        }
        if collection.enriching {
            self.enrich(app.clone(), settings.clone(), tools, collection.clone(), v);
        }
        Ok(collection)
    }

    pub async fn search(&self, settings: &Settings, platform: &str, query: &str) -> Result<Collection> {
        let tools = engine::locate(&settings.ytdlp_path, &settings.ffmpeg_path)?;
        let prefix = match platform {
            "soundcloud" => "scsearch20",
            _ => "ytsearch20",
        };
        let mut args = base_args(settings, &tools);
        args.extend(["-J".into(), "--flat-playlist".into(), "--".into(), format!("{prefix}:{query}")]);
        let v = run_json(&tools, args).await?;
        let id = format!("s{}", NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        let mut c = collection_from(&v, id, query);
        c.release_type = "search".into();
        c.release_type_source = "none".into();
        c.title = query.to_string();
        c.enriching = false;
        Ok(c)
    }

    pub async fn detail(&self, settings: &Settings, url: &str, index: u32) -> Result<Entry> {
        let tools = engine::locate(&settings.ytdlp_path, &settings.ffmpeg_path)?;
        let mut args = base_args(settings, &tools);
        args.extend(["-J", "--no-playlist", "--", url].map(String::from));
        let v = run_json(&tools, args).await?;
        let mut entry = entry_from_info(&v, index);
        if crate::direct::is_direct(&v) {
            crate::direct::enrich(&tools, settings, &v, &mut entry).await?;
        }
        Ok(entry)
    }

    pub fn cancel(&self, id: &str) {
        if let Some(n) = self.running.lock().unwrap().remove(id) {
            n.notify_one();
        }
    }

    fn enrich(self: &Arc<Self>, app: AppHandle, settings: Settings, tools: Tools, collection: Collection, raw: Value) {
        let stop = Arc::new(Notify::new());
        self.running.lock().unwrap().insert(collection.id.clone(), stop.clone());
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut args = base_args(&settings, &tools);
            args.extend(["-j", "--ignore-errors", "--yes-playlist", "--", &collection.url].map(String::from));
            let mut entries = collection.entries.clone();
            let mut failed = 0u32;
            let spawned = engine::command(&tools.ytdlp).args(&args).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn();
            if let Ok(mut child) = spawned {
                if let Some(stdout) = child.stdout.take() {
                    let mut lines = BufReader::new(stdout).lines();
                    loop {
                        tokio::select! {
                            _ = stop.notified() => { let _ = child.kill().await; return; }
                            line = lines.next_line() => {
                                let Ok(Some(line)) = line else { break };
                                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                                let idx = v["playlist_index"].as_u64().map(|i| i as u32);
                                let vid = s(&v["id"]).unwrap_or_default();
                                let slot = entries.iter().position(|e| Some(e.index) == idx).or_else(|| entries.iter().position(|e| e.id == vid));
                                if let Some(pos) = slot {
                                    let entry = entry_from_info(&v, entries[pos].index);
                                    entries[pos] = entry.clone();
                                    let _ = app.emit(PROBE_EVENT, ProbeEvent::Entry { collection: collection.id.clone(), entry: Box::new(entry) });
                                }
                            }
                        }
                    }
                }
                let _ = child.wait().await;
            }
            failed += entries.iter().filter(|e| !e.detailed).count() as u32;
            let extractor = s(&raw["extractor_key"]).unwrap_or_default();
            let pid = s(&raw["id"]).unwrap_or_default();
            let (release_type, source) = classify(&extractor, &pid, &collection.url, &collection.title, &entries);
            let owner = s(&raw["uploader"]).or_else(|| s(&raw["channel"])).unwrap_or_default();
            let artist = release_artist(&release_type, &owner, &entries);
            let year = entries.iter().find_map(|e| e.year);
            let _ = app.emit(
                PROBE_EVENT,
                ProbeEvent::Done { collection: collection.id.clone(), release_type, release_type_source: source, artist, year, failed },
            );
            this.running.lock().unwrap().remove(&collection.id);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn e(artist: &str, album: Option<&str>, mins: u64, n: Option<u32>) -> Entry {
        Entry {
            artist: artist.into(),
            album: album.map(String::from),
            duration_ms: Some(mins * 60000),
            track_number: n,
            ..Default::default()
        }
    }

    #[test]
    fn classifies_by_title_platform_and_metadata() {
        let five: Vec<_> = (0..5).map(|i| e("Nova", Some("Glass"), 4, Some(i + 1))).collect();
        assert_eq!(classify("SoundcloudSet", "1", "", "Glass", &five), ("ep".into(), "metadata".into()));
        let ten: Vec<_> = (0..10).map(|i| e("Nova", Some("Glass"), 4, Some(i + 1))).collect();
        assert_eq!(classify("SoundcloudSet", "1", "", "Glass", &ten).0, "album");
        assert_eq!(classify("SoundcloudSet", "1", "", "Midnight Drive EP", &ten), ("ep".into(), "title".into()));
        assert_eq!(classify("SoundcloudSet", "1", "", "Sunset LP", &ten).0, "lp");
        assert_eq!(classify("SoundcloudSet", "1", "", "Summer Mixtape 2024", &ten).0, "mixtape");
        assert_eq!(classify("BandcampAlbum", "1", "", "Glass", &ten), ("album".into(), "platform".into()));
        assert_eq!(classify("SoundcloudUser", "1", "", "Nova", &ten).0, "profile");
        let mixed = vec![e("A", None, 3, None), e("B", None, 3, None)];
        assert_eq!(classify("SoundcloudSet", "1", "", "Faves", &mixed).0, "playlist");
        assert_eq!(classify("SoundcloudSet", "1", "", "Help", &mixed).0, "playlist");
        assert_eq!(release_artist("compilation", "owner", &mixed), "Various Artists");
    }

    #[test]
    fn summarizes_formats_honestly() {
        let v = json!({
            "id": "1", "title": "Song", "extractor_key": "Soundcloud", "webpage_url": "https://soundcloud.com/a/b",
            "upload_date": "20210314", "duration": 200.5,
            "formats": [
                {"format_id": "hls_mp3_1_0", "acodec": "mp3", "vcodec": "none", "abr": 128, "ext": "mp3"},
                {"format_id": "hls_aac_160k", "acodec": "aac", "vcodec": "none", "abr": 160, "ext": "m4a"},
                {"format_id": "download", "ext": "wav", "acodec": "none", "vcodec": "none"},
                {"format_id": "x", "acodec": "aac", "vcodec": "none", "abr": 256, "has_drm": true}
            ]
        });
        let entry = entry_from_info(&v, 1);
        let m = entry.media.unwrap();
        assert_eq!(m.best_abr, Some(160));
        assert!(m.original && m.original_lossless && !m.lossless && !m.has_video);
        assert_eq!(entry.release_date.as_deref(), Some("2021-03-14"));
        assert_eq!(entry.platform, "SoundCloud");
        assert_eq!(entry.duration_ms, Some(200500));
        let drm = entry_from_info(&json!({"formats": [{"format_id": "a", "has_drm": true}]}), 1);
        assert_eq!(drm.availability, "drm");
    }

    #[test]
    fn splits_lyrics_from_descriptions() {
        let d = "New single out now!\n\nLyrics:\nHeadlights on the overpass\nEvery window holds a star\n\nWe don't need to say it twice\nNight drive keep the city close\n\nProd. by Kai Rivers\nInstagram: @novalumen\nhttps://example.com";
        let (l, rest) = split_lyrics(d);
        let l = l.unwrap();
        assert!(l.starts_with("Headlights") && l.ends_with("city close"));
        let rest = rest.unwrap();
        assert!(rest.contains("out now") && rest.contains("Prod. by") && !rest.contains("Headlights"));
        let timed = "Stream everywhere\n[00:01.00]One two three four\n[00:04.00]Five six seven eight\n[00:08.00]Nine ten eleven twelve\n[00:12.00]Thirteen fourteen fifteen";
        let (l, rest) = split_lyrics(timed);
        assert!(l.unwrap().starts_with("[00:01.00]"));
        assert_eq!(rest.as_deref(), Some("Stream everywhere"));
        let sections = "[Verse 1]\nI walk the line between the night and day\nCount the lights along the way\n[Chorus]\nHold on hold on we're nearly home\nHold on we're not alone\nFollow me on twitter";
        let (l, _) = split_lyrics(sections);
        assert!(l.unwrap().contains("[Chorus]"));
        let promo = "Free download! Mixed and mastered by me. Thanks for 10k plays";
        let (l, rest) = split_lyrics(promo);
        assert!(l.is_none());
        assert_eq!(rest.as_deref(), Some(promo));
    }

    #[test]
    fn humanizes_errors() {
        assert!(humanize("ERROR: Unsupported URL: https://x").contains("isn't supported"));
        assert!(humanize("ERROR: [youtube] abc: Private video. Sign in").contains("browser sign-in"));
    }
}

pub fn count_cookie_lines(text: &str) -> usize {
    text.lines()
        .map(|l| l.strip_prefix("#HttpOnly_").unwrap_or(l))
        .filter(|l| !l.starts_with('#') && l.split('\t').count() == 7)
        .count()
}

pub async fn check_signin(settings: &Settings) -> Result<String> {
    match settings.cookies_browser.trim() {
        "" => bail!("Choose a browser or a cookies file first."),
        "file" => {
            let path = settings.cookies_file.trim();
            if path.is_empty() {
                bail!("Choose your cookies.txt file first.");
            }
            let text = tokio::fs::read_to_string(path).await.map_err(|_| anyhow!("That cookies file couldn't be opened."))?;
            match count_cookie_lines(&text) {
                0 => bail!("That file has no cookies in it. Export a cookies.txt file in Netscape format while you're signed in."),
                n => Ok(format!("Found {n} cookies in that file. You're set.")),
            }
        }
        browser => {
            let tools = engine::locate(&settings.ytdlp_path, &settings.ffmpeg_path)?;
            let mut args = base_args(settings, &tools);
            args.extend(["--simulate", "--no-playlist", "--", "https://example.com/"].map(String::from));
            let run = engine::command(&tools.ytdlp).args(&args).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true).output();
            let out = tokio::time::timeout(std::time::Duration::from_secs(60), run).await.map_err(|_| anyhow!("Checking took too long. Try again."))??;
            let err = String::from_utf8_lossy(&out.stderr);
            static OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)extracted (\d+) cookies from").unwrap());
            if let Some(c) = OK.captures(&err) {
                return Ok(format!("Read your {} sign-in ({} cookies). You're set.", browser_label(browser), &c[1]));
            }
            bail!(cookie_error(&err).unwrap_or_else(|| format!("Couldn't read the {} sign-in. {}", browser_label(browser), humanize(&err))))
        }
    }
}

#[cfg(test)]
mod signin_tests {
    use super::*;

    #[test]
    fn explains_locked_and_encrypted_browsers() {
        let locked = "Extracting cookies from brave\nERROR: Could not copy Chrome cookie database. See https://github.com/yt-dlp/yt-dlp/issues/7271";
        assert!(humanize(locked).starts_with("Brave keeps its sign-in locked"));
        let enc = "Extracting cookies from chrome\nERROR: Failed to decrypt with DPAPI";
        assert!(cookie_error(enc).unwrap().starts_with("Chrome encrypts"));
        assert!(cookie_error("ERROR: Unsupported URL").is_none());
    }

    #[test]
    fn counts_netscape_cookie_lines() {
        let t = "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\ta\tb\n#HttpOnly_.x.com\tTRUE\t/\tTRUE\t0\tc\td\nnot a cookie";
        assert_eq!(count_cookie_lines(t), 2);
    }
}
