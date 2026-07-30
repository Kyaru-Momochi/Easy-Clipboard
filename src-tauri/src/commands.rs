use crate::{
    domain::{AppSettings, ClipboardItem, HistoryPolicy, HistoryQuery, ItemId},
    error::AppError,
    state::AppState,
};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

pub struct CommandService<'a> {
    state: &'a AppState,
}

impl<'a> CommandService<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub fn list_history(&self, query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError> {
        self.state.history.list(query)
    }

    pub fn paste_item(&self, id: &ItemId) -> Result<(), AppError> {
        self.state.clipboard.paste_item(id)
    }

    pub fn copy_item(&self, id: &ItemId) -> Result<(), AppError> {
        self.state.clipboard.copy_item(id)
    }

    pub fn set_favorite(&self, id: &ItemId, value: bool) -> Result<(), AppError> {
        if value {
            let current = self.state.history.list(&HistoryQuery::default())?;
            if current
                .iter()
                .find(|item| &item.id == id)
                .is_some_and(|item| !item.is_favorite)
            {
                let settings = self.state.history.load_settings()?;
                HistoryPolicy::new(settings.history_limit, settings.favorite_limit)
                    .can_favorite(&current)?;
            }
        }
        self.state.history.set_favorite(id, value)
    }

    pub fn delete_item(&self, id: &ItemId) -> Result<(), AppError> {
        self.state.history.delete(id)
    }

    pub fn clear_history(&self) -> Result<(), AppError> {
        self.state.history.clear_normal()
    }

    pub fn get_settings(&self) -> Result<AppSettings, AppError> {
        self.state.history.load_settings()
    }

    pub fn save_settings(&self, settings: AppSettings) -> Result<(), AppError> {
        validate_settings(&settings)?;
        let mut current = self
            .state
            .settings
            .write()
            .map_err(|_| AppError::CoordinatorUnavailable)?;
        let previous = current.clone();
        let hotkey_changed = previous.hotkey != settings.hotkey;
        let autostart_changed = previous.autostart != settings.autostart;

        if hotkey_changed {
            self.state
                .hotkey
                .replace(&previous.hotkey, &settings.hotkey)?;
        }
        if autostart_changed
            && let Err(error) = self.state.autostart.set_enabled(settings.autostart)
        {
            let mut rollback_failed = self
                .state
                .autostart
                .set_enabled(previous.autostart)
                .is_err();
            if hotkey_changed {
                rollback_failed |= self
                    .state
                    .hotkey
                    .replace(&settings.hotkey, &previous.hotkey)
                    .is_err();
            }
            return Err(if rollback_failed {
                AppError::SettingsConsistency
            } else {
                error
            });
        }
        if let Err(error) = self.state.history.save_settings(&settings) {
            let mut rollback_failed = self.state.history.save_settings(&previous).is_err();
            if autostart_changed {
                rollback_failed |= self
                    .state
                    .autostart
                    .set_enabled(previous.autostart)
                    .is_err();
            }
            if hotkey_changed {
                rollback_failed |= self
                    .state
                    .hotkey
                    .replace(&settings.hotkey, &previous.hotkey)
                    .is_err();
            }
            return Err(if rollback_failed {
                AppError::SettingsConsistency
            } else {
                error
            });
        }

        *current = settings;
        Ok(())
    }
}

const BYTES_PER_MEGABYTE: u64 = 1024 * 1024;
const MAX_ITEM_BYTES: u64 = 500 * BYTES_PER_MEGABYTE;

fn validate_settings(settings: &AppSettings) -> Result<(), AppError> {
    if !(BYTES_PER_MEGABYTE..=MAX_ITEM_BYTES).contains(&settings.max_item_bytes) {
        return Err(AppError::InvalidItemLimitMb {
            value: settings.max_item_bytes / BYTES_PER_MEGABYTE,
            min: 1,
            max: 500,
        });
    }
    if !settings.motion_scale.is_finite() {
        return Err(AppError::InvalidMotionScale);
    }
    Ok(())
}

#[tauri::command]
pub fn list_history(
    state: State<'_, AppState>,
    query: HistoryQuery,
) -> Result<Vec<ClipboardItem>, AppError> {
    CommandService::new(state.inner()).list_history(&query)
}

#[tauri::command]
pub fn paste_item(state: State<'_, AppState>, id: ItemId) -> Result<(), AppError> {
    CommandService::new(state.inner()).paste_item(&id)
}

#[tauri::command]
pub fn copy_item(state: State<'_, AppState>, id: ItemId) -> Result<(), AppError> {
    CommandService::new(state.inner()).copy_item(&id)
}

