use std::path::Path;

use ort::memory::Allocator;
use ort::session::Session;
use ort::value::{Tensor, TensorElementType};

use super::session::{expect_tensor_input, inference_error, open_session};
use super::{AlphaMask, InferenceDevice, SegmentationModel};
use crate::color::ColorMatrix;
use crate::error::CoreError;
use crate::frame::{FrameSize, Nv12Frame};
use crate::mask::{MaskParams, quantize_unit_f32};
use crate::model_input::{ModelInputBuilder, TensorLayout};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaPipeVariant {
    Square,
    Landscape,
}

impl MediaPipeVariant {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Square => "selfie_segmenter.onnx",
            Self::Landscape => "selfie_segmenter_landscape.onnx",
        }
    }

    pub fn for_frame(frame: FrameSize) -> Self {
        if frame.width() * 9 >= frame.height() * 14 {
            Self::Landscape
        } else {
            Self::Square
        }
    }
}

pub struct MediaPipeModel {
    name: String,
    session: Session,
    model_size: FrameSize,
    input: Tensor<f32>,
    input_builder: Option<ModelInputBuilder<f32>>,
    alpha: Vec<u8>,
}

impl MediaPipeModel {
    pub fn load(models_dir: &Path, variant: MediaPipeVariant, device: InferenceDevice) -> Result<Self, CoreError> {
        let session = open_session(&models_dir.join(variant.file_name()), device)?;
        let input_name = session
            .inputs()
            .first()
            .map(|i| i.name().to_owned())
            .ok_or_else(|| CoreError::ModelFormat("model has no inputs".into()))?;
        let shape = expect_tensor_input(&session, 0, &input_name, TensorElementType::Float32)?;
        let [1, height, width, 3] = shape[..] else {
            return Err(CoreError::ModelFormat(format!("unexpected input shape {shape:?}")));
        };
        let model_size = FrameSize::new(width as u32, height as u32)?;
        let input = Tensor::<f32>::new(&Allocator::default(), shape).map_err(inference_error)?;
        Ok(Self {
            name: format!("MediaPipe Selfie Segmentation {width}x{height}"),
            session,
            model_size,
            input,
            input_builder: None,
            alpha: vec![0; model_size.luma_len()],
        })
    }
}

impl SegmentationModel for MediaPipeModel {
    fn name(&self) -> &str {
        &self.name
    }

    fn mask_size(&self) -> FrameSize {
        self.model_size
    }

    fn default_mask_params(&self) -> MaskParams {
        MaskParams::MEMORYLESS_MODEL
    }

    fn segment(&mut self, frame: &Nv12Frame, matrix: ColorMatrix) -> Result<AlphaMask<'_>, CoreError> {
        let builder = match &mut self.input_builder {
            Some(builder) if builder.frame_size() == frame.size() => builder,
            slot => slot.insert(ModelInputBuilder::new(
                frame.size(),
                self.model_size,
                TensorLayout::Nhwc,
                matrix,
            )),
        };
        let (_, input) = self.input.extract_tensor_mut();
        builder.write_into(frame, input)?;
        let outputs = self.session.run(ort::inputs![&self.input]).map_err(inference_error)?;
        let (_, values) = outputs[0].try_extract_tensor::<f32>().map_err(inference_error)?;
        if values.len() != self.alpha.len() {
            return Err(CoreError::ModelFormat(format!("mask has {} values", values.len())));
        }
        quantize_unit_f32(values, &mut self.alpha);
        Ok(AlphaMask {
            width: self.model_size.width(),
            height: self.model_size.height(),
            data: &self.alpha,
        })
    }

    fn reset(&mut self) {}
}
