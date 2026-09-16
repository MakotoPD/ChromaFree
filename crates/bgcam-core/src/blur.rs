use crate::error::CoreError;
use crate::frame::{FrameSize, Nv12Frame};
use crate::scale::{PlaneScaler, ScaleMode};

const MIN_LUMA_LEVELS: f32 = 1.0;
const MAX_LUMA_LEVELS: f32 = 6.0;

struct Level {
    width: u32,
    height: u32,
    data: Vec<u8>,
    scratch: Vec<u8>,
    upscaler: PlaneScaler,
}

struct FractionalLevel {
    width: u32,
    height: u32,
    data: Vec<u8>,
    scratch: Vec<u8>,
    downscaler: PlaneScaler,
    upscaler: PlaneScaler,
}

struct PlanePyramid {
    width: u32,
    height: u32,
    channels: usize,
    levels: Vec<Level>,
    fractional: Option<FractionalLevel>,
}

impl PlanePyramid {
    fn new(width: u32, height: u32, channels: usize, levels: f32) -> Self {
        let depth = levels.max(0.0).floor() as usize;
        let mut dimensions = Vec::with_capacity(depth);
        let (mut w, mut h) = (width, height);
        for _ in 0..depth {
            if w < 4 || h < 4 {
                break;
            }
            w = w.div_ceil(2);
            h = h.div_ceil(2);
            dimensions.push((w, h));
        }
        let level_list = dimensions
            .iter()
            .enumerate()
            .map(|(i, &(w, h))| {
                let (parent_w, parent_h) = if i == 0 { (width, height) } else { dimensions[i - 1] };
                let len = w as usize * h as usize * channels;
                Level {
                    width: w,
                    height: h,
                    data: vec![0; len],
                    scratch: vec![0; len],
                    upscaler: PlaneScaler::new(w, h, parent_w, parent_h, channels, ScaleMode::Stretch),
                }
            })
            .collect::<Vec<_>>();
        let fraction = if dimensions.len() == depth {
            levels - depth as f32
        } else {
            0.0
        };
        let (base_w, base_h) = dimensions.last().copied().unwrap_or((width, height));
        let factor = 2f32.powf(fraction);
        let (frac_w, frac_h) = (
            ((base_w as f32 / factor).round() as u32).max(2),
            ((base_h as f32 / factor).round() as u32).max(2),
        );
        let fractional = (fraction > 0.01 && (frac_w, frac_h) != (base_w, base_h)).then(|| {
            let len = frac_w as usize * frac_h as usize * channels;
            FractionalLevel {
                width: frac_w,
                height: frac_h,
                data: vec![0; len],
                scratch: vec![0; len],
                downscaler: PlaneScaler::new(base_w, base_h, frac_w, frac_h, channels, ScaleMode::Stretch),
                upscaler: PlaneScaler::new(frac_w, frac_h, base_w, base_h, channels, ScaleMode::Stretch),
            }
        });
        Self {
            width,
            height,
            channels,
            levels: level_list,
            fractional,
        }
    }

    fn blur(&mut self, source: &[u8], target: &mut [u8]) -> Result<(), CoreError> {
        let channels = self.channels;
        let (mut parent_w, mut parent_h) = (self.width, self.height);
        for i in 0..self.levels.len() {
            let (done, rest) = self.levels.split_at_mut(i);
            let level = &mut rest[0];
            let parent = done.last().map_or(source, |l| l.data.as_slice());
            downsample_dispatch(
                parent,
                parent_w,
                parent_h,
                &mut level.data,
                level.width,
                level.height,
                channels,
            );
            (parent_w, parent_h) = (level.width, level.height);
        }

        let Some(deepest) = self.levels.last_mut() else {
            match &mut self.fractional {
                Some(f) => {
                    f.downscaler.scale(source, &mut f.data)?;
                    tent_dispatch(&mut f.data, &mut f.scratch, f.width, f.height, channels);
                    f.upscaler.scale(&f.data, target)?;
                }
                None => target.copy_from_slice(source),
            }
            return Ok(());
        };
        if let Some(f) = &mut self.fractional {
            f.downscaler.scale(&deepest.data, &mut f.data)?;
            tent_dispatch(&mut f.data, &mut f.scratch, f.width, f.height, channels);
            f.upscaler.scale(&f.data, &mut deepest.data)?;
        }
        tent_dispatch(
            &mut deepest.data,
            &mut deepest.scratch,
            deepest.width,
            deepest.height,
            channels,
        );

        let deepest_index = self.levels.len() - 1;
        for i in (1..=deepest_index).rev() {
            let (upper, lower) = self.levels.split_at_mut(i);
            let Level { upscaler, data, .. } = &mut lower[0];
            let parent = &mut upper[i - 1];
            upscaler.scale(data, &mut parent.data)?;
            tent_dispatch(
                &mut parent.data,
                &mut parent.scratch,
                parent.width,
                parent.height,
                channels,
            );
        }
        let Level { upscaler, data, .. } = &mut self.levels[0];
        upscaler.scale(data, target)
    }
}

