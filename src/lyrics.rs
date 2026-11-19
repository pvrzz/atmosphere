// atmosphere · lyrics.rs · Lyrics lookup across providers (local, LRCLIB, Musixmatch, NetEase, Kugou, custom) with LRC parsing and caching.
use crate::engine::data_dir;
use crate::library::hash;
use crate::settings::Settings;
use anyhow::{anyhow, Result};
use base64::Engine as _;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CACHE_DAYS: u64 = 14;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: Option<u64>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub t: u64,
    pub d: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub t: Option<u64>,
    pub text: String,
    pub words: Vec<Word>,
    pub translation: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub provider: String,
    pub synced: bool,
    pub word_synced: bool,
    pub lines: Vec<Line>,
    pub copyright: Option<String>,
    pub instrumental: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub provider: String,
    pub outcome: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub lyrics: Option<Lyrics>,
    pub tried: Vec<Attempt>,
    pub cached: bool,
}

static JUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*[\(\[][^)\]]*\b(official|video|audio|lyrics?|visuali[sz]er|music video|mv|m/v|hd|hq|4k|explicit|clean|full song)\b[^)\]]*[\)\]]").unwrap()
});
static TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*[|｜/]\s*.*$|\s+(official|lyrics?)\s*(video|audio)?\s*$").unwrap());

