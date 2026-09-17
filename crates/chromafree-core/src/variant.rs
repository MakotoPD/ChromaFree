use std::path::Path;

use crate::frame::FrameSize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelVariant {
    pub width: u32,
    pub height: u32,
}

impl ModelVariant {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn portrait(self) -> Self {
        Self::new(self.height, self.width)
    }

    pub fn aspect_log(self) -> f64 {
        (f64::from(self.width) / f64::from(self.height)).ln()
    }

    pub fn pixels(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn fits_within(self, frame: FrameSize) -> bool {
        self.width <= frame.width() && self.height <= frame.height()
    }

    pub fn rvm_file_name(self) -> String {
        format!("rvm_mobilenetv3_fp16_{}x{}_static.onnx", self.width, self.height)
    }

    pub fn rvm_downsample_ratio(self) -> f32 {
        320.0 / self.width.max(self.height) as f32
    }
}

pub const LANDSCAPE_RVM_VARIANTS: [ModelVariant; 23] = [
    ModelVariant::new(640, 360),
    ModelVariant::new(960, 540),
    ModelVariant::new(1024, 576),
    ModelVariant::new(1280, 720),
    ModelVariant::new(1600, 900),
    ModelVariant::new(1920, 1080),
    ModelVariant::new(2560, 1440),
    ModelVariant::new(640, 400),
    ModelVariant::new(960, 600),
    ModelVariant::new(1280, 800),
    ModelVariant::new(1440, 900),
    ModelVariant::new(1680, 1050),
    ModelVariant::new(1920, 1200),
    ModelVariant::new(2560, 1600),
    ModelVariant::new(640, 480),
    ModelVariant::new(800, 600),
    ModelVariant::new(960, 720),
    ModelVariant::new(1024, 768),
    ModelVariant::new(1280, 960),
    ModelVariant::new(1440, 1080),
    ModelVariant::new(1600, 1200),
    ModelVariant::new(1920, 1440),
    ModelVariant::new(2560, 1920),
];

pub fn all_rvm_variants() -> Vec<ModelVariant> {
    LANDSCAPE_RVM_VARIANTS.iter().flat_map(|v| [*v, v.portrait()]).collect()
}

pub fn available_rvm_variants(models_dir: &Path) -> Vec<ModelVariant> {
    all_rvm_variants()
        .into_iter()
        .filter(|v| models_dir.join(v.rvm_file_name()).is_file())
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VariantPreference {
    #[default]
    Auto,
    Fixed(ModelVariant),
}

const ASPECT_TOLERANCE: f64 = 1e-3;

pub fn select_variant(
    frame: FrameSize,
    available: &[ModelVariant],
    preference: VariantPreference,
) -> Option<ModelVariant> {
    if let VariantPreference::Fixed(wanted) = preference
        && available.contains(&wanted)
    {
        return Some(wanted);
    }
    let frame_aspect = (f64::from(frame.width()) / f64::from(frame.height())).ln();
    let best_distance = available
        .iter()
        .map(|v| (v.aspect_log() - frame_aspect).abs())
        .min_by(f64::total_cmp)?;
    let same_aspect = available
        .iter()
        .copied()
        .filter(|v| (v.aspect_log() - frame_aspect).abs() <= best_distance + ASPECT_TOLERANCE);
    let (fitting, larger): (Vec<_>, Vec<_>) = same_aspect.partition(|v| v.fits_within(frame));
    fitting
        .into_iter()
        .max_by_key(|v| v.pixels())
        .or_else(|| larger.into_iter().min_by_key(|v| v.pixels()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u32, height: u32) -> FrameSize {
        FrameSize::new(width, height).unwrap()
    }

    #[test]
    fn auto_picks_largest_matching_aspect_that_fits_the_frame() {
        let all = all_rvm_variants();
        let pick = |w, h| select_variant(size(w, h), &all, VariantPreference::Auto).unwrap();
        assert_eq!(pick(1920, 1080), ModelVariant::new(1920, 1080));
        assert_eq!(pick(1280, 720), ModelVariant::new(1280, 720));
        assert_eq!(pick(3840, 2160), ModelVariant::new(2560, 1440));
        assert_eq!(pick(1920, 1200), ModelVariant::new(1920, 1200));
        assert_eq!(pick(1440, 900), ModelVariant::new(1440, 900));
        assert_eq!(pick(1280, 960), ModelVariant::new(1280, 960));
        assert_eq!(pick(1024, 768), ModelVariant::new(1024, 768));
        assert_eq!(pick(1080, 1920), ModelVariant::new(1080, 1920));
    }

    #[test]
    fn auto_uses_smallest_matching_variant_when_frame_is_smaller_than_all() {
        let all = all_rvm_variants();
        assert_eq!(
            select_variant(size(320, 180), &all, VariantPreference::Auto),
            Some(ModelVariant::new(640, 360))
        );
    }

    #[test]
    fn auto_falls_back_to_nearest_aspect_ratio() {
        let all = all_rvm_variants();
        assert_eq!(
            select_variant(size(1366, 768), &all, VariantPreference::Auto),
            Some(ModelVariant::new(1280, 720))
        );
    }

    #[test]
    fn fixed_preference_is_honoured_when_available_regardless_of_frame() {
        let all = all_rvm_variants();
        let wanted = ModelVariant::new(2560, 1600);
        assert_eq!(
            select_variant(size(640, 480), &all, VariantPreference::Fixed(wanted)),
            Some(wanted)
        );
        let missing = ModelVariant::new(123, 456);
        assert_eq!(
            select_variant(size(1280, 720), &all, VariantPreference::Fixed(missing)),
            Some(ModelVariant::new(1280, 720))
        );
    }

    #[test]
    fn empty_catalogue_selects_nothing() {
        assert_eq!(select_variant(size(1280, 720), &[], VariantPreference::Auto), None);
    }

    #[test]
    fn downsample_ratio_keeps_the_long_side_at_320() {
        assert_eq!(ModelVariant::new(1280, 720).rvm_downsample_ratio(), 0.25);
        assert_eq!(ModelVariant::new(360, 640).rvm_downsample_ratio(), 0.5);
    }
}
