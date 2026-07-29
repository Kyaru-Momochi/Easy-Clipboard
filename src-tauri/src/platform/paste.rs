use std::{ffi::c_void, mem::size_of, sync::Mutex, thread, time::Duration};

use windows::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
            SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
        },
    },
};

use crate::{clipboard::PasteTarget, error::AppError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasteKeyEvent {
    virtual_key: u16,
    key_up: bool,
}

impl PasteKeyEvent {
    pub const fn key_down(virtual_key: u16) -> Self {
        Self {
            virtual_key,
            key_up: false,
        }
    }

    pub const fn key_up(virtual_key: u16) -> Self {
        Self {
            virtual_key,
            key_up: true,
        }
    }
}

pub const fn paste_sequence() -> [PasteKeyEvent; 4] {
    [
        PasteKeyEvent::key_down(VK_CONTROL.0),
        PasteKeyEvent::key_down(VK_V.0),
        PasteKeyEvent::key_up(VK_V.0),
        PasteKeyEvent::key_up(VK_CONTROL.0),
    ]
}

fn cleanup_for_prefix(sent: usize) -> Vec<PasteKeyEvent> {
    let sequence = paste_sequence();
    let mut held = Vec::<u16>::new();
    for event in sequence.iter().take(sent.min(sequence.len())) {
        if event.key_up {
            if let Some(index) = held.iter().rposition(|key| *key == event.virtual_key) {
                held.remove(index);
            }
        } else {
            held.push(event.virtual_key);
        }
    }
    held.into_iter().rev().map(PasteKeyEvent::key_up).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowIdentity {
    pub hwnd: isize,
    pub thread_id: u32,
    pub process_id: u32,
}

pub trait PasteWin32: Send + Sync {
    fn foreground_window(&self) -> isize;
    fn window_identity(&self, window: isize) -> Option<WindowIdentity>;
    fn set_foreground_window(&self, window: isize) -> bool;
    fn send_inputs(&self, inputs: &[PasteKeyEvent]) -> usize;
    fn key_down(&self, virtual_key: u16) -> bool;
    fn wait_ms(&self, milliseconds: u64);
}

#[derive(Default)]
pub struct SystemPasteWin32;

impl PasteWin32 for SystemPasteWin32 {
    fn foreground_window(&self) -> isize {
        // SAFETY: GetForegroundWindow has no memory-safety preconditions.
        unsafe { isize_from_hwnd(GetForegroundWindow()) }
    }

    fn window_identity(&self, window: isize) -> Option<WindowIdentity> {
        // SAFETY: the value is used only as an opaque handle and no pointer is dereferenced.
        let hwnd = hwnd_from_isize(window);
        if window == 0 || !unsafe { IsWindow(Some(hwnd)).as_bool() } {
            return None;
        }
        let mut process_id = 0_u32;
        // SAFETY: hwnd was validated and process_id is writable for the call.
        let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
        if thread_id == 0 || process_id == 0 {
            None
        } else {
            Some(WindowIdentity {
                hwnd: window,
                thread_id,
                process_id,
            })
        }
    }

    fn set_foreground_window(&self, window: isize) -> bool {
        // SAFETY: the handle is validated immediately before this method is called.
        unsafe { SetForegroundWindow(hwnd_from_isize(window)).as_bool() }
    }

    fn send_inputs(&self, inputs: &[PasteKeyEvent]) -> usize {
        let native = inputs.iter().copied().map(native_input).collect::<Vec<_>>();
        let input_size = match i32::try_from(size_of::<INPUT>()) {
            Ok(value) => value,
            Err(_) => return 0,
        };
        // SAFETY: the slice is fully initialized and cbSize is the exact INPUT structure size.
        usize::try_from(unsafe { SendInput(&native, input_size) }).unwrap_or(0)
    }

    fn key_down(&self, virtual_key: u16) -> bool {
        // SAFETY: GetAsyncKeyState accepts a virtual-key identifier and retains no pointer.
        unsafe { GetAsyncKeyState(i32::from(virtual_key)) < 0 }
    }

    fn wait_ms(&self, milliseconds: u64) {
        thread::sleep(Duration::from_millis(milliseconds));
    }
}

pub struct WindowsPasteTarget<A = SystemPasteWin32> {
    remembered: Mutex<Option<WindowIdentity>>,
    api: A,
}

impl WindowsPasteTarget<SystemPasteWin32> {
    pub fn new() -> Self {
        Self::with_api(SystemPasteWin32)
    }
}

impl Default for WindowsPasteTarget<SystemPasteWin32> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A> WindowsPasteTarget<A> {
    pub fn with_api(api: A) -> Self {
        Self {
            remembered: Mutex::new(None),
            api,
        }
    }

    #[cfg(test)]
    fn api(&self) -> &A {
        &self.api
    }
}

impl<A: PasteWin32> PasteTarget for WindowsPasteTarget<A> {
    fn remember_foreground(&self) -> Result<(), AppError> {
        let window = self.api.foreground_window();
        let mut remembered = self.remembered.lock().map_err(|_| AppError::Paste)?;
        *remembered = None;
        let identity = self.api.window_identity(window).ok_or(AppError::Paste)?;
        *remembered = Some(identity);
        Ok(())
    }

    fn paste_to_remembered(&self) -> Result<(), AppError> {
        let remembered = self
            .remembered
            .lock()
            .map_err(|_| AppError::Paste)?
            .ok_or(AppError::Paste)?;
        if self.api.window_identity(remembered.hwnd) != Some(remembered) {
            return Err(AppError::Paste);
        }
        if !wait_for_guard_keys_release(&self.api) {
            return Err(AppError::Paste);
        }
        if self.api.window_identity(remembered.hwnd) != Some(remembered) {
            return Err(AppError::Paste);
        }
        if self.api.foreground_window() != remembered.hwnd
            && (!self.api.set_foreground_window(remembered.hwnd)
                || self.api.foreground_window() != remembered.hwnd)
        {
            return Err(AppError::Paste);
        }
        if self.api.window_identity(remembered.hwnd) != Some(remembered) {
            return Err(AppError::Paste);
        }

        let sequence = paste_sequence();
        let sent = self.api.send_inputs(&sequence);
        if sent != sequence.len() {
            let cleanup = cleanup_for_prefix(sent);
            if !cleanup.is_empty() {
                let _ = self.api.send_inputs(&cleanup);
            }
            return Err(AppError::Paste);
        }
        Ok(())
    }
}

const GUARDED_KEYS: [u16; 6] = [
    VK_CONTROL.0,
    VK_SHIFT.0,
    VK_MENU.0,
    VK_LWIN.0,
    VK_RWIN.0,
    VK_V.0,
];
const RELEASE_RETRY_DELAYS_MS: [u64; 4] = [10, 20, 40, 80];

fn wait_for_guard_keys_release(api: &impl PasteWin32) -> bool {
    if guard_keys_released(api) {
        return true;
    }
    for delay in RELEASE_RETRY_DELAYS_MS {
        api.wait_ms(delay);
        if guard_keys_released(api) {
            return true;
        }
    }
    false
}

fn guard_keys_released(api: &impl PasteWin32) -> bool {
    GUARDED_KEYS.iter().all(|key| !api.key_down(*key))
}

fn native_input(event: PasteKeyEvent) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(event.virtual_key),
                dwFlags: if event.key_up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                ..KEYBDINPUT::default()
            },
        },
    }
}

