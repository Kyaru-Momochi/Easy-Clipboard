# Easy Clipboard v0.1.0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build, verify, package, and publish a lightweight Windows 10/11 x64 clipboard history application with focus-safe direct paste, 100-item history, text/image/file support, two polished themes, and a single NSIS installer.

**Architecture:** Tauri 2 hosts a Svelte 5 interface while Rust owns all clipboard, persistence, hotkey, tray, focus, and size-policy behavior. Domain services depend on small traits so policy and end-to-end flows can be tested without a real Windows clipboard; Windows-specific adapters are isolated behind `cfg(windows)`.

**Tech Stack:** Rust 1.96, Tauri 2.11, `windows` 0.62, SQLite via `rusqlite` 0.40, Svelte 5.56, TypeScript 7, Vite 8, Vitest 4, Node.js 24 LTS, NSIS, GitHub Actions.

---

## File map

### Root and frontend

- `package.json`: pinned frontend scripts and dependencies.
- `package-lock.json`: reproducible npm dependency graph.
- `index.html`: Vite entry document.
- `vite.config.ts`: Svelte, test, and Tauri development configuration.
- `tsconfig.json`: strict TypeScript configuration.
- `src/main.ts`: mounts the Svelte application.
- `src/App.svelte`: chooses clipboard or settings view from the Tauri window label.
- `src/app.css`: shared tokens, responsive layout, light/dark themes, and reduced-motion rules.
- `src/lib/types.ts`: frontend representations matching Rust serialization.
- `src/lib/api.ts`: all Tauri command calls.
- `src/lib/format.ts`: deterministic size, time, and file-kind formatting.
- `src/lib/stores/history.ts`: history query/filter/selection state.
- `src/lib/stores/settings.ts`: settings load/save/theme state.
- `src/lib/components/AppShell.svelte`: top-level clipboard layout.
- `src/lib/components/ClipboardCard.svelte`: type-aware history card.
- `src/lib/components/ClipboardList.svelte`: virtualized/limited history list and empty state.
- `src/lib/components/FilterBar.svelte`: type filters.
- `src/lib/components/SearchField.svelte`: visual search field driven by native key events.
- `src/lib/components/ContextMenu.svelte`: copy, favorite, delete, and reveal actions.
- `src/lib/components/SettingsView.svelte`: hotkey, limit, theme, motion, startup, and privacy controls.
- `src/lib/test/format.test.ts`: byte and media-type formatting tests.
- `src/lib/test/history-store.test.ts`: search, filter, and keyboard selection tests.
- `src/lib/test/clipboard-ui.test.ts`: history card and context-menu component tests.
- `src/lib/test/settings-ui.test.ts`: settings validation and feedback tests.
- `src/lib/test/theme.test.ts`: system, light, dark, and reduced-motion tests.

### Rust and Tauri

- `src-tauri/Cargo.toml`: Rust dependencies and Windows feature flags.
- `src-tauri/build.rs`: Tauri build hook.
- `src-tauri/tauri.conf.json`: windows, security, bundle, NSIS, and application metadata.
- `src-tauri/capabilities/default.json`: least-privilege permissions.
- `src-tauri/src/main.rs`: binary entry point.
- `src-tauri/src/lib.rs`: application composition root.
- `src-tauri/src/error.rs`: serializable application error.
- `src-tauri/src/state.rs`: shared application state.
- `src-tauri/src/commands.rs`: Tauri command boundary.
- `src-tauri/src/domain/model.rs`: `ClipboardItem`, payload, kind, settings, and query types.
- `src-tauri/src/domain/settings.rs`: defaults and validation.
- `src-tauri/src/domain/history.rs`: deduplication, favorites, and retention policy.
- `src-tauri/src/storage/mod.rs`: repository trait.
- `src-tauri/src/storage/sqlite.rs`: SQLite implementation and resource cleanup.
- `src-tauri/migrations/001_initial.sql`: initial schema and indexes.
- `src-tauri/src/clipboard/mod.rs`: clipboard source/sink traits and capture coordinator.
- `src-tauri/src/clipboard/normalize.rs`: raw format precedence, fingerprints, and size limits.
- `src-tauri/src/clipboard/windows.rs`: Windows clipboard listener, reader, and writer.
- `src-tauri/src/platform/mod.rs`: platform service composition.
- `src-tauri/src/platform/window.rs`: no-activate, topmost, positioning, and theme integration.
- `src-tauri/src/platform/keyboard.rs`: temporary visible-window keyboard routing.
- `src-tauri/src/platform/paste.rs`: foreground-window capture and paste dispatch.
- `src-tauri/src/platform/tray.rs`: tray menu and lifecycle.
- `src-tauri/tests/capture_flow.rs`: fake-backend integration tests.
- `src-tauri/tests/history_store.rs`: real SQLite integration tests.
- `src-tauri/tests/windows_smoke.rs`: ignored, Windows-only adapter smoke tests.

### Delivery

- `.github/workflows/ci.yml`: tests and release-build verification.
- `.github/workflows/release.yml`: tag-triggered Windows NSIS build and GitHub Release upload.
- `scripts/verify.ps1`: one-command lint, test, build, and artifact checks.
- `scripts/smoke-installer.ps1`: silent install, launch, and uninstall smoke test.
- `assets/screenshots/light.png`: verified light-theme screenshot.
- `assets/screenshots/dark.png`: verified dark-theme screenshot.
- `README.md`: feature-rich Chinese documentation.
- `THIRD_PARTY_NOTICES.md`: dependency/license summary.
- `release/Easy-Clipboard_0.1.0_x64-setup.exe`: final installer.
- `release/Easy-Clipboard_0.1.0_x64-setup.exe.sha256`: checksum.

## Task 1: Pin the toolchain and create the Tauri/Svelte skeleton

