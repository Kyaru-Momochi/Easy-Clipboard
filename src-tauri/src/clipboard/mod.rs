mod coordinator;
mod normalize;

pub use coordinator::{
    CaptureCoordinator, CaptureResult, ClipboardBackend, Clock, PasteTarget, SystemClock,
};
pub use normalize::{
    NormalizeOutcome, NormalizedClipboard, PendingImageResource, RawClipboardSnapshot, normalize,
};
