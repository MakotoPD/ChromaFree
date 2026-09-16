use std::path::{Path, PathBuf};
use std::sync::Arc;

use bgcam_core::segmentation::AlphaMask;
use bgcam_core::{
    BackgroundEffect, ColorMatrix, CoreError, CpuPipeline, FrameSize, MaskParams, ModelProvider, Nv12Frame,
    Orientation, OutputFormat, PipelineOutput, PipelineSettings, Rgb, RgbImage, Rotation, SegmentationModel,
    SegmentationSettings,
};
use bgcam_gpu::GpuPipeline;

const MASK: (u32, u32) = (160, 90);

fn ellipse_alpha() -> Vec<u8> {
    (0..MASK.1)
        .flat_map(|y| {
            (0..MASK.0).map(move |x| {
                let dx = (x as f32 + 0.5) / MASK.0 as f32 - 0.5;
                let dy = (y as f32 + 0.5) / MASK.1 as f32 - 0.55;
                let distance = (dx * dx / 0.05 + dy * dy / 0.12).sqrt();
                ((1.0 - (distance - 0.85) / 0.3).clamp(0.0, 1.0) * 255.0).round() as u8
            })
        })
        .collect()
}

struct FixedModel(Vec<u8>);

impl SegmentationModel for FixedModel {
    fn name(&self) -> &str {
        "fixed"
    }

    fn mask_size(&self) -> FrameSize {
        FrameSize::new(MASK.0, MASK.1).unwrap()
    }

    fn default_mask_params(&self) -> MaskParams {
        MaskParams::RECURRENT_MODEL
    }

    fn segment(&mut self, _: &Nv12Frame, _: ColorMatrix) -> Result<AlphaMask<'_>, CoreError> {
        Ok(AlphaMask {
            width: MASK.0,
            height: MASK.1,
            data: &self.0,
        })
    }

    fn reset(&mut self) {}
}

struct FixedProvider;

impl ModelProvider for FixedProvider {
    fn load(&self, _: &SegmentationSettings, _: FrameSize) -> Result<Box<dyn SegmentationModel>, CoreError> {
        Ok(Box::new(FixedModel(ellipse_alpha())))
    }
}

fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")
}

fn scene(width: u32, height: u32) -> Nv12Frame {
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).flat_map(move |x| {
                let checker = if (x / 64 + y / 64) % 2 == 0 { 60 } else { 0 };
                [(x * 255 / width) as u8, (y * 255 / height) as u8, 120 + checker]
            })
        })
        .collect();
    RgbImage::from_pixels(width, height, pixels)
        .unwrap()
        .to_nv12(FrameSize::new(width, height).unwrap(), ColorMatrix::Bt709)
        .unwrap()
}

fn bytes(output: PipelineOutput<'_>) -> Vec<u8> {
    match output {
        PipelineOutput::Nv12(frame) => frame.luma().iter().chain(frame.chroma()).copied().collect(),
        PipelineOutput::Bgra(frame) => frame.data().to_vec(),
    }
}

struct Difference {
    mean: f64,
    max: u8,
    above_8: f64,
}

fn difference(a: &[u8], b: &[u8]) -> Difference {
    assert_eq!(a.len(), b.len());
    let diffs: Vec<u8> = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).collect();
    Difference {
        mean: diffs.iter().map(|&d| f64::from(d)).sum::<f64>() / diffs.len() as f64,
        max: diffs.iter().copied().max().unwrap_or(0),
        above_8: diffs.iter().filter(|&&d| d > 8).count() as f64 / diffs.len() as f64 * 100.0,
    }
}

