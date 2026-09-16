use std::path::Path;

use half::f16;
use ort::memory::Allocator;
use ort::session::{HasSelectedOutputs, OutputSelector, RunOptions, Session};
use ort::value::{DynValue, Tensor, TensorElementType};

use super::session::{expect_tensor_input, inference_error, open_session};
use super::{AlphaMask, InferenceDevice, SegmentationModel};
use crate::color::ColorMatrix;
use crate::error::CoreError;
use crate::frame::{FrameSize, Nv12Frame};
use crate::mask::{MaskParams, quantize_unit_f16};
use crate::model_input::{ModelInputBuilder, TensorLayout};
use crate::variant::ModelVariant;

const STATE_INPUTS: [&str; 4] = ["r1i", "r2i", "r3i", "r4i"];
const STATE_OUTPUTS: [&str; 4] = ["r1o", "r2o", "r3o", "r4o"];

pub struct RvmModel {
    name: String,
    session: Session,
    model_size: FrameSize,
    state_shapes: [Vec<i64>; 4],
    source: Tensor<f16>,
    states: [DynValue; 4],
    input_builder: Option<ModelInputBuilder<f16>>,
    run_options: RunOptions<HasSelectedOutputs>,
    alpha: Vec<u8>,
}

fn zero_state(shape: &[i64]) -> Result<DynValue, CoreError> {
    let len = shape.iter().product::<i64>() as usize;
    Ok(Tensor::from_array((shape.to_vec(), vec![f16::ZERO; len]))
        .map_err(inference_error)?
        .into_dyn())
}

impl RvmModel {
    pub fn load(models_dir: &Path, variant: ModelVariant, device: InferenceDevice) -> Result<Self, CoreError> {
        let session = open_session(&models_dir.join(variant.rvm_file_name()), device)?;
        let source_shape = expect_tensor_input(&session, 0, "src", TensorElementType::Float16)?;
        let model_size = FrameSize::new(variant.width, variant.height)?;
        if source_shape != [1, 3, i64::from(variant.height), i64::from(variant.width)] {
            return Err(CoreError::ModelFormat(format!(
                "src shape {source_shape:?} does not match variant {}x{}",
                variant.width, variant.height
            )));
        }
        let state_shapes = [
            expect_tensor_input(&session, 1, STATE_INPUTS[0], TensorElementType::Float16)?,
            expect_tensor_input(&session, 2, STATE_INPUTS[1], TensorElementType::Float16)?,
            expect_tensor_input(&session, 3, STATE_INPUTS[2], TensorElementType::Float16)?,
            expect_tensor_input(&session, 4, STATE_INPUTS[3], TensorElementType::Float16)?,
        ];
        let states = [
            zero_state(&state_shapes[0])?,
            zero_state(&state_shapes[1])?,
            zero_state(&state_shapes[2])?,
            zero_state(&state_shapes[3])?,
        ];
        let source = Tensor::<f16>::new(&Allocator::default(), source_shape).map_err(inference_error)?;
        let selector = STATE_OUTPUTS
            .iter()
            .fold(OutputSelector::no_default().with("pha"), |selector, name| {
                selector.with(*name)
            });
        let run_options = RunOptions::new().map_err(inference_error)?.with_outputs(selector);
        Ok(Self {
            name: format!("RVM MobileNetV3 fp16 {}x{}", variant.width, variant.height),
            session,
            model_size,
            state_shapes,
            source,
            states,
            input_builder: None,
            run_options,
            alpha: vec![0; model_size.luma_len()],
        })
    }
}

impl SegmentationModel for RvmModel {
    fn name(&self) -> &str {
        &self.name
    }

    fn mask_size(&self) -> FrameSize {
        self.model_size
    }

    fn default_mask_params(&self) -> MaskParams {
        MaskParams::RECURRENT_MODEL
    }

    fn segment(&mut self, frame: &Nv12Frame, matrix: ColorMatrix) -> Result<AlphaMask<'_>, CoreError> {
        let builder = match &mut self.input_builder {
            Some(builder) if builder.frame_size() == frame.size() => builder,
            slot => slot.insert(ModelInputBuilder::new(
                frame.size(),
                self.model_size,
                TensorLayout::Nchw,
                matrix,
            )),
        };
        let (_, source) = self.source.extract_tensor_mut();
        builder.write_into(frame, source)?;

        let mut outputs = self
            .session
            .run_with_options(
                ort::inputs![
                    &self.source,
                    &self.states[0],
                    &self.states[1],
                    &self.states[2],
                    &self.states[3]
                ],
                &self.run_options,
            )
            .map_err(inference_error)?;
        let (_, pha) = outputs
            .get("pha")
            .ok_or_else(|| CoreError::ModelFormat("missing output pha".into()))?
            .try_extract_tensor::<f16>()
            .map_err(inference_error)?;
        if pha.len() != self.alpha.len() {
            return Err(CoreError::ModelFormat(format!("pha has {} values", pha.len())));
        }
        quantize_unit_f16(pha, &mut self.alpha);
        for (state, name) in self.states.iter_mut().zip(STATE_OUTPUTS) {
            *state = outputs
                .remove(name)
                .ok_or_else(|| CoreError::ModelFormat(format!("missing output {name}")))?;
        }
        Ok(AlphaMask {
            width: self.model_size.width(),
            height: self.model_size.height(),
            data: &self.alpha,
        })
    }

    fn reset(&mut self) {
        for (state, shape) in self.states.iter_mut().zip(&self.state_shapes) {
            if let Ok(zero) = zero_state(shape) {
                *state = zero;
            }
        }
    }
}