pub fn clean(q: &Query) -> Query {
    let mut title = JUNK.replace_all(q.title.trim(), "").to_string();
    title = TAIL.replace_all(&title, "").trim().to_string();
    let mut artist = q.artist.trim().trim_end_matches(" - Topic").to_string();
    if let Some((a, t)) = title.split_once(" - ").map(|(a, t)| (a.trim().to_string(), t.trim().to_string())) {
        let lower_a = a.to_lowercase();
        let related = artist.is_empty() || lower_a.contains(&artist.to_lowercase()) || artist.to_lowercase().contains(&lower_a);
        if related && !a.is_empty() && !t.is_empty() {
            artist = a;
            title = t;
        }
    }
    Query { title: if title.is_empty() { q.title.clone() } else { title }, artist, ..q.clone() }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderCfg {
    pub id: String,
    pub enabled: bool,
}

impl Default for ProviderCfg {
    fn default() -> Self {
        Self { id: String::new(), enabled: true }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub url: String,
    pub headers: String,
    pub format: String,
    pub json_path: String,
    pub key: String,
}

pub fn default_providers() -> Vec<ProviderCfg> {
    [("local", true), ("lrclib", true), ("netease", true), ("musixmatch", false), ("kugou", false)]
        .into_iter()
        .map(|(id, enabled)| ProviderCfg { id: id.into(), enabled })
        .collect()
}

static TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\]").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?>([^<]*)").unwrap());
static OFFSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)^\[offset:\s*([+-]?\d+)\]").unwrap());
static YRC_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[(\d+),(\d+)\](.*)$").unwrap());
static YRC_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d+),(\d+),\d+\)([^(]*)").unwrap());

fn ms(m: &str, s: &str, frac: Option<&str>) -> u64 {
    let f = frac.map(|f| match f.len() {
        1 => f.parse::<u64>().unwrap_or(0) * 100,
        2 => f.parse::<u64>().unwrap_or(0) * 10,
        _ => f[..3].parse::<u64>().unwrap_or(0),
    });
    m.parse::<u64>().unwrap_or(0) * 60_000 + s.parse::<u64>().unwrap_or(0) * 1000 + f.unwrap_or(0)
}

pub fn parse_lrc(text: &str) -> Vec<Line> {
    let offset: i64 = OFFSET.captures(text).and_then(|c| c[1].parse().ok()).unwrap_or(0);
    let shift = |t: u64| (t as i64 - offset).max(0) as u64;
    let mut lines = Vec::new();
    let mut plain = Vec::new();
    for raw in text.lines() {
        let stamps: Vec<u64> = TIME.captures_iter(raw).map(|c| shift(ms(&c[1], &c[2], c.get(3).map(|m| m.as_str())))).collect();
        let body = TIME.replace_all(raw, "").trim().to_string();
        if stamps.is_empty() {
            if !raw.trim().is_empty() && !raw.trim_start().starts_with('[') {
                plain.push(Line { t: None, text: raw.trim().to_string(), ..Default::default() });
            }
            continue;
        }
        let mut words: Vec<Word> = WORD
            .captures_iter(&body)
            .map(|c| Word { t: shift(ms(&c[1], &c[2], c.get(3).map(|m| m.as_str()))), d: 0, text: c[4].to_string() })
            .filter(|w| !w.text.is_empty())
            .collect();
        for i in 0..words.len() {
            let next = words.get(i + 1).map(|n| n.t);
            if let Some(n) = next {
                words[i].d = n.saturating_sub(words[i].t);
            }
        }
        let text = if words.is_empty() { body } else { words.iter().map(|w| w.text.as_str()).collect::<String>().trim().to_string() };
        for t in stamps {
            lines.push(Line { t: Some(t), text: text.clone(), words: words.clone(), translation: None });
        }
    }
    if lines.is_empty() {
        return plain;
    }
    lines.sort_by_key(|l| l.t);
    lines
}

pub fn parse_yrc(text: &str) -> Vec<Line> {
    text.lines()
        .filter_map(|l| {
            let c = YRC_LINE.captures(l.trim())?;
            let words: Vec<Word> = YRC_WORD.captures_iter(&c[3]).map(|w| Word { t: w[1].parse().unwrap_or(0), d: w[2].parse().unwrap_or(0), text: w[3].to_string() }).collect();
            let text = words.iter().map(|w| w.text.as_str()).collect::<String>().trim().to_string();
            (!text.is_empty()).then(|| Line { t: c[1].parse().ok(), text, words, translation: None })
        })
        .collect()
}

fn build(provider: &str, lines: Vec<Line>, copyright: Option<String>) -> Option<Lyrics> {
    let lines: Vec<Line> = lines.into_iter().filter(|l| !l.text.trim().is_empty() || l.t.is_some()).collect();
    if lines.iter().all(|l| l.text.trim().is_empty()) {
        return None;
    }
    let synced = lines.iter().any(|l| l.t.is_some());
    let word_synced = lines.iter().any(|l| l.words.len() > 1);
    Some(Lyrics { provider: provider.into(), synced, word_synced, lines, copyright, instrumental: false })
}

fn norm(s: &str) -> String {
    static PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*[\(\[][^)\]]*(feat|ft\.|remaster|version|edit|live|mix)[^)\]]*[\)\]]").unwrap());
    PAREN.replace_all(s, "").to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn score(q: &Query, title: &str, artist: &str, duration_ms: Option<u64>) -> i64 {
    let (qt, qa, t, a) = (norm(&q.title), norm(&q.artist), norm(title), norm(artist));
    let mut s = 0;
    if t == qt {
        s += 50;
    } else if t.contains(&qt) || qt.contains(&t) {
        s += 25;
    } else {
        return -100;
    }
    if !qa.is_empty() && (a.contains(&qa) || qa.contains(&a) || qa.split(' ').any(|w| w.len() > 2 && a.contains(w))) {
        s += 30;
    }
    if let (Some(x), Some(y)) = (q.duration_ms, duration_ms) {
        let diff = (x as i64 - y as i64).abs();
        if diff > 8000 {
            return -100;
        }
        s += 20 - (diff / 500).min(20);
    }
    s
}

fn local(q: &Query) -> Option<Lyrics> {
    let path = Path::new(q.path.as_deref()?);
    if let Ok(text) = std::fs::read_to_string(path.with_extension("lrc")) {
        if let Some(l) = build("Local .lrc", parse_lrc(&text), None) {
            return Some(l);
        }
    }
    let file = lofty::read_from_path(path).ok()?;
    use lofty::file::TaggedFileExt;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let text = tag.get_string(lofty::tag::ItemKey::Lyrics)?;
    build("Embedded", parse_lrc(text), None)
}

