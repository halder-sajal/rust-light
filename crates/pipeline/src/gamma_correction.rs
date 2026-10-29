//! Gamma correction: compensates for the LED strip's actual color output
//! not perceptually matching what's on screen (LEDs and monitors rarely
//! share the same response curve).
//!
//! Standard power-law curve, applied per channel on the normalized
//! (0.0..1.0) value: `output = input ^ gamma`. `gamma` < 1.0 brightens
//! midtones, `gamma` > 1.0 darkens them, 1.0 is neutral. Only a few dozen
//! LEDs per frame, so a handful of `powf` calls per frame is negligible —
//! no lookup table needed at this scale.

use crate::{Effect, Rgb};

pub struct GammaCorrection {
    /// > 0.0; 1.0 is neutral.
    pub gamma: f32,
}

impl Effect for GammaCorrection {
    fn apply(&mut self, colors: &mut [Rgb]) {
        let gamma = self.gamma.max(0.01);
        if (gamma - 1.0).abs() < 1e-6 {
            return;
        }

        for c in colors.iter_mut() {
            c.r = apply_gamma(c.r, gamma);
            c.g = apply_gamma(c.g, gamma);
            c.b = apply_gamma(c.b, gamma);
        }
    }
}

fn apply_gamma(v: u8, gamma: f32) -> u8 {
    let normalized = v as f32 / 255.0;
    (normalized.powf(gamma) * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_gamma_is_passthrough() {
        let mut colors = vec![Rgb { r: 10, g: 20, b: 30 }, Rgb { r: 128, g: 128, b: 128 }];
        let original = colors.clone();
        GammaCorrection { gamma: 1.0 }.apply(&mut colors);
        assert_eq!(colors, original);
    }

    #[test]
    fn pure_black_and_white_are_unaffected_by_any_gamma() {
        for gamma in [0.2, 0.5, 1.0, 2.0, 4.0] {
            let mut colors = vec![Rgb { r: 0, g: 0, b: 0 }, Rgb { r: 255, g: 255, b: 255 }];
            GammaCorrection { gamma }.apply(&mut colors);
            assert_eq!(colors[0], Rgb { r: 0, g: 0, b: 0 }, "gamma={gamma}");
            assert_eq!(colors[1], Rgb { r: 255, g: 255, b: 255 }, "gamma={gamma}");
        }
    }

    #[test]
    fn gamma_below_one_brightens_midtones() {
        let mut colors = vec![Rgb { r: 128, g: 128, b: 128 }];
        GammaCorrection { gamma: 0.5 }.apply(&mut colors);
        assert!(colors[0].r > 128);
    }

    #[test]
    fn gamma_above_one_darkens_midtones() {
        let mut colors = vec![Rgb { r: 128, g: 128, b: 128 }];
        GammaCorrection { gamma: 2.0 }.apply(&mut colors);
        assert!(colors[0].r < 128);
    }
}
