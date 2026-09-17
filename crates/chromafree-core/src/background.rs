use std::path::Path;

use crate::color::{ColorMatrix, Rgb};
use crate::error::CoreError;
use crate::frame::{FrameSize, Nv12Frame};
use crate::scale::{PlaneScaler, ScaleMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl RgbImage {
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, CoreError> {
        let expected = width as usize * height as usize * 3;
        if width == 0 || height == 0 || pixels.len() != expected {
            return Err(CoreError::PlaneSizeMismatch {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self { width, height, pixels })
    }

    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let decoded = image::open(path).map_err(|e| CoreError::Image(e.to_string()))?;
        Self::from_dynamic(decoded)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let decoded = image::load_from_memory(bytes).map_err(|e| CoreError::Image(e.to_string()))?;
        Self::from_dynamic(decoded)
    }

    fn from_dynamic(decoded: image::DynamicImage) -> Result<Self, CoreError> {
        let rgb = decoded.into_rgb8();
        let (width, height) = rgb.dimensions();
        Self::from_pixels(width, height, rgb.into_raw())
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn to_nv12(&self, target: FrameSize, matrix: ColorMatrix) -> Result<Nv12Frame, CoreError> {
        let mut scaler = PlaneScaler::new(
            self.width,
            self.height,
            target.width(),
            target.height(),
            3,
            ScaleMode::Fill,
        );
        let mut scaled = vec![0; scaler.target_len()];
        scaler.scale(&self.pixels, &mut scaled)?;
        let mut frame = Nv12Frame::new(target);
        let width = target.width() as usize;
        let (luma, chroma) = frame.planes_mut();
        for (out, rgb) in luma.iter_mut().zip(scaled.chunks_exact(3)) {
            *out = matrix.to_yuv(Rgb::new(rgb[0], rgb[1], rgb[2])).y;
        }
        for (i, out) in chroma.chunks_exact_mut(2).enumerate() {
            let (cx, cy) = (i % target.chroma_width() as usize, i / target.chroma_width() as usize);
            let (mut u, mut v) = (0u32, 0u32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let offset = ((cy * 2 + dy) * width + cx * 2 + dx) * 3;
                let yuv = matrix.to_yuv(Rgb::new(scaled[offset], scaled[offset + 1], scaled[offset + 2]));
                u += u32::from(yuv.u);
                v += u32::from(yuv.v);
            }
            out[0] = ((u + 2) >> 2) as u8;
            out[1] = ((v + 2) >> 2) as u8;
        }
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_image_converts_to_uniform_nv12() {
        let matrix = ColorMatrix::Bt709;
        let image = RgbImage::from_pixels(3, 5, [0u8, 71, 187].repeat(15)).unwrap();
        let frame = image.to_nv12(FrameSize::new(64, 36).unwrap(), matrix).unwrap();
        let expected = matrix.to_yuv(Rgb::BLUE_SCREEN);
        assert_eq!(frame, Nv12Frame::filled(frame.size(), expected));
    }

    #[test]
    fn png_bytes_round_trip_through_decoder() {
        let mut png = Vec::new();
        let buffer = image::RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 0]));
        buffer
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let decoded = RgbImage::decode(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
        assert_eq!(&decoded.pixels()[..3], &[255, 0, 0]);
        assert!(RgbImage::decode(b"not an image").is_err());
    }
}
