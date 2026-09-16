use std::path::PathBuf;
use std::sync::Arc;

use crate::background::RgbImage;
use crate::blur::BackgroundBlur;
use crate::color::{ColorMatrix, Rgb};
use crate::composite::{Background, blend_nv12, nv12_to_bgra};
use crate::error::CoreError;
use crate::frame::{BgraFrame, FrameSize, Nv12Frame};
use crate::mask::{MaskParams, MaskRefiner};
use crate::orientation::Orientation;
use crate::scale::{Nv12Scaler, ScaleMode};
use crate::segmentation::{InferenceDevice, MediaPipeModel, MediaPipeVariant, RvmModel, SegmentationModel};
use crate::variant::{ModelVariant, VariantPreference, available_rvm_variants, select_variant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SegmentationMethod {
    #[default]
    Rvm,
    MediaPipe,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SegmentationSettings {
    pub method: SegmentationMethod,
    pub variant: VariantPreference,
    pub device: InferenceDevice,
    pub mask: Option<MaskParams>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundEffect {
    Passthrough,
    Blur { strength: f32 },
    Color(Rgb),
    Image(Arc<RgbImage>),
    Transparent { fallback: Rgb },
}

impl Default for BackgroundEffect {
    fn default() -> Self {
        Self::Blur { strength: 0.5 }
    }
}

impl BackgroundEffect {
    pub fn needs_mask(&self) -> bool {
        !matches!(self, Self::Passthrough)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PipelineSettings {
    pub output: FrameSize,
    pub orientation: Orientation,
    pub effect: BackgroundEffect,
    pub segmentation: SegmentationSettings,
    pub color_matrix: Option<ColorMatrix>,
}

impl PipelineSettings {
    pub fn new(output: FrameSize) -> Self {
        Self {
            output,
            orientation: Orientation::default(),
            effect: BackgroundEffect::default(),
            segmentation: SegmentationSettings::default(),
            color_matrix: None,
        }
    }

    pub fn matrix(&self) -> ColorMatrix {
        self.color_matrix
            .unwrap_or_else(|| ColorMatrix::for_height(self.output.height().min(self.output.width())))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    Nv12,
    Bgra,
}

pub enum PipelineOutput<'a> {
    Nv12(&'a Nv12Frame),
    Bgra(&'a BgraFrame),
}

pub trait ModelProvider: Send {
    fn load(&self, settings: &SegmentationSettings, frame: FrameSize) -> Result<Box<dyn SegmentationModel>, CoreError>;
}

pub struct OnnxModelProvider {
    models_dir: PathBuf,
}

impl OnnxModelProvider {
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self {
            models_dir: models_dir.into(),
        }
    }

    pub fn resolve_rvm_variant(
        &self,
        preference: VariantPreference,
        frame: FrameSize,
    ) -> Result<ModelVariant, CoreError> {
        select_variant(frame, &available_rvm_variants(&self.models_dir), preference)
            .ok_or_else(|| CoreError::ModelNotFound(format!("no RVM variants in {}", self.models_dir.display())))
    }
}

impl ModelProvider for OnnxModelProvider {
    fn load(&self, settings: &SegmentationSettings, frame: FrameSize) -> Result<Box<dyn SegmentationModel>, CoreError> {
        match settings.method {
            SegmentationMethod::Rvm => {
                let variant = self.resolve_rvm_variant(settings.variant, frame)?;
                Ok(Box::new(RvmModel::load(&self.models_dir, variant, settings.device)?))
            }
            SegmentationMethod::MediaPipe => Ok(Box::new(MediaPipeModel::load(
                &self.models_dir,
                MediaPipeVariant::for_frame(frame),
                settings.device,
            )?)),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ModelKey {
    method: SegmentationMethod,
    variant: VariantPreference,
    device: InferenceDevice,
    frame: FrameSize,
}

impl ModelKey {
    fn new(settings: &SegmentationSettings, frame: FrameSize) -> Self {
        Self {
            method: settings.method,
            variant: settings.variant,
            device: settings.device,
            frame,
        }
    }
}

struct Segmenter {
    key: ModelKey,
    model: Box<dyn SegmentationModel>,
    refiner: MaskRefiner,
}

struct ImageBackground {
    source: Arc<RgbImage>,
    size: FrameSize,
    matrix: ColorMatrix,
    frame: Nv12Frame,
}

pub struct CpuPipeline {
    provider: Box<dyn ModelProvider>,
    settings: PipelineSettings,
    input_size: Option<FrameSize>,
    oriented: Option<Nv12Frame>,
    scaler: Option<Nv12Scaler>,
    frame: Nv12Frame,
    composed: Nv12Frame,
    bgra: BgraFrame,
    blurred: Nv12Frame,
    blur: Option<BackgroundBlur>,
    segmenter: Option<Segmenter>,
    image: Option<ImageBackground>,
}

impl CpuPipeline {
    pub fn new(provider: Box<dyn ModelProvider>, settings: PipelineSettings) -> Self {
        let output = settings.output;
        Self {
            provider,
            settings,
            input_size: None,
            oriented: None,
            scaler: None,
            frame: Nv12Frame::new(output),
            composed: Nv12Frame::new(output),
            bgra: BgraFrame::new(output),
            blurred: Nv12Frame::new(output),
            blur: None,
            segmenter: None,
            image: None,
        }
    }

    pub fn settings(&self) -> &PipelineSettings {
        &self.settings
    }

    pub fn active_model(&self) -> Option<&str> {
        self.segmenter.as_ref().map(|s| s.model.name())
    }

    pub fn apply_settings(&mut self, settings: PipelineSettings) {
        if settings.output != self.settings.output {
            self.frame = Nv12Frame::new(settings.output);
            self.composed = Nv12Frame::new(settings.output);
            self.bgra = BgraFrame::new(settings.output);
            self.blurred = Nv12Frame::new(settings.output);
            self.scaler = None;
            self.blur = None;
            self.segmenter = None;
            self.image = None;
        }
        if settings.orientation != self.settings.orientation {
            self.input_size = None;
        }
        if let Some(segmenter) = &mut self.segmenter {
            if segmenter.key != ModelKey::new(&settings.segmentation, settings.output) {
                self.segmenter = None;
            } else if settings.segmentation.mask != self.settings.segmentation.mask {
                let params = settings
                    .segmentation
                    .mask
                    .unwrap_or_else(|| segmenter.model.default_mask_params());
                segmenter.refiner.set_params(params);
            }
        }
        if !settings.effect.needs_mask() {
            self.segmenter = None;
        }
        self.settings = settings;
    }

    pub fn reset_temporal_state(&mut self) {
        if let Some(segmenter) = &mut self.segmenter {
            segmenter.model.reset();
            segmenter.refiner.reset();
        }
    }

    pub fn process(&mut self, input: &Nv12Frame, format: OutputFormat) -> Result<PipelineOutput<'_>, CoreError> {
        self.prepare_frame(input)?;
        let matrix = self.settings.matrix();
        let effect = self.settings.effect.clone();
        if !effect.needs_mask() {
            return self.finish(format, matrix, true);
        }
        self.ensure_segmenter()?;
        let Some(segmenter) = self.segmenter.as_mut() else {
            return Err(CoreError::Inference("segmenter unavailable".into()));
        };
        let alpha = segmenter.model.segment(&self.frame, matrix)?;
        let mask = segmenter.refiner.refine(alpha.data)?;
        match &effect {
            BackgroundEffect::Passthrough => {}
            BackgroundEffect::Color(rgb) => {
                blend_nv12(
                    &self.frame,
                    Background::Solid(matrix.to_yuv(*rgb)),
                    mask.luma,
                    mask.chroma,
                    &mut self.composed,
                )?;
            }
            BackgroundEffect::Transparent { fallback } => {
                if format == OutputFormat::Bgra {
                    nv12_to_bgra(&self.frame, Some(mask.luma), matrix, &mut self.bgra)?;
                    return Ok(PipelineOutput::Bgra(&self.bgra));
                }
                blend_nv12(
                    &self.frame,
                    Background::Solid(matrix.to_yuv(*fallback)),
                    mask.luma,
                    mask.chroma,
                    &mut self.composed,
                )?;
            }
            BackgroundEffect::Blur { strength } => {
                let output = self.settings.output;
                let blur = match &mut self.blur {
                    Some(blur) if blur.strength() == strength.clamp(0.0, 1.0) => blur,
                    slot => slot.insert(BackgroundBlur::new(output, *strength)),
                };
                blur.apply(&self.frame, &mut self.blurred)?;
                blend_nv12(
                    &self.frame,
                    Background::Frame(&self.blurred),
                    mask.luma,
                    mask.chroma,
                    &mut self.composed,
                )?;
            }
            BackgroundEffect::Image(source) => {
                let output = self.settings.output;
                let stale = self.image.as_ref().is_none_or(|image| {
                    !Arc::ptr_eq(&image.source, source) || image.size != output || image.matrix != matrix
                });
                if stale {
                    self.image = Some(ImageBackground {
                        source: Arc::clone(source),
                        size: output,
                        matrix,
                        frame: source.to_nv12(output, matrix)?,
                    });
                }
                let background = self
                    .image
                    .as_ref()
                    .map(|image| &image.frame)
                    .ok_or_else(|| CoreError::Image("background image unavailable".into()))?;
                blend_nv12(
                    &self.frame,
                    Background::Frame(background),
                    mask.luma,
                    mask.chroma,
                    &mut self.composed,
                )?;
            }
        }
        self.finish(format, matrix, false)
    }

    fn prepare_frame(&mut self, input: &Nv12Frame) -> Result<(), CoreError> {
        let orientation = self.settings.orientation;
        let oriented_size = orientation.output_size(input.size());
        if self.input_size != Some(input.size()) {
            self.input_size = Some(input.size());
            self.oriented = (!orientation.is_identity()).then(|| Nv12Frame::new(oriented_size));
            self.scaler = None;
            self.reset_temporal_state();
        }
        let source = match &mut self.oriented {
            Some(oriented) => {
                orientation.apply(input, oriented)?;
                &*oriented
            }
            None => input,
        };
        let output = self.settings.output;
        let scaler = match &mut self.scaler {
            Some(scaler) if scaler.source() == source.size() && scaler.target() == output => scaler,
            slot => slot.insert(Nv12Scaler::new(source.size(), output, ScaleMode::Fill)),
        };
        scaler.scale(source, &mut self.frame)
    }

    fn ensure_segmenter(&mut self) -> Result<(), CoreError> {
        let frame = self.settings.output;
        let settings = self.settings.segmentation;
        let key = ModelKey::new(&settings, frame);
        if self.segmenter.as_ref().is_some_and(|s| s.key == key) {
            return Ok(());
        }
        let model = self.provider.load(&settings, frame)?;
        let params = settings.mask.unwrap_or_else(|| model.default_mask_params());
        let mask_size = model.mask_size();
        let refiner = MaskRefiner::new(mask_size.width(), mask_size.height(), frame, params);
        self.segmenter = Some(Segmenter { key, model, refiner });
        Ok(())
    }

    fn finish(
        &mut self,
        format: OutputFormat,
        matrix: ColorMatrix,
        passthrough: bool,
    ) -> Result<PipelineOutput<'_>, CoreError> {
        let result = if passthrough { &self.frame } else { &self.composed };
        match format {
            OutputFormat::Nv12 => Ok(PipelineOutput::Nv12(result)),
            OutputFormat::Bgra => {
                nv12_to_bgra(result, None, matrix, &mut self.bgra)?;
                Ok(PipelineOutput::Bgra(&self.bgra))
            }
        }
    }
}
