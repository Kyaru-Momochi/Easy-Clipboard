use std::{
    cell::RefCell,
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
};

use serde::{Deserialize, Serialize};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, GetKeyState, GetKeyboardLayout, HKL, MAPVK_VSC_TO_VK_EX,
                MapVirtualKeyExW, ToUnicodeEx, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DOWN, VK_ESCAPE,
                VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NUMLOCK,
                VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SCROLL,
                VK_SHIFT, VK_UP,
            },
            WindowsAndMessaging::{
                CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, HC_ACTION, HHOOK,
                HWND_MESSAGE, IsWindow, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG,
                PostQuitMessage, RegisterClassW, SendMessageW, SetWindowsHookExW, TranslateMessage,
                UnhookWindowsHookEx, UnregisterClassW, WH_KEYBOARD_LL, WINDOW_EX_STYLE,
                WINDOW_STYLE, WM_CLOSE, WM_DESTROY, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
                WM_SYSKEYUP, WNDCLASSW,
            },
        },
    },
    core::PCWSTR,
};

use crate::error::AppError;

const DEFAULT_KEY_CHANNEL_CAPACITY: usize = 32;
static KEYBOARD_CLASS_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyInput {
    Character(char),
    Text(String),
    Backspace,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Enter,
    Escape,
    Modified {
        control: bool,
        shift: bool,
        alt: bool,
        windows: bool,
        key: char,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum KeyAction {
    SearchText(String),
    DeleteBackward,
    Move(i32),
    Paste,
    Hide,
}

pub fn route_key(input: KeyInput) -> Option<KeyAction> {
    match input {
        KeyInput::Character(character) if !character.is_control() => {
            Some(KeyAction::SearchText(character.to_string()))
        }
        KeyInput::Text(text)
            if !text.is_empty() && text.chars().all(|character| !character.is_control()) =>
        {
            Some(KeyAction::SearchText(text))
        }
        KeyInput::Backspace => Some(KeyAction::DeleteBackward),
        KeyInput::ArrowUp => Some(KeyAction::Move(-1)),
        KeyInput::ArrowDown => Some(KeyAction::Move(1)),
        KeyInput::ArrowLeft | KeyInput::ArrowRight => None,
        KeyInput::Enter => Some(KeyAction::Paste),
        KeyInput::Escape => Some(KeyAction::Hide),
        KeyInput::Character(_) | KeyInput::Text(_) | KeyInput::Modified { .. } => None,
    }
}

struct HookWorker {
    hwnd: isize,
    owner_thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl HookWorker {
    fn shutdown(&mut self) -> Result<(), AppError> {
        if self.join.is_none() {
            return Ok(());
        }
        let hwnd = hwnd_from_isize(self.hwnd);
        let finished = self.join.as_ref().is_some_and(JoinHandle::is_finished);
        // SAFETY: hwnd is treated as an opaque handle.
        let live = unsafe { IsWindow(Some(hwnd)).as_bool() };
        // SAFETY: querying an opaque HWND retains no pointers. The owner id check prevents
        // messaging an unrelated window if the numeric handle has been recycled.
        let owner_matches =
            live && unsafe { GetWindowThreadProcessId(hwnd, None) } == self.owner_thread_id;
        match shutdown_decision(finished, live, owner_matches) {
            ShutdownDecision::CloseThenJoin => {
                // SAFETY: this thread/window pair is fully controlled by this module. Its WndProc
                // only destroys the message-only window and posts WM_QUIT; hook callbacks are
                // bounded and panic-contained. Therefore synchronous WM_CLOSE cannot depend on
                // external code and returns after the owner thread has begun deterministic exit.
                unsafe { SendMessageW(hwnd, WM_CLOSE, Some(WPARAM(0)), Some(LPARAM(0))) };
            }
            ShutdownDecision::JoinOnly => {}
            ShutdownDecision::RetainAndError => return Err(AppError::Platform),
        }

        self.join
            .take()
            .ok_or(AppError::Platform)?
            .join()
            .map_err(|_| AppError::Platform)
    }
}

impl Drop for HookWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct HookLifecycle<T> {
    worker: Option<T>,
}

impl<T> Default for HookLifecycle<T> {
    fn default() -> Self {
        Self { worker: None }
    }
}

impl<T> HookLifecycle<T> {
    fn start(&mut self, worker: T) -> Result<(), T> {
        if self.worker.is_some() {
            Err(worker)
        } else {
            self.worker = Some(worker);
            Ok(())
        }
    }

    fn stop(&mut self) -> Option<T> {
        self.worker.take()
    }

    fn is_visible(&self) -> bool {
        self.worker.is_some()
    }
}

/// Installs a low-level keyboard hook only between `show` and `hide`.
///
/// Actions are sent through a bounded channel; a full/disconnected channel causes the key to pass
/// through instead of being swallowed.
#[derive(Default)]
pub struct VisibleKeyboardRouter {
    lifecycle: Mutex<HookLifecycle<HookWorker>>,
}

impl VisibleKeyboardRouter {
    pub fn show(&self) -> Result<Receiver<KeyAction>, AppError> {
        self.show_with_capacity(DEFAULT_KEY_CHANNEL_CAPACITY)
    }

    pub fn show_with_capacity(&self, capacity: usize) -> Result<Receiver<KeyAction>, AppError> {
        if capacity == 0 {
            return Err(AppError::Platform);
        }
        let mut lifecycle = self.lifecycle.lock().map_err(|_| AppError::Platform)?;
        if lifecycle.is_visible() {
            return Err(AppError::Platform);
        }
        let (action_sender, action_receiver) = sync_channel(capacity);
        let (ready_sender, ready_receiver) = sync_channel(1);
        let join = thread::Builder::new()
            .name("easy-clipboard-keyboard".into())
            .spawn(move || keyboard_thread(action_sender, ready_sender))
            .map_err(|_| AppError::Platform)?;
        let hwnd = match ready_receiver.recv() {
            Ok(Ok(ready)) => ready,
            _ => {
                let _ = join.join();
                return Err(AppError::Platform);
            }
        };
        lifecycle
            .start(HookWorker {
                hwnd: hwnd.hwnd,
                owner_thread_id: hwnd.owner_thread_id,
                join: Some(join),
            })
            .map_err(|_| AppError::Platform)?;
        Ok(action_receiver)
    }

    pub fn hide(&self) -> Result<(), AppError> {
        let mut lifecycle = self.lifecycle.lock().map_err(|_| AppError::Platform)?;
        let Some(worker) = lifecycle.worker.as_mut() else {
            return Ok(());
        };
        worker.shutdown()?;
        let _ = lifecycle.stop();
        Ok(())
    }

    pub fn stop(&self) -> Result<(), AppError> {
        self.hide()
    }

    pub fn is_visible(&self) -> bool {
        self.lifecycle
            .lock()
            .map(|lifecycle| lifecycle.is_visible())
            .unwrap_or(false)
    }
}

impl Drop for VisibleKeyboardRouter {
    fn drop(&mut self) {
        let Some(lifecycle) = self.lifecycle.get_mut().ok() else {
            return;
        };
        let Some(worker) = lifecycle.worker.as_mut() else {
            return;
        };
        if worker.shutdown().is_ok() {
            let _ = lifecycle.stop();
        } else if let Some(worker) = lifecycle.stop() {
            // Owner mismatch is an impossible invariant violation in normal operation. Leaking the
            // still-owned JoinHandle is safer than detaching it or messaging a recycled HWND.
            std::mem::forget(worker);
        }
    }
}

struct HookContext {
    sender: SyncSender<KeyAction>,
    swallowed: HashSet<PhysicalKey>,
    keyboard: KeyboardState,
}

thread_local! {
    static HOOK_CONTEXT: RefCell<Option<HookContext>> = const { RefCell::new(None) };
}

fn clear_hook_context() {
    HOOK_CONTEXT.with(|slot| *slot.borrow_mut() = None);
}

fn initialize_hook_snapshot_context<H, E>(
    install_hook: impl FnOnce() -> Result<H, E>,
    snapshot: impl FnOnce() -> KeyboardState,
    publish_context: impl FnOnce(KeyboardState),
) -> Result<H, E> {
    let hook = install_hook()?;
    let keyboard = snapshot();
    publish_context(keyboard);
    Ok(hook)
}

fn keyboard_thread(
    action_sender: SyncSender<KeyAction>,
    ready_sender: SyncSender<Result<HookReady, AppError>>,
) {
    // SAFETY: querying the current module handle requires no borrowed memory. The handle remains
    // loaded for the process lifetime.
    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => HINSTANCE(module.0),
        Err(_) => {
            let _ = ready_sender.send(Err(AppError::Platform));
            clear_hook_context();
            return;
        }
    };
    let class_id = KEYBOARD_CLASS_ID.fetch_add(1, Ordering::Relaxed);
    let class_name = format!("EasyClipboardKeyboard.{class_id}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(keyboard_window_proc),
        hInstance: module,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..WNDCLASSW::default()
    };
    // SAFETY: class_name remains live until after this thread unregisters the class.
    if unsafe { RegisterClassW(&window_class) } == 0 {
        let _ = ready_sender.send(Err(AppError::Platform));
        clear_hook_context();
        return;
    }
    // SAFETY: the registered class and strings are live; HWND_MESSAGE creates a hidden
    // message-only window owned by this hook thread.
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
            Some(module),
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(_) => {
            // SAFETY: no live window was created for this class.
            let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(module)) };
            let _ = ready_sender.send(Err(AppError::Platform));
            clear_hook_context();
            return;
        }
    };
    let hook_guard = match initialize_hook_snapshot_context(
        || {
            // SAFETY: callback ABI matches HOOKPROC; thread id zero requests the desktop-wide
            // low-level hook, and the module remains loaded until process exit.
            unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), Some(module), 0)
                    .map(|hook| HookGuard(Some(hook)))
            }
        },
        snapshot_keyboard_state,
        |keyboard| {
            HOOK_CONTEXT.with(|slot| {
                *slot.borrow_mut() = Some(HookContext {
                    sender: action_sender,
                    swallowed: HashSet::new(),
                    keyboard,
                });
            });
        },
    ) {
        Ok(hook_guard) => hook_guard,
        Err(_) => {
            // SAFETY: hwnd belongs to this thread and the hook was not installed.
            let _ = unsafe { DestroyWindow(hwnd) };
            // SAFETY: the class no longer has a live window.
            let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(module)) };
            let _ = ready_sender.send(Err(AppError::Platform));
            clear_hook_context();
            return;
        }
    };
    // SAFETY: hwnd is live and owned by this thread.
    let owner_thread_id = unsafe { GetWindowThreadProcessId(hwnd, None) };
    if owner_thread_id == 0
        || ready_sender
            .send(Ok(HookReady {
                hwnd: hwnd.0 as isize,
                owner_thread_id,
            }))
            .is_err()
    {
        drop(hook_guard);
        // SAFETY: hwnd belongs to this thread.
        let _ = unsafe { DestroyWindow(hwnd) };
        // SAFETY: the class no longer has a live window.
        let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(module)) };
        clear_hook_context();
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is valid writable storage for the duration of each call.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        // SAFETY: both functions consume the initialized MSG and retain no pointer to it.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    drop(hook_guard);
    // A callback panic posts WM_QUIT directly, so its window may still be live here.
    // SAFETY: hwnd belongs to this thread; DestroyWindow is harmlessly skipped after WM_CLOSE.
    if unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)).as_bool() } {
        let _ = unsafe { DestroyWindow(hwnd) };
    }
    // SAFETY: all windows of this unique class have now been destroyed.
    let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(module)) };
    clear_hook_context();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HookReady {
    hwnd: isize,
    owner_thread_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShutdownDecision {
    CloseThenJoin,
    JoinOnly,
    RetainAndError,
}

fn shutdown_decision(
    worker_finished: bool,
    window_live: bool,
    owner_matches: bool,
) -> ShutdownDecision {
    if worker_finished || !window_live {
        ShutdownDecision::JoinOnly
    } else if owner_matches {
        ShutdownDecision::CloseThenJoin
    } else {
        ShutdownDecision::RetainAndError
    }
}

fn hwnd_from_isize(value: isize) -> HWND {
    HWND(value as *mut std::ffi::c_void)
}

unsafe extern "system" fn keyboard_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            // SAFETY: hwnd is the live message-only window owned by this thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: posts WM_QUIT to this window's owner thread.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: the window contract requires forwarding unhandled messages.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

struct HookGuard(Option<HHOOK>);

impl Drop for HookGuard {
    fn drop(&mut self) {
        if let Some(hook) = self.0.take() {
            // SAFETY: this guard uniquely owns the hook returned by SetWindowsHookExW.
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }
    }
}

unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let decision = callback_decision(
        catch_unwind(AssertUnwindSafe(|| {
            if n_code < HC_ACTION as i32 {
                return false;
            }
            let phase = match wparam.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => KeyPhase::Down,
                WM_KEYUP | WM_SYSKEYUP => KeyPhase::Up,
                _ => return false,
            };
            let pointer = lparam.0 as *const KBDLLHOOKSTRUCT;
            if pointer.is_null() {
                return false;
            }
            // SAFETY: Windows guarantees LPARAM points to a KBDLLHOOKSTRUCT for keyboard hook calls.
            let key = unsafe { *pointer };
            process_key_event(&key, phase)
        }))
        .map_err(|_| ()),
    );

    match decision {
        CallbackDecision::Handled => LRESULT(1),
        CallbackDecision::PassThrough => {
            // SAFETY: the hook contract requires every unhandled path to call the next hook with
            // the original values.
            unsafe { CallNextHookEx(None, n_code, wparam, lparam) }
        }
        CallbackDecision::PassThroughAndQuit => {
            // SAFETY: the current event must be forwarded before requesting this hook thread's
            // message loop to exit.
            let result = unsafe { CallNextHookEx(None, n_code, wparam, lparam) };
            // SAFETY: low-level callbacks execute on the installing thread, so this posts WM_QUIT
            // to the correct hook message loop. HookGuard unhooks after the loop exits.
            unsafe { PostQuitMessage(0) };
            result
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallbackDecision {
    Handled,
    PassThrough,
    PassThroughAndQuit,
}

fn callback_decision(result: Result<bool, ()>) -> CallbackDecision {
    match result {
        Ok(true) => CallbackDecision::Handled,
        Ok(false) => CallbackDecision::PassThrough,
        Err(()) => CallbackDecision::PassThroughAndQuit,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PhysicalKey {
    virtual_key: u32,
    scan_code: u32,
    extended: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyPhase {
    Down,
    Up,
}

struct KeyboardState {
    bytes: [u8; 256],
    down: [bool; 256],
}

impl KeyboardState {
    fn from_snapshot(mut bytes: [u8; 256]) -> Self {
        for (aggregate, left, right) in modifier_families() {
            let aggregate_down = key_is_down(&bytes, aggregate);
            let mut left_down = key_is_down(&bytes, left);
            let right_down = key_is_down(&bytes, right);
            if aggregate_down && !left_down && !right_down {
                left_down = true;
            }
            set_key_down(&mut bytes, left, left_down);
            set_key_down(&mut bytes, right, right_down);
            set_key_down(&mut bytes, aggregate, left_down || right_down);
        }
        let down = std::array::from_fn(|index| bytes[index] & 0x80 != 0);
        Self { bytes, down }
    }

    fn bytes(&self) -> &[u8; 256] {
        &self.bytes
    }

    fn apply(&mut self, key: PhysicalKey, phase: KeyPhase, injected: bool) {
        if injected {
            return;
        }
        let Ok(index) = usize::try_from(key.virtual_key) else {
            return;
        };
        if index >= self.bytes.len() {
            return;
        }
        match phase {
            KeyPhase::Down => {
                let first_down = !self.down[index];
                self.down[index] = true;
                self.bytes[index] |= 0x80;
                if first_down && is_toggle_key(key.virtual_key as u16) {
                    self.bytes[index] ^= 0x01;
                }
            }
            KeyPhase::Up => {
                self.down[index] = false;
                self.bytes[index] &= !0x80;
            }
        }
        if let Some((aggregate, left, right)) = modifier_family(key.virtual_key as u16) {
            let aggregate_index = usize::from(aggregate);
            match phase {
                KeyPhase::Down => {
                    self.down[aggregate_index] = true;
                    self.bytes[aggregate_index] |= 0x80;
                }
                KeyPhase::Up => {
                    let family_still_down =
                        self.down[usize::from(left)] || self.down[usize::from(right)];
                    self.down[aggregate_index] = family_still_down;
                    if family_still_down {
                        self.bytes[aggregate_index] |= 0x80;
                    } else {
                        self.bytes[aggregate_index] &= !0x80;
                    }
                }
            }
        }
    }
}

fn modifier_family(key: u16) -> Option<(u16, u16, u16)> {
    match key {
        key if key == VK_LSHIFT.0 || key == VK_RSHIFT.0 => {
            Some((VK_SHIFT.0, VK_LSHIFT.0, VK_RSHIFT.0))
        }
        key if key == VK_LCONTROL.0 || key == VK_RCONTROL.0 => {
            Some((VK_CONTROL.0, VK_LCONTROL.0, VK_RCONTROL.0))
        }
        key if key == VK_LMENU.0 || key == VK_RMENU.0 => Some((VK_MENU.0, VK_LMENU.0, VK_RMENU.0)),
        _ => None,
    }
}

fn modifier_families() -> [(u16, u16, u16); 3] {
    [
        (VK_SHIFT.0, VK_LSHIFT.0, VK_RSHIFT.0),
        (VK_CONTROL.0, VK_LCONTROL.0, VK_RCONTROL.0),
        (VK_MENU.0, VK_LMENU.0, VK_RMENU.0),
    ]
}

fn set_key_down(state: &mut [u8; 256], key: u16, down: bool) {
    let value = &mut state[usize::from(key)];
    if down {
        *value |= 0x80;
    } else {
        *value &= !0x80;
    }
}

fn is_toggle_key(key: u16) -> bool {
    key == VK_CAPITAL.0 || key == VK_NUMLOCK.0 || key == VK_SCROLL.0
}

fn snapshot_keyboard_state() -> KeyboardState {
    snapshot_keyboard_state_with(
        |virtual_key| {
            // SAFETY: the provider receives only fixed modifier, Windows, and toggle virtual-key
            // identifiers. The high bit is the current physical state.
            (unsafe { GetAsyncKeyState(i32::from(virtual_key)) }) < 0
        },
        |virtual_key| {
            // SAFETY: the provider receives only fixed toggle virtual-key identifiers.
            (unsafe { GetKeyState(i32::from(virtual_key)) }) & 0x01 != 0
        },
    )
}

fn snapshot_keyboard_state_with(
    mut is_down: impl FnMut(u16) -> bool,
    mut is_toggled: impl FnMut(u16) -> bool,
) -> KeyboardState {
    let mut bytes = [0_u8; 256];
    for virtual_key in [
        VK_SHIFT.0,
        VK_LSHIFT.0,
        VK_RSHIFT.0,
        VK_CONTROL.0,
        VK_LCONTROL.0,
        VK_RCONTROL.0,
        VK_MENU.0,
        VK_LMENU.0,
        VK_RMENU.0,
        VK_LWIN.0,
        VK_RWIN.0,
        VK_CAPITAL.0,
        VK_NUMLOCK.0,
        VK_SCROLL.0,
    ] {
        if is_down(virtual_key) {
            bytes[usize::from(virtual_key)] |= 0x80;
        }
    }
    for virtual_key in [VK_CAPITAL.0, VK_NUMLOCK.0, VK_SCROLL.0] {
        if is_toggled(virtual_key) {
            bytes[usize::from(virtual_key)] |= 0x01;
        }
    }
    KeyboardState::from_snapshot(bytes)
}

fn foreground_keyboard_layout() -> Option<HKL> {
    // SAFETY: both APIs query opaque window/thread/layout identifiers without attaching input
    // queues or changing keyboard state.
    let foreground_thread = unsafe { GetWindowThreadProcessId(GetForegroundWindow(), None) };
    if foreground_thread == 0 {
        return None;
    }
    let layout = unsafe { GetKeyboardLayout(foreground_thread) };
    (!layout.is_invalid()).then_some(layout)
}

fn normalize_modifier_key(key: &KBDLLHOOKSTRUCT, layout: Option<HKL>) -> PhysicalKey {
    let physical = PhysicalKey {
        virtual_key: key.vkCode,
        scan_code: key.scanCode,
        extended: key.flags.0 & LLKHF_EXTENDED.0 != 0,
    };
    let mapped_shift = if key.vkCode == u32::from(VK_SHIFT.0) {
        layout.map_or(0, |layout| {
            // SAFETY: the scan code comes from Windows and the layout belongs to the foreground
            // thread. This call only maps identifiers and does not mutate keyboard state.
            unsafe { MapVirtualKeyExW(key.scanCode, MAPVK_VSC_TO_VK_EX, Some(layout)) }
        })
    } else {
        0
    };
    normalize_modifier_physical_key(physical, mapped_shift)
}

fn normalize_modifier_physical_key(mut key: PhysicalKey, mapped_shift: u32) -> PhysicalKey {
    key.virtual_key = if key.virtual_key == u32::from(VK_SHIFT.0) {
        if mapped_shift == u32::from(VK_LSHIFT.0) || mapped_shift == u32::from(VK_RSHIFT.0) {
            mapped_shift
        } else if key.scan_code == 0x36 {
            u32::from(VK_RSHIFT.0)
        } else {
            u32::from(VK_LSHIFT.0)
        }
    } else if key.virtual_key == u32::from(VK_CONTROL.0) {
        if key.extended {
            u32::from(VK_RCONTROL.0)
        } else {
            u32::from(VK_LCONTROL.0)
        }
    } else if key.virtual_key == u32::from(VK_MENU.0) {
        if key.extended {
            u32::from(VK_RMENU.0)
        } else {
            u32::from(VK_LMENU.0)
        }
    } else {
        key.virtual_key
    };
    key
}

fn process_key_event(key: &KBDLLHOOKSTRUCT, phase: KeyPhase) -> bool {
    let physical = PhysicalKey {
        virtual_key: key.vkCode,
        scan_code: key.scanCode,
        extended: key.flags.0 & LLKHF_EXTENDED.0 != 0,
    };
    let injected = key.flags.0 & LLKHF_INJECTED.0 != 0;
    let layout = (!injected).then(foreground_keyboard_layout).flatten();
    let normalized = normalize_modifier_key(key, layout);

    HOOK_CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(context) = slot.as_mut() else {
            return false;
        };
        context.keyboard.apply(normalized, phase, injected);
        if injected || phase == KeyPhase::Up {
            return pair_key_event(&mut context.swallowed, physical, phase, injected, false);
        }

        let repeated = context.swallowed.contains(&physical);
        let delivered = translate_key_input(key, context.keyboard.bytes(), layout)
            .and_then(route_key)
            .is_some_and(|action| match context.sender.try_send(action) {
                Ok(()) => true,
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
            });
        if repeated {
            return true;
        }
        pair_key_event(&mut context.swallowed, physical, phase, false, delivered)
    })
}

fn pair_key_event(
    swallowed: &mut HashSet<PhysicalKey>,
    key: PhysicalKey,
    phase: KeyPhase,
    injected: bool,
    first_down_delivered: bool,
) -> bool {
    if injected {
        return false;
    }
    match phase {
        KeyPhase::Up => swallowed.remove(&key),
        KeyPhase::Down if swallowed.contains(&key) => true,
        KeyPhase::Down if first_down_delivered => {
            swallowed.insert(key);
            true
        }
        KeyPhase::Down => false,
    }
}

fn translate_key_input(
    key: &KBDLLHOOKSTRUCT,
    state: &[u8; 256],
    layout: Option<HKL>,
) -> Option<KeyInput> {
    translate_key_input_with(key, state, || translate_text(key, state, layout))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModifierPolicy {
    Plain,
    Shortcut,
    AltGr,
}

fn modifier_policy(state: &[u8; 256]) -> ModifierPolicy {
    let windows = key_is_down(state, VK_LWIN.0) || key_is_down(state, VK_RWIN.0);
    let left_control = key_is_down(state, VK_LCONTROL.0);
    let right_control = key_is_down(state, VK_RCONTROL.0);
    let left_alt = key_is_down(state, VK_LMENU.0);
    let right_alt = key_is_down(state, VK_RMENU.0);
    if windows {
        ModifierPolicy::Shortcut
    } else if left_control && right_alt && !left_alt && !right_control {
        // Windows commonly synthesizes Left Control while AltGr (Right Alt) is held.
        ModifierPolicy::AltGr
    } else if left_control || right_control || left_alt || right_alt {
        ModifierPolicy::Shortcut
    } else {
        ModifierPolicy::Plain
    }
}

fn translate_key_input_with(
    key: &KBDLLHOOKSTRUCT,
    state: &[u8; 256],
    translate: impl FnOnce() -> Option<String>,
) -> Option<KeyInput> {
    match modifier_policy(state) {
        ModifierPolicy::Shortcut => None,
        ModifierPolicy::AltGr => translate()
            .filter(|text| {
                !text.is_empty() && text.chars().all(|character| !character.is_control())
            })
            .map(KeyInput::Text),
        ModifierPolicy::Plain => match key.vkCode as u16 {
            value if value == VK_BACK.0 => Some(KeyInput::Backspace),
            value if value == VK_UP.0 => Some(KeyInput::ArrowUp),
            value if value == VK_DOWN.0 => Some(KeyInput::ArrowDown),
            value if value == VK_LEFT.0 => Some(KeyInput::ArrowLeft),
            value if value == VK_RIGHT.0 => Some(KeyInput::ArrowRight),
            value if value == VK_RETURN.0 => Some(KeyInput::Enter),
            value if value == VK_ESCAPE.0 => Some(KeyInput::Escape),
            _ => translate().map(KeyInput::Text),
        },
    }
}

fn key_is_down(state: &[u8; 256], key: u16) -> bool {
    state
        .get(usize::from(key))
        .is_some_and(|value| value & 0x80 != 0)
}

#[cfg(test)]
fn key_is_toggled(state: &[u8; 256], key: u16) -> bool {
    state
        .get(usize::from(key))
        .is_some_and(|value| value & 0x01 != 0)
}

fn translate_text(key: &KBDLLHOOKSTRUCT, state: &[u8; 256], layout: Option<HKL>) -> Option<String> {
    let layout = layout?;
    let mut output = [0_u16; 16];
    // SAFETY: all slices are valid, the scan/vk codes came from Windows, and the current foreground
    // window's owning thread selects the active input layout. Bit 2 prevents ToUnicodeEx from
    // mutating the keyboard buffer's dead-key state on supported Windows 10/11 builds.
    let count = unsafe {
        ToUnicodeEx(
            key.vkCode,
            key.scanCode,
            state,
            &mut output,
            0x4,
            Some(layout),
        )
    };
    decode_translated_text(count, &output)
}

fn decode_translated_text(count: i32, output: &[u16]) -> Option<String> {
    if count <= 0 {
        return None;
    }
    let count = usize::try_from(count).ok()?.min(output.len());
    String::from_utf16(&output[..count]).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        CallbackDecision, HookLifecycle, KeyAction, KeyInput, KeyPhase, KeyboardState,
        ModifierPolicy, PhysicalKey, ShutdownDecision, callback_decision, decode_translated_text,
        initialize_hook_snapshot_context, key_is_down, key_is_toggled, modifier_policy,
        normalize_modifier_key, normalize_modifier_physical_key, pair_key_event, route_key,
        shutdown_decision, snapshot_keyboard_state_with, translate_key_input_with,
    };
    use std::cell::RefCell;
    use windows::Win32::UI::WindowsAndMessaging::{
        KBDLLHOOKSTRUCT, KBDLLHOOKSTRUCT_FLAGS, LLKHF_EXTENDED,
    };

    fn hook_key(virtual_key: u32, scan_code: u32, extended: bool) -> KBDLLHOOKSTRUCT {
        KBDLLHOOKSTRUCT {
            vkCode: virtual_key,
            scanCode: scan_code,
            flags: if extended {
                LLKHF_EXTENDED
            } else {
                KBDLLHOOKSTRUCT_FLAGS::default()
            },
            ..KBDLLHOOKSTRUCT::default()
        }
    }

    fn state_with_down_keys(keys: &[usize]) -> KeyboardState {
        let mut bytes = [0_u8; 256];
        for &key in keys {
            bytes[key] = 0x80;
        }
        KeyboardState::from_snapshot(bytes)
    }

    #[test]
    fn hook_install_precedes_snapshot_and_context_publication() {
        let events = RefCell::new(Vec::new());

        let installed = initialize_hook_snapshot_context(
            || {
                events.borrow_mut().push("install");
                Ok::<_, ()>("hook")
            },
            || {
                events.borrow_mut().push("snapshot");
                KeyboardState::from_snapshot([0_u8; 256])
            },
            |_| events.borrow_mut().push("context"),
        )
        .unwrap();

        assert_eq!(installed, "hook");
        assert_eq!(*events.borrow(), ["install", "snapshot", "context"]);
    }

    #[test]
    fn startup_snapshot_queries_only_modifiers_windows_and_toggle_keys() {
        let async_queries = RefCell::new(Vec::new());
        let toggle_queries = RefCell::new(Vec::new());

        let _ = snapshot_keyboard_state_with(
            |key| {
                async_queries.borrow_mut().push(key);
                false
            },
            |key| {
                toggle_queries.borrow_mut().push(key);
                false
            },
        );

        assert_eq!(
            *async_queries.borrow(),
            [
                0x10, 0xA0, 0xA1, 0x11, 0xA2, 0xA3, 0x12, 0xA4, 0xA5, 0x5B, 0x5C, 0x14, 0x90, 0x91
            ]
        );
        assert_eq!(*toggle_queries.borrow(), [0x14, 0x90, 0x91]);
    }

    #[test]
    fn visible_keyboard_router_maps_text_and_editing_keys() {
        assert_eq!(
            route_key(KeyInput::Character('A')),
            Some(KeyAction::SearchText("A".into()))
        );
        assert_eq!(
            route_key(KeyInput::Text("你好".into())),
            Some(KeyAction::SearchText("你好".into()))
        );
        assert_eq!(
            route_key(KeyInput::Character('你')),
            Some(KeyAction::SearchText("你".into()))
        );
        assert_eq!(
            route_key(KeyInput::Backspace),
            Some(KeyAction::DeleteBackward)
        );
    }

    #[test]
    fn visible_keyboard_router_maps_navigation_paste_and_hide() {
        assert_eq!(route_key(KeyInput::ArrowUp), Some(KeyAction::Move(-1)));
        assert_eq!(route_key(KeyInput::ArrowDown), Some(KeyAction::Move(1)));
        assert_eq!(route_key(KeyInput::ArrowLeft), None);
        assert_eq!(route_key(KeyInput::ArrowRight), None);
        assert_eq!(route_key(KeyInput::Enter), Some(KeyAction::Paste));
        assert_eq!(route_key(KeyInput::Escape), Some(KeyAction::Hide));
    }

    #[test]
    fn router_ignores_control_alt_and_windows_shortcuts() {
        for input in [
            KeyInput::Modified {
                control: true,
                shift: false,
                alt: false,
                windows: false,
                key: 'C',
            },
            KeyInput::Modified {
                control: false,
                shift: false,
                alt: true,
                windows: false,
                key: 'x',
            },
            KeyInput::Modified {
                control: false,
                shift: true,
                alt: false,
                windows: true,
                key: 'R',
            },
        ] {
            assert_eq!(route_key(input), None);
        }
    }

    #[test]
    fn character_input_preserves_layout_and_shift_output() {
        assert_eq!(
            route_key(KeyInput::Character('!')),
            Some(KeyAction::SearchText("!".into()))
        );
        assert_eq!(
            route_key(KeyInput::Character('é')),
            Some(KeyAction::SearchText("é".into()))
        );
    }

    #[test]
    fn control_characters_are_not_added_to_search() {
        assert_eq!(route_key(KeyInput::Character('\0')), None);
        assert_eq!(route_key(KeyInput::Character('\n')), None);
    }

    #[test]
    fn unicode_translation_preserves_multiple_units_and_rejects_dead_or_invalid_text() {
        assert_eq!(
            decode_translated_text(3, &[b'A' as u16, b'B' as u16, b'C' as u16]),
            Some("ABC".into())
        );
        assert_eq!(
            decode_translated_text(2, &[0xd83d, 0xde00]),
            Some("😀".into())
        );
        assert_eq!(decode_translated_text(-1, &[b'A' as u16]), None);
        assert_eq!(decode_translated_text(1, &[0xd800]), None);
    }

    #[test]
    fn keyboard_queue_state_reads_shift_down_and_caps_toggle_bits() {
        let mut state = [0_u8; 256];
        state[0x10] = 0x80;
        state[0x14] = 0x01;

        assert!(key_is_down(&state, 0x10));
        assert!(key_is_toggled(&state, 0x14));
        assert!(!key_is_down(&state, 0x14));
        assert!(!key_is_toggled(&state, 0x10));
    }

    #[test]
    fn startup_snapshot_preserves_control_shift_and_toggle_state() {
        let mut bytes = [0_u8; 256];
        bytes[0x11] = 0x80;
        bytes[0x10] = 0x80;
        bytes[0x14] = 0x01;
        let state = KeyboardState::from_snapshot(bytes);

        assert!(key_is_down(state.bytes(), 0x11));
        assert!(key_is_down(state.bytes(), 0x10));
        assert!(key_is_toggled(state.bytes(), 0x14));
    }

    #[test]
    fn generic_modifier_keyups_clear_snapshot_left_keys_and_aggregates() {
        let mut bytes = [0_u8; 256];
        for virtual_key in [0x10, 0x11, 0xA0, 0xA2] {
            bytes[virtual_key] = 0x80;
        }
        let mut state = KeyboardState::from_snapshot(bytes);
        let raw_control = hook_key(0x11, 0x1D, false);
        let raw_shift = hook_key(0x10, 0x2A, false);

        state.apply(
            normalize_modifier_key(&raw_control, None),
            KeyPhase::Up,
            false,
        );
        state.apply(
            normalize_modifier_key(&raw_shift, None),
            KeyPhase::Up,
            false,
        );

        assert!(!key_is_down(state.bytes(), 0xA2));
        assert!(!key_is_down(state.bytes(), 0x11));
        assert!(!key_is_down(state.bytes(), 0xA0));
        assert!(!key_is_down(state.bytes(), 0x10));
    }

    #[test]
    fn extended_generic_control_and_alt_normalize_to_right_keys() {
        for (virtual_key, scan_code, expected) in [(0x11, 0x1D, 0xA3), (0x12, 0x38, 0xA5)] {
            let raw = hook_key(virtual_key, scan_code, true);

            assert_eq!(normalize_modifier_key(&raw, None).virtual_key, expected);
        }
    }

    #[test]
    fn generic_shift_uses_mapped_side_and_scan_code_fallback() {
        let right_shift = hook_key(0x10, 0x36, false);
        let left_shift = hook_key(0x10, 0x2A, false);
        let raw_shift = PhysicalKey {
            virtual_key: 0x10,
            scan_code: 0x2A,
            extended: false,
        };

        assert_eq!(
            normalize_modifier_physical_key(raw_shift, 0xA1).virtual_key,
            0xA1
        );
        assert_eq!(normalize_modifier_key(&right_shift, None).virtual_key, 0xA1);
        assert_eq!(normalize_modifier_key(&left_shift, None).virtual_key, 0xA0);
    }

    #[test]
    fn releasing_one_side_keeps_modifier_aggregate_down_until_both_are_up() {
        let mut state = KeyboardState::from_snapshot([0_u8; 256]);
        let left = PhysicalKey {
            virtual_key: 0xA2,
            scan_code: 0x1D,
            extended: false,
        };
        let right = PhysicalKey {
            virtual_key: 0xA3,
            scan_code: 0x1D,
            extended: true,
        };

        state.apply(left, KeyPhase::Down, false);
        state.apply(right, KeyPhase::Down, false);
        state.apply(left, KeyPhase::Up, false);

        assert!(!key_is_down(state.bytes(), 0xA2));
        assert!(key_is_down(state.bytes(), 0xA3));
        assert!(key_is_down(state.bytes(), 0x11));
    }

    #[test]
    fn generic_only_startup_modifier_states_are_canonicalized_to_left_sides() {
        for (aggregate, left, right) in [
            (0x10_u16, 0xA0_u16, 0xA1_u16),
            (0x11_u16, 0xA2_u16, 0xA3_u16),
            (0x12_u16, 0xA4_u16, 0xA5_u16),
        ] {
            let mut bytes = [0_u8; 256];
            bytes[usize::from(aggregate)] = 0x80;

            let state = KeyboardState::from_snapshot(bytes);

            assert!(key_is_down(state.bytes(), left));
            assert!(!key_is_down(state.bytes(), right));
            assert!(key_is_down(state.bytes(), aggregate));
        }
    }

    #[test]
    fn shift_down_letter_state_and_keyups_follow_event_order() {
        let mut state = KeyboardState::from_snapshot([0_u8; 256]);
        let shift = PhysicalKey {
            virtual_key: 0xA0,
            scan_code: 42,
            extended: false,
        };
        let letter = PhysicalKey {
            virtual_key: u32::from(b'A'),
            scan_code: 30,
            extended: false,
        };

        state.apply(shift, KeyPhase::Down, false);
        assert!(key_is_down(state.bytes(), 0xA0));
        assert!(key_is_down(state.bytes(), 0x10));
        state.apply(letter, KeyPhase::Down, false);
        assert!(key_is_down(state.bytes(), u16::from(b'A')));
        assert!(key_is_down(state.bytes(), 0x10));
        state.apply(letter, KeyPhase::Up, false);
        assert!(!key_is_down(state.bytes(), u16::from(b'A')));
        assert!(key_is_down(state.bytes(), 0x10));
        state.apply(shift, KeyPhase::Up, false);
        assert!(!key_is_down(state.bytes(), 0xA0));
        assert!(!key_is_down(state.bytes(), 0x10));
    }

    #[test]
    fn left_control_alt_and_windows_keys_block_clipboard_shortcuts() {
        for (virtual_key, aggregate) in [(0xA2, 0x11), (0xA4, 0x12), (0x5B, 0x5B)] {
            let mut state = KeyboardState::from_snapshot([0_u8; 256]);
            let modifier = PhysicalKey {
                virtual_key,
                scan_code: 0,
                extended: false,
            };

            state.apply(modifier, KeyPhase::Down, false);

            assert!(key_is_down(state.bytes(), aggregate));
            assert_eq!(modifier_policy(state.bytes()), ModifierPolicy::Shortcut);
        }
    }

    #[test]
    fn altgr_candidate_accepts_printable_complete_translation_strings() {
        let state = state_with_down_keys(&[0xA2, 0xA5]);
        let key = hook_key(u32::from(b'Q'), 0x10, false);

        for text in ["@", "€", "ab"] {
            assert_eq!(
                translate_key_input_with(&key, state.bytes(), || Some(text.into())),
                Some(KeyInput::Text(text.into()))
            );
        }
    }

    #[test]
    fn right_alt_without_left_control_is_a_shortcut_and_never_translates() {
        let state = state_with_down_keys(&[0xA5]);
        let key = hook_key(u32::from(b'Q'), 0x10, false);

        assert_eq!(
            translate_key_input_with(&key, state.bytes(), || {
                panic!("Right Alt alone must not invoke text translation")
            }),
            None
        );
    }

    #[test]
    fn ordinary_control_alt_and_windows_combinations_pass_through() {
        for keys in [&[0xA2, 0xA4][..], &[0xA3, 0xA5][..], &[0x5B][..]] {
            let state = state_with_down_keys(keys);
            let key = hook_key(u32::from(b'Q'), 0x10, false);

            assert_eq!(
                translate_key_input_with(&key, state.bytes(), || {
                    panic!("shortcut translation must not run")
                }),
                None
            );
        }
    }

    #[test]
    fn altgr_enter_never_maps_to_paste_and_control_text_is_rejected() {
        let state = state_with_down_keys(&[0xA2, 0xA5]);
        let enter = hook_key(0x0D, 0x1C, false);

        let action = translate_key_input_with(&enter, state.bytes(), || Some("\r".into()))
            .and_then(route_key);

        assert_eq!(action, None);
    }

    #[test]
    fn altgr_without_translated_text_passes_through() {
        let state = state_with_down_keys(&[0xA2, 0xA5]);
        let key = hook_key(u32::from(b'Q'), 0x10, false);

        assert_eq!(translate_key_input_with(&key, state.bytes(), || None), None);
    }

    #[test]
    fn caps_lock_toggles_only_on_first_down_not_repeat_or_keyup() {
        let mut state = KeyboardState::from_snapshot([0_u8; 256]);
        let caps = PhysicalKey {
            virtual_key: 0x14,
            scan_code: 58,
            extended: false,
        };

        state.apply(caps, KeyPhase::Down, false);
        assert!(key_is_toggled(state.bytes(), 0x14));
        state.apply(caps, KeyPhase::Down, false);
        assert!(key_is_toggled(state.bytes(), 0x14));
        state.apply(caps, KeyPhase::Up, false);
        assert!(key_is_toggled(state.bytes(), 0x14));
    }

    #[test]
    fn injected_events_never_mutate_keyboard_state() {
        let mut state = KeyboardState::from_snapshot([0_u8; 256]);
        let shift = PhysicalKey {
            virtual_key: 0x10,
            scan_code: 42,
            extended: false,
        };

        state.apply(shift, KeyPhase::Down, true);

        assert!(!key_is_down(state.bytes(), 0x10));
    }

    #[test]
    fn swallowed_key_state_pairs_keyup_and_keeps_autorepeat_swallowed() {
        let key = PhysicalKey {
            virtual_key: b'A' as u32,
            scan_code: 30,
            extended: false,
        };
        let mut swallowed = std::collections::HashSet::new();

        assert!(pair_key_event(
            &mut swallowed,
            key,
            KeyPhase::Down,
            false,
            true,
        ));
        assert!(pair_key_event(
            &mut swallowed,
            key,
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(pair_key_event(
            &mut swallowed,
            key,
            KeyPhase::Up,
            false,
            false,
        ));
        assert!(swallowed.is_empty());
    }

    #[test]
    fn channel_full_first_down_and_injected_events_are_fully_passed_through() {
        let key = PhysicalKey {
            virtual_key: b'B' as u32,
            scan_code: 48,
            extended: false,
        };
        let mut swallowed = std::collections::HashSet::new();

        assert!(!pair_key_event(
            &mut swallowed,
            key,
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(!pair_key_event(
            &mut swallowed,
            key,
            KeyPhase::Up,
            false,
            false,
        ));
        assert!(!pair_key_event(
            &mut swallowed,
            key,
            KeyPhase::Down,
            true,
            true,
        ));
        assert!(swallowed.is_empty());
    }

    #[test]
    fn hook_lifecycle_rejects_double_start_and_stops_idempotently() {
        let mut lifecycle = HookLifecycle::default();
        assert_eq!(lifecycle.start("first"), Ok(()));
        assert_eq!(lifecycle.start("second"), Err("second"));
        assert!(lifecycle.is_visible());
        assert_eq!(lifecycle.stop(), Some("first"));
        assert_eq!(lifecycle.stop(), None);
        assert!(!lifecycle.is_visible());
    }

    #[test]
    fn panic_callback_decision_passes_current_key_then_requests_hook_shutdown() {
        assert_eq!(
            callback_decision(Err(())),
            CallbackDecision::PassThroughAndQuit
        );
        assert_eq!(callback_decision(Ok(false)), CallbackDecision::PassThrough);
        assert_eq!(callback_decision(Ok(true)), CallbackDecision::Handled);
    }

    #[test]
    fn worker_shutdown_decision_never_messages_a_recycled_window() {
        assert_eq!(
            shutdown_decision(false, true, true),
            ShutdownDecision::CloseThenJoin
        );
        assert_eq!(
            shutdown_decision(true, true, false),
            ShutdownDecision::JoinOnly
        );
        assert_eq!(
            shutdown_decision(false, false, false),
            ShutdownDecision::JoinOnly
        );
        assert_eq!(
            shutdown_decision(false, true, false),
            ShutdownDecision::RetainAndError
        );
    }

    #[test]
    fn key_action_event_payload_has_a_stable_tagged_shape() {
        assert_eq!(
            serde_json::to_value(KeyAction::SearchText("A".into())).unwrap(),
            serde_json::json!({"type": "searchText", "value": "A"})
        );
        assert_eq!(
            serde_json::to_value(KeyAction::Paste).unwrap(),
            serde_json::json!({"type": "paste"})
        );
    }

    #[test]
    #[ignore = "installs a desktop-wide keyboard hook; run only in an interactive smoke session"]
    fn real_hook_is_scoped_to_visible_lifetime() {
        let router = super::VisibleKeyboardRouter::default();
        let receiver = router.show().unwrap();
        assert!(router.is_visible());
        router.hide().unwrap();
        assert!(!router.is_visible());
        assert!(receiver.recv().is_err());
    }
}
