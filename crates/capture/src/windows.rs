//! Windows backend: wraps `windows-capture` (Windows.Graphics.Capture).
//!
//! `windows-capture`'s API is push-based — it calls `on_frame_arrived` from
//! its own capture thread. `ScreenCapturer::capture` is pull-based, so this
//! module bridges the two with a size-1 channel: only the most recent frame
//! is kept, older ones are dropped. Ambient lighting only ever wants the
//! current screen state, never a backlog.

use crate::{CaptureError, DisplayId, DisplayInfo, Frame, ScreenCapturer};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame as WcFrame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

type HandlerError = Box<dyn std::error::Error + Send + Sync>;

struct FrameSink {
    tx: SyncSender<Frame>,
}

impl GraphicsCaptureApiHandler for FrameSink {
    type Flags = SyncSender<Frame>;
    type Error = HandlerError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { tx: ctx.flags })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut WcFrame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let width = frame.width();
        let height = frame.height();
        let buffer = frame.buffer()?;
        let mut scratch = Vec::new();
        let rgba = buffer.as_nopadding_buffer(&mut scratch).to_vec();

        // Drop the frame if the consumer hasn't kept up — never block the
        // capture thread waiting for a slow reader.
        let _ = self.tx.try_send(Frame { width, height, rgba });
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Lists capturable monitors without starting a capture session — for
/// populating a display picker in settings UI.
pub fn available_displays() -> Result<Vec<DisplayInfo>, CaptureError> {
    let monitors = Monitor::enumerate().map_err(|e| CaptureError::Backend(e.to_string()))?;
    if monitors.is_empty() {
        return Err(CaptureError::NoDisplays);
    }
    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let width = m.width().map_err(|e| CaptureError::Backend(e.to_string()))?;
            let height = m.height().map_err(|e| CaptureError::Backend(e.to_string()))?;
            Ok(DisplayInfo {
                id: DisplayId(i.to_string()),
                name: format!("Display {} — {width}x{height}", i + 1),
                width,
                height,
            })
        })
        .collect()
}

/// Captures the primary monitor via Windows.Graphics.Capture.
///
/// Multi-monitor selection isn't wired up yet — `capture()`'s `display`
/// argument is currently ignored and only the primary monitor is captured.
pub struct WindowsCapturer {
    rx: Receiver<Frame>,
    control: Option<CaptureControl<FrameSink, HandlerError>>,
}

impl WindowsCapturer {
    pub fn new() -> Result<Self, CaptureError> {
        let monitor = Monitor::primary().map_err(|e| CaptureError::Backend(e.to_string()))?;
        let (tx, rx) = sync_channel(1);

        let settings = Settings::new(
            monitor,
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Rgba8,
            tx,
        );

        let control = FrameSink::start_free_threaded(settings)
            .map_err(|e| CaptureError::Backend(e.to_string()))?;

        Ok(Self {
            rx,
            control: Some(control),
        })
    }
}

impl ScreenCapturer for WindowsCapturer {
    fn list_displays(&self) -> Result<Vec<DisplayId>, CaptureError> {
        Ok(available_displays()?.into_iter().map(|d| d.id).collect())
    }

    /// Blocks until the next frame arrives from the primary monitor.
    fn capture(&mut self, _display: &DisplayId) -> Result<Frame, CaptureError> {
        self.rx.recv().map_err(|e| CaptureError::Backend(e.to_string()))
    }

    fn try_capture(&mut self, _display: &DisplayId) -> Result<Option<Frame>, CaptureError> {
        match self.rx.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(e) => Err(CaptureError::Backend(e.to_string())),
        }
    }
}

impl Drop for WindowsCapturer {
    fn drop(&mut self) {
        if let Some(control) = self.control.take() {
            let _ = control.stop();
        }
    }
}
