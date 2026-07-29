use std::io::{self, Cursor, Write};

use image::{
    ColorType, DynamicImage, ImageEncoder, ImageFormat, ImageReader, Limits, RgbaImage,
    codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder},
};
use sha2::{Digest, Sha256};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    domain::{
        AppSettings, ClipboardItem, ClipboardKind, ClipboardPayload, FileEntry, ItemId, MediaKind,
    },
    error::AppError,
};

const PREVIEW_MAX_GRAPHEMES: usize = 120;
const THUMBNAIL_MAX_WIDTH: u32 = 320;
const THUMBNAIL_MAX_HEIGHT: u32 = 240;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_IMAGE_ALLOCATION_BYTES: u64 = 256 * 1024 * 1024;
const IMAGE_PROCESSING_OVERHEAD_BYTES: u64 = 1024 * 1024;
const PNG_ENCODER_SCRATCH_BYTES: u64 = 256 * 1024;

const FILES_FINGERPRINT_TAG: &[u8] = b"easy-clipboard:files:v1";
const IMAGE_FINGERPRINT_TAG: &[u8] = b"easy-clipboard:image:v1";
const TEXT_FINGERPRINT_TAG: &[u8] = b"easy-clipboard:text:v1";

#[derive(Clone, Debug, Default)]
pub struct RawClipboardSnapshot {
    pub files: Vec<FileEntry>,
    /// A selected file-list format whose roots exceeded a platform-side shared traversal budget.
    pub files_oversize_bytes: Option<u64>,
    pub png: Option<Vec<u8>>,
    /// BMP-decodable bytes. This layer does not parse raw CF_DIB; the Windows adapter adds its
    /// BITMAPFILEHEADER before normalization.
    pub dib: Option<Vec<u8>>,
    /// A selected image format whose allocation was rejected by the platform adapter.
    pub image_oversize_bytes: Option<u64>,
    pub plain_text: Option<String>,
    pub html: Option<String>,
    pub rtf: Option<Vec<u8>>,
    /// A selected text representation whose allocation was rejected by the platform adapter.
    pub text_oversize_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingImageResource {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedClipboard {
    pub item: ClipboardItem,
    pub image_resources: Vec<PendingImageResource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NormalizeOutcome {
    Accepted(NormalizedClipboard),
    IgnoredEmpty,
    IgnoredOversize {
        measured_bytes: u64,
        limit_bytes: u64,
    },
}

pub fn normalize(
    raw: RawClipboardSnapshot,
    settings: &AppSettings,
    now_ms: i64,
) -> Result<NormalizeOutcome, AppError> {
    if let Some(measured_bytes) = raw.files_oversize_bytes {
        return Ok(oversize(measured_bytes, settings.max_item_bytes));
    }

    if !raw.files.is_empty() {
        return Ok(normalize_files(raw.files, settings.max_item_bytes, now_ms));
    }

    if let Some(measured_bytes) = raw.image_oversize_bytes {
        return Ok(oversize(measured_bytes, settings.max_item_bytes));
    }

    if let Some(png) = raw.png {
        return normalize_image(png, ImageFormat::Png, settings.max_item_bytes, now_ms);
    }

    if let Some(dib) = raw.dib {
        return normalize_image(dib, ImageFormat::Bmp, settings.max_item_bytes, now_ms);
    }

    if let Some(measured_bytes) = raw.text_oversize_bytes {
        return Ok(oversize(measured_bytes, settings.max_item_bytes));
    }

    Ok(normalize_text(
        raw.plain_text,
        raw.html,
        raw.rtf,
        settings.max_item_bytes,
        now_ms,
    ))
}

fn normalize_files(entries: Vec<FileEntry>, limit_bytes: u64, now_ms: i64) -> NormalizeOutcome {
    let Some(measured_bytes) = checked_sum(entries.iter().map(|entry| entry.size_bytes)) else {
        return oversize(u64::MAX, limit_bytes);
    };
    if measured_bytes > limit_bytes {
        return oversize(measured_bytes, limit_bytes);
    }

    let fingerprint = fingerprint_files(&entries);
    let preview = files_preview(&entries);
    NormalizeOutcome::Accepted(NormalizedClipboard {
        item: clipboard_item(
            ClipboardKind::Files,
            ClipboardPayload::Files { entries },
            fingerprint,
            preview,
            measured_bytes,
            now_ms,
        ),
        image_resources: Vec::new(),
    })
}

fn normalize_image(
    input: Vec<u8>,
    format: ImageFormat,
    limit_bytes: u64,
    now_ms: i64,
) -> Result<NormalizeOutcome, AppError> {
    let input_bytes = u64::try_from(input.len()).unwrap_or(u64::MAX);
    if input_bytes > limit_bytes {
        return Ok(oversize(input_bytes, limit_bytes));
    }

    let working_cap = image_working_cap(limit_bytes);
    let decode_budget = working_cap
        .checked_sub(input_bytes)
        .ok_or(AppError::InvalidClipboardImage)?;
    let decoded = decode_image(&input, format, decode_budget)?;
    let source_bytes = usize_as_u64(decoded.as_bytes().len());
    let decode_peak = input_bytes
        .checked_add(source_bytes)
        .ok_or(AppError::InvalidClipboardImage)?;
    ensure_image_budget(decode_peak, working_cap)?;
    drop(input);

    let width = decoded.width();
    let height = decoded.height();
    let rgba_bytes = rgba_allocation_bytes(width, height)?;
    let conversion_peak = if matches!(&decoded, DynamicImage::ImageRgba8(_)) {
        source_bytes
    } else {
        source_bytes
            .checked_add(rgba_bytes)
            .ok_or(AppError::InvalidClipboardImage)?
    };
    ensure_image_budget(conversion_peak, working_cap)?;
    let image = decoded.into_rgba8();

    let normalized_png = encode_png(&image, png_output_budget(working_cap, rgba_bytes)?)?;
    let measured_bytes = u64::try_from(normalized_png.len()).unwrap_or(u64::MAX);
    if measured_bytes > limit_bytes {
        return Ok(oversize(measured_bytes, limit_bytes));
    }

    let normalized_live_bytes = rgba_bytes
        .checked_add(measured_bytes)
        .ok_or(AppError::InvalidClipboardImage)?;
    ensure_image_budget(normalized_live_bytes, working_cap)?;
    let (thumbnail_width, thumbnail_height) = thumbnail_dimensions(width, height)?;
    let thumbnail_bytes = rgba_allocation_bytes(thumbnail_width, thumbnail_height)?;
    let thumbnail_live_bytes = normalized_live_bytes
        .checked_add(thumbnail_bytes)
        .ok_or(AppError::InvalidClipboardImage)?;
    ensure_image_budget(thumbnail_live_bytes, working_cap)?;

    // imageops::thumbnail allocates the destination directly. Its exact RGBA bytes are budgeted
    // above while the original RGBA image and normalized PNG remain live.
    let thumbnail = image::imageops::thumbnail(&image, thumbnail_width, thumbnail_height);
    let thumbnail_png = encode_png(
        &thumbnail,
        png_output_budget(working_cap, thumbnail_live_bytes)?,
    )?;
    drop(thumbnail);
    drop(image);

    let fingerprint = fingerprint_image(&normalized_png);
    let png_file_name = format!("{fingerprint}.png");
    let thumbnail_file_name = format!("{fingerprint}-thumb.png");
    let payload = ClipboardPayload::Image {
        png_path: png_file_name.clone(),
        thumbnail_path: thumbnail_file_name.clone(),
        width,
        height,
    };

    Ok(NormalizeOutcome::Accepted(NormalizedClipboard {
        item: clipboard_item(
            ClipboardKind::Image,
            payload,
            fingerprint,
            format!("{width} × {height} image"),
            measured_bytes,
            now_ms,
        ),
        image_resources: vec![
            PendingImageResource {
                file_name: png_file_name,
                bytes: normalized_png,
            },
            PendingImageResource {
                file_name: thumbnail_file_name,
                bytes: thumbnail_png,
            },
        ],
    }))
}

fn normalize_text(
    plain_text: Option<String>,
    html: Option<String>,
    rtf: Option<Vec<u8>>,
    limit_bytes: u64,
    now_ms: i64,
) -> NormalizeOutcome {
    let has_content = plain_text.as_ref().is_some_and(|plain| !plain.is_empty())
        || html.as_ref().is_some_and(|rich| !rich.is_empty())
        || rtf.as_ref().is_some_and(|rich| !rich.is_empty());
    if !has_content {
        return NormalizeOutcome::IgnoredEmpty;
    }

    let lengths = [
        usize_as_u64(plain_text.as_ref().map_or(0, String::len)),
        usize_as_u64(html.as_ref().map_or(0, String::len)),
        usize_as_u64(rtf.as_ref().map_or(0, Vec::len)),
    ];
    let Some(measured_bytes) = checked_sum(lengths) else {
        return oversize(u64::MAX, limit_bytes);
    };
    if measured_bytes > limit_bytes {
        return oversize(measured_bytes, limit_bytes);
    }

    let preview = match plain_text.as_deref() {
        Some(plain) => {
            let collapsed = collapsed_preview(plain);
            if collapsed.is_empty() {
                "Text".to_owned()
            } else {
                collapsed
            }
        }
        None => "Rich text".to_owned(),
    };
    let fingerprint = fingerprint_text(plain_text.as_deref().unwrap_or_default(), &html, &rtf);
    let payload = ClipboardPayload::Text {
        plain: plain_text.unwrap_or_default(),
        html,
        rtf,
    };

    NormalizeOutcome::Accepted(NormalizedClipboard {
        item: clipboard_item(
            ClipboardKind::Text,
            payload,
            fingerprint,
            preview,
            measured_bytes,
            now_ms,
        ),
        image_resources: Vec::new(),
    })
}

fn decode_image(
    input: &[u8],
    format: ImageFormat,
    allocation_budget: u64,
) -> Result<DynamicImage, AppError> {
    let dimensions = ImageReader::with_format(Cursor::new(input), format)
        .into_dimensions()
        .map_err(|_| AppError::InvalidClipboardImage)?;
    validate_image_dimensions(dimensions)?;

    let mut reader = ImageReader::with_format(Cursor::new(input), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(allocation_budget);
    reader.limits(limits);

    reader.decode().map_err(|_| AppError::InvalidClipboardImage)
}

fn validate_image_dimensions((width, height): (u32, u32)) -> Result<(), AppError> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(AppError::InvalidClipboardImage);
    }

    Ok(())
}

fn encode_png(image: &RgbaImage, max_output_bytes: u64) -> Result<Vec<u8>, AppError> {
    let mut output = LimitedWriter::new(max_output_bytes);
    PngEncoder::new_with_quality(&mut output, CompressionType::Best, PngFilterType::Adaptive)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ColorType::Rgba8.into(),
        )
        .map_err(|_| AppError::InvalidClipboardImage)?;
    Ok(output.into_inner())
}

fn image_working_cap(limit_bytes: u64) -> u64 {
    // The stored-item limit gets a fixed 1 MiB processing allowance for bounded thumbnail and
    // codec output buffers, then a 256 MiB hard ceiling. Each encoding stage separately reserves
    // PNG_ENCODER_SCRATCH_BYTES before its limited output writer may grow.
    limit_bytes
        .checked_add(IMAGE_PROCESSING_OVERHEAD_BYTES)
        .unwrap_or(MAX_IMAGE_ALLOCATION_BYTES)
        .min(MAX_IMAGE_ALLOCATION_BYTES)
}

fn ensure_image_budget(live_bytes: u64, working_cap: u64) -> Result<(), AppError> {
    if live_bytes <= working_cap {
        Ok(())
    } else {
        Err(AppError::InvalidClipboardImage)
    }
}

fn png_output_budget(working_cap: u64, live_bytes: u64) -> Result<u64, AppError> {
    working_cap
        .checked_sub(live_bytes)
        .and_then(|remaining| remaining.checked_sub(PNG_ENCODER_SCRATCH_BYTES))
        .ok_or(AppError::InvalidClipboardImage)
}

fn rgba_allocation_bytes(width: u32, height: u32) -> Result<u64, AppError> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(AppError::InvalidClipboardImage)
}

