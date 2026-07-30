use std::{fs, path::Path};

use easy_clipboard_lib::{
    domain::{
        AppSettings, ClipboardItem, ClipboardKind, ClipboardPayload, FileEntry, HistoryQuery,
        ItemId, MediaKind, ThemeMode, UpsertDecision,
    },
    error::AppError,
    storage::{HistoryRepository, SqliteHistoryRepository},
};
use tempfile::TempDir;

#[cfg(windows)]
fn create_directory_redirect(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("junction command should run");
    assert!(
        output.status.success(),
        "junction creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn create_directory_redirect(link: &Path, target: &Path) {
    std::os::unix::fs::symlink(target, link).expect("directory symlink should be created");
}

#[cfg(windows)]
fn remove_directory_redirect(link: &Path) {
    fs::remove_dir(link).expect("junction should be removed");
}

#[cfg(unix)]
fn remove_directory_redirect(link: &Path) {
    fs::remove_file(link).expect("directory symlink should be removed");
}

fn repository(temp: &TempDir) -> SqliteHistoryRepository {
    SqliteHistoryRepository::open(temp.path()).expect("repository should open")
}

fn text_item(
    id: &str,
    fingerprint: &str,
    preview: &str,
    favorite: bool,
    updated_at_ms: i64,
) -> ClipboardItem {
    ClipboardItem {
        id: ItemId(id.into()),
        kind: ClipboardKind::Text,
        payload: ClipboardPayload::Text {
            plain: preview.into(),
            html: None,
            rtf: None,
        },
        fingerprint: fingerprint.into(),
        preview: preview.into(),
        byte_size: preview.len() as u64,
        is_favorite: favorite,
        created_at_ms: updated_at_ms.saturating_sub(1),
        updated_at_ms,
    }
}

fn image_item(
    id: &str,
    fingerprint: &str,
    png_path: &Path,
    thumbnail_path: &Path,
    favorite: bool,
    updated_at_ms: i64,
) -> ClipboardItem {
    ClipboardItem {
        id: ItemId(id.into()),
        kind: ClipboardKind::Image,
        payload: ClipboardPayload::Image {
            png_path: png_path.to_string_lossy().into_owned(),
            thumbnail_path: thumbnail_path.to_string_lossy().into_owned(),
            width: 10,
            height: 20,
        },
        fingerprint: fingerprint.into(),
        preview: format!("image-{id}"),
        byte_size: 4,
        is_favorite: favorite,
        created_at_ms: updated_at_ms.saturating_sub(1),
        updated_at_ms,
    }
}

fn files_item(id: &str, fingerprint: &str, preview: &str, updated_at_ms: i64) -> ClipboardItem {
    ClipboardItem {
        id: ItemId(id.into()),
        kind: ClipboardKind::Files,
        payload: ClipboardPayload::Files {
            entries: vec![FileEntry {
                path: r"C:\data\example.txt".into(),
                name: "example.txt".into(),
                extension: "txt".into(),
                size_bytes: 7,
                media_kind: MediaKind::Other,
                available: true,
            }],
        },
        fingerprint: fingerprint.into(),
        preview: preview.into(),
        byte_size: 7,
        is_favorite: false,
        created_at_ms: updated_at_ms.saturating_sub(1),
        updated_at_ms,
    }
}

fn insert(repository: &SqliteHistoryRepository, item: ClipboardItem) -> ClipboardItem {
    repository
        .upsert(item, UpsertDecision::Insert { evict: None })
        .expect("item should insert")
}

fn all(repository: &SqliteHistoryRepository) -> Vec<ClipboardItem> {
    repository
        .list(&HistoryQuery::default())
        .expect("history should list")
}

#[test]
fn upsert_duplicate_touches_existing_row() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let original = text_item("original", "same-fingerprint", "old", true, 10);
    insert(&repository, original.clone());
    let incoming = text_item("incoming", "same-fingerprint", "new", false, 20);

    let persisted = repository
        .upsert(
            incoming,
            UpsertDecision::Touch {
                existing: original.id.clone(),
            },
        )
        .unwrap();

    assert_eq!(persisted.id, original.id);
    assert!(persisted.is_favorite);
    assert_eq!(persisted.created_at_ms, original.created_at_ms);
    assert_eq!(persisted.updated_at_ms, 20);
    assert_eq!(persisted.preview, "new");
    assert_eq!(all(&repository), vec![persisted]);
}

