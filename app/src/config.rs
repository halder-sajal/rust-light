//! On-disk settings, persisted as TOML under the OS config directory.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub monitor_index: usize,
    /// 0 means "native" — no downscale before processing.
    pub capture_width: u32,
    pub capture_height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LedType {
    Rgb,
    Rgbw,
}

impl Default for LedType {
    fn default() -> Self {
        LedType::Rgb
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Clockwise,
    CounterClockwise,
}

impl Default for Direction {
    fn default() -> Self {
        Direction::Clockwise
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct EdgeCounts {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

impl EdgeCounts {
    pub fn total(&self) -> u32 {
        self.top + self.right + self.bottom + self.left
    }

    pub fn for_edge(&self, edge: Edge) -> u32 {
        match edge {
            Edge::Top => self.top,
            Edge::Right => self.right,
            Edge::Bottom => self.bottom,
            Edge::Left => self.left,
        }
    }
}

/// Where logical LED #1 sits: `offset` LEDs into `edge`, counted from that
/// edge's canonical clockwise start (Top: from the left; Right: from the
/// top; Bottom: from the right; Left: from the bottom).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StartPosition {
    pub edge: Edge,
    pub offset: u32,
}

impl Default for StartPosition {
    fn default() -> Self {
        Self { edge: Edge::Top, offset: 0 }
    }
}

/// A physical break in the strip: it eats into `edge`'s length, so that
/// edge's LEDs compact to fit in the remaining space. `after_led` is a
/// 0-based count of LEDs on that edge (in the edge's own canonical walk
/// order — see `led_layout`) that come before the gap. `size` is in
/// LED-slot-equivalent units (e.g. 5 means "as wide as 5 LEDs").
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Gap {
    pub edge: Edge,
    pub after_led: u32,
    pub size: f32,
}

/// One LED's screen-sampling rectangle, in coordinates relative to the
/// captured frame (0.0..1.0 on each axis) so it's resolution-independent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LedZone {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedConfig {
    pub led_type: LedType,
    pub counts: EdgeCounts,
    pub direction: Direction,
    pub start: StartPosition,
    #[serde(default)]
    pub gaps: Vec<Gap>,
    pub sampling_depth_pct: f32,
    /// 0.0..100.0, 0.0 is the default. How much adjacent edges' LEDs
    /// overlap near a shared corner: 0 keeps a clean diagonal taper (a
    /// corner-most LED samples a thin sliver), 100 disables the taper
    /// entirely (full depth everywhere, like before corner tapering
    /// existed) — useful so corner content (e.g. a letterboxed video
    /// reaching into the corner) doesn't get missed. See `led_layout`.
    #[serde(default)]
    pub corner_overlap_pct: f32,
    /// Explicit per-LED zones, in logical (final, numbered) order. Empty,
    /// or a length mismatch against `counts.total()`, means "recompute
    /// defaults" — see `led_layout::compute_default_zones`.
    #[serde(default)]
    pub zones: Vec<LedZone>,
}

impl Default for LedConfig {
    fn default() -> Self {
        Self {
            led_type: LedType::default(),
            counts: EdgeCounts { top: 20, right: 10, bottom: 20, left: 10 },
            direction: Direction::default(),
            start: StartPosition::default(),
            gaps: Vec::new(),
            sampling_depth_pct: 10.0,
            corner_overlap_pct: 0.0,
            zones: Vec::new(),
        }
    }
}

fn default_smoothing() -> f32 {
    0.0
}
fn default_threshold() -> f32 {
    50.0
}
fn default_gamma() -> f32 {
    50.0
}
fn default_color_temp() -> f32 {
    50.0
}
fn default_letterbox_cutoff() -> f32 {
    0.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ProcessingConfig {
    /// 0.0..100.0 — how much each LED blends with its two neighbors.
    #[serde(default = "default_smoothing")]
    pub smoothing: f32,
    /// 0.0..100.0, 50.0 is neutral. Below 50: crushes dark colors toward
    /// black (stronger the lower it goes). Above 50: lifts/brightens
    /// everything, including blacks (stronger the higher it goes).
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    /// 0.0..100.0, 50.0 is neutral. Below 50: brightens midtones (gamma
    /// < 1.0). Above 50: darkens midtones (gamma > 1.0). Compensates for
    /// the LED strip's actual color output not matching the screen.
    #[serde(default = "default_gamma")]
    pub gamma: f32,
    /// 0.0..100.0, 50.0 is neutral. Below 50: cooler (boosts blue, cuts
    /// red). Above 50: warmer (boosts red, cuts blue). Compensates for
    /// the LED strip's white point not matching the screen's.
    #[serde(default = "default_color_temp")]
    pub color_temp: f32,
    /// 0.0..100.0, 0.0 = plain average. Higher values de-weight dark
    /// pixels in each zone's average (more strongly near the zone's outer
    /// edge than its inner/center-facing edge), so a letterbox bar or
    /// other dark border doesn't dilute a bright highlight in the sample.
    #[serde(default = "default_letterbox_cutoff")]
    pub letterbox_cutoff: f32,
}

impl Default for ProcessingConfig {
    fn default() -> Self {
        Self { smoothing: 0.0, threshold: 50.0, gamma: 50.0, color_temp: 50.0, letterbox_cutoff: 0.0 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
    #[serde(default)]
    pub led: LedConfig,
    #[serde(default)]
    pub processing: ProcessingConfig,
}

impl Config {
    pub fn load() -> Self {
        Self::path()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|contents| toml::from_str(&contents).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path().ok_or_else(|| io::Error::other("no config directory available"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let toml = toml::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, toml)
    }

    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "rustlight", "rustlight")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }
}
