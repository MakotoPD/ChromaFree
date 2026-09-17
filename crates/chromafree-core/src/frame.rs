use crate::color::Yuv;
use crate::error::CoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameSize {
    width: u32,
    height: u32,
}

impl FrameSize {
    pub fn new(width: u32, height: u32) -> Result<Self, CoreError> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(CoreError::InvalidFrameSize { width, height });
        }
        Ok(Self { width, height })
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    pub const fn chroma_width(self) -> u32 {
        self.width / 2
    }

    pub const fn chroma_height(self) -> u32 {
        self.height / 2
    }

    pub const fn luma_len(self) -> usize {
        self.width as usize * self.height as usize
    }

    pub const fn chroma_len(self) -> usize {
        self.chroma_width() as usize * self.chroma_height() as usize * 2
    }

    pub const fn swapped(self) -> Self {
        Self {
            width: self.height,
            height: self.width,
        }
    }

    pub fn ensure_eq(self, actual: FrameSize) -> Result<(), CoreError> {
        if self == actual {
            return Ok(());
        }
        Err(CoreError::FrameSizeMismatch {
            expected_width: self.width,
            expected_height: self.height,
            actual_width: actual.width,
            actual_height: actual.height,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nv12Frame {
    size: FrameSize,
    luma: Vec<u8>,
    chroma: Vec<u8>,
}

impl Nv12Frame {
    pub fn new(size: FrameSize) -> Self {
        Self::filled(size, Yuv::BLACK)
    }

    pub fn filled(size: FrameSize, color: Yuv) -> Self {
        let mut frame = Self {
            size,
            luma: vec![0; size.luma_len()],
            chroma: vec![0; size.chroma_len()],
        };
        frame.fill(color);
        frame
    }

    pub fn from_planes(size: FrameSize, luma: Vec<u8>, chroma: Vec<u8>) -> Result<Self, CoreError> {
        check_len(size.luma_len(), luma.len())?;
        check_len(size.chroma_len(), chroma.len())?;
        Ok(Self { size, luma, chroma })
    }

    pub fn size(&self) -> FrameSize {
        self.size
    }

    pub fn luma(&self) -> &[u8] {
        &self.luma
    }

    pub fn chroma(&self) -> &[u8] {
        &self.chroma
    }

    pub fn planes_mut(&mut self) -> (&mut [u8], &mut [u8]) {
        (&mut self.luma, &mut self.chroma)
    }

    pub fn fill(&mut self, color: Yuv) {
        self.luma.fill(color.y);
        for pair in self.chroma.as_chunks_mut::<2>().0.iter_mut() {
            pair[0] = color.u;
            pair[1] = color.v;
        }
    }

    pub fn copy_from(&mut self, source: &Nv12Frame) -> Result<(), CoreError> {
        self.size.ensure_eq(source.size)?;
        self.luma.copy_from_slice(&source.luma);
        self.chroma.copy_from_slice(&source.chroma);
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BgraFrame {
    size: FrameSize,
    data: Vec<u8>,
}

impl BgraFrame {
    pub fn new(size: FrameSize) -> Self {
        Self {
            size,
            data: vec![0; size.luma_len() * 4],
        }
    }

    pub fn from_data(size: FrameSize, data: Vec<u8>) -> Result<Self, CoreError> {
        check_len(size.luma_len() * 4, data.len())?;
        Ok(Self { size, data })
    }

    pub fn size(&self) -> FrameSize {
        self.size
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Mask {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize],
        }
    }

    pub fn from_data(width: u32, height: u32, data: Vec<u8>) -> Result<Self, CoreError> {
        check_len(width as usize * height as usize, data.len())?;
        Ok(Self { width, height, data })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

fn check_len(expected: usize, actual: usize) -> Result<(), CoreError> {
    if expected == actual {
        Ok(())
    } else {
        Err(CoreError::PlaneSizeMismatch { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_size_rejects_odd_and_zero_dimensions() {
        assert!(FrameSize::new(1280, 720).is_ok());
        assert_eq!(
            FrameSize::new(641, 360),
            Err(CoreError::InvalidFrameSize {
                width: 641,
                height: 360
            })
        );
        assert!(FrameSize::new(640, 0).is_err());
    }

    #[test]
    fn nv12_plane_lengths_follow_size() {
        let size = FrameSize::new(6, 4).unwrap();
        let frame = Nv12Frame::new(size);
        assert_eq!(frame.luma().len(), 24);
        assert_eq!(frame.chroma().len(), 12);
        assert!(frame.luma().iter().all(|&y| y == Yuv::BLACK.y));
        assert!(Nv12Frame::from_planes(size, vec![0; 24], vec![0; 11]).is_err());
    }
}
