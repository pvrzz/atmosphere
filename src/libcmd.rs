// atmosphere · libcmd.rs · Library, playlist, lyrics and window commands, plus the atmo:// media protocol that streams local files.
use crate::library::{Snapshot, Track};
use crate::lyrics::{self, Found, Lyrics, Query};
use crate::tagging::{self, TagFields};
use crate::{err, AppState, CmdResult};
use notify::{RecursiveMode, Watcher};
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::Duration;
use std::path::{Path, PathBuf};
use tauri::http::{Request, Response};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

pub const LIBRARY_EVENT: &str = crate::downloader::LIBRARY_EVENT;
const MAX_CHUNK: u64 = 4 * 1024 * 1024;

fn dirs(state: &AppState) -> (Vec<PathBuf>, PathBuf) {
    let s = state.snapshot();
    (s.library_folders.iter().map(PathBuf::from).collect(), PathBuf::from(&s.playlists_dir))
}

fn changed(app: &AppHandle) {
    let _ = app.emit(LIBRARY_EVENT, serde_json::json!({ "type": "changed" }));
}

#[tauri::command]
pub fn library_get(state: State<'_, AppState>) -> Snapshot {
    let (_, pl) = dirs(&state);
    state.library.snapshot(&pl)
}

pub fn spawn_scan(app: AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (roots, pl) = dirs(&state);
        let _ = std::fs::create_dir_all(&pl);
        let emitter = app.clone();
        let result = state.library.scan(&roots, &pl, |done, total| {
            let _ = emitter.emit(LIBRARY_EVENT, serde_json::json!({ "type": "progress", "done": done, "total": total }));
        });
        let _ = app.emit(LIBRARY_EVENT, match result {
            Ok(n) => serde_json::json!({ "type": "scanned", "count": n }),
            Err(e) => serde_json::json!({ "type": "error", "message": format!("{e:#}") }),
        });
    });
}

static WATCHER: Mutex<Option<notify::RecommendedWatcher>> = Mutex::new(None);

fn apply_changes(app: &AppHandle, paths: HashSet<PathBuf>) {
    let state = app.state::<AppState>();
    let (_, pl) = dirs(&state);
    let temp = crate::engine::temp_root();
    let mut touched = false;
    for p in paths {
        if p.starts_with(&temp) || p.file_name().and_then(|n| n.to_str()).is_some_and(crate::storage::is_partial) {
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) == Some("playlist.m3u8") {
            touched = true;
            continue;
        }
        if !p.exists() {
            touched |= state.library.remove_under(&p) > 0;
        } else if p.is_dir() {
            for e in walkdir::WalkDir::new(&p).into_iter().flatten().filter(|e| e.file_type().is_file()) {
                if crate::library::is_playable(e.path()) && !state.library.knows(e.path()) {
                    touched |= state.library.add_file(e.path(), &pl).is_ok();
                }
            }
        } else if crate::library::is_playable(&p) && !state.library.knows(&p) {
            touched |= state.library.add_file(&p, &pl).is_ok();
        }
    }
    if touched {
        changed(app);
    }
}

pub fn start_watch(app: AppHandle) {
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let Ok(mut watcher) = notify::recommended_watcher(tx) else { return };
    let (roots, pl) = dirs(&app.state::<AppState>());
    let _ = std::fs::create_dir_all(&pl);
    let mut watched: Vec<PathBuf> = Vec::new();
    let linked: Vec<PathBuf> = app.state::<AppState>().library.linked_list().into_iter().map(|l| PathBuf::from(l.path)).collect();
    for r in roots.into_iter().chain(std::iter::once(pl)).chain(linked) {
        if r.is_dir() && !watched.iter().any(|w| r.starts_with(w)) && watcher.watch(&r, RecursiveMode::Recursive).is_ok() {
            watched.push(r);
        }
    }
    *WATCHER.lock().unwrap() = Some(watcher);
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut batch: HashSet<PathBuf> = HashSet::new();
            if let Ok(ev) = first {
                batch.extend(ev.paths);
            }
            while let Ok(next) = rx.recv_timeout(Duration::from_millis(700)) {
                if let Ok(ev) = next {
                    batch.extend(ev.paths);
                }
            }
            if !batch.is_empty() {
                apply_changes(&app, batch);
            }
        }
    });
}