**Files:**
- Modify: `.gitignore`
- Create: `package.json`
- Create: `index.html`
- Create: `vite.config.ts`
- Create: `tsconfig.json`
- Create: `src/main.ts`
- Create: `src/App.svelte`
- Create: `src-tauri/Cargo.toml`
- Create: `src-tauri/build.rs`
- Create: `src-tauri/tauri.conf.json`
- Create: `src-tauri/capabilities/default.json`
- Create: `src-tauri/src/main.rs`
- Create: `src-tauri/src/lib.rs`

- [ ] **Step 1: Install an isolated Node.js 24 LTS toolchain**

Run from PowerShell:

```powershell
$nodeArchive = 'D:\Easy-Clipboard\.tools\node-v24.18.0-win-x64.zip'
$nodeDirectory = 'D:\Easy-Clipboard\.tools\node-v24.18.0-win-x64'
New-Item -ItemType Directory -Force 'D:\Easy-Clipboard\.tools' | Out-Null
Invoke-WebRequest 'https://nodejs.org/dist/v24.18.0/node-v24.18.0-win-x64.zip' -OutFile $nodeArchive
Expand-Archive -LiteralPath $nodeArchive -DestinationPath 'D:\Easy-Clipboard\.tools' -Force
$env:Path = "$nodeDirectory;$env:Path"
node --version
npm --version
```

Expected: Node prints `v24.18.0`; npm prints a compatible version.

- [ ] **Step 2: Create the pinned package manifest**

Create `package.json` with these scripts and dependency ranges, then run `npm install`:

```json
{
  "name": "easy-clipboard",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "engines": { "node": ">=24.18.0" },
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "check": "svelte-check --tsconfig ./tsconfig.json",
    "test": "vitest run",
    "test:watch": "vitest",
    "tauri": "tauri"
  },
  "dependencies": {
    "@tauri-apps/api": "2.11.1",
    "@tauri-apps/plugin-autostart": "2.5.1",
    "@tauri-apps/plugin-global-shortcut": "2.3.2",
    "@tauri-apps/plugin-opener": "2.5.4",
    "svelte": "5.56.8"
  },
  "devDependencies": {
    "@sveltejs/vite-plugin-svelte": "7.2.0",
    "@tauri-apps/cli": "2.11.4",
    "@testing-library/jest-dom": "7.0.0",
    "@testing-library/svelte": "5.4.2",
    "@types/node": "24.13.3",
    "jsdom": "30.0.0",
    "svelte-check": "4.7.4",
    "typescript": "7.0.2",
    "vite": "8.1.5",
    "vitest": "4.1.10"
  }
}
```

- [ ] **Step 3: Add minimal entry files and Tauri configuration**

Use a single borderless `clipboard` window hidden at startup and a normal `settings` window created on demand. Configure `bundle.targets` to `["nsis"]`, `bundle.windows.nsis.installMode` to `"currentUser"`, `webviewInstallMode.type` to `"downloadBootstrapper"`, product name `Easy Clipboard`, identifier `com.kyaru-momochi.easy-clipboard`, and version `0.1.0`.

The first Rust composition root must contain no product behavior:

```rust
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to run Easy Clipboard");
}
```

- [ ] **Step 4: Verify the empty skeleton**

Run:

```powershell
npm run check
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: all three commands exit `0`; Vite emits `dist/index.html`.

- [ ] **Step 5: Commit and push the skeleton**

```powershell
git add .gitignore package.json package-lock.json index.html vite.config.ts tsconfig.json src src-tauri
git commit -m "chore: scaffold Tauri and Svelte application"
git push origin main
```

## Task 2: Define domain models and validated settings

**Files:**
- Create: `src-tauri/src/domain/mod.rs`
- Create: `src-tauri/src/domain/model.rs`
- Create: `src-tauri/src/domain/settings.rs`
- Create: `src-tauri/src/error.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing settings tests**

Add tests that assert the exact defaults and bounds:

```rust
#[test]
fn settings_default_to_confirmed_product_values() {
    let settings = AppSettings::default();
    assert_eq!(settings.history_limit, 100);
    assert_eq!(settings.favorite_limit, 20);
    assert_eq!(settings.max_item_bytes, 50 * 1024 * 1024);
    assert_eq!(settings.hotkey, "Ctrl+Shift+V");
    assert_eq!(settings.theme, ThemeMode::System);
    assert!(!settings.autostart);
}

#[test]
fn item_limit_accepts_one_to_five_hundred_megabytes() {
    assert!(AppSettings::default().with_limit_mb(1).is_ok());
    assert!(AppSettings::default().with_limit_mb(500).is_ok());
    assert!(AppSettings::default().with_limit_mb(0).is_err());
    assert!(AppSettings::default().with_limit_mb(501).is_err());
}
```

- [ ] **Step 2: Run the test and confirm the red state**

Run: `cargo test --manifest-path src-tauri/Cargo.toml domain::settings -- --nocapture`

Expected: compile failure because `AppSettings` and `ThemeMode` do not exist.

- [ ] **Step 3: Implement serializable models and validation**

Define:

```rust
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardKind { Text, Image, Files }

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeMode { System, Light, Dark }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub history_limit: usize,
    pub favorite_limit: usize,
    pub max_item_bytes: u64,
    pub hotkey: String,
    pub theme: ThemeMode,
    pub motion_scale: f32,
    pub autostart: bool,
    pub clear_on_exit: bool,
}
```

Also define `ClipboardItem`, `ClipboardPayload`, `FileEntry`, `HistoryQuery`, `ItemId`, and an `AppError` enum with serializable `code` and human-readable `message`. Keep payload variants tagged with `type` so TypeScript can discriminate them.

