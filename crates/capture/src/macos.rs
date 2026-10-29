//! macOS backend — not implemented yet.
//!
//! Likely path when this gets built: ScreenCaptureKit (macOS 12.3+), which
//! is push-based like Windows.Graphics.Capture, so the same channel-bridge
//! pattern used in `windows.rs` (push callback -> size-1 `sync_channel` ->
//! pull-based `capture`/`try_capture`) should carry over directly. Nothing
//! here is wired up yet — this module exists so `app` and `pipeline` can be
//! built/tested on macOS today, and so implementing this backend later
//! doesn't require touching any other crate.

use crate::{CaptureError, DisplayId, DisplayInfo, Frame, ScreenCapturer};

const NOT_IMPLEMENTED: &str = "macOS screen capture is not implemented yet";

/// Lists capturable displays without starting a capture session.
pub fn available_displays() -> Result<Vec<DisplayInfo>, CaptureError> {
    Err(CaptureError::NotImplemented(NOT_IMPLEMENTED))
}

pub struct MacosCapturer;

impl MacosCapturer {
    pub fn new() -> Result<Self, CaptureError> {
        Err(CaptureError::NotImplemented(NOT_IMPLEMENTED))
    }
}

impl ScreenCapturer for MacosCapturer {
    fn list_displays(&self) -> Result<Vec<DisplayId>, CaptureError> {
        Err(CaptureError::NotImplemented(NOT_IMPLEMENTED))
    }

    fn capture(&mut self, _display: &DisplayId) -> Result<Frame, CaptureError> {
        Err(CaptureError::NotImplemented(NOT_IMPLEMENTED))
    }

    fn try_capture(&mut self, _display: &DisplayId) -> Result<Option<Frame>, CaptureError> {
        Err(CaptureError::NotImplemented(NOT_IMPLEMENTED))
    }
}
