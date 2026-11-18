// atmosphere · library.rs · Local music library: folder scanning, tags, artwork cache, folder-based playlists, play stats and artist profiles.
use crate::downloader::sanitize;
use crate::engine::data_dir;
use anyhow::{anyhow, bail, Context, Result};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const AUDIO_EXT: [&str; 9] = ["mp3", "m4a", "flac", "ogg", "opus", "wav", "aac", "aiff", "aif"];
const ORDER_FILE: &str = "playlist.m3u8";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Track {
    pub id: String,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_no: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub genre: String,
    pub duration_ms: u64,
    pub bitrate: Option<u32>,
    pub format: String,
    pub size: u64,
    pub mtime: u64,
    pub art: Option<String>,
    pub playlist: Option<String>,
    pub has_lyrics: bool,
    pub added: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Stats {
    plays: HashMap<String, u32>,
    last_played: HashMap<String, u64>,
    liked: HashSet<String>,
    listens: Vec<Listen>,
    app_time: std::collections::BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Listen {
    pub id: String,
    pub at: u64,
    pub ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsData {
    pub listens: Vec<Listen>,
    pub app_time: std::collections::BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArtistMeta {
    pub name: String,
    pub image: Option<String>,
    pub bio: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub name: String,
    pub path: String,
    pub order: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub plays: HashMap<String, u32>,
    pub last_played: HashMap<String, u64>,
    pub liked: Vec<String>,
    pub artists: HashMap<String, ArtistMeta>,
    pub scanning: bool,
    pub hidden: usize,
}

pub struct Library {
    tracks: RwLock<HashMap<String, Track>>,
    stats: RwLock<Stats>,
    artists: RwLock<HashMap<String, ArtistMeta>>,
    hidden: RwLock<HashSet<String>>,
    scanning: AtomicBool,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn hash(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().take(10).map(|b| format!("{b:02x}")).collect()
}

pub fn artist_key(name: &str) -> String {
    hash(&name.trim().to_lowercase())
}

fn art_dir() -> PathBuf {
    data_dir().join("art")
}

fn load<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    std::fs::read(data_dir().join(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn store<T: Serialize>(name: &str, value: &T) {
    let path = data_dir().join(name);
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(data) = serde_json::to_vec(value) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

pub fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| AUDIO_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

fn file_stamp(p: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(p).ok()?;
    let mtime = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some((m.len(), mtime))
}

fn save_art(key: &str, data: &[u8]) -> Option<String> {
    let dir = art_dir();
    let out = dir.join(format!("{key}.jpg"));
    if out.exists() {
        return Some(key.to_string());
    }
    std::fs::create_dir_all(&dir).ok()?;
    let img = image::load_from_memory(data).ok()?;
    let rgb = img.thumbnail(600, 600).to_rgb8();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 86).encode_image(&rgb).ok()?;
    std::fs::write(&out, buf).ok()?;
    Some(key.to_string())
}

fn year_of(s: &str) -> Option<u32> {
    let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).take(4).collect();
    digits.parse().ok().filter(|y| (1000..3000).contains(y))
}

pub fn read_track(path: &Path, playlist: Option<String>) -> Result<Track> {
    let (size, mtime) = file_stamp(path).ok_or_else(|| anyhow!("unreadable"))?;
    let file = lofty::read_from_path(path)?;
    let props = file.properties();
    let tag = file.primary_tag().or_else(|| file.first_tag());
    let get = |f: &dyn Fn(&lofty::tag::Tag) -> Option<String>| tag.and_then(f).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string();
    let title = get(&|t| t.title().map(|c| c.into_owned())).unwrap_or(stem);
    let artist = get(&|t| t.artist().map(|c| c.into_owned())).unwrap_or_default();
    let album = get(&|t| t.album().map(|c| c.into_owned())).unwrap_or_default();
    let album_artist = get(&|t| t.get_string(ItemKey::AlbumArtist).map(String::from)).unwrap_or_default();
    let year = get(&|t| t.get_string(ItemKey::Year).or_else(|| t.get_string(ItemKey::RecordingDate)).or_else(|| t.get_string(ItemKey::ReleaseDate)).map(String::from)).and_then(|y| year_of(&y));
    let lyrics_tag = tag.and_then(|t| t.get_string(ItemKey::Lyrics)).is_some_and(|l| !l.trim().is_empty());
    let group = format!("{}|{}", if album_artist.is_empty() { &artist } else { &album_artist }, album).to_lowercase();
    let art_key = if album.is_empty() { hash(&path.to_string_lossy()) } else { hash(&group) };
    let art = tag.and_then(|t| t.pictures().first()).and_then(|p| save_art(&art_key, p.data()));
    let path_s = path.to_string_lossy().into_owned();
    Ok(Track {
        id: hash(&path_s),
        title,
        artist,
        album,
        album_artist,
        track_no: tag.and_then(|t| t.track()),
        track_total: tag.and_then(|t| t.track_total()),
        disc_no: tag.and_then(|t| t.disk()),
        year,
        genre: get(&|t| t.genre().map(|c| c.into_owned())).unwrap_or_default(),
        duration_ms: props.duration().as_millis() as u64,
        bitrate: props.audio_bitrate(),
        format: path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase(),
        size,
        mtime,
        art,
        playlist,
        has_lyrics: lyrics_tag || path.with_extension("lrc").exists(),
        added: now(),
        path: path_s,
    })
}

fn playlist_of(path: &Path, playlists_dir: &Path) -> Option<String> {
    let rel = path.strip_prefix(playlists_dir).ok()?;
    let mut parts = rel.components();
    let first = parts.next()?.as_os_str().to_string_lossy().into_owned();
    parts.next().map(|_| first)
}

fn read_order(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(ORDER_FILE))
        .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect())
        .unwrap_or_default()
}

fn write_order(dir: &Path, names: &[String]) -> Result<()> {
    let mut body = String::from("#EXTM3U\n");
    for n in names {
        body.push_str(n);
        body.push('\n');
    }
    std::fs::write(dir.join(ORDER_FILE), body)?;
    Ok(())
}

impl Library {
    pub fn load() -> Self {
        let cached: Vec<Track> = load("library.json");
        Self {
            tracks: RwLock::new(cached.into_iter().map(|t| (t.id.clone(), t)).collect()),
            stats: RwLock::new(load("stats.json")),
            artists: RwLock::new(load("artists.json")),
            hidden: RwLock::new(load("hidden.json")),
            scanning: AtomicBool::new(false),
        }
    }

    fn persist(&self) {
        let list: Vec<Track> = self.tracks.read().unwrap().values().cloned().collect();
        store("library.json", &list);
    }

    pub fn track(&self, id: &str) -> Option<Track> {
        self.tracks.read().unwrap().get(id).cloned()
    }

    pub fn art_path(&self, key: &str) -> Option<PathBuf> {
        let safe: String = key.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let p = art_dir().join(format!("{safe}.jpg"));
        p.exists().then_some(p)
    }

    pub fn scan(&self, roots: &[PathBuf], playlists_dir: &Path, mut progress: impl FnMut(usize, usize)) -> Result<usize> {
        if self.scanning.swap(true, Ordering::SeqCst) {
            bail!("A scan is already running.");
        }
        let result = self.scan_files(roots, playlists_dir, &mut progress);
        self.scanning.store(false, Ordering::SeqCst);
        result
    }

    fn scan_files(&self, roots: &[PathBuf], playlists_dir: &Path, progress: &mut impl FnMut(usize, usize)) -> Result<usize> {
        {
            let old: HashMap<String, Track> = self.tracks.read().unwrap().values().map(|t| (t.path.clone(), t.clone())).collect();
            let mut files: Vec<PathBuf> = Vec::new();
            let mut seen = HashSet::new();
            let mut all_roots: Vec<PathBuf> = roots.to_vec();
            all_roots.push(playlists_dir.to_path_buf());
            for root in all_roots.iter().filter(|r| r.is_dir()) {
                for entry in walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.')).flatten() {
                    let p = entry.path();
                    if entry.file_type().is_file() && is_audio(p) && seen.insert(p.to_path_buf()) {
                        files.push(p.to_path_buf());
                    }
                }
            }
            let hidden = self.hidden.read().unwrap().clone();
            files.retain(|p| !hidden.contains(&*p.to_string_lossy()));
            let total = files.len();
            let mut next: HashMap<String, Track> = HashMap::with_capacity(total);
            for (i, p) in files.iter().enumerate() {
                let playlist = playlist_of(p, playlists_dir);
                let key = p.to_string_lossy().into_owned();
                let reuse = old.get(&key).filter(|t| file_stamp(p) == Some((t.size, t.mtime))).cloned();
                let track = match reuse {
                    Some(mut t) => {
                        t.playlist = playlist;
                        Some(t)
                    }
                    None => read_track(p, playlist).ok(),
                };
                if let Some(t) = track {
                    next.insert(t.id.clone(), t);
                }
                if i % 25 == 0 || i + 1 == total {
                    progress(i + 1, total);
                }
            }
            let count = next.len();
            *self.tracks.write().unwrap() = next;
            self.persist();
            Ok(count)
        }
    }

    pub fn add_file(&self, path: &Path, playlists_dir: &Path) -> Result<Track> {
        if self.hidden.read().unwrap().contains(&*path.to_string_lossy()) {
            bail!("hidden");
        }
        let t = read_track(path, playlist_of(path, playlists_dir))?;
        self.tracks.write().unwrap().insert(t.id.clone(), t.clone());
        self.persist();
        Ok(t)
    }

    pub fn snapshot(&self, playlists_dir: &Path) -> Snapshot {
        let tracks: Vec<Track> = self.tracks.read().unwrap().values().cloned().collect();
        let stats = self.stats.read().unwrap();
        Snapshot {
            playlists: self.playlists(playlists_dir, &tracks),
            tracks,
            plays: stats.plays.clone(),
            last_played: stats.last_played.clone(),
            liked: stats.liked.iter().cloned().collect(),
            artists: self.artists.read().unwrap().clone(),
            scanning: self.scanning.load(Ordering::SeqCst),
            hidden: self.hidden.read().unwrap().len(),
        }
    }

    fn playlists(&self, dir: &Path, tracks: &[Track]) -> Vec<Playlist> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in rd.flatten().filter(|e| e.path().is_dir()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let folder = entry.path();
            let mut members: Vec<&Track> = tracks.iter().filter(|t| t.playlist.as_deref() == Some(name.as_str())).collect();
            let order = read_order(&folder);
            let file_name = |t: &Track| Path::new(&t.path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            members.sort_by_key(|t| (order.iter().position(|o| *o == file_name(t)).unwrap_or(usize::MAX), file_name(t).to_lowercase()));
            out.push(Playlist { name, path: folder.to_string_lossy().into_owned(), order: members.iter().map(|t| t.id.clone()).collect() });
        }
        out.sort_by_key(|p| p.name.to_lowercase());
        out
    }

    pub fn playlist_create(&self, dir: &Path, name: &str) -> Result<String> {
        let clean = sanitize(name.trim());
        let folder = dir.join(&clean);
        if folder.exists() {
            bail!("A playlist called “{clean}” already exists.");
        }
        std::fs::create_dir_all(&folder).context("Couldn't create the playlist folder")?;
        write_order(&folder, &[])?;
        Ok(clean)
    }

    pub fn playlist_rename(&self, dir: &Path, from: &str, to: &str) -> Result<String> {
        let clean = sanitize(to.trim());
        let (a, b) = (dir.join(from), dir.join(&clean));
        if b.exists() {
            bail!("A playlist called “{clean}” already exists.");
        }
        std::fs::rename(&a, &b).context("Couldn't rename the playlist folder")?;
        let moved: Vec<Track> = self.tracks.read().unwrap().values().filter(|t| t.playlist.as_deref() == Some(from)).cloned().collect();
        let mut tracks = self.tracks.write().unwrap();
        for t in moved {
            tracks.remove(&t.id);
            let new_path = b.join(Path::new(&t.path).file_name().unwrap_or_default());
            let mut nt = t.clone();
            nt.path = new_path.to_string_lossy().into_owned();
            nt.id = hash(&nt.path);
            nt.playlist = Some(clean.clone());
            tracks.insert(nt.id.clone(), nt);
        }
        drop(tracks);
        self.persist();
        Ok(clean)
    }

    pub fn playlist_delete(&self, dir: &Path, name: &str) -> Result<()> {
        trash::delete(dir.join(name)).map_err(|e| anyhow!("Couldn't move the playlist to the trash ({e})"))?;
        self.tracks.write().unwrap().retain(|_, t| t.playlist.as_deref() != Some(name));
        self.persist();
        Ok(())
    }

    pub fn playlist_add(&self, dir: &Path, name: &str, sources: &[PathBuf], link: bool) -> Result<usize> {
        let folder = dir.join(name);
        std::fs::create_dir_all(&folder)?;
        let mut order = read_order(&folder);
        let mut added = 0;
        for src in sources {
            let Some(file) = src.file_name() else { continue };
            let dest = folder.join(file);
            let fname = file.to_string_lossy().into_owned();
            if !dest.exists() {
                let linked = link && std::fs::hard_link(src, &dest).is_ok();
                if !linked {
                    std::fs::copy(src, &dest).with_context(|| format!("Couldn't add {fname}"))?;
                }
                if let Some(lrc) = Some(src.with_extension("lrc")).filter(|l| l.exists()) {
                    let _ = std::fs::copy(&lrc, dest.with_extension("lrc"));
                }
                added += 1;
            }
            if !order.contains(&fname) {
                order.push(fname);
            }
            if let Ok(t) = read_track(&dest, Some(name.to_string())) {
                self.tracks.write().unwrap().insert(t.id.clone(), t);
            }
        }
        write_order(&folder, &order)?;
        self.persist();
        Ok(added)
    }

    pub fn playlist_remove(&self, dir: &Path, name: &str, ids: &[String]) -> Result<()> {
        let folder = dir.join(name);
        let mut order = read_order(&folder);
        for id in ids {
            let Some(t) = self.track(id).filter(|t| t.playlist.as_deref() == Some(name)) else { continue };
            let p = PathBuf::from(&t.path);
            trash::delete(&p).map_err(|e| anyhow!("Couldn't move the file to the trash ({e})"))?;
            let lrc = p.with_extension("lrc");
            if lrc.exists() {
                let _ = trash::delete(&lrc);
            }
            let fname = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            order.retain(|o| *o != fname);
            self.tracks.write().unwrap().remove(id);
        }
        write_order(&folder, &order)?;
        self.persist();
        Ok(())
    }

    pub fn playlist_reorder(&self, dir: &Path, name: &str, ids: &[String]) -> Result<()> {
        let names: Vec<String> = ids
            .iter()
            .filter_map(|id| self.track(id))
            .filter_map(|t| Path::new(&t.path).file_name().map(|f| f.to_string_lossy().into_owned()))
            .collect();
        write_order(&dir.join(name), &names)
    }

    pub fn remove_under(&self, path: &Path) -> usize {
        let mut tracks = self.tracks.write().unwrap();
        let before = tracks.len();
        tracks.retain(|_, t| {
            let p = Path::new(&t.path);
            p != path && !p.starts_with(path)
        });
        let n = before - tracks.len();
        drop(tracks);
        if n > 0 {
            self.persist();
        }
        n
    }

    pub fn knows(&self, path: &Path) -> bool {
        let stamp = file_stamp(path);
        self.tracks.read().unwrap().values().any(|t| Path::new(&t.path) == path && stamp == Some((t.size, t.mtime)))
    }

    pub fn hide(&self, ids: &[String]) -> usize {
        let mut hidden = self.hidden.write().unwrap();
        let mut tracks = self.tracks.write().unwrap();
        let mut n = 0;
        for id in ids {
            if let Some(t) = tracks.remove(id) {
                hidden.insert(t.path);
                n += 1;
            }
        }
        store("hidden.json", &*hidden);
        drop(tracks);
        self.persist();
        n
    }

    pub fn unhide_all(&self) -> usize {
        let mut hidden = self.hidden.write().unwrap();
        let n = hidden.len();
        hidden.clear();
        store("hidden.json", &*hidden);
        n
    }

    fn same_song(a: &Track, b: &Track) -> bool {
        let n = |s: &str| s.trim().to_lowercase();
        n(&a.title) == n(&b.title) && n(&a.artist) == n(&b.artist) && a.duration_ms.abs_diff(b.duration_ms) < 2000
    }

    pub fn delete(&self, ids: &[String], playlists_dir: &Path, also_playlists: bool) -> Result<usize> {
        let targets: Vec<Track> = ids.iter().filter_map(|id| self.track(id)).collect();
        let mut doomed: Vec<Track> = targets.clone();
        if also_playlists {
            let all: Vec<Track> = self.tracks.read().unwrap().values().cloned().collect();
            for t in targets.iter().filter(|t| t.playlist.is_none()) {
                doomed.extend(all.iter().filter(|p| p.playlist.is_some() && p.id != t.id && Self::same_song(t, p)).cloned());
            }
        }
        doomed.sort_by(|a, b| a.id.cmp(&b.id));
        doomed.dedup_by(|a, b| a.id == b.id);
        let mut deleted = 0;
        let mut failures = Vec::new();
        for t in &doomed {
            let p = PathBuf::from(&t.path);
            if p.exists() {
                if let Err(e) = trash::delete(&p) {
                    failures.push(format!("{} ({e})", t.title));
                    continue;
                }
            }
            let lrc = p.with_extension("lrc");
            if lrc.exists() {
                let _ = trash::delete(&lrc);
            }
            if let Some(name) = &t.playlist {
                let folder = playlists_dir.join(name);
                let fname = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                let mut order = read_order(&folder);
                order.retain(|o| *o != fname);
                let _ = write_order(&folder, &order);
            }
            self.tracks.write().unwrap().remove(&t.id);
            if t.playlist.is_none() {
                deleted += 1;
            }
        }
        {
            let mut s = self.stats.write().unwrap();
            for t in &doomed {
                s.plays.remove(&t.id);
                s.last_played.remove(&t.id);
                s.liked.remove(&t.id);
            }
            let gone: HashSet<&str> = doomed.iter().map(|t| t.id.as_str()).collect();
            s.listens.retain(|l| !gone.contains(l.id.as_str()));
            store("stats.json", &*s);
        }
        self.persist();
        if !failures.is_empty() {
            bail!("Couldn't move to the trash: {}", failures.join(", "));
        }
        Ok(deleted.max(targets.iter().filter(|t| t.playlist.is_some()).count()))
    }

    pub fn listened(&self, id: &str, ms: u64) {
        if ms < 1000 || self.track(id).is_none() {
            return;
        }
        let mut s = self.stats.write().unwrap();
        s.listens.push(Listen { id: id.to_string(), at: now(), ms: ms.min(6 * 3600 * 1000) });
        store("stats.json", &*s);
    }

    pub fn app_seconds(&self, day: &str, secs: u64) {
        if day.len() != 10 || secs == 0 {
            return;
        }
        let mut s = self.stats.write().unwrap();
        *s.app_time.entry(day.to_string()).or_default() += secs.min(600);
        store("stats.json", &*s);
    }

    pub fn stats(&self) -> StatsData {
        let tracks = self.tracks.read().unwrap();
        let s = self.stats.read().unwrap();
        StatsData { listens: s.listens.iter().filter(|l| tracks.contains_key(&l.id)).cloned().collect(), app_time: s.app_time.clone() }
    }

    pub fn played(&self, id: &str) {
        let mut s = self.stats.write().unwrap();
        *s.plays.entry(id.to_string()).or_default() += 1;
        s.last_played.insert(id.to_string(), now());
        store("stats.json", &*s);
    }

    pub fn like(&self, id: &str, liked: bool) {
        let mut s = self.stats.write().unwrap();
        if liked {
            s.liked.insert(id.to_string());
        } else {
            s.liked.remove(id);
        }
        store("stats.json", &*s);
    }

    pub fn set_artist(&self, name: &str, image: Option<&Path>, bio: Option<String>) -> Result<ArtistMeta> {
        let key = artist_key(name);
        let mut artists = self.artists.write().unwrap();
        let meta = artists.entry(key.clone()).or_default();
        meta.name = name.trim().to_string();
        if let Some(src) = image {
            let data = std::fs::read(src).context("Couldn't read that image")?;
            let id = format!("artist{key}");
            let _ = std::fs::remove_file(art_dir().join(format!("{id}.jpg")));
            meta.image = Some(save_art(&id, &data).ok_or_else(|| anyhow!("That image couldn't be read"))?);
        }
        if let Some(b) = bio {
            meta.bio = b;
        }
        let out = meta.clone();
        store("artists.json", &*artists);
        Ok(out)
    }

    pub fn refresh(&self, id: &str, playlists_dir: &Path) -> Result<Track> {
        let t = self.track(id).ok_or_else(|| anyhow!("That track is no longer in the library"))?;
        let _ = std::fs::remove_file(art_dir().join(format!("{}.jpg", t.art.clone().unwrap_or_default())));
        self.tracks.write().unwrap().remove(id);
        self.add_file(Path::new(&t.path), playlists_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_playlist_membership() {
        let root = Path::new("/music/Playlists");
        assert_eq!(playlist_of(Path::new("/music/Playlists/Gym/a.mp3"), root), Some("Gym".into()));
        assert_eq!(playlist_of(Path::new("/music/Playlists/a.mp3"), root), None);
        assert_eq!(playlist_of(Path::new("/music/Other/a.mp3"), root), None);
    }

    #[test]
    fn parses_years_and_extensions() {
        assert_eq!(year_of("2019-04-12"), Some(2019));
        assert_eq!(year_of("Released 1998"), Some(1998));
        assert_eq!(year_of("n/a"), None);
        assert!(is_audio(Path::new("x/Song.FLAC")));
        assert!(!is_audio(Path::new("x/cover.jpg")));
    }

    fn wav(path: &Path, seconds: u32) {
        let rate = 8000u32;
        let n = rate * seconds;
        let mut d = Vec::new();
        d.extend(b"RIFF");
        d.extend((36 + n * 2).to_le_bytes());
        d.extend(b"WAVEfmt ");
        d.extend(16u32.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(rate.to_le_bytes());
        d.extend((rate * 2).to_le_bytes());
        d.extend(2u16.to_le_bytes());
        d.extend(16u16.to_le_bytes());
        d.extend(b"data");
        d.extend((n * 2).to_le_bytes());
        d.extend(std::iter::repeat_n(0u8, (n * 2) as usize));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, d).unwrap();
    }

    #[test]
    fn hide_restore_and_folder_removal() {
        let root = std::env::temp_dir().join(format!("atmo-lib-{}", now()));
        std::env::set_var("ATMOSPHERE_DATA_DIR", root.join("data"));
        let music = root.join("music");
        let lists = music.join("Playlists");
        wav(&music.join("Album A").join("one.wav"), 2);
        wav(&music.join("Album A").join("two.wav"), 3);
        wav(&lists.join("Gym").join("one.wav"), 2);
        let lib = Library::load();
        assert_eq!(lib.scan(std::slice::from_ref(&music), &lists, |_, _| {}).unwrap(), 3);
        let snap = lib.snapshot(&lists);
        assert_eq!(snap.playlists.len(), 1);
        assert_eq!(snap.playlists[0].order.len(), 1);
        let one = snap.tracks.iter().find(|t| t.path.ends_with("one.wav") && t.playlist.is_none()).unwrap().id.clone();
        assert_eq!(lib.hide(std::slice::from_ref(&one)), 1);
        assert_eq!(lib.scan(std::slice::from_ref(&music), &lists, |_, _| {}).unwrap(), 2);
        assert_eq!(lib.snapshot(&lists).hidden, 1);
        assert_eq!(lib.unhide_all(), 1);
        assert_eq!(lib.scan(std::slice::from_ref(&music), &lists, |_, _| {}).unwrap(), 3);
        assert_eq!(lib.remove_under(&music.join("Album A")), 2);
        assert_eq!(lib.snapshot(&lists).tracks.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn playlist_order_roundtrip() {
        let dir = std::env::temp_dir().join(format!("atmo-test-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        write_order(&dir, &["b.mp3".into(), "a.mp3".into()]).unwrap();
        assert_eq!(read_order(&dir), vec!["b.mp3".to_string(), "a.mp3".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