Use these exact shapes:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ItemId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind { Audio, Video, Other }

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub extension: String,
    pub size_bytes: u64,
    pub media_kind: MediaKind,
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClipboardPayload {
    Text { plain: String, html: Option<String>, rtf: Option<Vec<u8>> },
    Image {
        png_path: String,
        thumbnail_path: String,
        width: u32,
        height: u32,
    },
    Files { entries: Vec<FileEntry> },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardItem {
    pub id: ItemId,
    pub kind: ClipboardKind,
    pub payload: ClipboardPayload,
    pub fingerprint: String,
    pub preview: String,
    pub byte_size: u64,
    pub is_favorite: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    pub kind: Option<ClipboardKind>,
    pub search: String,
}
```

- [ ] **Step 4: Run all Rust tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: settings tests pass with no warnings.

- [ ] **Step 5: Commit and push**

```powershell
git add src-tauri/src
git commit -m "feat: define clipboard domain and settings"
git push origin main
```

## Task 3: Implement history deduplication, favorites, and retention

**Files:**
- Create: `src-tauri/src/domain/history.rs`
- Modify: `src-tauri/src/domain/mod.rs`
- Test: `src-tauri/src/domain/history.rs`

- [ ] **Step 1: Write failing policy tests**

Cover four separate behaviors:

```rust
fn item(id: usize, fingerprint: &str, favorite: bool, updated_at_ms: i64) -> ClipboardItem {
    ClipboardItem {
        id: ItemId(id.to_string()),
        kind: ClipboardKind::Text,
        payload: ClipboardPayload::Text {
            plain: fingerprint.to_owned(),
            html: None,
            rtf: None,
        },
        fingerprint: fingerprint.to_owned(),
        preview: fingerprint.to_owned(),
        byte_size: fingerprint.len() as u64,
        is_favorite: favorite,
        created_at_ms: updated_at_ms,
        updated_at_ms,
    }
}

#[test]
fn duplicate_content_moves_to_front_without_growing_history() {
    let current = vec![item(1, "newer", false, 20), item(2, "repeat", false, 10)];
    let decision = HistoryPolicy::new(100, 20).decide_upsert("repeat", &current);
    assert_eq!(decision, UpsertDecision::Touch { existing: ItemId("2".into()) });
}

#[test]
fn adding_item_101_evicts_oldest_non_favorite() {
    let current = (0..100)
        .map(|index| item(index, &format!("fp-{index}"), false, index as i64))
        .collect::<Vec<_>>();
    let decision = HistoryPolicy::new(100, 20).decide_upsert("new", &current);
    assert_eq!(decision, UpsertDecision::Insert { evict: Some(ItemId("0".into())) });
}

#[test]
fn favorites_are_kept_outside_normal_rotation() {
    let mut current = vec![item(500, "favorite", true, 0)];
    current.extend((0..100).map(|index| item(index, &format!("fp-{index}"), false, index as i64)));
    let decision = HistoryPolicy::new(100, 20).decide_upsert("new", &current);
    assert_eq!(decision, UpsertDecision::Insert { evict: Some(ItemId("0".into())) });
}

#[test]
fn favorite_21_is_rejected() {
    let current = (0..20)
        .map(|index| item(index, &format!("fp-{index}"), true, index as i64))
        .collect::<Vec<_>>();
    let error = HistoryPolicy::new(100, 20).can_favorite(&current).unwrap_err();
    assert_eq!(error.code(), "favoriteLimitReached");
}
```

Use fixed UUIDs and timestamps so ordering assertions are deterministic.

- [ ] **Step 2: Verify the policy tests fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml domain::history -- --nocapture`

Expected: compile failure because `HistoryPolicy` does not exist.

- [ ] **Step 3: Implement the pure policy**

Expose this API:

```rust
pub struct HistoryPolicy {
    normal_limit: usize,
    favorite_limit: usize,
}

pub enum UpsertDecision {
    Insert { evict: Option<ItemId> },
    Touch { existing: ItemId },
}

impl HistoryPolicy {
    pub fn decide_upsert(&self, incoming_fingerprint: &str, current: &[ClipboardItem]) -> UpsertDecision;
    pub fn can_favorite(&self, current: &[ClipboardItem]) -> Result<(), AppError>;
}
```

The implementation must never evict a favorite and must count only non-favorites against the 100-item normal limit.

- [ ] **Step 4: Verify the complete Rust suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: all tests pass.

- [ ] **Step 5: Commit and push**

```powershell
git add src-tauri/src/domain
git commit -m "feat: add clipboard history policy"
git push origin main
```

## Task 4: Add transactional SQLite persistence

**Files:**
- Create: `src-tauri/src/storage/mod.rs`
- Create: `src-tauri/src/storage/sqlite.rs`
- Create: `src-tauri/migrations/001_initial.sql`
- Test: `src-tauri/tests/history_store.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing repository integration tests**

Use `tempfile::TempDir` and a real SQLite file. Verify:

```rust
fn repository() -> (tempfile::TempDir, SqliteHistoryRepository) {
    let directory = tempfile::tempdir().unwrap();
    let repository = SqliteHistoryRepository::open(directory.path()).unwrap();
    (directory, repository)
}

#[test]
fn upsert_duplicate_touches_existing_row() {
    let (_directory, repository) = repository();
    let first = item(1, "same", false, 10);
    repository.upsert(first, UpsertDecision::Insert { evict: None }).unwrap();
    let touched = item(1, "same", false, 20);
    repository.upsert(
        touched,
        UpsertDecision::Touch { existing: ItemId("1".into()) },
    ).unwrap();
    let rows = repository.list(&HistoryQuery::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].updated_at_ms, 20);
}

#[test]
fn transaction_inserts_and_evicts_atomically() {
    let (_directory, repository) = repository();
    repository.upsert(item(1, "one", false, 1), UpsertDecision::Insert { evict: None }).unwrap();
    repository.upsert(item(2, "two", false, 2), UpsertDecision::Insert { evict: None }).unwrap();
    repository.upsert(
        item(3, "three", false, 3),
        UpsertDecision::Insert { evict: Some(ItemId("1".into())) },
    ).unwrap();
    let ids = repository.list(&HistoryQuery::default()).unwrap()
        .into_iter().map(|row| row.id).collect::<Vec<_>>();
    assert_eq!(ids, vec![ItemId("3".into()), ItemId("2".into())]);
}

