use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions as StdOpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, Transaction, functions::FunctionFlags, params};

use crate::{
    domain::{
        AppSettings, ClipboardItem, ClipboardKind, ClipboardPayload, HistoryQuery, ItemId,
        UpsertDecision,
    },
    error::AppError,
};

use super::HistoryRepository;

const DATABASE_FILE: &str = "history.sqlite3";
const LOCK_FILE: &str = ".repository.lock";
const HISTORY_LIMIT: usize = 120;
const CURRENT_SCHEMA_VERSION: u32 = 1;
const MIGRATION_001: &str = include_str!("../../migrations/001_initial.sql");
const APP_META_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS app_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";
const ITEM_COLUMNS: &str = "
    id, kind, fingerprint, payload_json, preview, byte_size, is_favorite,
    created_at_ms, updated_at_ms
";

pub struct SqliteHistoryRepository {
    connection: Mutex<Connection>,
    images_dir: Dir,
    images_root: PathBuf,
    _repository_lock: File,
}

impl SqliteHistoryRepository {
    pub fn open(app_data_root: impl AsRef<Path>) -> Result<Self, AppError> {
        fs::create_dir_all(app_data_root.as_ref()).map_err(|_| AppError::Storage)?;
        let root = fs::canonicalize(app_data_root.as_ref()).map_err(|_| AppError::Storage)?;
        let repository_lock = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(LOCK_FILE))
            .map_err(|_| AppError::Storage)?;
        match repository_lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if error.kind() == fs2::lock_contended_error().kind() => {
                return Err(AppError::RepositoryInUse);
            }
            Err(_) => return Err(AppError::Storage),
        }
        let expected_images_root = root.join("images");
        fs::create_dir_all(&expected_images_root).map_err(|_| AppError::Storage)?;
        let images_root = fs::canonicalize(&expected_images_root).map_err(|_| AppError::Storage)?;
        if images_root != expected_images_root {
            return Err(AppError::Storage);
        }
        let images_dir = Dir::open_ambient_dir(&images_root, ambient_authority())
            .map_err(|_| AppError::Storage)?;

        let mut connection =
            Connection::open(root.join(DATABASE_FILE)).map_err(|_| AppError::Storage)?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|_| AppError::Storage)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|_| AppError::Storage)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|_| AppError::Storage)?;
        register_unicode_lower(&connection)?;
        migrate(&mut connection)?;

        let repository = Self {
            connection: Mutex::new(connection),
            images_dir,
            images_root,
            _repository_lock: repository_lock,
        };
        repository.clean_orphan_resources()?;
        Ok(repository)
    }

    pub fn write_image_resource(&self, file_name: &str, bytes: &[u8]) -> Result<PathBuf, AppError> {
        validate_resource_name(file_name)?;
        let _connection = self.lock_connection()?;
        let destination = self.images_root.join(file_name);
        let temporary = format!("{file_name}.tmp");

        if self
            .images_dir
            .try_exists(&temporary)
            .map_err(|_| AppError::Storage)?
        {
            self.images_dir
                .remove_file(&temporary)
                .map_err(|_| AppError::Storage)?;
        }

        let write_result = (|| {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            let mut file = self
                .images_dir
                .open_with(&temporary, &options)
                .map_err(|_| AppError::Storage)?;
            file.write_all(bytes).map_err(|_| AppError::Storage)?;
            file.flush().map_err(|_| AppError::Storage)?;
            file.sync_all().map_err(|_| AppError::Storage)?;
            self.images_dir
                .rename(&temporary, &self.images_dir, file_name)
                .map_err(|_| AppError::Storage)
        })();

        if write_result.is_err() && self.images_dir.is_file(&temporary) {
            let _ = self.images_dir.remove_file(&temporary);
        }
        write_result?;
        Ok(destination)
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, AppError> {
        self.connection.lock().map_err(|_| AppError::Storage)
    }

    fn clean_orphan_resources(&self) -> Result<(), AppError> {
        let connection = self.lock_connection()?;
        let referenced = referenced_image_names(&connection, &self.images_root)?;
        let entries = self.images_dir.entries().map_err(|_| AppError::Storage)?;

        for entry in entries {
            let entry = entry.map_err(|_| AppError::Storage)?;
            let file_type = entry.file_type().map_err(|_| AppError::Storage)?;
            if !file_type.is_file() {
                continue;
            }

            let name = PathBuf::from(entry.file_name());
            let extension = name.extension().and_then(|value| value.to_str());
            if extension == Some("tmp") {
                self.images_dir
                    .remove_file(name)
                    .map_err(|_| AppError::Storage)?;
            } else if extension == Some("png") && !referenced.contains(&name) {
                self.images_dir
                    .remove_file(name)
                    .map_err(|_| AppError::Storage)?;
            }
        }
        Ok(())
    }
}