#[test]
fn transaction_inserts_and_evicts_atomically() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let first_resource = repository
        .write_image_resource("first.png", b"first")
        .unwrap();
    let first = insert(
        &repository,
        image_item(
            "first",
            "fp-first",
            &first_resource,
            &first_resource,
            false,
            10,
        ),
    );
    let second = insert(
        &repository,
        text_item("second", "fp-second", "second", false, 20),
    );

    let third = repository
        .upsert(
            text_item("third", "fp-third", "third", false, 30),
            UpsertDecision::Insert {
                evict: Some(first.id.clone()),
            },
        )
        .unwrap();

    assert_eq!(
        all(&repository)
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        vec![third.id, second.id.clone()]
    );
    assert!(!first_resource.exists());

    let victim_resource = repository
        .write_image_resource("victim.png", b"victim")
        .unwrap();
    let victim = insert(
        &repository,
        image_item(
            "victim",
            "fp-victim",
            &victim_resource,
            &victim_resource,
            false,
            5,
        ),
    );
    let collision = text_item("collision", "fp-second", "collision", false, 40);
    assert!(
        repository
            .upsert(
                collision,
                UpsertDecision::Insert {
                    evict: Some(victim.id.clone()),
                },
            )
            .is_err()
    );
    assert_eq!(repository.get(&victim.id).unwrap(), victim);
    assert!(
        victim_resource.exists(),
        "post-rollback resources must remain referenced"
    );
}

#[test]
fn deleting_image_removes_only_unreferenced_safe_resources() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let shared = repository
        .write_image_resource("shared.png", b"png")
        .unwrap();
    let first = insert(
        &repository,
        image_item("image-a", "image-fp-a", &shared, &shared, false, 10),
    );
    let second = insert(
        &repository,
        image_item("image-b", "image-fp-b", &shared, &shared, false, 20),
    );

    repository.delete(&first.id).unwrap();
    assert!(shared.exists(), "a resource still referenced must remain");
    repository.delete(&second.id).unwrap();
    assert!(
        !shared.exists(),
        "the final reference removal should clean it"
    );

    let external_root = TempDir::new().unwrap();
    let external = external_root.path().join("external.png");
    fs::write(&external, b"external").unwrap();
    let unsafe_item = insert(
        &repository,
        image_item("external", "external-fp", &external, &external, false, 30),
    );
    repository.delete(&unsafe_item.id).unwrap();
    assert!(
        external.exists(),
        "paths outside images must never be deleted"
    );
}

