// atmosphere · tagging.rs · Applies the user's edited metadata and cover art on top of what yt-dlp embedded.
use anyhow::{anyhow, bail, Result};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::items::Timestamp;
use lofty::tag::{Accessor, ItemKey, Tag};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Artwork {
    #[default]
    Keep,
    None,
    File {
        path: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TagFields {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub date: String,
    pub track_number: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    pub composer: String,
    pub label: String,
    pub isrc: String,
    pub bpm: Option<u32>,
    pub key: String,
    pub comment: String,
    pub lyrics: String,
    pub artwork: Artwork,
}

const LOFTY: [&str; 10] = ["mp3", "m4a", "mp4", "m4v", "flac", "ogg", "opus", "wav", "aiff", "aif"];

fn parse_date(raw: &str) -> Option<Timestamp> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<Timestamp>().ok().or_else(|| {
        let year = raw.get(..4)?.parse().ok()?;
        Some(Timestamp { year, ..Default::default() })
    })
}

fn put(tag: &mut Tag, key: ItemKey, value: &str) {
    let v = value.trim();
    if !v.is_empty() {
        tag.insert_text(key, v.to_string());
    }
}

fn write_lofty(path: &Path, f: &TagFields, source_url: &str) -> Result<()> {
    let mut file = lofty::read_from_path(path)?;
    let kind = file.primary_tag_type();
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.primary_tag_mut().ok_or_else(|| anyhow!("no writable tag"))?;
    put(tag, ItemKey::TrackTitle, &f.title);
    put(tag, ItemKey::TrackArtist, &f.artist);
    put(tag, ItemKey::AlbumTitle, &f.album);
    put(tag, ItemKey::AlbumArtist, &f.album_artist);
    put(tag, ItemKey::Genre, &f.genre);
    put(tag, ItemKey::Composer, &f.composer);
    put(tag, ItemKey::Label, &f.label);
    put(tag, ItemKey::Isrc, &f.isrc);
    put(tag, ItemKey::InitialKey, &f.key);
    put(tag, ItemKey::Comment, &f.comment);
    put(tag, ItemKey::AudioSourceUrl, source_url);
    if !f.lyrics.trim().is_empty() {
        tag.insert_text(ItemKey::Lyrics, crate::lyrics::plain_text(&f.lyrics));
    }
    if let Some(bpm) = f.bpm.filter(|b| *b > 0) {
        tag.insert_text(ItemKey::IntegerBpm, bpm.to_string());
    }
    if let Some(date) = parse_date(&f.date) {
        tag.set_date(date);
    }
    if let Some(n) = f.track_number {
        tag.set_track(n);
    }
    if let Some(total) = f.track_total {
        tag.set_track_total(total);
    }
    if let Some(disc) = f.disc_number {
        tag.set_disk(disc);
    }
    match &f.artwork {
        Artwork::Keep => {}
        Artwork::None => {
            while !tag.pictures().is_empty() {
                tag.remove_picture(0);
            }
        }
        Artwork::File { path } => {
            let data = std::fs::read(path).map_err(|e| anyhow!("couldn't read the cover image ({e})"))?;
            let mime = if data.starts_with(&[0x89, b'P', b'N', b'G']) { MimeType::Png } else { MimeType::Jpeg };
            while !tag.pictures().is_empty() {
                tag.remove_picture(0);
            }
            tag.push_picture(Picture::unchecked(data).pic_type(PictureType::CoverFront).mime_type(mime).build());
        }
    }
    file.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

async fn write_ffmpeg(ffmpeg: &Path, path: &Path, f: &TagFields) -> Result<()> {
    let pairs = [
        ("title", f.title.clone()),
        ("artist", f.artist.clone()),
        ("album", f.album.clone()),
        ("album_artist", f.album_artist.clone()),
        ("genre", f.genre.clone()),
        ("date", f.date.clone()),
        ("composer", f.composer.clone()),
        ("comment", f.comment.clone()),
        ("track", f.track_number.map(|n| n.to_string()).unwrap_or_default()),
    ];
    let meta: Vec<(&str, String)> = pairs.into_iter().filter(|(_, v)| !v.trim().is_empty()).collect();
    if meta.is_empty() {
        return Ok(());
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
    let tmp: PathBuf = path.with_extension(format!("tagging.{ext}"));
    let mut cmd = crate::engine::command(ffmpeg);
    cmd.args(["-y", "-v", "error", "-i"]).arg(path).args(["-map", "0", "-c", "copy"]);
    for (k, v) in &meta {
        cmd.arg("-metadata").arg(format!("{k}={}", v.trim()));
    }
    let out = cmd.arg(&tmp).stdout(Stdio::null()).stderr(Stdio::piped()).output().await?;
    if !out.status.success() {
        let _ = tokio::fs::remove_file(&tmp).await;
        bail!("{}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("ffmpeg failed"));
    }
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

pub async fn apply(path: &Path, f: &TagFields, source_url: &str, ffmpeg: Option<&Path>) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    if LOFTY.contains(&ext.as_str()) {
        let (path, f, url) = (path.to_path_buf(), f.clone(), source_url.to_string());
        return tokio::task::spawn_blocking(move || write_lofty(&path, &f, &url)).await?;
    }
    match ffmpeg {
        Some(ff) => write_ffmpeg(ff, path, f).await,
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dates() {
        assert_eq!(parse_date("2021").unwrap().year, 2021);
        let full = parse_date("2019-04-12").unwrap();
        assert_eq!((full.year, full.month, full.day), (2019, Some(4), Some(12)));
        assert!(parse_date("  ").is_none());
    }

    #[test]
    fn artwork_defaults_to_keep() {
        let f: TagFields = serde_json::from_str(r#"{"title":"x"}"#).unwrap();
        assert!(matches!(f.artwork, Artwork::Keep));
        let f: TagFields = serde_json::from_str(r#"{"artwork":{"kind":"file","path":"c.jpg"}}"#).unwrap();
        assert!(matches!(f.artwork, Artwork::File { .. }));
    }
}
