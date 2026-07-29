use std::{
    collections::VecDeque,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        Arc, Barrier, Mutex, RwLock,
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use easy_clipboard_lib::{
    clipboard::{
        CaptureCoordinator, CaptureResult, ClipboardBackend, Clock, PasteTarget,
        RawClipboardSnapshot,
    },
    domain::{
        AppSettings, ClipboardItem, ClipboardKind, ClipboardPayload, FileEntry, HistoryQuery,
        ItemId, MediaKind, UpsertDecision,
    },
    error::AppError,
    storage::{HistoryRepository, ImageResourceStore, SqliteHistoryRepository},
};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use tempfile::TempDir;

#[derive(Default)]
struct ClipboardState {
    snapshot: RawClipboardSnapshot,
    sequence: u64,
    sequence_after_write: u64,
    writes: Vec<ClipboardPayload>,
}

#[derive(Default)]
struct FakeClipboard {
    state: Mutex<ClipboardState>,
    reads: AtomicUsize,
    read_limits: Mutex<Vec<u64>>,
    fail_read: AtomicBool,
    fail_write: AtomicBool,
    fail_sequence: AtomicBool,
    panic_sequence: AtomicBool,
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl FakeClipboard {
    fn with_snapshot(snapshot: RawClipboardSnapshot, sequence: u64) -> Self {
        Self {
            state: Mutex::new(ClipboardState {
                snapshot,
                sequence,
                sequence_after_write: sequence.saturating_add(1),
                writes: Vec::new(),
            }),
            ..Self::default()
        }
    }

    fn set_snapshot(&self, snapshot: RawClipboardSnapshot, sequence: u64) {
        let mut state = self.state.lock().unwrap();
        state.snapshot = snapshot;
        state.sequence = sequence;
    }

    fn set_sequence_after_write(&self, sequence: u64) {
        self.state.lock().unwrap().sequence_after_write = sequence;
    }

    fn writes(&self) -> Vec<ClipboardPayload> {
        self.state.lock().unwrap().writes.clone()
    }

    fn read_limits(&self) -> Vec<u64> {
        self.read_limits.lock().unwrap().clone()
    }
}

impl ClipboardBackend for FakeClipboard {
    fn read(&self, max_item_bytes: u64) -> Result<RawClipboardSnapshot, AppError> {
        self.events.lock().unwrap().push("read");
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.read_limits.lock().unwrap().push(max_item_bytes);
        if self.fail_read.load(Ordering::SeqCst) {
            return Err(AppError::Clipboard);
        }
        Ok(self.state.lock().unwrap().snapshot.clone())
    }

    fn write(&self, payload: &ClipboardPayload) -> Result<u64, AppError> {
        self.events.lock().unwrap().push("write");
        if self.fail_write.load(Ordering::SeqCst) {
            return Err(AppError::Clipboard);
        }
        let mut state = self.state.lock().unwrap();
        state.writes.push(payload.clone());
        state.sequence = state.sequence_after_write;
        Ok(state.sequence)
    }

    fn sequence_number(&self) -> Result<u64, AppError> {
        self.events.lock().unwrap().push("sequence");
        if self.panic_sequence.swap(false, Ordering::SeqCst) {
            panic!("deliberate backend panic while coordinator operation is locked");
        }
        if self.fail_sequence.load(Ordering::SeqCst) {
            return Err(AppError::Clipboard);
        }
        Ok(self.state.lock().unwrap().sequence)
    }
}

#[derive(Default)]
struct FakePasteTarget {
    remembers: AtomicUsize,
    pastes: AtomicUsize,
    fail_paste: AtomicBool,
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl PasteTarget for FakePasteTarget {
    fn remember_foreground(&self) -> Result<(), AppError> {
        self.events.lock().unwrap().push("remember");
        self.remembers.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn paste_to_remembered(&self) -> Result<(), AppError> {
        self.events.lock().unwrap().push("paste");
        self.pastes.fetch_add(1, Ordering::SeqCst);
        if self.fail_paste.load(Ordering::SeqCst) {
            Err(AppError::Paste)
        } else {
            Ok(())
        }
    }
}

struct FixedClock(AtomicI64);

impl FixedClock {
    fn new(now_ms: i64) -> Self {
        Self(AtomicI64::new(now_ms))
    }

    fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

type TestCoordinator = CaptureCoordinator<SqliteHistoryRepository, FakeClipboard, FakePasteTarget>;

fn coordinator(
    temp: &TempDir,
    clipboard: Arc<FakeClipboard>,
    paste: Arc<FakePasteTarget>,
    clock: Arc<FixedClock>,
    settings: AppSettings,
) -> (Arc<SqliteHistoryRepository>, TestCoordinator) {
    let repository = Arc::new(SqliteHistoryRepository::open(temp.path()).unwrap());
    let coordinator = CaptureCoordinator::new(
        Arc::clone(&repository),
        clipboard,
        paste,
        shared_settings(settings),
        clock,
    );
    (repository, coordinator)
}

fn shared_settings(settings: AppSettings) -> Arc<RwLock<AppSettings>> {
    Arc::new(RwLock::new(settings))
}

fn text_snapshot(text: &str) -> RawClipboardSnapshot {
    RawClipboardSnapshot {
        plain_text: Some(text.into()),
        ..RawClipboardSnapshot::default()
    }
}

fn all(repository: &impl HistoryRepository) -> Vec<ClipboardItem> {
    repository.list(&HistoryQuery::default()).unwrap()
}

fn stored(result: CaptureResult) -> ClipboardItem {
    match result {
        CaptureResult::Stored(item) => item,
        other => panic!("expected stored item, got {other:?}"),
    }
}

fn png_bytes() -> Vec<u8> {
    let image = RgbaImage::from_pixel(3, 2, Rgba([20, 40, 60, 255]));
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

fn unavailable_file_item() -> ClipboardItem {
    ClipboardItem {
        id: ItemId("missing-file".into()),
        kind: ClipboardKind::Files,
        payload: ClipboardPayload::Files {
            entries: vec![FileEntry {
                path: r"C:\missing.txt".into(),
                name: "missing.txt".into(),
                extension: "txt".into(),
                size_bytes: 10,
                media_kind: MediaKind::Other,
                available: false,
            }],
        },
        fingerprint: "missing-file-fingerprint".into(),
        preview: "missing.txt".into(),
        byte_size: 10,
        is_favorite: false,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn available_text_item(id: &str, text: &str) -> ClipboardItem {
    ClipboardItem {
        id: ItemId(id.into()),
        kind: ClipboardKind::Text,
        payload: ClipboardPayload::Text {
            plain: text.into(),
            html: None,
            rtf: None,
        },
        fingerprint: format!("{id}-fingerprint"),
        preview: text.into(),
        byte_size: text.len() as u64,
        is_favorite: false,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

struct VisibleWriteClipboard {
    state: Mutex<ClipboardState>,
    release_write: Mutex<mpsc::Receiver<()>>,
    sequence_calls: AtomicUsize,
    events: Mutex<Vec<&'static str>>,
    event_sender: mpsc::Sender<&'static str>,
}

impl VisibleWriteClipboard {
    fn record(&self, event: &'static str) {
        self.events.lock().unwrap().push(event);
        self.event_sender.send(event).unwrap();
    }
}

impl ClipboardBackend for VisibleWriteClipboard {
    fn read(&self, _max_item_bytes: u64) -> Result<RawClipboardSnapshot, AppError> {
        self.record("Read");
        Ok(RawClipboardSnapshot::default())
    }

    fn write(&self, payload: &ClipboardPayload) -> Result<u64, AppError> {
        {
            let mut state = self.state.lock().unwrap();
            state.writes.push(payload.clone());
            state.sequence = 77;
        }
        self.record("WriteVisible");
        self.release_write.lock().unwrap().recv().unwrap();
        Ok(77)
    }

    fn sequence_number(&self) -> Result<u64, AppError> {
        let event = match self.sequence_calls.fetch_add(1, Ordering::SeqCst) {
            0 => "SequenceCapture",
            extra => panic!("unexpected sequence call {extra}"),
        };
        self.record(event);
        Ok(self.state.lock().unwrap().sequence)
    }
}

struct QueuedClipboard {
    snapshots: Mutex<VecDeque<RawClipboardSnapshot>>,
}

impl ClipboardBackend for QueuedClipboard {
    fn read(&self, _max_item_bytes: u64) -> Result<RawClipboardSnapshot, AppError> {
        Ok(self
            .snapshots
            .lock()
            .unwrap()
            .pop_front()
            .expect("test supplied one snapshot per capture"))
    }

    fn write(&self, _payload: &ClipboardPayload) -> Result<u64, AppError> {
        Ok(1)
    }

    fn sequence_number(&self) -> Result<u64, AppError> {
        Ok(1)
    }
}

struct InterposedReceiptClipboard {
    current_sequence: AtomicU64,
    snapshot: Mutex<RawClipboardSnapshot>,
    writes: Mutex<Vec<ClipboardPayload>>,
    interpose_on_sequence_read: AtomicBool,
    sequence_reads: AtomicUsize,
}

impl InterposedReceiptClipboard {
    fn replace_external(&self, sequence: u64, snapshot: RawClipboardSnapshot) {
        *self.snapshot.lock().unwrap() = snapshot;
        self.current_sequence.store(sequence, Ordering::SeqCst);
    }
}

impl ClipboardBackend for InterposedReceiptClipboard {
    fn read(&self, _max_item_bytes: u64) -> Result<RawClipboardSnapshot, AppError> {
        Ok(self.snapshot.lock().unwrap().clone())
    }

    fn write(&self, payload: &ClipboardPayload) -> Result<u64, AppError> {
        self.writes.lock().unwrap().push(payload.clone());
        self.current_sequence.store(44, Ordering::SeqCst);
        Ok(44)
    }

    fn sequence_number(&self) -> Result<u64, AppError> {
        self.sequence_reads.fetch_add(1, Ordering::SeqCst);
        if self
            .interpose_on_sequence_read
            .swap(false, Ordering::SeqCst)
        {
            self.replace_external(45, text_snapshot("external 45"));
        }
        Ok(self.current_sequence.load(Ordering::SeqCst))
    }
}

struct HookedHistoryRepository {
    items: Mutex<Vec<ClipboardItem>>,
    list_calls: AtomicUsize,
    upsert_calls: AtomicUsize,
    release_first_list: Mutex<mpsc::Receiver<()>>,
    events: Mutex<Vec<&'static str>>,
    event_sender: mpsc::Sender<&'static str>,
}

impl HookedHistoryRepository {
    fn record(&self, event: &'static str) {
        self.events.lock().unwrap().push(event);
        self.event_sender.send(event).unwrap();
    }

    fn snapshot(&self) -> Vec<ClipboardItem> {
        self.items.lock().unwrap().clone()
    }
}

impl HistoryRepository for HookedHistoryRepository {
    fn list(&self, _query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError> {
        match self.list_calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                self.record("List1");
                self.release_first_list.lock().unwrap().recv().unwrap();
            }
            1 => self.record("List2"),
            extra => panic!("unexpected list call {extra}"),
        }
        Ok(self.snapshot())
    }

    fn get(&self, id: &ItemId) -> Result<ClipboardItem, AppError> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|item| &item.id == id)
            .cloned()
            .ok_or(AppError::ItemNotFound)
    }

    fn upsert(
        &self,
        mut item: ClipboardItem,
        decision: UpsertDecision,
    ) -> Result<ClipboardItem, AppError> {
        let event = match self.upsert_calls.fetch_add(1, Ordering::SeqCst) {
            0 => "Upsert1",
            1 => "Upsert2",
            extra => panic!("unexpected upsert call {extra}"),
        };
        self.record(event);
        let mut items = self.items.lock().unwrap();
        match decision {
            UpsertDecision::Insert { evict } => {
                if let Some(evict) = evict {
                    items.retain(|existing| existing.id != evict);
                }
                items.push(item.clone());
            }
            UpsertDecision::Touch { existing } => {
                let previous = items
                    .iter_mut()
                    .find(|candidate| candidate.id == existing)
                    .ok_or(AppError::ItemNotFound)?;
                item.id = previous.id.clone();
                item.is_favorite = previous.is_favorite;
                item.created_at_ms = previous.created_at_ms;
                *previous = item.clone();
            }
        }
        Ok(item)
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
        let original_len = items.len();
        items.retain(|item| &item.id != id);
        if items.len() == original_len {
            Err(AppError::ItemNotFound)
        } else {
            Ok(())
        }
    }

    fn clear_normal(&self) -> Result<(), AppError> {
        self.items.lock().unwrap().retain(|item| item.is_favorite);
        Ok(())
    }

    fn load_settings(&self) -> Result<AppSettings, AppError> {
        Ok(AppSettings::default())
    }

    fn save_settings(&self, _settings: &AppSettings) -> Result<(), AppError> {
        Ok(())
    }
}

impl ImageResourceStore for HookedHistoryRepository {
    fn write_image_resource(&self, file_name: &str, _bytes: &[u8]) -> Result<PathBuf, AppError> {
        Ok(PathBuf::from(file_name))
    }

    fn discard_image_resource(&self, _resource_path: &Path) -> Result<(), AppError> {
        Ok(())
    }
}

fn expect_no_event(receiver: &mpsc::Receiver<&'static str>, context: &str) {
    match receiver.recv_timeout(Duration::from_millis(750)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Ok(event) => panic!("{context}: unexpectedly observed {event}"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("{context}: event channel disconnected")
        }
    }
}

#[test]
fn accepted_text_capture_is_persisted() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("hello"), 7));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(1_234));
    let (repository, coordinator) =
        coordinator(&temp, clipboard, paste, clock, AppSettings::default());

    let item = stored(coordinator.capture_now().unwrap());

    assert_eq!(
        item.payload,
        ClipboardPayload::Text {
            plain: "hello".into(),
            html: None,
            rtf: None,
        }
    );
    assert_eq!(item.created_at_ms, 1_234);
    assert_eq!(all(repository.as_ref()), vec![item]);
}

#[test]
fn shared_max_item_bytes_change_applies_to_the_next_capture() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("four"), 1));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let settings = shared_settings(AppSettings {
        max_item_bytes: 3,
        ..AppSettings::default()
    });
    let repository = Arc::new(SqliteHistoryRepository::open(temp.path()).unwrap());
    let coordinator = CaptureCoordinator::new(
        Arc::clone(&repository),
        Arc::clone(&clipboard),
        paste,
        Arc::clone(&settings),
        clock,
    );

    assert_eq!(
        coordinator.capture_now().unwrap(),
        CaptureResult::IgnoredOversize {
            measured: 4,
            limit: 3,
        }
    );
    settings.write().unwrap().max_item_bytes = 4;
    clipboard.set_snapshot(text_snapshot("four"), 2);

    assert!(matches!(
        coordinator.capture_now().unwrap(),
        CaptureResult::Stored(_)
    ));
    assert_eq!(clipboard.read_limits(), vec![3, 4]);
    assert_eq!(all(repository.as_ref()).len(), 1);
}

