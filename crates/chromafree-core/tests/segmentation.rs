use std::path::PathBuf;
use std::time::Instant;

use chromafree_core::{
    ColorMatrix, FrameSize, InferenceDevice, MediaPipeModel, MediaPipeVariant, ModelVariant, Nv12Frame, RgbImage,
    RvmModel, SegmentationModel,
};

const MATRIX: ColorMatrix = ColorMatrix::Bt709;

fn models_dir() -> PathBuf {
    std::env::var_os("CHROMAFREE_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models"))
}

fn person_scene(width: u32, height: u32) -> Nv12Frame {
    let (w, h) = (width as f32, height as f32);
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).flat_map(move |x| {
                let (fx, fy) = (x as f32 / w, y as f32 / h);
                let head = ((fx - 0.5) / 0.09).powi(2) + ((fy - 0.38) / 0.17).powi(2) < 1.0;
                let body = ((fx - 0.5) / 0.26).powi(2) + ((fy - 1.05) / 0.5).powi(2) < 1.0;
                let hair = ((fx - 0.5) / 0.1).powi(2) + ((fy - 0.28) / 0.1).powi(2) < 1.0 && fy < 0.3;
                if hair {
                    [40, 28, 20]
                } else if head {
                    [214, 170, 140]
                } else if body {
                    [45, 60, 110]
                } else {
                    [150 + (fx * 60.0) as u8, 165, 140 + (fy * 70.0) as u8]
                }
            })
        })
        .collect();
    RgbImage::from_pixels(width, height, pixels)
        .unwrap()
        .to_nv12(FrameSize::new(width, height).unwrap(), MATRIX)
        .unwrap()
}

fn mean(data: &[u8], width: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let height = data.len() as u32 / width;
    let (xa, xb) = ((x0 * width as f32) as u32, (x1 * width as f32) as u32);
    let (ya, yb) = ((y0 * height as f32) as u32, (y1 * height as f32) as u32);
    let mut sum = 0u64;
    let mut count = 0u64;
    for y in ya..yb {
        for x in xa..xb {
            sum += u64::from(data[(y * width + x) as usize]);
            count += 1;
        }
    }
    sum as f32 / count as f32
}

struct Run {
    masks: Vec<Vec<u8>>,
    median_ms: f64,
}

fn run_model(model: &mut dyn SegmentationModel, frame: &Nv12Frame, frames: usize) -> Run {
    let mut timings = Vec::with_capacity(frames);
    let mut masks = Vec::with_capacity(frames);
    let expected = model.mask_size();
    for _ in 0..frames {
        let start = Instant::now();
        let mask = model.segment(frame, MATRIX).unwrap();
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(mask.data.len(), (mask.width * mask.height) as usize);
        assert_eq!((mask.width, mask.height), (expected.width(), expected.height()));
        masks.push(mask.data.to_vec());
    }
    timings.sort_by(f64::total_cmp);
    Run {
        masks,
        median_ms: timings[timings.len() / 2],
    }
}

fn assert_finds_person(model: &mut dyn SegmentationModel, frames: usize) {
    let frame = person_scene(1280, 720);
    let run = run_model(model, &frame, frames);
    let last = run.masks.last().unwrap();
    let width = model.mask_size().width();
    let face = mean(last, width, 0.46, 0.34, 0.54, 0.42);
    let corner = mean(last, width, 0.0, 0.0, 0.15, 0.15);
    println!(
        "{}: face {face:.0}, corner {corner:.0}, median {:.2} ms",
        model.name(),
        run.median_ms
    );
    assert!(face > 128.0, "face region mean {face}");
    assert!(corner < 64.0, "background corner mean {corner}");
}

