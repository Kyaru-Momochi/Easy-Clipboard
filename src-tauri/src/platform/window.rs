use std::ffi::c_void;

use windows::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, SetLastError, WIN32_ERROR, WPARAM},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, GetWindowThreadProcessId, HWND_TOPMOST, IsWindow,
            MA_NOACTIVATE, SMTO_ABORTIFHUNG, SW_HIDE, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageTimeoutW,
            SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_APP, WM_MOUSEACTIVATE, WM_NCDESTROY,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        },
    },
};

use crate::error::AppError;

pub const DEFAULT_OVERLAY_SIZE: Size = Size {
    width: 460,
    height: 620,
};
pub const MIN_OVERLAY_SIZE: Size = Size {
    width: 360,
    height: 420,
};
pub const MAX_OVERLAY_SIZE: Size = Size {
    width: 720,
    height: 820,
};

const OVERLAY_SUBCLASS_ID: usize = 0x4543_4c50;
const DETACH_SUBCLASS_MESSAGE: u32 = WM_APP + 0x451;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Default for Size {
    fn default() -> Self {
        DEFAULT_OVERLAY_SIZE
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayPlacement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl OverlayPlacement {
    pub fn centered(work_area: Rect, requested: Size) -> Self {
        let available_width = nonnegative_span(work_area.left, work_area.right);
        let available_height = nonnegative_span(work_area.top, work_area.bottom);
        let width = requested
            .width
            .clamp(MIN_OVERLAY_SIZE.width, MAX_OVERLAY_SIZE.width)
            .min(available_width);
        let height = requested
            .height
            .clamp(MIN_OVERLAY_SIZE.height, MAX_OVERLAY_SIZE.height)
            .min(available_height);
        let x = centered_axis(work_area.left, available_width, width);
        let y = centered_axis(work_area.top, available_height, height);

        Self {
            x,
            y,
            width,
            height,
        }
    }
}

fn nonnegative_span(start: i32, end: i32) -> u32 {
    let span = i64::from(end) - i64::from(start);
    u32::try_from(span.max(0)).unwrap_or(u32::MAX)
}

fn centered_axis(start: i32, available: u32, used: u32) -> i32 {
    let offset = i64::from(available.saturating_sub(used) / 2);
    i32::try_from(i64::from(start).saturating_add(offset)).unwrap_or(start)
}

/// Owns the no-activate subclass installed on the Tauri webview's native window.
///
/// Attachment is performed by Tauri setup on the webview owner thread. The handle is stored as an
/// integer for managed state; Drop synchronously asks the owner window's subclass procedure to
/// detach itself instead of calling comctl32 subclass APIs from an arbitrary dropping thread.
pub struct OverlayController {
    hwnd: isize,
    owner_thread_id: u32,
}

impl OverlayController {
    pub fn attach(hwnd: isize) -> Result<Self, AppError> {
        let native = hwnd_from_isize(hwnd);
        // SAFETY: `native` is used only as an opaque handle and IsWindow validates it before the
        // style and subclass operations below.
        if hwnd == 0 || !unsafe { IsWindow(Some(native)).as_bool() } {
            return Err(AppError::Platform);
        }
        // SAFETY: native was validated above; null process-id storage requests only the owner
        // thread id.
        let owner_thread_id = unsafe { GetWindowThreadProcessId(native, None) };
        if owner_thread_id == 0 {
            return Err(AppError::Platform);
        }

        let original_style = install_no_activate_style(native)?;
        // SAFETY: the callback has the required ABI and carries no borrowed pointer in ref data.
        if !unsafe {
            SetWindowSubclass(
                native,
                Some(overlay_subclass_proc),
                OVERLAY_SUBCLASS_ID,
                original_style as usize,
            )
            .as_bool()
        } {
            let _ = restore_style(native, original_style);
            return Err(AppError::Platform);
        }

        Ok(Self {
            hwnd,
            owner_thread_id,
        })
    }

    pub fn show_centered(
        &self,
        remembered_foreground: isize,
        requested: Size,
    ) -> Result<OverlayPlacement, AppError> {
        let hwnd = hwnd_from_isize(self.hwnd);
        let monitor_source = if remembered_foreground == 0 {
            hwnd
        } else {
            hwnd_from_isize(remembered_foreground)
        };
        // SAFETY: both values are opaque window handles; MONITOR_DEFAULTTONEAREST guarantees a
        // monitor for a valid desktop configuration.
        let monitor = unsafe { MonitorFromWindow(monitor_source, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_invalid() {
            return Err(AppError::Platform);
        }

        let mut info = MONITORINFO {
            cbSize: u32::try_from(size_of::<MONITORINFO>()).map_err(|_| AppError::Platform)?,
            ..MONITORINFO::default()
        };
        // SAFETY: `info` is initialized with the required size and remains writable for the call.
        if !unsafe { GetMonitorInfoW(monitor, &mut info).as_bool() } {
            return Err(AppError::Platform);
        }
        let placement = OverlayPlacement::centered(
            Rect {
                left: info.rcWork.left,
                top: info.rcWork.top,
                right: info.rcWork.right,
                bottom: info.rcWork.bottom,
            },
            requested,
        );
        let width = i32::try_from(placement.width).map_err(|_| AppError::Platform)?;
        let height = i32::try_from(placement.height).map_err(|_| AppError::Platform)?;

        // SAFETY: the attached HWND was validated and no pointer data is passed. HWND_TOPMOST is
        // intentionally paired with no SWP_NOZORDER so topmost is reaffirmed at every show.
        unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                placement.x,
                placement.y,
                width,
                height,
                SWP_NOACTIVATE,
            )
            .map_err(|_| AppError::Platform)?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(placement)
    }

    pub fn hide(&self) -> Result<(), AppError> {
        let hwnd = hwnd_from_isize(self.hwnd);
        // SAFETY: the handle was validated at attachment. ShowWindow's false result means the
        // window was previously hidden, not that the operation failed.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        Ok(())
    }

    pub const fn native_handle(&self) -> isize {
        self.hwnd
    }
}

impl Drop for OverlayController {
    fn drop(&mut self) {
        let hwnd = hwnd_from_isize(self.hwnd);
        // SAFETY: IsWindow treats hwnd as opaque and detects an already-destroyed Tauri window.
        let live = unsafe { IsWindow(Some(hwnd)).as_bool() };
        // SAFETY: matching the captured owner id avoids sending to a recycled HWND.
        let owner_matches =
            live && unsafe { GetWindowThreadProcessId(hwnd, None) } == self.owner_thread_id;
        if let Some(message) = teardown_message(live, owner_matches) {
            let mut message_result = 0_usize;
            // SAFETY: synchronously dispatches the custom detach request on the owner thread. The
            // timeout prevents shutdown from hanging if that thread has stopped pumping messages.
            let _ = unsafe {
                SendMessageTimeoutW(
                    hwnd,
                    message,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    1_000,
                    Some(&mut message_result),
                )
            };
        }
    }
}

fn teardown_message(window_is_live: bool, owner_matches: bool) -> Option<u32> {
    (window_is_live && owner_matches).then_some(DETACH_SUBCLASS_MESSAGE)
}

fn install_no_activate_style(hwnd: HWND) -> Result<u32, AppError> {
    // SAFETY: clearing last error disambiguates a valid zero extended style.
    unsafe {
        SetLastError(WIN32_ERROR(0));
    }
    // SAFETY: the HWND was validated by `attach`; the API reads one integer style value.
    let existing = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    // SAFETY: GetLastError has no memory-safety preconditions.
    if existing == 0 && unsafe { GetLastError() } != WIN32_ERROR(0) {
        return Err(AppError::Platform);
    }
    let required = WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | existing as u32;
    // SAFETY: clearing last-error is required to distinguish a valid previous zero value from a
    // failed SetWindowLongPtrW call.
    unsafe {
        SetLastError(WIN32_ERROR(0));
    }
    // SAFETY: writes only the extended-style integer for the validated HWND.
    let previous = unsafe {
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            isize::try_from(required).map_err(|_| AppError::Platform)?,
        )
    };
    // SAFETY: GetLastError has no memory-safety preconditions.
    if previous == 0 && unsafe { GetLastError() } != WIN32_ERROR(0) {
        return Err(AppError::Platform);
    }
    refresh_frame(hwnd)?;
    Ok(existing as u32)
}

fn restore_style(hwnd: HWND, original_style: u32) -> Result<(), AppError> {
    // SAFETY: hwnd is a live window and this restores the value captured before attachment.
    unsafe {
        SetLastError(WIN32_ERROR(0));
    }
    // SAFETY: writes only the extended style integer for the target HWND.
    let previous = unsafe {
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            isize::try_from(original_style).map_err(|_| AppError::Platform)?,
        )
    };
    // SAFETY: GetLastError has no memory-safety preconditions.
    if previous == 0 && unsafe { GetLastError() } != WIN32_ERROR(0) {
        return Err(AppError::Platform);
    }
    refresh_frame(hwnd)
}

fn refresh_frame(hwnd: HWND) -> Result<(), AppError> {
    // SAFETY: this reapplies cached non-client/style state without moving, resizing, activating, or
    // changing z-order.
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        )
        .map_err(|_| AppError::Platform)
    }
}