#[test]
fn shared_history_limit_change_is_enforced_by_the_next_capture() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("first"), 1));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let settings = shared_settings(AppSettings {
        history_limit: 2,
        ..AppSettings::default()
    });
    let repository = Arc::new(SqliteHistoryRepository::open(temp.path()).unwrap());
    let coordinator = CaptureCoordinator::new(
        Arc::clone(&repository),
        Arc::clone(&clipboard),
        paste,
        Arc::clone(&settings),
        clock.clone(),
    );
    stored(coordinator.capture_now().unwrap());

    settings.write().unwrap().history_limit = 1;
    clipboard.set_snapshot(text_snapshot("second"), 2);
    clock.set(20);
    let second = stored(coordinator.capture_now().unwrap());

    assert_eq!(all(repository.as_ref()), vec![second]);
}

#[test]
fn oversize_and_empty_are_ignored_without_history() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("four"), 1));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let settings = AppSettings {
        max_item_bytes: 3,
        ..AppSettings::default()
    };
    let (repository, coordinator) =
        coordinator(&temp, Arc::clone(&clipboard), paste, clock, settings);

    assert_eq!(
        coordinator.capture_now().unwrap(),
        CaptureResult::IgnoredOversize {
            measured: 4,
            limit: 3,
        }
    );
    clipboard.set_snapshot(RawClipboardSnapshot::default(), 2);
    assert_eq!(
        coordinator.capture_now().unwrap(),
        CaptureResult::IgnoredEmpty
    );
    assert!(all(repository.as_ref()).is_empty());
}

