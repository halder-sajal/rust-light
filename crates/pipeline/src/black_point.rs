//! Black-point adjustment: one bidirectional control.
//!
//! - `amount` < 0 crushes dark colors to pure black and stretches what's
//!   left back out to the full range — a near-black screen reads as
//!   "off" instead of a faint, noisy glow, the more negative the harder.
//! - `amount` > 0 lifts/brightens everything, including blacks — an
//!   "overbrighten" effect, the more positive the stronger.
//! - `amount` == 0 is neutral (passthrough).
//!
//! Both directions are the same linear remap around a movable black
//! point; only its sign flips between crushing and lifting. O(n) over
//! plain arithmetic, no allocation beyond the input slice being mutated
//! in place.

use crate::{Effect, Rgb};

/// How far the remap can push the black point, in 0..255 units. Clamped
/// below 255 (and above -255) so the remap's denominator never reaches 0.
const MAX_SHIFT: f32 = 250.0;

pub struct BlackPoint {
    /// -1.0 (max crush) .. 0.0 (neutral) .. 1.0 (max lift).
    pub amount: f32,
}

impl Effect for BlackPoint {
    fn apply(&mut self, colors: &mut [Rgb]) {
        let amount = self.amount.clamp(-1.0, 1.0);
        if amount == 0.0 {
            return;
        }

        // amount<0 (crush) -> positive threshold; amount>0 (lift) -> negative.
        let threshold = -amount * MAX_SHIFT;
        let range = 255.0 - threshold;

        for c in colors.iter_mut() {
            c.r = remap(c.r, threshold, range);
            c.g = remap(c.g, threshold, range);
            c.b = remap(c.b, threshold, range);
        }
    }
}

fn remap(v: u8, threshold: f32, range: f32) -> u8 {
    (((v as f32 - threshold) / range) * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_passthrough() {
        let mut colors = vec![Rgb { r: 10, g: 20, b: 30 }, Rgb { r: 0, g: 0, b: 0 }];
        let original = colors.clone();
        BlackPoint { amount: 0.0 }.apply(&mut colors);
        assert_eq!(colors, original);
    }

    #[test]
    fn negative_amount_crushes_near_black_to_zero() {
        let mut colors = vec![Rgb { r: 5, g: 5, b: 5 }];
        BlackPoint { amount: -0.5 }.apply(&mut colors);
        assert_eq!(colors[0], Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn negative_amount_crushes_dim_midtones_too() {
        let mut colors = vec![Rgb { r: 20, g: 20, b: 20 }];
        BlackPoint { amount: -0.6 }.apply(&mut colors); // threshold = 150
        assert_eq!(colors[0], Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn crushing_still_leaves_bright_colors_visible() {
        let mut colors = vec![Rgb { r: 255, g: 255, b: 255 }];
        BlackPoint { amount: -0.5 }.apply(&mut colors);
        assert_eq!(colors[0], Rgb { r: 255, g: 255, b: 255 });
    }

    #[test]
    fn positive_amount_lifts_pure_black_off_the_floor() {
        let mut colors = vec![Rgb { r: 0, g: 0, b: 0 }];
        BlackPoint { amount: 0.5 }.apply(&mut colors);
        assert!(colors[0].r > 0, "lifting should brighten pure black, got {}", colors[0].r);
    }

    #[test]
    fn positive_amount_keeps_white_at_white() {
        let mut colors = vec![Rgb { r: 255, g: 255, b: 255 }];
        BlackPoint { amount: 1.0 }.apply(&mut colors);
        assert_eq!(colors[0], Rgb { r: 255, g: 255, b: 255 });
    }

    #[test]
    fn stronger_lift_brightens_black_more() {
        let mut weak = vec![Rgb { r: 0, g: 0, b: 0 }];
        BlackPoint { amount: 0.2 }.apply(&mut weak);
        let mut strong = vec![Rgb { r: 0, g: 0, b: 0 }];
        BlackPoint { amount: 0.9 }.apply(&mut strong);
        assert!(strong[0].r > weak[0].r);
    }
}
