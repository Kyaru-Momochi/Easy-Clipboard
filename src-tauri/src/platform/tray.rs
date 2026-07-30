use std::sync::atomic::{AtomicBool, Ordering};

use crate::{
    commands::{exit_application, open_settings_window},
    error::AppError,
    state::AppState,
};
use tauri::{
    App, Manager,
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::TrayIconBuilder,
};

pub const TRAY_MENU_LABELS: [&str; 4] = ["显示", "暂停监听", "设置", "退出"];

#[derive(Default)]
pub struct ListeningState {
    paused: AtomicBool,
}

impl ListeningState {
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    pub fn toggle(&self) -> bool {
        let next = !self.is_paused();
        self.paused.store(next, Ordering::Release);
        next
    }

    pub fn dispatch_capture(&self, capture: impl FnOnce() -> Result<(), AppError>) {
        if !self.is_paused() {
            let _ = capture();
        }
    }
}

const SHOW_ID: &str = "show";
const PAUSE_ID: &str = "pause-listening";
const SETTINGS_ID: &str = "settings";
const EXIT_ID: &str = "exit";

pub fn install(app: &App) -> Result<(), AppError> {
    let show = MenuItem::with_id(app, SHOW_ID, TRAY_MENU_LABELS[0], true, None::<&str>)
        .map_err(|_| AppError::Platform)?;
    let pause = CheckMenuItem::with_id(
        app,
        PAUSE_ID,
        TRAY_MENU_LABELS[1],
        true,
        false,
        None::<&str>,
    )
    .map_err(|_| AppError::Platform)?;
    let settings = MenuItem::with_id(app, SETTINGS_ID, TRAY_MENU_LABELS[2], true, None::<&str>)
        .map_err(|_| AppError::Platform)?;
    let exit = MenuItem::with_id(app, EXIT_ID, TRAY_MENU_LABELS[3], true, None::<&str>)
        .map_err(|_| AppError::Platform)?;
    let menu = Menu::with_items(app, &[&show, &pause, &settings, &exit])
        .map_err(|_| AppError::Platform)?;

    let pause_item = pause.clone();
    let mut tray = TrayIconBuilder::with_id("easy-clipboard")
        .menu(&menu)
        .tooltip("Easy Clipboard")
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            let state = app.state::<AppState>();
            match event.id().as_ref() {
                SHOW_ID => {
                    let _ = state.show_overlay();
                }
                PAUSE_ID => {
                    let paused = state.listening.toggle();
                    let _ = pause_item.set_checked(paused);
                    if paused {
                        let _ = state.hide_overlay();
                    }
                }
                SETTINGS_ID => {
                    let _ = open_settings_window(app);
                }
                EXIT_ID => {
                    let _ = exit_application(app, state.inner());
                }
                _ => {}
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app).map_err(|_| AppError::Platform)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{ListeningState, TRAY_MENU_LABELS};

    #[test]
    fn tray_uses_exact_chinese_product_labels() {
        assert_eq!(TRAY_MENU_LABELS, ["显示", "暂停监听", "设置", "退出"]);
    }

    #[test]
    fn paused_listener_suppresses_capture_until_resumed() {
        let listening = ListeningState::default();
        let captures = AtomicUsize::new(0);
        let capture = || {
            captures.fetch_add(1, Ordering::Relaxed);
            Ok(())
        };

        listening.dispatch_capture(capture);
        assert!(listening.toggle());
        listening.dispatch_capture(capture);
        assert!(!listening.toggle());
        listening.dispatch_capture(capture);

        assert_eq!(captures.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn explicit_pause_blocks_new_dispatches_without_toggling_state() {
        let listening = ListeningState::default();
        let captures = AtomicUsize::new(0);

        listening.set_paused(true);
        listening.set_paused(true);
        listening.dispatch_capture(|| {
            captures.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });

        assert!(listening.is_paused());
        assert_eq!(captures.load(Ordering::Relaxed), 0);
    }
}
