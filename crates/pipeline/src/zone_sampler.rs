//! A `Sampler` that averages pixels inside a fixed rectangle per LED.
//!
//! This is the first concrete sampler — zones come from the app's LED-page
//! layout (converted to this crate's plain `Zone` type so this crate stays
//! decoupled from the app's config/serde structs).
//!
//! A plain average over a zone dilutes highlights: if most of a zone is a
//! black letterbox bar and a sliver is vivid content, the LED ends up a
//! dim wash instead of the vivid color actually present. `letterbox_cutoff`
//! fixes that by de-weighting dark pixels in the average — more
//! aggressively near a zone's *outer* edge (facing the screen border,
//! where letterbox bars sit) than its *inner* edge (facing the screen's
//! center, where real content usually is). Bright pixels are never
//! discounted regardless of position, so a single bright pixel in an
//! otherwise-black zone can still dominate at full cutoff strength.

use crate::{Rgb, Sampler};

/// One LED's screen-sampling rectangle, in coordinates relative to the
/// captured frame (0.0..1.0 on each axis).
#[derive(Debug, Clone, Copy)]
pub struct Zone {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// At full `letterbox_cutoff`, a pixel at the outer edge with brightness
/// fraction `f` contributes a weight of `f.powf(MAX_EXPONENT)` — strong
/// enough that anything much short of full brightness is negligible next
/// to a truly bright pixel.
const MAX_EXPONENT: f32 = 10.0;

/// Samples every other pixel on each axis rather than every pixel — a 4x
/// speedup for a preview-grade average that doesn't need per-pixel
/// precision.
const STEP: u32 = 2;

pub struct ZoneSampler {
    pub zones: Vec<Zone>,
    /// 0.0 = plain average (no cutoff). 1.0 = dark pixels near a zone's
    /// outer edge are almost entirely excluded from the average.
    pub letterbox_cutoff: f32,
}

impl Sampler for ZoneSampler {
    fn sample(&self, frame: &capture::Frame, _led_count: usize) -> Vec<Rgb> {
        self.zones.iter().map(|z| sample_zone(frame, z, self.letterbox_cutoff)).collect()
    }
}

/// A zone's longer axis runs along the screen edge it sits on; the shorter
/// axis is its "depth" into the screen. Returns which side of that depth
/// axis is outer (closer to the screen border) vs inner (closer to the
/// screen's center): true = the zone's min-coordinate side is outer.
fn depth_axis_is_vertical_and_outer_is_min(zone: &Zone) -> (bool, bool) {
    let vertical = zone.h >= zone.w; // taller than wide -> left/right-edge-like
    let outer_is_min = if vertical {
        zone.x + zone.w / 2.0 < 0.5
    } else {
        zone.y + zone.h / 2.0 < 0.5
    };
    (vertical, outer_is_min)
}

fn sample_zone(frame: &capture::Frame, zone: &Zone, letterbox_cutoff: f32) -> Rgb {
    let fw = frame.width as f32;
    let fh = frame.height as f32;

    let x0 = ((zone.x * fw).round() as i64).clamp(0, frame.width as i64) as u32;
    let y0 = ((zone.y * fh).round() as i64).clamp(0, frame.height as i64) as u32;
    let x1 = (((zone.x + zone.w) * fw).round() as i64).clamp(x0 as i64, frame.width as i64) as u32;
    let y1 =
        (((zone.y + zone.h) * fh).round() as i64).clamp(y0 as i64, frame.height as i64) as u32;

    if x1 <= x0 || y1 <= y0 {
        return Rgb { r: 0, g: 0, b: 0 };
    }

    let cutoff = letterbox_cutoff.clamp(0.0, 1.0);
    let (depth_is_x, outer_is_min) = depth_axis_is_vertical_and_outer_is_min(zone);
    let span_x = (x1 - x0) as f32;
    let span_y = (y1 - y0) as f32;

    let row_bytes = frame.width as usize * 4;
    let mut r_sum = 0f32;
    let mut g_sum = 0f32;
    let mut b_sum = 0f32;
    let mut weight_sum = 0f32;

    let mut y = y0;
    while y < y1 {
        let row_start = y as usize * row_bytes;
        let rel_y = if span_y > 0.0 { (y - y0) as f32 / span_y } else { 0.0 };
        let mut x = x0;
        while x < x1 {
            let idx = row_start + x as usize * 4;
            if idx + 2 < frame.rgba.len() {
                let r = frame.rgba[idx];
                let g = frame.rgba[idx + 1];
                let b = frame.rgba[idx + 2];

                let weight = if cutoff <= 0.0 {
                    1.0
                } else {
                    let rel_x = if span_x > 0.0 { (x - x0) as f32 / span_x } else { 0.0 };
                    let rel = if depth_is_x { rel_x } else { rel_y };
                    let depth_t = if outer_is_min { rel } else { 1.0 - rel };

                    let brightness = r.max(g).max(b) as f32 / 255.0;
                    let exponent = cutoff * MAX_EXPONENT * (1.0 - depth_t);
                    if exponent <= 0.0 {
                        1.0
                    } else {
                        brightness.powf(exponent)
                    }
                };

                r_sum += r as f32 * weight;
                g_sum += g as f32 * weight;
                b_sum += b as f32 * weight;
                weight_sum += weight;
            }
            x += STEP;
        }
        y += STEP;
    }

    if weight_sum <= 1e-4 {
        return Rgb { r: 0, g: 0, b: 0 };
    }
    Rgb {
        r: (r_sum / weight_sum).round().clamp(0.0, 255.0) as u8,
        g: (g_sum / weight_sum).round().clamp(0.0, 255.0) as u8,
        b: (b_sum / weight_sum).round().clamp(0.0, 255.0) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_frame(width: u32, height: u32, r: u8, g: u8, b: u8) -> capture::Frame {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
        capture::Frame { width, height, rgba }
    }

    #[test]
    fn averages_a_solid_color_zone() {
        let frame = solid_frame(100, 100, 10, 20, 30);
        let sampler = ZoneSampler {
            zones: vec![Zone { x: 0.0, y: 0.0, w: 0.5, h: 0.5 }],
            letterbox_cutoff: 0.0,
        };
        let out = sampler.sample(&frame, 1);
        assert_eq!(out, vec![Rgb { r: 10, g: 20, b: 30 }]);
    }

    #[test]
    fn distinguishes_two_halves_of_a_split_frame() {
        let width = 100;
        let height = 10;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _y in 0..height {
            for x in 0..width {
                if x < width / 2 {
                    rgba.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    rgba.extend_from_slice(&[0, 255, 0, 255]);
                }
            }
        }
        let frame = capture::Frame { width, height, rgba };
        let sampler = ZoneSampler {
            zones: vec![
                Zone { x: 0.0, y: 0.0, w: 0.5, h: 1.0 },
                Zone { x: 0.5, y: 0.0, w: 0.5, h: 1.0 },
            ],
            letterbox_cutoff: 0.0,
        };
        let out = sampler.sample(&frame, 2);
        assert_eq!(out[0], Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(out[1], Rgb { r: 0, g: 255, b: 0 });
    }

    #[test]
    fn empty_or_out_of_bounds_zone_returns_black_without_panicking() {
        let frame = solid_frame(10, 10, 50, 50, 50);
        let sampler = ZoneSampler {
            zones: vec![Zone { x: 0.9, y: 0.9, w: 0.0, h: 0.0 }],
            letterbox_cutoff: 0.0,
        };
        let out = sampler.sample(&frame, 1);
        assert_eq!(out, vec![Rgb { r: 0, g: 0, b: 0 }]);
    }

    /// Frame rows 0..split are `outer_color`, split..height are
    /// `inner_color` — "outer"/"inner" here just names the two halves by
    /// row position; callers pick a zone whose own outer/inner side lines
    /// up with the intended one.
    fn two_band_frame(width: u32, height: u32, split: u32, top: [u8; 3], bottom: [u8; 3]) -> capture::Frame {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let [r, g, b] = if y < split { top } else { bottom };
            for _x in 0..width {
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        capture::Frame { width, height, rgba }
    }

    #[test]
    fn zero_cutoff_is_diluted_by_letterbox_bar() {
        // Top-edge-like zone (center well above y=0): rows 0..4 (its outer
        // half, near the screen's top border) are black, rows 4..8 (its
        // inner half, toward center) are red.
        let frame = two_band_frame(100, 20, 4, [0, 0, 0], [200, 0, 0]);
        let sampler = ZoneSampler {
            zones: vec![Zone { x: 0.0, y: 0.0, w: 1.0, h: 0.4 }], // rows 0..8
            letterbox_cutoff: 0.0,
        };
        let out = sampler.sample(&frame, 1);
        // Plain average of black half + red(200) half ~= 100, diluted.
        assert!(out[0].r < 150, "expected a diluted average, got r={}", out[0].r);
    }

    #[test]
    fn full_cutoff_recovers_the_undiluted_inner_content() {
        let frame = two_band_frame(100, 20, 4, [0, 0, 0], [200, 0, 0]);
        let sampler = ZoneSampler {
            zones: vec![Zone { x: 0.0, y: 0.0, w: 1.0, h: 0.4 }], // center_y=0.2, top-edge-like
            letterbox_cutoff: 1.0,
        };
        let out = sampler.sample(&frame, 1);
        // The outer (rows 0..4) black band should be almost entirely
        // excluded, leaving something close to the inner content's true
        // value (200).
        assert!(out[0].r > 180, "expected near-undiluted content, got r={}", out[0].r);
    }

    #[test]
    fn single_bright_pixel_dominates_at_full_cutoff() {
        // A thin top-edge-like zone (realistic LED-zone shape, not a
        // square) with exactly one pure-white pixel on its outer row and
        // everything else black.
        let width = 10;
        let height = 10;
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let bright_idx = (0 * width as usize + 4) * 4; // outer row (y=0), even x: STEP=2 lands on it
        rgba[bright_idx] = 255;
        rgba[bright_idx + 1] = 255;
        rgba[bright_idx + 2] = 255;
        rgba[bright_idx + 3] = 255;
        let frame = capture::Frame { width, height, rgba };

        let sampler = ZoneSampler {
            zones: vec![Zone { x: 0.0, y: 0.0, w: 1.0, h: 0.3 }], // center_y=0.15, top-edge-like
            letterbox_cutoff: 1.0,
        };
        let out = sampler.sample(&frame, 1);
        assert_eq!(out[0], Rgb { r: 255, g: 255, b: 255 });
    }

    #[test]
    fn bottom_edge_zone_treats_direction_oppositely_from_top_edge() {
        // Bottom-edge-like zone (center well below y=1, i.e. above 0.5):
        // its outer side is the MAX-y side, so put content near the top of
        // this band (closer to center) and the letterbox bar at the
        // bottom (closer to the screen's bottom border).
        let frame = two_band_frame(100, 20, 16, [200, 0, 0], [0, 0, 0]);
        let sampler = ZoneSampler {
            zones: vec![Zone { x: 0.0, y: 0.6, w: 1.0, h: 0.4 }], // rows 12..20, center_y=0.8
            letterbox_cutoff: 1.0,
        };
        let out = sampler.sample(&frame, 1);
        assert!(out[0].r > 180, "expected near-undiluted content, got r={}", out[0].r);
    }
}
