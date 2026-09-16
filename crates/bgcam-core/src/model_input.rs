use half::f16;

use crate::color::{ColorMatrix, clamp_u8};
use crate::error::CoreError;
use crate::frame::{FrameSize, Nv12Frame};
use crate::scale::{Nv12Scaler, ScaleMode};

pub trait TensorElement: Copy + Default + Send + Sync + 'static {
    fn from_unit(value: f32) -> Self;
    fn to_unit(self) -> f32;
}

impl TensorElement for f32 {
    fn from_unit(value: f32) -> Self {
        value
    }

    fn to_unit(self) -> f32 {
        self
    }
}

impl TensorElement for f16 {
    fn from_unit(value: f32) -> Self {
        f16::from_f32(value)
    }

    fn to_unit(self) -> f32 {
        self.to_f32()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TensorLayout {
    Nchw,
    Nhwc,
}

pub struct ModelInputBuilder<T: TensorElement> {
    frame_size: FrameSize,
    model_size: FrameSize,
    layout: TensorLayout,
    matrix: ColorMatrix,
    scaler: Nv12Scaler,
    scaled: Nv12Frame,
    lookup: [T; 256],
    tensor: Vec<T>,
}

impl<T: TensorElement> ModelInputBuilder<T> {
    pub fn new(frame_size: FrameSize, model_size: FrameSize, layout: TensorLayout, matrix: ColorMatrix) -> Self {
        let lookup = std::array::from_fn(|i| T::from_unit(i as f32 / 255.0));
        Self {
            frame_size,
            model_size,
            layout,
            matrix,
            scaler: Nv12Scaler::new(frame_size, model_size, ScaleMode::Stretch),
            scaled: Nv12Frame::new(model_size),
            lookup,
            tensor: vec![T::default(); model_size.luma_len() * 3],
        }
    }

    pub fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    pub fn model_size(&self) -> FrameSize {
        self.model_size
    }

    pub fn shape(&self) -> [i64; 4] {
        let (w, h) = (i64::from(self.model_size.width()), i64::from(self.model_size.height()));
        match self.layout {
            TensorLayout::Nchw => [1, 3, h, w],
            TensorLayout::Nhwc => [1, h, w, 3],
        }
    }

    pub fn build(&mut self, frame: &Nv12Frame) -> Result<&[T], CoreError> {
        self.scaler.scale(frame, &mut self.scaled)?;
        let width = self.model_size.width() as usize;
        let pixels = self.model_size.luma_len();
        let k = self.matrix.inverse();
        let lookup = &self.lookup;
        let luma = self.scaled.luma();
        let chroma = self.scaled.chroma();
        let convert = |index: usize| {
            let (x, y) = (index % width, index / width);
            let chroma_index = (y / 2) * width + (x / 2) * 2;
            let c = 298 * (i32::from(luma[index]) - 16);
            let d = i32::from(chroma[chroma_index]) - 128;
            let e = i32::from(chroma[chroma_index + 1]) - 128;
            let r = clamp_u8((c + k.red_v * e + 128) >> 8);
            let g = clamp_u8((c + k.green_u * d + k.green_v * e + 128) >> 8);
            let b = clamp_u8((c + k.blue_u * d + 128) >> 8);
            (lookup[usize::from(r)], lookup[usize::from(g)], lookup[usize::from(b)])
        };
        match self.layout {
            TensorLayout::Nchw => {
                let (red, rest) = self.tensor.split_at_mut(pixels);
                let (green, blue) = rest.split_at_mut(pixels);
                for (index, ((r, g), b)) in red.iter_mut().zip(green.iter_mut()).zip(blue.iter_mut()).enumerate() {
                    (*r, *g, *b) = convert(index);
                }
            }
            TensorLayout::Nhwc => {
                for (index, pixel) in self.tensor.chunks_exact_mut(3).enumerate() {
                    (pixel[0], pixel[1], pixel[2]) = convert(index);
                }
            }
        }
        Ok(&self.tensor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    fn solid(width: u32, height: u32, rgb: Rgb, matrix: ColorMatrix) -> Nv12Frame {
        Nv12Frame::filled(FrameSize::new(width, height).unwrap(), matrix.to_yuv(rgb))
    }

    #[test]
    fn nchw_planes_hold_red_green_blue_in_unit_range() {
        let matrix = ColorMatrix::Bt709;
        let frame = solid(1280, 720, Rgb::new(255, 128, 0), matrix);
        let mut builder = ModelInputBuilder::<f32>::new(
            frame.size(),
            FrameSize::new(64, 36).unwrap(),
            TensorLayout::Nchw,
            matrix,
        );
        assert_eq!(builder.shape(), [1, 3, 36, 64]);
        let tensor = builder.build(&frame).unwrap();
        let pixels = 64 * 36;
        assert!(tensor[..pixels].iter().all(|v| (v - 1.0).abs() < 0.02));
        assert!(tensor[pixels..2 * pixels].iter().all(|v| (v - 0.5).abs() < 0.02));
        assert!(tensor[2 * pixels..].iter().all(|v| v.abs() < 0.02));
    }

    #[test]
    fn nhwc_interleaves_channels_and_f16_matches_f32() {
        let matrix = ColorMatrix::Bt601;
        let frame = solid(640, 480, Rgb::GREEN_SCREEN, matrix);
        let model = FrameSize::new(256, 144).unwrap();
        let mut full = ModelInputBuilder::<f32>::new(frame.size(), model, TensorLayout::Nhwc, matrix);
        let mut half = ModelInputBuilder::<f16>::new(frame.size(), model, TensorLayout::Nhwc, matrix);
        let a = full.build(&frame).unwrap().to_vec();
        let b = half.build(&frame).unwrap();
        assert_eq!(full.shape(), [1, 144, 256, 3]);
        assert!(a[1] > 0.6 && a[0] < 0.02);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x - y.to_f32()).abs() < 1e-3);
        }
    }
}