#[test]
fn duplicate_capture_touches_original_id_preserves_favorite_and_count() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("same"), 1));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        paste,
        Arc::clone(&clock),
        AppSettings::default(),
    );
    let original = stored(coordinator.capture_now().unwrap());
    repository.set_favorite(&original.id, true).unwrap();

    clock.set(20);
    clipboard.set_snapshot(text_snapshot("same"), 2);
    let touched = stored(coordinator.capture_now().unwrap());

    assert_eq!(touched.id, original.id);
    assert_eq!(touched.created_at_ms, original.created_at_ms);
    assert!(touched.is_favorite);
    assert_eq!(touched.updated_at_ms, 20);
    assert_eq!(all(repository.as_ref()), vec![touched]);
}

#[test]
fn successful_direct_paste_writes_payload_then_pastes_and_next_exact_sequence_is_ignored() {
    let temp = TempDir::new().unwrap();
    let mut clipboard = FakeClipboard::with_snapshot(text_snapshot("paste me"), 1);
    clipboard.set_sequence_after_write(44);
    let events = Arc::new(Mutex::new(Vec::new()));
    clipboard.events = Arc::clone(&events);
    let clipboard = Arc::new(clipboard);
    let paste = Arc::new(FakePasteTarget {
        events: Arc::clone(&events),
        ..FakePasteTarget::default()
    });
    let clock = Arc::new(FixedClock::new(10));
    let (_repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        Arc::clone(&paste),
        clock,
        AppSettings::default(),
    );
    let item = stored(coordinator.capture_now().unwrap());
    events.lock().unwrap().clear();
    let reads_before = clipboard.reads.load(Ordering::SeqCst);

    coordinator.paste_item(&item.id).unwrap();

    assert_eq!(&*events.lock().unwrap(), &["write", "paste"]);
    assert_eq!(clipboard.writes(), vec![item.payload]);
    assert_eq!(paste.pastes.load(Ordering::SeqCst), 1);
    assert_eq!(
        coordinator.capture_now().unwrap(),
        CaptureResult::IgnoredSelfWrite
    );
    assert_eq!(clipboard.reads.load(Ordering::SeqCst), reads_before);
}