fn thumbnail_dimensions(width: u32, height: u32) -> Result<(u32, u32), AppError> {
    if width <= THUMBNAIL_MAX_WIDTH && height <= THUMBNAIL_MAX_HEIGHT {
        return Ok((width, height));
    }

    let width_limited = u64::from(width)
        .checked_mul(u64::from(THUMBNAIL_MAX_HEIGHT))
        .ok_or(AppError::InvalidClipboardImage)?
        > u64::from(height)
            .checked_mul(u64::from(THUMBNAIL_MAX_WIDTH))
            .ok_or(AppError::InvalidClipboardImage)?;
    let (thumbnail_width, thumbnail_height) = if width_limited {
        let scaled_height = u64::from(height)
            .checked_mul(u64::from(THUMBNAIL_MAX_WIDTH))
            .ok_or(AppError::InvalidClipboardImage)?
            / u64::from(width);
        (
            THUMBNAIL_MAX_WIDTH,
            u32::try_from(scaled_height.max(1)).map_err(|_| AppError::InvalidClipboardImage)?,
        )
    } else {
        let scaled_width = u64::from(width)
            .checked_mul(u64::from(THUMBNAIL_MAX_HEIGHT))
            .ok_or(AppError::InvalidClipboardImage)?
            / u64::from(height);
        (
            u32::try_from(scaled_width.max(1)).map_err(|_| AppError::InvalidClipboardImage)?,
            THUMBNAIL_MAX_HEIGHT,
        )
    };

    Ok((thumbnail_width, thumbnail_height))
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: u64,
}