impl HistoryRepository for SqliteHistoryRepository {
    fn list(&self, query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError> {
        let connection = self.lock_connection()?;
        let kind = query.kind.as_ref().map(kind_to_database);
        let escaped_search = escape_like_pattern(&query.search);
        let pattern = format!("%{escaped_search}%");
        let sql = format!(
            "SELECT {ITEM_COLUMNS}
             FROM clipboard_items
             WHERE (?1 IS NULL OR kind = ?1)
               AND (?2 = '' OR unicode_lower(preview) LIKE unicode_lower(?3) ESCAPE '\\')
             ORDER BY is_favorite DESC, updated_at_ms DESC, id ASC
             LIMIT {HISTORY_LIMIT}"
        );
        let mut statement = connection.prepare(&sql).map_err(|_| AppError::Storage)?;
        let mut rows = statement
            .query(params![kind, query.search, pattern])
            .map_err(|_| AppError::Storage)?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|_| AppError::Storage)? {
            items.push(read_item(row)?);
        }
        Ok(items)
    }

    fn get(&self, id: &ItemId) -> Result<ClipboardItem, AppError> {
        let connection = self.lock_connection()?;
        get_item(&connection, id)
    }

    fn upsert(
        &self,
        item: ClipboardItem,
        decision: UpsertDecision,
    ) -> Result<ClipboardItem, AppError> {
        validate_item(&item)?;
        let byte_size = i64::try_from(item.byte_size).map_err(|_| AppError::Storage)?;
        let payload_json = serde_json::to_string(&item.payload).map_err(|_| AppError::Storage)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(|_| AppError::Storage)?;

        let (persisted, resources_to_clean) = match decision {
            UpsertDecision::Insert { evict } => {
                let resources = match evict.as_ref() {
                    Some(id) => optional_item(&transaction, id)?
                        .map(|item| image_paths(&item.payload))
                        .unwrap_or_default(),
                    None => Vec::new(),
                };
                if let Some(id) = evict {
                    transaction
                        .execute("DELETE FROM clipboard_items WHERE id = ?1", [&id.0])
                        .map_err(|_| AppError::Storage)?;
                }
                transaction
                    .execute(
                        "INSERT INTO clipboard_items (
                            id, kind, fingerprint, payload_json, preview, byte_size,
                            is_favorite, created_at_ms, updated_at_ms
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            item.id.0,
                            kind_to_database(&item.kind),
                            item.fingerprint,
                            payload_json,
                            item.preview,
                            byte_size,
                            item.is_favorite,
                            item.created_at_ms,
                            item.updated_at_ms,
                        ],
                    )
                    .map_err(|_| AppError::Storage)?;
                (get_item(&transaction, &item.id)?, resources)
            }
            UpsertDecision::Touch { existing } => {
                let previous = get_item(&transaction, &existing)?;
                let resources = image_paths(&previous.payload);
                let changed = transaction
                    .execute(
                        "UPDATE clipboard_items
                         SET kind = ?1, fingerprint = ?2, payload_json = ?3, preview = ?4,
                             byte_size = ?5, updated_at_ms = ?6
                         WHERE id = ?7",
                        params![
                            kind_to_database(&item.kind),
                            item.fingerprint,
                            payload_json,
                            item.preview,
                            byte_size,
                            item.updated_at_ms,
                            existing.0,
                        ],
                    )
                    .map_err(|_| AppError::Storage)?;
                if changed == 0 {
                    return Err(AppError::ItemNotFound);
                }
                (get_item(&transaction, &existing)?, resources)
            }
        };