#[test]
fn deleting_image_removes_unreferenced_resource() {
    let (directory, repository) = repository();
    let png = directory.path().join("images").join("image-1.png");
    std::fs::create_dir_all(png.parent().unwrap()).unwrap();
    std::fs::write(&png, [137, 80, 78, 71]).unwrap();
    let mut image = item(1, "image", false, 1);
    image.kind = ClipboardKind::Image;
    image.payload = ClipboardPayload::Image {
        png_path: png.to_string_lossy().into_owned(),
        thumbnail_path: png.to_string_lossy().into_owned(),
        width: 1,
        height: 1,
    };
    repository.upsert(image, UpsertDecision::Insert { evict: None }).unwrap();
    repository.delete(&ItemId("1".into())).unwrap();
    assert!(!png.exists());
}

#[test]
fn query_filters_kind_and_case_insensitive_text() {
    let (_directory, repository) = repository();
    repository.upsert(item(1, "Hello World", false, 1), UpsertDecision::Insert { evict: None }).unwrap();
    repository.upsert(item(2, "Other", false, 2), UpsertDecision::Insert { evict: None }).unwrap();
    let query = HistoryQuery { kind: Some(ClipboardKind::Text), search: "hello".into() };
    let ids = repository.list(&query).unwrap().into_iter().map(|row| row.id).collect::<Vec<_>>();
    assert_eq!(ids, vec![ItemId("1".into())]);
}
```

- [ ] **Step 2: Verify red state**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test history_store -- --nocapture`

Expected: compile failure because `SqliteHistoryRepository` does not exist.

- [ ] **Step 3: Create the schema and repository trait**

Schema tables:

```sql
CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1), json TEXT NOT NULL);
CREATE TABLE clipboard_items (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('text','image','files')),
  fingerprint TEXT NOT NULL UNIQUE,
  payload_json TEXT NOT NULL,
  preview TEXT NOT NULL,
  byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
  is_favorite INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0,1)),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE INDEX clipboard_items_order ON clipboard_items(is_favorite DESC, updated_at_ms DESC);
CREATE INDEX clipboard_items_kind ON clipboard_items(kind);
```

Repository contract:

```rust
pub trait HistoryRepository: Send + Sync {
    fn list(&self, query: &HistoryQuery) -> Result<Vec<ClipboardItem>, AppError>;
    fn get(&self, id: &ItemId) -> Result<ClipboardItem, AppError>;
    fn upsert(&self, item: ClipboardItem, decision: UpsertDecision) -> Result<ClipboardItem, AppError>;
    fn set_favorite(&self, id: &ItemId, value: bool) -> Result<(), AppError>;
    fn delete(&self, id: &ItemId) -> Result<(), AppError>;
    fn clear_normal(&self) -> Result<(), AppError>;
    fn load_settings(&self) -> Result<AppSettings, AppError>;
    fn save_settings(&self, settings: &AppSettings) -> Result<(), AppError>;
}
```

- [ ] **Step 4: Implement migrations, transactions, and atomic image resources**

Open SQLite with WAL, foreign keys, and a 2-second busy timeout. Store PNG files under `%LOCALAPPDATA%\Easy Clipboard\images`; write to `.tmp`, flush, then rename. On startup delete `.tmp` files and PNGs not referenced by any row.

- [ ] **Step 5: Verify storage tests and commit**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --test history_store
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: both commands pass.

```powershell
git add src-tauri/migrations src-tauri/src/storage src-tauri/tests/history_store.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: persist clipboard history with SQLite"
git push origin main
```

## Task 5: Normalize clipboard formats and enforce size limits

**Files:**
- Create: `src-tauri/src/clipboard/mod.rs`
- Create: `src-tauri/src/clipboard/normalize.rs`
- Test: `src-tauri/src/clipboard/normalize.rs`

- [ ] **Step 1: Write failing normalizer tests**

Define test fixtures for `RawClipboardSnapshot` and verify:

```rust
fn file_entry() -> FileEntry {
    FileEntry {
        path: r"C:\media\mix.flac".into(),
        name: "mix.flac".into(),
        extension: "flac".into(),
        size_bytes: 1024,
        media_kind: MediaKind::Audio,
        available: true,
    }
}

#[test]
fn files_win_over_bitmap_and_text_formats() {
    let raw = RawClipboardSnapshot {
        files: vec![file_entry()],
        png: Some(vec![1, 2, 3]),
        dib: None,
        plain_text: Some("text".into()),
        html: None,
        rtf: None,
    };
    let outcome = normalize(raw, &AppSettings::default(), 1).unwrap();
    assert!(matches!(outcome, NormalizeOutcome::Accepted(ClipboardItem {
        kind: ClipboardKind::Files, ..
    })));
}

#[test]
fn image_wins_over_text_when_no_files_exist() {
    let raw = RawClipboardSnapshot {
        files: vec![],
        png: Some(vec![137, 80, 78, 71]),
        dib: None,
        plain_text: Some("text".into()),
        html: None,
        rtf: None,
    };
    let outcome = normalize(raw, &AppSettings::default(), 1).unwrap();
    assert!(matches!(outcome, NormalizeOutcome::Accepted(ClipboardItem {
        kind: ClipboardKind::Image, ..
    })));
}

#[test]
fn rich_text_keeps_plain_html_and_rtf() {
    let raw = RawClipboardSnapshot {
        files: vec![],
        png: None,
        dib: None,
        plain_text: Some("Hello".into()),
        html: Some("<b>Hello</b>".into()),
        rtf: Some(br"{\rtf1 Hello}".to_vec()),
    };
    let outcome = normalize(raw, &AppSettings::default(), 1).unwrap();
    let NormalizeOutcome::Accepted(item) = outcome else { panic!("text should be accepted") };
    assert_eq!(item.payload, ClipboardPayload::Text {
        plain: "Hello".into(),
        html: Some("<b>Hello</b>".into()),
        rtf: Some(br"{\rtf1 Hello}".to_vec()),
    });
}

#[test]
fn payload_over_configured_limit_is_ignored() {
    let mut settings = AppSettings::default();
    settings.max_item_bytes = 50 * 1024 * 1024;
    let raw = RawClipboardSnapshot {
        files: vec![FileEntry { size_bytes: 51 * 1024 * 1024, ..file_entry() }],
        png: None,
        dib: None,
        plain_text: None,
        html: None,
        rtf: None,
    };
    assert!(matches!(
        normalize(raw, &settings, 1).unwrap(),
        NormalizeOutcome::IgnoredOversize { .. }
    ));
}

#[test]
fn duplicate_payloads_have_identical_sha256_fingerprint() {
    let raw = || RawClipboardSnapshot {
        files: vec![],
        png: None,
        dib: None,
        plain_text: Some("stable".into()),
        html: None,
        rtf: None,
    };
    let NormalizeOutcome::Accepted(first) = normalize(raw(), &AppSettings::default(), 1).unwrap() else { panic!() };
    let NormalizeOutcome::Accepted(second) = normalize(raw(), &AppSettings::default(), 2).unwrap() else { panic!() };
    assert_eq!(first.fingerprint, second.fingerprint);
}
```

