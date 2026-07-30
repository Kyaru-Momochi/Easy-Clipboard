use std::{
    collections::HashSet,
    ffi::c_void,
    fs, io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use image::ImageFormat;
use windows::{
    Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::{
                AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
                GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
                RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{
                GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
            },
        },
        UI::{
            Shell::{DragQueryFileW, HDROP},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
                GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, MSG, PostQuitMessage, RegisterClassW,
                SendMessageW, SetWindowLongPtrW, UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE,
                WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY, WNDCLASSW,
            },
        },
    },
    core::PCWSTR,
};

use crate::{
    domain::{ClipboardPayload, FileEntry, MediaKind},
    error::AppError,
};

use super::{ClipboardBackend, RawClipboardSnapshot, normalize::normalized_windows_path};

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
const DROPFILES_HEADER_BYTES: usize = 20;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;
const BITMAPV5HEADER_BYTES: usize = 124;
const BITMAPFILEHEADER_BYTES: usize = 14;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILE_TREE_BYTES: u64 = 500 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 100_000;
const OPEN_RETRY_DELAYS_MS: [u64; 4] = [10, 20, 40, 80];
static LISTENER_CLASS_ID: AtomicU64 = AtomicU64::new(1);
static OWNER_CLASS_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug)]
pub struct WindowsClipboard {
    html_format: u32,
    rtf_format: u32,
    png_format: u32,
}

impl WindowsClipboard {
    pub fn new() -> Result<Self, AppError> {
        let html_format = register_clipboard_format(windows::core::w!("HTML Format"))?;
        let rtf_format = register_clipboard_format(windows::core::w!("Rich Text Format"))?;
        let png_format = register_clipboard_format(windows::core::w!("PNG"))?;
        Ok(Self {
            html_format,
            rtf_format,
            png_format,
        })
    }
}

impl ClipboardBackend for WindowsClipboard {
    fn read(&self, max_item_bytes: u64) -> Result<RawClipboardSnapshot, AppError> {
        let max_item_bytes = clamp_capture_limit(max_item_bytes);
        let clipboard = ClipboardGuard::open_for_read()?;
        let mut availability = ClipboardFormatAvailability {
            files: format_available(CF_HDROP),
            png: format_available(self.png_format),
            dib_v5: format_available(CF_DIBV5),
            dib: format_available(CF_DIB),
            plain_text: format_available(CF_UNICODETEXT),
        };
        loop {
            match choose_read_format(availability) {
                ClipboardReadFormat::Files => {
                    let paths = match read_file_paths(max_item_bytes)? {
                        ClipboardFilePaths::Paths(paths) => paths,
                        ClipboardFilePaths::Oversize(measured) => {
                            return Ok(RawClipboardSnapshot {
                                files_oversize_bytes: Some(measured),
                                ..RawClipboardSnapshot::default()
                            });
                        }
                    };
                    if !paths.is_empty() {
                        drop(clipboard);
                        let mut source = FsFileTreeSource;
                        return Ok(
                            match measure_file_roots_with_budget(
                                paths,
                                max_item_bytes,
                                MAX_DIRECTORY_ENTRIES,
                                &mut source,
                            ) {
                                FileRootsOutcome::Accepted(files) => RawClipboardSnapshot {
                                    files,
                                    ..RawClipboardSnapshot::default()
                                },
                                FileRootsOutcome::Oversize(measured) => RawClipboardSnapshot {
                                    files_oversize_bytes: Some(measured),
                                    ..RawClipboardSnapshot::default()
                                },
                            },
                        );
                    }
                    availability.files = false;
                }
                ClipboardReadFormat::Png => {
                    match read_optional_bytes(self.png_format, max_item_bytes)? {
                        ClipboardBytes::Data(png) => {
                            return Ok(RawClipboardSnapshot {
                                png: Some(png),
                                ..RawClipboardSnapshot::default()
                            });
                        }
                        ClipboardBytes::Oversize(measured) => {
                            return Ok(RawClipboardSnapshot {
                                image_oversize_bytes: Some(measured),
                                ..RawClipboardSnapshot::default()
                            });
                        }
                        ClipboardBytes::Missing => availability.png = false,
                    }
                }
                ClipboardReadFormat::DibV5 => match read_optional_dib(CF_DIBV5, max_item_bytes) {
                    Ok(ClipboardBytes::Data(dib)) => {
                        return Ok(RawClipboardSnapshot {
                            dib: Some(dib),
                            ..RawClipboardSnapshot::default()
                        });
                    }
                    Ok(ClipboardBytes::Oversize(measured)) => {
                        return Ok(RawClipboardSnapshot {
                            image_oversize_bytes: Some(measured),
                            ..RawClipboardSnapshot::default()
                        });
                    }
                    Ok(ClipboardBytes::Missing) | Err(_) => availability.dib_v5 = false,
                },
                ClipboardReadFormat::Dib => match read_optional_dib(CF_DIB, max_item_bytes) {
                    Ok(ClipboardBytes::Data(dib)) => {
                        return Ok(RawClipboardSnapshot {
                            dib: Some(dib),
                            ..RawClipboardSnapshot::default()
                        });
                    }
                    Ok(ClipboardBytes::Oversize(measured)) => {
                        return Ok(RawClipboardSnapshot {
                            image_oversize_bytes: Some(measured),
                            ..RawClipboardSnapshot::default()
                        });
                    }
                    Ok(ClipboardBytes::Missing) | Err(_) => availability.dib = false,
                },
                ClipboardReadFormat::Text => {
                    let mut budget = ClipboardByteBudget::new(max_item_bytes);
                    let plain_text = match read_unicode_text(CF_UNICODETEXT, budget.remaining())? {
                        Some(BudgetValue::Accepted {
                            value,
                            measured_bytes,
                        }) => {
                            if let Err(measured) = budget.claim(measured_bytes) {
                                return Ok(RawClipboardSnapshot {
                                    text_oversize_bytes: Some(measured),
                                    ..RawClipboardSnapshot::default()
                                });
                            }
                            value
                        }
                        Some(BudgetValue::Oversize(measured)) => {
                            return Ok(RawClipboardSnapshot {
                                text_oversize_bytes: Some(budget.measured_with(measured)),
                                ..RawClipboardSnapshot::default()
                            });
                        }
                        None => return Ok(RawClipboardSnapshot::default()),
                    };
                    let html =
                        match read_optional_custom_bytes(self.html_format, budget.remaining())? {
                            ClipboardBytes::Data(bytes) => {
                                if let Err(measured) =
                                    budget.claim(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                                {
                                    return Ok(RawClipboardSnapshot {
                                        text_oversize_bytes: Some(measured),
                                        ..RawClipboardSnapshot::default()
                                    });
                                }
                                Some(String::from_utf8(bytes).map_err(|_| AppError::Clipboard)?)
                            }
                            ClipboardBytes::Oversize(measured) => {
                                return Ok(RawClipboardSnapshot {
                                    text_oversize_bytes: Some(budget.measured_with(measured)),
                                    ..RawClipboardSnapshot::default()
                                });
                            }
                            ClipboardBytes::Missing => None,
                        };
                    let rtf = match read_optional_custom_bytes(self.rtf_format, budget.remaining())?
                    {
                        ClipboardBytes::Data(bytes) => {
                            if let Err(measured) =
                                budget.claim(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                            {
                                return Ok(RawClipboardSnapshot {
                                    text_oversize_bytes: Some(measured),
                                    ..RawClipboardSnapshot::default()
                                });
                            }
                            Some(bytes)
                        }
                        ClipboardBytes::Oversize(measured) => {
                            return Ok(RawClipboardSnapshot {
                                text_oversize_bytes: Some(budget.measured_with(measured)),
                                ..RawClipboardSnapshot::default()
                            });
                        }
                        ClipboardBytes::Missing => None,
                    };
                    return Ok(RawClipboardSnapshot {
                        plain_text: Some(plain_text),
                        html,
                        rtf,
                        ..RawClipboardSnapshot::default()
                    });
                }
                ClipboardReadFormat::Empty => return Ok(RawClipboardSnapshot::default()),
            }
        }
    }

    fn write(&self, payload: &ClipboardPayload) -> Result<u64, AppError> {
        let mut formats = self.prepare_formats(payload)?;
        let owner = ClipboardOwnerWindow::create()?;
        let _clipboard = ClipboardGuard::open_for_write(owner.hwnd())?;

        // SAFETY: the clipboard is open on this thread and all memory blocks are prepared before
        // its contents are cleared.
        unsafe { EmptyClipboard() }.map_err(|_| AppError::Clipboard)?;
        for format in &mut formats {
            format.publish()?;
        }

        // SAFETY: querying the process-wide clipboard sequence does not dereference memory. It is
        // intentionally read while the clipboard guard is still held, making this the receipt for
        // the exact write above rather than a later writer.
        let receipt = unsafe { GetClipboardSequenceNumber() };
        if receipt == 0 {
            return Err(AppError::Clipboard);
        }
        Ok(u64::from(receipt))
    }

    fn sequence_number(&self) -> Result<u64, AppError> {
        // SAFETY: querying the clipboard sequence has no pointer or ownership preconditions.
        let sequence = unsafe { GetClipboardSequenceNumber() };
        Ok(u64::from(sequence))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ClipboardFormatAvailability {
    files: bool,
    png: bool,
    dib_v5: bool,
    dib: bool,
    plain_text: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardReadFormat {
    Files,
    Png,
    DibV5,
    Dib,
    Text,
    Empty,
}

fn choose_read_format(availability: ClipboardFormatAvailability) -> ClipboardReadFormat {
    if availability.files {
        ClipboardReadFormat::Files
    } else if availability.png {
        ClipboardReadFormat::Png
    } else if availability.dib_v5 {
        ClipboardReadFormat::DibV5
    } else if availability.dib {
        ClipboardReadFormat::Dib
    } else if availability.plain_text {
        ClipboardReadFormat::Text
    } else {
        ClipboardReadFormat::Empty
    }
}

impl WindowsClipboard {
    fn prepare_formats(&self, payload: &ClipboardPayload) -> Result<Vec<PreparedFormat>, AppError> {
        match payload {
            ClipboardPayload::Text { plain, html, rtf } => {
                let mut formats = vec![PreparedFormat::new(
                    CF_UNICODETEXT,
                    &encode_utf16_text(plain)?,
                )?];
                if let Some(html) = html {
                    formats.push(PreparedFormat::new(
                        self.html_format,
                        &nul_terminated_bytes(html.as_bytes()),
                    )?);
                }
                if let Some(rtf) = rtf {
                    formats.push(PreparedFormat::new(
                        self.rtf_format,
                        &nul_terminated_bytes(rtf),
                    )?);
                }
                Ok(formats)
            }
            ClipboardPayload::Image { png_path, .. } => {
                let png = read_bounded_image(Path::new(png_path))?;
                let dib = png_to_dibv5(&png)?;
                Ok(vec![
                    PreparedFormat::new(self.png_format, &png)?,
                    PreparedFormat::new(CF_DIBV5, &dib)?,
                    PreparedFormat::new(CF_DIB, &dib)?,
                ])
            }
            ClipboardPayload::Files { entries } => {
                if entries.is_empty()
                    || entries
                        .iter()
                        .any(|entry| !entry.available || !Path::new(&entry.path).exists())
                {
                    return Err(AppError::ClipboardItemUnavailable);
                }
                let paths = entries
                    .iter()
                    .map(|entry| PathBuf::from(&entry.path))
                    .collect::<Vec<_>>();
                Ok(vec![PreparedFormat::new(
                    CF_HDROP,
                    &encode_drop_files(&paths)?,
                )?])
            }
        }
    }
}

struct ClipboardGuard;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardAccess {
    Read,
    Write(HWND),
}

impl ClipboardAccess {
    fn owner(self) -> Option<HWND> {
        match self {
            Self::Read => None,
            Self::Write(owner) => Some(owner),
        }
    }
}

impl ClipboardGuard {
    fn open_for_read() -> Result<Self, AppError> {
        Self::open(ClipboardAccess::Read)
    }

    fn open_for_write(owner: HWND) -> Result<Self, AppError> {
        Self::open(ClipboardAccess::Write(owner))
    }

    fn open(access: ClipboardAccess) -> Result<Self, AppError> {
        let owner = access.owner();
        // SAFETY: owner is either null for a read or a live window owned by the current thread.
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return Ok(Self);
        }
        for delay_ms in OPEN_RETRY_DELAYS_MS {
            thread::sleep(Duration::from_millis(delay_ms));
            // SAFETY: owner remains valid for every retry. A successful acquisition is paired
            // with CloseClipboard in this guard's Drop implementation.
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Ok(Self);
            }
        }
        Err(AppError::Clipboard)
    }
}

struct ClipboardOwnerWindow {
    hwnd: HWND,
    instance: HINSTANCE,
    class_name: Vec<u16>,
}

impl ClipboardOwnerWindow {
    fn create() -> Result<Self, AppError> {
        let class_id = OWNER_CLASS_ID.fetch_add(1, Ordering::Relaxed);
        let class_name = format!("EasyClipboardOwner.{class_id}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: null requests the module containing the current executable.
        let module = unsafe { GetModuleHandleW(None) }.map_err(|_| AppError::Clipboard)?;
        let instance = HINSTANCE(module.0);
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(clipboard_owner_window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: class_name remains live in the returned owner until the class is unregistered.
        if unsafe { RegisterClassW(&window_class) } == 0 {
            return Err(AppError::Clipboard);
        }
        // SAFETY: the registered class is live and HWND_MESSAGE creates a non-visible window on
        // the current writing thread.
        let hwnd = match unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            )
        } {
            Ok(hwnd) => hwnd,
            Err(_) => {
                // SAFETY: the class was registered above and has no live windows.
                let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(instance)) };
                return Err(AppError::Clipboard);
            }
        };
        Ok(Self {
            hwnd,
            instance,
            class_name,
        })
    }

    fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for ClipboardOwnerWindow {
    fn drop(&mut self) {
        // SAFETY: hwnd was created on this thread and the clipboard guard is dropped first.
        let _ = unsafe { DestroyWindow(self.hwnd) };
        // SAFETY: the only window using this unique class has just been destroyed.
        let _ = unsafe { UnregisterClassW(PCWSTR(self.class_name.as_ptr()), Some(self.instance)) };
    }
}

unsafe extern "system" fn clipboard_owner_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the owner window has no custom messages or state, so every message is forwarded.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: this guard only exists after OpenClipboard succeeded, and it is not clonable.
        let _ = unsafe { CloseClipboard() };
    }
}