impl LimitedWriter {
    fn new(limit: u64) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next_len = usize_as_u64(self.bytes.len())
            .checked_add(usize_as_u64(buffer.len()))
            .ok_or_else(|| io::Error::other("PNG output length overflow"))?;
        if next_len > self.limit {
            return Err(io::Error::other("PNG output exceeds allocation budget"));
        }
        self.bytes
            .try_reserve_exact(buffer.len())
            .map_err(|_| io::Error::other("PNG output allocation failed"))?;
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn clipboard_item(
    kind: ClipboardKind,
    payload: ClipboardPayload,
    fingerprint: String,
    preview: String,
    byte_size: u64,
    now_ms: i64,
) -> ClipboardItem {
    ClipboardItem {
        id: ItemId(fingerprint.clone()),
        kind,
        payload,
        fingerprint,
        preview,
        byte_size,
        is_favorite: false,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    }
}

fn oversize(measured_bytes: u64, limit_bytes: u64) -> NormalizeOutcome {
    NormalizeOutcome::IgnoredOversize {
        measured_bytes,
        limit_bytes,
    }
}

fn usize_as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn checked_sum(values: impl IntoIterator<Item = u64>) -> Option<u64> {
    values
        .into_iter()
        .try_fold(0_u64, |total, value| total.checked_add(value))
}

fn fingerprint_files(entries: &[FileEntry]) -> String {
    let mut hasher = Sha256::new();
    write_frame(&mut hasher, FILES_FINGERPRINT_TAG);
    write_u64(&mut hasher, usize_as_u64(entries.len()));
    for entry in entries {
        write_frame(&mut hasher, normalized_windows_path(&entry.path).as_bytes());
        write_frame(
            &mut hasher,
            normalized_path_derived_field(&entry.name).as_bytes(),
        );
        write_frame(
            &mut hasher,
            normalized_path_derived_field(&entry.extension).as_bytes(),
        );
        write_u64(&mut hasher, entry.size_bytes);
        hasher.update([media_kind_marker(&entry.media_kind)]);
        hasher.update([u8::from(entry.available)]);
    }
    finish_fingerprint(hasher)
}

fn fingerprint_image(normalized_png: &[u8]) -> String {
    let mut hasher = Sha256::new();
    write_frame(&mut hasher, IMAGE_FINGERPRINT_TAG);
    write_frame(&mut hasher, normalized_png);
    finish_fingerprint(hasher)
}

fn fingerprint_text(plain: &str, html: &Option<String>, rtf: &Option<Vec<u8>>) -> String {
    let mut hasher = Sha256::new();
    write_frame(&mut hasher, TEXT_FINGERPRINT_TAG);
    write_frame(&mut hasher, plain.as_bytes());
    write_optional_frame(&mut hasher, html.as_deref().map(str::as_bytes));
    write_optional_frame(&mut hasher, rtf.as_deref());
    finish_fingerprint(hasher)
}

fn write_optional_frame(hasher: &mut Sha256, bytes: Option<&[u8]>) {
    match bytes {
        Some(bytes) => {
            hasher.update([1]);
            write_frame(hasher, bytes);
        }
        None => hasher.update([0]),
    }
}

fn write_frame(hasher: &mut Sha256, bytes: &[u8]) {
    write_u64(hasher, usize_as_u64(bytes.len()));
    hasher.update(bytes);
}

fn write_u64(hasher: &mut Sha256, value: u64) {
    hasher.update(value.to_be_bytes());
}

fn finish_fingerprint(hasher: Sha256) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let digest = hasher.finalize();
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        fingerprint.push(char::from(HEX[usize::from(byte >> 4)]));
        fingerprint.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    fingerprint
}

/// Produces a portable, lexical Windows identity only. This does not access the filesystem and
/// therefore does not resolve symlinks, junctions, short names, volume aliases, or filesystem
/// case-folding rules beyond the app's Unicode lowercase policy.
///
/// Verbatim/device paths beginning `\\?\` keep their prefix and component structure after slash
/// normalization and lowercasing. They deliberately remain distinct from ordinary Win32 paths:
/// this can retain duplicate history entries for two names of the same file, but avoids falsely
/// merging verbatim names whose repeated separators, dot components, or trailing characters are
/// semantically significant.
pub(crate) fn normalized_windows_path(path: &str) -> String {
    let separated = path.replace('/', "\\").to_lowercase();
    if separated.starts_with("\\\\?\\") {
        return separated;
    }
    let normalized_prefix = separated;

    if normalized_prefix.starts_with("\\\\") {
        return normalize_unc_path(&normalized_prefix);
    }
    if is_drive_absolute(&normalized_prefix) {
        let drive = &normalized_prefix[..2];
        let components = resolve_lexical_components(normalized_prefix[2..].split('\\'), true);
        return if components.is_empty() {
            format!("{drive}\\")
        } else {
            format!("{drive}\\{}", components.join("\\"))
        };
    }
    if normalized_prefix.starts_with('\\') {
        let components = resolve_lexical_components(
            normalized_prefix.trim_start_matches('\\').split('\\'),
            true,
        );
        return if components.is_empty() {
            "\\".to_owned()
        } else {
            format!("\\{}", components.join("\\"))
        };
    }
    if has_drive_prefix(&normalized_prefix) {
        let drive = &normalized_prefix[..2];
        let components = resolve_lexical_components(normalized_prefix[2..].split('\\'), false);
        return if components.is_empty() {
            drive.to_owned()
        } else {
            format!("{drive}{}", components.join("\\"))
        };
    }

    let components = resolve_lexical_components(normalized_prefix.split('\\'), false);
    if components.is_empty() {
        if normalized_prefix.is_empty() {
            String::new()
        } else {
            ".".to_owned()
        }
    } else {
        components.join("\\")
    }
}