- [ ] **Step 2: Verify red state**

Run: `cargo test --manifest-path src-tauri/Cargo.toml clipboard::normalize -- --nocapture`

Expected: compile failure because `normalize` and `RawClipboardSnapshot` do not exist.

- [ ] **Step 3: Implement normalization**

Use:

```rust
pub struct RawClipboardSnapshot {
    pub files: Vec<FileEntry>,
    pub png: Option<Vec<u8>>,
    pub dib: Option<Vec<u8>>,
    pub plain_text: Option<String>,
    pub html: Option<String>,
    pub rtf: Option<Vec<u8>>,
}

pub enum NormalizeOutcome {
    Accepted(ClipboardItem),
    IgnoredEmpty,
    IgnoredOversize { measured_bytes: u64, limit_bytes: u64 },
}

pub fn normalize(
    raw: RawClipboardSnapshot,
    settings: &AppSettings,
    now_ms: i64,
) -> Result<NormalizeOutcome, AppError>;
```

Hash canonical payload bytes with SHA-256. For directories, use a capped walker that stops immediately once the configured limit is exceeded.

- [ ] **Step 4: Verify all normalizer and domain tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: all tests pass.

- [ ] **Step 5: Commit and push**

```powershell
git add src-tauri/src/clipboard src-tauri/Cargo.toml
git commit -m "feat: normalize clipboard formats and limits"
git push origin main
```

## Task 6: Build the testable capture-to-paste coordinator

**Files:**
- Modify: `src-tauri/src/clipboard/mod.rs`
- Create: `src-tauri/tests/capture_flow.rs`
- Modify: `src-tauri/src/state.rs`

- [ ] **Step 1: Write failing fake-backend flow tests**

Create in-memory implementations of:

```rust
pub trait ClipboardBackend: Send + Sync {
    fn read(&self) -> Result<RawClipboardSnapshot, AppError>;
    fn write(&self, payload: &ClipboardPayload) -> Result<(), AppError>;
}

pub trait PasteTarget: Send + Sync {
    fn remember_foreground(&self) -> Result<(), AppError>;
    fn paste_to_remembered(&self) -> Result<(), AppError>;
}
```

Test accepted capture, oversize ignore, duplicate touch, successful direct paste, and missing-file rejection.

- [ ] **Step 2: Verify red state**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test capture_flow -- --nocapture`

Expected: compile failure because `CaptureCoordinator` does not exist.

- [ ] **Step 3: Implement the coordinator**

Expose:

```rust
pub struct CaptureCoordinator<R, C, P> {
    repository: Arc<R>,
    clipboard: Arc<C>,
    paste_target: Arc<P>,
    settings: Arc<RwLock<AppSettings>>,
}

impl<R, C, P> CaptureCoordinator<R, C, P>
where
    R: HistoryRepository,
    C: ClipboardBackend,
    P: PasteTarget,
{
    pub fn capture_now(&self) -> Result<CaptureResult, AppError>;
    pub fn paste_item(&self, id: &ItemId) -> Result<(), AppError>;
}
```

Before writing an item during direct paste, mark the internal clipboard sequence so the listener does not immediately re-capture the app’s own write.

- [ ] **Step 4: Verify integration and unit tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: all tests pass.

- [ ] **Step 5: Commit and push**

```powershell
git add src-tauri/src/clipboard src-tauri/src/state.rs src-tauri/tests/capture_flow.rs
git commit -m "feat: coordinate capture and direct paste"
git push origin main
```

## Task 7: Add Windows clipboard adapters

**Files:**
- Create: `src-tauri/src/clipboard/windows.rs`
- Create: `src-tauri/src/platform/mod.rs`
- Test: `src-tauri/tests/windows_smoke.rs`
- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1: Add ignored Windows adapter smoke tests**

Tests must be marked `#[ignore = "mutates the interactive Windows clipboard"]` and cover plain text round-trip, PNG round-trip, and `CF_HDROP` file-list round-trip.

