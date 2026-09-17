use crate::error::CoreError;
use crate::frame::FrameSize;
use crate::scale::{PlaneScaler, ScaleMode};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskParams {
    pub temporal_smoothing: f32,
    pub edge_low: f32,
    pub edge_high: f32,
}

impl MaskParams {
    pub const RECURRENT_MODEL: MaskParams = MaskParams {
        temporal_smoothing: 0.0,
        edge_low: 0.0,
        edge_high: 1.0,
    };

    pub const MEMORYLESS_MODEL: MaskParams = MaskParams {
        temporal_smoothing: 0.6,
        edge_low: 0.3,
        edge_high: 0.7,
    };
}

impl Default for MaskParams {
    fn default() -> Self {
        Self::RECURRENT_MODEL
    }
}

pub struct RefinedMask<'a> {
    pub luma: &'a [u8],
    pub chroma: &'a [u8],
}

pub struct MaskRefiner {
    model_width: u32,
    model_height: u32,
    output: FrameSize,
    smoothing_weight: u32,
    edge_lookup: [u8; 256],
    history: Vec<u16>,
    has_history: bool,
    shaped: Vec<u8>,
    luma_scaler: PlaneScaler,
    chroma_scaler: PlaneScaler,
    luma: Vec<u8>,
    chroma: Vec<u8>,
}

fn edge_lookup(low: f32, high: f32) -> [u8; 256] {
    let low = low.clamp(0.0, 1.0);
    let high = high.clamp(low + f32::EPSILON, 1.0);
    std::array::from_fn(|i| {
        let t = ((i as f32 / 255.0 - low) / (high - low)).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        (smooth * 255.0).round() as u8
    })
}

impl MaskRefiner {
    pub fn new(model_width: u32, model_height: u32, output: FrameSize, params: MaskParams) -> Self {
        let model_pixels = model_width as usize * model_height as usize;
        let mut refiner = Self {
            model_width,
            model_height,
            output,
            smoothing_weight: 0,
            edge_lookup: [0; 256],
            history: vec![0; model_pixels],
            has_history: false,
            shaped: vec![0; model_pixels],
            luma_scaler: PlaneScaler::new(
                model_width,
                model_height,
                output.width(),
                output.height(),
                1,
                ScaleMode::Stretch,
            ),
            chroma_scaler: PlaneScaler::new(
                model_width,
                model_height,
                output.chroma_width(),
                output.chroma_height(),
                1,
                ScaleMode::Stretch,
            ),
            luma: vec![0; output.luma_len()],
            chroma: vec![0; output.chroma_len() / 2],
        };
        refiner.set_params(params);
        refiner
    }

    pub fn output(&self) -> FrameSize {
        self.output
    }

    pub fn model_dimensions(&self) -> (u32, u32) {
        (self.model_width, self.model_height)
    }

    pub fn set_params(&mut self, params: MaskParams) {
        self.smoothing_weight = (params.temporal_smoothing.clamp(0.0, 0.98) * 256.0).round() as u32;
        self.edge_lookup = edge_lookup(params.edge_low, params.edge_high);
    }

    pub fn reset(&mut self) {
        self.has_history = false;
    }

