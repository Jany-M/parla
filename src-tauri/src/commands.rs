//! Thin command layer: every command forwards to `parla_core::service::Service`.
use crate::state::AppState;
use parla_core::audio::device::{list_inputs, list_outputs, DeviceInfo};
use parla_core::models::Cancel;
use parla_core::service::{ModelStatus, SystemInfo};
use parla_core::settings::Settings;
use parla_core::types::{MeetingDetail, MeetingListItem};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

type Res<T> = Result<T, String>;

/// Every command failure shown to the user is also written to the log file.
fn err(e: anyhow::Error) -> String {
    let m = format!("{e:#}");
    log::error!("command failed: {m}");
    m
}

/// Heavy work never runs on the UI thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(err)
}

#[derive(Serialize)]
pub struct AppInfo {
    pub version: String,
    pub system: SystemInfo,
}

/// Opens the folder that holds parla.log in the system file manager.
#[tauri::command]
pub fn open_logs(s: State<AppState>) -> Res<()> {
    let dir = s.svc.paths.logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let opener = "xdg-open";
    // explorer.exe reports exit code 1 even on success, so only a failure to start counts
    std::process::Command::new(opener).arg(&dir).spawn().map(|_| ()).map_err(|e| {
        log::error!("could not open the logs folder: {e}");
        e.to_string()
    })
}

/// Errors from the web UI land in the same log file.
#[tauri::command]
pub fn log_ui_error(message: String) {
    log::error!("ui: {}", message.chars().take(2000).collect::<String>());
}

#[tauri::command]
pub fn app_info(app: AppHandle, s: State<AppState>) -> AppInfo {
    AppInfo { version: app.package_info().version.to_string(), system: s.svc.system_info() }
}

#[tauri::command]
pub fn get_settings(s: State<AppState>) -> Settings {
    s.svc.settings()
}

#[tauri::command]
pub fn save_settings(s: State<AppState>, settings: Settings) -> Res<()> {
    s.svc.update_settings(settings).map_err(err)
}

#[derive(Serialize)]
pub struct Lang {
    pub code: &'static str,
    pub name: &'static str,
}

#[tauri::command]
pub fn list_languages() -> Vec<Lang> {
    parla_core::languages::LANGUAGES.iter().map(|l| Lang { code: l.code, name: l.name }).collect()
}

