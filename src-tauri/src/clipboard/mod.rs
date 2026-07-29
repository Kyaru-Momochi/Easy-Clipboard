mod coordinator;
mod normalize;
#[cfg(windows)]
mod windows;

pub use coordinator::{
    CaptureCoordinator, CaptureResult, ClipboardBackend, Clock, PasteTarget, SystemClock,
};
pub use normalize::{
    NormalizeOutcome, NormalizedClipboard, PendingImageResource, RawClipboardSnapshot, normalize,
};
#[cfg(windows)]
pub use windows::{ClipboardListener, WindowsClipboard};
