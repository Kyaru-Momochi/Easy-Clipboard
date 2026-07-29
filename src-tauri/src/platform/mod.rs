#[cfg(windows)]
pub use crate::clipboard::{ClipboardListener, WindowsClipboard};

#[cfg(windows)]
pub mod keyboard;
#[cfg(windows)]
pub mod overlay;
#[cfg(windows)]
pub mod paste;
#[cfg(windows)]
pub mod window;

#[cfg(windows)]
pub use keyboard::{KeyAction, VisibleKeyboardRouter};
#[cfg(windows)]
pub use overlay::{
    OVERLAY_HIDE_EVENT, OverlayRuntime, SEARCH_INPUT_EVENT, SELECTION_MOVE_EVENT,
    SELECTION_PASTE_EVENT, SearchInputPayload, SelectionMovePayload,
};
#[cfg(windows)]
pub use paste::WindowsPasteTarget;
#[cfg(windows)]
pub use window::{OverlayController, OverlayPlacement, Rect, Size};
