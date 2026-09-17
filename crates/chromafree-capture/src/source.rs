use std::time::{Duration, Instant};

use chromafree_core::{FrameSize, Nv12Frame};

use crate::error::CaptureError;

pub trait FrameSource {
    fn size(&self) -> FrameSize;
    fn read(&mut self) -> Result<&Nv12Frame, CaptureError>;

    fn capture_age(&self) -> Option<Duration> {
        None
    }
}

pub struct SyntheticSource {
    frame: Nv12Frame,
    period: Duration,
    next_frame: Instant,
    frame_number: u64,
}

impl SyntheticSource {
    pub fn new(size: FrameSize, fps: u32) -> Self {
        Self {
            frame: Nv12Frame::new(size),
            period: Duration::from_secs(1) / fps.max(1),
            next_frame: Instant::now(),
            frame_number: 0,
        }
    }

    pub fn frame_number(&self) -> u64 {
        self.frame_number
    }

    fn draw(&mut self) {
        let size = self.frame.size();
        let width = size.width() as usize;
        let bar_start = (self.frame_number as usize * 8) % width;
        let bar = bar_start..bar_start + width / 16;
        let (luma, chroma) = self.frame.planes_mut();
        for row in luma.chunks_exact_mut(width) {
            for (x, value) in row.iter_mut().enumerate() {
                *value = if bar.contains(&x) {
                    235
                } else {
                    16 + (x * 200 / width) as u8
                };
            }
        }
        chroma.fill(128);
    }
}

impl FrameSource for SyntheticSource {
    fn size(&self) -> FrameSize {
        self.frame.size()
    }

    fn read(&mut self) -> Result<&Nv12Frame, CaptureError> {
        let now = Instant::now();
        if self.next_frame > now {
            std::thread::sleep(self.next_frame - now);
        }
        self.next_frame = (self.next_frame + self.period).max(Instant::now());
        self.frame_number += 1;
        self.draw();
        Ok(&self.frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_source_is_paced_and_moves() {
        let mut source = SyntheticSource::new(FrameSize::new(64, 36).unwrap(), 100);
        let started = Instant::now();
        let first = source.read().unwrap().luma().to_vec();
        for _ in 0..9 {
            source.read().unwrap();
        }
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(85), "{elapsed:?}");
        assert_ne!(source.read().unwrap().luma(), first.as_slice());
        assert_eq!(source.frame_number(), 11);
    }
}
