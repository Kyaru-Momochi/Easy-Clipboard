mod sqlite;

use crate::{
    domain::{AppSettings, ClipboardItem, HistoryQuery, ItemId, UpsertDecision},
    error::AppError,
};

pub use sqlite::SqliteHistoryRepository;

pub trait HistoryRepository: Send + Sync {
    fn list(&self, query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError>;
    fn get(&self, id: &ItemId) -> Result<ClipboardItem, AppError>;
    /// Database commit success is authoritative. Image cleanup after commit is best-effort;
    /// leftover resources are reclaimed on a later successful repository open.
    fn upsert(
        &self,
        item: ClipboardItem,
        decision: UpsertDecision,
    ) -> Result<ClipboardItem, AppError>;
    fn set_favorite(&self, id: &ItemId, value: bool) -> Result<(), AppError>;
    /// Database commit success is authoritative. Image cleanup after commit is best-effort;
    /// leftover resources are reclaimed on a later successful repository open.
    fn delete(&self, id: &ItemId) -> Result<(), AppError>;
    /// Database commit success is authoritative. Image cleanup after commit is best-effort;
    /// leftover resources are reclaimed on a later successful repository open.
    fn clear_normal(&self) -> Result<(), AppError>;
    fn load_settings(&self) -> Result<AppSettings, AppError>;
    fn save_settings(&self, settings: &AppSettings) -> Result<(), AppError>;
}