fn hwnd_from_isize(value: isize) -> HWND {
    HWND(value as *mut c_void)
}

unsafe extern "system" fn overlay_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    if message == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if message == DETACH_SUBCLASS_MESSAGE {
        let _ = restore_style(hwnd, ref_data as u32);
        // SAFETY: this runs on the HWND owner thread and removes this exact callback/id pair.
        let _ =
            unsafe { RemoveWindowSubclass(hwnd, Some(overlay_subclass_proc), OVERLAY_SUBCLASS_ID) };
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        // SAFETY: owner-thread final teardown prevents comctl32 retaining the callback past HWND
        // destruction. Style restoration is unnecessary because the window is being destroyed.
        let _ =
            unsafe { RemoveWindowSubclass(hwnd, Some(overlay_subclass_proc), OVERLAY_SUBCLASS_ID) };
    }

    // SAFETY: the subclass contract requires forwarding every unhandled message with its original
    // values to DefSubclassProc.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{DETACH_SUBCLASS_MESSAGE, OverlayPlacement, Rect, Size, teardown_message};

    #[test]
    fn overlay_is_centered_and_clamped_to_work_area() {
        let placement = OverlayPlacement::centered(
            Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1040,
            },
            Size {
                width: 460,
                height: 620,
            },
        );

        assert_eq!(
            placement,
            OverlayPlacement {
                x: 730,
                y: 210,
                width: 460,
                height: 620,
            }
        );
    }

    #[test]
    fn requested_size_is_clamped_to_product_bounds() {
        let work_area = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };

        assert_eq!(
            OverlayPlacement::centered(
                work_area,
                Size {
                    width: 1,
                    height: 9,
                },
            ),
            OverlayPlacement {
                x: -1140,
                y: 330,
                width: 360,
                height: 420,
            }
        );
        assert_eq!(
            OverlayPlacement::centered(
                work_area,
                Size {
                    width: 900,
                    height: 900,
                },
            ),
            OverlayPlacement {
                x: -1320,
                y: 130,
                width: 720,
                height: 820,
            }
        );
    }

    #[test]
    fn tiny_work_area_never_overflows_or_places_overlay_outside_it() {
        let placement = OverlayPlacement::centered(
            Rect {
                left: 100,
                top: 200,
                right: 300,
                bottom: 350,
            },
            Size::default(),
        );

        assert_eq!(
            placement,
            OverlayPlacement {
                x: 100,
                y: 200,
                width: 200,
                height: 150,
            }
        );
    }

    #[test]
    fn inverted_or_empty_work_area_produces_a_zero_sized_safe_placement() {
        let placement = OverlayPlacement::centered(
            Rect {
                left: i32::MAX,
                top: i32::MAX,
                right: i32::MIN,
                bottom: i32::MIN,
            },
            Size::default(),
        );

        assert_eq!(placement.width, 0);
        assert_eq!(placement.height, 0);
        assert_eq!(placement.x, i32::MAX);
        assert_eq!(placement.y, i32::MAX);
    }

    #[test]
    fn owner_thread_teardown_is_requested_only_while_window_is_live() {
        assert_eq!(teardown_message(true, true), Some(DETACH_SUBCLASS_MESSAGE));
        assert_eq!(teardown_message(false, true), None);
        assert_eq!(teardown_message(true, false), None);
    }
}