        transaction.commit().map_err(|_| AppError::Storage)?;
        delete_unreferenced_resources(
            &connection,
            &self.images_dir,
            &self.images_root,
            resources_to_clean,
        );
        Ok(persisted)
    }

    fn set_favorite(&self, id: &ItemId, value: bool) -> Result<(), AppError> {
        let connection = self.lock_connection()?;
        let changed = connection
            .execute(
                "UPDATE clipboard_items SET is_favorite = ?1 WHERE id = ?2",
                params![value, id.0],
            )
            .map_err(|_| AppError::Storage)?;
        if changed == 0 {
            Err(AppError::ItemNotFound)
        } else {
            Ok(())
        }
    }

    fn delete(&self, id: &ItemId) -> Result<(), AppError> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(|_| AppError::Storage)?;
        let item = get_item(&transaction, id)?;
        let resources = image_paths(&item.payload);
        let changed = transaction
            .execute("DELETE FROM clipboard_items WHERE id = ?1", [&id.0])
            .map_err(|_| AppError::Storage)?;
        if changed == 0 {
            return Err(AppError::ItemNotFound);
        }
        transaction.commit().map_err(|_| AppError::Storage)?;
        delete_unreferenced_resources(&connection, &self.images_dir, &self.images_root, resources);
        Ok(())
    }

    fn clear_normal(&self) -> Result<(), AppError> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(|_| AppError::Storage)?;
        let resources = normal_image_paths(&transaction)?;
        transaction
            .execute("DELETE FROM clipboard_items WHERE is_favorite = 0", [])
            .map_err(|_| AppError::Storage)?;
        transaction.commit().map_err(|_| AppError::Storage)?;
        delete_unreferenced_resources(&connection, &self.images_dir, &self.images_root, resources);
        Ok(())
    }

    fn load_settings(&self) -> Result<AppSettings, AppError> {
        let connection = self.lock_connection()?;
        let json: Option<String> = connection
            .query_row("SELECT json FROM settings WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|_| AppError::Storage)?;
        match json {
            Some(json) => serde_json::from_str(&json).map_err(|_| AppError::Storage),
            None => Ok(AppSettings::default()),
        }
    }

    fn save_settings(&self, settings: &AppSettings) -> Result<(), AppError> {
        if !settings.motion_scale.is_finite() {
            return Err(AppError::InvalidMotionScale);
        }
        let json = serde_json::to_string(settings).map_err(|_| AppError::Storage)?;
        let connection = self.lock_connection()?;
        connection
            .execute(
                "INSERT INTO settings(id, json) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET json = excluded.json",
                [&json],
            )
            .map_err(|_| AppError::Storage)?;
        Ok(())
    }
}

fn migrate(connection: &mut Connection) -> Result<(), AppError> {
    let transaction = connection.transaction().map_err(|_| AppError::Storage)?;
    transaction
        .execute_batch(APP_META_TABLE)
        .map_err(|_| AppError::Storage)?;
    let marker: Option<String> = transaction
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| AppError::Storage)?;
    let mut version = match marker {
        Some(marker) => marker
            .parse::<u32>()
            .map_err(|_| AppError::UnsupportedDatabaseVersion)?,
        None => 0,
    };
    if version > CURRENT_SCHEMA_VERSION {
        return Err(AppError::UnsupportedDatabaseVersion);
    }

    while version < CURRENT_SCHEMA_VERSION {
        let next_version = version + 1;
        match next_version {
            1 => transaction
                .execute_batch(MIGRATION_001)
                .map_err(|_| AppError::Storage)?,
            _ => return Err(AppError::UnsupportedDatabaseVersion),
        }
        transaction
            .execute(
                "INSERT INTO app_meta(key, value) VALUES ('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [next_version.to_string()],
            )
            .map_err(|_| AppError::Storage)?;
        version = next_version;
    }

    transaction.commit().map_err(|_| AppError::Storage)
}

