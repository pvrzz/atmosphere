// atmosphere · storage.rs · Temp-file hygiene: startup and exit cleanup, stalled partial downloads, orphaned caches and the Storage report.
use crate::engine::{bin_dir, data_dir, temp_root};
use crate::library::Snapshot;
use crate::settings::Settings;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub bytes: u64,
    pub count: usize,
    pub cleanable: bool,
}

pub fn is_partial(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".part") || n.ends_with(".ytdl") || n.contains(".part-frag") || n.contains(".atmosphere-src.") || n.contains(".tagging.") || n.ends_with(".temp") || n.ends_with(".download")
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, keep: &dyn Fn(&Path) -> bool) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => walk(&p, depth - 1, out, keep),
            Ok(t) if t.is_file() && keep(&p) => out.push(p),
            _ => {}
        }
    }
}

fn size(paths: &[PathBuf]) -> u64 {
    paths.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum()
}

fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, 6, &mut out, &|_| true);
    out
}

fn stalled(settings: &Settings) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut roots: Vec<PathBuf> = vec![PathBuf::from(&settings.output_dir), PathBuf::from(&settings.playlists_dir), bin_dir()];
    roots.dedup();
    for r in roots {
        walk(&r, 4, &mut out, &|p| p.file_name().and_then(|n| n.to_str()).is_some_and(is_partial));
    }
    out.sort();
    out.dedup();
    out
}

fn orphan_art(lib: &Snapshot) -> Vec<PathBuf> {
    let used: HashSet<String> = lib
        .tracks
        .iter()
        .filter_map(|t| t.art.clone())
        .chain(lib.artists.values().flat_map(|a| [a.image.clone(), a.logo.clone()]).flatten())
        .chain(lib.albums.values().filter_map(|a| a.logo.clone()))
        .collect();
    let mut out = Vec::new();
    walk(&data_dir().join("art"), 0, &mut out, &|p| p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| !used.contains(s)));
    out
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

pub fn report(_settings: &Settings, lib: &Snapshot, busy: bool) -> Vec<Item> {
    let temp = all_files(&temp_root());
    let orphans = orphan_art(lib);
    let lyrics = all_files(&data_dir().join("lyrics"));
    let art = all_files(&data_dir().join("art"));
    let engine = all_files(&bin_dir());
    vec![
        Item { id: "temp".into(), label: "Temporary files".into(), detail: "Pieces of downloads in progress. Wiped automatically, along with any stalled partial downloads.".into(), bytes: size(&temp), count: temp.len(), cleanable: !busy },
        Item { id: "orphan-art".into(), label: "Unused cover art".into(), detail: "Artwork for songs no longer in your library.".into(), bytes: size(&orphans), count: orphans.len(), cleanable: true },
        Item { id: "lyrics".into(), label: "Lyrics cache".into(), detail: "Lyrics fetched online; they're fetched again when needed.".into(), bytes: size(&lyrics), count: lyrics.len(), cleanable: true },
        Item { id: "art".into(), label: "Cover art in use".into(), detail: format!("Thumbnails for your library ({}).", mb(size(&art).saturating_sub(size(&orphans)))), bytes: size(&art).saturating_sub(size(&orphans)), count: art.len().saturating_sub(orphans.len()), cleanable: false },
        Item { id: "engine".into(), label: "Download engine".into(), detail: "yt-dlp and ffmpeg managed by Atmosphere.".into(), bytes: size(&engine), count: engine.len(), cleanable: false },
    ]
}

fn remove_all(paths: &[PathBuf]) -> (usize, u64) {
    let mut n = 0;
    let mut bytes = 0;
    for p in paths {
        let len = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        if std::fs::remove_file(p).is_ok() {
            n += 1;
            bytes += len;
        }
    }
    (n, bytes)
}

pub fn wipe_temp() -> (usize, u64) {
    let files = all_files(&temp_root());
    let r = remove_all(&files);
    let _ = std::fs::remove_dir_all(temp_root());
    r
}

pub fn clean(settings: &Settings, lib: &Snapshot, ids: &[String], busy: bool) -> (usize, u64) {
    let mut total = (0, 0);
    let mut add = |r: (usize, u64)| {
        total.0 += r.0;
        total.1 += r.1;
    };
    for id in ids {
        match id.as_str() {
            "temp" if !busy => add(wipe_temp()),
            "stalled" if !busy => add(remove_all(&stalled(settings))),
            "orphan-art" => add(remove_all(&orphan_art(lib))),
            "lyrics" => add(remove_all(&all_files(&data_dir().join("lyrics")))),
            _ => {}
        }
    }
    total
}

pub fn clear_stalled(settings: &Settings) -> (usize, u64) {
    remove_all(&stalled(settings))
}

pub fn startup(settings: &Settings) -> (usize, u64) {
    let a = wipe_temp();
    let b = remove_all(&stalled(settings));
    (a.0 + b.0, a.1 + b.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_partial_files_only() {
        for n in ["Song.mp3.part", "x.f251.webm.part-Frag12", "a.ytdl", "03 - Track.atmosphere-src.m4a", "v.tagging.mkv", "yt-dlp.download"] {
            assert!(is_partial(n), "{n}");
        }
        for n in ["Song.mp3", "partner.flac", "cover.jpg", "playlist.m3u8", "Night Drive.lrc"] {
            assert!(!is_partial(n), "{n}");
        }
    }
}