- [ ] **Step 2: Verify the adapter module is missing**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test windows_smoke --no-run`

Expected: compile failure because `WindowsClipboard` does not exist.

- [ ] **Step 3: Implement safe Win32 clipboard ownership**

Use `windows` 0.62 features for `Win32_Foundation`, `Win32_System_DataExchange`, `Win32_System_Memory`, `Win32_UI_Shell`, and `Win32_UI_WindowsAndMessaging`.

The adapter must:

- retry `OpenClipboard` at 10, 20, 40, and 80 ms;
- always call `CloseClipboard` through an RAII guard;
- copy bytes before closing;
- decode `CF_UNICODETEXT`, registered HTML/RTF formats, `CF_DIBV5`, PNG, and `CF_HDROP`;
- allocate movable global memory for writes and transfer ownership only after `SetClipboardData` succeeds;
- expose a hidden message-only window registered with `AddClipboardFormatListener`.

- [ ] **Step 4: Compile and run non-mutating tests**

Run:

```powershell
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: both pass; ignored interactive tests are listed but not executed.

- [ ] **Step 5: Commit and push**

```powershell
git add src-tauri/Cargo.toml src-tauri/src/clipboard/windows.rs src-tauri/src/platform src-tauri/tests/windows_smoke.rs
git commit -m "feat: integrate Windows clipboard formats"
git push origin main
```

## Task 8: Implement focus-safe overlay, keyboard routing, and paste dispatch

**Files:**
- Create: `src-tauri/src/platform/window.rs`
- Create: `src-tauri/src/platform/keyboard.rs`
- Create: `src-tauri/src/platform/paste.rs`
- Modify: `src-tauri/src/platform/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write unit tests for pure key routing and placement**

Test:

```rust
#[test]
fn overlay_is_centered_and_clamped_to_work_area() {
    let placement = OverlayPlacement::centered(
        Rect { left: 0, top: 0, right: 1920, bottom: 1040 },
        Size { width: 460, height: 620 },
    );
    assert_eq!(placement, OverlayPlacement { x: 730, y: 210, width: 460, height: 620 });
}

#[test]
fn visible_keyboard_router_maps_text_arrows_enter_and_escape() {
    assert_eq!(route_key(KeyInput::Character('A')), Some(KeyAction::SearchText("A".into())));
    assert_eq!(route_key(KeyInput::ArrowDown), Some(KeyAction::Move(1)));
    assert_eq!(route_key(KeyInput::Enter), Some(KeyAction::Paste));
    assert_eq!(route_key(KeyInput::Escape), Some(KeyAction::Hide));
}

#[test]
fn router_ignores_modifier_shortcuts_except_supported_actions() {
    assert_eq!(
        route_key(KeyInput::Modified { control: true, shift: false, alt: false, key: 'C' }),
        None,
    );
}
```

- [ ] **Step 2: Verify tests fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml platform -- --nocapture`

Expected: compile failure because `OverlayPlacement` and `route_key` do not exist.

- [ ] **Step 3: Implement the no-activate overlay**

After Tauri creates the webview HWND, add `WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW`, return `MA_NOACTIVATE` for `WM_MOUSEACTIVATE`, and display through `ShowWindow(hwnd, SW_SHOWNOACTIVATE)`. Center within `MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST)` work area and clamp user-resized dimensions to `360×420` through `720×820`.

- [ ] **Step 4: Implement scoped keyboard routing and paste dispatch**

Install `WH_KEYBOARD_LL` only while the overlay is visible. Route printable characters, Backspace, arrows, Enter, and Escape to Tauri events; return `CallNextHookEx` for every other key. Remove the hook on hide, exit, and panic cleanup.

Remember the original `GetForegroundWindow()` handle. For paste, write the selected payload, call `SetForegroundWindow` only if the original handle is no longer foreground, then send Ctrl+V with `SendInput`.

- [ ] **Step 5: Verify, commit, and push**

Run:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: all pass.

```powershell
git add src-tauri/src/platform src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: add focus-safe overlay and paste routing"
git push origin main
```

## Task 9: Add Tauri commands, tray, hotkey, and autostart