fn normalize_unc_path(path: &str) -> String {
    let components = path
        .trim_start_matches('\\')
        .split('\\')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    match components.as_slice() {
        [] => "\\\\".to_owned(),
        [server] => format!("\\\\{server}"),
        [server, share, remaining @ ..] => {
            let remaining = resolve_lexical_components(remaining.iter().copied(), true);
            if remaining.is_empty() {
                format!("\\\\{server}\\{share}\\")
            } else {
                format!("\\\\{server}\\{share}\\{}", remaining.join("\\"))
            }
        }
    }
}

fn resolve_lexical_components<'a>(
    components: impl IntoIterator<Item = &'a str>,
    absolute: bool,
) -> Vec<&'a str> {
    let mut resolved = Vec::new();
    for component in components {
        match component {
            "" | "." => {}
            ".." => {
                if resolved.last().is_some_and(|last| *last != "..") {
                    resolved.pop();
                } else if !absolute {
                    resolved.push(component);
                }
            }
            _ => resolved.push(component),
        }
    }
    resolved
}

fn has_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_drive_absolute(path: &str) -> bool {
    has_drive_prefix(path) && path.as_bytes().get(2) == Some(&b'\\')
}

fn normalized_path_derived_field(value: &str) -> String {
    let normalized = normalized_windows_path(value);
    normalized
        .trim_end_matches('\\')
        .rsplit('\\')
        .next()
        .unwrap_or_default()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn media_kind_marker(media_kind: &MediaKind) -> u8 {
    match media_kind {
        MediaKind::Audio => 1,
        MediaKind::Video => 2,
        MediaKind::Other => 3,
    }
}

fn files_preview(entries: &[FileEntry]) -> String {
    let mut preview = String::new();
    let mut grapheme_count = 0;

    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            if grapheme_count + 2 > PREVIEW_MAX_GRAPHEMES {
                break;
            }
            preview.push_str(", ");
            grapheme_count += 2;
        }
        if !push_preview_graphemes(&mut preview, &mut grapheme_count, &entry.name) {
            break;
        }
    }

    if preview.is_empty() {
        "Files".to_owned()
    } else {
        preview
    }
}

fn collapsed_preview(input: &str) -> String {
    let mut preview = String::new();
    let mut grapheme_count = 0;
    let mut pending_space = false;

    for grapheme in input.graphemes(true) {
        if grapheme.chars().all(char::is_whitespace) {
            pending_space = !preview.is_empty();
            continue;
        }

        if pending_space {
            if grapheme_count + 1 >= PREVIEW_MAX_GRAPHEMES {
                break;
            }
            preview.push(' ');
            grapheme_count += 1;
            pending_space = false;
        }
        if grapheme_count == PREVIEW_MAX_GRAPHEMES {
            break;
        }
        preview.push_str(grapheme);
        grapheme_count += 1;
    }

    preview
}

