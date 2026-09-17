use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;

use chromafree_core::{FrameSize, MediaPipeVariant, ModelVariant};
use ort::AsPointer;
use ort::ep::directml::{DMLResource, resource_from_d3d};
use ort::memory::{AllocationDevice, AllocatorType, MemoryInfo, MemoryType};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{IoBinding, Session};
use ort::sys::ONNXTensorElementDataType;
use ort::value::DynValue;
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
use windows::core::Interface;

use crate::device::GpuDevice;
use crate::error::{GpuError, inference_error};

const STATE_INPUTS: [&str; 4] = ["r1i", "r2i", "r3i", "r4i"];
const STATE_OUTPUTS: [&str; 4] = ["r1o", "r2o", "r3o", "r4o"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Element {
    Half,
    Float,
}

impl Element {
    fn bytes(self) -> usize {
        match self {
            Self::Half => 2,
            Self::Float => 4,
        }
    }

    fn onnx(self) -> ONNXTensorElementDataType {
        match self {
            Self::Half => ONNXTensorElementDataType::ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT16,
            Self::Float => ONNXTensorElementDataType::ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
        }
    }
}

pub(crate) struct GpuTensor {
    pub resource: ID3D12Resource,
    dml: DMLResource,
    shape: Vec<i64>,
    element: Element,
}

unsafe impl Send for GpuTensor {}

impl GpuTensor {
    fn new(device: &GpuDevice, shape: &[i64], element: Element) -> Result<Self, GpuError> {
        let bytes = shape.iter().product::<i64>() as usize * element.bytes();
        let resource = device.storage_buffer(bytes)?;
        let dml = unsafe { resource_from_d3d(resource.as_raw().cast::<()>()) }.map_err(inference_error)?;
        Ok(Self {
            resource,
            dml,
            shape: shape.to_vec(),
            element,
        })
    }

    fn value(&self) -> Result<DynValue, GpuError> {
        let info = MemoryInfo::new(
            AllocationDevice::DIRECTML,
            0,
            AllocatorType::Device,
            MemoryType::Default,
        )
        .map_err(inference_error)?;
        let bytes = self.shape.iter().product::<i64>() as usize * self.element.bytes();
        let mut value = std::ptr::null_mut();
        let status = unsafe {
            (ort::api().CreateTensorWithDataAsOrtValue)(
                info.ptr(),
                *self.dml,
                bytes,
                self.shape.as_ptr(),
                self.shape.len(),
                self.element.onnx(),
                &mut value,
            )
        };
        if !status.0.is_null() {
            let message = unsafe { std::ffi::CStr::from_ptr((ort::api().GetErrorMessage)(status.0)) }
                .to_string_lossy()
                .into_owned();
            unsafe { (ort::api().ReleaseStatus)(status.0) };
            return Err(GpuError::Inference(message));
        }
        let value = NonNull::new(value).ok_or_else(|| GpuError::Inference("null tensor".into()))?;
        Ok(unsafe { DynValue::from_ptr(value, None) })
    }
}

fn open_session(device: &GpuDevice, path: &Path) -> Result<Session, GpuError> {
    if !path.is_file() {
        return Err(chromafree_core::CoreError::ModelNotFound(path.display().to_string()).into());
    }
    let mut builder = Session::builder()
        .map_err(inference_error)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(inference_error)?
        .with_intra_op_spinning(false)
        .map_err(inference_error)?
        .with_inter_op_spinning(false)
        .map_err(inference_error)?
        .with_memory_pattern(false)
        .map_err(inference_error)?
        .with_parallel_execution(false)
        .map_err(inference_error)?;
    let api =
        ort::ep::directml::api().ok_or_else(|| GpuError::DirectMl("missing DirectML execution provider".into()))?;
    let status = unsafe {
        (api.SessionOptionsAppendExecutionProvider_DML1)(
            builder.ptr_mut(),
            device.dml.as_raw().cast::<c_void>(),
            device.queue.as_raw().cast::<c_void>(),
        )
    };
    if !status.0.is_null() {
        unsafe { (ort::api().ReleaseStatus)(status.0) };
        return Err(GpuError::DirectMl(
            "registering DirectML on the shared device failed".into(),
        ));
    }
    builder.commit_from_file(path).map_err(inference_error)
}

fn input_shape(session: &Session, name: &str) -> Result<Vec<i64>, GpuError> {
    let input = session
        .inputs()
        .iter()
        .find(|i| i.name() == name)
        .ok_or_else(|| GpuError::Inference(format!("model has no input `{name}`")))?;
    let shape = input
        .dtype()
        .tensor_shape()
        .ok_or_else(|| GpuError::Inference(format!("input `{name}` is not a tensor")))?;
    if shape.iter().any(|&d| d <= 0) {
        return Err(GpuError::Inference(format!("input `{name}` has a dynamic shape")));
    }
    Ok(shape.to_vec())
}

pub(crate) struct RvmGpu {
    name: String,
    session: Session,
    size: FrameSize,
    input: GpuTensor,
    alpha: GpuTensor,
    states: [Vec<GpuTensor>; 2],
    bindings: [IoBinding; 2],
    turn: usize,
}

pub(crate) struct MediaPipeGpu {
    name: String,
    session: Session,
    size: FrameSize,
    input: GpuTensor,
    alpha: GpuTensor,
    binding: IoBinding,
}

pub(crate) enum GpuModel {
    Rvm(Box<RvmGpu>),
    MediaPipe(Box<MediaPipeGpu>),
}

impl GpuModel {
    pub fn load_rvm(device: &GpuDevice, models_dir: &Path, variant: ModelVariant) -> Result<Self, GpuError> {
        let session = open_session(device, &models_dir.join(variant.rvm_file_name()))?;
        let size = FrameSize::new(variant.width, variant.height)?;
        let input = GpuTensor::new(device, &input_shape(&session, "src")?, Element::Half)?;
        let alpha = GpuTensor::new(
            device,
            &[1, 1, i64::from(variant.height), i64::from(variant.width)],
            Element::Half,
        )?;
        let shapes = STATE_INPUTS
            .iter()
            .map(|name| input_shape(&session, name))
            .collect::<Result<Vec<_>, _>>()?;
        let states = [
            shapes
                .iter()
                .map(|s| GpuTensor::new(device, s, Element::Half))
                .collect::<Result<Vec<_>, _>>()?,
            shapes
                .iter()
                .map(|s| GpuTensor::new(device, s, Element::Half))
                .collect::<Result<Vec<_>, _>>()?,
        ];
        let mut bindings = [
            session.create_binding().map_err(inference_error)?,
            session.create_binding().map_err(inference_error)?,
        ];
        for (turn, binding) in bindings.iter_mut().enumerate() {
            binding.bind_input("src", &input.value()?).map_err(inference_error)?;
            for (index, name) in STATE_INPUTS.iter().enumerate() {
                binding
                    .bind_input(*name, &states[turn][index].value()?)
                    .map_err(inference_error)?;
            }
            for (index, name) in STATE_OUTPUTS.iter().enumerate() {
                binding
                    .bind_output(*name, states[1 - turn][index].value()?)
                    .map_err(inference_error)?;
            }
            binding.bind_output("pha", alpha.value()?).map_err(inference_error)?;
        }
        Ok(Self::Rvm(Box::new(RvmGpu {
            name: format!("RVM MobileNetV3 fp16 {}x{} (DirectML)", variant.width, variant.height),
            session,
            size,
            input,
            alpha,
            states,
            bindings,
            turn: 0,
        })))
    }

    pub fn load_mediapipe(device: &GpuDevice, models_dir: &Path, variant: MediaPipeVariant) -> Result<Self, GpuError> {
        let session = open_session(device, &models_dir.join(variant.file_name()))?;
        let input_name = session
            .inputs()
            .first()
            .map(|i| i.name().to_owned())
            .ok_or_else(|| GpuError::Inference("model has no inputs".into()))?;
        let output_name = session
            .outputs()
            .first()
            .map(|o| o.name().to_owned())
            .ok_or_else(|| GpuError::Inference("model has no outputs".into()))?;
        let shape = input_shape(&session, &input_name)?;
        let [1, height, width, 3] = shape[..] else {
            return Err(GpuError::Inference(format!("unexpected input shape {shape:?}")));
        };
        let size = FrameSize::new(width as u32, height as u32)?;
        let input = GpuTensor::new(device, &shape, Element::Float)?;
        let alpha = GpuTensor::new(device, &[1, height, width, 1], Element::Float)?;
        let mut binding = session.create_binding().map_err(inference_error)?;
        binding
            .bind_input(input_name, &input.value()?)
            .map_err(inference_error)?;
        binding
            .bind_output(output_name, alpha.value()?)
            .map_err(inference_error)?;
        Ok(Self::MediaPipe(Box::new(MediaPipeGpu {
            name: format!("MediaPipe Selfie Segmentation {width}x{height} (DirectML)"),
            session,
            size,
            input,
            alpha,
            binding,
        })))
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Rvm(m) => &m.name,
            Self::MediaPipe(m) => &m.name,
        }
    }

    pub fn size(&self) -> FrameSize {
        match self {
            Self::Rvm(m) => m.size,
            Self::MediaPipe(m) => m.size,
        }
    }

    pub fn input_element(&self) -> Element {
        match self {
            Self::Rvm(_) => Element::Half,
            Self::MediaPipe(_) => Element::Float,
        }
    }

    pub fn input(&self) -> &ID3D12Resource {
        match self {
            Self::Rvm(m) => &m.input.resource,
            Self::MediaPipe(m) => &m.input.resource,
        }
    }

    pub fn alpha(&self) -> &ID3D12Resource {
        match self {
            Self::Rvm(m) => &m.alpha.resource,
            Self::MediaPipe(m) => &m.alpha.resource,
        }
    }

    pub fn is_recurrent(&self) -> bool {
        matches!(self, Self::Rvm(_))
    }

    pub fn run(&mut self) -> Result<(), GpuError> {
        match self {
            Self::Rvm(m) => {
                let binding = &m.bindings[m.turn];
                drop(m.session.run_binding(binding).map_err(inference_error)?);
                m.turn = 1 - m.turn;
            }
            Self::MediaPipe(m) => {
                drop(m.session.run_binding(&m.binding).map_err(inference_error)?);
            }
        }
        Ok(())
    }

    pub fn state_buffers(&self) -> Vec<&ID3D12Resource> {
        match self {
            Self::Rvm(m) => m.states.iter().flatten().map(|t| &t.resource).collect(),
            Self::MediaPipe(_) => Vec::new(),
        }
    }
}
