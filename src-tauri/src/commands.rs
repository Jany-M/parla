//! Thin command layer: every command forwards to `parla_core::service::Service`.
use crate::state::AppState;
#[cfg(windows)]
use anyhow::Context;
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

/// Mic / screen / system-audio permission snapshot (macOS; always OK elsewhere).
#[tauri::command]
pub fn capture_permissions() -> crate::macos_permissions::CapturePermissions {
    crate::macos_permissions::status()
}

/// Ask for any missing capture permissions (shows system prompts when possible).
#[tauri::command]
pub fn request_capture_permissions() -> crate::macos_permissions::CapturePermissions {
    crate::macos_permissions::request_missing()
}

/// Opens System Settings to the Privacy panes for Microphone and Screen & System Audio.
#[tauri::command]
pub fn open_capture_permission_settings() -> Res<()> {
    crate::macos_permissions::open_settings()
}

/// Opens an http(s) URL in the default browser (new window/tab).
#[tauri::command]
pub fn open_url(url: String) -> Res<()> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) links are allowed".into());
    }
    #[cfg(target_os = "windows")]
    {
        // `start` needs an empty title argument when the URL is quoted.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map(|_| ())
            .map_err(|e| {
                log::error!("could not open url: {e}");
                e.to_string()
            })
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
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
pub async fn start_translation(app: AppHandle, s: State<'_, AppState>, target: String) -> Res<()> {
    let svc = s.svc.clone();
    blocking(move || svc.start_translation(&target)).await?;
    // Always raise the overlay when translation starts (Show overlay is only for re-opening after hide).
    show_overlay(app, true);
    Ok(())
}

#[tauri::command]
pub async fn stop_translation(app: AppHandle, s: State<'_, AppState>) -> Res<()> {
    let svc = s.svc.clone();
    blocking(move || {
        svc.stop_translation();
        Ok(())
    })
    .await?;
    show_overlay(app, false);
    Ok(())
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
            show_overlay_window(&w);
        }
    }
}

fn show_overlay_window(w: &tauri::WebviewWindow) {
    // Windows can leave a never-shown always-on-top window inert after a bare `show()`.
    let _ = w.unminimize();
    if let Err(e) = w.show() {
        log::warn!("could not show overlay: {e}");
    }
    let _ = w.set_always_on_top(true);
}

#[tauri::command]
pub fn show_overlay(app: AppHandle, show: bool) {
    if let Some(w) = app.get_webview_window("overlay") {
        if show {
            show_overlay_window(&w);
        } else {
            let _ = w.hide();
        }
    } else {
        log::warn!("overlay window missing");
    }
}

// ---- updates (GitHub Releases) --------------------------------------------

#[derive(Serialize, Clone)]
pub struct UpdateInfoDto {
    pub version: String,
    pub tag: String,
    pub asset_name: String,
    pub size: u64,
    /// True only on Windows when a `-setup.exe` asset exists. macOS opens the Releases page instead.
    pub auto_install: bool,
}

#[cfg(windows)]
#[derive(Serialize, Clone)]
struct UpdateProgress {
    done: u64,
    total: u64,
}

fn update_dto(u: parla_core::update::UpdateInfo) -> UpdateInfoDto {
    let auto_install = cfg!(windows) && u.asset_name.to_ascii_lowercase().ends_with("-setup.exe");
    UpdateInfoDto {
        version: u.version,
        tag: u.tag,
        asset_name: u.asset_name,
        size: u.size,
        auto_install,
    }
}

/// Compares the running version to https://github.com/Jany-M/parla/releases/latest.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Res<Option<UpdateInfoDto>> {
    let current = app.package_info().version.to_string();
    blocking(move || match parla_core::update::check_latest(&current) {
        Ok(v) => Ok(v.map(update_dto)),
        Err(e) => {
            log::warn!("update check failed: {e:#}");
            Err(e)
        }
    })
    .await
}

/// Windows: download the NSIS installer and apply it after exit.
/// macOS: open the GitHub Releases page (unsigned DMG; no silent in-app install).
#[tauri::command]
pub async fn install_update(app: AppHandle, s: State<'_, AppState>) -> Res<()> {
    #[cfg(not(windows))]
    {
        let _ = s;
        log::info!("update: opening Releases page (no silent installer on this OS)");
        open_url(parla_core::update::RELEASES_LATEST_URL.to_string())?;
        let _ = app;
        return Ok(());
    }
    #[cfg(windows)]
    {
        let current = app.package_info().version.to_string();
        let logs_dir = s.svc.paths.logs_dir();
        let svc = s.svc.clone();
        s.install_cancel.lock().unwrap().cancel();
        s.import_cancel.lock().unwrap().store(true, Ordering::SeqCst);

        let app_for_progress = app.clone();
        let installer = blocking(move || {
            let info = parla_core::update::check_latest(&current)?
                .ok_or_else(|| anyhow::anyhow!("no newer release is available"))?;
            if !info.asset_name.to_ascii_lowercase().ends_with("-setup.exe") {
                anyhow::bail!(
                    "latest release has no Windows installer yet; download from {}",
                    parla_core::update::RELEASES_LATEST_URL
                );
            }
            let dir = std::env::temp_dir().join("parla-update");
            parla_core::update::download_installer(&info, &dir, &mut |done, total| {
                let _ = app_for_progress.emit("update-progress", UpdateProgress { done, total });
            })
        })
        .await
        .map_err(|e| {
            log::error!("update install failed before helper: {e}");
            e
        })?;

        // Stop recording / translation and any leftover llama-server before files are replaced.
        let _ = svc.stop_live();
        parla_core::llm::kill_stale(&svc.paths.models_dir());

        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        log::info!(
            "update: spawning helper for {} -> {}; helper log {}",
            installer.display(),
            exe.display(),
            logs_dir.join(parla_core::update::UPDATE_HELPER_LOG).display()
        );
        spawn_windows_updater(installer, exe, logs_dir).map_err(err)?;
        // Give `cmd /c start` a moment to hand off before this process (and its job) tear down.
        std::thread::sleep(std::time::Duration::from_millis(750));
        log::info!("update helper started; exiting so files can be replaced");
        log::logger().flush();
        app.exit(0);
        Ok(())
    }
}