#[test]
fn query_filters_kind_and_case_insensitive_literal_text() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    insert(
        &repository,
        text_item("favorite-b", "fp-favorite-b", "Alpha %_ value", true, 50),
    );
    insert(
        &repository,
        text_item("favorite-a", "fp-favorite-a", "alpha %_ value", true, 50),
    );
    insert(
        &repository,
        text_item("normal", "fp-normal", "ALPHA %_ value", false, 100),
    );
    insert(
        &repository,
        text_item("wildcard-only", "fp-wildcard", "alpha xx value", false, 200),
    );
    insert(
        &repository,
        files_item("files", "fp-files", "alpha %_ value", 300),
    );

    let filtered = repository
        .list(&HistoryQuery {
            kind: Some(ClipboardKind::Text),
            search: "%_".into(),
        })
        .unwrap();
    assert_eq!(
        filtered
            .iter()
            .map(|item| item.id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["favorite-a", "favorite-b", "normal"]
    );

    let case_insensitive = repository
        .list(&HistoryQuery {
            kind: Some(ClipboardKind::Text),
            search: "aLpHa".into(),
        })
        .unwrap();
    assert_eq!(case_insensitive.len(), 4);

    insert(
        &repository,
        text_item(
            "literal-backslash",
            "fp-literal-backslash",
            r"literal \ marker",
            false,
            400,
        ),
    );
    insert(
        &repository,
        text_item(
            "backslash-decoy",
            "fp-backslash-decoy",
            "literal x marker",
            false,
            500,
        ),
    );
    let literal_backslash = repository
        .list(&HistoryQuery {
            kind: Some(ClipboardKind::Text),
            search: r"\".into(),
        })
        .unwrap();
    assert_eq!(
        literal_backslash
            .iter()
            .map(|item| item.id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["literal-backslash"]
    );

    insert(
        &repository,
        text_item("unicode-case", "fp-unicode-case", "ÉCOLE déjà", false, 600),
    );
    let unicode_case_insensitive = repository
        .list(&HistoryQuery {
            kind: Some(ClipboardKind::Text),
            search: "école".into(),
        })
        .unwrap();
    assert_eq!(
        unicode_case_insensitive
            .iter()
            .map(|item| item.id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["unicode-case"]
    );

    for index in 0..125 {
        insert(
            &repository,
            text_item(
                &format!("bulk-{index:03}"),
                &format!("fp-bulk-{index:03}"),
                "bulk",
                false,
                1_000 + index,
            ),
        );
    }
    assert_eq!(
        repository
            .list(&HistoryQuery {
                kind: None,
                search: "bulk".into(),
            })
            .unwrap()
            .len(),
        120
    );
}

#[test]
fn get_set_favorite_and_delete_missing_return_item_not_found() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let missing = ItemId("missing".into());

    for error in [
        repository.get(&missing).unwrap_err(),
        repository.set_favorite(&missing, true).unwrap_err(),
        repository.delete(&missing).unwrap_err(),
    ] {
        assert_eq!(error.code(), "itemNotFound");
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::json!({
                "code": "itemNotFound",
                "message": "Clipboard item was not found"
            })
        );
    }
}

#[test]
fn clear_normal_preserves_favorites_and_cleans_only_removed_images() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let normal_path = repository
        .write_image_resource("normal.png", b"normal")
        .unwrap();
    let favorite_path = repository
        .write_image_resource("favorite.png", b"favorite")
        .unwrap();
    insert(
        &repository,
        image_item("normal", "normal-fp", &normal_path, &normal_path, false, 10),
    );
    let favorite = insert(
        &repository,
        image_item(
            "favorite",
            "favorite-fp",
            &favorite_path,
            &favorite_path,
            true,
            20,
        ),
    );
    insert(
        &repository,
        text_item("normal-text", "normal-text-fp", "normal", false, 30),
    );

    repository.clear_normal().unwrap();

    assert_eq!(all(&repository), vec![favorite]);
    assert!(!normal_path.exists());
    assert!(favorite_path.exists());
}

#[test]
fn settings_default_and_round_trip() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    assert_eq!(repository.load_settings().unwrap(), AppSettings::default());
    let first = AppSettings {
        history_limit: 42,
        favorite_limit: 7,
        max_item_bytes: 3_000_000,
        hotkey: "Alt+V".into(),
        theme: ThemeMode::Dark,
        motion_scale: 0.75,
        autostart: true,
        clear_on_exit: true,
    };
    let second = AppSettings {
        history_limit: 73,
        favorite_limit: 11,
        max_item_bytes: 9_000_000,
        hotkey: "Ctrl+Alt+C".into(),
        theme: ThemeMode::Light,
        motion_scale: 0.5,
        autostart: false,
        clear_on_exit: false,
    };

    repository.save_settings(&first).unwrap();
    repository.save_settings(&second).unwrap();

    assert_eq!(repository.load_settings().unwrap(), second);
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    let row_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
        .unwrap();
    assert_eq!(row_count, 1);
}

#[test]
fn corrupt_settings_json_is_repaired_to_defaults_without_losing_history() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let saved = insert(
        &repository,
        text_item("kept", "kept-fingerprint", "kept history", false, 7),
    );
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT INTO settings(id, json) VALUES (1, '{broken')
             ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            [],
        )
        .unwrap();

    assert_eq!(repository.load_settings().unwrap(), AppSettings::default());
    assert_eq!(all(&repository), vec![saved]);

    let repaired: String = connection
        .query_row("SELECT json FROM settings WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<AppSettings>(&repaired).unwrap(),
        AppSettings::default()
    );
}