struct OwnedGlobal {
    handle: Option<HGLOBAL>,
}

impl OwnedGlobal {
    fn from_bytes(bytes: &[u8]) -> Result<Self, AppError> {
        let allocation_size = bytes.len().max(1);
        // SAFETY: flags request a movable zero-initialized block; the returned handle is owned by
        // this value until a successful SetClipboardData transfers it.
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, allocation_size) }
            .map_err(|_| AppError::Clipboard)?;
        // SAFETY: handle is a live global allocation. We copy exactly bytes.len() bytes into the
        // allocation, which is at least that large.
        let pointer = unsafe { GlobalLock(handle) }.cast::<u8>();
        if pointer.is_null() {
            // SAFETY: handle has not been transferred and is still owned here.
            let _ = unsafe { GlobalFree(Some(handle)) };
            return Err(AppError::Clipboard);
        }
        // SAFETY: source and destination are valid for bytes.len() bytes and cannot overlap.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len()) };
        // SAFETY: this balances the successful GlobalLock. A false Win32 result also means the
        // lock count reached zero, so no error handling is required here.
        let _ = unsafe { GlobalUnlock(handle) };
        Ok(Self {
            handle: Some(handle),
        })
    }

    fn transfer(&mut self, format: u32) -> Result<(), AppError> {
        let handle = self.handle.ok_or(AppError::Clipboard)?;
        // SAFETY: clipboard is open and emptied by the caller; handle is a GMEM_MOVEABLE block
        // still owned by this value.
        unsafe { SetClipboardData(format, Some(HANDLE(handle.0))) }
            .map_err(|_| AppError::Clipboard)?;
        self.handle = None;
        Ok(())
    }
}

impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: only untransferred handles remain in this value.
            let _ = unsafe { GlobalFree(Some(handle)) };
        }
    }
}

struct PreparedFormat {
    format: u32,
    memory: OwnedGlobal,
}

impl PreparedFormat {
    fn new(format: u32, bytes: &[u8]) -> Result<Self, AppError> {
        Ok(Self {
            format,
            memory: OwnedGlobal::from_bytes(bytes)?,
        })
    }

    fn publish(&mut self) -> Result<(), AppError> {
        self.memory.transfer(self.format)
    }
}

fn register_clipboard_format(name: PCWSTR) -> Result<u32, AppError> {
    // SAFETY: name points to a static NUL-terminated UTF-16 string.
    let format = unsafe { RegisterClipboardFormatW(name) };
    if format == 0 {
        Err(AppError::Clipboard)
    } else {
        Ok(format)
    }
}