#[tauri::command]
pub fn set_favorite(state: State<'_, AppState>, id: ItemId, value: bool) -> Result<(), AppError> {
    CommandService::new(state.inner()).set_favorite(&id, value)
}

#[tauri::command]
pub fn delete_item(state: State<'_, AppState>, id: ItemId) -> Result<(), AppError> {
    CommandService::new(state.inner()).delete_item(&id)
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), AppError> {
    CommandService::new(state.inner()).clear_history()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, AppError> {
    CommandService::new(state.inner()).get_settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: AppSettings) -> Result<(), AppError> {
    CommandService::new(state.inner()).save_settings(settings)
}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<(), AppError> {
    open_settings_window(&app)
}

pub fn open_settings_window(app: &AppHandle) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window("settings") {
        window.show().map_err(|_| AppError::Platform)?;
        window.set_focus().map_err(|_| AppError::Platform)?;
        return Ok(());
    }

    WebviewWindowBuilder::new(
        app,
        "settings",
        WebviewUrl::App("index.html?view=settings".into()),
    )
    .title("Easy Clipboard 设置")
    .inner_size(560.0, 680.0)
    .resizable(true)
    .decorations(true)
    .focused(true)
    .build()
    .map_err(|_| AppError::Platform)?;
    Ok(())
}

