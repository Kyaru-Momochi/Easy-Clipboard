use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    domain::{AppSettings, ClipboardItem, ClipboardPayload, HistoryPolicy, HistoryQuery, ItemId},
    error::AppError,
    storage::{HistoryRepository, ImageResourceStore},
};

use super::{NormalizeOutcome, RawClipboardSnapshot, normalize};

pub trait ClipboardBackend: Send + Sync {
    /// Reads one selected clipboard category without allocating more than this capture's current
    /// configured byte limit. Platform adapters may clamp invalid values to their hard safety cap.
    fn read(&self, max_item_bytes: u64) -> Result<RawClipboardSnapshot, AppError>;
    /// Writes `payload` and returns the authoritative, nonzero clipboard sequence receipt
    /// obtained as part of that same backend operation, before another writer can interpose.
    fn write(&self, payload: &ClipboardPayload) -> Result<u64, AppError>;
    /// Observes the current sequence at capture start. Implementers must not use this as a
    /// substitute for the atomic receipt returned by `write`.
    fn sequence_number(&self) -> Result<u64, AppError>;
}

pub trait PasteTarget: Send + Sync {
    fn remember_foreground(&self) -> Result<(), AppError>;
    fn paste_to_remembered(&self) -> Result<(), AppError>;
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
            .unwrap_or(i64::MAX)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureResult {
    Stored(ClipboardItem),
    IgnoredEmpty,
    IgnoredOversize { measured: u64, limit: u64 },
    IgnoredSelfWrite,
}

pub struct CaptureCoordinator<R, C, P> {
    repository: Arc<R>,
    clipboard: Arc<C>,
    paste_target: Arc<P>,
    settings: Arc<RwLock<AppSettings>>,
    clock: Arc<dyn Clock>,
    last_internal_sequence: AtomicU64,
    operation_gate: Mutex<()>,
}

impl<R, C, P> CaptureCoordinator<R, C, P>
where
    R: HistoryRepository + ImageResourceStore,
    C: ClipboardBackend,
    P: PasteTarget,
{
    pub fn new(
        repository: Arc<R>,
        clipboard: Arc<C>,
        paste_target: Arc<P>,
        settings: Arc<RwLock<AppSettings>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repository,
            clipboard,
            paste_target,
            settings,
            clock,
            last_internal_sequence: AtomicU64::new(0),
            operation_gate: Mutex::new(()),
        }
    }

    pub fn prepare_overlay(&self) -> Result<(), AppError> {
        let _operation = self.lock_operation()?;
        self.paste_target.remember_foreground()
    }

    pub fn capture_now(&self) -> Result<CaptureResult, AppError> {
        let _operation = self.lock_operation()?;
        let sequence = self.clipboard.sequence_number()?;
        let pending_internal_sequence = self.last_internal_sequence.swap(0, Ordering::AcqRel);
        if sequence != 0 && sequence == pending_internal_sequence {
            return Ok(CaptureResult::IgnoredSelfWrite);
        }

        let settings = self
            .settings
            .read()
            .map_err(|_| AppError::CoordinatorUnavailable)?
            .clone();
        let raw = self.clipboard.read(settings.max_item_bytes)?;
        match normalize(raw, &settings, self.clock.now_ms())? {
            NormalizeOutcome::IgnoredEmpty => Ok(CaptureResult::IgnoredEmpty),
            NormalizeOutcome::IgnoredOversize {
                measured_bytes,
                limit_bytes,
            } => Ok(CaptureResult::IgnoredOversize {
                measured: measured_bytes,
                limit: limit_bytes,
            }),
            NormalizeOutcome::Accepted(normalized) => {
                let current = self.repository.list(&HistoryQuery::default())?;
                let decision = HistoryPolicy::new(settings.history_limit, settings.favorite_limit)
                    .decide_upsert(&normalized.item.fingerprint, &current);
                let mut item = normalized.item;
                let mut written = Vec::with_capacity(normalized.image_resources.len());
                let mut paths = HashMap::with_capacity(normalized.image_resources.len());

                for resource in normalized.image_resources {
                    match self
                        .repository
                        .write_image_resource(&resource.file_name, &resource.bytes)
                    {
                        Ok(path) => {
                            paths.insert(resource.file_name, path.clone());
                            written.push(path);
                        }
                        Err(error) => {
                            self.discard_pending(&written);
                            return Err(error);
                        }
                    }
                }
                if let Err(error) = replace_image_paths(&mut item.payload, &paths) {
                    self.discard_pending(&written);
                    return Err(error);
                }

                match self.repository.upsert(item, decision) {
                    Ok(persisted) => Ok(CaptureResult::Stored(persisted)),
                    Err(error) => {
                        self.discard_pending(&written);
                        Err(error)
                    }
                }
            }
        }
    }

    pub fn paste_item(&self, id: &ItemId) -> Result<(), AppError> {
        let _operation = self.lock_operation()?;
        let item = self.repository.get(id)?;
        ensure_available(&item.payload)?;
        self.write_item(&item)?;
        self.paste_target.paste_to_remembered()
    }

    pub fn copy_item(&self, id: &ItemId) -> Result<(), AppError> {
        let _operation = self.lock_operation()?;
        let item = self.repository.get(id)?;
        ensure_available(&item.payload)?;
        self.write_item(&item)
    }

    fn write_item(&self, item: &ClipboardItem) -> Result<(), AppError> {
        let sequence = self.clipboard.write(&item.payload)?;
        self.last_internal_sequence
            .store(sequence, Ordering::Release);
        Ok(())
    }

    fn discard_pending(&self, written: &[PathBuf]) {
        for path in written {
            let _ = self.repository.discard_image_resource(path);
        }
    }

    fn lock_operation(&self) -> Result<MutexGuard<'_, ()>, AppError> {
        self.operation_gate
            .lock()
            .map_err(|_| AppError::CoordinatorUnavailable)
    }
}

fn replace_image_paths(
    payload: &mut ClipboardPayload,
    paths: &HashMap<String, PathBuf>,
) -> Result<(), AppError> {
    let ClipboardPayload::Image {
        png_path,
        thumbnail_path,
        ..
    } = payload
    else {
        return Ok(());
    };

    *png_path = paths
        .get(png_path)
        .ok_or(AppError::Storage)?
        .to_string_lossy()
        .into_owned();
    *thumbnail_path = paths
        .get(thumbnail_path)
        .ok_or(AppError::Storage)?
        .to_string_lossy()
        .into_owned();
    Ok(())
}

fn ensure_available(payload: &ClipboardPayload) -> Result<(), AppError> {
    if matches!(
        payload,
        ClipboardPayload::Files { entries } if entries.iter().any(|entry| !entry.available)
    ) {
        Err(AppError::ClipboardItemUnavailable)
    } else {
        Ok(())
    }
}