fn format_available(format: u32) -> bool {
    // SAFETY: format is a clipboard format identifier, with no pointer preconditions.
    unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ByteReadPlan {
    Copy(usize),
    Oversize(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BudgetValue<T> {
    Accepted { value: T, measured_bytes: u64 },
    Oversize(u64),
}

struct ClipboardByteBudget {
    max_bytes: u64,
    measured_bytes: u64,
}

impl ClipboardByteBudget {
    fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            measured_bytes: 0,
        }
    }

    fn remaining(&self) -> u64 {
        self.max_bytes.saturating_sub(self.measured_bytes)
    }

    fn claim(&mut self, bytes: u64) -> Result<(), u64> {
        let measured = self.measured_bytes.saturating_add(bytes);
        if measured > self.max_bytes {
            Err(measured.max(self.max_bytes.saturating_add(1)))
        } else {
            self.measured_bytes = measured;
            Ok(())
        }
    }

    fn measured_with(&self, bytes: u64) -> u64 {
        self.measured_bytes
            .saturating_add(bytes)
            .max(self.max_bytes.saturating_add(1))
    }
}

fn clamp_capture_limit(max_item_bytes: u64) -> u64 {
    max_item_bytes.clamp(1, MAX_FILE_TREE_BYTES)
}

fn decode_utf16_with_budget(
    units: &[u16],
    max_item_bytes: u64,
) -> Result<BudgetValue<String>, AppError> {
    decode_utf16_with_budget_counted(units, max_item_bytes).map(|(value, _)| value)
}

fn decode_utf16_with_budget_counted(
    units: &[u16],
    max_item_bytes: u64,
) -> Result<(BudgetValue<String>, usize), AppError> {
    let mut text = String::new();
    let mut measured_bytes = 0_u64;
    let mut inspected_units = 0_usize;
    let mut index = 0_usize;

    while index < units.len() {
        let unit = units[index];
        inspected_units = inspected_units.saturating_add(1);
        index = index.saturating_add(1);
        if unit == 0 {
            break;
        }

        let character = match unit {
            0xd800..=0xdbff => {
                let Some(&low) = units.get(index) else {
                    return Err(AppError::Clipboard);
                };
                inspected_units = inspected_units.saturating_add(1);
                index = index.saturating_add(1);
                if !(0xdc00..=0xdfff).contains(&low) {
                    return Err(AppError::Clipboard);
                }
                let scalar =
                    0x1_0000 + (((u32::from(unit) - 0xd800) << 10) | (u32::from(low) - 0xdc00));
                char::from_u32(scalar).ok_or(AppError::Clipboard)?
            }
            0xdc00..=0xdfff => return Err(AppError::Clipboard),
            _ => char::from_u32(u32::from(unit)).ok_or(AppError::Clipboard)?,
        };

        measured_bytes = measured_bytes.saturating_add(character.len_utf8() as u64);
        if measured_bytes > max_item_bytes {
            return Ok((
                BudgetValue::Oversize(measured_bytes.max(max_item_bytes.saturating_add(1))),
                inspected_units,
            ));
        }
        text.push(character);
    }

    Ok((
        BudgetValue::Accepted {
            value: text,
            measured_bytes,
        },
        inspected_units,
    ))
}

fn byte_read_plan(size: usize, max_copy_bytes: usize) -> ByteReadPlan {
    if size > max_copy_bytes {
        ByteReadPlan::Oversize(u64::try_from(size).unwrap_or(u64::MAX))
    } else {
        ByteReadPlan::Copy(size)
    }
}

enum ClipboardBytes {
    Missing,
    Data(Vec<u8>),
    Oversize(u64),
}

struct ClipboardGlobalMemory {
    handle: HGLOBAL,
    size: usize,
}

impl ClipboardGlobalMemory {
    fn get(format: u32) -> Result<Option<Self>, AppError> {
        if !format_available(format) {
            return Ok(None);
        }
        // SAFETY: clipboard is held open by the caller. The returned handle remains owned by the
        // clipboard and is never freed here.
        let handle = unsafe { GetClipboardData(format) }.map_err(|_| AppError::Clipboard)?;
        let handle = HGLOBAL(handle.0);
        // SAFETY: supported formats in this adapter are global-memory clipboard blocks.
        let size = unsafe { GlobalSize(handle) };
        if size == 0 {
            return Err(AppError::Clipboard);
        }
        Ok(Some(Self { handle, size }))
    }

    fn lock(&self) -> Result<ClipboardGlobalLock, AppError> {
        // SAFETY: handle is a live clipboard-owned global allocation while the clipboard is open.
        let pointer = unsafe { GlobalLock(self.handle) }.cast::<u8>();
        if pointer.is_null() {
            return Err(AppError::Clipboard);
        }
        Ok(ClipboardGlobalLock {
            handle: self.handle,
            pointer,
            size: self.size,
        })
    }
}

struct ClipboardGlobalLock {
    handle: HGLOBAL,
    pointer: *const u8,
    size: usize,
}

impl ClipboardGlobalLock {
    fn bytes(&self) -> &[u8] {
        // SAFETY: pointer was returned by GlobalLock and remains valid for GlobalSize bytes until
        // this guard unlocks it.
        unsafe { std::slice::from_raw_parts(self.pointer, self.size) }
    }
}

impl Drop for ClipboardGlobalLock {
    fn drop(&mut self) {
        // SAFETY: balances the successful GlobalLock used to construct this guard.
        let _ = unsafe { GlobalUnlock(self.handle) };
    }
}

fn read_optional_bytes(format: u32, max_copy_bytes: u64) -> Result<ClipboardBytes, AppError> {
    let Some(memory) = ClipboardGlobalMemory::get(format)? else {
        return Ok(ClipboardBytes::Missing);
    };
    let max_copy_bytes = usize::try_from(max_copy_bytes).unwrap_or(usize::MAX);
    let copy_size = match byte_read_plan(memory.size, max_copy_bytes) {
        ByteReadPlan::Copy(size) => size,
        ByteReadPlan::Oversize(measured) => return Ok(ClipboardBytes::Oversize(measured)),
    };
    let lock = memory.lock()?;
    let mut bytes = vec![0_u8; copy_size];
    bytes.copy_from_slice(lock.bytes());
    Ok(ClipboardBytes::Data(bytes))
}

fn read_optional_custom_bytes(
    format: u32,
    remaining_bytes: u64,
) -> Result<ClipboardBytes, AppError> {
    let Some(memory) = ClipboardGlobalMemory::get(format)? else {
        return Ok(ClipboardBytes::Missing);
    };
    let lock = memory.lock()?;
    let (value, _) = decode_custom_bytes_with_budget_counted(lock.bytes(), remaining_bytes);
    Ok(match value {
        BudgetValue::Accepted { value, .. } => ClipboardBytes::Data(value),
        BudgetValue::Oversize(measured) => ClipboardBytes::Oversize(measured),
    })
}

fn decode_custom_bytes_with_budget_counted(
    bytes: &[u8],
    remaining_bytes: u64,
) -> (BudgetValue<Vec<u8>>, usize) {
    let window_limit = usize::try_from(remaining_bytes.saturating_add(1)).unwrap_or(usize::MAX);
    let window_len = bytes.len().min(window_limit);

    for (index, byte) in bytes[..window_len].iter().enumerate() {
        if *byte == 0 {
            return (
                BudgetValue::Accepted {
                    value: bytes[..index].to_vec(),
                    measured_bytes: u64::try_from(index).unwrap_or(u64::MAX),
                },
                index.saturating_add(1),
            );
        }
    }

    let total_size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if total_size > remaining_bytes {
        (
            BudgetValue::Oversize(remaining_bytes.saturating_add(1)),
            window_len,
        )
    } else {
        (
            BudgetValue::Accepted {
                value: bytes.to_vec(),
                measured_bytes: total_size,
            },
            window_len,
        )
    }
}

fn read_unicode_text(
    format: u32,
    max_item_bytes: u64,
) -> Result<Option<BudgetValue<String>>, AppError> {
    let Some(memory) = ClipboardGlobalMemory::get(format)? else {
        return Ok(None);
    };
    if !memory.size.is_multiple_of(2) {
        return Err(AppError::Clipboard);
    }
    let lock = memory.lock()?;
    let unit_count = memory.size / 2;
    // SAFETY: GlobalLock returns suitably aligned global memory; the byte size is even and the
    // lock remains live for this slice's complete use.
    let units = unsafe { std::slice::from_raw_parts(lock.pointer.cast::<u16>(), unit_count) };
    decode_utf16_with_budget(units, max_item_bytes).map(Some)
}

fn read_optional_dib(format: u32, max_item_bytes: u64) -> Result<ClipboardBytes, AppError> {
    let Some(memory) = ClipboardGlobalMemory::get(format)? else {
        return Ok(ClipboardBytes::Missing);
    };
    let lock = memory.lock()?;
    let dib = lock.bytes();
    match dib_read_plan(dib, max_item_bytes)? {
        ByteReadPlan::Oversize(measured) => return Ok(ClipboardBytes::Oversize(measured)),
        ByteReadPlan::Copy(_) => {}
    }
    let layout = dib_layout(dib)?;
    Ok(ClipboardBytes::Data(build_bmp(dib, layout)?))
}

enum ClipboardFilePaths {
    Paths(Vec<PathBuf>),
    Oversize(u64),
}

fn read_file_paths(max_item_bytes: u64) -> Result<ClipboardFilePaths, AppError> {
    if !format_available(CF_HDROP) {
        return Ok(ClipboardFilePaths::Paths(Vec::new()));
    }
    // SAFETY: clipboard is held open by the caller; this handle remains clipboard-owned.
    let handle = unsafe { GetClipboardData(CF_HDROP) }.map_err(|_| AppError::Clipboard)?;
    let drop_handle = HDROP(handle.0);
    // SAFETY: a CF_HDROP handle can be queried with u32::MAX to obtain its item count.
    let count = unsafe { DragQueryFileW(drop_handle, u32::MAX, None) };
    let count_usize = usize::try_from(count).map_err(|_| AppError::Clipboard)?;
    if root_count_plan(count_usize, MAX_DIRECTORY_ENTRIES).is_err() {
        return Ok(ClipboardFilePaths::Oversize(
            max_item_bytes.saturating_add(1),
        ));
    }
    let mut paths = Vec::with_capacity(count_usize);
    for index in 0..count {
        // SAFETY: querying an in-range item with no buffer returns its UTF-16 length.
        let length = unsafe { DragQueryFileW(drop_handle, index, None) };
        let buffer_len = usize::try_from(length)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or(AppError::Clipboard)?;
        let mut buffer = vec![0_u16; buffer_len];
        // SAFETY: buffer includes room for the terminating NUL and index is below count.
        let copied = unsafe { DragQueryFileW(drop_handle, index, Some(&mut buffer)) };
        if copied != length {
            return Err(AppError::Clipboard);
        }
        let copied = usize::try_from(copied).map_err(|_| AppError::Clipboard)?;
        paths.push(PathBuf::from(std::ffi::OsString::from_wide(
            &buffer[..copied],
        )));
    }
    Ok(ClipboardFilePaths::Paths(paths))
}

#[cfg(test)]
fn decode_nul_terminated_utf16(units: &[u16]) -> Result<String, AppError> {
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16(&units[..end]).map_err(|_| AppError::Clipboard)
}

fn encode_utf16_text(text: &str) -> Result<Vec<u8>, AppError> {
    if text.encode_utf16().any(|unit| unit == 0) {
        return Err(AppError::Clipboard);
    }
    let mut bytes = Vec::with_capacity(text.len().saturating_mul(2).saturating_add(2));
    for unit in text.encode_utf16().chain(std::iter::once(0)) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(bytes)
}

fn nul_terminated_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut terminated = bytes.to_vec();
    if terminated.last() != Some(&0) {
        terminated.push(0);
    }
    terminated
}