**Files:**
- Create: `src-tauri/src/commands.rs`
- Create: `src-tauri/src/platform/tray.rs`
- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/capabilities/default.json`

- [ ] **Step 1: Write failing command-service tests**

Test list/filter, paste, copy-only, favorite, delete, clear, setting validation, and hotkey rollback when a requested shortcut cannot be registered.

- [ ] **Step 2: Verify red state**

Run: `cargo test --manifest-path src-tauri/Cargo.toml commands -- --nocapture`

Expected: compile failure because command service functions do not exist.

- [ ] **Step 3: Implement command boundary**

Register these exact commands once in `generate_handler!`:

```rust
list_history,
paste_item,
copy_item,
set_favorite,
delete_item,
clear_history,
get_settings,
save_settings,
open_settings,
reveal_file,
exit_app
```

Return `Result<T, AppError>` from every fallible command. Frontend capability permissions must include only core event/window operations and the autostart/global-shortcut/opener permissions actually called by the webview.

- [ ] **Step 4: Implement native lifecycle**

Register the configured hotkey in Rust. On press, remember foreground, show overlay without activation, and enable key routing. Tray items are `显示`, `暂停监听`, `设置`, and `退出`. Close requests hide the clipboard window; the explicit exit command shuts down after optional history clearing.

- [ ] **Step 5: Verify, commit, and push**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Expected: all tests pass and clippy reports no warnings.

```powershell
git add src-tauri/src src-tauri/capabilities src-tauri/Cargo.toml
git commit -m "feat: wire commands hotkey tray and startup"
git push origin main
```

## Task 10: Create typed frontend state and format utilities

**Files:**
- Create: `src/lib/types.ts`
- Create: `src/lib/api.ts`
- Create: `src/lib/format.ts`
- Create: `src/lib/stores/history.ts`
- Create: `src/lib/stores/settings.ts`
- Test: `src/lib/test/format.test.ts`
- Test: `src/lib/test/history-store.test.ts`

- [ ] **Step 1: Write failing TypeScript tests**

Verify:

```ts
expect(formatBytes(0)).toBe('0 B');
expect(formatBytes(1_572_864)).toBe('1.5 MB');
expect(classifyExtension('mix.FLAC')).toBe('audio');
expect(filterItems(fixtures, { kind: 'image', search: '' })).toHaveLength(1);
expect(filterItems(fixtures, { kind: 'all', search: 'hello' }).map(i => i.id)).toEqual(['text-1']);
```

- [ ] **Step 2: Verify red state**

Run: `npm test -- src/lib/test/format.test.ts src/lib/test/history-store.test.ts`

Expected: failure because the imported modules do not exist.

- [ ] **Step 3: Implement types, API, and pure state functions**

Mirror Rust camel-case JSON exactly. Keep all `invoke` calls in `api.ts`. Export pure `filterItems`, `moveSelection`, `formatBytes`, `formatRelativeTime`, and `classifyExtension` functions; wrap them in Svelte stores only after pure tests pass.

- [ ] **Step 4: Verify frontend tests and static checks**

Run:

```powershell
npm test
npm run check
```

Expected: all tests and Svelte checks pass.

- [ ] **Step 5: Commit and push**

```powershell
git add src/lib
git commit -m "feat: add typed clipboard frontend state"
git push origin main
```

## Task 11: Build the responsive clipboard interface

**Files:**
- Modify: `src/App.svelte`
- Create: `src/lib/components/AppShell.svelte`
- Create: `src/lib/components/SearchField.svelte`
- Create: `src/lib/components/FilterBar.svelte`
- Create: `src/lib/components/ClipboardList.svelte`
- Create: `src/lib/components/ClipboardCard.svelte`
- Create: `src/lib/components/ContextMenu.svelte`
- Test: `src/lib/test/clipboard-ui.test.ts`

- [ ] **Step 1: Write failing component tests**

Render typed fixtures and assert:

- text content is visible;
- image dimensions and byte size are visible;
- audio/video files use localized media labels;
- unavailable files are disabled;
- selecting a filter hides nonmatching cards;
- context menu exposes the four confirmed actions.

- [ ] **Step 2: Verify red state**

Run: `npm test -- src/lib/test/clipboard-ui.test.ts`

Expected: failure because components do not exist.

- [ ] **Step 3: Implement semantic, keyboard-readable components**

Use buttons for cards, `aria-selected` for active keyboard selection, `role="menu"` for context actions, lazy image loading, and a maximum of 100 rendered normal cards plus 20 favorites. Subscribe to native `search-input`, `selection-move`, `selection-paste`, and `overlay-hide` events.

- [ ] **Step 4: Verify interface tests and production build**

Run:

```powershell
npm test
npm run check
npm run build
```

Expected: all pass and `dist` is produced.

- [ ] **Step 5: Commit and push**

```powershell
git add src
git commit -m "feat: build responsive clipboard interface"
git push origin main
```

## Task 12: Add the Aurora light theme, Ink dark theme, settings, and motion

**Files:**
- Create: `src/app.css`
- Create: `src/lib/components/SettingsView.svelte`
- Modify: `src/App.svelte`
- Modify: `src/lib/stores/settings.ts`
- Test: `src/lib/test/settings-ui.test.ts`
- Test: `src/lib/test/theme.test.ts`

- [ ] **Step 1: Write failing theme and settings tests**

Assert system/light/dark resolution, 1 and 500 MB acceptance, 0 and 501 MB rejection, hotkey error rendering, autostart toggle, clear-history confirmation, and reduced-motion class behavior.

- [ ] **Step 2: Verify red state**

Run: `npm test -- src/lib/test/settings-ui.test.ts src/lib/test/theme.test.ts`

Expected: failure because settings view and theme resolver do not exist.

- [ ] **Step 3: Implement visual tokens and responsive rules**

Define shared variables for radius, spacing, shadows, typography, and 160–240 ms motion. Light mode uses cool translucent surfaces and Aurora purple/teal accents; dark mode uses near-black surfaces, fine borders, and violet focus. Apply `backdrop-filter` only where supported and provide an opaque fallback. At widths below 400 px reduce padding and hide secondary metadata. Honor `prefers-reduced-motion: reduce`.

- [ ] **Step 4: Implement settings behavior**

Open settings in a normal activated Tauri window. Save only validated values. If hotkey registration fails, restore the previous hotkey and show the native error. Theme changes update both windows in about 200 ms.

- [ ] **Step 5: Verify, commit, and push**

Run:

```powershell
npm test
npm run check
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: all pass.

```powershell
git add src
git commit -m "feat: add polished themes and settings"
git push origin main
```

## Task 13: Add full verification scripts and Windows manual harness

**Files:**
- Create: `scripts/verify.ps1`
- Create: `scripts/smoke-installer.ps1`
- Modify: `src-tauri/tests/windows_smoke.rs`
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write verification script assertions**

`scripts/verify.ps1` must stop on the first failure and run:

```powershell
$ErrorActionPreference = 'Stop'
npm ci
npm test
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
npm run tauri build -- --bundles nsis
```

After the build, assert exactly one `*-setup.exe` exists under `src-tauri\target\release\bundle\nsis`.

- [ ] **Step 2: Add Windows manual smoke coverage**

Run ignored tests one at a time on a disposable clipboard, then manually verify:

1. copy plain and rich text into a test editor;
2. copy a PNG from Paint;
3. copy an audio file, a video file, and a folder in Explorer;
4. confirm a 51 MB item is ignored at the default limit;
5. press `Ctrl+Shift+V`, click each stored type, and confirm the original target retains focus and receives paste;
6. resize to minimum and maximum dimensions;
7. switch system, light, and dark themes;
8. pause/resume from tray;
9. restart and verify persisted history.

- [ ] **Step 3: Create Windows CI**

On `windows-2025`, install Node 24 and stable Rust, run `npm ci`, the frontend suite, Rust fmt/clippy/tests, and `npm run tauri build -- --bundles nsis`. Upload the NSIS installer as a workflow artifact.

