use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ort::ep;
use ort::session::Session;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::value::TensorElementType;

use crate::error::CoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InferenceDevice {
    DirectMl { adapter_index: u32 },
    Cpu { threads: usize },
}

impl Default for InferenceDevice {
    fn default() -> Self {
        Self::DirectMl { adapter_index: 0 }
    }
}

const DIRECTML_LIBRARY: &str = "DirectML.dll";

pub fn bundled_directml() -> Option<&'static Path> {
    static LOADED: OnceLock<Option<PathBuf>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            exe.ancestors()
                .skip(1)
                .take(2)
                .map(|dir| dir.join(DIRECTML_LIBRARY))
                .find(|candidate| candidate.is_file() && ort::util::preload_dylib(candidate.as_path()).is_ok())
        })
        .as_deref()
}

pub(crate) fn inference_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Inference(error.to_string())
}

fn configure(device: InferenceDevice) -> Result<SessionBuilder, CoreError> {
    let builder = Session::builder()
        .map_err(inference_error)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(inference_error)?
        .with_intra_op_spinning(false)
        .map_err(inference_error)?
        .with_inter_op_spinning(false)
        .map_err(inference_error)?;
    match device {
        InferenceDevice::DirectMl { adapter_index } => {
            bundled_directml();
            builder
                .with_memory_pattern(false)
                .map_err(inference_error)?
                .with_parallel_execution(false)
                .map_err(inference_error)?
                .with_execution_providers([ep::DirectML::default()
                    .with_device_id(adapter_index as i32)
                    .build()
                    .error_on_failure()])
                .map_err(inference_error)
        }
        InferenceDevice::Cpu { threads } => builder.with_intra_threads(threads.max(1)).map_err(inference_error),
    }
}

pub(crate) fn open_session(path: &Path, device: InferenceDevice) -> Result<Session, CoreError> {
    if !path.is_file() {
        return Err(CoreError::ModelNotFound(path.display().to_string()));
    }
    configure(device)?.commit_from_file(path).map_err(inference_error)
}

pub(crate) fn expect_tensor_input(
    session: &Session,
    index: usize,
    name: &str,
    element: TensorElementType,
) -> Result<Vec<i64>, CoreError> {
    let input = session
        .inputs()
        .get(index)
        .ok_or_else(|| CoreError::ModelFormat(format!("missing input #{index} ({name})")))?;
    if input.name() != name {
        return Err(CoreError::ModelFormat(format!(
            "input #{index} is `{}`, expected `{name}`",
            input.name()
        )));
    }
    if input.dtype().tensor_type() != Some(element) {
        return Err(CoreError::ModelFormat(format!("input `{name}` is not {element:?}")));
    }
    let shape = input
        .dtype()
        .tensor_shape()
        .ok_or_else(|| CoreError::ModelFormat(format!("input `{name}` has no shape")))?;
    if shape.iter().any(|&d| d <= 0) {
        return Err(CoreError::ModelFormat(format!(
            "input `{name}` has dynamic shape {:?}; use a static model variant",
            &shape[..]
        )));
    }
    Ok(shape.to_vec())
}