fn register_unicode_lower(connection: &Connection) -> Result<(), AppError> {
    connection
        .create_scalar_function(
            "unicode_lower",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| {
                let value = context.get::<String>(0)?;
                Ok(value.to_lowercase())
            },
        )
        .map_err(|_| AppError::Storage)
}

fn validate_item(item: &ClipboardItem) -> Result<(), AppError> {
    if kind_matches_payload(&item.kind, &item.payload) {
        Ok(())
    } else {
        Err(AppError::InvalidClipboardItem)
    }
}

fn kind_matches_payload(kind: &ClipboardKind, payload: &ClipboardPayload) -> bool {
    matches!(
        (kind, payload),
        (ClipboardKind::Text, ClipboardPayload::Text { .. })
            | (ClipboardKind::Image, ClipboardPayload::Image { .. })
            | (ClipboardKind::Files, ClipboardPayload::Files { .. })
    )
}

fn validate_resource_name(file_name: &str) -> Result<(), AppError> {
    let path = Path::new(file_name);
    let mut components = path.components();
    let one_normal_component =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    let invalid_windows_character = file_name
        .chars()
        .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character));

    if file_name.is_empty()
        || !file_name.ends_with(".png")
        || !one_normal_component
        || invalid_windows_character
    {
        Err(AppError::Storage)
    } else {
        Ok(())
    }
}

fn kind_to_database(kind: &ClipboardKind) -> &'static str {
    match kind {
        ClipboardKind::Text => "text",
        ClipboardKind::Image => "image",
        ClipboardKind::Files => "files",
    }
}

fn kind_from_database(kind: &str) -> Result<ClipboardKind, AppError> {
    match kind {
        "text" => Ok(ClipboardKind::Text),
        "image" => Ok(ClipboardKind::Image),
        "files" => Ok(ClipboardKind::Files),
        _ => Err(AppError::Storage),
    }
}

fn escape_like_pattern(search: &str) -> String {
    search
        .replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_")
}

fn read_item(row: &rusqlite::Row<'_>) -> Result<ClipboardItem, AppError> {
    let kind: String = row.get(1).map_err(|_| AppError::Storage)?;
    let payload_json: String = row.get(3).map_err(|_| AppError::Storage)?;
    let byte_size: i64 = row.get(5).map_err(|_| AppError::Storage)?;
    let kind = kind_from_database(&kind)?;
    let payload = serde_json::from_str(&payload_json).map_err(|_| AppError::Storage)?;
    if !kind_matches_payload(&kind, &payload) {
        return Err(AppError::Storage);
    }
    Ok(ClipboardItem {
        id: ItemId(row.get(0).map_err(|_| AppError::Storage)?),
        kind,
        fingerprint: row.get(2).map_err(|_| AppError::Storage)?,
        payload,
        preview: row.get(4).map_err(|_| AppError::Storage)?,
        byte_size: u64::try_from(byte_size).map_err(|_| AppError::Storage)?,
        is_favorite: row.get(6).map_err(|_| AppError::Storage)?,
        created_at_ms: row.get(7).map_err(|_| AppError::Storage)?,
        updated_at_ms: row.get(8).map_err(|_| AppError::Storage)?,
    })
}

fn get_item(connection: &Connection, id: &ItemId) -> Result<ClipboardItem, AppError> {
    optional_item(connection, id)?.ok_or(AppError::ItemNotFound)
}