fn downsample_dispatch(
    source: &[u8],
    width: u32,
    height: u32,
    target: &mut [u8],
    target_width: u32,
    target_height: u32,
    channels: usize,
) {
    debug_assert_eq!(target.len(), target_width as usize * target_height as usize * channels);
    if channels == 1 {
        downsample::<1>(source, width as usize, height as usize, target, target_width as usize);
    } else {
        downsample::<2>(source, width as usize, height as usize, target, target_width as usize);
    }
}

fn downsample<const C: usize>(source: &[u8], width: usize, height: usize, target: &mut [u8], target_width: usize) {
    let stride = width * C;
    for (ty, row) in target.chunks_exact_mut(target_width * C).enumerate() {
        let y0 = ty * 2;
        let y1 = (y0 + 1).min(height - 1);
        let top = &source[y0 * stride..(y0 + 1) * stride];
        let bottom = &source[y1 * stride..(y1 + 1) * stride];
        let full_pairs = width / 2;
        for (tx, pixel) in row.chunks_exact_mut(C).enumerate() {
            let x0 = tx * 2 * C;
            let x1 = if tx < full_pairs { x0 + C } else { x0 };
            for c in 0..C {
                let sum = u16::from(top[x0 + c])
                    + u16::from(top[x1 + c])
                    + u16::from(bottom[x0 + c])
                    + u16::from(bottom[x1 + c]);
                pixel[c] = ((sum + 2) >> 2) as u8;
            }
        }
    }
}

fn tent_dispatch(data: &mut [u8], scratch: &mut [u8], width: u32, height: u32, channels: usize) {
    if channels == 1 {
        tent::<1>(data, scratch, width as usize, height as usize);
    } else {
        tent::<2>(data, scratch, width as usize, height as usize);
    }
}

fn tent<const C: usize>(data: &mut [u8], scratch: &mut [u8], width: usize, height: usize) {
    let stride = width * C;
    for (source_row, target_row) in data.chunks_exact(stride).zip(scratch.chunks_exact_mut(stride)) {
        let last = stride - C;
        let right_of_first = if width > 1 { C } else { 0 };
        let left_of_last = last.saturating_sub(C);
        for c in 0..C {
            let first = 3 * u16::from(source_row[c]) + u16::from(source_row[right_of_first + c]);
            target_row[c] = ((first + 2) >> 2) as u8;
            let final_pixel = u16::from(source_row[left_of_last + c]) + 3 * u16::from(source_row[last + c]);
            target_row[last + c] = ((final_pixel + 2) >> 2) as u8;
        }
        if width > 2 {
            for (out, window) in target_row[C..last].iter_mut().zip(source_row.windows(2 * C + 1)) {
                let sum = u16::from(window[0]) + 2 * u16::from(window[C]) + u16::from(window[2 * C]);
                *out = ((sum + 2) >> 2) as u8;
            }
        }
    }
    for y in 0..height {
        let up = y.saturating_sub(1) * stride;
        let middle = y * stride;
        let down = (y + 1).min(height - 1) * stride;
        let (above, center, below) = (
            &scratch[up..up + stride],
            &scratch[middle..middle + stride],
            &scratch[down..down + stride],
        );
        for (((out, &a), &m), &b) in data[middle..middle + stride]
            .iter_mut()
            .zip(above)
            .zip(center)
            .zip(below)
        {
            let sum = u16::from(a) + 2 * u16::from(m) + u16::from(b);
            *out = ((sum + 2) >> 2) as u8;
        }
    }
}

