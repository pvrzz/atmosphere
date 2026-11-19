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
                if crate::library::is_audio(e.path()) && !state.library.knows(e.path()) {
                    touched |= state.library.add_file(e.path(), &pl).is_ok();
                }
            }
        } else if crate::library::is_audio(&p) && !state.library.knows(&p) {
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
    let updated = state.library.refresh(&id, &pl).map_err(err)?;
    changed(&app);
    Ok(updated)
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
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg; codecs=opus",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
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
        "a" => match state.library.art_path(id) {
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
