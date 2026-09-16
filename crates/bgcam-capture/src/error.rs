use bgcam_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("Media Foundation call failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("camera {0} is not connected")]
    CameraNotFound(String),
    #[error("camera does not offer {0}")]
    FormatNotAvailable(String),
    #[error("camera stopped delivering frames")]
    StreamEnded,
    #[error("camera frame buffer has {actual} bytes, expected at least {expected}")]
    ShortBuffer { expected: usize, actual: usize },
    #[error(transparent)]
    Core(#[from] CoreError),
}
