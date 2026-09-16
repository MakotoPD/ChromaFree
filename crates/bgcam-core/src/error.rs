use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("frame size {width}x{height} must be non-zero and even in both dimensions")]
    InvalidFrameSize { width: u32, height: u32 },
    #[error("plane has {actual} bytes, expected {expected}")]
    PlaneSizeMismatch { expected: usize, actual: usize },
    #[error("frame is {actual_width}x{actual_height}, expected {expected_width}x{expected_height}")]
    FrameSizeMismatch {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
}