fn encode_drop_files(paths: &[PathBuf]) -> Result<Vec<u8>, AppError> {
    if paths.is_empty() {
        return Err(AppError::Clipboard);
    }
    let mut bytes = Vec::with_capacity(DROPFILES_HEADER_BYTES + paths.len() * 64);
    bytes.extend_from_slice(&(DROPFILES_HEADER_BYTES as u32).to_le_bytes());
    bytes.extend_from_slice(&0_i32.to_le_bytes());
    bytes.extend_from_slice(&0_i32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&1_i32.to_le_bytes());
    for path in paths {
        let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if units.is_empty() || units.contains(&0) {
            return Err(AppError::Clipboard);
        }
        for unit in units.into_iter().chain(std::iter::once(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    Ok(bytes)
}

#[cfg(test)]
fn parse_drop_files(bytes: &[u8]) -> Result<Vec<PathBuf>, AppError> {
    if bytes.len() < DROPFILES_HEADER_BYTES {
        return Err(AppError::Clipboard);
    }
    let offset = usize::try_from(u32::from_le_bytes(
        bytes[0..4].try_into().map_err(|_| AppError::Clipboard)?,
    ))
    .map_err(|_| AppError::Clipboard)?;
    let wide = i32::from_le_bytes(bytes[16..20].try_into().map_err(|_| AppError::Clipboard)?);
    if wide == 0 || offset < DROPFILES_HEADER_BYTES || offset > bytes.len() {
        return Err(AppError::Clipboard);
    }
    let payload = &bytes[offset..];
    if !payload.len().is_multiple_of(2) {
        return Err(AppError::Clipboard);
    }
    let units = payload
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    if units.len() < 2 || units[units.len() - 2..] != [0, 0] {
        return Err(AppError::Clipboard);
    }
    let mut paths = Vec::new();
    let mut start = 0;
    while start < units.len() {
        let relative_end = units[start..]
            .iter()
            .position(|unit| *unit == 0)
            .ok_or(AppError::Clipboard)?;
        if relative_end == 0 {
            break;
        }
        let end = start + relative_end;
        paths.push(PathBuf::from(std::ffi::OsString::from_wide(
            &units[start..end],
        )));
        start = end + 1;
    }
    if paths.is_empty() {
        Err(AppError::Clipboard)
    } else {
        Ok(paths)
    }
}

#[derive(Clone, Copy)]
struct BmpLayout {
    file_size: usize,
    pixel_offset: usize,
}

fn dib_layout(dib: &[u8]) -> Result<BmpLayout, AppError> {
    if dib.len() < 40 {
        return Err(AppError::Clipboard);
    }
    let header_size = read_u32(dib, 0)?;
    let header_size_usize = usize::try_from(header_size).map_err(|_| AppError::Clipboard)?;
    if header_size < 40 || header_size_usize > dib.len() {
        return Err(AppError::Clipboard);
    }
    let bit_count = u32::from(read_u16(dib, 14)?);
    let compression = read_u32(dib, 16)?;
    let colors_used = read_u32(dib, 32)?;
    let mask_bytes = if header_size == 40 {
        match compression {
            BI_BITFIELDS => 12_usize,
            BI_ALPHABITFIELDS => 16_usize,
            _ => 0,
        }
    } else {
        0
    };
    let palette_entries = if bit_count <= 8 {
        if colors_used == 0 {
            1_u32.checked_shl(bit_count).ok_or(AppError::Clipboard)?
        } else {
            colors_used
        }
    } else {
        colors_used
    };
    let palette_bytes = usize::try_from(palette_entries)
        .ok()
        .and_then(|entries| entries.checked_mul(4))
        .ok_or(AppError::Clipboard)?;
    let dib_pixel_offset = header_size_usize
        .checked_add(mask_bytes)
        .and_then(|offset| offset.checked_add(palette_bytes))
        .ok_or(AppError::Clipboard)?;
    if dib_pixel_offset > dib.len() {
        return Err(AppError::Clipboard);
    }
    let file_size = BITMAPFILEHEADER_BYTES
        .checked_add(dib.len())
        .ok_or(AppError::Clipboard)?;
    let pixel_offset = BITMAPFILEHEADER_BYTES
        .checked_add(dib_pixel_offset)
        .ok_or(AppError::Clipboard)?;
    u32::try_from(file_size).map_err(|_| AppError::Clipboard)?;
    u32::try_from(pixel_offset).map_err(|_| AppError::Clipboard)?;
    Ok(BmpLayout {
        file_size,
        pixel_offset,
    })
}

fn dib_read_plan(dib: &[u8], max_item_bytes: u64) -> Result<ByteReadPlan, AppError> {
    let layout = dib_layout(dib)?;
    Ok(byte_read_plan(
        layout.file_size,
        usize::try_from(max_item_bytes).unwrap_or(usize::MAX),
    ))
}

fn build_bmp(dib: &[u8], layout: BmpLayout) -> Result<Vec<u8>, AppError> {
    let file_size_u32 = u32::try_from(layout.file_size).map_err(|_| AppError::Clipboard)?;
    let pixel_offset_u32 = u32::try_from(layout.pixel_offset).map_err(|_| AppError::Clipboard)?;
    let mut bmp = Vec::with_capacity(layout.file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size_u32.to_le_bytes());
    bmp.extend_from_slice(&0_u16.to_le_bytes());
    bmp.extend_from_slice(&0_u16.to_le_bytes());
    bmp.extend_from_slice(&pixel_offset_u32.to_le_bytes());
    bmp.extend_from_slice(dib);
    Ok(bmp)
}

#[cfg(test)]
fn dib_to_bmp(dib: &[u8]) -> Result<Vec<u8>, AppError> {
    build_bmp(dib, dib_layout(dib)?)
}

fn png_to_dibv5(png: &[u8]) -> Result<Vec<u8>, AppError> {
    let (width, height) = validated_png_dimensions(png)?;
    let image = image::load_from_memory_with_format(png, ImageFormat::Png)
        .map_err(|_| AppError::InvalidClipboardImage)?
        .into_rgba8();
    if image.width() != width || image.height() != height {
        return Err(AppError::InvalidClipboardImage);
    }
    let pixel_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(AppError::InvalidClipboardImage)?;
    let capacity = BITMAPV5HEADER_BYTES
        .checked_add(usize::try_from(pixel_bytes).map_err(|_| AppError::InvalidClipboardImage)?)
        .ok_or(AppError::InvalidClipboardImage)?;
    let mut dib = vec![0_u8; BITMAPV5HEADER_BYTES];
    dib[0..4].copy_from_slice(&(BITMAPV5HEADER_BYTES as u32).to_le_bytes());
    dib[4..8].copy_from_slice(
        &i32::try_from(width)
            .map_err(|_| AppError::InvalidClipboardImage)?
            .to_le_bytes(),
    );
    dib[8..12].copy_from_slice(
        &i32::try_from(height)
            .map_err(|_| AppError::InvalidClipboardImage)?
            .checked_neg()
            .ok_or(AppError::InvalidClipboardImage)?
            .to_le_bytes(),
    );
    dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
    dib[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
    dib[20..24].copy_from_slice(
        &u32::try_from(pixel_bytes)
            .map_err(|_| AppError::InvalidClipboardImage)?
            .to_le_bytes(),
    );
    dib[40..44].copy_from_slice(&0x00ff_0000_u32.to_le_bytes());
    dib[44..48].copy_from_slice(&0x0000_ff00_u32.to_le_bytes());
    dib[48..52].copy_from_slice(&0x0000_00ff_u32.to_le_bytes());
    dib[52..56].copy_from_slice(&0xff00_0000_u32.to_le_bytes());
    dib[56..60].copy_from_slice(&0x7352_4742_u32.to_le_bytes());
    dib.reserve(capacity - BITMAPV5HEADER_BYTES);
    for pixel in image.pixels() {
        dib.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    Ok(dib)
}

fn validated_png_dimensions(png: &[u8]) -> Result<(u32, u32), AppError> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || &png[..8] != PNG_SIGNATURE || &png[12..16] != b"IHDR" {
        return Err(AppError::InvalidClipboardImage);
    }
    let width = u32::from_be_bytes(
        png[16..20]
            .try_into()
            .map_err(|_| AppError::InvalidClipboardImage)?,
    );
    let height = u32::from_be_bytes(
        png[20..24]
            .try_into()
            .map_err(|_| AppError::InvalidClipboardImage)?,
    );
    let decoded_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(AppError::InvalidClipboardImage)?;
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || decoded_bytes > MAX_IMAGE_BYTES
    {
        return Err(AppError::InvalidClipboardImage);
    }
    Ok((width, height))
}

fn read_bounded_image(path: &Path) -> Result<Vec<u8>, AppError> {
    let metadata = fs::metadata(path).map_err(|_| AppError::ClipboardItemUnavailable)?;
    if metadata.len() > MAX_FILE_TREE_BYTES {
        return Err(AppError::InvalidClipboardImage);
    }
    fs::read(path).map_err(|_| AppError::ClipboardItemUnavailable)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, AppError> {
    let end = offset.checked_add(2).ok_or(AppError::Clipboard)?;
    let slice = bytes.get(offset..end).ok_or(AppError::Clipboard)?;
    Ok(u16::from_le_bytes(
        slice.try_into().map_err(|_| AppError::Clipboard)?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, AppError> {
    let end = offset.checked_add(4).ok_or(AppError::Clipboard)?;
    let slice = bytes.get(offset..end).ok_or(AppError::Clipboard)?;
    Ok(u32::from_le_bytes(
        slice.try_into().map_err(|_| AppError::Clipboard)?,
    ))
}

fn make_file_entry(path: PathBuf, size_bytes: u64, available: bool) -> FileEntry {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    FileEntry {
        path: path.to_string_lossy().into_owned(),
        name,
        extension,
        size_bytes,
        media_kind: media_kind_for_path(&path),
        available,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TreeNode {
    File(u64),
    Directory,
    Reparse,
    Unavailable,
}

trait FileTreeSource {
    fn inspect(&mut self, path: &Path) -> io::Result<TreeNode>;
    fn children(&mut self, path: &Path, max_children: usize) -> io::Result<Vec<PathBuf>>;
}

struct FsFileTreeSource;

impl FileTreeSource for FsFileTreeSource {
    fn inspect(&mut self, path: &Path) -> io::Result<TreeNode> {
        let metadata = fs::symlink_metadata(path)?;
        if is_reparse_or_symlink(&metadata) {
            Ok(TreeNode::Reparse)
        } else if metadata.is_file() {
            Ok(TreeNode::File(metadata.len()))
        } else if metadata.is_dir() {
            Ok(TreeNode::Directory)
        } else {
            Ok(TreeNode::Unavailable)
        }
    }

    fn children(&mut self, path: &Path, max_children: usize) -> io::Result<Vec<PathBuf>> {
        let mut children = Vec::with_capacity(max_children.min(256));
        for entry in fs::read_dir(path)? {
            children.push(entry?.path());
            if children.len() == max_children {
                break;
            }
        }
        Ok(children)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FileRootsOutcome {
    Accepted(Vec<FileEntry>),
    Oversize(u64),
}

struct FileTraversalBudget {
    max_bytes: u64,
    measured_bytes: u64,
    max_items: usize,
    visited_items: usize,
}

impl FileTraversalBudget {
    fn visit(&mut self) -> Result<(), u64> {
        self.visited_items = self.visited_items.saturating_add(1);
        if self.visited_items > self.max_items {
            Err(self.max_bytes.saturating_add(1))
        } else {
            Ok(())
        }
    }

    fn add_bytes(&mut self, bytes: u64) -> Result<(), u64> {
        let measured = self.measured_bytes.saturating_add(bytes);
        if measured > self.max_bytes {
            self.measured_bytes = self.max_bytes.saturating_add(1);
            Err(self.measured_bytes)
        } else {
            self.measured_bytes = measured;
            Ok(())
        }
    }

    fn remaining_items(&self) -> usize {
        self.max_items.saturating_sub(self.visited_items)
    }
}

fn measure_file_roots_with_budget<W: FileTreeSource>(
    paths: Vec<PathBuf>,
    max_bytes: u64,
    max_items: usize,
    source: &mut W,
) -> FileRootsOutcome {
    let mut budget = FileTraversalBudget {
        max_bytes,
        measured_bytes: 0,
        max_items,
        visited_items: 0,
    };
    let mut identities = HashSet::new();
    let mut entries = Vec::new();

    for path in paths {
        if let Err(measured) = budget.visit() {
            return FileRootsOutcome::Oversize(measured);
        }
        let identity = normalized_windows_path(&path.to_string_lossy());
        if !identities.insert(identity) {
            continue;
        }
        let root = match source.inspect(&path) {
            Ok(root) => root,
            Err(_) => {
                entries.push(make_file_entry(path, 0, false));
                continue;
            }
        };
        match root {
            TreeNode::File(size) => {
                if let Err(measured) = budget.add_bytes(size) {
                    return FileRootsOutcome::Oversize(measured);
                }
                entries.push(make_file_entry(path, size, true));
            }
            TreeNode::Directory => {
                let size = match measure_directory(&path, &mut budget, source) {
                    Ok(size) => size,
                    Err(measured) => return FileRootsOutcome::Oversize(measured),
                };
                entries.push(make_file_entry(path, size, true));
            }
            TreeNode::Reparse | TreeNode::Unavailable => {
                entries.push(make_file_entry(path, 0, false));
            }
        }
    }
    FileRootsOutcome::Accepted(entries)
}

fn measure_directory<W: FileTreeSource>(
    root: &Path,
    budget: &mut FileTraversalBudget,
    source: &mut W,
) -> Result<u64, u64> {
    let starting_total = budget.measured_bytes;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let remaining = budget.remaining_items();
        let request_count = remaining.saturating_add(1);
        let children = source
            .children(&directory, request_count)
            .map_err(|_| budget.max_bytes.saturating_add(1))?;
        if children.len() > remaining {
            return Err(budget.max_bytes.saturating_add(1));
        }
        for child in children {
            budget.visit()?;
            match source
                .inspect(&child)
                .map_err(|_| budget.max_bytes.saturating_add(1))?
            {
                TreeNode::File(size) => budget.add_bytes(size)?,
                TreeNode::Directory => pending.push(child),
                TreeNode::Reparse | TreeNode::Unavailable => {}
            }
        }
    }
    Ok(budget.measured_bytes.saturating_sub(starting_total))
}

fn root_count_plan(count: usize, max_items: usize) -> Result<usize, ()> {
    if count > max_items {
        Err(())
    } else {
        Ok(count)
    }
}

#[cfg(test)]
fn capped_path_size(path: &Path, cap: u64) -> io::Result<u64> {
    let mut source = FsFileTreeSource;
    match measure_file_roots_with_budget(
        vec![path.to_path_buf()],
        cap,
        MAX_DIRECTORY_ENTRIES,
        &mut source,
    ) {
        FileRootsOutcome::Accepted(entries) => {
            Ok(entries.first().map(|entry| entry.size_bytes).unwrap_or(0))
        }
        FileRootsOutcome::Oversize(measured) => Ok(measured),
    }
}

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn media_kind_for_path(path: &Path) -> MediaKind {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(
        extension.as_str(),
        "aac" | "aiff" | "alac" | "flac" | "m4a" | "mp3" | "ogg" | "opus" | "wav" | "wma"
    ) {
        MediaKind::Audio
    } else if matches!(
        extension.as_str(),
        "avi" | "flv" | "m4v" | "mkv" | "mov" | "mp4" | "mpeg" | "mpg" | "webm" | "wmv"
    ) {
        MediaKind::Video
    } else {
        MediaKind::Other
    }
}

pub struct ClipboardListener {
    hwnd: isize,
    window_thread: Option<JoinHandle<()>>,
}

impl ClipboardListener {
    pub fn start() -> Result<(Self, mpsc::Receiver<()>), AppError> {
        let (updates_tx, updates_rx) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let window_thread = match thread::Builder::new()
            .name("easy-clipboard-listener".into())
            .spawn(move || {
                let _ = run_listener_window(updates_tx, ready_tx);
            }) {
            Ok(thread) => thread,
            Err(_) => return Err(AppError::Clipboard),
        };
        let hwnd = match ready_rx.recv() {
            Ok(Ok(hwnd)) => hwnd,
            _ => {
                let _ = window_thread.join();
                return Err(AppError::Clipboard);
            }
        };
        Ok((
            Self {
                hwnd,
                window_thread: Some(window_thread),
            },
            updates_rx,
        ))
    }

    pub fn stop(mut self) -> Result<(), AppError> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<(), AppError> {
        let Some(thread) = self.window_thread.take() else {
            return Ok(());
        };
        let hwnd = HWND(self.hwnd as *mut c_void);
        // SAFETY: hwnd belongs to the controlled listener thread. SendMessageW synchronously
        // executes WM_CLOSE on that thread, whose WndProc removes the clipboard listener and
        // destroys the window before returning.
        unsafe { SendMessageW(hwnd, WM_CLOSE, Some(WPARAM(0)), Some(LPARAM(0))) };
        thread.join().map_err(|_| AppError::Clipboard)
    }
}

impl Drop for ClipboardListener {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct ListenerContext {
    updates: SyncSender<()>,
}

fn run_listener_window(
    updates: SyncSender<()>,
    ready: SyncSender<Result<isize, AppError>>,
) -> Result<(), AppError> {
    let class_id = LISTENER_CLASS_ID.fetch_add(1, Ordering::Relaxed);
    let class_name = format!("EasyClipboardListener.{class_id}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: null requests the module containing the current executable.
    let module = unsafe { GetModuleHandleW(None) }.map_err(|_| AppError::Clipboard)?;
    let instance = HINSTANCE(module.0);
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(clipboard_window_proc),
        hInstance: instance,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: window_class points to live class-name storage for this thread's lifetime.
    if unsafe { RegisterClassW(&window_class) } == 0 {
        let _ = ready.send(Err(AppError::Clipboard));
        return Err(AppError::Clipboard);
    }
    // SAFETY: the registered class and all string pointers are valid; HWND_MESSAGE creates a
    // non-visible message-only window owned by this thread.
    let hwnd = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(_) => {
            // SAFETY: class was registered above and no window was created from it.
            let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(instance)) };
            let _ = ready.send(Err(AppError::Clipboard));
            return Err(AppError::Clipboard);
        }
    };
    let context = Box::new(ListenerContext { updates });
    let context_pointer = Box::into_raw(context);
    // SAFETY: context_pointer remains allocated until after the message loop and is only accessed
    // on this window thread by clipboard_window_proc.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, context_pointer as isize) };
    // SAFETY: hwnd is a live message-only window.
    if unsafe { AddClipboardFormatListener(hwnd) }.is_err() {
        // SAFETY: clear the window slot before reclaiming the allocation.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        // SAFETY: context_pointer came from Box::into_raw above and has not been reclaimed.
        drop(unsafe { Box::from_raw(context_pointer) });
        // SAFETY: hwnd and class are owned by this thread.
        let _ = unsafe { DestroyWindow(hwnd) };
        // SAFETY: all windows for this class have been destroyed.
        let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(instance)) };
        let _ = ready.send(Err(AppError::Clipboard));
        return Err(AppError::Clipboard);
    }
    if ready.send(Ok(hwnd.0 as isize)).is_err() {
        // SAFETY: hwnd is live and registered as a clipboard listener.
        let _ = unsafe { RemoveClipboardFormatListener(hwnd) };
        // SAFETY: hwnd is owned by this thread.
        let _ = unsafe { DestroyWindow(hwnd) };
    } else {
        let mut message = MSG::default();
        loop {
            // SAFETY: message points to initialized writable storage; this thread owns the queue.
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 <= 0 {
                break;
            }
            // SAFETY: message was initialized by a successful GetMessageW.
            unsafe { DispatchMessageW(&message) };
        }
    }

    // SAFETY: the loop has ended and no further dispatch can access the context.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
    // SAFETY: context_pointer came from Box::into_raw and is reclaimed exactly once here.
    drop(unsafe { Box::from_raw(context_pointer) });
    // SAFETY: hwnd has been destroyed before WM_QUIT and no window of this class remains.
    let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(instance)) };
    Ok(())
}

unsafe extern "system" fn clipboard_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLIPBOARDUPDATE => {
            // SAFETY: the slot is set to a ListenerContext pointer before listener registration
            // and cleared only after message dispatch has stopped.
            let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
            if pointer != 0 {
                // SAFETY: pointer is the live ListenerContext stored by run_listener_window.
                let context = unsafe { &*(pointer as *const ListenerContext) };
                match context.updates.try_send(()) {
                    Ok(()) | Err(TrySendError::Full(())) | Err(TrySendError::Disconnected(())) => {}
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: hwnd is a live registered listener window.
            let _ = unsafe { RemoveClipboardFormatListener(hwnd) };
            // SAFETY: hwnd belongs to the current window thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: posts WM_QUIT to the current thread's message queue.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: forwarding unhandled messages is required by the Win32 window contract.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs, io,
        path::{Path, PathBuf},
    };

    use image::ImageEncoder;
    use tempfile::tempdir;

    use super::{
        BudgetValue, ByteReadPlan, ClipboardAccess, ClipboardByteBudget,
        ClipboardFormatAvailability, ClipboardListener, ClipboardOwnerWindow, ClipboardReadFormat,
        FileRootsOutcome, FileTreeSource, MAX_FILE_TREE_BYTES, TreeNode, byte_read_plan,
        capped_path_size, choose_read_format, clamp_capture_limit,
        decode_custom_bytes_with_budget_counted, decode_nul_terminated_utf16,
        decode_utf16_with_budget, decode_utf16_with_budget_counted, dib_read_plan, dib_to_bmp,
        encode_drop_files, measure_file_roots_with_budget, media_kind_for_path, parse_drop_files,
        png_to_dibv5, root_count_plan,
    };
    use crate::domain::MediaKind;

    #[test]
    fn utf16_decoder_stops_at_first_nul_and_preserves_unicode() {
        let encoded = [
            0x0045, 0x0061, 0x0073, 0x0079, 0x0020, 0xd83d, 0xdccb, 0, 0x0058,
        ];

        assert_eq!(decode_nul_terminated_utf16(&encoded).unwrap(), "Easy 📋");
    }

    #[test]
    fn utf16_decoder_rejects_unpaired_surrogates() {
        assert!(decode_nul_terminated_utf16(&[0xd800, 0]).is_err());
    }

    #[test]
    fn write_owner_window_is_a_live_non_null_message_window() {
        let owner = ClipboardOwnerWindow::create().unwrap();

        assert!(!owner.hwnd().0.is_null());
        assert_eq!(ClipboardAccess::Read.owner(), None);
        assert_eq!(
            ClipboardAccess::Write(owner.hwnd()).owner(),
            Some(owner.hwnd())
        );
    }

    #[test]
    fn explicit_listener_stop_joins_message_thread_and_disconnects_updates() {
        let (listener, updates) = ClipboardListener::start().unwrap();

        listener.stop().unwrap();

        assert!(matches!(
            updates.recv_timeout(std::time::Duration::from_secs(1)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn listener_drop_finishes_within_a_bounded_time() {
        let (finished_tx, finished_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = ClipboardListener::start().map(|(listener, updates)| {
                drop(listener);
                matches!(
                    updates.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Disconnected)
                )
            });
            let _ = finished_tx.send(result);
        });

        assert!(matches!(
            finished_rx.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(Ok(true))
        ));
    }

    #[test]
    fn read_format_selection_short_circuits_by_domain_priority() {
        let all = ClipboardFormatAvailability {
            files: true,
            png: true,
            dib_v5: true,
            dib: true,
            plain_text: true,
        };
        assert_eq!(choose_read_format(all), ClipboardReadFormat::Files);
        assert_eq!(
            choose_read_format(ClipboardFormatAvailability {
                files: false,
                ..all
            }),
            ClipboardReadFormat::Png
        );
        assert_eq!(
            choose_read_format(ClipboardFormatAvailability {
                files: false,
                png: false,
                ..all
            }),
            ClipboardReadFormat::DibV5
        );
        assert_eq!(
            choose_read_format(ClipboardFormatAvailability {
                files: false,
                png: false,
                dib_v5: false,
                ..all
            }),
            ClipboardReadFormat::Dib
        );
        assert_eq!(
            choose_read_format(ClipboardFormatAvailability {
                files: false,
                png: false,
                dib_v5: false,
                dib: false,
                ..all
            }),
            ClipboardReadFormat::Text
        );
    }

    #[test]
    fn byte_read_plan_reports_oversize_without_allocating_a_truncated_buffer() {
        assert_eq!(byte_read_plan(10, 10), ByteReadPlan::Copy(10));
        assert_eq!(byte_read_plan(11, 10), ByteReadPlan::Oversize(11));
    }

    #[test]
    fn dynamic_limit_is_clamped_to_one_byte_through_five_hundred_megabytes() {
        assert_eq!(clamp_capture_limit(0), 1);
        assert_eq!(clamp_capture_limit(37), 37);
        assert_eq!(
            clamp_capture_limit(MAX_FILE_TREE_BYTES + 99),
            MAX_FILE_TREE_BYTES
        );
    }

    #[test]
    fn utf16_budget_counts_final_utf8_bytes_without_an_intermediate_byte_buffer() {
        let units = [b'a' as u16, 0xd83d, 0xdccb, 0];

        assert_eq!(
            decode_utf16_with_budget(&units, 5).unwrap(),
            BudgetValue::Accepted {
                value: "a📋".into(),
                measured_bytes: 5,
            }
        );
        assert_eq!(
            decode_utf16_with_budget(&units, 4).unwrap(),
            BudgetValue::Oversize(5)
        );
    }

    #[test]
    fn utf16_budget_scan_is_bounded_and_stops_at_the_first_nul() {
        let mut early_nul = vec![b'a' as u16, 0];
        early_nul.resize(1_000_000, 0xd800);
        let (value, inspected) = decode_utf16_with_budget_counted(&early_nul, 50).unwrap();
        assert_eq!(
            value,
            BudgetValue::Accepted {
                value: "a".into(),
                measured_bytes: 1,
            }
        );
        assert_eq!(inspected, 2);

        let no_nul = vec![b'a' as u16; 1_000_000];
        let (value, inspected) = decode_utf16_with_budget_counted(&no_nul, 4).unwrap();
        assert_eq!(value, BudgetValue::Oversize(5));
        assert_eq!(inspected, 5);
    }

    #[test]
    fn utf16_budget_handles_surrogate_boundaries_and_invalid_pairs_without_tail_scans() {
        let units = [0xd83d, 0xdccb, b'x' as u16, 0, 0xd800];
        let (value, inspected) = decode_utf16_with_budget_counted(&units, 3).unwrap();
        assert_eq!(value, BudgetValue::Oversize(4));
        assert_eq!(inspected, 2);

        let (value, inspected) = decode_utf16_with_budget_counted(&units, 4).unwrap();
        assert_eq!(value, BudgetValue::Oversize(5));
        assert_eq!(inspected, 3);

        assert!(decode_utf16_with_budget_counted(&[0xd83d, b'x' as u16], 50).is_err());
        assert!(decode_utf16_with_budget_counted(&[0xdccb], 50).is_err());

        let (value, inspected) =
            decode_utf16_with_budget_counted(&[0, 0xd800, 0xdccb], 50).unwrap();
        assert_eq!(
            value,
            BudgetValue::Accepted {
                value: String::new(),
                measured_bytes: 0,
            }
        );
        assert_eq!(inspected, 1);
    }

    #[test]
    fn custom_byte_budget_uses_a_bounded_prefix_and_first_nul() {
        let mut early_nul = vec![b'a', 0];
        early_nul.resize(1_000_000, b'x');
        let (value, inspected) = decode_custom_bytes_with_budget_counted(&early_nul, 4);
        assert_eq!(
            value,
            BudgetValue::Accepted {
                value: vec![b'a'],
                measured_bytes: 1,
            }
        );
        assert_eq!(inspected, 2);

        let no_nul = vec![b'a'; 1_000_000];
        let (value, inspected) = decode_custom_bytes_with_budget_counted(&no_nul, 4);
        assert_eq!(value, BudgetValue::Oversize(5));
        assert_eq!(inspected, 5);

        let (value, inspected) = decode_custom_bytes_with_budget_counted(b"html", 4);
        assert_eq!(
            value,
            BudgetValue::Accepted {
                value: b"html".to_vec(),
                measured_bytes: 4,
            }
        );
        assert_eq!(inspected, 4);
    }

    #[test]
    fn plain_html_and_rtf_claim_one_shared_text_budget() {
        let mut budget = ClipboardByteBudget::new(10);

        assert_eq!(budget.claim(5), Ok(()));
        assert_eq!(budget.remaining(), 5);
        assert_eq!(budget.claim(4), Ok(()));
        assert_eq!(budget.remaining(), 1);
        assert_eq!(budget.claim(2), Err(11));
    }

    #[test]
    fn root_count_plan_rejects_before_querying_more_than_global_item_budget() {
        assert_eq!(root_count_plan(3, 3), Ok(3));
        assert_eq!(root_count_plan(4, 3), Err(()));
    }

    #[test]
    fn clipboard_open_schedule_is_initial_attempt_plus_four_backoffs() {
        assert_eq!(super::OPEN_RETRY_DELAYS_MS, [10, 20, 40, 80]);
    }

    #[test]
    fn drop_files_round_trip_uses_wide_double_nul_payload() {
        let paths = [
            PathBuf::from(r"C:\资料\one.txt"),
            PathBuf::from(r"D:\media\two.mp4"),
        ];

        let bytes = encode_drop_files(&paths).unwrap();

        assert_eq!(parse_drop_files(&bytes).unwrap(), paths);
        assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn drop_files_parser_rejects_truncated_header() {
        assert!(parse_drop_files(&[0; 8]).is_err());
    }

    #[test]
    fn dib_to_bmp_adds_a_valid_file_header_for_v5_rgba() {
        let mut dib = vec![0_u8; 124 + 4];
        dib[0..4].copy_from_slice(&124_u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1_i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1_i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3_u32.to_le_bytes());
        dib[40..44].copy_from_slice(&0x00ff_0000_u32.to_le_bytes());
        dib[44..48].copy_from_slice(&0x0000_ff00_u32.to_le_bytes());
        dib[48..52].copy_from_slice(&0x0000_00ff_u32.to_le_bytes());
        dib[52..56].copy_from_slice(&0xff00_0000_u32.to_le_bytes());
        dib[124..128].copy_from_slice(&[3, 2, 1, 255]);

        let bmp = dib_to_bmp(&dib).unwrap();

        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(
            u32::from_le_bytes(bmp[10..14].try_into().unwrap()),
            14 + 124
        );
        assert_eq!(&bmp[14..], dib);
    }

    #[test]
    fn dib_to_bmp_rejects_invalid_header_and_palette_overflow() {
        assert!(dib_to_bmp(&[0; 39]).is_err());

        let mut dib = vec![0_u8; 40];
        dib[0..4].copy_from_slice(&40_u32.to_le_bytes());
        dib[14..16].copy_from_slice(&8_u16.to_le_bytes());
        dib[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(dib_to_bmp(&dib).is_err());
    }

    #[test]
    fn dib_budget_includes_the_fourteen_byte_bmp_file_header() {
        let mut dib = vec![0_u8; 124 + 4];
        dib[0..4].copy_from_slice(&124_u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1_i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1_i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3_u32.to_le_bytes());

        assert_eq!(dib_read_plan(&dib, 142).unwrap(), ByteReadPlan::Copy(142));
        assert_eq!(
            dib_read_plan(&dib, 141).unwrap(),
            ByteReadPlan::Oversize(142)
        );
    }

    #[test]
    fn png_to_dibv5_emits_top_down_bgra_pixels() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[1, 2, 3, 4], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();

        let dib = png_to_dibv5(&png).unwrap();

        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 124);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), -1);
        assert_eq!(&dib[124..128], &[3, 2, 1, 4]);
        assert!(
            image::load_from_memory_with_format(
                &dib_to_bmp(&dib).unwrap(),
                image::ImageFormat::Bmp
            )
            .is_ok()
        );
    }

    #[test]
    fn capped_directory_size_stops_at_cap_plus_one() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("one.bin"), [0_u8; 7]).unwrap();
        fs::create_dir(directory.path().join("nested")).unwrap();
        fs::write(directory.path().join("nested").join("two.bin"), [0_u8; 9]).unwrap();

        assert_eq!(capped_path_size(directory.path(), 10).unwrap(), 11);
        assert_eq!(capped_path_size(directory.path(), 20).unwrap(), 16);
    }

    #[test]
    fn missing_path_is_zero_and_unavailable() {
        let directory = tempdir().unwrap();
        let missing = directory.path().join("missing");

        assert_eq!(capped_path_size(&missing, MAX_FILE_TREE_BYTES).unwrap(), 0);
    }

    #[test]
    fn media_kind_uses_case_insensitive_audio_and_video_extensions() {
        assert_eq!(
            media_kind_for_path(Path::new("track.FLAC")),
            MediaKind::Audio
        );
        assert_eq!(media_kind_for_path(Path::new("clip.MkV")), MediaKind::Video);
        assert_eq!(
            media_kind_for_path(Path::new("document.pdf")),
            MediaKind::Other
        );
    }

    #[derive(Clone)]
    enum FakeTreeEntry {
        File(u64),
        Directory(Vec<PathBuf>),
        Reparse,
        Error(io::ErrorKind),
    }

    #[derive(Default)]
    struct FakeTree {
        entries: HashMap<PathBuf, FakeTreeEntry>,
        visits: Vec<PathBuf>,
    }

    impl FakeTree {
        fn with(mut self, path: &str, entry: FakeTreeEntry) -> Self {
            self.entries.insert(PathBuf::from(path), entry);
            self
        }
    }

    impl FileTreeSource for FakeTree {
        fn inspect(&mut self, path: &Path) -> io::Result<TreeNode> {
            self.visits.push(path.to_path_buf());
            match self.entries.get(path) {
                Some(FakeTreeEntry::File(size)) => Ok(TreeNode::File(*size)),
                Some(FakeTreeEntry::Directory(_)) => Ok(TreeNode::Directory),
                Some(FakeTreeEntry::Reparse) => Ok(TreeNode::Reparse),
                Some(FakeTreeEntry::Error(kind)) => Err(io::Error::new(*kind, "fake tree error")),
                None => Err(io::Error::new(io::ErrorKind::NotFound, "missing fake node")),
            }
        }

        fn children(&mut self, path: &Path, max_children: usize) -> io::Result<Vec<PathBuf>> {
            match self.entries.get(path) {
                Some(FakeTreeEntry::Directory(children)) => {
                    Ok(children.iter().take(max_children).cloned().collect())
                }
                Some(FakeTreeEntry::Error(kind)) => Err(io::Error::new(*kind, "fake tree error")),
                _ => Ok(Vec::new()),
            }
        }
    }

    #[test]
    fn multiple_directory_roots_share_one_byte_and_item_budget() {
        let roots = vec![PathBuf::from(r"C:\one"), PathBuf::from(r"D:\two")];
        let mut tree = FakeTree::default()
            .with(
                r"C:\one",
                FakeTreeEntry::Directory(vec![
                    PathBuf::from(r"C:\one\a"),
                    PathBuf::from(r"C:\one\b"),
                ]),
            )
            .with(r"C:\one\a", FakeTreeEntry::File(3))
            .with(r"C:\one\b", FakeTreeEntry::File(4))
            .with(
                r"D:\two",
                FakeTreeEntry::Directory(vec![PathBuf::from(r"D:\two\c")]),
            )
            .with(r"D:\two\c", FakeTreeEntry::File(5));

        let outcome = measure_file_roots_with_budget(roots, 20, 10, &mut tree);

        let FileRootsOutcome::Accepted(entries) = outcome else {
            panic!("expected accepted roots");
        };
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.size_bytes)
                .collect::<Vec<_>>(),
            vec![7, 5]
        );
        assert_eq!(tree.visits.len(), 5);
    }

    #[test]
    fn duplicate_roots_use_windows_identity_and_preserve_first_spelling() {
        let first = PathBuf::from(r"C:\Tree");
        let duplicate = PathBuf::from(r"c:/tree/.");
        let mut tree = FakeTree::default().with(r"C:\Tree", FakeTreeEntry::File(6));

        let outcome =
            measure_file_roots_with_budget(vec![first.clone(), duplicate], 10, 10, &mut tree);

        let FileRootsOutcome::Accepted(entries) = outcome else {
            panic!("expected accepted roots");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, first.to_string_lossy());
        assert_eq!(tree.visits, vec![first]);
    }

    #[test]
    fn cumulative_cross_root_oversize_stops_before_later_root_access() {
        let first = PathBuf::from(r"C:\first");
        let second = PathBuf::from(r"D:\second");
        let never = PathBuf::from(r"E:\never");
        let mut tree = FakeTree::default()
            .with(r"C:\first", FakeTreeEntry::File(6))
            .with(r"D:\second", FakeTreeEntry::File(6))
            .with(r"E:\never", FakeTreeEntry::Error(io::ErrorKind::Other));

        let outcome = measure_file_roots_with_budget(
            vec![first.clone(), second.clone(), never],
            10,
            10,
            &mut tree,
        );

        assert_eq!(outcome, FileRootsOutcome::Oversize(11));
        assert_eq!(tree.visits, vec![first, second]);
    }

    #[test]
    fn reparse_root_is_not_followed_and_is_marked_unavailable() {
        let root = PathBuf::from(r"C:\link");
        let mut tree = FakeTree::default().with(r"C:\link", FakeTreeEntry::Reparse);

        let outcome = measure_file_roots_with_budget(vec![root], 10, 10, &mut tree);

        let FileRootsOutcome::Accepted(entries) = outcome else {
            panic!("expected unavailable entry");
        };
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].available);
        assert_eq!(entries[0].size_bytes, 0);
    }

    #[test]
    fn unreadable_root_is_preserved_as_unavailable_without_size_underflow() {
        let root = PathBuf::from(r"C:\denied");
        let mut tree = FakeTree::default().with(
            r"C:\denied",
            FakeTreeEntry::Error(io::ErrorKind::PermissionDenied),
        );

        let outcome = measure_file_roots_with_budget(vec![root], 10, 10, &mut tree);

        let FileRootsOutcome::Accepted(entries) = outcome else {
            panic!("expected unavailable entry");
        };
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].available);
        assert_eq!(entries[0].size_bytes, 0);
    }

    #[test]
    fn unreadable_child_conservatively_rejects_entire_file_snapshot() {
        let root = PathBuf::from(r"C:\root");
        let child = PathBuf::from(r"C:\root\denied");
        let mut tree = FakeTree::default()
            .with(r"C:\root", FakeTreeEntry::Directory(vec![child.clone()]))
            .with(
                r"C:\root\denied",
                FakeTreeEntry::Error(io::ErrorKind::PermissionDenied),
            );

        let outcome = measure_file_roots_with_budget(vec![root], 10, 10, &mut tree);

        assert_eq!(outcome, FileRootsOutcome::Oversize(11));
        assert_eq!(tree.visits, vec![PathBuf::from(r"C:\root"), child]);
    }
}