/// Escape a path for a PowerShell single-quoted string (`'` → `''`).
#[cfg(windows)]
fn ps_quote(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

/// Wait for this process to exit, run the NSIS installer silently, then relaunch Parla.
///
/// Tauri NSIS flags: `/S` silent, `/UPDATE` keeps app data (hooks.nsh), `/R` relaunches the app.
///
/// The helper must outlive `app.exit`. Spawning PowerShell directly with
/// `CREATE_BREAKAWAY_FROM_JOB` still dies under WebView2's job on some machines
/// (symptom: leftover `parla-apply-update-*.ps1`, no `update.log`, version unchanged).
/// `cmd /c start` starts a process outside that job.
#[cfg(windows)]
fn spawn_windows_updater(installer: PathBuf, app_exe: PathBuf, logs_dir: PathBuf) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let pid = std::process::id();
    let script = std::env::temp_dir().join(format!("parla-apply-update-{pid}.ps1"));
    std::fs::create_dir_all(&logs_dir).context("creating logs dir for update helper")?;
    let log = logs_dir.join(parla_core::update::UPDATE_HELPER_LOG);
    let app_dir = app_exe.parent().unwrap_or(app_exe.as_path()).to_path_buf();
    // `/S /UPDATE /R`: silent upgrade without wiping %LOCALAPPDATA%\Parla, then restart.
    let body = format!(
        r#"$ErrorActionPreference = 'Continue'
$log = '{log}'
function Write-Log([string]$m) {{
  $line = '{{0:u}} {{1}}' -f (Get-Date).ToUniversalTime(), $m
  Add-Content -LiteralPath $log -Value $line -Encoding utf8
}}
Write-Log 'helper alive; waiting for pid {pid} to exit'
while (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 400 }}
Start-Sleep -Seconds 2
# Release locks from leftover Parla / WebView2 children still under the install dir.
$appDir = '{app_dir}'
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
  Where-Object {{ $_.ExecutablePath -and $_.ExecutablePath.StartsWith($appDir, [System.StringComparison]::OrdinalIgnoreCase) }} |
  ForEach-Object {{
    Write-Log ("stopping leftover pid={{0}} path={{1}}" -f $_.ProcessId, $_.ExecutablePath)
    Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
  }}
Start-Sleep -Seconds 1
$setup = '{setup}'
if (-not (Test-Path -LiteralPath $setup)) {{
  Write-Log ("installer missing: {{0}}" -f $setup)
  exit 2
}}
Write-Log ("running installer: {{0}} /S /UPDATE /R" -f $setup)
$p = Start-Process -FilePath $setup -ArgumentList '/S','/UPDATE','/R' -PassThru
# Cap wait: some hosts leave the setup process around after /R relaunches the app.
if (-not $p.WaitForExit(600000)) {{
  Write-Log 'installer still running after 10 min; stopping waiter'
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  $code = 'timeout'
}} else {{
  $code = if ($null -eq $p.ExitCode) {{ 'null' }} else {{ [string]$p.ExitCode }}
}}
Write-Log ("installer exit code: {{0}}" -f $code)
# `/R` should relaunch; if the new binary exists and nothing started it, start manually.
Start-Sleep -Seconds 2
$app = '{app}'
$running = Get-Process -Name 'parla' -ErrorAction SilentlyContinue
if (-not $running -and (Test-Path -LiteralPath $app)) {{
  Write-Log 'relaunching app (installer /R did not)'
  Start-Process -FilePath $app
}} elseif ($running) {{
  Write-Log 'parla already running after installer'
}} else {{
  Write-Log ("app binary missing after install: {{0}}" -f $app)
}}
Remove-Item -LiteralPath $MyInvocation.MyCommand.Path -Force -ErrorAction SilentlyContinue
Write-Log 'done'
"#,
        log = ps_quote(&log),
        setup = ps_quote(&installer),
        app = ps_quote(&app_exe),
        app_dir = ps_quote(&app_dir),
    );
    std::fs::write(&script, body).context("writing update helper script")?;
    // Stamp before spawn so the next launch can tell spawn was attempted even if the helper dies.
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        let _ = writeln!(
            f,
            "parla: requesting helper via cmd start for {} (pid {pid})",
            installer.display()
        );
    }
    log::info!("update helper script {}; log {}", script.display(), log.display());

    // `start ""` needs an empty window title before the command when paths may be quoted.
    let script_arg = script.to_string_lossy().into_owned();
    let mut cmd = std::process::Command::new("cmd");
    cmd.args([
        "/C",
        "start",
        "",
        "/MIN",
        "powershell.exe",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-File",
        &script_arg,
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .creation_flags(CREATE_NO_WINDOW);
    let child = cmd.spawn().context("starting update helper via cmd start")?;
    log::info!("update helper launcher pid={}", child.id());
    Ok(())
}
