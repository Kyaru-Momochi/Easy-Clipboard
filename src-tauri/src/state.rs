use std::sync::{Arc, RwLock};

#[cfg(windows)]
use crate::{
    clipboard::ClipboardListener,
    platform::{ListeningState, OverlayRuntime, Size},
};
use crate::{
    clipboard::{CaptureCoordinator, ClipboardBackend, PasteTarget},
    domain::{AppSettings, ClipboardItem, HistoryQuery, ItemId},
    error::AppError,
    storage::{HistoryRepository, ImageResourceStore},
};
#[cfg(windows)]
use std::{sync::Mutex, thread::JoinHandle};
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

pub trait CommandHistory: Send + Sync {
    fn list(&self, query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError>;
    fn set_favorite(&self, id: &ItemId, value: bool) -> Result<(), AppError>;
    fn delete(&self, id: &ItemId) -> Result<(), AppError>;
    fn clear_normal(&self) -> Result<(), AppError>;
    fn load_settings(&self) -> Result<AppSettings, AppError>;
    fn save_settings(&self, settings: &AppSettings) -> Result<(), AppError>;
}

impl<T: HistoryRepository> CommandHistory for T {
    fn list(&self, query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError> {
        HistoryRepository::list(self, query)
    }

    fn set_favorite(&self, id: &ItemId, value: bool) -> Result<(), AppError> {
        HistoryRepository::set_favorite(self, id, value)
    }

    fn delete(&self, id: &ItemId) -> Result<(), AppError> {
        HistoryRepository::delete(self, id)
    }

    fn clear_normal(&self) -> Result<(), AppError> {
        HistoryRepository::clear_normal(self)
    }

    fn load_settings(&self) -> Result<AppSettings, AppError> {
        HistoryRepository::load_settings(self)
    }

    fn save_settings(&self, settings: &AppSettings) -> Result<(), AppError> {
        HistoryRepository::save_settings(self, settings)
    }
}

pub trait ClipboardActions: Send + Sync {
    fn prepare_overlay(&self) -> Result<(), AppError>;
    fn paste_item(&self, id: &ItemId) -> Result<(), AppError>;
    fn copy_item(&self, id: &ItemId) -> Result<(), AppError>;
    fn capture_now(&self) -> Result<(), AppError>;
}

impl<R, C, P> ClipboardActions for CaptureCoordinator<R, C, P>
where
    R: HistoryRepository + ImageResourceStore + 'static,
    C: ClipboardBackend + 'static,
    P: PasteTarget + 'static,
{
    fn prepare_overlay(&self) -> Result<(), AppError> {
        CaptureCoordinator::prepare_overlay(self)
    }

    fn paste_item(&self, id: &ItemId) -> Result<(), AppError> {
        CaptureCoordinator::paste_item(self, id)
    }

    fn copy_item(&self, id: &ItemId) -> Result<(), AppError> {
        CaptureCoordinator::copy_item(self, id)
    }

    fn capture_now(&self) -> Result<(), AppError> {
        CaptureCoordinator::capture_now(self).map(|_| ())
    }
}

pub trait HotkeyControl: Send + Sync {
    fn replace(&self, old: &str, new: &str) -> Result<(), AppError>;
}

pub trait AutostartControl: Send + Sync {
    fn set_enabled(&self, enabled: bool) -> Result<(), AppError>;
}

pub trait StartupAutostartControl {
    fn is_enabled(&self) -> Result<bool, AppError>;
    fn set_enabled(&self, enabled: bool) -> Result<(), AppError>;
}

pub fn reconcile_startup_autostart(
    backend: &impl StartupAutostartControl,
    desired: bool,
) -> Result<(), AppError> {
    if backend.is_enabled()? != desired {
        backend.set_enabled(desired)?;
    }
    Ok(())
}

pub struct NativeHotkey {
    app: AppHandle,
}

impl NativeHotkey {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl HotkeyControl for NativeHotkey {
    fn replace(&self, old: &str, new: &str) -> Result<(), AppError> {
        replace_registered_shortcut(&TauriShortcutBackend { app: &self.app }, old, new)
    }
}

pub trait ShortcutRegistrationBackend {
    fn register(&self, shortcut: &str) -> Result<(), AppError>;
    fn unregister(&self, shortcut: &str) -> Result<(), AppError>;
}

struct TauriShortcutBackend<'a> {
    app: &'a AppHandle,
}

impl ShortcutRegistrationBackend for TauriShortcutBackend<'_> {
    fn register(&self, shortcut: &str) -> Result<(), AppError> {
        self.app
            .global_shortcut()
            .register(shortcut)
            .map_err(|_| AppError::Platform)
    }