#[test]
fn unrelated_sequence_after_internal_write_is_not_ignored() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("first"), 1));
    clipboard.set_sequence_after_write(44);
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        paste,
        Arc::clone(&clock),
        AppSettings::default(),
    );
    let first = stored(coordinator.capture_now().unwrap());
    coordinator.copy_item(&first.id).unwrap();
    clipboard.set_snapshot(text_snapshot("unrelated"), 45);
    clock.set(20);

    let unrelated = stored(coordinator.capture_now().unwrap());

    assert_ne!(unrelated.id, first.id);
    assert_eq!(all(repository.as_ref()).len(), 2);
}

#[test]
fn write_receipt_marks_only_internal_sequence_and_mismatch_retires_marker() {
    let temp = TempDir::new().unwrap();
    let repository = Arc::new(SqliteHistoryRepository::open(temp.path()).unwrap());
    let item = available_text_item("copy-receipt-source", "copy receipt source");
    repository
        .upsert(item.clone(), UpsertDecision::Insert { evict: None })
        .unwrap();
    let clipboard = Arc::new(InterposedReceiptClipboard {
        current_sequence: AtomicU64::new(1),
        snapshot: Mutex::new(RawClipboardSnapshot::default()),
        writes: Mutex::new(Vec::new()),
        interpose_on_sequence_read: AtomicBool::new(true),
        sequence_reads: AtomicUsize::new(0),
    });
    let coordinator = Arc::new(CaptureCoordinator::new(
        Arc::clone(&repository),
        Arc::clone(&clipboard),
        Arc::new(FakePasteTarget::default()),
        shared_settings(AppSettings::default()),
        Arc::new(FixedClock::new(10)),
    ));
    coordinator.copy_item(&item.id).unwrap();
    assert_eq!(clipboard.sequence_reads.load(Ordering::SeqCst), 0);

    let external_45 = stored(coordinator.capture_now().unwrap());

    assert_eq!(external_45.preview, "external 45");
    clipboard.replace_external(44, text_snapshot("later external 44"));
    let later_44 = stored(coordinator.capture_now().unwrap());
    assert_eq!(later_44.preview, "later external 44");
    assert_eq!(all(repository.as_ref()).len(), 3);
}

