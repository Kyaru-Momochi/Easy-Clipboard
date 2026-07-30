pub mod clipboard;
pub mod commands;
pub mod domain;
pub mod error;
pub mod platform;
pub mod state;
pub mod storage;

#[cfg(windows)]
fn handle_global_shortcut<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    shortcut: &tauri_plugin_global_shortcut::Shortcut,
    event: tauri_plugin_global_shortcut::ShortcutEvent,
) {
    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
        use tauri::Manager;
        let state = app.state::<state::AppState>();
        let is_configured = state
            .settings
            .read()
            .ok()
            .and_then(|settings| settings.hotkey.parse().ok())
            .is_some_and(|configured| shortcut == &configured);
        if is_configured {
            let _ = state.show_overlay();
        }
    }
}

#[cfg(windows)]
fn handle_window_event<R: tauri::Runtime>(window: &tauri::Window<R>, event: &tauri::WindowEvent) {
    if window.label() == "clipboard"
        && let tauri::WindowEvent::CloseRequested { api, .. } = event
    {
        use tauri::Manager;
        api.prevent_close();
        let _ = window
            .app_handle()
            .state::<state::AppState>()
            .hide_overlay();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    #[cfg(windows)]
                    handle_global_shortcut(app, shortcut, event);
                    #[cfg(not(windows))]
                    {
                        let _ = (app, shortcut, event);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(windows)]
            {
                use std::{
                    sync::{Arc, Mutex, RwLock},
                    thread,
                };

                use clipboard::{
                    CaptureCoordinator, ClipboardListener, SystemClock, WindowsClipboard,
                };
                use state::{AppState, ClipboardMonitor, NativeAutostart, NativeHotkey};
                use storage::{HistoryRepository, SqliteHistoryRepository};
                use tauri::Manager;
                use tauri_plugin_autostart::ManagerExt;
                use tauri_plugin_global_shortcut::GlobalShortcutExt;

                let window = app.get_webview_window("clipboard").ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "clipboard webview window was not created",
                    )
                })?;
                let hwnd = window.hwnd()?;
                let overlay = platform::OverlayController::attach(hwnd.0 as isize)?;
                let overlay = Arc::new(platform::OverlayRuntime::new(overlay, window));

                let app_data = app.path().app_data_dir()?;
                let repository = Arc::new(SqliteHistoryRepository::open(app_data)?);
                let settings = repository.load_settings()?;
                let shared_settings = Arc::new(RwLock::new(settings.clone()));
                let clipboard = Arc::new(WindowsClipboard::new()?);
                let paste_target = Arc::new(platform::WindowsPasteTarget::default());
                let coordinator = Arc::new(CaptureCoordinator::new(
                    Arc::clone(&repository),
                    clipboard,
                    paste_target,
                    Arc::clone(&shared_settings),
                    Arc::new(SystemClock),
                ));
                let listening = Arc::new(platform::ListeningState::default());
                let (listener, updates) = ClipboardListener::start()?;
                let capture = Arc::clone(&coordinator);
                let capture_gate = Arc::clone(&listening);
                let worker = thread::Builder::new()
                    .name("easy-clipboard-capture".into())
                    .spawn(move || {
                        while updates.recv().is_ok() {
                            capture_gate.dispatch_capture(|| capture.capture_now().map(|_| ()));
                        }
                    })
                    .map_err(|_| error::AppError::Platform)?;

                let handle = app.handle().clone();
                app.manage(AppState {
                    history: repository,
                    clipboard: coordinator,
                    settings: shared_settings,
                    hotkey: Arc::new(NativeHotkey::new(handle.clone())),
                    autostart: Arc::new(NativeAutostart::new(handle)),
                    listening,
                    overlay: Some(overlay),
                    monitor: Mutex::new(Some(Box::new(ClipboardMonitor::new(listener, worker)))),
                });

                app.global_shortcut()
                    .register(settings.hotkey.as_str())
                    .map_err(|_| error::AppError::Platform)?;
                if settings.autostart {
                    app.autolaunch()
                        .enable()
                        .map_err(|_| error::AppError::Platform)?;
                } else {
                    app.autolaunch()
                        .disable()
                        .map_err(|_| error::AppError::Platform)?;
                }
                platform::tray::install(app)?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(windows)]
            handle_window_event(window, event);
            #[cfg(not(windows))]
            {
                let _ = (window, event);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_history,
            commands::paste_item,
            commands::copy_item,
            commands::set_favorite,
            commands::delete_item,
            commands::clear_history,
            commands::get_settings,
            commands::save_settings,
            commands::open_settings,
            commands::reveal_file,
            commands::exit_app
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Easy Clipboard");
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_app_state_access_is_confined_to_windows_only_handlers() {
        let source = include_str!("lib.rs");
        let run_body = source
            .split_once("pub fn run()")
            .unwrap()
            .1
            .split_once("#[cfg(test)]")
            .unwrap()
            .0;
        let state_access = [".state::<state::", "AppState>()"].concat();

        assert!(
            !run_body.contains(&state_access),
            "run must not resolve Windows-only AppState methods on other targets"
        );
        assert!(
            source.contains("#[cfg(windows)]\nfn handle_global_shortcut"),
            "global shortcut state access must live in a Windows-only helper"
        );
        assert!(
            source.contains("#[cfg(windows)]\nfn handle_window_event"),
            "window event state access must live in a Windows-only helper"
        );
        assert_eq!(source.matches(&state_access).count(), 2);
    }

    #[test]
    fn webview_capability_contains_only_directly_used_core_permissions() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();

        assert_eq!(
            capability["permissions"],
            serde_json::json!([
                "core:event:allow-listen",
                "core:event:allow-unlisten",
                "core:window:allow-hide"
            ])
        );
    }
}