async fn get_json(http: &reqwest::Client, url: &str, headers: &[(String, String)]) -> Result<Value> {
    let mut req = http.get(url).timeout(Duration::from_secs(10));
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await?;
    if resp.status().as_u16() == 404 {
        return Ok(Value::Null);
    }
    Ok(resp.error_for_status()?.json().await?)
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

async fn lrclib(http: &reqwest::Client, q: &Query) -> Result<Option<Lyrics>> {
    let from = |v: &Value| -> Option<Lyrics> {
        if v["instrumental"].as_bool() == Some(true) {
            return Some(Lyrics { provider: "LRCLIB".into(), instrumental: true, ..Default::default() });
        }
        v["syncedLyrics"].as_str().and_then(|s| build("LRCLIB", parse_lrc(s), None)).or_else(|| v["plainLyrics"].as_str().and_then(|s| build("LRCLIB", parse_lrc(s), None)))
    };
    let mut url = format!("https://lrclib.net/api/get?track_name={}&artist_name={}", enc(&q.title), enc(&q.artist));
    if !q.album.is_empty() {
        url.push_str(&format!("&album_name={}", enc(&q.album)));
    }
    if let Some(d) = q.duration_ms {
        url.push_str(&format!("&duration={}", d / 1000));
    }
    let v = get_json(http, &url, &[]).await?;
    if let Some(l) = from(&v) {
        return Ok(Some(l));
    }
    let mut list = get_json(http, &format!("https://lrclib.net/api/search?track_name={}&artist_name={}", enc(&q.title), enc(&q.artist)), &[]).await?;
    if list.as_array().is_none_or(|a| a.is_empty()) {
        list = get_json(http, &format!("https://lrclib.net/api/search?q={}", enc(&format!("{} {}", q.artist, q.title))), &[]).await?;
    }
    let best = list.as_array().and_then(|a| {
        a.iter()
            .map(|v| (score(q, v["trackName"].as_str().unwrap_or(""), v["artistName"].as_str().unwrap_or(""), v["duration"].as_f64().map(|d| (d * 1000.0) as u64)), v))
            .filter(|(s, _)| *s >= 40)
            .max_by_key(|(s, v)| (*s, v["syncedLyrics"].is_string() as i64))
            .map(|(_, v)| v.clone())
    });
    Ok(best.as_ref().and_then(from))
}

async fn musixmatch(http: &reqwest::Client, q: &Query, key: &str) -> Result<Option<Lyrics>> {
    if key.trim().is_empty() {
        return Ok(None);
    }
    let base = "https://api.musixmatch.com/ws/1.1";
    let common = format!("q_track={}&q_artist={}&apikey={}", enc(&q.title), enc(&q.artist), enc(key.trim()));
    let mut sub = format!("{base}/matcher.subtitle.get?{common}");
    if let Some(d) = q.duration_ms {
        sub.push_str(&format!("&f_subtitle_length={}&f_subtitle_length_max_deviation=5", d / 1000));
    }
    let v = get_json(http, &sub, &[]).await?;
    if v["message"]["header"]["status_code"].as_u64() == Some(401) {
        return Err(anyhow!("Musixmatch rejected the API key"));
    }
    let body = &v["message"]["body"]["subtitle"];
    if let Some(text) = body["subtitle_body"].as_str() {
        if let Some(l) = build("Musixmatch", parse_lrc(text), body["lyrics_copyright"].as_str().map(String::from)) {
            return Ok(Some(l));
        }
    }
    let v = get_json(http, &format!("{base}/matcher.lyrics.get?{common}"), &[]).await?;
    let body = &v["message"]["body"]["lyrics"];
    if body["instrumental"].as_u64() == Some(1) {
        return Ok(Some(Lyrics { provider: "Musixmatch".into(), instrumental: true, ..Default::default() }));
    }
    let text = body["lyrics_body"].as_str().unwrap_or("");
    let cleaned: String = text.lines().take_while(|l| !l.contains("*******")).collect::<Vec<_>>().join("\n");
    Ok(build("Musixmatch", parse_lrc(&cleaned), body["lyrics_copyright"].as_str().map(String::from)))
}

async fn netease(http: &reqwest::Client, q: &Query) -> Result<Option<Lyrics>> {
    let search = get_json(http, &format!("https://music.163.com/api/search/get?s={}&type=1&limit=8", enc(&format!("{} {}", q.artist, q.title))), &[]).await?;
    let best = search["result"]["songs"].as_array().and_then(|songs| {
        songs
            .iter()
            .map(|s| {
                let artist = s["artists"].as_array().map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                (score(q, s["name"].as_str().unwrap_or(""), &artist, s["duration"].as_u64()), s["id"].as_u64())
            })
            .filter(|(sc, _)| *sc >= 45)
            .max_by_key(|(sc, _)| *sc)
            .and_then(|(_, id)| id)
    });
    let Some(id) = best else { return Ok(None) };
    let v = get_json(http, &format!("https://music.163.com/api/song/lyric?id={id}&lv=1&tv=-1&yv=1"), &[]).await?;
    let mut lines = v["yrc"]["lyric"].as_str().map(parse_yrc).filter(|l| !l.is_empty()).unwrap_or_else(|| v["lrc"]["lyric"].as_str().map(parse_lrc).unwrap_or_default());
    if let Some(tr) = v["tlyric"]["lyric"].as_str().map(parse_lrc) {
        for line in lines.iter_mut() {
            if let Some(t) = line.t {
                line.translation = tr.iter().find(|x| x.t.is_some_and(|xt| xt.abs_diff(t) < 600)).map(|x| x.text.clone()).filter(|s| !s.is_empty());
            }
        }
    }
    Ok(build("NetEase Cloud Music", lines, None))
}

async fn kugou(http: &reqwest::Client, q: &Query) -> Result<Option<Lyrics>> {
    let mut url = format!("https://lyrics.kugou.com/search?ver=1&man=yes&client=pc&keyword={}", enc(&format!("{} - {}", q.artist, q.title)));
    if let Some(d) = q.duration_ms {
        url.push_str(&format!("&duration={d}"));
    }
    let v = get_json(http, &url, &[]).await?;
    let best = v["candidates"].as_array().and_then(|c| {
        c.iter()
            .map(|x| (score(q, x["song"].as_str().unwrap_or(""), x["singer"].as_str().unwrap_or(""), x["duration"].as_u64()), x))
            .filter(|(s, _)| *s >= 45)
            .max_by_key(|(s, _)| *s)
            .map(|(_, x)| x.clone())
    });
    let Some(c) = best else { return Ok(None) };
    let dl = format!(
        "https://lyrics.kugou.com/download?ver=1&client=pc&id={}&accesskey={}&fmt=lrc&charset=utf8",
        enc(&c["id"].as_str().map(String::from).unwrap_or_else(|| c["id"].to_string())),
        enc(c["accesskey"].as_str().unwrap_or(""))
    );
    let v = get_json(http, &dl, &[]).await?;
    let Some(content) = v["content"].as_str() else { return Ok(None) };
    let bytes = base64::engine::general_purpose::STANDARD.decode(content.trim())?;
    Ok(build("Kugou", parse_lrc(&String::from_utf8_lossy(&bytes)), None))
}

fn json_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').filter(|p| !p.is_empty()).try_fold(v, |cur, part| match part.parse::<usize>() {
        Ok(i) => cur.get(i),
        Err(_) => cur.get(part),
    })
}

