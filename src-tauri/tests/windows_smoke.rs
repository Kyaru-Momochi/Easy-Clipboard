#![cfg(windows)]

use std::fs;

use easy_clipboard_lib::{
    clipboard::{ClipboardBackend, WindowsClipboard},
    domain::{ClipboardPayload, FileEntry, MediaKind},
};
use image::{ImageEncoder, codecs::png::PngEncoder};
use tempfile::tempdir;

#[test]
#[ignore = "mutates the interactive Windows clipboard"]
fn plain_text_round_trip() {
    let clipboard = WindowsClipboard::new().unwrap();
    let payload = ClipboardPayload::Text {
        plain: "Easy Clipboard smoke \u{1f4cb}".into(),
        html: None,
        rtf: None,
    };

    let receipt = clipboard.write(&payload).unwrap();
    let snapshot = clipboard.read(50 * 1024 * 1024).unwrap();

    assert_ne!(receipt, 0);
    assert_eq!(
        snapshot.plain_text.as_deref(),
        Some("Easy Clipboard smoke 📋")
    );
}

#[test]
#[ignore = "mutates the interactive Windows clipboard"]
fn png_round_trip() {
    let directory = tempdir().unwrap();
    let png_path = directory.path().join("pixel.png");
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&[12, 34, 56, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    fs::write(&png_path, &png).unwrap();
    let clipboard = WindowsClipboard::new().unwrap();
    let payload = ClipboardPayload::Image {
        png_path: png_path.to_string_lossy().into_owned(),
        thumbnail_path: png_path.to_string_lossy().into_owned(),
        width: 1,
        height: 1,
    };

    clipboard.write(&payload).unwrap();
    let snapshot = clipboard.read(50 * 1024 * 1024).unwrap();

    assert_eq!(snapshot.png.as_deref(), Some(png.as_slice()));
}

#[test]
#[ignore = "mutates the interactive Windows clipboard"]
fn cf_hdrop_file_list_round_trip() {
    let directory = tempdir().unwrap();
    let file_path = directory.path().join("sample.mp3");
    fs::write(&file_path, b"audio").unwrap();
    let clipboard = WindowsClipboard::new().unwrap();
    let payload = ClipboardPayload::Files {
        entries: vec![FileEntry {
            path: file_path.to_string_lossy().into_owned(),
            name: "sample.mp3".into(),
            extension: "mp3".into(),
            size_bytes: 5,
            media_kind: MediaKind::Audio,
            available: true,
        }],
    };

    clipboard.write(&payload).unwrap();
    let snapshot = clipboard.read(50 * 1024 * 1024).unwrap();

    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(snapshot.files[0].path, file_path.to_string_lossy());
}