#[test]
fn copy_item_writes_without_paste_and_suppresses_exact_sequence() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("copy me"), 1));
    clipboard.set_sequence_after_write(8);
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (_repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        Arc::clone(&paste),
        clock,
        AppSettings::default(),
    );
    let item = stored(coordinator.capture_now().unwrap());

    coordinator.copy_item(&item.id).unwrap();

    assert_eq!(clipboard.writes(), vec![item.payload]);
    assert_eq!(paste.pastes.load(Ordering::SeqCst), 0);
    assert_eq!(
        coordinator.capture_now().unwrap(),
        CaptureResult::IgnoredSelfWrite
    );
}

#[test]
fn missing_file_is_rejected_without_write_or_paste() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::default());
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        Arc::clone(&paste),
        clock,
        AppSettings::default(),
    );
    let item = unavailable_file_item();
    repository
        .upsert(item.clone(), UpsertDecision::Insert { evict: None })
        .unwrap();

    let error = coordinator.paste_item(&item.id).unwrap_err();

    assert_eq!(error.code(), "clipboardItemUnavailable");
    assert!(clipboard.writes().is_empty());
    assert_eq!(paste.pastes.load(Ordering::SeqCst), 0);
}

#[test]
fn prepare_overlay_remembers_foreground_only() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("unchanged"), 3));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        Arc::clone(&paste),
        clock,
        AppSettings::default(),
    );

    coordinator.prepare_overlay().unwrap();

    assert_eq!(paste.remembers.load(Ordering::SeqCst), 1);
    assert_eq!(paste.pastes.load(Ordering::SeqCst), 0);
    assert!(clipboard.writes().is_empty());
    assert_eq!(clipboard.reads.load(Ordering::SeqCst), 0);
    assert!(all(repository.as_ref()).is_empty());
}

#[test]
fn image_capture_writes_two_resources_and_persists_absolute_paths() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(
        RawClipboardSnapshot {
            png: Some(png_bytes()),
            ..RawClipboardSnapshot::default()
        },
        1,
    ));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (repository, coordinator) =
        coordinator(&temp, clipboard, paste, clock, AppSettings::default());

    let item = stored(coordinator.capture_now().unwrap());

    let ClipboardPayload::Image {
        png_path,
        thumbnail_path,
        ..
    } = &item.payload
    else {
        panic!("expected image payload");
    };
    let png_path = PathBuf::from(png_path);
    let thumbnail_path = PathBuf::from(thumbnail_path);
    assert!(png_path.is_absolute());
    assert!(thumbnail_path.is_absolute());
    assert!(png_path.is_file());
    assert!(thumbnail_path.is_file());
    assert_ne!(png_path, thumbnail_path);
    assert_eq!(all(repository.as_ref()), vec![item]);
}

