use std::path::PathBuf;

use chromafree_core::{
    Background, BackgroundBlur, BgraFrame, ColorMatrix, FrameSize, MaskParams, MaskRefiner, Nv12Frame, Orientation,
    Rgb, RgbImage, Rotation, blend_nv12, nv12_to_bgra,
};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const MATRIX: ColorMatrix = ColorMatrix::Bt601;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden")
        .join(name)
}

fn size() -> FrameSize {
    FrameSize::new(WIDTH, HEIGHT).unwrap()
}

fn scene() -> Nv12Frame {
    let pixels = (0..HEIGHT)
        .flat_map(|y| {
            (0..WIDTH).flat_map(move |x| {
                let stripe = if (x / 10 + y / 10) % 2 == 0 { 40 } else { 0 };
                [(x * 255 / WIDTH) as u8, (y * 255 / HEIGHT) as u8, 160 + stripe]
            })
        })
        .collect();
    RgbImage::from_pixels(WIDTH, HEIGHT, pixels)
        .unwrap()
        .to_nv12(size(), MATRIX)
        .unwrap()
}

fn person_alpha(model_width: u32, model_height: u32) -> Vec<u8> {
    (0..model_height)
        .flat_map(|y| {
            (0..model_width).map(move |x| {
                let dx = (x as f32 + 0.5) / model_width as f32 - 0.5;
                let dy = (y as f32 + 0.5) / model_height as f32 - 0.75;
                let distance = (dx * dx / 0.06 + dy * dy / 0.35).sqrt();
                ((1.0 - (distance - 0.9) / 0.2).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            })
        })
        .collect()
}

fn background_image() -> RgbImage {
    let pixels = (0..48u32)
        .flat_map(|y| {
            (0..64u32).flat_map(move |x| {
                if (x / 8 + y / 8) % 2 == 0 {
                    [230, 180, 40]
                } else {
                    [30, 30, 90]
                }
            })
        })
        .collect();
    RgbImage::from_pixels(64, 48, pixels).unwrap()
}

fn to_rgba(frame: &Nv12Frame, alpha: Option<&[u8]>) -> Vec<u8> {
    let mut bgra = BgraFrame::new(frame.size());
    nv12_to_bgra(frame, alpha, MATRIX, &mut bgra).unwrap();
    bgra.data()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect()
}

fn assert_golden(name: &str, width: u32, height: u32, rgba: Vec<u8>) {
    let path = fixture_path(name);
    let actual = image::RgbaImage::from_raw(width, height, rgba).unwrap();
    if std::env::var("CHROMAFREE_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        actual.save(&path).unwrap();
        return;
    }
    let expected = image::open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "missing golden file {}: {e}; run with CHROMAFREE_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .into_rgba8();
    assert_eq!(expected.dimensions(), actual.dimensions(), "{name} dimensions");
    if expected.as_raw() != actual.as_raw() {
        let failed = path.with_extension("actual.png");
        actual.save(&failed).unwrap();
        panic!(
            "{name} differs from golden file, actual output written to {}",
            failed.display()
        );
    }
}

fn refined_mask() -> (Vec<u8>, Vec<u8>) {
    let (model_width, model_height) = (64, 36);
    let mut refiner = MaskRefiner::new(model_width, model_height, size(), MaskParams::RECURRENT_MODEL);
    let mask = refiner.refine(&person_alpha(model_width, model_height)).unwrap();
    (mask.luma.to_vec(), mask.chroma.to_vec())
}

#[test]
fn golden_mask() {
    let (luma, _) = refined_mask();
    let rgba = luma.iter().flat_map(|&v| [v, v, v, 255]).collect();
    assert_golden("mask.png", WIDTH, HEIGHT, rgba);
}

#[test]
fn golden_green_screen() {
    let (luma, chroma) = refined_mask();
    let mut out = Nv12Frame::new(size());
    let green = MATRIX.to_yuv(Rgb::GREEN_SCREEN);
    blend_nv12(&scene(), Background::Solid(green), &luma, &chroma, &mut out).unwrap();
    assert_golden("green_screen.png", WIDTH, HEIGHT, to_rgba(&out, None));
}

#[test]
fn golden_blur() {
    let (luma, chroma) = refined_mask();
    let source = scene();
    let mut blurred = Nv12Frame::new(size());
    BackgroundBlur::new(size(), 0.4).apply(&source, &mut blurred).unwrap();
    let mut out = Nv12Frame::new(size());
    blend_nv12(&source, Background::Frame(&blurred), &luma, &chroma, &mut out).unwrap();
    assert_golden("blur.png", WIDTH, HEIGHT, to_rgba(&out, None));
}

#[test]
fn golden_image_background() {
    let (luma, chroma) = refined_mask();
    let background = background_image().to_nv12(size(), MATRIX).unwrap();
    let mut out = Nv12Frame::new(size());
    blend_nv12(&scene(), Background::Frame(&background), &luma, &chroma, &mut out).unwrap();
    assert_golden("image_background.png", WIDTH, HEIGHT, to_rgba(&out, None));
}

#[test]
fn golden_transparent() {
    let (luma, _) = refined_mask();
    assert_golden("transparent.png", WIDTH, HEIGHT, to_rgba(&scene(), Some(&luma)));
}

#[test]
fn golden_rotated_mirrored() {
    let orientation = Orientation {
        rotation: Rotation::Clockwise90,
        mirror_horizontal: true,
        mirror_vertical: false,
    };
    let source = scene();
    let mut out = Nv12Frame::new(orientation.output_size(source.size()));
    orientation.apply(&source, &mut out).unwrap();
    assert_golden("rotated_mirrored.png", HEIGHT, WIDTH, to_rgba(&out, None));
}