pub struct BackgroundBlur {
    size: FrameSize,
    strength: f32,
    luma: PlanePyramid,
    chroma: PlanePyramid,
}

impl BackgroundBlur {
    pub fn new(size: FrameSize, strength: f32) -> Self {
        let strength = strength.clamp(0.0, 1.0);
        let luma_levels = MIN_LUMA_LEVELS + strength * (MAX_LUMA_LEVELS - MIN_LUMA_LEVELS);
        Self {
            size,
            strength,
            luma: PlanePyramid::new(size.width(), size.height(), 1, luma_levels),
            chroma: PlanePyramid::new(size.chroma_width(), size.chroma_height(), 2, luma_levels - 1.0),
        }
    }

    pub fn size(&self) -> FrameSize {
        self.size
    }

    pub fn strength(&self) -> f32 {
        self.strength
    }

    pub fn apply(&mut self, source: &Nv12Frame, target: &mut Nv12Frame) -> Result<(), CoreError> {
        self.size.ensure_eq(source.size())?;
        self.size.ensure_eq(target.size())?;
        let (luma, chroma) = target.planes_mut();
        self.luma.blur(source.luma(), luma)?;
        self.chroma.blur(source.chroma(), chroma)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Yuv;

    #[test]
    fn uniform_frame_stays_uniform() {
        let size = FrameSize::new(320, 180).unwrap();
        let color = Yuv { y: 100, u: 60, v: 200 };
        let source = Nv12Frame::filled(size, color);
        let mut target = Nv12Frame::new(size);
        BackgroundBlur::new(size, 0.6).apply(&source, &mut target).unwrap();
        assert_eq!(target, source);
    }

    #[test]
    fn sharp_edge_is_spread_and_energy_is_roughly_preserved() {
        let size = FrameSize::new(256, 64).unwrap();
        let mut source = Nv12Frame::filled(size, Yuv::BLACK);
        for row in source.planes_mut().0.chunks_exact_mut(256) {
            row[128..].fill(235);
        }
        let mut target = Nv12Frame::new(size);
        let mut blur = BackgroundBlur::new(size, 0.4);
        blur.apply(&source, &mut target).unwrap();
        let row = &target.luma()[32 * 256..33 * 256];
        assert!(row[124] > 16 && row[124] < 235);
        assert!(row[132] > 16 && row[132] < 235);
        let before: u64 = source.luma().iter().map(|&v| u64::from(v)).sum();
        let after: u64 = target.luma().iter().map(|&v| u64::from(v)).sum();
        assert!(before.abs_diff(after) * 50 < before);
    }

    #[test]
    fn stronger_blur_spreads_further() {
        let size = FrameSize::new(512, 256).unwrap();
        let mut source = Nv12Frame::filled(size, Yuv::BLACK);
        for row in source.planes_mut().0.chunks_exact_mut(512) {
            row[256..].fill(235);
        }
        let spread = |strength| {
            let mut target = Nv12Frame::new(size);
            BackgroundBlur::new(size, strength).apply(&source, &mut target).unwrap();
            target.luma()[128 * 512..129 * 512]
                .iter()
                .filter(|&&v| v > 20 && v < 230)
                .count()
        };
        assert!(spread(0.8) > spread(0.2));
        let steps: Vec<usize> = [0.0, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9, 1.0].iter().map(|&s| spread(s)).collect();
        assert!(steps.windows(2).all(|w| w[1] >= w[0]), "{steps:?}");
    }

    #[test]
    fn tiny_frames_do_not_panic() {
        let size = FrameSize::new(2, 2).unwrap();
        let source = Nv12Frame::filled(size, Yuv { y: 50, u: 1, v: 2 });
        let mut target = Nv12Frame::new(size);
        BackgroundBlur::new(size, 1.0).apply(&source, &mut target).unwrap();
        assert_eq!(target, source);
    }
}