#[test]
fn image_second_write_failure_discards_first_pending_resource_and_no_db_row() {
    let temp = TempDir::new().unwrap();
    let input = png_bytes();
    let normalized = easy_clipboard_lib::clipboard::normalize(
        RawClipboardSnapshot {
            png: Some(input.clone()),
            ..RawClipboardSnapshot::default()
        },
        &AppSettings::default(),
        10,
    )
    .unwrap();
    let easy_clipboard_lib::clipboard::NormalizeOutcome::Accepted(normalized) = normalized else {
        panic!("expected accepted image");
    };
    let first_name = normalized.image_resources[0].file_name.clone();
    let second_name = normalized.image_resources[1].file_name.clone();
    fs::create_dir_all(temp.path().join("images").join(&second_name)).unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(
        RawClipboardSnapshot {
            png: Some(input),
            ..RawClipboardSnapshot::default()
        },
        1,
    ));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (repository, coordinator) =
        coordinator(&temp, clipboard, paste, clock, AppSettings::default());

    let error = coordinator.capture_now().unwrap_err();

    assert_eq!(error.code(), "storageError");
    assert!(!temp.path().join("images").join(first_name).exists());
    assert!(temp.path().join("images").join(second_name).is_dir());
    assert!(all(repository.as_ref()).is_empty());
}

struct FailingImageRepository {
    items: Mutex<Vec<ClipboardItem>>,
    root: PathBuf,
}

impl FailingImageRepository {
    fn new(root: PathBuf, items: Vec<ClipboardItem>) -> Self {
        fs::create_dir_all(&root).unwrap();
        Self {
            items: Mutex::new(items),
            root,
        }
    }
}

impl HistoryRepository for FailingImageRepository {
    fn list(&self, _query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError> {
        Ok(self.items.lock().unwrap().clone())
    }

    fn get(&self, id: &ItemId) -> Result<ClipboardItem, AppError> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|item| &item.id == id)
            .cloned()
            .ok_or(AppError::ItemNotFound)
    }

    fn upsert(
        &self,
        _item: ClipboardItem,
        _decision: UpsertDecision,
    ) -> Result<ClipboardItem, AppError> {
        Err(AppError::Storage)
    }

    fn set_favorite(&self, _id: &ItemId, _value: bool) -> Result<(), AppError> {
        Err(AppError::Storage)
    }

    fn delete(&self, _id: &ItemId) -> Result<(), AppError> {
        Err(AppError::Storage)
    }

    fn clear_normal(&self) -> Result<(), AppError> {
        Err(AppError::Storage)
    }

    fn load_settings(&self) -> Result<AppSettings, AppError> {
        Ok(AppSettings::default())
    }

    fn save_settings(&self, _settings: &AppSettings) -> Result<(), AppError> {
        Err(AppError::Storage)
    }
}

impl ImageResourceStore for FailingImageRepository {
    fn write_image_resource(&self, file_name: &str, bytes: &[u8]) -> Result<PathBuf, AppError> {
        let path = self.root.join(file_name);
        if !path.exists() {
            fs::write(&path, bytes).map_err(|_| AppError::Storage)?;
        }
        Ok(path)
    }

    fn discard_image_resource(&self, resource_path: &Path) -> Result<(), AppError> {
        if resource_path.parent() != Some(self.root.as_path()) {
            return Ok(());
        }
        let referenced = self.items.lock().unwrap().iter().any(|item| {
            matches!(
                &item.payload,
                ClipboardPayload::Image {
                    png_path,
                    thumbnail_path,
                    ..
                } if Path::new(png_path) == resource_path
                    || Path::new(thumbnail_path) == resource_path
            )
        });
        if !referenced && resource_path.is_file() {
            fs::remove_file(resource_path).map_err(|_| AppError::Storage)?;
        }
        Ok(())
    }
}

