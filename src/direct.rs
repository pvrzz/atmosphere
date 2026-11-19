// atmosphere · direct.rs · Reads real details from direct audio/video file links using ffprobe
use crate::engine::{self, Tools};
use crate::media::{Entry, MediaInfo};
use crate::settings::Settings;
use anyhow::{bail, Result};
use base64::Engine as _;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";
const NOT_MEDIA: &str = "That link didn't open as an audio or video file. Make sure it points straight at the file and that anyone can open it without signing in.";

pub fn is_direct(v: &Value) -> bool {
    v["direct"].as_bool().unwrap_or(false) && v["_type"].as_str().is_none_or(|t| t == "video")
}

fn ffprobe(tools: &Tools) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
    tools.ffmpeg.as_ref().and_then(|f| f.parent()).map(|d| d.join(exe)).filter(|p| p.is_file()).or_else(|| engine::find_tool("ffprobe"))
}

fn net_args(settings: &Settings) -> Vec<String> {
    let mut a: Vec<String> = ["-user_agent", UA, "-rw_timeout", "20000000"].map(String::from).into();
    let proxy = settings.proxy.trim();
    if proxy.starts_with("http://") {
        a.extend(["-http_proxy".into(), proxy.into()]);
    }
    a
}

fn tag(tags: &[&Value], key: &str) -> Option<String> {
    tags.iter().find_map(|t| {
        t.as_object()?
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .and_then(|(_, v)| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

fn leading_num(s: &str) -> Option<u32> {
    s.split(['/', ' ']).next()?.trim().parse().ok()
}

fn kbps(v: &Value) -> Option<u32> {
    v.as_str().and_then(|b| b.parse::<f64>().ok()).filter(|b| *b > 0.0).map(|b| (b / 1000.0).round() as u32)
}

fn is_lossless(codec: &str) -> bool {
    codec.starts_with("pcm_") || ["flac", "alac", "wavpack", "ape", "tta", "mlp", "truehd"].contains(&codec)
}

fn codec_label(codec: &str) -> String {
    match codec {
        c if c.starts_with("pcm_") => "pcm".into(),
        "mp3" | "mp3float" => "mp3".into(),
        "aac" => "aac".into(),
        c => c.to_string(),
    }
}

async fn run(program: &Path, args: &[String]) -> Option<Vec<u8>> {
    let child = engine::command(program).args(args).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).output();
    match tokio::time::timeout(Duration::from_secs(45), child).await {
        Ok(Ok(out)) if out.status.success() && !out.stdout.is_empty() => Some(out.stdout),
        _ => None,
    }
}

async fn cover(tools: &Tools, settings: &Settings, url: &str) -> Option<String> {
    let ff = tools.ffmpeg.as_ref()?;
    let mut args = vec!["-v".into(), "error".into()];
    args.extend(net_args(settings));
    args.extend(["-i", url, "-map", "0:v:0", "-frames:v", "1", "-c:v", "mjpeg", "-q:v", "3", "-f", "image2pipe", "pipe:1"].map(String::from));
    let jpg = run(ff, &args).await?;
    let img = image::load_from_memory(&jpg).ok()?;
    let img = if img.width() > 600 { img.thumbnail(600, 600) } else { img };
    let mut out = Vec::new();
    img.to_rgb8().write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).ok()?;
    Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(out)))
}

