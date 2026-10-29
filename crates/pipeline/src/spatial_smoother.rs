//! Blends each LED's color with its neighbors along the strip, so adjacent
//! LEDs transition gradually instead of flipping abruptly.
//!
//! This is spatial (along the strip), not temporal (across frames) — it
//! treats the LED order as a closed loop (the perimeter wraps around).
//! `amount` scales both the blend weight *and* how many passes run, so
//! turning it up actually widens how far a color's influence spreads
//! (0% = untouched, 100% = thoroughly blurred across many neighbors) —
//! not just a fixed-radius 3-tap blend at varying strength. Still O(n) per
//! pass, a handful of passes at most, plain arithmetic — cheap enough for
//! a real-time send loop.

use crate::{Rgb, Smoother};

/// Running more passes at high `amount` is what makes "full smoothing"
/// actually reach beyond a LED's immediate neighbors.
const MAX_PASSES: u32 = 8;

pub struct SpatialSmoother {
    /// 0.0 = no smoothing (passthrough), 1.0 = heavily blurred across
    /// several neighbors in each direction.
    pub amount: f32,
}

impl Smoother for SpatialSmoother {
    fn smooth(&mut self, target: &[Rgb]) -> Vec<Rgb> {
        let n = target.len();
        let amount = self.amount.clamp(0.0, 1.0);
        if n < 3 || amount <= 0.0 {
            return target.to_vec();
        }

        let passes = 1 + (amount * (MAX_PASSES - 1) as f32).round() as u32;
        let mut current = target.to_vec();
        for _ in 0..passes {
            current = one_pass(&current, amount);
        }
        current
    }
}

fn one_pass(colors: &[Rgb], amount: f32) -> Vec<Rgb> {
    let n = colors.len();
    (0..n)
        .map(|i| {
            let prev = colors[(i + n - 1) % n];
            let next = colors[(i + 1) % n];
            let cur = colors[i];
            Rgb {
                r: blend(cur.r, prev.r, next.r, amount),
                g: blend(cur.g, prev.g, next.g, amount),
                b: blend(cur.b, prev.b, next.b, amount),
            }
        })
        .collect()
}

fn blend(current: u8, prev: u8, next: u8, amount: f32) -> u8 {
    let neighbor_avg = (prev as f32 + next as f32) / 2.0;
    (current as f32 * (1.0 - amount) + neighbor_avg * amount).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_passthrough() {
        let colors =
            vec![Rgb { r: 10, g: 20, b: 30 }, Rgb { r: 200, g: 0, b: 0 }, Rgb { r: 0, g: 0, b: 0 }];
        let mut s = SpatialSmoother { amount: 0.0 };
        assert_eq!(s.smooth(&colors), colors);
    }

    #[test]
    fn one_pass_at_full_weight_becomes_exact_neighbor_average() {
        // `one_pass` is the single-iteration building block; verify it in
        // isolation before relying on multi-pass behavior built from it.
        let colors = vec![
            Rgb { r: 0, g: 0, b: 0 },
            Rgb { r: 100, g: 100, b: 100 },
            Rgb { r: 200, g: 200, b: 200 },
        ];
        let out = one_pass(&colors, 1.0);
        assert_eq!(out[1], Rgb { r: 100, g: 100, b: 100 });
    }

    #[test]
    fn wraps_around_the_loop_at_the_ends() {
        let colors = vec![
            Rgb { r: 255, g: 0, b: 0 },
            Rgb { r: 0, g: 0, b: 0 },
            Rgb { r: 0, g: 0, b: 0 },
            Rgb { r: 0, g: 0, b: 0 },
        ];
        // index 0's neighbors are index 3 (wrapping) and index 1, both 0.
        let out = one_pass(&colors, 1.0);
        assert_eq!(out[0], Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn higher_amount_spreads_influence_further_than_immediate_neighbors() {
        // A single bright spike surrounded by black, on a long strip.
        let mut colors = vec![Rgb { r: 0, g: 0, b: 0 }; 21];
        colors[10] = Rgb { r: 255, g: 0, b: 0 };

        // amount=0.01 -> 1 pass, which can't reach two LEDs away.
        let mut low = SpatialSmoother { amount: 0.01 };
        let low_out = low.smooth(&colors);
        // amount=1.0 -> MAX_PASSES passes, which comfortably reaches it.
        let mut high = SpatialSmoother { amount: 1.0 };
        let high_out = high.smooth(&colors);

        assert_eq!(low_out[8].r, 0, "a single pass shouldn't reach two LEDs away");
        assert!(high_out[8].r > 0, "several passes should spread influence past immediate neighbors");
    }

    #[test]
    fn output_channels_never_leave_valid_u8_range() {
        let colors = vec![Rgb { r: 255, g: 255, b: 255 }, Rgb { r: 0, g: 0, b: 0 }, Rgb { r: 128, g: 64, b: 200 }];
        let mut s = SpatialSmoother { amount: 1.0 };
        // Just needs to not panic and stay in range; u8 return type already
        // guarantees the range, this exercises every pass without crashing.
        let _ = s.smooth(&colors);
    }
}
