pub mod clipboard;
pub mod domain;
pub mod error;
pub mod platform;
pub mod storage;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(windows)]
            {
                use tauri::Manager;

                let window = app.get_webview_window("clipboard").ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "clipboard webview window was not created",
                    )
                })?;
                let hwnd = window.hwnd()?;
                let overlay = platform::OverlayController::attach(hwnd.0 as isize)?;
                app.manage(platform::OverlayRuntime::new(overlay, window));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Easy Clipboard");
}
