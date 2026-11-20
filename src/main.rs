// atmosphere · main.rs · App entry point: shared state, the rights gate and every command the UI can call.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod artistinfo;
mod backup;
mod direct;
mod discord;
mod downloader;
mod engine;
mod history;
mod libcmd;
mod library;
mod lyrics;
mod media;
mod settings;
mod storage;
mod tagging;
mod updates;

use base64::Engine as _;
use downloader::{Downloader, JobRequest, SaveOptions};
use history::{History, Record};
use media::{Collection, Entry, Prober};
use serde::Serialize;
use settings::{Settings, SettingsPatch, TERMS_VERSION};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

pub(crate) struct AppState {
    settings: Arc<Mutex<Settings>>,
    pub(crate) http: reqwest::Client,
    prober: Arc<Prober>,
    dl: Arc<Downloader>,
    history: Arc<History>,
    pub(crate) library: Arc<library::Library>,
    startup_cleanup: Mutex<Option<(usize, u64)>>,
    pending_open: Mutex<Vec<String>>,
}

pub(crate) type CmdResult<T> = Result<T, String>;

pub(crate) fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

impl AppState {
    pub(crate) fn snapshot(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub(crate) fn update(&self, f: impl FnOnce(&mut Settings)) -> CmdResult<Settings> {
        let mut s = self.settings.lock().unwrap();
        f(&mut s);
        settings::save(&s).map_err(err)?;
        Ok(s.clone())
    }

    fn require_rights(&self) -> CmdResult<Settings> {
        let s = self.snapshot();
        if !s.rights_ok() {
            return Err("Confirm the rights acknowledgment before using Atmosphere.".into());
        }
        Ok(s)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    #[serde(flatten)]
    settings: Settings,
    rights_accepted: bool,
    terms_version: u32,
}

fn view(s: Settings) -> SettingsView {
    SettingsView { rights_accepted: s.rights_ok(), terms_version: TERMS_VERSION, settings: s }
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> SettingsView {
    view(state.snapshot())
}

#[tauri::command]
fn accept_rights(state: State<'_, AppState>, version: u32) -> CmdResult<SettingsView> {
    if version != TERMS_VERSION {
        return Err("The acknowledgment has changed. Please review it again.".into());
    }
    Ok(view(state.update(Settings::accept_rights)?))
}

#[tauri::command]
fn revoke_rights(state: State<'_, AppState>) -> CmdResult<SettingsView> {
    state.dl.cancel_all();
    Ok(view(state.update(Settings::revoke_rights)?))
}

#[tauri::command]
fn save_settings(app: AppHandle, state: State<'_, AppState>, patch: SettingsPatch) -> CmdResult<SettingsView> {
    let before = state.snapshot();
    let after = state.update(|s| s.apply(patch))?;
    if after.concurrency != before.concurrency {
        state.dl.set_concurrency(after.concurrency);
    }
    if (after.api_enabled, after.api_port) != (before.api_enabled, before.api_port) {
        api::apply(&app);
    }
    let opts = discord::Options::from(&after);
    if opts != discord::Options::from(&before) {
        app.state::<discord::DiscordHub>().configure(opts);
    }
    Ok(view(after))
}

#[tauri::command]
fn api_status(hub: State<'_, api::ApiHub>) -> api::Status {
    hub.status()
}

#[tauri::command]
fn api_new_token(app: AppHandle, hub: State<'_, api::ApiHub>) -> CmdResult<String> {
    let token = hub.new_token().map_err(err)?;
    api::apply(&app);
    Ok(token)
}

#[tauri::command]
fn discord_status(hub: State<'_, discord::DiscordHub>) -> discord::Status {
    hub.status()
}

#[tauri::command]
fn discord_now(hub: State<'_, discord::DiscordHub>, now: Option<discord::NowPlaying>) {
    hub.now_playing(now);
}

#[tauri::command]
async fn discord_save_assets(app: AppHandle) -> CmdResult<Option<String>> {
    let start = dirs::desktop_dir().or_else(dirs::download_dir).unwrap_or_default();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_directory(start).set_title("Choose where to save the Discord images").blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(dir) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let dir = dir.join("Atmosphere for Discord");
    discord::write_assets(&dir).map_err(err)?;
    Ok(Some(dir.to_string_lossy().into_owned()))
}

#[tauri::command]
fn api_publish(hub: State<'_, api::ApiHub>, state: serde_json::Value) {
    hub.publish(state);
}

#[tauri::command]
async fn tools_status(state: State<'_, AppState>) -> CmdResult<engine::ToolStatus> {
    let s = state.snapshot();
    Ok(engine::status(&s.ytdlp_path, &s.ffmpeg_path).await)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallProgress {
    tool: String,
    received: u64,
    total: Option<u64>,
}

#[tauri::command]
async fn install_tool(app: AppHandle, state: State<'_, AppState>, tool: String) -> CmdResult<engine::ToolStatus> {
    let emit = |received: u64, total: Option<u64>| {
        let _ = app.emit("atmosphere://install", InstallProgress { tool: tool.clone(), received, total });
    };
    match tool.as_str() {
        "ytdlp" => engine::install_ytdlp(&state.http, emit).await.map_err(err)?,
        "ffmpeg" => engine::install_ffmpeg(&state.http, emit).await.map_err(err)?,
        _ => return Err("Unknown tool".into()),
    };
    let s = state.snapshot();
    Ok(engine::status(&s.ytdlp_path, &s.ffmpeg_path).await)
}

#[tauri::command]
async fn update_ytdlp(state: State<'_, AppState>) -> CmdResult<String> {
    let s = state.snapshot();
    let tools = engine::locate(&s.ytdlp_path, &s.ffmpeg_path).map_err(err)?;
    engine::update_ytdlp(&tools.ytdlp).await.map_err(err)
}

#[tauri::command]
async fn probe(app: AppHandle, state: State<'_, AppState>, url: String) -> CmdResult<Collection> {
    let s = state.require_rights()?;
    state.prober.probe(&app, &s, url.trim()).await.map_err(err)
}

#[tauri::command]
async fn search(state: State<'_, AppState>, platform: String, query: String) -> CmdResult<Collection> {
    let s = state.require_rights()?;
    state.prober.search(&s, &platform, query.trim()).await.map_err(err)
}

#[tauri::command]
async fn detail(state: State<'_, AppState>, url: String, index: u32) -> CmdResult<Entry> {
    let s = state.require_rights()?;
    state.prober.detail(&s, &url, index).await.map_err(err)
}

#[tauri::command]
fn forget(state: State<'_, AppState>, id: String) {
    state.prober.cancel(&id);
}

#[tauri::command]
fn enqueue(state: State<'_, AppState>, jobs: Vec<JobRequest>, save: SaveOptions, attested: bool) -> CmdResult<Vec<u64>> {
    state.require_rights()?;
    if !attested {
        return Err("Confirm you have the rights to download these items first.".into());
    }
    Ok(state.dl.enqueue(jobs, save))
}

#[tauri::command]
fn cancel(state: State<'_, AppState>, id: u64) {
    state.dl.cancel(id);
}

#[tauri::command]
fn cancel_all(state: State<'_, AppState>) {
    state.dl.cancel_all();
}

#[tauri::command]
fn history_list(state: State<'_, AppState>) -> Vec<Record> {
    state.history.list()
}

#[tauri::command]
fn history_remove(state: State<'_, AppState>, ids: Vec<String>) {
    state.history.remove(&ids);
}

#[tauri::command]
fn history_clear(state: State<'_, AppState>) {
    state.history.clear();
}

#[tauri::command]
async fn pick_folder(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let start = state.snapshot().output_dir;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_directory(start).set_title("Choose where downloads are saved").blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PickedImage {
    path: String,
    data_url: String,
}

#[tauri::command]
async fn pick_image(app: AppHandle) -> CmdResult<Option<PickedImage>> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("Choose cover art").add_filter("Images", &["jpg", "jpeg", "png"]).blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let data = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
    if data.len() > 20 * 1024 * 1024 {
        return Err("That image is over 20 MB. Pick a smaller cover.".into());
    }
    let mime = if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else {
        return Err("Cover art must be a JPEG or PNG image.".into());
    };
    let data_url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&data));
    Ok(Some(PickedImage { path: path.to_string_lossy().into_owned(), data_url }))
}

#[tauri::command]
fn reveal(app: AppHandle, path: String) -> CmdResult<()> {
    let p = std::path::PathBuf::from(&path);
    let result = if p.is_file() {
        app.opener().reveal_item_in_dir(&p)
    } else {
        std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
        app.opener().open_path(path, None::<&str>)
    };
    result.map_err(|e| e.to_string())
}

#[tauri::command]
fn open_link(app: AppHandle, url: String) -> CmdResult<()> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("Only web links can be opened.".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn storage_report(state: State<'_, AppState>) -> Vec<storage::Item> {
    let s = state.snapshot();
    let lib = state.library.snapshot(std::path::Path::new(&s.playlists_dir));
    storage::report(&s, &lib, state.dl.busy())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Cleaned {
    files: usize,
    bytes: u64,
}

#[tauri::command]
fn storage_clean(state: State<'_, AppState>, ids: Vec<String>) -> Cleaned {
    let s = state.snapshot();
    let lib = state.library.snapshot(std::path::Path::new(&s.playlists_dir));
    let (files, bytes) = storage::clean(&s, &lib, &ids, state.dl.busy());
    Cleaned { files, bytes }
}

#[tauri::command]
async fn backup_save(app: AppHandle, name: String) -> CmdResult<Option<(String, usize)>> {
    let a = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut d = a.dialog().file().set_title("Back up Atmosphere").set_file_name(&name).add_filter("Atmosphere backup", &["zip"]);
        if let Some(docs) = dirs::document_dir() {
            d = d.set_directory(docs);
        }
        d.blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let p = path.clone();
    let n = tauri::async_runtime::spawn_blocking(move || backup::create(&p)).await.map_err(|e| e.to_string())?.map_err(err)?;
    Ok(Some((path.to_string_lossy().into_owned(), n)))
}

#[tauri::command]
async fn backup_pick(app: AppHandle) -> CmdResult<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_title("Restore from a backup").add_filter("Atmosphere backup", &["zip"]).blocking_pick_file())
        .await
        .map_err(|e| e.to_string())?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn backup_restore(app: AppHandle, state: State<'_, AppState>, path: String) -> CmdResult<()> {
    if state.dl.busy() {
        return Err("Wait for downloads to finish before restoring.".into());
    }
    tauri::async_runtime::spawn_blocking(move || backup::restore(std::path::Path::new(&path))).await.map_err(|e| e.to_string())?.map_err(err)?;
    app.restart();
}

fn parse_hex(color: &str) -> Option<(u8, u8, u8)> {
    let c = color.trim().trim_start_matches('#');
    if c.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(c, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[tauri::command]
fn frame_color(window: tauri::WebviewWindow, color: String) {
    let Some((r, g, b)) = parse_hex(&color) else { return };
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR};
        let colorref: u32 = (r as u32) | ((g as u32) << 8) | ((b as u32) << 16);
        unsafe {
            DwmSetWindowAttribute(hwnd.0 as _, DWMWA_BORDER_COLOR as _, &colorref as *const u32 as _, 4);
        }
    }
    #[cfg(not(windows))]
    let _ = (window, r, g, b);
}

#[tauri::command]
fn startup_cleanup(state: State<'_, AppState>) -> Option<Cleaned> {
    state.startup_cleanup.lock().unwrap().take().filter(|(n, _)| *n > 0).map(|(files, bytes)| Cleaned { files, bytes })
}

#[tauri::command]
async fn signin_check(state: State<'_, AppState>, browser: String, file: String) -> CmdResult<String> {
    let mut s = state.snapshot();
    s.cookies_browser = browser;
    s.cookies_file = file;
    media::check_signin(&s).await.map_err(err)
}

#[tauri::command]
async fn pick_cookies_file(app: AppHandle) -> CmdResult<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("Choose a cookies.txt file").add_filter("Cookies", &["txt"]).blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn pick_text_file(app: AppHandle) -> CmdResult<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("Import lyrics").add_filter("Lyrics", &["lrc", "txt"]).blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let data = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
    if data.len() > 1024 * 1024 {
        return Err("That file is too large to be lyrics.".into());
    }
    Ok(Some(String::from_utf8_lossy(&data).trim_start_matches('\u{feff}').to_string()))
}

#[tauri::command]
fn clipboard_text(app: AppHandle) -> String {
    app.clipboard().read_text().unwrap_or_default()
}

#[tauri::command]
fn clipboard_write(app: AppHandle, text: String) -> CmdResult<()> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_clipboard(app: AppHandle, state: State<'_, AppState>) -> Option<String> {
    if !state.snapshot().watch_clipboard {
        return None;
    }
    let text = app.clipboard().read_text().ok()?;
    let t = text.trim();
    (t.len() < 2048 && (t.starts_with("https://") || t.starts_with("http://")) && !t.contains(char::is_whitespace)).then(|| t.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--backup") {
        let code = match args.get(i + 1) {
            Some(dest) => i32::from(backup::create(std::path::Path::new(dest)).is_err()),
            None => 2,
        };
        std::process::exit(code);
    }
    let mut loaded = settings::load();
    if loaded.accept_from_installer() {
        let _ = settings::save(&loaded);
    }
    let settings = Arc::new(Mutex::new(loaded));
    let http = reqwest::Client::builder()
        .user_agent(concat!("atmosphere/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(90))
        .build()
        .expect("http client");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
            let files = libcmd::audio_args(argv.into_iter().skip(1));
            if !files.is_empty() {
                let _ = app.emit("atmosphere://open-files", files);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .register_asynchronous_uri_scheme_protocol("atmo", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || responder.respond(libcmd::serve(&app, &request)));
        })
        .setup(move |app| {
            let history = Arc::new(History::load(engine::data_dir().join("history.json")));
            let cleaned = storage::startup(&settings.lock().unwrap());
            let library = Arc::new(library::Library::load());
            let dl = Arc::new(Downloader::new(app.handle().clone(), settings.clone(), history.clone(), library.clone(), http.clone()));
            app.manage(AppState { settings: settings.clone(), http: http.clone(), prober: Arc::new(Prober::default()), dl, history, library, startup_cleanup: Mutex::new(Some(cleaned)), pending_open: Mutex::new(libcmd::audio_args(std::env::args().skip(1))) });
            app.manage(api::ApiHub::load());
            app.manage(discord::DiscordHub::start(app.handle().clone(), http.clone(), discord::Options::from(&settings.lock().unwrap())));
            api::apply(app.handle());
            libcmd::spawn_scan(app.handle().clone());
            libcmd::start_linked_monitor(app.handle().clone());
            libcmd::start_watch(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            signin_check,
            updates::app_version,
            updates::update_check,
            updates::update_install,
            api_status,
            api_new_token,
            api_publish,
            discord_status,
            discord_now,
            discord_save_assets,
            frame_color,
            backup_save,
            backup_pick,
            backup_restore,
            pick_cookies_file,
            get_settings,
            accept_rights,
            revoke_rights,
            save_settings,
            tools_status,
            install_tool,
            update_ytdlp,
            probe,
            search,
            detail,
            forget,
            enqueue,
            cancel,
            cancel_all,
            history_list,
            history_remove,
            history_clear,
            pick_folder,
            pick_image,
            reveal,
            open_link,
            read_clipboard,
            clipboard_text,
            pick_text_file,
            clipboard_write,
            storage_report,
            storage_clean,
            startup_cleanup,
            libcmd::library_get,
            libcmd::library_scan,
            libcmd::library_add_folder,
            libcmd::library_remove_folder,
            libcmd::playlist_create,
            libcmd::playlist_rename,
            libcmd::playlist_delete,
            libcmd::playlist_add,
            libcmd::playlist_add_files,
            libcmd::playlist_remove,
            libcmd::playlist_reorder,
            libcmd::track_played,
            libcmd::track_listened,
            libcmd::app_heartbeat,
            libcmd::stats_get,
            libcmd::image_save,
            libcmd::image_copy,
            libcmd::tracks_delete,
            libcmd::tracks_hide,
            libcmd::tracks_unhide_all,
            libcmd::track_like,
            libcmd::track_edit,
            libcmd::artist_update,
            libcmd::lyrics_get,
            libcmd::lyrics_save,
            libcmd::lyrics_save_text,
            libcmd::set_fullscreen,
            libcmd::playlist_link,
            libcmd::track_loudness,
            libcmd::tracks_edit,
            libcmd::open_files,
            libcmd::folder_report,
            libcmd::move_music,
            libcmd::startup_files,
            libcmd::import_file,
            libcmd::artist_fetch,
            libcmd::artist_media,
            libcmd::artist_choices,
            libcmd::artist_apply,
            libcmd::playlist_cover,
            libcmd::playlist_relocate,
            libcmd::playlist_unlink
        ])
        .build(tauri::generate_context!())
        .expect("error while building atmosphere")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                let files: Vec<String> = urls.iter().filter_map(|u| u.to_file_path().ok()).map(|p| p.to_string_lossy().into_owned()).collect();
                let files = libcmd::audio_args(files);
                if !files.is_empty() {
                    if let Some(state) = app.try_state::<AppState>() {
                        state.pending_open.lock().unwrap().extend(files.clone());
                    }
                    let _ = app.emit("atmosphere://open-files", files);
                }
            }
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.dl.cancel_all();
                    storage::clear_stalled(&state.snapshot());
                }
                storage::wipe_temp();
            }
        });
}