    pub fn refine(&mut self, alpha: &[u8]) -> Result<RefinedMask<'_>, CoreError> {
        if alpha.len() != self.history.len() {
            return Err(CoreError::PlaneSizeMismatch {
                expected: self.history.len(),
                actual: alpha.len(),
            });
        }
        let keep = self.smoothing_weight;
        let take = 256 - keep;
        let blend_history = self.has_history && keep > 0;
        for ((value, history), shaped) in alpha.iter().zip(self.history.iter_mut()).zip(self.shaped.iter_mut()) {
            let current = u32::from(*value) << 8;
            let smoothed = if blend_history {
                (u32::from(*history) * keep + current * take) >> 8
            } else {
                current
            };
            *history = smoothed as u16;
            *shaped = self.edge_lookup[((smoothed + 128) >> 8).min(255) as usize];
        }
        self.has_history = true;
        self.luma_scaler.scale(&self.shaped, &mut self.luma)?;
        self.chroma_scaler.scale(&self.shaped, &mut self.chroma)?;
        Ok(RefinedMask {
            luma: &self.luma,
            chroma: &self.chroma,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> FrameSize {
        FrameSize::new(8, 4).unwrap()
    }

    #[test]
    fn identity_params_quantise_and_upscale() {
        let mut refiner = MaskRefiner::new(4, 2, output(), MaskParams::RECURRENT_MODEL);
        let alpha = [255u8; 8];
        let mask = refiner.refine(&alpha).unwrap();
        assert!(mask.luma.iter().all(|&v| v == 255));
        assert_eq!(mask.luma.len(), 32);
        assert_eq!(mask.chroma.len(), 8);
        assert!(mask.chroma.iter().all(|&v| v == 255));
    }

    #[test]
    fn edge_shaping_hardens_values_outside_thresholds() {
        let params = MaskParams {
            temporal_smoothing: 0.0,
            edge_low: 0.3,
            edge_high: 0.7,
        };
        let mut refiner = MaskRefiner::new(4, 1, FrameSize::new(4, 2).unwrap(), params);
        let mask = refiner.refine(&[51u8, 128, 204, 74]).unwrap();
        assert_eq!(mask.luma[0], 0);
        assert_eq!(mask.luma[2], 255);
        assert!((120..=135).contains(&mask.luma[1]), "{}", mask.luma[1]);
    }

    #[test]
    fn temporal_smoothing_moves_gradually_and_reset_forgets_history() {
        let params = MaskParams {
            temporal_smoothing: 0.5,
            edge_low: 0.0,
            edge_high: 1.0,
        };
        let mut refiner = MaskRefiner::new(2, 2, FrameSize::new(2, 2).unwrap(), params);
        refiner.refine(&[0u8; 4]).unwrap();
        let second = refiner.refine(&[255u8; 4]).unwrap().luma[0];
        assert!((120..=135).contains(&second), "{second}");
        let third = refiner.refine(&[255u8; 4]).unwrap().luma[0];
        assert!(third > second);
        refiner.reset();
        assert_eq!(refiner.refine(&[0u8; 4]).unwrap().luma[0], 0);
    }

    #[test]
    fn wrong_alpha_length_is_rejected() {
        let mut refiner = MaskRefiner::new(4, 2, output(), MaskParams::default());
        assert!(refiner.refine(&[0u8; 3]).is_err());
    }
}

pub fn quantize_unit_f32(values: &[f32], output: &mut [u8]) {
    for (out, &value) in output.iter_mut().zip(values) {
        *out = (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
}

pub fn quantize_unit_f16(values: &[half::f16], output: &mut [u8]) {
    let lookup = f16_lookup();
    for (out, value) in output.iter_mut().zip(values) {
        *out = lookup[usize::from(value.to_bits())];
    }
}

fn f16_lookup() -> &'static [u8; 65536] {
    static LOOKUP: std::sync::OnceLock<Box<[u8; 65536]>> = std::sync::OnceLock::new();
    LOOKUP.get_or_init(|| {
        let mut table = Box::new([0u8; 65536]);
        for (bits, entry) in table.iter_mut().enumerate() {
            let value = half::f16::from_bits(bits as u16).to_f32();
            *entry = if value.is_nan() {
                0
            } else {
                (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            };
        }
        table
    })
}

#[cfg(test)]
mod quantize_tests {
    use super::*;

    #[test]
    fn f16_and_f32_quantisation_agree() {
        let values: Vec<f32> = (-20..=280).map(|i| i as f32 / 255.0).collect();
        let halves: Vec<half::f16> = values.iter().map(|&v| half::f16::from_f32(v)).collect();
        let mut a = vec![0; values.len()];
        let mut b = vec![0; values.len()];
        quantize_unit_f32(&values, &mut a);
        quantize_unit_f16(&halves, &mut b);
        for ((x, y), v) in a.iter().zip(&b).zip(&values) {
            assert!(x.abs_diff(*y) <= 1, "{v}: {x} vs {y}");
        }
        assert_eq!(a[0], 0);
        assert_eq!(*a.last().unwrap(), 255);
    }
}
