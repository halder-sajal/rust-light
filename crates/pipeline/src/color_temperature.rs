//! Color temperature (warm/cool) adjustment: shifts the red/blue balance
//! to compensate for the LED strip's white point not matching the screen's.
//!
//! Green is left alone — a standard simplification for a single warmth
//! slider (full blackbody-curve correction isn't needed here). O(n) over
//! plain arithmetic.

use crate::{Effect, Rgb};

/// At the extreme (amount = ±1.0), red or blue is scaled by up to this
/// fraction.
const MAX_SHIFT: f32 = 0.6;

pub struct ColorTemperature {
    /// -1.0 (max cool/blue) .. 0.0 (neutral) .. 1.0 (max warm/orange).
    pub amount: f32,
}

impl Effect for ColorTemperature {
    fn apply(&mut self, colors: &mut [Rgb]) {
        let amount = self.amount.clamp(-1.0, 1.0);
        if amount == 0.0 {
            return;
        }

        let shift = amount * MAX_SHIFT;
        for c in colors.iter_mut() {
            c.r = scale_channel(c.r, 1.0 + shift);
            c.b = scale_channel(c.b, 1.0 - shift);
        }
    }
}

fn scale_channel(v: u8, factor: f32) -> u8 {
    (v as f32 * factor).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_passthrough() {
        let mut colors = vec![Rgb { r: 100, g: 150, b: 200 }];
        let original = colors.clone();
        ColorTemperature { amount: 0.0 }.apply(&mut colors);
        assert_eq!(colors, original);
    }

    #[test]
    fn warm_boosts_red_and_reduces_blue() {
        let mut colors = vec![Rgb { r: 100, g: 150, b: 100 }];
        ColorTemperature { amount: 1.0 }.apply(&mut colors);
        assert!(colors[0].r > 100);
        assert!(colors[0].b < 100);
        assert_eq!(colors[0].g, 150, "green should be untouched");
    }

    #[test]
    fn cool_boosts_blue_and_reduces_red() {
        let mut colors = vec![Rgb { r: 100, g: 150, b: 100 }];
        ColorTemperature { amount: -1.0 }.apply(&mut colors);
        assert!(colors[0].r < 100);
        assert!(colors[0].b > 100);
        assert_eq!(colors[0].g, 150, "green should be untouched");
    }

    #[test]
    fn channels_stay_in_valid_u8_range_at_extremes() {
        let mut colors = vec![Rgb { r: 255, g: 255, b: 255 }, Rgb { r: 0, g: 0, b: 0 }];
        ColorTemperature { amount: 1.0 }.apply(&mut colors);
        // Just needs to not panic; clamping already guarantees range via u8.
        let mut colors2 = vec![Rgb { r: 255, g: 255, b: 255 }, Rgb { r: 0, g: 0, b: 0 }];
        ColorTemperature { amount: -1.0 }.apply(&mut colors2);
    }
}
