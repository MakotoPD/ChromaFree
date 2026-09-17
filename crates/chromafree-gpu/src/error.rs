use chromafree_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("Direct3D call failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("shader `{entry}` failed to compile: {message}")]
    Shader { entry: String, message: String },
    #[error("no DirectX 12 adapter with index {0}")]
    AdapterNotFound(u32),
    #[error("DirectML is not available: {0}")]
    DirectMl(String),
    #[error("the GPU device was removed: {0}")]
    DeviceRemoved(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error(transparent)]
    Core(#[from] CoreError),
}

pub(crate) fn inference_error(error: impl std::fmt::Display) -> GpuError {
    GpuError::Inference(error.to_string())
}