#[test]
fn upsert_failure_discards_pending_but_never_removes_preexisting_referenced_resources() {
    let temp = TempDir::new().unwrap();
    let input = png_bytes();
    let normalized = easy_clipboard_lib::clipboard::normalize(
        RawClipboardSnapshot {
            png: Some(input.clone()),
            ..RawClipboardSnapshot::default()
        },
        &AppSettings::default(),
        10,
    )
    .unwrap();
    let easy_clipboard_lib::clipboard::NormalizeOutcome::Accepted(normalized) = normalized else {
        panic!("expected accepted image");
    };
    let image_root = temp.path().join("images");
    fs::create_dir_all(&image_root).unwrap();
    let png_path = image_root.join(&normalized.image_resources[0].file_name);
    let thumbnail_path = image_root.join(&normalized.image_resources[1].file_name);
    fs::write(&png_path, b"existing png").unwrap();
    fs::write(&thumbnail_path, b"existing thumbnail").unwrap();
    let mut existing = normalized.item.clone();
    existing.payload = ClipboardPayload::Image {
        png_path: png_path.to_string_lossy().into_owned(),
        thumbnail_path: thumbnail_path.to_string_lossy().into_owned(),
        width: 3,
        height: 2,
    };
    existing.is_favorite = true;
    existing.created_at_ms = 1;
    existing.updated_at_ms = 1;
    let repository = Arc::new(FailingImageRepository::new(
        image_root,
        vec![existing.clone()],
    ));
    let clipboard = Arc::new(FakeClipboard::with_snapshot(
        RawClipboardSnapshot {
            png: Some(input),
            ..RawClipboardSnapshot::default()
        },
        1,
    ));
    let paste = Arc::new(FakePasteTarget::default());
    let coordinator = CaptureCoordinator::new(
        Arc::clone(&repository),
        clipboard,
        paste,
        shared_settings(AppSettings::default()),
        Arc::new(FixedClock::new(10)),
    );

    let error = coordinator.capture_now().unwrap_err();

    assert_eq!(error.code(), "storageError");
    assert_eq!(fs::read(&png_path).unwrap(), b"existing png");
    assert_eq!(fs::read(&thumbnail_path).unwrap(), b"existing thumbnail");
    assert_eq!(
        repository.list(&HistoryQuery::default()).unwrap(),
        vec![existing]
    );
}

#[test]
fn clipboard_write_receipt_failure_does_not_paste_or_suppress() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("item"), 1));
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (_repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        Arc::clone(&paste),
        clock,
        AppSettings::default(),
    );
    let item = stored(coordinator.capture_now().unwrap());
    clipboard.fail_write.store(true, Ordering::SeqCst);

    let error = coordinator.paste_item(&item.id).unwrap_err();

    assert_eq!(error.code(), "clipboardError");
    assert_eq!(paste.pastes.load(Ordering::SeqCst), 0);
    clipboard.fail_write.store(false, Ordering::SeqCst);
    assert!(matches!(
        coordinator.capture_now().unwrap(),
        CaptureResult::Stored(_)
    ));
}

#[test]
fn paste_failure_keeps_clipboard_write_and_exact_sequence_suppression() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("item"), 1));
    clipboard.set_sequence_after_write(9);
    let paste = Arc::new(FakePasteTarget::default());
    paste.fail_paste.store(true, Ordering::SeqCst);
    let clock = Arc::new(FixedClock::new(10));
    let (_repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        Arc::clone(&paste),
        clock,
        AppSettings::default(),
    );
    let item = stored(coordinator.capture_now().unwrap());

    let error = coordinator.paste_item(&item.id).unwrap_err();

    assert_eq!(error.code(), "pasteError");
    assert_eq!(clipboard.writes(), vec![item.payload]);
    assert_eq!(paste.pastes.load(Ordering::SeqCst), 1);
    assert_eq!(
        coordinator.capture_now().unwrap(),
        CaptureResult::IgnoredSelfWrite
    );
}

#[test]
fn sequence_read_failure_propagates_without_history_mutation() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("item"), 1));
    clipboard.fail_sequence.store(true, Ordering::SeqCst);
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (repository, coordinator) = coordinator(
        &temp,
        Arc::clone(&clipboard),
        paste,
        clock,
        AppSettings::default(),
    );

    let error = coordinator.capture_now().unwrap_err();

    assert_eq!(error.code(), "clipboardError");
    assert_eq!(clipboard.reads.load(Ordering::SeqCst), 0);
    assert!(all(repository.as_ref()).is_empty());
}

