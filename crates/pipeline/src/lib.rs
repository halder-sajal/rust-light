//! Capture → color pipeline: pure logic, no network or OS I/O.
//!
//! Stages are trait boundaries only for now — no sampling grid, smoothing
//! algorithm, or effect has been decided yet. This crate just fixes how
//! they compose so each can be built and swapped independently later.

pub use wled_proto::Rgb;

mod black_point;
mod color_temperature;
mod gamma_correction;
mod spatial_smoother;
mod zone_sampler;
pub use black_point::BlackPoint;
pub use color_temperature::ColorTemperature;
pub use gamma_correction::GammaCorrection;
pub use spatial_smoother::SpatialSmoother;
pub use zone_sampler::{Zone, ZoneSampler};

/// Turns a captured frame into one color per LED. Sampling strategy
/// (zones, edge-weighting, downscaling, ...) is not decided yet.
pub trait Sampler {
    fn sample(&self, frame: &capture::Frame, led_count: usize) -> Vec<Rgb>;
}

/// Transforms one frame's full set of LED colors into another — along the
/// strip (blending neighbors, e.g. `SpatialSmoother`), across frames (e.g.
/// an EMA to cut flicker), or both. The signature doesn't care which axis
/// an implementation smooths over.
pub trait Smoother {
    fn smooth(&mut self, target: &[Rgb]) -> Vec<Rgb>;
}

/// A post-processing lighting effect applied after sampling/smoothing
/// (e.g. gamma correction, saturation boost, color-cycle overlays).
pub trait Effect {
    fn apply(&mut self, colors: &mut [Rgb]);
}

/// Identity smoother — passes colors through unchanged. Useful as a
/// default/no-op while a real smoothing algorithm isn't chosen yet.
#[derive(Default)]
pub struct NoOpSmoother;

impl Smoother for NoOpSmoother {
    fn smooth(&mut self, target: &[Rgb]) -> Vec<Rgb> {
        target.to_vec()
    }
}

/// Wires a sampler, a smoother, and a chain of effects into one
/// frame-in / LED-colors-out step.
pub struct Pipeline<S: Sampler, M: Smoother> {
    sampler: S,
    smoother: M,
    effects: Vec<Box<dyn Effect>>,
    led_count: usize,
}

impl<S: Sampler, M: Smoother> Pipeline<S, M> {
    pub fn new(sampler: S, smoother: M, led_count: usize) -> Self {
        Self {
            sampler,
            smoother,
            effects: Vec::new(),
            led_count,
        }
    }

    pub fn add_effect(&mut self, effect: Box<dyn Effect>) {
        self.effects.push(effect);
    }

    pub fn process(&mut self, frame: &capture::Frame) -> Vec<Rgb> {
        let sampled = self.sampler.sample(frame, self.led_count);
        let mut colors = self.smoother.smooth(&sampled);
        for effect in &mut self.effects {
            effect.apply(&mut colors);
        }
        colors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConstSampler(Rgb);
    impl Sampler for ConstSampler {
        fn sample(&self, _frame: &capture::Frame, led_count: usize) -> Vec<Rgb> {
            vec![self.0; led_count]
        }
    }

    struct Invert;
    impl Effect for Invert {
        fn apply(&mut self, colors: &mut [Rgb]) {
            for c in colors {
                c.r = 255 - c.r;
                c.g = 255 - c.g;
                c.b = 255 - c.b;
            }
        }
    }

    #[test]
    fn pipeline_runs_sampler_smoother_and_effects_in_order() {
        let sampler = ConstSampler(Rgb { r: 10, g: 20, b: 30 });
        let mut pipeline = Pipeline::new(sampler, NoOpSmoother, 3);
        pipeline.add_effect(Box::new(Invert));

        let frame = capture::Frame {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        };
        let out = pipeline.process(&frame);

        assert_eq!(out.len(), 3);
        assert_eq!(out[0], Rgb { r: 245, g: 235, b: 225 });
    }
}
