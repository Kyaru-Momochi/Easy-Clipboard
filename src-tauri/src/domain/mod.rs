mod history;
mod model;
mod settings;

pub use history::{HistoryPolicy, UpsertDecision};
pub use model::{
    ClipboardItem, ClipboardKind, ClipboardPayload, FileEntry, HistoryQuery, ItemId, MediaKind,
    ThemeMode,
};
pub use settings::AppSettings;