#[test]
fn concurrent_capture_waits_for_internal_sequence_marker_then_ignores_exact_write() {
    let temp = TempDir::new().unwrap();
    let repository = Arc::new(SqliteHistoryRepository::open(temp.path()).unwrap());
    let item = available_text_item("copy-source", "copy source");
    repository
        .upsert(item.clone(), UpsertDecision::Insert { evict: None })
        .unwrap();
    let (event_tx, event_rx) = mpsc::channel();
    let (release_write_tx, release_write_rx) = mpsc::channel();
    let clipboard = Arc::new(VisibleWriteClipboard {
        state: Mutex::new(ClipboardState {
            sequence: 1,
            sequence_after_write: 77,
            ..ClipboardState::default()
        }),
        release_write: Mutex::new(release_write_rx),
        sequence_calls: AtomicUsize::new(0),
        events: Mutex::new(Vec::new()),
        event_sender: event_tx,
    });
    let coordinator = Arc::new(CaptureCoordinator::new(
        Arc::clone(&repository),
        Arc::clone(&clipboard),
        Arc::new(FakePasteTarget::default()),
        shared_settings(AppSettings::default()),
        Arc::new(FixedClock::new(10)),
    ));
    let copy_coordinator = Arc::clone(&coordinator);
    let copy_id = item.id.clone();
    let copy = thread::spawn(move || copy_coordinator.copy_item(&copy_id));
    assert_eq!(
        event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        "WriteVisible"
    );
    repository.delete(&item.id).unwrap();

    let attempt_barrier = Arc::new(Barrier::new(2));
    let (capture_started_tx, capture_started_rx) = mpsc::channel();
    let capture_coordinator = Arc::clone(&coordinator);
    let capture_barrier = Arc::clone(&attempt_barrier);
    let capture = thread::spawn(move || {
        capture_barrier.wait();
        capture_started_tx.send(()).unwrap();
        capture_coordinator.capture_now()
    });
    attempt_barrier.wait();
    capture_started_rx.recv().unwrap();

    expect_no_event(
        &event_rx,
        "capture entered the backend before the internal sequence marker was stored",
    );
    release_write_tx.send(()).unwrap();
    copy.join().unwrap().unwrap();
    assert_eq!(
        capture.join().unwrap().unwrap(),
        CaptureResult::IgnoredSelfWrite
    );
    assert_eq!(
        event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        "SequenceCapture"
    );
    assert_eq!(
        &*clipboard.events.lock().unwrap(),
        &["WriteVisible", "SequenceCapture"]
    );
    assert!(all(repository.as_ref()).is_empty());
}

#[test]
fn concurrent_captures_are_serialized_across_history_decision_and_upsert() {
    let (release_first_list_tx, release_first_list_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let repository = Arc::new(HookedHistoryRepository {
        items: Mutex::new(Vec::new()),
        list_calls: AtomicUsize::new(0),
        upsert_calls: AtomicUsize::new(0),
        release_first_list: Mutex::new(release_first_list_rx),
        events: Mutex::new(Vec::new()),
        event_sender: event_tx,
    });
    let clipboard = Arc::new(QueuedClipboard {
        snapshots: Mutex::new(VecDeque::from([
            text_snapshot("first"),
            text_snapshot("second"),
        ])),
    });
    let coordinator = Arc::new(CaptureCoordinator::new(
        Arc::clone(&repository),
        clipboard,
        Arc::new(FakePasteTarget::default()),
        shared_settings(AppSettings {
            history_limit: 1,
            ..AppSettings::default()
        }),
        Arc::new(FixedClock::new(10)),
    ));
    let first_coordinator = Arc::clone(&coordinator);
    let first = thread::spawn(move || first_coordinator.capture_now());
    assert_eq!(
        event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        "List1"
    );

    let attempt_barrier = Arc::new(Barrier::new(2));
    let (second_started_tx, second_started_rx) = mpsc::channel();
    let second_coordinator = Arc::clone(&coordinator);
    let second_barrier = Arc::clone(&attempt_barrier);
    let second = thread::spawn(move || {
        second_barrier.wait();
        second_started_tx.send(()).unwrap();
        second_coordinator.capture_now()
    });
    attempt_barrier.wait();
    second_started_rx.recv().unwrap();

    expect_no_event(
        &event_rx,
        "second capture entered history list while the first capture was between list and upsert",
    );
    release_first_list_tx.send(()).unwrap();
    assert!(matches!(
        first.join().unwrap().unwrap(),
        CaptureResult::Stored(_)
    ));
    assert!(matches!(
        second.join().unwrap().unwrap(),
        CaptureResult::Stored(_)
    ));
    assert_eq!(
        [
            "List1",
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ],
        ["List1", "Upsert1", "List2", "Upsert2"]
    );
    assert_eq!(
        &*repository.events.lock().unwrap(),
        &["List1", "Upsert1", "List2", "Upsert2"]
    );
    let history = repository.snapshot();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].preview, "second");
}

#[test]
fn poisoned_operation_gate_returns_sanitized_coordinator_unavailable_error() {
    let temp = TempDir::new().unwrap();
    let clipboard = Arc::new(FakeClipboard::with_snapshot(text_snapshot("item"), 1));
    clipboard.panic_sequence.store(true, Ordering::SeqCst);
    let paste = Arc::new(FakePasteTarget::default());
    let clock = Arc::new(FixedClock::new(10));
    let (repository, coordinator) =
        coordinator(&temp, clipboard, paste, clock, AppSettings::default());

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = coordinator.capture_now();
    }));
    assert!(panic.is_err());

    let error = coordinator.capture_now().unwrap_err();
    assert_eq!(error.code(), "coordinatorUnavailable");
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        serde_json::json!({
            "code": "coordinatorUnavailable",
            "message": "Clipboard coordinator is unavailable"
        })
    );
    assert!(all(repository.as_ref()).is_empty());
}
