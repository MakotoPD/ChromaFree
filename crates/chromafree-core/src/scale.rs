use crate::error::CoreError;
use crate::frame::{FrameSize, Nv12Frame};

const FRACTION_BITS: u32 = 8;
const FRACTION_ONE: u32 = 1 << FRACTION_BITS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScaleMode {
    #[default]
    Fill,
    Stretch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SourceRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl SourceRect {
    fn for_mode(mode: ScaleMode, source_width: u32, source_height: u32, target_width: u32, target_height: u32) -> Self {
        let (sw, sh) = (f64::from(source_width), f64::from(source_height));
        match mode {
            ScaleMode::Stretch => Self {
                x: 0.0,
                y: 0.0,
                width: sw,
                height: sh,
            },
            ScaleMode::Fill => {
                let scale = (f64::from(target_width) / sw).max(f64::from(target_height) / sh);
                let width = f64::from(target_width) / scale;
                let height = f64::from(target_height) / scale;
                Self {
                    x: (sw - width) / 2.0,
                    y: (sh - height) / 2.0,
                    width,
                    height,
                }
            }
        }
    }

    fn halved(self) -> Self {
        Self {
            x: self.x / 2.0,
            y: self.y / 2.0,
            width: self.width / 2.0,
            height: self.height / 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tap {
    first: u32,
    second: u32,
    fraction: u32,
}

const NO_ROW: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct PlaneScaler {
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    channels: usize,
    column_taps: Vec<Tap>,
    row_taps: Vec<Tap>,
    cached_rows: [u32; 2],
    row_cache: [Vec<u16>; 2],
}

fn taps(source_len: u32, target_len: u32, rect_start: f64, rect_len: f64) -> Vec<Tap> {
    let last = source_len - 1;
    let max_index = f64::from(last);
    (0..target_len)
        .map(|i| {
            let position =
                ((f64::from(i) + 0.5) * rect_len / f64::from(target_len) - 0.5 + rect_start).clamp(0.0, max_index);
            let index = position.floor();
            let fraction = ((position - index) * f64::from(FRACTION_ONE)).round() as u32;
            let (first, fraction) = if fraction >= FRACTION_ONE {
                ((index as u32 + 1).min(last), 0)
            } else {
                (index as u32, fraction)
            };
            Tap {
                first,
                second: (first + 1).min(last),
                fraction,
            }
        })
        .collect()
}

fn interpolate_row<const C: usize>(taps: &[Tap], source_row: &[u8], output: &mut [u16]) {
    for (tap, pixel) in taps.iter().zip(output.as_chunks_mut::<C>().0.iter_mut()) {
        let left = &source_row[tap.first as usize * C..tap.first as usize * C + C];
        let right = &source_row[tap.second as usize * C..tap.second as usize * C + C];
        let fx = tap.fraction as u16;
        let ix = FRACTION_ONE as u16 - fx;
        for c in 0..C {
            pixel[c] = u16::from(left[c]) * ix + u16::from(right[c]) * fx;
        }
    }
}

fn blend_rows(top: &[u16], bottom: &[u16], fraction: u32, output: &mut [u8]) {
    if fraction == 0 {
        for (out, &t) in output.iter_mut().zip(top) {
            *out = ((u32::from(t) + (FRACTION_ONE >> 1)) >> FRACTION_BITS) as u8;
        }
        return;
    }
    let fy = fraction;
    let iy = FRACTION_ONE - fraction;
    let round = 1 << (2 * FRACTION_BITS - 1);
    for ((out, &t), &b) in output.iter_mut().zip(top).zip(bottom) {
        *out = ((u32::from(t) * iy + u32::from(b) * fy + round) >> (2 * FRACTION_BITS)) as u8;
    }
}

impl PlaneScaler {
    pub fn new(
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        channels: usize,
        mode: ScaleMode,
    ) -> Self {
        let rect = SourceRect::for_mode(mode, source_width, source_height, target_width, target_height);
        Self::with_rect(source_width, source_height, target_width, target_height, channels, rect)
    }

    fn with_rect(
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        channels: usize,
        rect: SourceRect,
    ) -> Self {
        let row_len = target_width as usize * channels;
        Self {
            source_width,
            source_height,
            target_width,
            target_height,
            channels,
            column_taps: taps(source_width, target_width, rect.x, rect.width),
            row_taps: taps(source_height, target_height, rect.y, rect.height),
            cached_rows: [NO_ROW; 2],
            row_cache: [vec![0; row_len], vec![0; row_len]],
        }
    }

    pub fn source_len(&self) -> usize {
        self.source_width as usize * self.source_height as usize * self.channels
    }

    pub fn target_len(&self) -> usize {
        self.target_width as usize * self.target_height as usize * self.channels
    }

    pub fn scale(&mut self, source: &[u8], target: &mut [u8]) -> Result<(), CoreError> {
        check_len(self.source_len(), source.len())?;
        check_len(self.target_len(), target.len())?;
        match self.channels {
            1 => self.scale_channels::<1>(source, target),
            2 => self.scale_channels::<2>(source, target),
            3 => self.scale_channels::<3>(source, target),
            _ => self.scale_channels::<4>(source, target),
        }
        Ok(())
    }

    fn ensure_row<const C: usize>(&mut self, source: &[u8], row: u32, keep: Option<usize>) -> usize {
        if let Some(slot) = self.cached_rows.iter().position(|&cached| cached == row) {
            return slot;
        }
        let slot = match keep {
            Some(kept) => 1 - kept,
            None => usize::from(self.cached_rows[1] == NO_ROW || self.cached_rows[1] < self.cached_rows[0]),
        };
        let stride = self.source_width as usize * C;
        let start = row as usize * stride;
        interpolate_row::<C>(
            &self.column_taps,
            &source[start..start + stride],
            &mut self.row_cache[slot],
        );
        self.cached_rows[slot] = row;
        slot
    }

    fn scale_channels<const C: usize>(&mut self, source: &[u8], target: &mut [u8]) {
        self.cached_rows = [NO_ROW; 2];
        let row_len = self.target_width as usize * C;
        for (index, target_row) in target.chunks_exact_mut(row_len).enumerate() {
            let tap = self.row_taps[index];
            let top = self.ensure_row::<C>(source, tap.first, None);
            if tap.fraction == 0 {
                blend_rows(&self.row_cache[top], &self.row_cache[top], 0, target_row);
                continue;
            }
            let bottom = self.ensure_row::<C>(source, tap.second, Some(top));
            blend_rows(&self.row_cache[top], &self.row_cache[bottom], tap.fraction, target_row);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Nv12Scaler {
    source: FrameSize,
    target: FrameSize,
    luma: PlaneScaler,
    chroma: PlaneScaler,
    identity: bool,
}

impl Nv12Scaler {
    pub fn new(source: FrameSize, target: FrameSize, mode: ScaleMode) -> Self {
        let rect = SourceRect::for_mode(mode, source.width(), source.height(), target.width(), target.height());
        Self {
            source,
            target,
            luma: PlaneScaler::with_rect(
                source.width(),
                source.height(),
                target.width(),
                target.height(),
                1,
                rect,
            ),
            chroma: PlaneScaler::with_rect(
                source.chroma_width(),
                source.chroma_height(),
                target.chroma_width(),
                target.chroma_height(),
                2,
                rect.halved(),
            ),
            identity: source == target,
        }
    }

    pub fn source(&self) -> FrameSize {
        self.source
    }

    pub fn target(&self) -> FrameSize {
        self.target
    }

    pub fn scale(&mut self, source: &Nv12Frame, target: &mut Nv12Frame) -> Result<(), CoreError> {
        self.source.ensure_eq(source.size())?;
        self.target.ensure_eq(target.size())?;
        if self.identity {
            return target.copy_from(source);
        }
        let (luma, chroma) = target.planes_mut();
        self.luma.scale(source.luma(), luma)?;
        self.chroma.scale(source.chroma(), chroma)
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
    use crate::color::Yuv;

    #[test]
    fn identity_scale_copies_plane() {
        let mut scaler = PlaneScaler::new(4, 3, 4, 3, 1, ScaleMode::Fill);
        let source: Vec<u8> = (0..12).map(|i| i * 10).collect();
        let mut target = vec![0; 12];
        scaler.scale(&source, &mut target).unwrap();
        assert_eq!(target, source);
    }

    #[test]
    fn upscale_interpolates_between_neighbours() {
        let mut scaler = PlaneScaler::new(2, 1, 4, 1, 1, ScaleMode::Stretch);
        let mut target = vec![0; 4];
        scaler.scale(&[0, 200], &mut target).unwrap();
        assert_eq!(target, vec![0, 50, 150, 200]);
    }

    #[test]
    fn downscale_by_two_averages_pairs() {
        let mut scaler = PlaneScaler::new(4, 2, 2, 1, 1, ScaleMode::Stretch);
        let mut target = vec![0; 2];
        scaler.scale(&[0, 100, 200, 40, 20, 120, 220, 60], &mut target).unwrap();
        assert_eq!(target, vec![60, 130]);
    }

    #[test]
    fn fill_mode_crops_the_centre_of_a_wider_source() {
        let source: Vec<u8> = [0u8, 0, 90, 90, 180, 180, 0, 0].repeat(2);
        let mut scaler = PlaneScaler::new(8, 2, 4, 2, 1, ScaleMode::Fill);
        let mut target = vec![0; 8];
        scaler.scale(&source, &mut target).unwrap();
        assert_eq!(&target[..4], &[90, 90, 180, 180]);
    }

    #[test]
    fn chroma_channels_are_scaled_independently() {
        let mut scaler = PlaneScaler::new(2, 1, 4, 1, 2, ScaleMode::Stretch);
        let mut target = vec![0; 8];
        scaler.scale(&[0, 255, 200, 55], &mut target).unwrap();
        assert_eq!(target, vec![0, 255, 50, 205, 150, 105, 200, 55]);
    }

    #[test]
    fn uniform_nv12_frame_stays_uniform_after_scaling() {
        let color = Yuv { y: 81, u: 90, v: 240 };
        let source = Nv12Frame::filled(FrameSize::new(1920, 1080).unwrap(), color);
        let target_size = FrameSize::new(640, 480).unwrap();
        let mut target = Nv12Frame::new(target_size);
        Nv12Scaler::new(source.size(), target_size, ScaleMode::Fill)
            .scale(&source, &mut target)
            .unwrap();
        assert_eq!(target, Nv12Frame::filled(target_size, color));
    }
}
