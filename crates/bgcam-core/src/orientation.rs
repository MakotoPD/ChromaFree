use crate::error::CoreError;
use crate::frame::{FrameSize, Nv12Frame};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rotation {
    #[default]
    None,
    Clockwise90,
    Clockwise180,
    Clockwise270,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Orientation {
    pub rotation: Rotation,
    pub mirror_horizontal: bool,
    pub mirror_vertical: bool,
}

struct AffineMap {
    x_from_out_x: i64,
    x_from_out_y: i64,
    x_offset: i64,
    y_from_out_x: i64,
    y_from_out_y: i64,
    y_offset: i64,
}

impl Orientation {
    pub fn is_identity(self) -> bool {
        self == Self::default()
    }

    pub fn output_size(self, input: FrameSize) -> FrameSize {
        match self.rotation {
            Rotation::Clockwise90 | Rotation::Clockwise270 => input.swapped(),
            Rotation::None | Rotation::Clockwise180 => input,
        }
    }

    pub fn apply(self, source: &Nv12Frame, destination: &mut Nv12Frame) -> Result<(), CoreError> {
        let input = source.size();
        self.output_size(input).ensure_eq(destination.size())?;
        if self.is_identity() {
            return destination.copy_from(source);
        }
        let (luma, chroma) = destination.planes_mut();
        self.transform_plane(source.luma(), input.width(), input.height(), 1, luma);
        self.transform_plane(source.chroma(), input.chroma_width(), input.chroma_height(), 2, chroma);
        Ok(())
    }

    fn source_coordinate(self, x: i64, y: i64, width: i64, height: i64) -> (i64, i64) {
        let (rx, ry) = match self.rotation {
            Rotation::None => (x, y),
            Rotation::Clockwise90 => (y, height - 1 - x),
            Rotation::Clockwise180 => (width - 1 - x, height - 1 - y),
            Rotation::Clockwise270 => (width - 1 - y, x),
        };
        let sx = if self.mirror_horizontal { width - 1 - rx } else { rx };
        let sy = if self.mirror_vertical { height - 1 - ry } else { ry };
        (sx, sy)
    }

    fn affine_map(self, width: i64, height: i64) -> AffineMap {
        let (x0, y0) = self.source_coordinate(0, 0, width, height);
        let (x1, y1) = self.source_coordinate(1, 0, width, height);
        let (x2, y2) = self.source_coordinate(0, 1, width, height);
        AffineMap {
            x_from_out_x: x1 - x0,
            x_from_out_y: x2 - x0,
            x_offset: x0,
            y_from_out_x: y1 - y0,
            y_from_out_y: y2 - y0,
            y_offset: y0,
        }
    }

    fn transform_plane(self, source: &[u8], width: u32, height: u32, bytes_per_pixel: usize, destination: &mut [u8]) {
        let (w, h) = (i64::from(width), i64::from(height));
        let out_width = match self.rotation {
            Rotation::Clockwise90 | Rotation::Clockwise270 => height as usize,
            Rotation::None | Rotation::Clockwise180 => width as usize,
        };
        let map = self.affine_map(w, h);
        let step = (map.x_from_out_x + map.y_from_out_x * w) as isize * bytes_per_pixel as isize;
        let row_advance = (map.x_from_out_y + map.y_from_out_y * w) as isize * bytes_per_pixel as isize;
        let mut row_start = (map.x_offset + map.y_offset * w) as isize * bytes_per_pixel as isize;
        for out_row in destination.chunks_exact_mut(out_width * bytes_per_pixel) {
            let mut index = row_start;
            for out_pixel in out_row.chunks_exact_mut(bytes_per_pixel) {
                let start = index as usize;
                out_pixel.copy_from_slice(&source[start..start + bytes_per_pixel]);
                index += step;
            }
            row_start += row_advance;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered_frame(width: u32, height: u32) -> Nv12Frame {
        let size = FrameSize::new(width, height).unwrap();
        let luma = (0..size.luma_len()).map(|i| i as u8).collect();
        let chroma = (0..size.chroma_len()).map(|i| 100 + i as u8).collect();
        Nv12Frame::from_planes(size, luma, chroma).unwrap()
    }

    fn apply(orientation: Orientation, source: &Nv12Frame) -> Nv12Frame {
        let mut out = Nv12Frame::new(orientation.output_size(source.size()));
        orientation.apply(source, &mut out).unwrap();
        out
    }

    fn rotation(rotation: Rotation) -> Orientation {
        Orientation {
            rotation,
            ..Default::default()
        }
    }

    #[test]
    fn rotate_clockwise_90_moves_bottom_left_to_top_left() {
        let source = numbered_frame(4, 2);
        let out = apply(rotation(Rotation::Clockwise90), &source);
        assert_eq!(out.size(), FrameSize::new(2, 4).unwrap());
        assert_eq!(out.luma(), &[4, 0, 5, 1, 6, 2, 7, 3]);
        assert_eq!(out.chroma(), &[100, 101, 102, 103]);
    }

    #[test]
    fn rotate_180_reverses_pixels_and_keeps_chroma_pairs() {
        let source = numbered_frame(4, 2);
        let out = apply(rotation(Rotation::Clockwise180), &source);
        assert_eq!(out.luma(), &[7, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(out.chroma(), &[102, 103, 100, 101]);
    }

    #[test]
    fn mirror_horizontal_reverses_each_row() {
        let source = numbered_frame(4, 2);
        let out = apply(
            Orientation {
                mirror_horizontal: true,
                ..Default::default()
            },
            &source,
        );
        assert_eq!(out.luma(), &[3, 2, 1, 0, 7, 6, 5, 4]);
    }

    #[test]
    fn four_quarter_turns_restore_the_original() {
        let source = numbered_frame(6, 4);
        let mut frame = source.clone();
        for _ in 0..4 {
            frame = apply(rotation(Rotation::Clockwise90), &frame);
        }
        assert_eq!(frame, source);
        let half = apply(rotation(Rotation::Clockwise180), &source);
        assert_eq!(apply(rotation(Rotation::Clockwise180), &half), source);
    }

    #[test]
    fn clockwise_270_inverts_clockwise_90() {
        let source = numbered_frame(6, 4);
        let turned = apply(rotation(Rotation::Clockwise90), &source);
        assert_eq!(apply(rotation(Rotation::Clockwise270), &turned), source);
    }

    #[test]
    fn mismatched_destination_is_rejected() {
        let source = numbered_frame(4, 2);
        let mut out = Nv12Frame::new(FrameSize::new(4, 2).unwrap());
        assert!(rotation(Rotation::Clockwise90).apply(&source, &mut out).is_err());
    }
}
