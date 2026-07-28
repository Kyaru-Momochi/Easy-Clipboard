#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ItemId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardKind {
    Text,
    Image,
    Files,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Audio,
    Video,
    Other,
}

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
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClipboardPayload {
    Text {
        plain: String,
        html: Option<String>,
        rtf: Option<Vec<u8>>,
    },
    Image {
        png_path: String,
        thumbnail_path: String,
        width: u32,
        height: u32,
    },
    Files {
        entries: Vec<FileEntry>,
    },
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn item_id_serializes_as_a_scalar_and_round_trips() {
        let id = ItemId("item-1".into());

        let json = serde_json::to_value(&id).unwrap();

        assert_eq!(json, json!("item-1"));
        assert_eq!(serde_json::from_value::<ItemId>(json).unwrap(), id);
    }

    #[test]
    fn domain_enums_use_exact_lower_camel_case_spellings() {
        let clipboard_kinds = [
            (ClipboardKind::Text, json!("text")),
            (ClipboardKind::Image, json!("image")),
            (ClipboardKind::Files, json!("files")),
        ];
        for (value, expected_json) in clipboard_kinds {
            assert_eq!(serde_json::to_value(&value).unwrap(), expected_json);
            assert_eq!(
                serde_json::from_value::<ClipboardKind>(expected_json).unwrap(),
                value
            );
        }

        let theme_modes = [
            (ThemeMode::System, json!("system")),
            (ThemeMode::Light, json!("light")),
            (ThemeMode::Dark, json!("dark")),
        ];
        for (value, expected_json) in theme_modes {
            assert_eq!(serde_json::to_value(&value).unwrap(), expected_json);
            assert_eq!(
                serde_json::from_value::<ThemeMode>(expected_json).unwrap(),
                value
            );
        }

        let media_kinds = [
            (MediaKind::Audio, json!("audio")),
            (MediaKind::Video, json!("video")),
            (MediaKind::Other, json!("other")),
        ];
        for (value, expected_json) in media_kinds {
            assert_eq!(serde_json::to_value(&value).unwrap(), expected_json);
            assert_eq!(
                serde_json::from_value::<MediaKind>(expected_json).unwrap(),
                value
            );
        }
    }

    #[test]
    fn file_entry_uses_complete_camel_case_shape_and_round_trips() {
        let entry = FileEntry {
            path: r"C:\media\song.mp3".into(),
            name: "song.mp3".into(),
            extension: "mp3".into(),
            size_bytes: 1_024,
            media_kind: MediaKind::Audio,
            available: true,
        };
        let expected_json = json!({
            "path": r"C:\media\song.mp3",
            "name": "song.mp3",
            "extension": "mp3",
            "sizeBytes": 1_024,
            "mediaKind": "audio",
            "available": true
        });

        assert_eq!(serde_json::to_value(&entry).unwrap(), expected_json);
        assert_eq!(
            serde_json::from_value::<FileEntry>(expected_json).unwrap(),
            entry
        );
    }

    #[test]
    fn text_payload_round_trips_with_optional_formats_present() {
        let payload = ClipboardPayload::Text {
            plain: "Hello".into(),
            html: Some("<p>Hello</p>".into()),
            rtf: Some(vec![1, 2, 255]),
        };
        let expected_json = json!({
            "type": "text",
            "plain": "Hello",
            "html": "<p>Hello</p>",
            "rtf": [1, 2, 255]
        });

        assert_eq!(serde_json::to_value(&payload).unwrap(), expected_json);
        assert_eq!(
            serde_json::from_value::<ClipboardPayload>(expected_json).unwrap(),
            payload
        );
    }

    #[test]
    fn text_payload_round_trips_with_optional_formats_absent() {
        let payload = ClipboardPayload::Text {
            plain: "Plain only".into(),
            html: None,
            rtf: None,
        };
        let expected_json = json!({
            "type": "text",
            "plain": "Plain only",
            "html": null,
            "rtf": null
        });

        assert_eq!(serde_json::to_value(&payload).unwrap(), expected_json);
        assert_eq!(
            serde_json::from_value::<ClipboardPayload>(expected_json).unwrap(),
            payload
        );
    }

    #[test]
    fn image_payload_uses_complete_camel_case_shape_and_round_trips() {
        let payload = ClipboardPayload::Image {
            png_path: "image.png".into(),
            thumbnail_path: "thumbnail.png".into(),
            width: 320,
            height: 180,
        };
        let expected_json = json!({
            "type": "image",
            "pngPath": "image.png",
            "thumbnailPath": "thumbnail.png",
            "width": 320,
            "height": 180
        });

        assert_eq!(serde_json::to_value(&payload).unwrap(), expected_json);
        assert_eq!(
            serde_json::from_value::<ClipboardPayload>(expected_json).unwrap(),
            payload
        );
    }

    #[test]
    fn files_payload_uses_complete_entry_shape_and_round_trips() {
        let payload = ClipboardPayload::Files {
            entries: vec![FileEntry {
                path: r"C:\docs\notes.txt".into(),
                name: "notes.txt".into(),
                extension: "txt".into(),
                size_bytes: 42,
                media_kind: MediaKind::Other,
                available: false,
            }],
        };
        let expected_json = json!({
            "type": "files",
            "entries": [{
                "path": r"C:\docs\notes.txt",
                "name": "notes.txt",
                "extension": "txt",
                "sizeBytes": 42,
                "mediaKind": "other",
                "available": false
            }]
        });

        assert_eq!(serde_json::to_value(&payload).unwrap(), expected_json);
        assert_eq!(
            serde_json::from_value::<ClipboardPayload>(expected_json).unwrap(),
            payload
        );
    }

    #[test]
    fn clipboard_item_uses_complete_camel_case_shape_and_round_trips() {
        let item = ClipboardItem {
            id: ItemId("item-1".into()),
            kind: ClipboardKind::Text,
            payload: ClipboardPayload::Text {
                plain: "Hello".into(),
                html: None,
                rtf: None,
            },
            fingerprint: "fingerprint-1".into(),
            preview: "Hello".into(),
            byte_size: 5,
            is_favorite: true,
            created_at_ms: 1_000,
            updated_at_ms: 2_000,
        };
        let expected_json = json!({
            "id": "item-1",
            "kind": "text",
            "payload": {
                "type": "text",
                "plain": "Hello",
                "html": null,
                "rtf": null
            },
            "fingerprint": "fingerprint-1",
            "preview": "Hello",
            "byteSize": 5,
            "isFavorite": true,
            "createdAtMs": 1_000,
            "updatedAtMs": 2_000
        });

        assert_eq!(serde_json::to_value(&item).unwrap(), expected_json);
        assert_eq!(
            serde_json::from_value::<ClipboardItem>(expected_json).unwrap(),
            item
        );
    }

    #[test]
    fn history_query_uses_complete_shape_defaults_and_round_trips() {
        let default_query = HistoryQuery::default();
        let default_json = json!({
            "kind": null,
            "search": ""
        });

        assert_eq!(serde_json::to_value(&default_query).unwrap(), default_json);
        assert_eq!(
            serde_json::from_value::<HistoryQuery>(default_json).unwrap(),
            default_query
        );

        let filtered_query = HistoryQuery {
            kind: Some(ClipboardKind::Image),
            search: "needle".into(),
        };
        let filtered_json = json!({
            "kind": "image",
            "search": "needle"
        });

        assert_eq!(
            serde_json::to_value(&filtered_query).unwrap(),
            filtered_json
        );
        assert_eq!(
            serde_json::from_value::<HistoryQuery>(filtered_json).unwrap(),
            filtered_query
        );
    }
}