#[tauri::command]
pub fn reveal_file(app: AppHandle, path: String) -> Result<(), AppError> {
    match reveal_action(&path)? {
        RevealAction::RevealFile(path) => app
            .opener()
            .reveal_item_in_dir(path)
            .map_err(|_| AppError::Platform),
        RevealAction::OpenDirectory(path) => app
            .opener()
            .open_path(path.to_string_lossy().into_owned(), None::<String>)
            .map_err(|_| AppError::Platform),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RevealAction {
    RevealFile(PathBuf),
    OpenDirectory(PathBuf),
}

fn reveal_action(raw_path: &str) -> Result<RevealAction, AppError> {
    if raw_path.is_empty() || raw_path.contains('\0') {
        return Err(AppError::InvalidPath);
    }

    let path = Path::new(raw_path);
    if !path.is_absolute() {
        return Err(AppError::InvalidPath);
    }

    match path.metadata() {
        Ok(metadata) if metadata.is_file() => Ok(RevealAction::RevealFile(path.to_path_buf())),
        Ok(metadata) if metadata.is_dir() => Ok(existing_directory_target(path)),
        Ok(_) => Err(AppError::InvalidPath),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => existing_parent(path)
            .map(RevealAction::OpenDirectory)
            .ok_or(AppError::InvalidPath),
        Err(_) => Err(AppError::InvalidPath),
    }
}

fn existing_directory_target(path: &Path) -> RevealAction {
    RevealAction::OpenDirectory(path.parent().unwrap_or(path).to_path_buf())
}

fn existing_parent(path: &Path) -> Option<PathBuf> {
    path.parent()?
        .ancestors()
        .find(|candidate| candidate.is_dir())
        .map(Path::to_path_buf)
}

#[tauri::command]
pub fn exit_app(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    exit_application(&app, state.inner())
}

pub fn exit_application(app: &AppHandle, state: &AppState) -> Result<(), AppError> {
    prepare_for_exit(state)?;
    app.exit(0);
    Ok(())
}

fn prepare_for_exit(state: &AppState) -> Result<(), AppError> {
    #[cfg(windows)]
    state.shutdown_capture()?;
    if CommandService::new(state).get_settings()?.clear_on_exit {
        CommandService::new(state).clear_history()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    #[cfg(windows)]
    use std::path::Path;
    #[cfg(windows)]
    use std::sync::mpsc;
    use std::sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    };
    #[cfg(windows)]
    use std::thread;

    use super::{CommandService, RevealAction, reveal_action};
    #[cfg(windows)]
    use super::{existing_directory_target, prepare_for_exit};
    #[cfg(windows)]
    use crate::state::CaptureMonitorControl;
    use crate::{
        clipboard::{
            CaptureCoordinator, ClipboardBackend, PasteTarget, RawClipboardSnapshot, SystemClock,
        },
        domain::{
            AppSettings, ClipboardItem, ClipboardKind, ClipboardPayload, HistoryQuery, ItemId,
            UpsertDecision,
        },
        error::AppError,
        state::{AppState, AutostartControl, ClipboardActions, CommandHistory, HotkeyControl},
        storage::{HistoryRepository, SqliteHistoryRepository},
    };
    use tempfile::TempDir;

    #[test]
    fn reveal_policy_rejects_empty_relative_and_unparseable_paths() {
        for path in ["", "   ", "relative.txt", "bad\0path"] {
            assert_eq!(reveal_action(path), Err(AppError::InvalidPath));
        }
    }

    #[test]
    fn reveal_policy_locates_an_existing_file() {
        let directory = TempDir::new().unwrap();
        let file = directory.path().join("item.txt");
        fs::write(&file, "clipboard").unwrap();

        assert_eq!(
            reveal_action(file.to_str().unwrap()),
            Ok(RevealAction::RevealFile(file))
        );
    }

    #[test]
    fn reveal_policy_opens_the_parent_of_an_existing_directory() {
        let parent = TempDir::new().unwrap();
        let directory = parent.path().join("folder");
        fs::create_dir(&directory).unwrap();

        assert_eq!(
            reveal_action(directory.to_str().unwrap()),
            Ok(RevealAction::OpenDirectory(parent.path().to_path_buf()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn existing_drive_and_unc_roots_open_themselves() {
        for root in [Path::new(r"C:\"), Path::new(r"\\server\share")] {
            assert_eq!(
                existing_directory_target(root),
                RevealAction::OpenDirectory(root.to_path_buf())
            );
        }
    }

    #[test]
    fn reveal_policy_opens_the_nearest_existing_parent_for_a_missing_file() {
        let parent = TempDir::new().unwrap();
        let missing = parent.path().join("missing").join("item.txt");

        assert_eq!(
            reveal_action(missing.to_str().unwrap()),
            Ok(RevealAction::OpenDirectory(parent.path().to_path_buf()))
        );
    }

    struct FakeHistory {
        items: Mutex<Vec<ClipboardItem>>,
        settings: Mutex<AppSettings>,
        save_results: Mutex<VecDeque<Result<(), AppError>>>,
        save_attempts: Mutex<Vec<AppSettings>>,
    }

    impl CommandHistory for FakeHistory {
        fn list(&self, query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError> {
            Ok(self
                .items
                .lock()
                .unwrap()
                .iter()
                .filter(|item| query.kind.as_ref().is_none_or(|kind| kind == &item.kind))
                .filter(|item| item.preview.contains(&query.search))
                .cloned()
                .collect())
        }

        fn set_favorite(&self, id: &ItemId, value: bool) -> Result<(), AppError> {
            let mut items = self.items.lock().unwrap();
            let item = items
                .iter_mut()
                .find(|item| &item.id == id)
                .ok_or(AppError::ItemNotFound)?;
            item.is_favorite = value;
            Ok(())
        }

        fn delete(&self, id: &ItemId) -> Result<(), AppError> {
            let mut items = self.items.lock().unwrap();
            let index = items
                .iter()
                .position(|item| &item.id == id)
                .ok_or(AppError::ItemNotFound)?;
            items.remove(index);
            Ok(())
        }

        fn clear_normal(&self) -> Result<(), AppError> {
            self.items.lock().unwrap().retain(|item| item.is_favorite);
            Ok(())
        }

        fn load_settings(&self) -> Result<AppSettings, AppError> {
            Ok(self.settings.lock().unwrap().clone())
        }

        fn save_settings(&self, settings: &AppSettings) -> Result<(), AppError> {
            self.save_attempts.lock().unwrap().push(settings.clone());
            if let Some(result) = self.save_results.lock().unwrap().pop_front() {
                result?;
            }
            *self.settings.lock().unwrap() = settings.clone();
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeClipboard {
        calls: Mutex<Vec<String>>,
    }

    impl ClipboardActions for FakeClipboard {
        fn prepare_overlay(&self) -> Result<(), AppError> {
            Ok(())
        }

        fn paste_item(&self, id: &ItemId) -> Result<(), AppError> {
            self.calls.lock().unwrap().push(format!("paste:{}", id.0));
            Ok(())
        }

        fn copy_item(&self, id: &ItemId) -> Result<(), AppError> {
            self.calls.lock().unwrap().push(format!("copy:{}", id.0));
            Ok(())
        }

        fn capture_now(&self) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct FakeHotkey {
        active: Mutex<String>,
        rejected: Mutex<Option<String>>,
        replacements: Mutex<Vec<(String, String)>>,
    }

    impl HotkeyControl for FakeHotkey {
        fn replace(&self, old: &str, new: &str) -> Result<(), AppError> {
            self.replacements
                .lock()
                .unwrap()
                .push((old.into(), new.into()));
            if self.rejected.lock().unwrap().as_deref() == Some(new) {
                return Err(AppError::Platform);
            }
            let mut active = self.active.lock().unwrap();
            if active.as_str() != old {
                return Err(AppError::Platform);
            }
            *active = new.into();
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeAutostart {
        enabled: Mutex<bool>,
        results: Mutex<VecDeque<Result<(), AppError>>>,
        operations: Mutex<Vec<bool>>,
    }

    impl AutostartControl for FakeAutostart {
        fn set_enabled(&self, enabled: bool) -> Result<(), AppError> {
            self.operations.lock().unwrap().push(enabled);
            if let Some(result) = self.results.lock().unwrap().pop_front() {
                result?;
            }
            *self.enabled.lock().unwrap() = enabled;
            Ok(())
        }
    }

    struct Fixture {
        state: AppState,
        history: Arc<FakeHistory>,
        clipboard: Arc<FakeClipboard>,
        hotkey: Arc<FakeHotkey>,
        autostart: Arc<FakeAutostart>,
    }

    fn item(id: &str, kind: ClipboardKind, preview: &str, favorite: bool) -> ClipboardItem {
        ClipboardItem {
            id: ItemId(id.into()),
            kind,
            payload: ClipboardPayload::Text {
                plain: preview.into(),
                html: None,
                rtf: None,
            },
            fingerprint: format!("fingerprint-{id}"),
            preview: preview.into(),
            byte_size: preview.len() as u64,
            is_favorite: favorite,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn fixture() -> Fixture {
        let history = Arc::new(FakeHistory {
            items: Mutex::new(vec![
                item("text", ClipboardKind::Text, "needle text", false),
                item("image", ClipboardKind::Image, "needle image", true),
            ]),
            settings: Mutex::new(AppSettings::default()),
            save_results: Mutex::new(VecDeque::new()),
            save_attempts: Mutex::new(Vec::new()),
        });
        let clipboard = Arc::new(FakeClipboard::default());
        let hotkey = Arc::new(FakeHotkey {
            active: Mutex::new(AppSettings::default().hotkey),
            rejected: Mutex::new(None),
            replacements: Mutex::new(Vec::new()),
        });
        let settings = Arc::new(RwLock::new(AppSettings::default()));
        let autostart = Arc::new(FakeAutostart::default());
        let state = AppState {
            history: history.clone(),
            clipboard: clipboard.clone(),
            settings,
            hotkey: hotkey.clone(),
            autostart: autostart.clone(),
            #[cfg(windows)]
            listening: Arc::new(crate::platform::ListeningState::default()),
            #[cfg(windows)]
            overlay: None,
            #[cfg(windows)]
            monitor: Mutex::new(None),
        };
        Fixture {
            state,
            history,
            clipboard,
            hotkey,
            autostart,
        }
    }

    #[test]
    fn commands_list_history_forwards_kind_and_search_filters() {
        let fixture = fixture();
        let listed = CommandService::new(&fixture.state)
            .list_history(&HistoryQuery {
                kind: Some(ClipboardKind::Image),
                search: "needle".into(),
            })
            .unwrap();

        assert_eq!(
            listed
                .iter()
                .map(|item| item.id.0.as_str())
                .collect::<Vec<_>>(),
            ["image"]
        );
    }

    #[test]
    fn commands_paste_and_copy_only_remain_distinct() {
        let fixture = fixture();
        let service = CommandService::new(&fixture.state);

        service.paste_item(&ItemId("text".into())).unwrap();
        service.copy_item(&ItemId("image".into())).unwrap();

        assert_eq!(
            *fixture.clipboard.calls.lock().unwrap(),
            ["paste:text", "copy:image"]
        );
    }

    #[test]
    fn commands_favorite_delete_and_clear_use_repository_semantics() {
        let fixture = fixture();
        let service = CommandService::new(&fixture.state);

        service.set_favorite(&ItemId("text".into()), true).unwrap();
        service.delete_item(&ItemId("image".into())).unwrap();
        service.clear_history().unwrap();

        let items = fixture.history.items.lock().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, ItemId("text".into()));
        assert!(items[0].is_favorite);
    }

    #[test]
    fn commands_enforce_favorite_limit_before_mutating_history() {
        let fixture = fixture();
        fixture.history.settings.lock().unwrap().favorite_limit = 1;

        let error = CommandService::new(&fixture.state)
            .set_favorite(&ItemId("text".into()), true)
            .unwrap_err();

        assert_eq!(error, AppError::FavoriteLimitReached { limit: 1 });
        assert!(
            !fixture
                .history
                .items
                .lock()
                .unwrap()
                .iter()
                .find(|item| item.id == ItemId("text".into()))
                .unwrap()
                .is_favorite
        );
    }

    #[test]
    fn commands_reject_invalid_settings_without_persisting_them() {
        let fixture = fixture();
        let invalid = AppSettings {
            max_item_bytes: 0,
            ..AppSettings::default()
        };

        let error = CommandService::new(&fixture.state)
            .save_settings(invalid)
            .unwrap_err();

        assert_eq!(error.code(), "invalidItemLimit");
        assert_eq!(
            fixture.history.load_settings().unwrap(),
            AppSettings::default()
        );
    }

    #[test]
    fn commands_keep_old_hotkey_registered_and_persisted_when_new_one_fails() {
        let fixture = fixture();
        *fixture.hotkey.rejected.lock().unwrap() = Some("Ctrl+Alt+X".into());
        let requested = AppSettings {
            hotkey: "Ctrl+Alt+X".into(),
            ..AppSettings::default()
        };

        let error = CommandService::new(&fixture.state)
            .save_settings(requested)
            .unwrap_err();

        assert_eq!(error, AppError::Platform);
        assert_eq!(
            fixture.hotkey.active.lock().unwrap().as_str(),
            "Ctrl+Shift+V"
        );
        assert_eq!(
            fixture.history.load_settings().unwrap().hotkey,
            "Ctrl+Shift+V"
        );
        assert_eq!(
            fixture.state.settings.read().unwrap().hotkey,
            "Ctrl+Shift+V"
        );
        assert_eq!(
            *fixture.hotkey.replacements.lock().unwrap(),
            [("Ctrl+Shift+V".into(), "Ctrl+Alt+X".into())]
        );
    }

    fn changed_settings() -> AppSettings {
        AppSettings {
            hotkey: "Ctrl+Alt+X".into(),
            autostart: true,
            ..AppSettings::default()
        }
    }

    #[test]
    fn poisoned_settings_lock_prevents_every_external_side_effect() {
        let fixture = fixture();
        let settings = Arc::clone(&fixture.state.settings);
        let _ = std::panic::catch_unwind(move || {
            let _guard = settings.write().unwrap();
            panic!("poison settings lock");
        });

        let error = CommandService::new(&fixture.state)
            .save_settings(changed_settings())
            .unwrap_err();

        assert_eq!(error, AppError::CoordinatorUnavailable);
        assert!(fixture.hotkey.replacements.lock().unwrap().is_empty());
        assert!(fixture.autostart.operations.lock().unwrap().is_empty());
        assert!(fixture.history.save_attempts.lock().unwrap().is_empty());
    }

    #[test]
    fn autostart_apply_failure_rolls_back_autostart_then_hotkey() {
        let fixture = fixture();
        fixture
            .autostart
            .results
            .lock()
            .unwrap()
            .extend([Err(AppError::Platform), Ok(())]);

        let error = CommandService::new(&fixture.state)
            .save_settings(changed_settings())
            .unwrap_err();

        assert_eq!(error, AppError::Platform);
        assert_eq!(*fixture.autostart.operations.lock().unwrap(), [true, false]);
        assert_eq!(
            *fixture.hotkey.replacements.lock().unwrap(),
            [
                ("Ctrl+Shift+V".into(), "Ctrl+Alt+X".into()),
                ("Ctrl+Alt+X".into(), "Ctrl+Shift+V".into())
            ]
        );
        assert!(fixture.history.save_attempts.lock().unwrap().is_empty());
    }

    #[test]
    fn database_save_failure_rolls_back_database_autostart_and_hotkey_in_reverse_order() {
        let fixture = fixture();
        fixture
            .history
            .save_results
            .lock()
            .unwrap()
            .extend([Err(AppError::Storage), Ok(())]);

        let error = CommandService::new(&fixture.state)
            .save_settings(changed_settings())
            .unwrap_err();

        assert_eq!(error, AppError::Storage);
        assert_eq!(
            *fixture.history.save_attempts.lock().unwrap(),
            [changed_settings(), AppSettings::default()]
        );
        assert_eq!(*fixture.autostart.operations.lock().unwrap(), [true, false]);
        assert_eq!(
            *fixture.hotkey.replacements.lock().unwrap(),
            [
                ("Ctrl+Shift+V".into(), "Ctrl+Alt+X".into()),
                ("Ctrl+Alt+X".into(), "Ctrl+Shift+V".into())
            ]
        );
    }

    #[test]
    fn hotkey_rollback_failure_returns_explicit_consistency_error() {
        let fixture = fixture();
        *fixture.hotkey.rejected.lock().unwrap() = Some("Ctrl+Shift+V".into());
        fixture
            .autostart
            .results
            .lock()
            .unwrap()
            .extend([Err(AppError::Platform), Ok(())]);

        let error = CommandService::new(&fixture.state)
            .save_settings(changed_settings())
            .unwrap_err();

        assert_eq!(error, AppError::SettingsConsistency);
        assert_eq!(fixture.hotkey.active.lock().unwrap().as_str(), "Ctrl+Alt+X");
        assert_eq!(
            fixture.history.load_settings().unwrap(),
            AppSettings::default()
        );
    }

    #[test]
    fn autostart_rollback_failure_still_rolls_back_hotkey_and_reports_consistency() {
        let fixture = fixture();
        fixture
            .autostart
            .results
            .lock()
            .unwrap()
            .extend([Ok(()), Err(AppError::Platform)]);
        fixture
            .history
            .save_results
            .lock()
            .unwrap()
            .extend([Err(AppError::Storage), Ok(())]);

        let error = CommandService::new(&fixture.state)
            .save_settings(changed_settings())
            .unwrap_err();

        assert_eq!(error, AppError::SettingsConsistency);
        assert_eq!(
            fixture.hotkey.active.lock().unwrap().as_str(),
            "Ctrl+Shift+V"
        );
        assert!(*fixture.autostart.enabled.lock().unwrap());
    }

    #[derive(Default)]
    struct RecordingClipboard {
        writes: Mutex<Vec<ClipboardPayload>>,
        next_sequence: AtomicU64,
    }

    impl ClipboardBackend for RecordingClipboard {
        fn read(&self, _max_item_bytes: u64) -> Result<RawClipboardSnapshot, AppError> {
            Ok(RawClipboardSnapshot::default())
        }

        fn write(&self, payload: &ClipboardPayload) -> Result<u64, AppError> {
            self.writes.lock().unwrap().push(payload.clone());
            Ok(self.next_sequence.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn sequence_number(&self) -> Result<u64, AppError> {
            Ok(0)
        }
    }

    #[derive(Default)]
    struct RecordingPasteTarget {
        paste_calls: AtomicUsize,
    }

    impl PasteTarget for RecordingPasteTarget {
        fn remember_foreground(&self) -> Result<(), AppError> {
            Ok(())
        }

        fn paste_to_remembered(&self) -> Result<(), AppError> {
            self.paste_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    struct RealFixture {
        _temp: TempDir,
        state: AppState,
        repository: Arc<SqliteHistoryRepository>,
        clipboard: Arc<RecordingClipboard>,
        paste: Arc<RecordingPasteTarget>,
    }

    fn insert(repository: &SqliteHistoryRepository, item: ClipboardItem) {
        repository
            .upsert(item, UpsertDecision::Insert { evict: None })
            .unwrap();
    }

    fn real_fixture() -> RealFixture {
        let temp = tempfile::tempdir().unwrap();
        let repository = Arc::new(SqliteHistoryRepository::open(temp.path()).unwrap());
        let settings = Arc::new(RwLock::new(AppSettings::default()));
        let clipboard = Arc::new(RecordingClipboard::default());
        let paste = Arc::new(RecordingPasteTarget::default());
        let coordinator = Arc::new(CaptureCoordinator::new(
            Arc::clone(&repository),
            Arc::clone(&clipboard),
            Arc::clone(&paste),
            Arc::clone(&settings),
            Arc::new(SystemClock),
        ));
        let hotkey = Arc::new(FakeHotkey {
            active: Mutex::new(AppSettings::default().hotkey),
            rejected: Mutex::new(None),
            replacements: Mutex::new(Vec::new()),
        });
        let state = AppState {
            history: repository.clone(),
            clipboard: coordinator,
            settings,
            hotkey,
            autostart: Arc::new(FakeAutostart::default()),
            #[cfg(windows)]
            listening: Arc::new(crate::platform::ListeningState::default()),
            #[cfg(windows)]
            overlay: None,
            #[cfg(windows)]
            monitor: Mutex::new(None),
        };
        RealFixture {
            _temp: temp,
            state,
            repository,
            clipboard,
            paste,
        }
    }

    #[test]
    fn real_command_service_lists_using_sqlite_kind_and_search_filters() {
        let fixture = real_fixture();
        insert(
            &fixture.repository,
            item("matching", ClipboardKind::Text, "Needle text", false),
        );
        insert(
            &fixture.repository,
            item("other", ClipboardKind::Text, "irrelevant", false),
        );
        let mut image = item("image", ClipboardKind::Image, "Needle image", false);
        image.payload = ClipboardPayload::Image {
            png_path: "image.png".into(),
            thumbnail_path: "thumbnail.png".into(),
            width: 1,
            height: 1,
        };
        insert(&fixture.repository, image);

        let listed = CommandService::new(&fixture.state)
            .list_history(&HistoryQuery {
                kind: Some(ClipboardKind::Text),
                search: "needle".into(),
            })
            .unwrap();

        assert_eq!(
            listed
                .iter()
                .map(|item| item.id.0.as_str())
                .collect::<Vec<_>>(),
            ["matching"]
        );
    }

    #[test]
    fn real_command_service_routes_paste_and_copy_through_capture_coordinator() {
        let fixture = real_fixture();
        insert(
            &fixture.repository,
            item("paste", ClipboardKind::Text, "paste payload", false),
        );
        insert(
            &fixture.repository,
            item("copy", ClipboardKind::Text, "copy payload", false),
        );
        let service = CommandService::new(&fixture.state);

        service.paste_item(&ItemId("paste".into())).unwrap();
        service.copy_item(&ItemId("copy".into())).unwrap();

        assert_eq!(fixture.clipboard.writes.lock().unwrap().len(), 2);
        assert_eq!(fixture.paste.paste_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn real_command_service_favorites_deletes_and_clears_sqlite_history() {
        let fixture = real_fixture();
        insert(
            &fixture.repository,
            item("keep", ClipboardKind::Text, "keep", false),
        );
        insert(
            &fixture.repository,
            item("delete", ClipboardKind::Text, "delete", false),
        );
        insert(
            &fixture.repository,
            item("clear", ClipboardKind::Text, "clear", false),
        );
        let service = CommandService::new(&fixture.state);

        service.set_favorite(&ItemId("keep".into()), true).unwrap();
        service.delete_item(&ItemId("delete".into())).unwrap();
        service.clear_history().unwrap();

        let remaining =
            HistoryRepository::list(fixture.repository.as_ref(), &HistoryQuery::default()).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, ItemId("keep".into()));
        assert!(remaining[0].is_favorite);
    }

    #[cfg(windows)]
    struct ShutdownHistory {
        settings: Mutex<AppSettings>,
        normal_count: AtomicUsize,
        events: Mutex<Vec<&'static str>>,
        clear_notification: Mutex<Option<mpsc::Sender<()>>>,
    }

    #[cfg(windows)]
    impl ShutdownHistory {
        fn new(clear_notification: Option<mpsc::Sender<()>>) -> Self {
            Self {
                settings: Mutex::new(AppSettings {
                    clear_on_exit: true,
                    ..AppSettings::default()
                }),
                normal_count: AtomicUsize::new(0),
                events: Mutex::new(Vec::new()),
                clear_notification: Mutex::new(clear_notification),
            }
        }

        fn complete_capture(&self) {
            self.normal_count.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("capture");
        }
    }

    #[cfg(windows)]
    impl CommandHistory for ShutdownHistory {
        fn list(&self, _query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError> {
            Ok(Vec::new())
        }

        fn set_favorite(&self, _id: &ItemId, _value: bool) -> Result<(), AppError> {
            Ok(())
        }

        fn delete(&self, _id: &ItemId) -> Result<(), AppError> {
            Ok(())
        }

        fn clear_normal(&self) -> Result<(), AppError> {
            self.events.lock().unwrap().push("clear");
            self.normal_count.store(0, Ordering::SeqCst);
            if let Some(notification) = self.clear_notification.lock().unwrap().take() {
                notification.send(()).unwrap();
            }
            Ok(())
        }

        fn load_settings(&self) -> Result<AppSettings, AppError> {
            Ok(self.settings.lock().unwrap().clone())
        }

        fn save_settings(&self, settings: &AppSettings) -> Result<(), AppError> {
            *self.settings.lock().unwrap() = settings.clone();
            Ok(())
        }
    }

    #[cfg(windows)]
    struct TestCaptureMonitor {
        worker: Option<thread::JoinHandle<()>>,
        stop_started: Option<mpsc::Sender<()>>,
        stop_calls: Arc<AtomicUsize>,
        result: Result<(), AppError>,
    }

    #[cfg(windows)]
    impl CaptureMonitorControl for TestCaptureMonitor {
        fn stop_and_join(&mut self) -> Result<(), AppError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(notification) = self.stop_started.take() {
                notification.send(()).unwrap();
            }
            if let Some(worker) = self.worker.take() {
                worker
                    .join()
                    .map_err(|_| AppError::CoordinatorUnavailable)?;
            }
            self.result.clone()
        }
    }

    #[cfg(windows)]
    fn shutdown_state(
        history: Arc<ShutdownHistory>,
        listening: Arc<crate::platform::ListeningState>,
        monitor: Option<Box<dyn CaptureMonitorControl>>,
    ) -> AppState {
        AppState {
            history,
            clipboard: Arc::new(FakeClipboard::default()),
            settings: Arc::new(RwLock::new(AppSettings::default())),
            hotkey: Arc::new(FakeHotkey {
                active: Mutex::new(AppSettings::default().hotkey),
                rejected: Mutex::new(None),
                replacements: Mutex::new(Vec::new()),
            }),
            autostart: Arc::new(FakeAutostart::default()),
            #[cfg(windows)]
            listening,
            #[cfg(windows)]
            overlay: None,
            #[cfg(windows)]
            monitor: Mutex::new(monitor),
        }
    }

    #[cfg(windows)]
    #[test]
    fn shutdown_joins_an_entered_capture_before_clearing_and_is_idempotent() {
        let (clear_tx, clear_rx) = mpsc::channel();
        let history = Arc::new(ShutdownHistory::new(Some(clear_tx)));
        let listening = Arc::new(crate::platform::ListeningState::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let capture_history = Arc::clone(&history);
        let capture_gate = Arc::clone(&listening);
        let worker = thread::spawn(move || {
            capture_gate.dispatch_capture(|| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                capture_history.complete_capture();
                Ok(())
            });
        });
        entered_rx.recv().unwrap();

        let (stop_started_tx, stop_started_rx) = mpsc::channel();
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let monitor = TestCaptureMonitor {
            worker: Some(worker),
            stop_started: Some(stop_started_tx),
            stop_calls: Arc::clone(&stop_calls),
            result: Ok(()),
        };
        let state = Arc::new(shutdown_state(
            Arc::clone(&history),
            Arc::clone(&listening),
            Some(Box::new(monitor)),
        ));
        let shutdown_state = Arc::clone(&state);
        let shutdown = thread::spawn(move || prepare_for_exit(&shutdown_state));

        stop_started_rx.recv().unwrap();
        let rejected_dispatches = AtomicUsize::new(0);
        listening.dispatch_capture(|| {
            rejected_dispatches.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(rejected_dispatches.load(Ordering::SeqCst), 0);
        assert!(matches!(
            clear_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        release_tx.send(()).unwrap();
        shutdown.join().unwrap().unwrap();
        clear_rx.recv().unwrap();

        assert_eq!(*history.events.lock().unwrap(), ["capture", "clear"]);
        assert_eq!(history.normal_count.load(Ordering::SeqCst), 0);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        state.shutdown_capture().unwrap();
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(windows)]
    #[test]
    fn shutdown_monitor_error_prevents_clear_and_preserves_error() {
        let history = Arc::new(ShutdownHistory::new(None));
        history.normal_count.store(1, Ordering::SeqCst);
        let listening = Arc::new(crate::platform::ListeningState::default());
        let monitor = TestCaptureMonitor {
            worker: None,
            stop_started: None,
            stop_calls: Arc::new(AtomicUsize::new(0)),
            result: Err(AppError::Clipboard),
        };
        let state = shutdown_state(
            Arc::clone(&history),
            Arc::clone(&listening),
            Some(Box::new(monitor)),
        );

        assert_eq!(prepare_for_exit(&state), Err(AppError::Clipboard));
        assert!(listening.is_paused());
        assert_eq!(history.normal_count.load(Ordering::SeqCst), 1);
        assert!(history.events.lock().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn poisoned_monitor_lock_prevents_clear_with_coordinator_error() {
        let history = Arc::new(ShutdownHistory::new(None));
        history.normal_count.store(1, Ordering::SeqCst);
        let listening = Arc::new(crate::platform::ListeningState::default());
        let state = Arc::new(shutdown_state(
            Arc::clone(&history),
            Arc::clone(&listening),
            None,
        ));
        let poison_target = Arc::clone(&state);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = poison_target.monitor.lock().unwrap();
            panic!("poison monitor lock");
        }));

        assert_eq!(
            prepare_for_exit(&state),
            Err(AppError::CoordinatorUnavailable)
        );
        assert!(listening.is_paused());
        assert_eq!(history.normal_count.load(Ordering::SeqCst), 1);
        assert!(history.events.lock().unwrap().is_empty());
    }
}