pub fn start_linked_monitor(app: AppHandle) {
    std::thread::spawn(move || {
        let mut seen: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        loop {
            let state = app.state::<AppState>();
            let (_, pl) = dirs(&state);
            let mut changed_any = false;
            let mut rewatch = false;
            for l in state.library.linked_list() {
                let here = Path::new(&l.path).is_dir();
                let before = seen.insert(l.path.clone(), here);
                if before == Some(here) || before.is_none() {
                    continue;
                }
                changed_any = true;
                if here {
                    state.library.scan_folder(Path::new(&l.path), &pl);
                    rewatch = true;
                } else {
                    state.library.remove_under(Path::new(&l.path));
                }
            }
            if rewatch {
                start_watch(app.clone());
            }
            if changed_any {
                changed(&app);
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    });
}

async fn pick_folder(app: &AppHandle, title: &str) -> CmdResult<Option<PathBuf>> {
    let a = app.clone();
    let title = title.to_string();
    let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title(&title).blocking_pick_folder()).await.map_err(|e| e.to_string())?;
    Ok(picked.and_then(|p| p.into_path().ok()))
}

#[tauri::command]
pub async fn playlist_link(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let Some(path) = pick_folder(&app, "Add an existing folder as a playlist").await? else { return Ok(None) };
    let (_, pl) = dirs(&state);
    let lib = state.library.clone();
    let name = tauri::async_runtime::spawn_blocking(move || lib.playlist_link(&pl, &path)).await.map_err(|e| e.to_string())?.map_err(err)?;
    start_watch(app.clone());
    changed(&app);
    Ok(Some(name))
}

#[tauri::command]
pub async fn playlist_relocate(app: AppHandle, state: State<'_, AppState>, name: String) -> CmdResult<bool> {
    let Some(path) = pick_folder(&app, &format!("Locate the folder for “{name}”")).await? else { return Ok(false) };
    let (_, pl) = dirs(&state);
    let lib = state.library.clone();
    tauri::async_runtime::spawn_blocking(move || lib.playlist_relocate(&pl, &name, &path)).await.map_err(|e| e.to_string())?.map_err(err)?;
    start_watch(app.clone());
    changed(&app);
    Ok(true)
}

#[tauri::command]
pub fn playlist_unlink(app: AppHandle, state: State<'_, AppState>, name: String) -> CmdResult<()> {
    let (roots, _) = dirs(&state);
    state.library.playlist_unlink(&name, &roots).map_err(err)?;
    start_watch(app.clone());
    changed(&app);
    Ok(())
}

#[tauri::command]
pub fn library_scan(app: AppHandle) {
    spawn_scan(app);
}

#[tauri::command]
pub async fn library_add_folder(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let a = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title("Add a music folder").blocking_pick_folder())
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let p = path.to_string_lossy().into_owned();
    state.update(|s| {
        if !s.library_folders.contains(&p) {
            s.library_folders.push(p.clone());
        }
    })?;
    spawn_scan(app.clone());
    start_watch(app);
    Ok(Some(p))
}

#[tauri::command]
pub fn library_remove_folder(app: AppHandle, state: State<'_, AppState>, path: String) -> CmdResult<()> {
    state.update(|s| s.library_folders.retain(|f| *f != path))?;
    spawn_scan(app.clone());
    start_watch(app);
    Ok(())
}

#[tauri::command]
pub fn playlist_create(app: AppHandle, state: State<'_, AppState>, name: String) -> CmdResult<String> {
    let (_, pl) = dirs(&state);
    let n = state.library.playlist_create(&pl, &name).map_err(err)?;
    changed(&app);
    Ok(n)
}

#[tauri::command]
pub fn playlist_rename(app: AppHandle, state: State<'_, AppState>, from: String, to: String) -> CmdResult<String> {
    let (_, pl) = dirs(&state);
    let n = state.library.playlist_rename(&pl, &from, &to).map_err(err)?;
    changed(&app);
    Ok(n)
}

#[tauri::command]
pub fn playlist_delete(app: AppHandle, state: State<'_, AppState>, name: String) -> CmdResult<()> {
    let (_, pl) = dirs(&state);
    state.library.playlist_delete(&pl, &name).map_err(err)?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn playlist_add(app: AppHandle, state: State<'_, AppState>, name: String, ids: Vec<String>) -> CmdResult<usize> {
    let (_, pl) = dirs(&state);
    let sources: Vec<PathBuf> = ids.iter().filter_map(|id| state.library.track(id)).map(|t| PathBuf::from(t.path)).collect();
    let link = !state.snapshot().playlist_copy;
    let lib = state.library.clone();
    let n = tauri::async_runtime::spawn_blocking(move || lib.playlist_add(&pl, &name, &sources, link)).await.map_err(|e| e.to_string())?.map_err(err)?;
    changed(&app);
    Ok(n)
}

#[tauri::command]
pub async fn playlist_add_files(app: AppHandle, state: State<'_, AppState>, name: String) -> CmdResult<usize> {
    let a = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        a.dialog().file().set_title("Add songs to the playlist").add_filter("Audio", &crate::library::AUDIO_EXT).blocking_pick_files()
    })
    .await
    .map_err(|e| e.to_string())?;
    let files: Vec<PathBuf> = picked.unwrap_or_default().into_iter().filter_map(|p| p.into_path().ok()).collect();
    if files.is_empty() {
        return Ok(0);
    }
    let (_, pl) = dirs(&state);
    let lib = state.library.clone();
    let n = tauri::async_runtime::spawn_blocking(move || lib.playlist_add(&pl, &name, &files, false)).await.map_err(|e| e.to_string())?.map_err(err)?;
    changed(&app);
    Ok(n)
}

