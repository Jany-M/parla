// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use parla_core::logging::FileLogger;
use parla_core::models::Manifest;
use parla_core::paths::AppPaths;
use parla_core::service::{EngineLoader, Service, SourceFactory};
use parla_core::audio::device::DeviceSource;
use parla_core::sherpa;
use std::sync::Arc;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// Rust panics are caught by the panic hook; a crash inside a native model library is not.
/// This writes the exception code and address to the log so the next crash leaves a trace.
#[cfg(windows)]
unsafe extern "system" fn crash_filter(info: *const windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the OS passes a valid pointer to the exception description.
    let rec = unsafe { (*info).ExceptionRecord };
    if !rec.is_null() {
        let (code, addr) = unsafe { ((*rec).ExceptionCode, (*rec).ExceptionAddress) };
        log::error!("native crash: exception 0x{:08X} at {:p}", code as u32, addr);
    }
    log::logger().flush();
    0 // EXCEPTION_CONTINUE_SEARCH: Windows ends the process as usual
}

/// Brings the main window back from the tray or the taskbar.
fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let open = MenuItem::with_id(app, "open", "Open Parla", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Parla", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("Parla")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id.as_ref() {
            "open" => show_main(app),
            "quit" => {
                log::info!("quit from the tray menu");
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn build_service(app: &tauri::AppHandle, paths: AppPaths) -> anyhow::Result<Arc<Service>> {
    let loader: EngineLoader = Arc::new(|p| {
        let engine = sherpa::build_engine(&p.stt_dir, p.speaker_model.as_deref(), p.threads)?;
        Ok((engine, sherpa::detector_factory(p.vad_model.clone())))
    });
    let sources: SourceFactory = Arc::new(|s| {
        Ok(vec![Box::new(DeviceSource::mic(s.mic_device.clone())) as _, Box::new(DeviceSource::system(s.system_device.clone())) as _])
    });
    Service::new(paths, Manifest::bundled(), Arc::new(state::TauriSink(app.clone())), loader, sources)
}

fn main() {
    tauri::Builder::default()
        // a second launch just brings the running (maybe hidden) app forward
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let paths = AppPaths::new(AppPaths::default_root());
            paths.ensure_dirs()?;
            let _ = FileLogger::init(&paths.logs_dir());
            log::info!("Parla {} starting, data in {}", env!("CARGO_PKG_VERSION"), paths.root.display());
            std::panic::set_hook(Box::new(|info| log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture())));
            #[cfg(windows)]
            // SAFETY: registers a plain function; it only logs.
            unsafe {
                windows_sys::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(crash_filter));
            }
            // A crashed previous run may have left its language-model server running.
            parla_core::llm::kill_stale(&paths.models_dir());

            let svc = build_service(app.handle(), paths.clone())?;
            app.manage(state::AppState::new(svc));

            // The webview profile lives inside our one data folder, so uninstalling removes it too.
            let main = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Parla")
                // Shorter default height so 4K @ 300% (≈1280×720 logical) still fits.
                .inner_size(1080.0, 560.0)
                .min_inner_size(820.0, 480.0)
                .data_directory(paths.webview_dir())
                .build()?;
            let _ = main.set_focus();

            // The overlay is created hidden and reused; excluded from screen capture/sharing.
            WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("index.html#/overlay".into()))
                .title("Parla translation")
                .inner_size(720.0, 280.0)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .content_protected(true)
                .visible(false)
                .data_directory(paths.webview_dir())
                .build()?;

            if let Err(e) = build_tray(app) {
                log::error!("could not create the tray icon: {e}");
            }

            // Ctrl+Alt+T shows/hides the overlay. A conflict with another app is not fatal.
            let handle = app.handle().clone();
            if let Err(e) = app.global_shortcut().on_shortcut("ctrl+alt+t", move |_, _, ev| {
                if ev.state == ShortcutState::Pressed {
                    commands::toggle_overlay_window(&handle);
                }
            }) {
                log::warn!("could not register the overlay hotkey: {e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // The X button hides Parla to the tray (notification area); "Quit Parla" in the
                // tray menu ends it. Closing the window never interrupts a running recording.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::open_logs,
            commands::open_url,
            commands::log_ui_error,
            commands::get_settings,
            commands::save_settings,
            commands::list_devices,
            commands::list_languages,
            commands::model_status,
            commands::install_model,
            commands::cancel_install,
            commands::uninstall_model,
            commands::delete_all_data,
            commands::start_live,
            commands::stop_live,
            commands::recording_state,
            commands::set_paused,
            commands::is_paused,
            commands::start_translation,
            commands::stop_translation,
            commands::translation_target,
            commands::import_file,
            commands::cancel_import,
            commands::list_meetings,
            commands::get_meeting,
            commands::rename_meeting,
            commands::set_notes,
            commands::delete_meeting,
            commands::rename_speaker,
            commands::merge_speakers,
            commands::summarize,
            commands::export_markdown,
            commands::save_text,
            commands::show_overlay,
            commands::check_for_update,
            commands::install_update,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Parla")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Closing the app stops a running recording and frees any loaded model.
                if let Some(s) = app.try_state::<state::AppState>() {
                    let _ = s.svc.stop_live();
                }
            }
        });
}