fn push_preview_graphemes(preview: &mut String, grapheme_count: &mut usize, input: &str) -> bool {
    for grapheme in input.graphemes(true) {
        if *grapheme_count == PREVIEW_MAX_GRAPHEMES {
            return false;
        }
        preview.push_str(grapheme);
        *grapheme_count += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

    use super::{
        NormalizeOutcome, NormalizedClipboard, RawClipboardSnapshot, checked_sum, normalize,
    };
    use crate::{
        domain::{AppSettings, ClipboardKind, ClipboardPayload, FileEntry, ItemId, MediaKind},
        error::AppError,
    };

    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5,
        0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64,
        0xf8, 0x0f, 0x00, 0x01, 0x05, 0x01, 0x01, 0x27, 0x18, 0xe3, 0x66, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn settings_with_limit(max_item_bytes: u64) -> AppSettings {
        AppSettings {
            max_item_bytes,
            ..AppSettings::default()
        }
    }

    fn file(path: &str, name: &str, size_bytes: u64) -> FileEntry {
        FileEntry {
            path: path.into(),
            name: name.into(),
            extension: "txt".into(),
            size_bytes,
            media_kind: MediaKind::Other,
            available: true,
        }
    }

    fn accepted(outcome: NormalizeOutcome) -> NormalizedClipboard {
        match outcome {
            NormalizeOutcome::Accepted(normalized) => normalized,
            other => panic!("expected accepted clipboard, got {other:?}"),
        }
    }

    fn fingerprint_for_file(entry: FileEntry) -> String {
        accepted(
            normalize(
                RawClipboardSnapshot {
                    files: vec![entry],
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
        )
        .item
        .fingerprint
    }

    fn bmp(width: u32, height: u32) -> Vec<u8> {
        let row_size = (width * 3).div_ceil(4) * 4;
        let image_size = row_size * height;
        let file_size = 54 + image_size;
        let mut bytes = vec![0_u8; file_size as usize];
        bytes[0..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&file_size.to_le_bytes());
        bytes[10..14].copy_from_slice(&54_u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40_u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&(width as i32).to_le_bytes());
        bytes[22..26].copy_from_slice(&(height as i32).to_le_bytes());
        bytes[26..28].copy_from_slice(&1_u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24_u16.to_le_bytes());
        bytes[34..38].copy_from_slice(&image_size.to_le_bytes());
        for pixel in bytes[54..].chunks_exact_mut(3) {
            pixel.copy_from_slice(&[0x20, 0x80, 0xe0]);
        }
        bytes
    }

    fn hostile_bmp_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![0_u8; 54];
        bytes[0..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&54_u32.to_le_bytes());
        bytes[10..14].copy_from_slice(&54_u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40_u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&(width as i32).to_le_bytes());
        bytes[22..26].copy_from_slice(&(height as i32).to_le_bytes());
        bytes[26..28].copy_from_slice(&1_u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24_u16.to_le_bytes());
        bytes
    }

    fn rgba8_png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, Rgba([0x20, 0x80, 0xe0, 0xff]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn rgba16_png(width: u32, height: u32) -> Vec<u8> {
        let image =
            image::ImageBuffer::from_pixel(width, height, Rgba([0x1000, 0x8000, 0xe000, 0xffff]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba16(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
        assert_eq!(&bytes[..8], PNG_SIGNATURE);
        (
            u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
            u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
        )
    }

    #[test]
    fn files_win_over_bitmap_and_text_formats() {
        let entries = vec![file(r"C:\notes.txt", "notes.txt", 42)];
        let raw = RawClipboardSnapshot {
            files: entries.clone(),
            files_oversize_bytes: None,
            png: Some(vec![0xff]),
            dib: Some(vec![0xfe]),
            image_oversize_bytes: None,
            plain_text: Some("ignored".into()),
            html: Some("<b>ignored</b>".into()),
            rtf: Some(b"{\\rtf1 ignored}".to_vec()),
            text_oversize_bytes: None,
        };

        let normalized = accepted(normalize(raw, &AppSettings::default(), 100).unwrap());

        assert_eq!(normalized.item.kind, ClipboardKind::Files);
        assert_eq!(normalized.item.payload, ClipboardPayload::Files { entries });
        assert!(normalized.image_resources.is_empty());
    }

    #[test]
    fn bmp_decodable_dib_fallback_wins_over_text_when_no_files_exist() {
        let raw = RawClipboardSnapshot {
            dib: Some(bmp(2, 1)),
            plain_text: Some("ignored".into()),
            html: Some("<b>ignored</b>".into()),
            ..RawClipboardSnapshot::default()
        };

        let normalized = accepted(normalize(raw, &AppSettings::default(), 100).unwrap());

        assert_eq!(normalized.item.kind, ClipboardKind::Image);
        assert!(matches!(
            normalized.item.payload,
            ClipboardPayload::Image {
                width: 2,
                height: 1,
                ..
            }
        ));
    }

    #[test]
    fn png_wins_over_dib() {
        let raw = RawClipboardSnapshot {
            png: Some(TINY_PNG.to_vec()),
            dib: Some(bmp(2, 1)),
            ..RawClipboardSnapshot::default()
        };

        let normalized = accepted(normalize(raw, &AppSettings::default(), 100).unwrap());

        assert!(matches!(
            normalized.item.payload,
            ClipboardPayload::Image {
                width: 1,
                height: 1,
                ..
            }
        ));
    }

    #[test]
    fn rich_text_keeps_plain_html_and_rtf_and_domain_item_fields_are_correct() {
        let plain = "  Hello\nworld  ".to_owned();
        let html = "<p>Hello <b>world</b></p>".to_owned();
        let rtf = b"{\\rtf1 Hello world}".to_vec();
        let measured = (plain.len() + html.len() + rtf.len()) as u64;
        let raw = RawClipboardSnapshot {
            plain_text: Some(plain.clone()),
            html: Some(html.clone()),
            rtf: Some(rtf.clone()),
            ..RawClipboardSnapshot::default()
        };

        let normalized = accepted(normalize(raw, &AppSettings::default(), 1_234).unwrap());
        let item = normalized.item;

        assert_eq!(item.kind, ClipboardKind::Text);
        assert_eq!(
            item.payload,
            ClipboardPayload::Text {
                plain,
                html: Some(html),
                rtf: Some(rtf),
            }
        );
        assert_eq!(item.preview, "Hello world");
        assert_eq!(item.byte_size, measured);
        assert!(!item.is_favorite);
        assert_eq!(item.created_at_ms, 1_234);
        assert_eq!(item.updated_at_ms, 1_234);
        assert_eq!(item.id, ItemId(item.fingerprint.clone()));
        assert_eq!(item.fingerprint.len(), 64);
        assert!(
            item.fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );

        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["kind"], "text");
        assert_eq!(json["payload"]["type"], "text");
        assert_eq!(json["payload"]["plain"], "  Hello\nworld  ");
        assert_eq!(json["payload"]["html"], "<p>Hello <b>world</b></p>");
        assert_eq!(
            json["payload"]["rtf"],
            serde_json::json!(b"{\\rtf1 Hello world}")
        );
        assert_eq!(json["byteSize"], measured);
        assert_eq!(json["createdAtMs"], 1_234);
        assert_eq!(json["updatedAtMs"], 1_234);
    }

    #[test]
    fn payload_over_configured_limit_is_ignored_for_files_text_and_raw_image() {
        let settings = settings_with_limit(5);
        let files = normalize(
            RawClipboardSnapshot {
                files: vec![file("a", "a", 6)],
                ..RawClipboardSnapshot::default()
            },
            &settings,
            0,
        )
        .unwrap();
        let text = normalize(
            RawClipboardSnapshot {
                plain_text: Some("123456".into()),
                ..RawClipboardSnapshot::default()
            },
            &settings,
            0,
        )
        .unwrap();
        let image_settings = settings_with_limit(TINY_PNG.len() as u64 - 1);
        let image = normalize(
            RawClipboardSnapshot {
                png: Some(TINY_PNG.to_vec()),
                ..RawClipboardSnapshot::default()
            },
            &image_settings,
            0,
        )
        .unwrap();

        assert_eq!(
            files,
            NormalizeOutcome::IgnoredOversize {
                measured_bytes: 6,
                limit_bytes: 5
            }
        );
        assert_eq!(
            text,
            NormalizeOutcome::IgnoredOversize {
                measured_bytes: 6,
                limit_bytes: 5
            }
        );
        assert_eq!(
            image,
            NormalizeOutcome::IgnoredOversize {
                measured_bytes: TINY_PNG.len() as u64,
                limit_bytes: TINY_PNG.len() as u64 - 1
            }
        );
    }

    #[test]
    fn adapter_image_oversize_hint_wins_over_lower_priority_text_without_allocation() {
        let settings = AppSettings::default().with_limit_mb(1).unwrap();
        let raw = RawClipboardSnapshot {
            image_oversize_bytes: Some(500 * 1024 * 1024 + 1),
            plain_text: Some("lower priority".into()),
            ..RawClipboardSnapshot::default()
        };

        assert_eq!(
            normalize(raw, &settings, 0).unwrap(),
            NormalizeOutcome::IgnoredOversize {
                measured_bytes: 500 * 1024 * 1024 + 1,
                limit_bytes: 1024 * 1024,
            }
        );
    }

    #[test]
    fn adapter_file_oversize_hint_wins_over_all_lower_priority_formats() {
        let settings = AppSettings::default().with_limit_mb(1).unwrap();
        let raw = RawClipboardSnapshot {
            files_oversize_bytes: Some(2 * 1024 * 1024),
            png: Some(TINY_PNG.to_vec()),
            plain_text: Some("lower priority".into()),
            ..RawClipboardSnapshot::default()
        };

        assert_eq!(
            normalize(raw, &settings, 0).unwrap(),
            NormalizeOutcome::IgnoredOversize {
                measured_bytes: 2 * 1024 * 1024,
                limit_bytes: 1024 * 1024,
            }
        );
    }

    #[test]
    fn higher_priority_payload_wins_over_lower_priority_oversize_hint() {
        let settings = AppSettings::default().with_limit_mb(1).unwrap();
        let file = FileEntry {
            path: r"C:\small.txt".into(),
            name: "small.txt".into(),
            extension: "txt".into(),
            size_bytes: 1,
            media_kind: MediaKind::Other,
            available: true,
        };

        let files = normalize(
            RawClipboardSnapshot {
                files: vec![file],
                image_oversize_bytes: Some(500 * 1024 * 1024 + 1),
                text_oversize_bytes: Some(500 * 1024 * 1024 + 1),
                ..RawClipboardSnapshot::default()
            },
            &settings,
            0,
        )
        .unwrap();
        assert!(matches!(
            files,
            NormalizeOutcome::Accepted(NormalizedClipboard {
                item: crate::domain::ClipboardItem {
                    kind: ClipboardKind::Files,
                    ..
                },
                ..
            })
        ));

        let image = normalize(
            RawClipboardSnapshot {
                png: Some(TINY_PNG.to_vec()),
                text_oversize_bytes: Some(500 * 1024 * 1024 + 1),
                ..RawClipboardSnapshot::default()
            },
            &settings,
            0,
        )
        .unwrap();
        assert!(matches!(
            image,
            NormalizeOutcome::Accepted(NormalizedClipboard {
                item: crate::domain::ClipboardItem {
                    kind: ClipboardKind::Image,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn adapter_text_oversize_hint_is_ignored_by_size_policy_without_text_bytes() {
        let settings = AppSettings::default().with_limit_mb(1).unwrap();
        let raw = RawClipboardSnapshot {
            text_oversize_bytes: Some(2 * 1024 * 1024),
            ..RawClipboardSnapshot::default()
        };

        assert_eq!(
            normalize(raw, &settings, 0).unwrap(),
            NormalizeOutcome::IgnoredOversize {
                measured_bytes: 2 * 1024 * 1024,
                limit_bytes: 1024 * 1024,
            }
        );
    }

    #[test]
    fn payload_equal_to_limit_is_accepted() {
        let settings = settings_with_limit(6);
        let files = normalize(
            RawClipboardSnapshot {
                files: vec![file("a", "a", 6)],
                ..RawClipboardSnapshot::default()
            },
            &settings,
            0,
        )
        .unwrap();
        let text = normalize(
            RawClipboardSnapshot {
                plain_text: Some("123456".into()),
                ..RawClipboardSnapshot::default()
            },
            &settings,
            0,
        )
        .unwrap();
        let image_probe = accepted(
            normalize(
                RawClipboardSnapshot {
                    png: Some(TINY_PNG.to_vec()),
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
        );
        let image_limit = image_probe.item.byte_size;
        assert!(TINY_PNG.len() as u64 <= image_limit);
        let image = normalize(
            RawClipboardSnapshot {
                png: Some(TINY_PNG.to_vec()),
                ..RawClipboardSnapshot::default()
            },
            &settings_with_limit(image_limit),
            0,
        )
        .unwrap();

        assert!(matches!(files, NormalizeOutcome::Accepted(_)));
        assert!(matches!(text, NormalizeOutcome::Accepted(_)));
        assert!(matches!(image, NormalizeOutcome::Accepted(_)));
    }

    #[test]
    fn duplicate_payloads_have_identical_sha256_fingerprint_independent_of_timestamp() {
        let raw = RawClipboardSnapshot {
            plain_text: Some("same".into()),
            html: Some("<p>same</p>".into()),
            rtf: Some(b"{\\rtf1 same}".to_vec()),
            ..RawClipboardSnapshot::default()
        };

        let first = accepted(normalize(raw.clone(), &AppSettings::default(), 1).unwrap());
        let second = accepted(normalize(raw, &AppSettings::default(), 999).unwrap());

        assert_eq!(first.item.fingerprint, second.item.fingerprint);
        assert_eq!(first.item.id, second.item.id);
        assert_ne!(first.item.created_at_ms, second.item.created_at_ms);
    }

    #[test]
    fn canonical_framing_prevents_text_representation_collision() {
        let first = RawClipboardSnapshot {
            plain_text: Some("a".into()),
            html: Some("bc".into()),
            ..RawClipboardSnapshot::default()
        };
        let second = RawClipboardSnapshot {
            plain_text: Some("ab".into()),
            html: Some("c".into()),
            ..RawClipboardSnapshot::default()
        };

        let first = accepted(normalize(first, &AppSettings::default(), 0).unwrap());
        let second = accepted(normalize(second, &AppSettings::default(), 0).unwrap());

        assert_ne!(first.item.fingerprint, second.item.fingerprint);
    }

    #[test]
    fn file_order_changes_fingerprint_and_identical_lists_are_stable() {
        let first_file = file(r"C:\a.txt", "a.txt", 1);
        let second_file = file(r"C:\b.txt", "b.txt", 2);
        let first_order = RawClipboardSnapshot {
            files: vec![first_file.clone(), second_file.clone()],
            ..RawClipboardSnapshot::default()
        };
        let second_order = RawClipboardSnapshot {
            files: vec![second_file, first_file],
            ..RawClipboardSnapshot::default()
        };

        let first = accepted(normalize(first_order.clone(), &AppSettings::default(), 1).unwrap());
        let duplicate = accepted(normalize(first_order, &AppSettings::default(), 2).unwrap());
        let reordered = accepted(normalize(second_order, &AppSettings::default(), 1).unwrap());

        assert_eq!(first.item.fingerprint, duplicate.item.fingerprint);
        assert_ne!(first.item.fingerprint, reordered.item.fingerprint);
    }

    #[test]
    fn file_fingerprint_includes_every_entry_field() {
        let original = FileEntry {
            path: r"C:\docs\note.txt".into(),
            name: "note.txt".into(),
            extension: "txt".into(),
            size_bytes: 42,
            media_kind: MediaKind::Other,
            available: true,
        };
        let original_fingerprint = accepted(
            normalize(
                RawClipboardSnapshot {
                    files: vec![original.clone()],
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
        )
        .item
        .fingerprint;
        let variants = [
            FileEntry {
                path: r"C:\docs\other.txt".into(),
                ..original.clone()
            },
            FileEntry {
                name: "renamed.txt".into(),
                ..original.clone()
            },
            FileEntry {
                extension: "md".into(),
                ..original.clone()
            },
            FileEntry {
                size_bytes: 43,
                ..original.clone()
            },
            FileEntry {
                media_kind: MediaKind::Audio,
                ..original.clone()
            },
            FileEntry {
                available: false,
                ..original.clone()
            },
        ];

        for variant in variants {
            let fingerprint = accepted(
                normalize(
                    RawClipboardSnapshot {
                        files: vec![variant],
                        ..RawClipboardSnapshot::default()
                    },
                    &AppSettings::default(),
                    0,
                )
                .unwrap(),
            )
            .item
            .fingerprint;
            assert_ne!(original_fingerprint, fingerprint);
        }

        let duplicate = accepted(
            normalize(
                RawClipboardSnapshot {
                    files: vec![original],
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                999,
            )
            .unwrap(),
        );
        assert_eq!(original_fingerprint, duplicate.item.fingerprint);
    }

    #[test]
    fn lexical_windows_path_normalization_handles_unicode_case_and_slashes() {
        let slash = RawClipboardSnapshot {
            files: vec![file("C:/Ärger/文档/Note.txt", "Note.txt", 1)],
            ..RawClipboardSnapshot::default()
        };
        let backslash = RawClipboardSnapshot {
            files: vec![file(r"c:\ärger\文档\note.txt", "Note.txt", 1)],
            ..RawClipboardSnapshot::default()
        };

        let slash = accepted(normalize(slash, &AppSettings::default(), 0).unwrap());
        let backslash = accepted(normalize(backslash, &AppSettings::default(), 0).unwrap());

        assert_eq!(slash.item.fingerprint, backslash.item.fingerprint);
    }

    #[test]
    fn path_name_and_extension_case_variants_share_lexical_identity() {
        let upper = FileEntry {
            path: r"C:\Docs\Note.TXT".into(),
            name: "Note.TXT".into(),
            extension: "TXT".into(),
            size_bytes: 1,
            media_kind: MediaKind::Other,
            available: true,
        };
        let lower = FileEntry {
            path: "c:/docs/note.txt".into(),
            name: "note.txt".into(),
            extension: "txt".into(),
            ..upper.clone()
        };

        assert_eq!(fingerprint_for_file(upper), fingerprint_for_file(lower));
    }

    #[test]
    fn lexical_path_identity_collapses_separators_dots_parents_and_non_root_trailing_slashes() {
        let verbose = file(r"C:\\Docs\\Draft\.\..\Note.txt\\", "Note.txt", 1);
        let compact = file("c:/docs/note.txt", "note.txt", 1);

        assert_eq!(fingerprint_for_file(verbose), fingerprint_for_file(compact));
    }

    #[test]
    fn ordinary_lexical_path_identity_normalizes_drive_and_unc_roots() {
        let drive_plain = file("c:/docs/note.txt", "note.txt", 1);
        let drive_root_repeated = file(r"C:\\\", "", 0);
        let drive_root_plain = file("c:/", "", 0);
        let drive_above_root = file(r"C:\..\..\Docs\Note.txt", "note.txt", 1);
        let unc_plain = file(r"\\server\share\note.txt", "note.txt", 1);
        let unc_above_root = file(r"\\server\share\..\..\note.txt", "note.txt", 1);
        let unc_root_trailing = file(r"\\Server\Share\\", "", 0);
        let unc_root_plain = file(r"\\server\share", "", 0);

        assert_eq!(
            fingerprint_for_file(drive_root_repeated),
            fingerprint_for_file(drive_root_plain)
        );
        assert_eq!(
            fingerprint_for_file(drive_above_root),
            fingerprint_for_file(drive_plain)
        );
        assert_eq!(
            fingerprint_for_file(unc_above_root),
            fingerprint_for_file(unc_plain.clone())
        );
        assert_eq!(
            fingerprint_for_file(unc_root_trailing),
            fingerprint_for_file(unc_root_plain)
        );
    }

    #[test]
    fn verbatim_drive_and_unc_namespaces_do_not_collide_with_ordinary_paths() {
        let verbatim_drive = file(r"\\?\C:\Docs\Note.txt", "note.txt", 1);
        let ordinary_drive = file(r"C:\Docs\Note.txt", "note.txt", 1);
        let verbatim_unc = file(r"\\?\UNC\Server\Share\Docs\Note.txt", "note.txt", 1);
        let ordinary_unc = file(r"\\Server\Share\Docs\Note.txt", "note.txt", 1);

        assert_ne!(
            fingerprint_for_file(verbatim_drive),
            fingerprint_for_file(ordinary_drive)
        );
        assert_ne!(
            fingerprint_for_file(verbatim_unc),
            fingerprint_for_file(ordinary_unc)
        );
    }

    #[test]
    fn verbatim_namespace_preserves_component_and_trailing_structure() {
        let base_fingerprint = fingerprint_for_file(file(r"\\?\C:\docs\note.txt", "note.txt", 1));
        let variants = [
            file(r"\\?\C:\docs\.\note.txt", "note.txt", 1),
            file(r"\\?\C:\docs\draft\..\note.txt", "note.txt", 1),
            file(r"\\?\C:\\docs\\note.txt", "note.txt", 1),
            file(r"\\?\C:\docs\note.txt\", "note.txt", 1),
            file(r"\\?\C:\docs\note.txt.", "note.txt", 1),
            file(r"\\?\C:\docs\note.txt ", "note.txt", 1),
        ];

        for variant in variants {
            assert_ne!(base_fingerprint, fingerprint_for_file(variant));
        }
    }

    #[test]
    fn verbatim_namespace_identity_is_stable_for_case_and_slash_spelling() {
        let upper = file(r"\\?\C:\Docs\Note.TXT", "note.txt", 1);
        let lower_slashes = file("//?/c:/docs/note.txt", "note.txt", 1);

        assert_eq!(
            fingerprint_for_file(upper),
            fingerprint_for_file(lower_slashes)
        );
    }

    #[test]
    fn genuinely_different_lexical_paths_names_and_extensions_do_not_collide() {
        let base = FileEntry {
            path: r"C:\docs\note.txt".into(),
            name: "note.txt".into(),
            extension: "txt".into(),
            size_bytes: 1,
            media_kind: MediaKind::Other,
            available: true,
        };
        let base_fingerprint = fingerprint_for_file(base.clone());
        let variants = [
            FileEntry {
                path: r"C:\other\note.txt".into(),
                ..base.clone()
            },
            FileEntry {
                name: "other.txt".into(),
                ..base.clone()
            },
            FileEntry {
                extension: "md".into(),
                ..base
            },
        ];

        for variant in variants {
            assert_ne!(base_fingerprint, fingerprint_for_file(variant));
        }
    }

    #[test]
    fn valid_png_yields_dimensions_two_pending_resources_safe_names_and_bounded_thumbnail() {
        let raw = RawClipboardSnapshot {
            png: Some(rgba8_png(640, 480)),
            ..RawClipboardSnapshot::default()
        };

        let normalized = accepted(normalize(raw, &AppSettings::default(), 100).unwrap());
        let fingerprint = normalized.item.fingerprint.clone();
        let (png_path, thumbnail_path, width, height) = match &normalized.item.payload {
            ClipboardPayload::Image {
                png_path,
                thumbnail_path,
                width,
                height,
            } => (png_path, thumbnail_path, *width, *height),
            payload => panic!("expected image payload, got {payload:?}"),
        };

        assert_eq!((width, height), (640, 480));
        assert_eq!(png_path, &format!("{fingerprint}.png"));
        assert_eq!(thumbnail_path, &format!("{fingerprint}-thumb.png"));
        assert_eq!(normalized.image_resources.len(), 2);
        assert_eq!(normalized.image_resources[0].file_name, *png_path);
        assert_eq!(normalized.image_resources[1].file_name, *thumbnail_path);
        for resource in &normalized.image_resources {
            assert!(resource.file_name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
            }));
            assert_eq!(&resource.bytes[..8], PNG_SIGNATURE);
        }
        assert_eq!(
            png_dimensions(&normalized.image_resources[0].bytes),
            (640, 480)
        );
        let (thumbnail_width, thumbnail_height) =
            png_dimensions(&normalized.image_resources[1].bytes);
        assert_eq!((thumbnail_width, thumbnail_height), (320, 240));
    }

    #[test]
    fn rgba16_conversion_rejects_tight_peak_budget_and_accepts_adequate_budget() {
        let png = rgba16_png(600, 600);
        assert_eq!(png[24], 16);
        let tight = normalize(
            RawClipboardSnapshot {
                png: Some(png.clone()),
                ..RawClipboardSnapshot::default()
            },
            &settings_with_limit(3 * 1024 * 1024),
            0,
        )
        .unwrap_err();
        let adequate = accepted(
            normalize(
                RawClipboardSnapshot {
                    png: Some(png),
                    ..RawClipboardSnapshot::default()
                },
                &settings_with_limit(4 * 1024 * 1024),
                0,
            )
            .unwrap(),
        );

        assert_eq!(tight, AppError::InvalidClipboardImage);
        assert_eq!(adequate.image_resources[0].bytes[24], 8);
    }

    #[test]
    fn invalid_or_corrupt_image_returns_invalid_clipboard_image() {
        let error = normalize(
            RawClipboardSnapshot {
                png: Some(vec![0xff, 0x00, 0x55]),
                ..RawClipboardSnapshot::default()
            },
            &AppSettings::default(),
            0,
        )
        .unwrap_err();

        assert_eq!(error, AppError::InvalidClipboardImage);
        assert_eq!(error.code(), "invalidClipboardImage");
        assert_eq!(
            serde_json::to_value(error).unwrap()["code"],
            "invalidClipboardImage"
        );
    }

    #[test]
    fn decompression_dimension_and_allocation_guard_rejects_hostile_header_without_huge_allocation()
    {
        let error = normalize(
            RawClipboardSnapshot {
                dib: Some(hostile_bmp_header(20_000, 20_000)),
                ..RawClipboardSnapshot::default()
            },
            &AppSettings::default(),
            0,
        )
        .unwrap_err();

        assert_eq!(error, AppError::InvalidClipboardImage);
    }

    #[test]
    fn allocation_guard_rejects_hostile_header_below_dimension_ceiling() {
        let error = normalize(
            RawClipboardSnapshot {
                dib: Some(hostile_bmp_header(8_000, 8_000)),
                ..RawClipboardSnapshot::default()
            },
            &settings_with_limit(8 * 1024 * 1024),
            0,
        )
        .unwrap_err();

        assert_eq!(error, AppError::InvalidClipboardImage);
    }

    #[test]
    fn checked_file_and_text_size_overflow_does_not_panic_and_returns_oversize() {
        let outcome = normalize(
            RawClipboardSnapshot {
                files: vec![file("max", "max", u64::MAX), file("one", "one", 1)],
                ..RawClipboardSnapshot::default()
            },
            &AppSettings::default(),
            0,
        )
        .unwrap();

        assert_eq!(
            outcome,
            NormalizeOutcome::IgnoredOversize {
                measured_bytes: u64::MAX,
                limit_bytes: AppSettings::default().max_item_bytes,
            }
        );
        assert_eq!(checked_sum([u64::MAX, 1]), None);
    }

    #[test]
    fn empty_snapshot_and_empty_representations_are_ignored() {
        assert_eq!(
            normalize(RawClipboardSnapshot::default(), &AppSettings::default(), 0).unwrap(),
            NormalizeOutcome::IgnoredEmpty
        );
        assert_eq!(
            normalize(
                RawClipboardSnapshot {
                    plain_text: Some(String::new()),
                    html: Some(String::new()),
                    rtf: Some(Vec::new()),
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
            NormalizeOutcome::IgnoredEmpty
        );
    }

    #[test]
    fn unicode_preview_truncation_does_not_split_or_panic() {
        let text = "😀".repeat(500);
        let normalized = accepted(
            normalize(
                RawClipboardSnapshot {
                    plain_text: Some(text),
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
        );
        let files = accepted(
            normalize(
                RawClipboardSnapshot {
                    files: vec![file("unicode", &"文".repeat(500), 0)],
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
        );

        assert!(normalized.item.preview.chars().count() <= 120);
        assert!(
            normalized
                .item
                .preview
                .is_char_boundary(normalized.item.preview.len())
        );
        assert!(files.item.preview.chars().count() <= 120);
        assert!(
            files
                .item
                .preview
                .is_char_boundary(files.item.preview.len())
        );
    }

    #[test]
    fn preview_truncation_keeps_combining_grapheme_whole_at_limit() {
        let expected = format!("{}e\u{301}", "a".repeat(119));
        let normalized = accepted(
            normalize(
                RawClipboardSnapshot {
                    plain_text: Some(format!("{expected}tail")),
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
        );

        assert_eq!(normalized.item.preview, expected);
    }

    #[test]
    fn preview_truncation_keeps_zwj_family_emoji_whole_at_limit() {
        let family = "👨‍👩‍👧‍👦";
        let expected = format!("{}{family}", "a".repeat(119));
        let normalized = accepted(
            normalize(
                RawClipboardSnapshot {
                    plain_text: Some(format!("{expected}tail")),
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
        );

        assert_eq!(normalized.item.preview, expected);
    }

    #[test]
    fn whitespace_collapse_preserves_grapheme_clusters_at_limit() {
        let family = "👨‍👩‍👧‍👦";
        let expected = format!("{} {family}", "a".repeat(118));
        let normalized = accepted(
            normalize(
                RawClipboardSnapshot {
                    plain_text: Some(format!("{}\n\t{family}tail", "a".repeat(118))),
                    ..RawClipboardSnapshot::default()
                },
                &AppSettings::default(),
                0,
            )
            .unwrap(),
        );

        assert_eq!(normalized.item.preview, expected);
    }
}