#[tauri::command]
pub fn playlist_remove(app: AppHandle, state: State<'_, AppState>, name: String, ids: Vec<String>) -> CmdResult<()> {
    let (_, pl) = dirs(&state);
    state.library.playlist_remove(&pl, &name, &ids).map_err(err)?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub fn playlist_reorder(state: State<'_, AppState>, name: String, ids: Vec<String>) -> CmdResult<()> {
    let (_, pl) = dirs(&state);
    state.library.playlist_reorder(&pl, &name, &ids).map_err(err)
}

#[tauri::command]
pub async fn tracks_delete(app: AppHandle, state: State<'_, AppState>, ids: Vec<String>, playlists: bool) -> CmdResult<usize> {
    let (_, pl) = dirs(&state);
    let lib = state.library.clone();
    let n = tauri::async_runtime::spawn_blocking(move || lib.delete(&ids, &pl, playlists)).await.map_err(|e| e.to_string())?;
    changed(&app);
    n.map_err(err)
}

#[tauri::command]
pub fn tracks_hide(app: AppHandle, state: State<'_, AppState>, ids: Vec<String>) -> usize {
    let n = state.library.hide(&ids);
    changed(&app);
    n
}

#[tauri::command]
pub fn tracks_unhide_all(app: AppHandle, state: State<'_, AppState>) -> usize {
    let n = state.library.unhide_all();
    spawn_scan(app);
    n
}

#[tauri::command]
pub fn track_played(state: State<'_, AppState>, id: String) {
    state.library.played(&id);
}

#[tauri::command]
pub fn track_listened(state: State<'_, AppState>, id: String, ms: u64) {
    state.library.listened(&id, ms);
}

#[tauri::command]
pub fn app_heartbeat(state: State<'_, AppState>, day: String, secs: u64) {
    state.library.app_seconds(&day, secs);
}

#[tauri::command]
pub fn stats_get(state: State<'_, AppState>) -> crate::library::StatsData {
    state.library.stats()
}

fn png_from(data: &str) -> CmdResult<Vec<u8>> {
    use base64::Engine as _;
    let raw = data.split_once(',').map(|(_, b)| b).unwrap_or(data);
    base64::engine::general_purpose::STANDARD.decode(raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn image_save(app: AppHandle, data: String, name: String) -> CmdResult<Option<String>> {
    let bytes = png_from(&data)?;
    let a = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title("Save your stats").set_file_name(&name).add_filter("PNG image", &["png"]).blocking_save_file())
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn image_copy(app: AppHandle, data: String) -> CmdResult<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let bytes = png_from(&data)?;
    let img = tauri::image::Image::from_bytes(&bytes).map_err(|e| e.to_string())?;
    app.clipboard().write_image(&img).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn track_like(state: State<'_, AppState>, id: String, liked: bool) {
    state.library.like(&id, liked);
}

#[tauri::command]
pub async fn track_edit(app: AppHandle, state: State<'_, AppState>, id: String, meta: TagFields) -> CmdResult<Track> {
    let t = state.library.track(&id).ok_or("That track is no longer in the library")?;
    let s = state.snapshot();
    let ffmpeg = crate::engine::locate(&s.ytdlp_path, &s.ffmpeg_path).ok().and_then(|t| t.ffmpeg);
    tagging::apply(Path::new(&t.path), &meta, "", ffmpeg.as_deref()).await.map_err(err)?;
    let (_, pl) = dirs(&state);
    let mut updated = state.library.refresh(&id, &pl).map_err(err)?;
    if let Some(moved) = file_by_album(&state, &[(t, updated.clone())]).into_iter().next() {
        updated = moved;
    }
    changed(&app);
    Ok(updated)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    n(a) == n(b)
}

pub fn album_home(base: &Path, file: &Path, album: &str, known: Option<PathBuf>) -> Option<PathBuf> {
    let (tracks, albums) = (base.join("Tracks"), base.join("Albums"));
    let parent = file.parent()?;
    let in_tracks = same_dir(parent, &tracks);
    let in_album = parent.parent().is_some_and(|p| same_dir(p, &albums));
    let album = album.trim();
    if !(in_tracks || in_album) || album.is_empty() {
        return None;
    }
    let name = crate::downloader::sanitize(album);
    let target = known.or_else(|| crate::downloader::existing_dir(&albums, &name)).unwrap_or_else(|| albums.join(&name));
    (!same_dir(&target, parent)).then_some(target)
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

pub fn move_with_sidecars(file: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let parent = file.parent().ok_or_else(|| std::io::Error::other("no folder"))?.to_path_buf();
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut dest = dir.join(format!("{stem}{ext}"));
    let mut n = 2;
    while dest.exists() {
        dest = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    move_file(file, &dest)?;
    let new_stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    for e in std::fs::read_dir(&parent)?.flatten() {
        let p = e.path();
        if !p.is_file() || p.file_stem().map(|s| s.to_string_lossy().into_owned()).as_deref() != Some(stem.as_str()) {
            continue;
        }
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let to = dir.join(format!("{new_stem}{ext}"));
        if !to.exists() {
            let _ = move_file(&p, &to);
        }
    }
    Ok(dest)
}

fn tidy_album_dir(dir: &Path, albums: &Path, went_to: &HashSet<PathBuf>) {
    if !dir.parent().is_some_and(|p| same_dir(p, albums)) {
        return;
    }
    let Ok(rest) = std::fs::read_dir(dir) else { return };
    let rest: Vec<PathBuf> = rest.flatten().map(|e| e.path()).collect();
    if rest.iter().any(|p| p.is_dir() || crate::library::is_playable(p)) {
        return;
    }
    if let (1, Some(target)) = (went_to.len(), went_to.iter().next()) {
        for p in &rest {
            if let Some(name) = p.file_name() {
                if !target.join(name).exists() {
                    let _ = move_file(p, &target.join(name));
                }
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

fn file_by_album(state: &AppState, edits: &[(Track, Track)]) -> Vec<Track> {
    let s = state.snapshot();
    if !s.collection_folders {
        return Vec::new();
    }
    let base = PathBuf::from(&s.output_dir);
    let albums = base.join("Albums");
    let editing: HashSet<&str> = edits.iter().map(|(_, a)| a.path.as_str()).collect();
    let norm = crate::artistinfo::norm;
    let mut out = Vec::new();
    let mut left: std::collections::HashMap<PathBuf, HashSet<PathBuf>> = std::collections::HashMap::new();
    for (before, after) in edits {
        if norm(&before.album) == norm(&after.album) || after.album.trim().is_empty() {
            continue;
        }
        let file = PathBuf::from(&after.path);
        let (want, by) = (norm(&after.album), norm(&after.album_artist));
        let in_albums = |t: &Track| Path::new(&t.path).parent().and_then(Path::parent).is_some_and(|p| same_dir(p, &albums));
        let same_artist = |t: &Track| by.is_empty() || norm(&t.album_artist).is_empty() || norm(&t.album_artist) == by;
        let known = state
            .library
            .find_track(|t| !editing.contains(t.path.as_str()) && norm(&t.album) == want && same_artist(t) && in_albums(t))
            .and_then(|t| Path::new(&t.path).parent().map(Path::to_path_buf));
        let Some(dir) = album_home(&base, &file, &after.album, known) else { continue };
        let Ok(dest) = move_with_sidecars(&file, &dir) else { continue };
        if let Some(old_dir) = file.parent() {
            left.entry(old_dir.to_path_buf()).or_default().insert(dir.clone());
        }
        if let Some(t) = state.library.remap_file(&file, &dest).and_then(|id| state.library.track(&id)) {
            out.push(t);
        }
    }
    for (dir, went_to) in &left {
        tidy_album_dir(dir, &albums, went_to);
    }
    out
}

#[tauri::command]
pub async fn artist_update(app: AppHandle, state: State<'_, AppState>, name: String, pick_image: bool, bio: Option<String>) -> CmdResult<crate::library::ArtistMeta> {
    let image = if pick_image {
        let a = app.clone();
        let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title("Choose an artist image").add_filter("Images", &["jpg", "jpeg", "png"]).blocking_pick_file())
            .await
            .map_err(|e| e.to_string())?;
        match picked.and_then(|p| p.into_path().ok()) {
            Some(p) => Some(p),
            None => return Err("cancelled".into()),
        }
    } else {
        None
    };
    let meta = state.library.set_artist(&name, image.as_deref(), bio).map_err(err)?;
    changed(&app);
    Ok(meta)
}

#[tauri::command]
pub async fn lyrics_get(state: State<'_, AppState>, query: Query, refresh: bool) -> CmdResult<Found> {
    let s = state.snapshot();
    lyrics::find(&state.http, &s, &query, refresh).await.map_err(err)
}

#[tauri::command]
pub fn lyrics_save(app: AppHandle, state: State<'_, AppState>, id: String, lyrics: Lyrics) -> CmdResult<()> {
    let t = state.library.track(&id).ok_or("That track is no longer in the library")?;
    std::fs::write(Path::new(&t.path).with_extension("lrc"), lyrics::to_lrc(&lyrics)).map_err(|e| e.to_string())?;
    let (_, pl) = dirs(&state);
    let _ = state.library.refresh(&id, &pl);
    changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn lyrics_save_text(app: AppHandle, state: State<'_, AppState>, id: String, text: String) -> CmdResult<()> {
    let t = state.library.track(&id).ok_or("That track is no longer in the library")?;
    let path = PathBuf::from(&t.path);
    let lrc = path.with_extension("lrc");
    let text = text.trim().to_string();
    if lyrics::is_timed(&text) {
        std::fs::write(&lrc, &text).map_err(|e| e.to_string())?;
    } else if lrc.exists() {
        let _ = trash::delete(&lrc);
    }
    let meta = TagFields { lyrics: if text.is_empty() { String::new() } else { text.clone() }, ..Default::default() };
    if !text.is_empty() {
        tagging::apply(&path, &meta, "", None).await.map_err(err)?;
    }
    let (_, pl) = dirs(&state);
    let _ = state.library.refresh(&id, &pl);
    changed(&app);
    Ok(())
}

#[tauri::command]
pub fn set_fullscreen(app: AppHandle, on: bool) -> CmdResult<bool> {
    let w = app.get_webview_window("main").ok_or("No window")?;
    w.set_fullscreen(on).map_err(|e| e.to_string())?;
    w.is_fullscreen().map_err(|e| e.to_string())
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase().as_str() {
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg; codecs=opus",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        _ => "application/octet-stream",
    }
}

fn respond(status: u16, ctype: &str, body: Vec<u8>, extra: &[(&str, String)]) -> Response<Vec<u8>> {
    let mut b = Response::builder().status(status).header("Content-Type", ctype).header("Access-Control-Allow-Origin", "*").header("Accept-Ranges", "bytes");
    for (k, v) in extra {
        b = b.header(*k, v.as_str());
    }
    b.body(body).unwrap_or_else(|_| Response::new(Vec::new()))
}

fn range_of(header: Option<&str>, len: u64) -> Option<(u64, u64)> {
    let spec = header?.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { (s + MAX_CHUNK - 1).min(len - 1) } else { b.parse::<u64>().ok()?.min(len - 1) };
        (s, e)
    };
    (start <= end && start < len).then_some((start, end))
}

fn stream(path: &Path, range: Option<&str>) -> Response<Vec<u8>> {
    let Ok(mut f) = std::fs::File::open(path) else { return respond(404, "text/plain", b"not found".to_vec(), &[]) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return respond(200, mime(path), Vec::new(), &[]);
    }
    match range_of(range, len) {
        Some((start, end)) => {
            let mut buf = vec![0u8; (end - start + 1) as usize];
            if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() {
                return respond(416, "text/plain", Vec::new(), &[]);
            }
            respond(206, mime(path), buf, &[("Content-Range", format!("bytes {start}-{end}/{len}"))])
        }
        None => {
            let mut buf = Vec::with_capacity(len as usize);
            let _ = f.read_to_end(&mut buf);
            respond(200, mime(path), buf, &[])
        }
    }
}

pub fn serve(app: &AppHandle, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let state = app.state::<AppState>();
    let path = request.uri().path().trim_start_matches('/').to_string();
    let range = request.headers().get("range").and_then(|v| v.to_str().ok());
    let (kind, id) = path.split_once('/').unwrap_or(("", ""));
    match kind {
        "t" => match state.library.track(id) {
            Some(t) => stream(Path::new(&t.path), range),
            None => respond(404, "text/plain", b"unknown track".to_vec(), &[]),
        },
        "v" => match state.library.track(id).and_then(|t| t.video) {
            Some(v) => stream(Path::new(&v), range),
            None => respond(404, "text/plain", b"no video".to_vec(), &[]),
        },
        "a" => match state.library.art_path(id) {
            Some(p) if p.extension().is_some_and(|e| e == "png") => match std::fs::read(&p) {
                Ok(data) => respond(200, "image/png", data, &[("Cache-Control", "max-age=86400".into())]),
                Err(_) => respond(404, "text/plain", Vec::new(), &[]),
            },
            Some(p) => {
                let data = std::fs::read(&p).unwrap_or_default();
                respond(200, "image/jpeg", data, &[("Cache-Control", "max-age=86400".into())])
            }
            None => respond(404, "text/plain", Vec::new(), &[]),
        },
        _ => respond(400, "text/plain", Vec::new(), &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_byte_ranges() {
        assert_eq!(range_of(Some("bytes=0-"), 10_000_000), Some((0, MAX_CHUNK - 1)));
        assert_eq!(range_of(Some("bytes=100-199"), 1000), Some((100, 199)));
        assert_eq!(range_of(Some("bytes=-100"), 1000), Some((900, 999)));
        assert_eq!(range_of(Some("bytes=5000-"), 1000), None);
        assert_eq!(range_of(None, 1000), None);
    }
}

static LOUDNESS: std::sync::LazyLock<tokio::sync::Semaphore> = std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(1));

pub fn gain_from_ebur128(log: &str) -> Option<f32> {
    let lufs = regex::Regex::new(r"I:\s+(-?[\d.]+)\s+LUFS").ok()?.captures_iter(log).last()?.get(1)?.as_str().parse::<f32>().ok()?;
    let peak = regex::Regex::new(r"Peak:\s+(-?[\d.]+|-inf)\s+dBFS").ok()?.captures_iter(log).last().and_then(|c| c[1].parse::<f32>().ok());
    if !lufs.is_finite() || lufs < -70.0 {
        return None;
    }
    let mut gain = (-14.0 - lufs).clamp(-20.0, 12.0);
    if let Some(p) = peak.filter(|p| p.is_finite()) {
        gain = gain.min(-1.0 - p);
    }
    Some((gain * 10.0).round() / 10.0)
}

#[tauri::command]
pub async fn track_loudness(state: State<'_, AppState>, id: String) -> CmdResult<Option<f32>> {
    let Some(t) = state.library.track(&id) else { return Ok(None) };
    if t.gain_db.is_some() {
        return Ok(t.gain_db);
    }
    let s = state.snapshot();
    let Some(ffmpeg) = crate::engine::locate(&s.ytdlp_path, &s.ffmpeg_path).ok().and_then(|t| t.ffmpeg).or_else(|| crate::engine::find_tool("ffmpeg")) else { return Ok(None) };
    let _permit = LOUDNESS.acquire().await.map_err(|e| e.to_string())?;
    if let Some(g) = state.library.track(&id).and_then(|t| t.gain_db) {
        return Ok(Some(g));
    }
    let out = crate::engine::command(&ffmpeg)
        .args(["-hide_banner", "-nostats", "-i", &t.path, "-map", "0:a:0", "-af", "ebur128=peak=true", "-f", "null", "-"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let gain = gain_from_ebur128(&String::from_utf8_lossy(&out.stderr));
    if let Some(g) = gain {
        state.library.set_gain(&id, g);
    }
    Ok(gain)
}

#[cfg(test)]
mod loudness_tests {
    use super::*;

    #[test]
    fn levels_toward_minus_14_without_clipping() {
        let loud = "Summary:\n  Integrated loudness:\n    I:          -8.0 LUFS\n  True peak:\n    Peak:        0.4 dBFS";
        assert_eq!(gain_from_ebur128(loud), Some(-6.0));
        let quiet = "    I:         -20.0 LUFS\n    Peak:       -9.0 dBFS";
        assert_eq!(gain_from_ebur128(quiet), Some(6.0));
        let peaky = "    I:         -20.0 LUFS\n    Peak:       -2.0 dBFS";
        assert_eq!(gain_from_ebur128(peaky), Some(1.0));
        assert_eq!(gain_from_ebur128("nothing"), None);
        assert_eq!(crate::library::parse_gain("-6.20 dB"), Some(-6.2));
    }
}

#[tauri::command]
pub async fn tracks_edit(app: AppHandle, state: State<'_, AppState>, ids: Vec<String>, meta: TagFields) -> CmdResult<usize> {
    let s = state.snapshot();
    let ffmpeg = crate::engine::locate(&s.ytdlp_path, &s.ffmpeg_path).ok().and_then(|t| t.ffmpeg);
    let (_, pl) = dirs(&state);
    let mut done = 0;
    let mut failed = Vec::new();
    let mut edits = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let Some(t) = state.library.track(id) else { continue };
        match tagging::apply(Path::new(&t.path), &meta, "", ffmpeg.as_deref()).await {
            Ok(()) => {
                if let Ok(after) = state.library.refresh(id, &pl) {
                    edits.push((t, after));
                }
                done += 1;
            }
            Err(e) => failed.push(format!("{} ({e})", t.title)),
        }
        let _ = app.emit(LIBRARY_EVENT, serde_json::json!({ "type": "editing", "done": i + 1, "total": ids.len() }));
    }
    file_by_album(&state, &edits);
    changed(&app);
    if done == 0 && !failed.is_empty() {
        return Err(format!("Couldn't update {}", failed.join(", ")));
    }
    Ok(done)
}

#[tauri::command]
pub fn playlist_cover(app: AppHandle, state: State<'_, AppState>, name: String, path: Option<String>) -> CmdResult<()> {
    let (_, pl) = dirs(&state);
    state.library.playlist_cover(&pl, &name, path.as_deref().map(Path::new)).map_err(err)?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn artist_fetch(state: State<'_, AppState>, name: String, force: bool) -> CmdResult<Option<crate::library::ArtistMeta>> {
    let s = state.snapshot();
    if !s.artist_info && !force {
        return Ok(None);
    }
    let existing = state.library.artist(&name).unwrap_or_default();
    if existing.checked.is_some() && !force {
        return Ok(Some(existing));
    }
    let want_photo = force || existing.image.is_none();
    let want_bio = force || existing.bio.trim().is_empty();
    let want_logo = force || existing.logo.is_none();
    let found = crate::artistinfo::find(&state.http, &name, want_photo, want_bio, want_logo).await.map_err(err)?;
    Ok(Some(state.library.apply_artist_info(&name, found, force)))
}

#[tauri::command]
pub async fn artist_media(app: AppHandle, state: State<'_, AppState>, name: String, kind: String, pick: bool) -> CmdResult<crate::library::ArtistMeta> {
    let image = if pick {
        let a = app.clone();
        let title = if kind == "logo" { "Choose a logo (a transparent PNG looks best)" } else { "Choose an image" };
        let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title(title).add_filter("Images", &["png", "jpg", "jpeg", "webp"]).blocking_pick_file())
            .await
            .map_err(|e| e.to_string())?;
        match picked.and_then(|p| p.into_path().ok()) {
            Some(p) => Some(p),
            None => return Err("cancelled".into()),
        }
    } else {
        None
    };
    state.library.artist_media(&name, &kind, image.as_deref()).map_err(err)
}

pub fn audio_args<I: IntoIterator<Item = String>>(args: I) -> Vec<String> {
    args.into_iter()
        .filter(|a| !a.starts_with('-'))
        .map(|a| a.strip_prefix("file://").map(|p| p.to_string()).unwrap_or(a))
        .filter(|a| {
            let p = Path::new(a);
            p.is_file() && crate::library::is_audio(p)
        })
        .collect()
}

#[tauri::command]
pub fn open_files(state: State<'_, AppState>, paths: Vec<String>) -> Vec<Track> {
    let list: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    state.library.open_paths(&list)
}

#[tauri::command]
pub fn startup_files(state: State<'_, AppState>) -> Vec<String> {
    std::mem::take(&mut *state.pending_open.lock().unwrap())
}

#[tauri::command]
pub async fn import_file(app: AppHandle, state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> CmdResult<Track> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else { return Err("Expected file contents".into()) };
    let raw_name = request.headers().get("x-file-name").and_then(|v| v.to_str().ok()).unwrap_or("Imported song");
    let name: String = url::form_urlencoded::parse(format!("n={raw_name}").as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default();
    let name: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let path0 = Path::new(&name);
    if !crate::library::is_audio(path0) || bytes.is_empty() {
        return Err("That isn't an audio file Atmosphere can play.".into());
    }
    let s = state.snapshot();
    let dir = PathBuf::from(&s.output_dir).join("Imported");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem = path0.file_stem().and_then(|x| x.to_str()).unwrap_or("Imported song").to_string();
    let ext = path0.extension().and_then(|x| x.to_str()).unwrap_or("mp3").to_string();
    let mut dest = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while dest.exists() {
        if std::fs::metadata(&dest).map(|m| m.len() as usize == bytes.len()).unwrap_or(false) {
            break;
        }
        dest = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    if !dest.exists() {
        std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
    }
    let (_, pl) = dirs(&state);
    let track = state.library.add_file(&dest, &pl).map_err(err)?;
    changed(&app);
    Ok(track)
}

#[tauri::command]
pub async fn artist_choices(state: State<'_, AppState>, query: String) -> CmdResult<serde_json::Value> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(serde_json::json!({ "photos": [], "pages": [], "logos": [] }));
    }
    let (photos, pages, logos) = crate::artistinfo::choices(&state.http, q).await;
    Ok(serde_json::json!({ "photos": photos, "pages": pages, "logos": logos }))
}

#[tauri::command]
pub async fn artist_apply(state: State<'_, AppState>, name: String, photo: Option<String>, page: Option<String>, logo: Option<String>) -> CmdResult<crate::library::ArtistMeta> {
    if photo.as_deref().is_some_and(|u| !crate::artistinfo::allowed_photo(u)) || logo.as_deref().is_some_and(|u| !crate::artistinfo::allowed_logo(u)) {
        return Err("That image isn't from a source Atmosphere trusts.".into());
    }
    let found = crate::artistinfo::chosen(&state.http, photo.as_deref(), page.as_deref(), logo.as_deref()).await;
    if found.photo.is_none() && found.bio.is_none() && found.logo.is_none() {
        return Err("Couldn't get anything from that choice. Try another one.".into());
    }
    Ok(state.library.apply_artist_info(&name, found, true))
}

#[tauri::command]
pub async fn album_fetch(state: State<'_, AppState>, artist: String, title: String, force: bool) -> CmdResult<Option<crate::library::AlbumMeta>> {
    if title.trim().is_empty() || (!state.snapshot().artist_info && !force) {
        return Ok(None);
    }
    let existing = state.library.album(&artist, &title).unwrap_or_default();
    if existing.checked.is_some() && !force {
        return Ok(Some(existing));
    }
    let found = crate::albuminfo::find(&state.http, &artist, &title, force || existing.bio.trim().is_empty(), force || existing.logo.is_none()).await;
    Ok(Some(state.library.apply_album_info(&artist, &title, found, force)))
}

#[tauri::command]
pub async fn album_choices(state: State<'_, AppState>, query: String, artist: String, title: String) -> CmdResult<serde_json::Value> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(serde_json::json!({ "covers": [], "pages": [], "logos": [] }));
    }
    let ((covers, pages), logos) = tokio::join!(crate::albuminfo::choices(&state.http, q, &artist, &title), crate::albuminfo::logo_choices(&state.http, q, &artist, &title));
    Ok(serde_json::json!({ "covers": covers, "pages": pages, "logos": logos }))
}

#[tauri::command]
pub async fn album_apply(state: State<'_, AppState>, artist: String, title: String, page: String) -> CmdResult<crate::library::AlbumMeta> {
    let found = crate::albuminfo::chosen(&state.http, &page).await;
    if found.bio.is_none() && found.logo.is_none() {
        return Err("Couldn't get anything from that page. Try another one.".into());
    }
    Ok(state.library.apply_album_info(&artist, &title, found, true))
}

#[tauri::command]
pub async fn album_cover_get(state: State<'_, AppState>, url: String) -> CmdResult<serde_json::Value> {
    use base64::Engine as _;
    let bytes = crate::albuminfo::cover(&state.http, &url).await.map_err(err)?;
    let dir = crate::engine::temp_root().join("covers");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.jpg", crate::library::hash(&url)));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let data_url = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
    Ok(serde_json::json!({ "path": path.to_string_lossy(), "dataUrl": data_url }))
}

#[tauri::command]
pub async fn album_media(app: AppHandle, state: State<'_, AppState>, artist: String, title: String, kind: String, pick: bool) -> CmdResult<crate::library::AlbumMeta> {
    let image = if pick {
        let a = app.clone();
        let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title("Choose a logo (a transparent PNG looks best)").add_filter("Images", &["png", "jpg", "jpeg", "webp"]).blocking_pick_file())
            .await
            .map_err(|e| e.to_string())?;
        match picked.and_then(|p| p.into_path().ok()) {
            Some(p) => Some(p),
            None => return Err("cancelled".into()),
        }
    } else {
        None
    };
    let meta = state.library.album_media(&artist, &title, &kind, image.as_deref()).map_err(err)?;
    changed(&app);
    Ok(meta)
}

#[tauri::command]
pub async fn album_logo_set(app: AppHandle, state: State<'_, AppState>, artist: String, title: String, url: String) -> CmdResult<crate::library::AlbumMeta> {
    let bytes = crate::albuminfo::logo(&state.http, &url).await.map_err(err)?;
    let meta = state.library.set_album_logo(&artist, &title, &bytes).map_err(err)?;
    changed(&app);
    Ok(meta)
}

#[tauri::command]
pub fn album_update(app: AppHandle, state: State<'_, AppState>, artist: String, title: String, bio: String) -> CmdResult<crate::library::AlbumMeta> {
    let meta = state.library.set_album_bio(&artist, &title, bio).map_err(err)?;
    changed(&app);
    Ok(meta)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderReport {
    files: usize,
    bytes: u64,
}

#[tauri::command]
pub async fn folder_report(path: String) -> FolderReport {
    tauri::async_runtime::spawn_blocking(move || {
        let mut files = 0;
        let mut bytes = 0;
        for e in walkdir::WalkDir::new(&path).into_iter().flatten().filter(|e| e.file_type().is_file()) {
            files += 1;
            bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
        FolderReport { files, bytes }
    })
    .await
    .unwrap_or(FolderReport { files: 0, bytes: 0 })
}

fn move_one(src: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dest)?;
    std::fs::remove_file(src)
}

pub fn move_tree(from: &Path, to: &Path, mut progress: impl FnMut(usize, usize)) -> (usize, Vec<String>) {
    let files: Vec<PathBuf> = walkdir::WalkDir::new(from).into_iter().flatten().filter(|e| e.file_type().is_file()).map(|e| e.into_path()).collect();
    let total = files.len();
    let mut moved = 0;
    let mut failed = Vec::new();
    for (i, src) in files.iter().enumerate() {
        let Ok(rel) = src.strip_prefix(from) else { continue };
        let mut dest = to.join(rel);
        if dest.exists() {
            let same = std::fs::metadata(&dest).ok().map(|m| m.len()) == std::fs::metadata(src).ok().map(|m| m.len());
            if same {
                if std::fs::remove_file(src).is_ok() {
                    moved += 1;
                }
                progress(i + 1, total);
                continue;
            }
            let stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = dest.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let mut n = 2;
            while dest.exists() {
                dest = dest.with_file_name(format!("{stem} ({n}){ext}"));
                n += 1;
            }
        }
        match move_one(src, &dest) {
            Ok(()) => moved += 1,
            Err(e) => failed.push(format!("{} ({e})", rel.display())),
        }
        if i % 10 == 0 || i + 1 == total {
            progress(i + 1, total);
        }
    }
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(from).into_iter().flatten().filter(|e| e.file_type().is_dir()).map(|e| e.into_path()).collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        let _ = std::fs::remove_dir(d);
    }
    (moved, failed)
}

#[tauri::command]
pub async fn move_music(app: AppHandle, state: State<'_, AppState>, to: String) -> CmdResult<serde_json::Value> {
    if state.dl.busy() {
        return Err("Wait for downloads to finish before moving your music.".into());
    }
    let s = state.snapshot();
    let from = PathBuf::from(&s.output_dir);
    let dest = PathBuf::from(&to);
    if dest.starts_with(&from) || from.starts_with(&dest) {
        return Err("Choose a folder that isn't inside the current one (or the other way round).".into());
    }
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let emitter = app.clone();
    let (f2, d2) = (from.clone(), dest.clone());
    let (moved, failed) = tauri::async_runtime::spawn_blocking(move || {
        move_tree(&f2, &d2, |done, total| {
            let _ = emitter.emit("atmosphere://move", serde_json::json!({ "done": done, "total": total }));
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    state.library.remap_paths(&from, &dest);
    let remap = |p: &str| -> String { Path::new(p).strip_prefix(&from).map(|r| dest.join(r).to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string()) };
    let view = state.update(|st| {
        st.output_dir = dest.to_string_lossy().into_owned();
        st.playlists_dir = remap(&st.playlists_dir);
        let mut seen = std::collections::HashSet::new();
        st.library_folders = st.library_folders.iter().map(|f| remap(f)).filter(|f| seen.insert(f.clone())).collect();
    })?;
    spawn_scan(app.clone());
    start_watch(app.clone());
    changed(&app);
    Ok(serde_json::json!({ "moved": moved, "failed": failed, "outputDir": view.output_dir }))
}

#[cfg(test)]
mod move_tests {
    use super::*;

    #[test]
    fn songs_given_an_album_move_into_its_folder_with_their_lyrics() {
        let base = std::env::temp_dir().join(format!("atmo-album-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (tracks, albums) = (base.join("Tracks"), base.join("Albums"));
        std::fs::create_dir_all(albums.join("Random Access Memories")).unwrap();
        std::fs::create_dir_all(&tracks).unwrap();
        let song = tracks.join("Daft Punk - Get Lucky.m4a");
        std::fs::write(&song, b"a").unwrap();
        std::fs::write(tracks.join("Daft Punk - Get Lucky.lrc"), b"l").unwrap();
        std::fs::write(tracks.join("Daft Punk - Get Lucky.mp4"), b"v").unwrap();

        assert_eq!(album_home(&base, &song, "random access memories", None), Some(albums.join("Random Access Memories")));
        assert_eq!(album_home(&base, &song, "Brand New", None), Some(albums.join("Brand New")));
        assert_eq!(album_home(&base, &song, "", None), None);
        assert_eq!(album_home(&base, &base.join("Mine").join("x.mp3"), "Brand New", None), None);
        assert_eq!(album_home(&base, &albums.join("Random Access Memories").join("x.mp3"), "Random Access Memories", None), None);
        assert_eq!(album_home(&base, &song, "RAM", Some(albums.join("Random Access Memories"))), Some(albums.join("Random Access Memories")));

        let dest = move_with_sidecars(&song, &albums.join("Random Access Memories")).unwrap();
        assert_eq!(dest, albums.join("Random Access Memories").join("Daft Punk - Get Lucky.m4a"));
        for ext in ["m4a", "lrc", "mp4"] {
            assert!(albums.join("Random Access Memories").join(format!("Daft Punk - Get Lucky.{ext}")).is_file());
            assert!(!tracks.join(format!("Daft Punk - Get Lucky.{ext}")).exists());
        }

        let old = albums.join("Old Name");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("cover.jpg"), b"c").unwrap();
        tidy_album_dir(&old, &albums, &HashSet::from([albums.join("Random Access Memories")]));
        assert!(!old.exists());
        assert!(albums.join("Random Access Memories").join("cover.jpg").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn moves_a_tree_and_keeps_both_on_name_clashes() {
        let root = std::env::temp_dir().join(format!("atmo-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(a.join("Albums").join("X")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("Albums").join("X").join("1.mp3"), b"one").unwrap();
        std::fs::write(a.join("same.mp3"), b"same").unwrap();
        std::fs::write(b.join("same.mp3"), b"same").unwrap();
        std::fs::write(a.join("clash.mp3"), b"mine").unwrap();
        std::fs::write(b.join("clash.mp3"), b"theirs!").unwrap();
        let (moved, failed) = move_tree(&a, &b, |_, _| {});
        assert!(failed.is_empty());
        assert_eq!(moved, 3);
        assert!(b.join("Albums").join("X").join("1.mp3").exists());
        assert_eq!(std::fs::read(b.join("clash (2).mp3")).unwrap(), b"mine");
        assert_eq!(std::fs::read(b.join("clash.mp3")).unwrap(), b"theirs!");
        assert!(!a.join("Albums").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