- [ ] **Step 4: Run the complete verification command**

Run: `powershell -ExecutionPolicy Bypass -File scripts/verify.ps1`

Expected: exit `0`, all tests pass, and one installer is found.

- [ ] **Step 5: Commit and push**

```powershell
git add scripts src-tauri/tests .github/workflows/ci.yml
git commit -m "test: add complete Windows verification"
git push origin main
```

## Task 14: Package, smoke-test, document, and publish v0.1.0

**Files:**
- Create: `.github/workflows/release.yml`
- Create: `README.md`
- Create: `THIRD_PARTY_NOTICES.md`
- Create: `assets/screenshots/light.png`
- Create: `assets/screenshots/dark.png`
- Create: `release/Easy-Clipboard_0.1.0_x64-setup.exe`
- Create: `release/Easy-Clipboard_0.1.0_x64-setup.exe.sha256`

- [ ] **Step 1: Build release artifacts from a clean dependency install**

Run:

```powershell
npm ci
powershell -ExecutionPolicy Bypass -File scripts/verify.ps1
Copy-Item 'src-tauri\target\release\bundle\nsis\*-setup.exe' 'release\Easy-Clipboard_0.1.0_x64-setup.exe'
$hash = (Get-FileHash 'release\Easy-Clipboard_0.1.0_x64-setup.exe' -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  Easy-Clipboard_0.1.0_x64-setup.exe" | Set-Content 'release\Easy-Clipboard_0.1.0_x64-setup.exe.sha256'
```

Expected: installer and checksum exist and are non-empty.

- [ ] **Step 2: Smoke-test the installer**

`scripts/smoke-installer.ps1` must:

- copy the installer to a temporary directory;
- run `Easy-Clipboard_0.1.0_x64-setup.exe /S`;
- wait for the installed executable;
- launch with an isolated `%LOCALAPPDATA%`;
- verify the process remains alive for five seconds;
- close the process;
- run the registered uninstaller with `/S`;
- verify installed binaries are removed.

Run: `powershell -ExecutionPolicy Bypass -File scripts/smoke-installer.ps1`

Expected: script prints `Installer smoke test passed` and exits `0`.

- [ ] **Step 3: Capture actual UI screenshots**

Launch the release build with deterministic sample fixtures, capture the clipboard window once in Aurora light and once in Ink dark, and save PNGs at `1440×900` canvas size under `assets/screenshots`. Inspect both images for clipping, missing glyphs, contrast, and consistent spacing.

- [ ] **Step 4: Write the release documentation**

README must contain:

- product banner and both actual screenshots;
- features and supported formats;
- install and uninstall instructions;
- default shortcuts and mouse actions;
- 50 MB default and 500 MB maximum;
- focus-safe paste explanation;
- privacy and local data path;
- Windows SmartScreen unsigned-publisher warning;
- development prerequisites and commands;
- architecture overview;
- test and build commands;
- roadmap and contribution guidance;
- SHA-256 verification command;
- license and third-party notice links.

- [ ] **Step 5: Create tag-driven release workflow**

On tags matching `v*`, build on `windows-2025`, rename the installer to the exact release filename, create its checksum, then run the runner-provided GitHub CLI with `GH_TOKEN: ${{ github.token }}`:

```powershell
gh release create $env:GITHUB_REF_NAME `
  release/Easy-Clipboard_0.1.0_x64-setup.exe `
  release/Easy-Clipboard_0.1.0_x64-setup.exe.sha256 `
  --title "Easy Clipboard v0.1.0" `
  --generate-notes
```

- [ ] **Step 6: Perform final verification**

Run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/verify.ps1
powershell -ExecutionPolicy Bypass -File scripts/smoke-installer.ps1
git diff --check
git status -sb
Get-FileHash 'release\Easy-Clipboard_0.1.0_x64-setup.exe' -Algorithm SHA256
```

Expected: both scripts exit `0`; diff check is clean; only intended release/documentation files are untracked or modified; the hash matches the checksum file.

- [ ] **Step 7: Commit, push, tag, and publish**

```powershell
git add README.md THIRD_PARTY_NOTICES.md assets .github/workflows/release.yml release
git commit -m "docs: package and document Easy Clipboard v0.1.0"
git push origin main
git tag -a v0.1.0 -m "Easy Clipboard v0.1.0"
git push origin v0.1.0
gh release create v0.1.0 release/Easy-Clipboard_0.1.0_x64-setup.exe release/Easy-Clipboard_0.1.0_x64-setup.exe.sha256 --title "Easy Clipboard v0.1.0" --notes-from-tag
```

Expected: GitHub Release `v0.1.0` contains the installer and checksum, and the local `release` directory contains identical files.

## Plan self-review record

- Spec coverage: every requirement in sections 2–10 of the design is mapped to Tasks 2–14.
- Requirement traceability: 100 normal items and 20 favorites map to Tasks 3–4; the 1–500 MB range with 50 MB default maps to Tasks 2 and 5; `Ctrl + Shift + V` maps to Tasks 1, 8, and 9; no-activate（无激活）direct paste maps to Tasks 6 and 8; Aurora light and Ink dark themes map to Task 12; NSIS, SmartScreen documentation, checksums, and GitHub Release map to Tasks 13–14.
- Boundaries: domain, storage, clipboard, Windows platform, Tauri commands, UI, and delivery have isolated files and explicit interfaces.
- TDD order: each behavior task begins with a failing unit, integration, or component test and records the expected red state before implementation.
- Type consistency: Rust payloads serialize in camel case and match the TypeScript discriminated unions introduced in Task 10.
- Release consistency: version `0.1.0`, tag `v0.1.0`, installer name, checksum name, and README commands use the same identifiers.
- Repository policy: each verified milestone is committed and immediately pushed to `origin/main`, matching the requested step-by-step GitHub history.