#[test]
fn settings_database_errors_are_not_treated_as_missing_or_corrupt_json() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    connection.execute("DROP TABLE settings", []).unwrap();

    assert_eq!(repository.load_settings().unwrap_err(), AppError::Storage);
}

#[cfg(any(windows, unix))]
#[test]
fn open_rejects_images_directory_redirected_outside_app_root() {
    let app_root = TempDir::new().unwrap();
    let external_root = TempDir::new().unwrap();
    let external_file = external_root.path().join("outside.png");
    fs::write(&external_file, b"outside").unwrap();
    create_directory_redirect(&app_root.path().join("images"), external_root.path());

    let error = match SqliteHistoryRepository::open(app_root.path()) {
        Ok(_) => panic!("redirected images directory must be rejected"),
        Err(error) => error,
    };

    assert_eq!(error.code(), "storageError");
    assert!(
        external_file.exists(),
        "rejecting open must not clean external files"
    );
}

#[test]
fn open_cleans_tmp_and_orphan_png_but_keeps_referenced_image() {
    let temp = TempDir::new().unwrap();
    let referenced = {
        let repository = repository(&temp);
        let referenced = repository
            .write_image_resource("referenced.png", b"referenced")
            .unwrap();
        insert(
            &repository,
            image_item(
                "referenced",
                "referenced-fp",
                &referenced,
                &referenced,
                false,
                10,
            ),
        );
        referenced
    };
    let images = temp.path().join("images");
    let orphan = images.join("orphan.png");
    let temporary = images.join("interrupted.tmp");
    fs::write(&orphan, b"orphan").unwrap();
    fs::write(&temporary, b"temporary").unwrap();
    fs::create_dir(images.join("nested")).unwrap();

    let reopened = repository(&temp);

    assert!(referenced.exists());
    assert!(!orphan.exists());
    assert!(!temporary.exists());
    assert!(images.join("nested").is_dir());
    assert_eq!(all(&reopened).len(), 1);
}

#[test]
fn atomic_resource_writer_rejects_unsafe_names() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);

    for name in [
        "",
        ".",
        "..",
        "image.jpg",
        "image.PNG",
        "./image.png",
        "../image.png",
        "nested/image.png",
        r"nested\image.png",
        r"C:\image.png",
    ] {
        let error = repository
            .write_image_resource(name, b"unsafe")
            .unwrap_err();
        assert_eq!(error.code(), "storageError", "name {name:?}");
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::json!({
                "code": "storageError",
                "message": "Clipboard storage operation failed"
            })
        );
    }

    let safe = repository
        .write_image_resource("safe-name.png", b"safe")
        .unwrap();
    assert_eq!(fs::read(&safe).unwrap(), b"safe");
    assert!(!safe.with_file_name("safe-name.png.tmp").exists());
}

#[test]
fn atomic_resource_writer_cleans_tmp_after_finalization_failure() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let images = temp.path().join("images");
    let conflicting_destination = images.join("conflict.png");
    let temporary = images.join("conflict.png.tmp");
    fs::create_dir(&conflicting_destination).unwrap();

    let error = repository
        .write_image_resource("conflict.png", b"cannot finalize")
        .unwrap_err();

    assert_eq!(error.code(), "storageError");
    assert!(conflicting_destination.is_dir());
    assert!(!temporary.exists());
}

#[test]
fn byte_size_over_i64_max_is_rejected_without_wrap() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let mut oversized = text_item("oversized", "oversized-fp", "large", false, 10);
    oversized.byte_size = i64::MAX as u64 + 1;

    let error = repository
        .upsert(oversized, UpsertDecision::Insert { evict: None })
        .unwrap_err();

    assert_eq!(error.code(), "storageError");
    assert!(all(&repository).is_empty());
}

#[test]
fn empty_database_migrates_to_current_schema_version() {
    let temp = TempDir::new().unwrap();

    let repository = repository(&temp);

    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    let version: String = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "1");
    assert!(all(&repository).is_empty());
}

