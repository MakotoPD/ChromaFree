#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const GREEN_SCREEN: Rgb = Rgb::new(0, 177, 64);
    pub const BLUE_SCREEN: Rgb = Rgb::new(0, 71, 187);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Yuv {
    pub y: u8,
    pub u: u8,
    pub v: u8,
}

impl Yuv {
    pub const BLACK: Yuv = Yuv { y: 16, u: 128, v: 128 };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorMatrix {
    Bt601,
    Bt709,
}

struct ForwardCoefficients {
    y: [i32; 3],
    u: [i32; 3],
    v: [i32; 3],
}

#[derive(Clone, Copy)]
pub struct InverseCoefficients {
    pub red_v: i32,
    pub green_u: i32,
    pub green_v: i32,
    pub blue_u: i32,
}

impl ColorMatrix {
    pub fn for_height(height: u32) -> Self {
        if height >= 720 { Self::Bt709 } else { Self::Bt601 }
    }

    fn forward(self) -> ForwardCoefficients {
        match self {
            Self::Bt601 => ForwardCoefficients {
                y: [66, 129, 25],
                u: [-38, -74, 112],
                v: [112, -94, -18],
            },
            Self::Bt709 => ForwardCoefficients {
                y: [47, 157, 16],
                u: [-26, -87, 112],
                v: [112, -102, -10],
            },
        }
    }

    pub fn inverse(self) -> InverseCoefficients {
        match self {
            Self::Bt601 => InverseCoefficients {
                red_v: 409,
                green_u: -100,
                green_v: -208,
                blue_u: 516,
            },
            Self::Bt709 => InverseCoefficients {
                red_v: 459,
                green_u: -55,
                green_v: -136,
                blue_u: 541,
            },
        }
    }

    pub fn to_yuv(self, rgb: Rgb) -> Yuv {
        let c = self.forward();
        let apply = |k: [i32; 3], offset: i32| {
            let value = (k[0] * i32::from(rgb.r) + k[1] * i32::from(rgb.g) + k[2] * i32::from(rgb.b) + 128) >> 8;
            clamp_u8(value + offset)
        };
        Yuv {
            y: apply(c.y, 16),
            u: apply(c.u, 128),
            v: apply(c.v, 128),
        }
    }

    pub fn to_rgb(self, yuv: Yuv) -> Rgb {
        let k = self.inverse();
        let c = 298 * (i32::from(yuv.y) - 16);
        let d = i32::from(yuv.u) - 128;
        let e = i32::from(yuv.v) - 128;
        Rgb {
            r: clamp_u8((c + k.red_v * e + 128) >> 8),
            g: clamp_u8((c + k.green_u * d + k.green_v * e + 128) >> 8),
            b: clamp_u8((c + k.blue_u * d + 128) >> 8),
        }
    }
}

#[inline]
pub fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_stays_within_rounding_error() {
        for matrix in [ColorMatrix::Bt601, ColorMatrix::Bt709] {
            for rgb in [
                Rgb::new(0, 0, 0),
                Rgb::new(255, 255, 255),
                Rgb::GREEN_SCREEN,
                Rgb::BLUE_SCREEN,
                Rgb::new(200, 30, 90),
            ] {
                let back = matrix.to_rgb(matrix.to_yuv(rgb));
                for (a, b) in [(rgb.r, back.r), (rgb.g, back.g), (rgb.b, back.b)] {
                    assert!(a.abs_diff(b) <= 3, "{matrix:?} {rgb:?} -> {back:?}");
                }
            }
        }
    }

    #[test]
    fn black_and_white_map_to_limited_range() {
        assert_eq!(ColorMatrix::Bt709.to_yuv(Rgb::new(0, 0, 0)), Yuv::BLACK);
        assert_eq!(ColorMatrix::Bt601.to_yuv(Rgb::new(255, 255, 255)).y, 235);
        assert_eq!(ColorMatrix::for_height(720), ColorMatrix::Bt709);
        assert_eq!(ColorMatrix::for_height(480), ColorMatrix::Bt601);
    }
}