fn hwnd_from_isize(value: isize) -> HWND {
    HWND(value as *mut c_void)
}

fn isize_from_hwnd(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{
        PasteKeyEvent, PasteWin32, WindowIdentity, WindowsPasteTarget, cleanup_for_prefix,
        paste_sequence,
    };
    use crate::{clipboard::PasteTarget, error::AppError};

    #[derive(Default)]
    struct FakeState {
        foreground: isize,
        valid: bool,
        identity: Option<WindowIdentity>,
        set_result: bool,
        foreground_after_set: Option<isize>,
        set_calls: Vec<isize>,
        batches: Vec<Vec<PasteKeyEvent>>,
        send_results: Vec<usize>,
        keys_down: Vec<u16>,
        waits: Vec<u64>,
    }

    #[derive(Default)]
    struct FakeWin32 {
        state: Mutex<FakeState>,
    }

    impl PasteWin32 for FakeWin32 {
        fn foreground_window(&self) -> isize {
            self.state.lock().unwrap().foreground
        }

        fn window_identity(&self, window: isize) -> Option<WindowIdentity> {
            let state = self.state.lock().unwrap();
            if !state.valid || window == 0 {
                None
            } else {
                Some(state.identity.unwrap_or(WindowIdentity {
                    hwnd: window,
                    thread_id: 7,
                    process_id: 11,
                }))
            }
        }

        fn set_foreground_window(&self, window: isize) -> bool {
            let mut state = self.state.lock().unwrap();
            state.set_calls.push(window);
            if let Some(foreground) = state.foreground_after_set {
                state.foreground = foreground;
            }
            state.set_result
        }

        fn send_inputs(&self, inputs: &[PasteKeyEvent]) -> usize {
            let mut state = self.state.lock().unwrap();
            state.batches.push(inputs.to_vec());
            if state.send_results.is_empty() {
                inputs.len()
            } else {
                state.send_results.remove(0)
            }
        }

        fn key_down(&self, virtual_key: u16) -> bool {
            self.state.lock().unwrap().keys_down.contains(&virtual_key)
        }

        fn wait_ms(&self, milliseconds: u64) {
            self.state.lock().unwrap().waits.push(milliseconds);
        }
    }

    fn target(state: FakeState) -> WindowsPasteTarget<FakeWin32> {
        WindowsPasteTarget::with_api(FakeWin32 {
            state: Mutex::new(state),
        })
    }

    #[test]
    fn paste_sequence_is_complete_ctrl_v_chord() {
        let sequence = paste_sequence();

        assert_eq!(sequence.len(), 4);
        assert_eq!(
            sequence,
            [
                PasteKeyEvent::key_down(0x11),
                PasteKeyEvent::key_down(b'V' as u16),
                PasteKeyEvent::key_up(b'V' as u16),
                PasteKeyEvent::key_up(0x11),
            ]
        );
    }

    #[test]
    fn partial_cleanup_releases_only_synthetic_keys_left_down_by_sent_prefix() {
        assert_eq!(cleanup_for_prefix(0), Vec::<PasteKeyEvent>::new());
        assert_eq!(cleanup_for_prefix(1), vec![PasteKeyEvent::key_up(0x11)]);
        assert_eq!(
            cleanup_for_prefix(2),
            vec![
                PasteKeyEvent::key_up(b'V' as u16),
                PasteKeyEvent::key_up(0x11),
            ]
        );
        assert_eq!(cleanup_for_prefix(3), vec![PasteKeyEvent::key_up(0x11)]);
    }

    #[test]
    fn window_identity_includes_handle_thread_and_process() {
        assert_ne!(
            WindowIdentity {
                hwnd: 41,
                thread_id: 2,
                process_id: 3,
            },
            WindowIdentity {
                hwnd: 41,
                thread_id: 4,
                process_id: 3,
            }
        );
    }

    #[test]
    fn already_foreground_target_does_not_call_set_foreground() {
        let target = target(FakeState {
            foreground: 41,
            valid: true,
            set_result: true,
            ..FakeState::default()
        });
        target.remember_foreground().unwrap();

        target.paste_to_remembered().unwrap();

        let state = target.api().state.lock().unwrap();
        assert!(state.set_calls.is_empty());
        assert_eq!(state.batches, vec![paste_sequence().to_vec()]);
    }

    #[test]
    fn foreground_switch_failure_does_not_send_input() {
        let target = target(FakeState {
            foreground: 41,
            valid: true,
            set_result: true,
            ..FakeState::default()
        });
        target.remember_foreground().unwrap();
        {
            let mut state = target.api().state.lock().unwrap();
            state.foreground = 99;
            state.set_result = false;
        }

        assert_eq!(target.paste_to_remembered().unwrap_err(), AppError::Paste);

        let state = target.api().state.lock().unwrap();
        assert_eq!(state.set_calls, vec![41]);
        assert!(state.batches.is_empty());
    }

    #[test]
    fn foreground_switch_must_be_observed_before_input_is_sent() {
        let target = target(FakeState {
            foreground: 41,
            valid: true,
            set_result: true,
            ..FakeState::default()
        });
        target.remember_foreground().unwrap();
        {
            let mut state = target.api().state.lock().unwrap();
            state.foreground = 99;
            state.foreground_after_set = Some(77);
        }

        assert_eq!(target.paste_to_remembered().unwrap_err(), AppError::Paste);
        assert!(target.api().state.lock().unwrap().batches.is_empty());
    }

    #[test]
    fn successful_foreground_switch_sends_input_after_target_is_observed() {
        let target = target(FakeState {
            foreground: 41,
            valid: true,
            set_result: true,
            ..FakeState::default()
        });
        target.remember_foreground().unwrap();
        {
            let mut state = target.api().state.lock().unwrap();
            state.foreground = 99;
            state.foreground_after_set = Some(41);
        }

        target.paste_to_remembered().unwrap();

        let state = target.api().state.lock().unwrap();
        assert_eq!(state.set_calls, vec![41]);
        assert_eq!(state.batches, vec![paste_sequence().to_vec()]);
    }

    #[test]
    fn partial_send_input_attempts_v_and_control_keyup_cleanup() {
        let target = target(FakeState {
            foreground: 41,
            valid: true,
            set_result: true,
            send_results: vec![2, 2],
            ..FakeState::default()
        });
        target.remember_foreground().unwrap();

        assert_eq!(target.paste_to_remembered().unwrap_err(), AppError::Paste);

        let state = target.api().state.lock().unwrap();
        assert_eq!(state.batches.len(), 2);
        assert_eq!(state.batches[0], paste_sequence());
        assert_eq!(
            state.batches[1],
            [
                PasteKeyEvent::key_up(b'V' as u16),
                PasteKeyEvent::key_up(0x11),
            ]
        );
    }

    #[test]
    fn send_input_prefix_zero_one_and_three_use_minimal_cleanup() {
        for (sent, expected_cleanup) in [
            (0, Vec::new()),
            (1, vec![PasteKeyEvent::key_up(0x11)]),
            (3, vec![PasteKeyEvent::key_up(0x11)]),
        ] {
            let target = target(FakeState {
                foreground: 41,
                valid: true,
                set_result: true,
                send_results: vec![sent, expected_cleanup.len()],
                ..FakeState::default()
            });
            target.remember_foreground().unwrap();

            assert_eq!(target.paste_to_remembered().unwrap_err(), AppError::Paste);

            let state = target.api().state.lock().unwrap();
            let mut expected = vec![paste_sequence().to_vec()];
            if !expected_cleanup.is_empty() {
                expected.push(expected_cleanup);
            }
            assert_eq!(state.batches, expected);
        }
    }

    #[test]
    fn held_control_or_shift_rejects_without_input_or_physical_key_cleanup() {
        for held in [0x11, 0x10] {
            let target = target(FakeState {
                foreground: 41,
                valid: true,
                set_result: true,
                keys_down: vec![held],
                ..FakeState::default()
            });
            target.remember_foreground().unwrap();

            assert_eq!(target.paste_to_remembered().unwrap_err(), AppError::Paste);

            let state = target.api().state.lock().unwrap();
            assert!(state.batches.is_empty());
            assert_eq!(state.waits, vec![10, 20, 40, 80]);
            assert_eq!(state.keys_down, vec![held]);
        }
    }

    #[test]
    fn recycled_numeric_hwnd_with_new_owner_identity_is_rejected() {
        let target = target(FakeState {
            foreground: 41,
            valid: true,
            set_result: true,
            ..FakeState::default()
        });
        target.remember_foreground().unwrap();
        target.api().state.lock().unwrap().identity = Some(WindowIdentity {
            hwnd: 41,
            thread_id: 99,
            process_id: 101,
        });

        assert_eq!(target.paste_to_remembered().unwrap_err(), AppError::Paste);

        let state = target.api().state.lock().unwrap();
        assert!(state.set_calls.is_empty());
        assert!(state.batches.is_empty());
        assert!(state.waits.is_empty());
    }

    #[test]
    fn disappeared_target_is_rejected_before_focus_or_input() {
        let target = target(FakeState {
            foreground: 41,
            valid: true,
            set_result: true,
            ..FakeState::default()
        });
        target.remember_foreground().unwrap();
        target.api().state.lock().unwrap().valid = false;

        assert_eq!(target.paste_to_remembered().unwrap_err(), AppError::Paste);

        let state = target.api().state.lock().unwrap();
        assert!(state.set_calls.is_empty());
        assert!(state.batches.is_empty());
    }

    #[test]
    fn invalid_foreground_is_not_remembered() {
        let target = target(FakeState {
            foreground: 0,
            valid: false,
            ..FakeState::default()
        });

        assert_eq!(target.remember_foreground().unwrap_err(), AppError::Paste);
        assert_eq!(target.paste_to_remembered().unwrap_err(), AppError::Paste);
    }
}