#[test]
fn current_schema_version_reopens_without_rewriting_or_losing_data() {
    let temp = TempDir::new().unwrap();
    let persisted = {
        let repository = repository(&temp);
        insert(
            &repository,
            text_item("persisted", "persisted-fp", "persisted", false, 10),
        )
    };
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE app_meta SET value = '01' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    drop(connection);

    let repository = repository(&temp);

    assert_eq!(repository.get(&persisted.id).unwrap(), persisted);
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    let marker: String = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker, "01", "opening v1 must not rewrite its marker");
}

#[test]
fn future_schema_version_is_rejected_without_downgrade() {
    let temp = TempDir::new().unwrap();
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO app_meta(key, value) VALUES ('schema_version', '2');",
        )
        .unwrap();
    drop(connection);

    let error = match SqliteHistoryRepository::open(temp.path()) {
        Ok(_) => panic!("future database version must be rejected"),
        Err(error) => error,
    };

    assert_eq!(error.code(), "unsupportedDatabaseVersion");
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::json!({
            "code": "unsupportedDatabaseVersion",
            "message": "Clipboard database version is not supported"
        })
    );
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    let marker: String = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker, "2");
}

#[test]
fn malformed_schema_version_is_rejected_without_rewrite() {
    let temp = TempDir::new().unwrap();
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO app_meta(key, value) VALUES ('schema_version', 'not-a-number');",
        )
        .unwrap();
    drop(connection);

    let error = match SqliteHistoryRepository::open(temp.path()) {
        Ok(_) => panic!("malformed database version must be rejected"),
        Err(error) => error,
    };

    assert_eq!(error.code(), "unsupportedDatabaseVersion");
}

#[test]
fn delete_succeeds_after_commit_when_image_cleanup_fails() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let resource = repository
        .write_image_resource("delete-cleanup.png", b"resource")
        .unwrap();
    let item = insert(
        &repository,
        image_item(
            "delete-cleanup",
            "delete-cleanup-fp",
            &resource,
            &resource,
            false,
            10,
        ),
    );
    fs::remove_file(&resource).unwrap();
    fs::create_dir(&resource).unwrap();

    repository.delete(&item.id).unwrap();

    assert_eq!(repository.get(&item.id).unwrap_err().code(), "itemNotFound");
    assert!(resource.is_dir());
}

#[test]
fn evicting_upsert_succeeds_after_commit_when_image_cleanup_fails() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let resource = repository
        .write_image_resource("evict-cleanup.png", b"resource")
        .unwrap();
    let victim = insert(
        &repository,
        image_item(
            "evict-cleanup",
            "evict-cleanup-fp",
            &resource,
            &resource,
            false,
            10,
        ),
    );
    fs::remove_file(&resource).unwrap();
    fs::create_dir(&resource).unwrap();
    let incoming = text_item("replacement", "replacement-fp", "replacement", false, 20);

    let persisted = repository
        .upsert(
            incoming.clone(),
            UpsertDecision::Insert {
                evict: Some(victim.id.clone()),
            },
        )
        .unwrap();

    assert_eq!(persisted, incoming);
    assert_eq!(
        repository.get(&victim.id).unwrap_err().code(),
        "itemNotFound"
    );
    assert!(resource.is_dir());
}

#[test]
fn clear_normal_succeeds_after_commit_when_image_cleanup_fails() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let resource = repository
        .write_image_resource("clear-cleanup.png", b"resource")
        .unwrap();
    insert(
        &repository,
        image_item(
            "clear-cleanup",
            "clear-cleanup-fp",
            &resource,
            &resource,
            false,
            10,
        ),
    );
    fs::remove_file(&resource).unwrap();
    fs::create_dir(&resource).unwrap();

    repository.clear_normal().unwrap();

    assert!(all(&repository).is_empty());
    assert!(resource.is_dir());
}

