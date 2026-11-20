// atmosphere · updates.rs · Checks GitHub Releases for signed updates and installs them, or links to the download where self-update isn't possible
use crate::{err, AppState, CmdResult};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;

pub const RELEASES: &str = "https://github.com/pvrzz/atmosphere/releases/latest";
const PROGRESS_EVENT: &str = "atmosphere://update";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current: String,
    notes: Option<String>,
    date: Option<String>,
    can_install: bool,
    page: String,
}

pub fn can_self_update() -> bool {
    crate::engine::portable_root().is_none()
}

#[tauri::command]
pub fn app_version(app: AppHandle) -> serde_json::Value {
    serde_json::json!({ "version": app.package_info().version.to_string(), "canSelfUpdate": can_self_update(), "page": RELEASES })
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> CmdResult<Option<UpdateInfo>> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let found = match updater.check().await {
        Ok(found) => found,
        Err(tauri_plugin_updater::Error::ReleaseNotFound) => None,
        Err(e) => return Err(format!("Couldn't reach GitHub to check for updates ({e})")),
    };
    Ok(found.map(|u| UpdateInfo {
        version: u.version.clone(),
        current: u.current_version.clone(),
        notes: u.body.as_deref().map(|b| b.split("<!-- install-notes -->").next().unwrap_or(b).trim().to_string()).filter(|b| !b.is_empty()),
        date: u.date.map(|d| d.to_string()),
        can_install: can_self_update(),
        page: RELEASES.into(),
    }))
}

#[tauri::command]
pub async fn update_install(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    if !can_self_update() {
        return Err("This copy of Atmosphere can't update itself. Download the new version instead.".into());
    }
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("Couldn't check for updates ({e})"))?
        .ok_or("Atmosphere is already up to date.")?;
    state.dl.cancel_all();
    let mut received: u64 = 0;
    let progress = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                received += chunk as u64;
                let _ = progress.emit(PROGRESS_EVENT, serde_json::json!({ "received": received, "total": total }));
            },
            || {},
        )
        .await
        .map_err(|e| err(anyhow::anyhow!("The update couldn't be installed ({e})")))?;
    crate::storage::wipe_temp();
    app.restart();
}