pub async fn enrich(tools: &Tools, settings: &Settings, v: &Value, entry: &mut Entry) -> Result<()> {
    let url = entry.url.clone();
    let Some(probe) = ffprobe(tools) else {
        entry.platform = "File link".into();
        return Ok(());
    };
    let mut args = vec!["-v".into(), "error".into(), "-print_format".into(), "json".into(), "-show_format".into(), "-show_streams".into()];
    args.extend(net_args(settings));
    args.push(url.clone());
    let Some(out) = run(&probe, &args).await else { bail!(NOT_MEDIA) };
    let info: Value = serde_json::from_slice(&out).unwrap_or_default();
    let streams = info["streams"].as_array().cloned().unwrap_or_default();
    let is_pic = |s: &Value| s["disposition"]["attached_pic"].as_i64() == Some(1);
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");
    let video: Vec<&Value> = streams.iter().filter(|s| s["codec_type"] == "video" && !is_pic(s)).collect();
    let has_art = streams.iter().any(|s| s["codec_type"] == "video" && is_pic(s));
    if audio.is_none() && video.is_empty() {
        bail!(NOT_MEDIA);
    }
    let format = &info["format"];
    let mut tag_sources = vec![&format["tags"]];
    if let Some(a) = audio {
        tag_sources.push(&a["tags"]);
    }
    let codec = audio.and_then(|a| a["codec_name"].as_str()).unwrap_or("");
    let lossless = !codec.is_empty() && is_lossless(codec);
    let abr = if lossless {
        None
    } else {
        audio.and_then(|a| kbps(&a["bit_rate"])).or_else(|| video.is_empty().then(|| kbps(&format["bit_rate"])).flatten())
    };
    let mut heights: Vec<u32> = video.iter().filter_map(|s| s["height"].as_u64().map(|h| h as u32)).collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    let ext = v["ext"].as_str().map(str::to_string).or_else(|| url.rsplit('.').next().map(|e| e.split(['?', '#']).next().unwrap_or(e).to_ascii_lowercase()));
    entry.media = Some(MediaInfo {
        has_video: !video.is_empty(),
        heights,
        best_abr: abr,
        best_codec: (!codec.is_empty()).then(|| codec_label(codec)),
        lossless,
        original: true,
        original_lossless: lossless,
        original_ext: ext,
    });
    entry.detailed = true;
    entry.availability = "ok".into();
    entry.platform = "File link".into();
    if let Some(d) = format["duration"].as_str().and_then(|d| d.parse::<f64>().ok()) {
        entry.duration_ms = Some((d * 1000.0) as u64);
    }
    if let Some(t) = tag(&tag_sources, "title") {
        entry.title = t;
    }
    if let Some(a) = tag(&tag_sources, "artist") {
        entry.artist = a.clone();
        entry.uploader = a;
    }
    entry.album = tag(&tag_sources, "album").or(entry.album.take());
    entry.album_artist = tag(&tag_sources, "album_artist").or_else(|| tag(&tag_sources, "albumartist")).or(entry.album_artist.take());
    entry.genre = tag(&tag_sources, "genre").or(entry.genre.take());
    entry.composer = tag(&tag_sources, "composer").or(entry.composer.take());
    entry.track_number = tag(&tag_sources, "track").as_deref().and_then(leading_num).or(entry.track_number);
    entry.disc_number = tag(&tag_sources, "disc").as_deref().and_then(leading_num).or(entry.disc_number);
    if let Some(date) = tag(&tag_sources, "date").or_else(|| tag(&tag_sources, "year")) {
        entry.year = date.get(..4).and_then(|y| y.parse().ok()).or(entry.year);
        if date.len() >= 10 {
            entry.release_date = Some(date[..10].to_string());
        }
    }
    if entry.lyrics.is_none() {
        entry.lyrics = tag(&tag_sources, "lyrics").or_else(|| tag(&tag_sources, "unsyncedlyrics"));
    }
    if entry.thumbnail.is_none() && has_art {
        entry.thumbnail = cover(tools, settings, &url).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tags_case_insensitively_and_numbers() {
        let t = serde_json::json!({ "TITLE": "Song", "Track": "3/12" });
        assert_eq!(tag(&[&t], "title").as_deref(), Some("Song"));
        assert_eq!(tag(&[&t], "track").as_deref().and_then(leading_num), Some(3));
        assert!(is_lossless("pcm_s16le") && is_lossless("flac") && !is_lossless("mp3"));
        assert_eq!(kbps(&serde_json::json!("320000")), Some(320));
    }

    #[tokio::test]
    #[ignore]
    async fn live_direct_mp3_reports_its_real_bitrate() {
        let tools = engine::locate("", "").unwrap();
        let settings = Settings::default();
        let mut e = Entry { url: "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3".into(), ..Default::default() };
        enrich(&tools, &settings, &serde_json::json!({ "ext": "mp3" }), &mut e).await.unwrap();
        let m = e.media.unwrap();
        assert_eq!((m.best_abr, m.best_codec.as_deref(), m.original), (Some(192), Some("mp3"), true));
        assert!(e.duration_ms.unwrap() > 370_000);
        let mut bad = Entry { url: "https://example.com/".into(), ..Default::default() };
        assert!(enrich(&tools, &settings, &serde_json::json!({}), &mut bad).await.is_err());
    }

    #[test]
    fn detects_direct_files_only() {
        assert!(is_direct(&serde_json::json!({ "direct": true, "_type": "video" })));
        assert!(!is_direct(&serde_json::json!({ "_type": "video" })));
        assert!(!is_direct(&serde_json::json!({ "direct": true, "_type": "playlist" })));
    }
}