#[cfg(any(windows, unix))]
#[test]
fn repository_keeps_image_operations_on_the_opened_directory_after_path_swap() {
    let app_root = TempDir::new().unwrap();
    let external_root = TempDir::new().unwrap();
    let sentinel = external_root.path().join("sentinel.png");
    fs::write(&sentinel, b"sentinel").unwrap();
    let repository = repository(&app_root);
    let visible_images = app_root.path().join("images");
    let moved_images = app_root.path().join("trusted-images");
    let rename_succeeded = fs::rename(&visible_images, &moved_images).is_ok();
    #[cfg(windows)]
    assert!(
        !rename_succeeded,
        "the trusted Windows directory handle must prevent path replacement"
    );
    #[cfg(unix)]
    assert!(
        rename_succeeded,
        "the Unix test must exercise a live visible-path replacement"
    );

    let (write_result, trusted_resource, external_write_observed, delete_result) =
        if rename_succeeded {
            create_directory_redirect(&visible_images, external_root.path());
            let write_result = repository.write_image_resource("after-swap.png", b"trusted");
            let external_write_observed = external_root.path().join("after-swap.png").exists();
            let trusted_resource = moved_images.join("after-swap.png");
            let delete_result = write_result.as_ref().ok().map(|serialized_path| {
                let item = insert(
                    &repository,
                    image_item(
                        "after-swap",
                        "after-swap-fp",
                        serialized_path,
                        serialized_path,
                        false,
                        10,
                    ),
                );
                repository.delete(&item.id)
            });
            (
                write_result,
                trusted_resource,
                external_write_observed,
                delete_result,
            )
        } else {
            let write_result = repository.write_image_resource("after-swap.png", b"trusted");
            let trusted_resource = visible_images.join("after-swap.png");
            let delete_result = write_result.as_ref().ok().map(|serialized_path| {
                let item = insert(
                    &repository,
                    image_item(
                        "after-swap",
                        "after-swap-fp",
                        serialized_path,
                        serialized_path,
                        false,
                        10,
                    ),
                );
                repository.delete(&item.id)
            });
            (write_result, trusted_resource, false, delete_result)
        };
    let write_succeeded_in_trusted_directory = write_result.is_ok() && !trusted_resource.exists();
    drop(repository);
    if rename_succeeded {
        remove_directory_redirect(&visible_images);
        fs::rename(&moved_images, &visible_images).unwrap();
    }

    assert!(sentinel.exists());
    assert!(
        !external_write_observed,
        "the swapped visible path must never receive resource bytes"
    );
    assert!(
        write_result.is_err() || write_succeeded_in_trusted_directory,
        "resource write must stay on the trusted handle or safely fail"
    );
    if let Some(delete_result) = delete_result {
        delete_result.unwrap();
    }
}

#[test]
fn mismatched_kind_and_payload_is_rejected_before_eviction() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let resource = repository
        .write_image_resource("mismatch-victim.png", b"victim")
        .unwrap();
    let victim = insert(
        &repository,
        image_item(
            "mismatch-victim",
            "mismatch-victim-fp",
            &resource,
            &resource,
            false,
            10,
        ),
    );
    let mut mismatch = text_item("mismatch", "mismatch-fp", "mismatch", false, 20);
    mismatch.payload = ClipboardPayload::Image {
        png_path: resource.to_string_lossy().into_owned(),
        thumbnail_path: resource.to_string_lossy().into_owned(),
        width: 1,
        height: 1,
    };

    let error = repository
        .upsert(
            mismatch,
            UpsertDecision::Insert {
                evict: Some(victim.id.clone()),
            },
        )
        .unwrap_err();

    assert_eq!(error.code(), "invalidClipboardItem");
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::json!({
            "code": "invalidClipboardItem",
            "message": "Clipboard item kind does not match its payload"
        })
    );
    assert_eq!(repository.get(&victim.id).unwrap(), victim);
    assert!(resource.exists());
}

#[test]
fn mismatched_stored_kind_and_payload_is_reported_as_corruption() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let item = insert(
        &repository,
        text_item("corrupt", "corrupt-fp", "corrupt", false, 10),
    );
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET kind = 'image' WHERE id = ?1",
            [&item.id.0],
        )
        .unwrap();

    let error = repository.get(&item.id).unwrap_err();

    assert_eq!(error.code(), "storageError");
}

