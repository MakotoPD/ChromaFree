mod mediapipe;
mod rvm;
mod session;

use crate::color::ColorMatrix;
use crate::error::CoreError;
use crate::frame::{FrameSize, Nv12Frame};
use crate::mask::MaskParams;

pub use mediapipe::{MediaPipeModel, MediaPipeVariant};
pub use rvm::RvmModel;
pub use session::{InferenceDevice, bundled_directml};

pub struct AlphaMask<'a> {
    pub width: u32,
    pub height: u32,
    pub data: &'a [u8],
}

pub trait SegmentationModel: Send {
    fn name(&self) -> &str;
    fn mask_size(&self) -> FrameSize;
    fn default_mask_params(&self) -> MaskParams;
    fn segment(&mut self, frame: &Nv12Frame, matrix: ColorMatrix) -> Result<AlphaMask<'_>, CoreError>;
    fn reset(&mut self);
}