fn max_difference(a: &[u8], b: &[u8]) -> u8 {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

#[test]
#[ignore = "requires models from models/download.ps1 and a DirectX 12 GPU"]
fn rvm_directml_matches_cpu_and_resets_state() {
    let frame = person_scene(1280, 720);
    let variant = ModelVariant::new(640, 360);
    let mut cpu = RvmModel::load(&models_dir(), variant, InferenceDevice::Cpu { threads: 4 }).unwrap();
    let mut gpu = RvmModel::load(&models_dir(), variant, InferenceDevice::default()).unwrap();
    assert!(
        chromafree_core::segmentation::bundled_directml().is_some(),
        "DirectML.dll from ort was not found next to the test binary"
    );
    let cpu_run = run_model(&mut cpu, &frame, 6);
    let gpu_run = run_model(&mut gpu, &frame, 6);
    for (a, b) in cpu_run.masks.iter().zip(&gpu_run.masks) {
        assert!(
            max_difference(a, b) <= 8,
            "cpu and directml masks differ by {}",
            max_difference(a, b)
        );
    }
    gpu.reset();
    let after_reset = run_model(&mut gpu, &frame, 1);
    assert!(max_difference(&after_reset.masks[0], &gpu_run.masks[0]) <= 1);
    println!(
        "{}: cpu median {:.2} ms, directml median {:.2} ms",
        gpu.name(),
        cpu_run.median_ms,
        gpu_run.median_ms
    );
}

#[test]
#[ignore = "requires models from models/download.ps1 and a DirectX 12 GPU"]
fn every_downloaded_rvm_variant_loads_on_directml() {
    let variants = chromafree_core::available_rvm_variants(&models_dir());
    assert_eq!(variants.len(), 46, "run models/download.ps1");
    for variant in [
        ModelVariant::new(640, 360),
        ModelVariant::new(360, 640),
        ModelVariant::new(1280, 960),
    ] {
        let mut model = RvmModel::load(&models_dir(), variant, InferenceDevice::default()).unwrap();
        let frame = person_scene(1280, 720);
        run_model(&mut model, &frame, 2);
    }
}

#[test]
#[ignore = "requires models from models/download.ps1"]
fn mediapipe_on_cpu_finds_the_person() {
    let mut model = MediaPipeModel::load(
        &models_dir(),
        MediaPipeVariant::Landscape,
        InferenceDevice::Cpu { threads: 2 },
    )
    .unwrap();
    assert_finds_person(&mut model, 8);
}

#[test]
#[ignore = "requires models from models/download.ps1 and a DirectX 12 GPU"]
fn mediapipe_on_directml_finds_the_person() {
    let mut model =
        MediaPipeModel::load(&models_dir(), MediaPipeVariant::Landscape, InferenceDevice::default()).unwrap();
    assert_finds_person(&mut model, 60);
}

fn person_photo(width: u32, height: u32) -> Nv12Frame {
    let path = std::env::var_os("CHROMAFREE_PERSON_IMAGE")
        .expect("set CHROMAFREE_PERSON_IMAGE to a photo of a person at a desk");
    RgbImage::load(std::path::Path::new(&path))
        .unwrap()
        .to_nv12(FrameSize::new(width, height).unwrap(), MATRIX)
        .unwrap()
}

fn save_mask(name: &str, width: u32, height: u32, data: &[u8]) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("masks");
    std::fs::create_dir_all(&dir).unwrap();
    image::GrayImage::from_raw(width, height, data.to_vec())
        .unwrap()
        .save(dir.join(name))
        .unwrap();
}

fn assert_person_photo_mask(model: &mut dyn SegmentationModel, file_name: &str) {
    let frame = person_photo(1280, 720);
    let run = run_model(model, &frame, 12);
    let mask = run.masks.last().unwrap();
    let size = model.mask_size();
    save_mask(file_name, size.width(), size.height(), mask);
    let face = mean(mask, size.width(), 0.45, 0.30, 0.53, 0.50);
    let shirt = mean(mask, size.width(), 0.40, 0.65, 0.55, 0.80);
    let wall = mean(mask, size.width(), 0.30, 0.02, 0.45, 0.14);
    let wardrobe = mean(mask, size.width(), 0.68, 0.25, 0.80, 0.55);
    println!(
        "{}: face {face:.0}, shirt {shirt:.0}, wall {wall:.0}, wardrobe {wardrobe:.0}, median {:.2} ms",
        model.name(),
        run.median_ms
    );
    assert!(face > 220.0, "face region mean {face}");
    assert!(shirt > 220.0, "shirt region mean {shirt}");
    assert!(wall < 30.0, "wall region mean {wall}");
    assert!(wardrobe < 30.0, "wardrobe region mean {wardrobe}");
}

#[test]
#[ignore = "requires models, a DirectX 12 GPU and CHROMAFREE_PERSON_IMAGE"]
fn rvm_segments_a_person_photo() {
    for variant in [ModelVariant::new(640, 360), ModelVariant::new(1280, 720)] {
        let mut model = RvmModel::load(&models_dir(), variant, InferenceDevice::default()).unwrap();
        assert_person_photo_mask(&mut model, &format!("rvm_{}x{}.png", variant.width, variant.height));
    }
}

#[test]
#[ignore = "requires models, a DirectX 12 GPU and CHROMAFREE_PERSON_IMAGE"]
fn mediapipe_segments_a_person_photo() {
    let mut model =
        MediaPipeModel::load(&models_dir(), MediaPipeVariant::Landscape, InferenceDevice::default()).unwrap();
    assert_person_photo_mask(&mut model, "mediapipe_256x144.png");
}

#[test]
fn missing_model_file_is_reported() {
    let result = RvmModel::load(
        &std::env::temp_dir().join("chromafree-no-models"),
        ModelVariant::new(640, 360),
        InferenceDevice::Cpu { threads: 1 },
    );
    assert!(matches!(result, Err(chromafree_core::CoreError::ModelNotFound(_))));
}
