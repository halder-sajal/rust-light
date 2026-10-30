//! Screen-capture abstraction. One backend per platform lives behind the
//! `ScreenCapturer` trait so the pipeline/app code never depends on how a
//! given OS captures frames — callers should use `PlatformCapturer` and
//! `available_displays()`, which always resolve to whichever backend
//! matches the build target, rather than naming a platform-specific type
//! directly.
//!
//! Windows is implemented (via `windows-capture` / Windows.Graphics.Capture).
//! Linux is implemented via xdg-desktop-portal's ScreenCast portal +
//! PipeWire (see `linux.rs` for the details and caveats - notably, there's
//! no X11-only fallback; a portal backend is required). macOS is scaffolded
//! but not implemented yet: the module exists, implements `ScreenCapturer`,
//! and compiles on that target, but every method returns
//! `CaptureError::NotImplemented` (likely path: ScreenCaptureKit).

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows::{available_displays, WindowsCapturer as PlatformCapturer};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::{available_displays, LinuxCapturer as PlatformCapturer};

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::{available_displays, MacosCapturer as PlatformCapturer};

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no displays found")]
    NoDisplays,
    #[error("capture backend error: {0}")]
    Backend(String),
    #[error("not implemented on this platform yet: {0}")]
    NotImplemented(&'static str),
}

/// One captured video frame.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, row-major, top-left origin.
    pub rgba: Vec<u8>,
}

/// Identifies a capturable display/monitor. Opaque until a backend defines
/// what it needs to hold.
#[derive(Debug, Clone)]
pub struct DisplayId(pub String);

/// Display metadata for UI purposes (picking a monitor in settings).
/// Listing displays doesn't require an active capture session.
#[derive(Debug, Clone)]
pub struct DisplayInfo {
    pub id: DisplayId,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// A source of screen frames. Implementations are platform-specific and
/// live outside this crate (or behind feature flags here) once chosen.
pub trait ScreenCapturer {
    fn list_displays(&self) -> Result<Vec<DisplayId>, CaptureError>;

    /// Capture one frame from the given display. Blocks until a frame is
    /// available — on backends that only deliver a frame when the screen
    /// content actually changes (e.g. Windows.Graphics.Capture), this can
    /// block for an arbitrarily long time on a static screen. Don't call
    /// this from a UI thread on a fixed timer; use `try_capture` there.
    fn capture(&mut self, display: &DisplayId) -> Result<Frame, CaptureError>;

    /// Non-blocking: returns the latest frame if a new one has arrived
    /// since the last call, or `Ok(None)` immediately if not. Safe to poll
    /// frequently from a UI thread without risking a stall.
    fn try_capture(&mut self, display: &DisplayId) -> Result<Option<Frame>, CaptureError>;
}