fn optional_item(connection: &Connection, id: &ItemId) -> Result<Option<ClipboardItem>, AppError> {
    let sql = format!("SELECT {ITEM_COLUMNS} FROM clipboard_items WHERE id = ?1");
    let mut statement = connection.prepare(&sql).map_err(|_| AppError::Storage)?;
    let mut rows = statement.query([&id.0]).map_err(|_| AppError::Storage)?;
    rows.next()
        .map_err(|_| AppError::Storage)?
        .map(read_item)
        .transpose()
}

fn normal_image_paths(transaction: &Transaction<'_>) -> Result<Vec<PathBuf>, AppError> {
    let mut statement = transaction
        .prepare(
            "SELECT payload_json FROM clipboard_items WHERE is_favorite = 0 AND kind = 'image'",
        )
        .map_err(|_| AppError::Storage)?;
    let mut rows = statement.query([]).map_err(|_| AppError::Storage)?;
    let mut paths = Vec::new();
    while let Some(row) = rows.next().map_err(|_| AppError::Storage)? {
        let json: String = row.get(0).map_err(|_| AppError::Storage)?;
        let payload: ClipboardPayload =
            serde_json::from_str(&json).map_err(|_| AppError::Storage)?;
        if !matches!(payload, ClipboardPayload::Image { .. }) {
            return Err(AppError::Storage);
        }
        paths.extend(image_paths(&payload));
    }
    Ok(paths)
}

fn image_paths(payload: &ClipboardPayload) -> Vec<PathBuf> {
    match payload {
        ClipboardPayload::Image {
            png_path,
            thumbnail_path,
            ..
        } => vec![PathBuf::from(png_path), PathBuf::from(thumbnail_path)],
        ClipboardPayload::Text { .. } | ClipboardPayload::Files { .. } => Vec::new(),
    }
}

fn referenced_image_names(
    connection: &Connection,
    images_root: &Path,
) -> Result<HashSet<PathBuf>, AppError> {
    let mut statement = connection
        .prepare("SELECT kind, payload_json FROM clipboard_items")
        .map_err(|_| AppError::Storage)?;
    let mut rows = statement.query([]).map_err(|_| AppError::Storage)?;
    let mut referenced = HashSet::new();
    while let Some(row) = rows.next().map_err(|_| AppError::Storage)? {
        let kind: String = row.get(0).map_err(|_| AppError::Storage)?;
        let json: String = row.get(1).map_err(|_| AppError::Storage)?;
        let kind = kind_from_database(&kind)?;
        let payload: ClipboardPayload =
            serde_json::from_str(&json).map_err(|_| AppError::Storage)?;
        if !kind_matches_payload(&kind, &payload) {
            return Err(AppError::Storage);
        }
        if matches!(kind, ClipboardKind::Image) {
            for path in image_paths(&payload) {
                if let Some(name) = safe_resource_name(images_root, &path) {
                    referenced.insert(name);
                }
            }
        }
    }
    Ok(referenced)
}

fn safe_resource_name(images_root: &Path, candidate: &Path) -> Option<PathBuf> {
    if !candidate.is_absolute() {
        return None;
    }
    let relative = candidate.strip_prefix(images_root).ok()?;
    let mut components = relative.components();
    let Component::Normal(_) = components.next()? else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    validate_resource_name(relative.to_str()?).ok()?;
    Some(relative.to_path_buf())
}

fn delete_unreferenced_resources(
    connection: &Connection,
    images_dir: &Dir,
    images_root: &Path,
    candidates: Vec<PathBuf>,
) {
    let Ok(referenced) = referenced_image_names(connection, images_root) else {
        return;
    };
    let mut deleted = HashSet::new();
    for candidate in candidates {
        let Some(name) = safe_resource_name(images_root, &candidate) else {
            continue;
        };
        if !referenced.contains(&name) && deleted.insert(name.clone()) {
            let _ = images_dir.remove_file(name);
        }
    }
}
