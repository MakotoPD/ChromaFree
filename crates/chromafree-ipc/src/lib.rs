mod camera;
mod channel;
mod error;
pub mod layout;
mod region;

pub use camera::{VIRTUAL_CAMERA_CLSID, VIRTUAL_CAMERA_NAME};
pub use channel::{ObjectNames, ProducerChannel, ProducerWaker, ReaderChannel, qpc_frequency, qpc_now};
pub use error::IpcError;
pub use region::{ConsumerState, FrameInfo, OutputMode, PixelFormat, SharedRegion};