#[test]
#[ignore = "requires a DirectX 12 GPU"]
fn gpu_compositing_matches_cpu_for_every_effect() {
    let camera = scene(1920, 1080);
    let alpha = ellipse_alpha();
    let mask = AlphaMask {
        width: MASK.0,
        height: MASK.1,
        data: &alpha,
    };
    let image = Arc::new(RgbImage::from_pixels(8, 6, [30u8, 200, 90, 220, 40, 60].repeat(24)).unwrap());
    let rotated = Orientation {
        rotation: Rotation::Clockwise90,
        mirror_horizontal: true,
        mirror_vertical: false,
    };
    let cases = [
        (
            "passthrough",
            BackgroundEffect::Passthrough,
            Orientation::default(),
            OutputFormat::Nv12,
            1.0,
            1.0,
        ),
        (
            "green",
            BackgroundEffect::Color(Rgb::GREEN_SCREEN),
            Orientation::default(),
            OutputFormat::Nv12,
            1.0,
            1.0,
        ),
        (
            "image",
            BackgroundEffect::Image(image),
            Orientation::default(),
            OutputFormat::Nv12,
            1.0,
            1.0,
        ),
        (
            "blur",
            BackgroundEffect::Blur { strength: 0.5 },
            Orientation::default(),
            OutputFormat::Nv12,
            3.0,
            10.0,
        ),
        (
            "transparent",
            BackgroundEffect::Transparent {
                fallback: Rgb::BLUE_SCREEN,
            },
            Orientation::default(),
            OutputFormat::Bgra,
            1.0,
            1.0,
        ),
        (
            "rotated green",
            BackgroundEffect::Color(Rgb::GREEN_SCREEN),
            rotated,
            OutputFormat::Nv12,
            1.0,
            1.0,
        ),
    ];
    let mut gpu = GpuPipeline::new(
        0,
        models_dir(),
        PipelineSettings::new(FrameSize::new(1280, 720).unwrap()),
    )
    .unwrap();
    println!("adapter: {}", gpu.adapter_name());
    for (name, effect, orientation, format, max_mean, max_above_8_percent) in cases {
        let mut settings = PipelineSettings::new(FrameSize::new(1280, 720).unwrap());
        settings.effect = effect;
        settings.orientation = orientation;
        settings.color_matrix = Some(ColorMatrix::Bt709);
        gpu.apply_settings(settings.clone()).unwrap();
        let gpu_bytes = bytes(gpu.process_with_alpha(&camera, &mask, format).unwrap());
        let mut cpu = CpuPipeline::new(Box::new(FixedProvider), settings);
        let cpu_bytes = bytes(cpu.process(&camera, format).unwrap());
        let dump = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("gpu_vs_cpu");
        std::fs::create_dir_all(&dump).unwrap();
        std::fs::write(dump.join(format!("{name}.gpu.raw")), &gpu_bytes).unwrap();
        std::fs::write(dump.join(format!("{name}.cpu.raw")), &cpu_bytes).unwrap();
        let d = difference(&gpu_bytes, &cpu_bytes);
        println!(
            "{name}: mean |diff| {:.3}, max {}, > 8: {:.3}%",
            d.mean, d.max, d.above_8
        );
        assert!(d.mean <= max_mean, "{name}: mean difference {:.3}", d.mean);
        assert!(
            d.above_8 <= max_above_8_percent,
            "{name}: {:.3}% of bytes differ by more than 8",
            d.above_8
        );
    }
}

#[test]
#[ignore = "requires models, a DirectX 12 GPU and BGCAM_PERSON_IMAGE"]
fn gpu_rvm_and_mediapipe_cut_out_the_person_photo() {
    let photo_path = std::env::var_os("BGCAM_PERSON_IMAGE").expect("set BGCAM_PERSON_IMAGE");
    let photo = RgbImage::load(Path::new(&photo_path))
        .unwrap()
        .to_nv12(FrameSize::new(1920, 1080).unwrap(), ColorMatrix::Bt709)
        .unwrap();
    let mut settings = PipelineSettings::new(FrameSize::new(1280, 720).unwrap());
    settings.effect = BackgroundEffect::Transparent {
        fallback: Rgb::GREEN_SCREEN,
    };
    let mut gpu = GpuPipeline::new(0, models_dir(), settings.clone()).unwrap();
    for method in [
        bgcam_core::SegmentationMethod::Rvm,
        bgcam_core::SegmentationMethod::MediaPipe,
    ] {
        settings.segmentation.method = method;
        gpu.apply_settings(settings.clone()).unwrap();
        let mut alpha = Vec::new();
        for _ in 0..10 {
            alpha = match gpu.process(&photo, OutputFormat::Bgra).unwrap() {
                PipelineOutput::Bgra(frame) => frame.data().chunks_exact(4).map(|p| p[3]).collect(),
                PipelineOutput::Nv12(_) => panic!("expected BGRA"),
            };
        }
        let mean = |x0: f32, y0: f32, x1: f32, y1: f32| {
            let mut sum = 0u64;
            let mut count = 0u64;
            for y in (y0 * 720.0) as usize..(y1 * 720.0) as usize {
                for x in (x0 * 1280.0) as usize..(x1 * 1280.0) as usize {
                    sum += u64::from(alpha[y * 1280 + x]);
                    count += 1;
                }
            }
            sum as f64 / count as f64
        };
        let face = mean(0.45, 0.30, 0.53, 0.50);
        let wall = mean(0.30, 0.02, 0.45, 0.14);
        println!(
            "{}: face {face:.0}, wall {wall:.0}",
            gpu.active_model().unwrap_or("none")
        );
        assert!(face > 220.0, "face alpha {face}");
        assert!(wall < 30.0, "wall alpha {wall}");
    }
}
