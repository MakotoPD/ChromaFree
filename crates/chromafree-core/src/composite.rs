use crate::color::{ColorMatrix, Yuv, clamp_u8};
use crate::error::CoreError;
use crate::frame::{BgraFrame, Nv12Frame};

#[derive(Clone, Copy)]
pub enum Background<'a> {
    Solid(Yuv),
    Frame(&'a Nv12Frame),
}

#[inline]
fn blend(foreground: u8, background: u8, alpha: u8) -> u8 {
    let a = u32::from(alpha);
    let value = u32::from(foreground) * a + u32::from(background) * (255 - a) + 128;
    ((value + (value >> 8)) >> 8) as u8
}

fn check_mask(expected: usize, actual: usize) -> Result<(), CoreError> {
    if expected == actual {
        Ok(())
    } else {
        Err(CoreError::PlaneSizeMismatch { expected, actual })
    }
}

pub fn blend_nv12(
    foreground: &Nv12Frame,
    background: Background<'_>,
    luma_mask: &[u8],
    chroma_mask: &[u8],
    target: &mut Nv12Frame,
) -> Result<(), CoreError> {
    let size = foreground.size();
    size.ensure_eq(target.size())?;
    check_mask(size.luma_len(), luma_mask.len())?;
    check_mask(size.chroma_len() / 2, chroma_mask.len())?;
    let (target_luma, target_chroma) = target.planes_mut();
    match background {
        Background::Solid(color) => {
            for ((out, &fg), &a) in target_luma.iter_mut().zip(foreground.luma()).zip(luma_mask) {
                *out = blend(fg, color.y, a);
            }
            for ((out, fg), &a) in target_chroma
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(foreground.chroma().as_chunks::<2>().0.iter())
                .zip(chroma_mask)
            {
                out[0] = blend(fg[0], color.u, a);
                out[1] = blend(fg[1], color.v, a);
            }
        }
        Background::Frame(frame) => {
            size.ensure_eq(frame.size())?;
            for (((out, &fg), &bg), &a) in target_luma
                .iter_mut()
                .zip(foreground.luma())
                .zip(frame.luma())
                .zip(luma_mask)
            {
                *out = blend(fg, bg, a);
            }
            for (((out, fg), bg), &a) in target_chroma
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(foreground.chroma().as_chunks::<2>().0.iter())
                .zip(frame.chroma().as_chunks::<2>().0.iter())
                .zip(chroma_mask)
            {
                out[0] = blend(fg[0], bg[0], a);
                out[1] = blend(fg[1], bg[1], a);
            }
        }
    }
    Ok(())
}

pub fn nv12_to_bgra(
    frame: &Nv12Frame,
    alpha: Option<&[u8]>,
    matrix: ColorMatrix,
    target: &mut BgraFrame,
) -> Result<(), CoreError> {
    let size = frame.size();
    size.ensure_eq(target.size())?;
    if let Some(mask) = alpha {
        check_mask(size.luma_len(), mask.len())?;
    }
    let width = size.width() as usize;
    let chroma_stride = size.chroma_width() as usize * 2;
    let k = matrix.inverse();
    for (y, (row, luma_row)) in target
        .data_mut()
        .chunks_exact_mut(width * 4)
        .zip(frame.luma().chunks_exact(width))
        .enumerate()
    {
        let chroma_row = &frame.chroma()[(y / 2) * chroma_stride..(y / 2 + 1) * chroma_stride];
        let alpha_row = alpha.map(|mask| &mask[y * width..(y + 1) * width]);
        for (x, (pixel, &luma)) in row.as_chunks_mut::<4>().0.iter_mut().zip(luma_row).enumerate() {
            let c = 298 * (i32::from(luma) - 16);
            let d = i32::from(chroma_row[x & !1]) - 128;
            let e = i32::from(chroma_row[(x & !1) + 1]) - 128;
            pixel[0] = clamp_u8((c + k.blue_u * d + 128) >> 8);
            pixel[1] = clamp_u8((c + k.green_u * d + k.green_v * e + 128) >> 8);
            pixel[2] = clamp_u8((c + k.red_v * e + 128) >> 8);
            pixel[3] = alpha_row.map_or(255, |a| a[x]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;
    use crate::frame::FrameSize;

    fn size() -> FrameSize {
        FrameSize::new(4, 2).unwrap()
    }

    #[test]
    fn blend_endpoints_are_exact() {
        for fg in [0u8, 17, 128, 255] {
            for bg in [0u8, 99, 255] {
                assert_eq!(blend(fg, bg, 255), fg);
                assert_eq!(blend(fg, bg, 0), bg);
            }
        }
        assert_eq!(blend(200, 100, 128), 150);
    }

    #[test]
    fn solid_background_replaces_masked_out_pixels() {
        let foreground = Nv12Frame::filled(size(), Yuv { y: 200, u: 10, v: 20 });
        let green = ColorMatrix::Bt709.to_yuv(Rgb::GREEN_SCREEN);
        let luma_mask = [255, 255, 0, 0, 255, 255, 0, 0];
        let chroma_mask = [255, 0];
        let mut target = Nv12Frame::new(size());
        blend_nv12(
            &foreground,
            Background::Solid(green),
            &luma_mask,
            &chroma_mask,
            &mut target,
        )
        .unwrap();
        assert_eq!(target.luma(), &[200, 200, green.y, green.y, 200, 200, green.y, green.y]);
        assert_eq!(target.chroma(), &[10, 20, green.u, green.v]);
    }

    #[test]
    fn frame_background_is_blended_per_pixel() {
        let foreground = Nv12Frame::filled(size(), Yuv { y: 235, u: 128, v: 128 });
        let background = Nv12Frame::filled(size(), Yuv { y: 16, u: 128, v: 128 });
        let mut target = Nv12Frame::new(size());
        blend_nv12(
            &foreground,
            Background::Frame(&background),
            &[128; 8],
            &[128; 2],
            &mut target,
        )
        .unwrap();
        assert!(target.luma().iter().all(|&v| v == 126));
    }

    #[test]
    fn bgra_conversion_carries_mask_as_alpha() {
        let matrix = ColorMatrix::Bt601;
        let frame = Nv12Frame::filled(size(), matrix.to_yuv(Rgb::new(250, 10, 10)));
        let mut target = BgraFrame::new(size());
        let alpha = [0, 64, 128, 255, 1, 2, 3, 4];
        nv12_to_bgra(&frame, Some(&alpha), matrix, &mut target).unwrap();
        let first = &target.data()[..4];
        assert!(first[2] > 240 && first[0] < 20 && first[1] < 20);
        let alphas: Vec<u8> = target.data().as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        assert_eq!(alphas, alpha);
        nv12_to_bgra(&frame, None, matrix, &mut target).unwrap();
        assert!(target.data().as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn mismatched_masks_are_rejected() {
        let frame = Nv12Frame::new(size());
        let mut target = Nv12Frame::new(size());
        assert!(blend_nv12(&frame, Background::Solid(Yuv::BLACK), &[0; 7], &[0; 2], &mut target).is_err());
    }
}