async fn custom(http: &reqwest::Client, q: &Query, p: &CustomProvider) -> Result<Option<Lyrics>> {
    if p.url.trim().is_empty() {
        return Ok(None);
    }
    let fill = |s: &str, encode: bool| {
        let e = |x: &str| if encode { enc(x) } else { x.to_string() };
        s.replace("{title}", &e(&q.title))
            .replace("{artist}", &e(&q.artist))
            .replace("{album}", &e(&q.album))
            .replace("{duration}", &q.duration_ms.map(|d| (d / 1000).to_string()).unwrap_or_default())
            .replace("{duration_ms}", &q.duration_ms.map(|d| d.to_string()).unwrap_or_default())
            .replace("{key}", &e(&p.key))
    };
    let url = fill(&p.url, true);
    let headers: Vec<(String, String)> = p.headers.lines().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), fill(v.trim(), false))).collect();
    let name = if p.name.trim().is_empty() { "Custom" } else { p.name.trim() };
    if p.format == "json" {
        let v = get_json(http, &url, &headers).await?;
        let text = json_path(&v, &p.json_path).and_then(|x| x.as_str()).unwrap_or("");
        return Ok(build(name, parse_lrc(text), None));
    }
    let mut req = http.get(&url).timeout(Duration::from_secs(10));
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    Ok(build(name, parse_lrc(&resp.text().await?), None))
}

fn cache_key(q: &Query) -> String {
    hash(&format!("{}|{}|{}", norm(&q.title), norm(&q.artist), q.duration_ms.map(|d| d / 2000).unwrap_or(0)))
}

fn cache_path(q: &Query) -> std::path::PathBuf {
    data_dir().join("lyrics").join(format!("{}.json", cache_key(q)))
}

#[derive(Serialize, Deserialize)]
struct Cached {
    at: u64,
    lyrics: Option<Lyrics>,
}

fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub async fn find(http: &reqwest::Client, settings: &Settings, raw: &Query, refresh: bool) -> Result<Found> {
    let mut tried = Vec::new();
    if settings.lyrics_providers.iter().any(|p| p.id == "local" && p.enabled) {
        if let Some(l) = local(raw) {
            return Ok(Found { lyrics: Some(l), tried, cached: false });
        }
        tried.push(Attempt { provider: "Your files".into(), outcome: "no .lrc or embedded lyrics".into() });
    }
    let q = clean(raw);
    let cpath = cache_path(&q);
    if !refresh {
        if let Some(c) = std::fs::read(&cpath).ok().and_then(|b| serde_json::from_slice::<Cached>(&b).ok()) {
            if c.lyrics.is_some() || now_s().saturating_sub(c.at) < CACHE_DAYS * 86400 {
                return Ok(Found { lyrics: c.lyrics, tried, cached: true });
            }
        }
    }
    let mut fallback: Option<Lyrics> = None;
    let enabled: Vec<&ProviderCfg> = settings.lyrics_providers.iter().filter(|p| p.enabled && p.id != "local").collect();
    for p in &enabled {
        let name = match p.id.as_str() {
            "lrclib" => "LRCLIB".to_string(),
            "musixmatch" => "Musixmatch".to_string(),
            "netease" => "NetEase".to_string(),
            "kugou" => "Kugou".to_string(),
            other => settings.custom_lyrics.iter().find(|c| format!("custom:{}", c.id) == other).map(|c| if c.name.trim().is_empty() { "Custom source".to_string() } else { c.name.clone() }).unwrap_or_else(|| "Custom source".into()),
        };
        let result = match p.id.as_str() {
            "lrclib" => lrclib(http, &q).await,
            "musixmatch" if settings.musixmatch_key.trim().is_empty() => {
                tried.push(Attempt { provider: name, outcome: "skipped, no API key".into() });
                continue;
            }
            "musixmatch" => musixmatch(http, &q, &settings.musixmatch_key).await,
            "netease" => netease(http, &q).await,
            "kugou" => kugou(http, &q).await,
            other => match settings.custom_lyrics.iter().find(|c| format!("custom:{}", c.id) == other) {
                Some(c) => custom(http, &q, c).await,
                None => Ok(None),
            },
        };
        match result {
            Ok(Some(l)) if l.synced || l.instrumental => {
                tried.push(Attempt { provider: name, outcome: if l.instrumental { "instrumental".into() } else { "found synced lyrics".into() } });
                fallback = Some(l);
                break;
            }
            Ok(Some(l)) => {
                tried.push(Attempt { provider: name, outcome: "found unsynced lyrics".into() });
                fallback.get_or_insert(l);
            }
            Ok(None) => tried.push(Attempt { provider: name, outcome: "not found".into() }),
            Err(e) => tried.push(Attempt { provider: name, outcome: format!("unreachable ({})", e.to_string().chars().take(80).collect::<String>()) }),
        }
    }
    let all_failed = !enabled.is_empty() && tried.iter().filter(|t| t.outcome.starts_with("unreachable")).count() == enabled.len();
    if !all_failed {
        let _ = std::fs::create_dir_all(cpath.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::write(&cpath, serde_json::to_vec(&Cached { at: now_s(), lyrics: fallback.clone() }).unwrap_or_default());
    }
    Ok(Found { lyrics: fallback, tried, cached: false })
}

pub fn is_timed(text: &str) -> bool {
    parse_lrc(text).iter().filter(|l| l.t.is_some()).count() >= 2
}

pub fn plain_text(text: &str) -> String {
    if !is_timed(text) {
        return text.trim().to_string();
    }
    parse_lrc(text).into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n")
}

pub fn to_lrc(l: &Lyrics) -> String {
    l.lines
        .iter()
        .map(|line| match line.t {
            Some(t) => format!("[{:02}:{:02}.{:02}]{}", t / 60000, (t / 1000) % 60, (t % 1000) / 10, line.text),
            None => line.text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc_with_offsets_and_repeats() {
        let l = parse_lrc("[ar:Nova]\n[offset:+500]\n[00:12.50][01:02.00]Hello there\n[00:05.1]First\n");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].t, Some(4600));
        assert_eq!(l[0].text, "First");
        assert_eq!(l[1].t, Some(12000));
        assert_eq!(l[2].t, Some(61500));
    }

    #[test]
    fn parses_enhanced_and_yrc_word_timing() {
        let l = parse_lrc("[00:01.00]<00:01.00>Hel<00:01.40>lo <00:02.00>world");
        assert_eq!(l[0].text, "Hello world");
        assert_eq!(l[0].words.len(), 3);
        assert_eq!(l[0].words[0].d, 400);
        let y = parse_yrc("{\"t\":0,\"c\":[]}\n[1000,2000](1000,500,0)Hi (1500,500,0)you");
        assert_eq!(y.len(), 1);
        assert_eq!(y[0].text, "Hi you");
        assert_eq!(y[0].words[1].t, 1500);
    }

    #[test]
    fn plain_text_stays_unsynced() {
        let l = parse_lrc("Line one\nLine two");
        assert_eq!(l.len(), 2);
        assert!(l.iter().all(|x| x.t.is_none()));
        assert!(build("x", l, None).is_some_and(|b| !b.synced));
    }

    #[test]
    fn scores_candidates_sensibly() {
        let q = Query { title: "Night Drive (feat. Kai)".into(), artist: "Nova Lumen".into(), duration_ms: Some(200_000), ..Default::default() };
        assert!(score(&q, "Night Drive", "Nova Lumen", Some(201_000)) > 80);
        assert!(score(&q, "Night Drive", "Nova Lumen", Some(260_000)) < 0);
        assert!(score(&q, "Daylight", "Nova Lumen", Some(200_000)) < 0);
        let v: Value = serde_json::json!({"a": {"b": [ {"c": "x"} ]}});
        assert_eq!(json_path(&v, "a.b.0.c").and_then(|x| x.as_str()), Some("x"));
    }

    #[test]
    fn cleans_video_style_titles() {
        let q = |t: &str, a: &str| clean(&Query { title: t.into(), artist: a.into(), ..Default::default() });
        let c = q("Daft Punk - Get Lucky (Official Video)", "Daft Punk");
        assert_eq!((c.artist.as_str(), c.title.as_str()), ("Daft Punk", "Get Lucky"));
        let c = q("Night Drive [Lyrics]", "Nova Lumen - Topic");
        assert_eq!((c.artist.as_str(), c.title.as_str()), ("Nova Lumen", "Night Drive"));
        let c = q("Ember (Night Edit)", "Kai Rivers");
        assert_eq!(c.title, "Ember (Night Edit)");
        let c = q("Halsey - Without Me | Official Music Video", "");
        assert_eq!((c.artist.as_str(), c.title.as_str()), ("Halsey", "Without Me"));
    }

    #[tokio::test]
    #[ignore = "hits the network; run with --ignored"]
    async fn live_lookup_finds_free_lyrics() {
        let http = reqwest::Client::builder().user_agent("atmosphere/0.1").build().unwrap();
        let settings = Settings::default();
        for (title, artist, dur) in [("Daft Punk - Get Lucky (Official Video)", "Daft Punk", 248_000), ("Blinding Lights", "The Weeknd", 200_000)] {
            let q = Query { title: title.into(), artist: artist.into(), duration_ms: Some(dur), ..Default::default() };
            let f = find(&http, &settings, &q, true).await.unwrap();
            let l = f.lyrics.as_ref();
            println!("{title}: provider={:?} synced={:?} words={:?} lines={:?} tried={:?}", l.map(|x| &x.provider), l.map(|x| x.synced), l.map(|x| x.word_synced), l.map(|x| x.lines.len()), f.tried.iter().map(|t| format!("{}: {}", t.provider, t.outcome)).collect::<Vec<_>>());
            assert!(l.is_some_and(|x| x.synced));
        }
        let only_netease = Settings { lyrics_providers: vec![ProviderCfg { id: "netease".into(), enabled: true }], ..Default::default() };
        let q = Query { title: "Get Lucky".into(), artist: "Daft Punk".into(), duration_ms: Some(248_000), ..Default::default() };
        let f = find(&http, &only_netease, &q, true).await.unwrap();
        let l = f.lyrics.as_ref();
        println!("NetEase only: provider={:?} synced={:?} words={:?} lines={:?}", l.map(|x| &x.provider), l.map(|x| x.synced), l.map(|x| x.word_synced), l.map(|x| x.lines.len()));
        assert!(l.is_some_and(|x| x.synced));
    }

    #[test]
    fn roundtrips_to_lrc() {
        let l = build("x", parse_lrc("[01:02.34]Hey"), None).unwrap();
        assert_eq!(to_lrc(&l), "[01:02.34]Hey");
    }
}