    fn unregister(&self, shortcut: &str) -> Result<(), AppError> {
        self.app
            .global_shortcut()
            .unregister(shortcut)
            .map_err(|_| AppError::Platform)
    }
}

fn replace_registered_shortcut(
    backend: &impl ShortcutRegistrationBackend,
    old: &str,
    new: &str,
) -> Result<(), AppError> {
    backend.register(new)?;
    if let Err(error) = backend.unregister(old) {
        return match backend.unregister(new) {
            Ok(()) => Err(error),
            Err(_) => Err(AppError::SettingsConsistency),
        };
    }
    Ok(())
}

pub struct NativeAutostart {
    app: AppHandle,
}

impl NativeAutostart {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl AutostartControl for NativeAutostart {
    fn set_enabled(&self, enabled: bool) -> Result<(), AppError> {
        let manager = self.app.autolaunch();
        if enabled {
            manager.enable()
        } else {
            manager.disable()
        }
        .map_err(|_| AppError::Platform)
    }
}

impl StartupAutostartControl for NativeAutostart {
    fn is_enabled(&self) -> Result<bool, AppError> {
        self.app.autolaunch().is_enabled().map_err(|_| AppError::Platform)
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), AppError> {
        AutostartControl::set_enabled(self, enabled)
    }
}

#[cfg(windows)]
pub struct ClipboardMonitor {
    listener: Option<ClipboardListener>,
    worker: Option<JoinHandle<()>>,
}

#[cfg(windows)]
pub trait CaptureMonitorControl: Send {
    fn stop_and_join(&mut self) -> Result<(), AppError>;
}

#[cfg(windows)]
impl ClipboardMonitor {
    pub fn new(listener: ClipboardListener, worker: JoinHandle<()>) -> Self {
        Self {
            listener: Some(listener),
            worker: Some(worker),
        }
    }
}

#[cfg(windows)]
impl CaptureMonitorControl for ClipboardMonitor {
    fn stop_and_join(&mut self) -> Result<(), AppError> {
        if let Some(listener) = self.listener.take() {
            listener.stop()?;
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| AppError::CoordinatorUnavailable)?;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for ClipboardMonitor {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

pub struct AppState {
    pub history: Arc<dyn CommandHistory>,
    pub clipboard: Arc<dyn ClipboardActions>,
    pub settings: Arc<RwLock<AppSettings>>,
    pub hotkey: Arc<dyn HotkeyControl>,
    pub autostart: Arc<dyn AutostartControl>,
    #[cfg(windows)]
    pub listening: Arc<ListeningState>,
    #[cfg(windows)]
    pub overlay: Option<Arc<OverlayRuntime>>,
    #[cfg(windows)]
    pub monitor: Mutex<Option<Box<dyn CaptureMonitorControl>>>,
}

#[cfg(windows)]
impl AppState {
    pub fn shutdown_capture(&self) -> Result<(), AppError> {
        self.listening.set_paused(true);
        let monitor = self
            .monitor
            .lock()
            .map_err(|_| AppError::CoordinatorUnavailable)?
            .take();
        if let Some(mut monitor) = monitor {
            monitor.stop_and_join()?;
        }
        Ok(())
    }

    pub fn show_overlay(&self) -> Result<(), AppError> {
        let overlay = self.overlay.as_ref().ok_or(AppError::Platform)?;
        if overlay.is_visible() {
            return Ok(());
        }
        self.clipboard.prepare_overlay()?;
        let foreground = crate::platform::foreground_window()?;
        overlay.show(foreground, Size::default())?;
        Ok(())
    }

    pub fn hide_overlay(&self) -> Result<(), AppError> {
        let overlay = self.overlay.as_ref().ok_or(AppError::Platform)?;
        if overlay.is_visible() {
            overlay.hide()
        } else {
            Ok(())
        }
    }
}

#[cfg(not(windows))]
impl AppState {
    pub fn hide_overlay(&self) -> Result<(), AppError> {
        Err(AppError::Platform)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Mutex};

    use super::{StartupAutostartControl, ShortcutRegistrationBackend, reconcile_startup_autostart, replace_registered_shortcut};
    use crate::error::AppError;

    struct FakeShortcuts {
        registered: Mutex<HashSet<String>>,
        operations: Mutex<Vec<String>>,
        fail_unregister: Mutex<HashSet<String>>,
    }

    struct FakeStartupAutostart {
        enabled: bool,
        operations: Mutex<Vec<bool>>,
    }

    impl StartupAutostartControl for FakeStartupAutostart {
        fn is_enabled(&self) -> Result<bool, AppError> { Ok(self.enabled) }
        fn set_enabled(&self, enabled: bool) -> Result<(), AppError> {
            self.operations.lock().unwrap().push(enabled);
            Ok(())
        }
    }

    #[test]
    fn startup_autostart_leaves_an_already_disabled_entry_untouched() {
        let backend = FakeStartupAutostart { enabled: false, operations: Mutex::new(Vec::new()) };
        reconcile_startup_autostart(&backend, false).unwrap();
        assert!(backend.operations.lock().unwrap().is_empty());
    }

    #[test]
    fn startup_autostart_disables_an_enabled_entry_when_requested() {
        let backend = FakeStartupAutostart { enabled: true, operations: Mutex::new(Vec::new()) };
        reconcile_startup_autostart(&backend, false).unwrap();
        assert_eq!(*backend.operations.lock().unwrap(), [false]);
    }

    impl ShortcutRegistrationBackend for FakeShortcuts {
        fn register(&self, shortcut: &str) -> Result<(), AppError> {
            self.operations
                .lock()
                .unwrap()
                .push(format!("register:{shortcut}"));
            self.registered.lock().unwrap().insert(shortcut.into());
            Ok(())
        }

        fn unregister(&self, shortcut: &str) -> Result<(), AppError> {
            self.operations
                .lock()
                .unwrap()
                .push(format!("unregister:{shortcut}"));
            if self.fail_unregister.lock().unwrap().contains(shortcut) {
                return Err(AppError::Platform);
            }
            self.registered.lock().unwrap().remove(shortcut);
            Ok(())
        }
    }

    #[test]
    fn failed_old_unregister_keeps_old_and_cleans_temporary_new_shortcut() {
        let backend = FakeShortcuts {
            registered: Mutex::new(HashSet::from(["Ctrl+Shift+V".into()])),
            operations: Mutex::new(Vec::new()),
            fail_unregister: Mutex::new(HashSet::from(["Ctrl+Shift+V".into()])),
        };

        let error =
            replace_registered_shortcut(&backend, "Ctrl+Shift+V", "Ctrl+Alt+X").unwrap_err();

        assert_eq!(error, AppError::Platform);
        assert_eq!(
            *backend.registered.lock().unwrap(),
            HashSet::from(["Ctrl+Shift+V".into()])
        );
        assert_eq!(
            *backend.operations.lock().unwrap(),
            [
                "register:Ctrl+Alt+X",
                "unregister:Ctrl+Shift+V",
                "unregister:Ctrl+Alt+X"
            ]
        );
    }
}
