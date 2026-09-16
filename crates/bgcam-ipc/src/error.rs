use thiserror::Error;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("Windows call failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error(
        "shared memory has magic {magic:#x} and protocol version {version}, expected {expected_magic:#x} and {expected_version}"
    )]
    IncompatibleProtocol {
        magic: u32,
        version: u32,
        expected_magic: u32,
        expected_version: u32,
    },
    #[error("mapped region of {actual} bytes is smaller than the required {required}")]
    RegionTooSmall { actual: usize, required: usize },
    #[error("frame of {size} bytes does not fit the {capacity} byte capacity")]
    FrameTooLarge { size: usize, capacity: usize },
    #[error("output buffer of {actual} bytes cannot hold a {needed} byte frame")]
    BufferTooSmall { needed: usize, actual: usize },
    #[error("frame size {width}x{height} is invalid")]
    InvalidDescriptor { width: u32, height: u32 },
    #[error("the writer kept updating the frame while it was being read")]
    Contended,
}