#[derive(Serialize)]
pub struct Devices {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

#[tauri::command]
pub async fn list_devices() -> Res<Devices> {
    blocking(|| Ok(Devices { inputs: list_inputs(), outputs: list_outputs() })).await
}

// ---- models ---------------------------------------------------------------

#[tauri::command]
pub fn model_status(s: State<AppState>) -> Vec<ModelStatus> {
    s.svc.model_status()
}

#[derive(Serialize, Clone)]
struct ModelProgress {
    id: String,
    done: u64,
    total: u64,
}

#[tauri::command]
pub async fn install_model(app: AppHandle, s: State<'_, AppState>, id: String) -> Res<()> {
    let cancel = Cancel::default();
    *s.install_cancel.lock().unwrap() = cancel.clone();
    let svc = s.svc.clone();
    blocking(move || {
        let mut last = 0u64;
        svc.install_model(&id, &cancel, &mut |done, total| {
            // ~1 event per MB keeps the UI smooth without flooding the bridge
            if done == total || done.saturating_sub(last) >= 1 << 20 {
                last = done;
                let _ = app.emit("model-progress", ModelProgress { id: id.clone(), done, total });
            }
        })
    })
    .await
}

#[tauri::command]
pub fn cancel_install(s: State<AppState>) {
    s.install_cancel.lock().unwrap().cancel();
}

#[tauri::command]
pub fn uninstall_model(s: State<AppState>, id: String) -> Res<()> {
    s.svc.uninstall_model(&id).map_err(err)
}

/// Meetings, downloaded models and settings — everything except the app itself.
#[tauri::command]
pub async fn delete_all_data(s: State<'_, AppState>) -> Res<()> {
    let svc = s.svc.clone();
    blocking(move || {
        svc.stop_live()?;
        svc.store().delete_all_meetings()?;
        svc.remove_all_models()?;
        svc.update_settings(Settings::default())
    })
    .await
}

// ---- recording ------------------------------------------------------------

#[tauri::command]
pub async fn start_live(s: State<'_, AppState>) -> Res<i64> {
    let svc = s.svc.clone();
    blocking(move || svc.start_live()).await
}

#[tauri::command]
pub async fn stop_live(s: State<'_, AppState>) -> Res<Option<i64>> {
    let svc = s.svc.clone();
    blocking(move || svc.stop_live()).await
}

#[tauri::command]
pub fn set_paused(s: State<AppState>, paused: bool) -> bool {
    s.svc.set_paused(paused)
}

#[tauri::command]
pub fn is_paused(s: State<AppState>) -> bool {
    s.svc.is_paused()
}

#[tauri::command]
pub fn recording_state(s: State<AppState>) -> Option<i64> {
    s.svc.is_recording()
}

#[tauri::command]
pub async fn start_translation(s: State<'_, AppState>, target: String) -> Res<()> {
    let svc = s.svc.clone();
    blocking(move || svc.start_translation(&target)).await
}

#[tauri::command]
pub async fn stop_translation(s: State<'_, AppState>) -> Res<()> {
    let svc = s.svc.clone();
    blocking(move || {
        svc.stop_translation();
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn translation_target(s: State<AppState>) -> Option<String> {
    s.svc.translation_target()
}

// ---- import ---------------------------------------------------------------

#[tauri::command]
pub async fn import_file(s: State<'_, AppState>, path: String) -> Res<i64> {
    let cancel = Arc::new(AtomicBool::new(false));
    *s.import_cancel.lock().unwrap() = cancel.clone();
    let svc = s.svc.clone();
    blocking(move || svc.import_file(&PathBuf::from(path), cancel)).await
}

#[tauri::command]
pub fn cancel_import(s: State<AppState>) {
    s.import_cancel.lock().unwrap().store(true, Ordering::SeqCst);
}

// ---- library --------------------------------------------------------------

#[tauri::command]
pub fn list_meetings(s: State<AppState>) -> Res<Vec<MeetingListItem>> {
    s.svc.store().list_meetings().map_err(err)
}

#[tauri::command]
pub fn get_meeting(s: State<AppState>, id: i64) -> Res<Option<MeetingDetail>> {
    s.svc.store().get_meeting(id).map_err(err)
}

#[tauri::command]
pub fn rename_meeting(s: State<AppState>, id: i64, title: String) -> Res<()> {
    s.svc.store().set_title(id, &title).map_err(err)
}

#[tauri::command]
pub fn set_notes(s: State<AppState>, id: i64, notes: String) -> Res<()> {
    s.svc.store().set_notes(id, &notes).map_err(err)
}

#[tauri::command]
pub fn delete_meeting(s: State<AppState>, id: i64) -> Res<()> {
    s.svc.store().delete_meeting(id).map_err(err)
}

#[tauri::command]
pub fn rename_speaker(s: State<AppState>, id: i64, name: String) -> Res<()> {
    s.svc.store().rename_speaker(id, &name).map_err(err)
}

#[tauri::command]
pub fn merge_speakers(s: State<AppState>, from: i64, into: i64) -> Res<()> {
    s.svc.store().merge_speakers(from, into).map_err(err)
}

#[derive(Serialize, Clone)]
struct SummaryProgress {
    meeting_id: i64,
    done: usize,
    total: usize,
}

#[tauri::command]
pub async fn summarize(app: AppHandle, s: State<'_, AppState>, id: i64) -> Res<String> {
    let svc = s.svc.clone();
    blocking(move || svc.summarize(id, &|done, total| {
        let _ = app.emit("summary-progress", SummaryProgress { meeting_id: id, done, total });
    }))
    .await
}

#[tauri::command]
pub fn export_markdown(s: State<AppState>, id: i64, date_label: String) -> Res<String> {
    let d = s.svc.store().get_meeting(id).map_err(err)?.ok_or("meeting not found")?;
    Ok(parla_core::export::to_markdown(&d, &date_label))
}

#[tauri::command]
pub fn save_text(path: String, text: String) -> Res<()> {
    std::fs::write(path, text).map_err(|e| e.to_string())
}

// ---- overlay --------------------------------------------------------------

pub fn toggle_overlay_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            let _ = w.show();
        }
    }
}

#[tauri::command]
pub fn show_overlay(app: AppHandle, show: bool) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = if show { w.show() } else { w.hide() };
    }
}
