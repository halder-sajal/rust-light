//! Linux backend — not implemented yet.
//!
//! Likely path when this gets built: PipeWire + xdg-desktop-portal's
//! ScreenCast portal (works across Wayland compositors and asks the user's
//! permission, similar in spirit to Windows.Graphics.Capture), with an X11
//! fallback (e.g. via `x11rb` + XShm/XComposite) for non-portal setups.
//! Nothing here is wired up yet — this module exists so `app` and `pipeline`
//! can be built/tested on Linux today, and so implementing this backend
//! later doesn't require touching any other crate.

use crate::{CaptureError, DisplayId, DisplayInfo, Frame, ScreenCapturer};

const NOT_IMPLEMENTED: &str = "Linux screen capture is not implemented yet";

/// Lists capturable displays without starting a capture session.
pub fn available_displays() -> Result<Vec<DisplayInfo>, CaptureError> {
    Err(CaptureError::NotImplemented(NOT_IMPLEMENTED))
}

pub struct LinuxCapturer;

impl LinuxCapturer {
    pub fn new() -> Result<Self, CaptureError> {
        Err(CaptureError::NotImplemented(NOT_IMPLEMENTED))
    }
}

impl ScreenCapturer for LinuxCapturer {
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