#[test]
fn startup_cleanup_does_not_misclassify_corrupt_image_payload() {
    let temp = TempDir::new().unwrap();
    let resource = {
        let repository = repository(&temp);
        let resource = repository
            .write_image_resource("corrupt-image.png", b"image")
            .unwrap();
        insert(
            &repository,
            image_item(
                "corrupt-image",
                "corrupt-image-fp",
                &resource,
                &resource,
                false,
                10,
            ),
        );
        resource
    };
    let connection = rusqlite::Connection::open(temp.path().join("history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET kind = 'text' WHERE id = 'corrupt-image'",
            [],
        )
        .unwrap();
    drop(connection);

    let error = match SqliteHistoryRepository::open(temp.path()) {
        Ok(_) => panic!("corrupt stored kind and payload must be rejected"),
        Err(error) => error,
    };

    assert_eq!(error.code(), "storageError");
    assert!(resource.exists());
}

#[test]
fn nonfinite_motion_scale_is_rejected_without_overwriting_settings() {
    let temp = TempDir::new().unwrap();
    let repository = repository(&temp);
    let valid = AppSettings {
        motion_scale: 0.75,
        ..AppSettings::default()
    };
    repository.save_settings(&valid).unwrap();

    for motion_scale in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let invalid = AppSettings {
            motion_scale,
            ..valid.clone()
        };
        let error = repository.save_settings(&invalid).unwrap_err();
        assert_eq!(error.code(), "invalidMotionScale");
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            serde_json::json!({
                "code": "invalidMotionScale",
                "message": "Motion scale must be finite"
            })
        );
        assert_eq!(repository.load_settings().unwrap(), valid);
    }
}

#[test]
fn favorite_changes_persist_across_reopen_and_control_ordering() {
    let temp = TempDir::new().unwrap();
    let older = text_item("older", "older-fp", "older", false, 10);
    let newer = text_item("newer", "newer-fp", "newer", false, 20);
    {
        let repository = repository(&temp);
        insert(&repository, older.clone());
        insert(&repository, newer.clone());
        repository.set_favorite(&older.id, true).unwrap();
    }
    {
        let repository = repository(&temp);
        assert!(repository.get(&older.id).unwrap().is_favorite);
        assert_eq!(all(&repository)[0].id, older.id);
        repository.set_favorite(&older.id, false).unwrap();
    }

    let repository = repository(&temp);
    assert!(!repository.get(&older.id).unwrap().is_favorite);
    assert_eq!(all(&repository)[0].id, newer.id);
}

#[test]
fn same_root_is_exclusive_and_lock_releases_after_drop() {
    let temp = TempDir::new().unwrap();
    let first = repository(&temp);
    let pending = first
        .write_image_resource("pending.png", b"pending")
        .unwrap();
    let sentinel = temp.path().join("images").join("sentinel.keep");
    fs::write(&sentinel, b"sentinel").unwrap();

    let second_error = match SqliteHistoryRepository::open(temp.path()) {
        Ok(second) => {
            drop(second);
            None
        }
        Err(error) => Some(error),
    };

    let error = second_error.expect("the same canonical root must be exclusively locked");
    assert_eq!(error.code(), "repositoryInUse");
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::json!({
            "code": "repositoryInUse",
            "message": "Clipboard repository is already in use"
        })
    );
    assert!(pending.exists());
    assert_eq!(fs::read(&sentinel).unwrap(), b"sentinel");

    drop(first);
    let second = repository(&temp);
    assert!(
        !pending.exists(),
        "the next successful open may reclaim the orphan"
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"sentinel");
    drop(second);
}

#[test]
fn separate_app_roots_can_be_open_concurrently() {
    let first_root = TempDir::new().unwrap();
    let second_root = TempDir::new().unwrap();

    let first = repository(&first_root);
    let second = repository(&second_root);

    assert!(first.write_image_resource("first.png", b"first").is_ok());
    assert!(second.write_image_resource("second.png", b"second").is_ok());
}
