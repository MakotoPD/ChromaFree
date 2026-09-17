use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chromafree_core::segmentation::AlphaMask;
use chromafree_core::{
    BackgroundEffect, ColorMatrix, CoreError, CpuPipeline, FrameSize, MaskParams, ModelProvider, Nv12Frame,
    OnnxModelProvider, Orientation, OutputFormat, PipelineOutput, PipelineSettings, Rgb, RgbImage, Rotation,
    SegmentationModel, SegmentationSettings, Yuv, nv12_to_bgra,
};

const MASK_WIDTH: u32 = 64;
const MASK_HEIGHT: u32 = 36;

struct EllipseModel {
    alpha: Vec<u8>,
    segments: Arc<AtomicUsize>,
}

impl SegmentationModel for EllipseModel {
    fn name(&self) -> &str {
        "ellipse"
    }

    fn mask_size(&self) -> FrameSize {
        FrameSize::new(MASK_WIDTH, MASK_HEIGHT).unwrap()
    }

    fn default_mask_params(&self) -> MaskParams {
        MaskParams::RECURRENT_MODEL
    }

    fn segment(&mut self, _frame: &Nv12Frame, _matrix: ColorMatrix) -> Result<AlphaMask<'_>, CoreError> {
        self.segments.fetch_add(1, Ordering::SeqCst);
        Ok(AlphaMask {
            width: MASK_WIDTH,
            height: MASK_HEIGHT,
            data: &self.alpha,
        })
    }

    fn reset(&mut self) {}
}

#[derive(Clone, Default)]
struct Counters {
    loads: Arc<AtomicUsize>,
    segments: Arc<AtomicUsize>,
}

struct EllipseProvider(Counters);

impl ModelProvider for EllipseProvider {
    fn load(
        &self,
        _settings: &SegmentationSettings,
        _frame: FrameSize,
    ) -> Result<Box<dyn SegmentationModel>, CoreError> {
        self.0.loads.fetch_add(1, Ordering::SeqCst);
        let alpha = (0..MASK_HEIGHT)
            .flat_map(|y| {
                (0..MASK_WIDTH).map(move |x| {
                    let dx = (x as f32 + 0.5) / MASK_WIDTH as f32 - 0.5;
                    let dy = (y as f32 + 0.5) / MASK_HEIGHT as f32 - 0.5;
                    if dx * dx / 0.04 + dy * dy / 0.09 < 1.0 { 255 } else { 0 }
                })
            })
            .collect();
        Ok(Box::new(EllipseModel {
            alpha,
            segments: Arc::clone(&self.0.segments),
        }))
    }
}

fn size(width: u32, height: u32) -> FrameSize {
    FrameSize::new(width, height).unwrap()
}

fn camera_frame() -> Nv12Frame {
    Nv12Frame::filled(size(1920, 1080), Yuv { y: 200, u: 90, v: 170 })
}

fn pipeline(effect: BackgroundEffect) -> (CpuPipeline, Counters) {
    let counters = Counters::default();
    let mut settings = PipelineSettings::new(size(1280, 720));
    settings.effect = effect;
    settings.color_matrix = Some(ColorMatrix::Bt709);
    (
        CpuPipeline::new(Box::new(EllipseProvider(counters.clone())), settings),
        counters,
    )
}

fn nv12(output: PipelineOutput<'_>) -> Nv12Frame {
    match output {
        PipelineOutput::Nv12(frame) => frame.clone(),
        PipelineOutput::Bgra(_) => panic!("expected NV12 output"),
    }
}

fn luma_at(frame: &Nv12Frame, x: u32, y: u32) -> u8 {
    frame.luma()[(y * frame.size().width() + x) as usize]
}

#[test]
fn passthrough_scales_to_output_and_never_loads_a_model() {
    let (mut pipeline, counters) = pipeline(BackgroundEffect::Passthrough);
    let out = nv12(pipeline.process(&camera_frame(), OutputFormat::Nv12).unwrap());
    assert_eq!(out, Nv12Frame::filled(size(1280, 720), Yuv { y: 200, u: 90, v: 170 }));
    assert_eq!(counters.loads.load(Ordering::SeqCst), 0);
}

#[test]
fn colour_background_keeps_the_person_and_replaces_the_rest() {
    let (mut pipeline, counters) = pipeline(BackgroundEffect::Color(Rgb::GREEN_SCREEN));
    let out = nv12(pipeline.process(&camera_frame(), OutputFormat::Nv12).unwrap());
    let green = ColorMatrix::Bt709.to_yuv(Rgb::GREEN_SCREEN);
    assert_eq!(luma_at(&out, 640, 360), 200);
    assert_eq!(luma_at(&out, 10, 10), green.y);
    assert_eq!(&out.chroma()[..2], &[green.u, green.v]);
    pipeline.process(&camera_frame(), OutputFormat::Nv12).unwrap();
    assert_eq!(counters.loads.load(Ordering::SeqCst), 1);
    assert_eq!(counters.segments.load(Ordering::SeqCst), 2);
}

#[test]
fn transparent_effect_carries_the_mask_in_bgra_and_falls_back_to_colour_in_nv12() {
    let (mut pipeline, _) = pipeline(BackgroundEffect::Transparent {
        fallback: Rgb::BLUE_SCREEN,
    });
    match pipeline.process(&camera_frame(), OutputFormat::Bgra).unwrap() {
        PipelineOutput::Bgra(bgra) => {
            let alpha = |x: usize, y: usize| bgra.data()[(y * 1280 + x) * 4 + 3];
            assert_eq!(alpha(640, 360), 255);
            assert_eq!(alpha(5, 5), 0);
        }
        PipelineOutput::Nv12(_) => panic!("expected BGRA output"),
    }
    let out = nv12(pipeline.process(&camera_frame(), OutputFormat::Nv12).unwrap());
    assert_eq!(luma_at(&out, 5, 5), ColorMatrix::Bt709.to_yuv(Rgb::BLUE_SCREEN).y);
}

#[test]
fn image_background_is_prepared_once_and_used_behind_the_person() {
    let image = Arc::new(RgbImage::from_pixels(4, 4, [255u8, 0, 0].repeat(16)).unwrap());
    let (mut pipeline, _) = pipeline(BackgroundEffect::Image(Arc::clone(&image)));
    let out = nv12(pipeline.process(&camera_frame(), OutputFormat::Nv12).unwrap());
    assert_eq!(luma_at(&out, 3, 3), ColorMatrix::Bt709.to_yuv(Rgb::new(255, 0, 0)).y);
    assert_eq!(luma_at(&out, 640, 360), 200);
}

#[test]
fn blur_changes_only_the_background() {
    let mut frame = camera_frame();
    for (i, value) in frame.planes_mut().0.iter_mut().enumerate() {
        *value = if (i % 1920 / 40 + i / 1920 / 40) % 2 == 0 {
            40
        } else {
            220
        };
    }
    let (mut blurred_pipeline, _) = pipeline(BackgroundEffect::Blur { strength: 1.0 });
    let blurred = nv12(blurred_pipeline.process(&frame, OutputFormat::Nv12).unwrap());
    let (mut plain_pipeline, _) = pipeline(BackgroundEffect::Passthrough);
    let plain = nv12(plain_pipeline.process(&frame, OutputFormat::Nv12).unwrap());
    assert_eq!(luma_at(&blurred, 640, 360), luma_at(&plain, 640, 360));
    let background_changes = (0..100)
        .filter(|&x| luma_at(&blurred, x, 20) != luma_at(&plain, x, 20))
        .count();
    assert!(background_changes > 50, "{background_changes}");
}

#[test]
fn orientation_and_mask_tuning_do_not_reload_the_model_but_output_size_does() {
    let (mut pipeline, counters) = pipeline(BackgroundEffect::Color(Rgb::GREEN_SCREEN));
    pipeline.process(&camera_frame(), OutputFormat::Nv12).unwrap();
    let mut settings = pipeline.settings().clone();
    settings.orientation = Orientation {
        rotation: Rotation::Clockwise90,
        mirror_horizontal: true,
        mirror_vertical: false,
    };
    settings.segmentation.mask = Some(MaskParams::MEMORYLESS_MODEL);
    pipeline.apply_settings(settings.clone());
    pipeline.process(&camera_frame(), OutputFormat::Nv12).unwrap();
    assert_eq!(counters.loads.load(Ordering::SeqCst), 1);
    settings.output = size(640, 480);
    pipeline.apply_settings(settings);
    let out = nv12(pipeline.process(&camera_frame(), OutputFormat::Nv12).unwrap());
    assert_eq!(out.size(), size(640, 480));
    assert_eq!(counters.loads.load(Ordering::SeqCst), 2);
}

#[test]
fn missing_models_are_reported_instead_of_panicking() {
    let settings = PipelineSettings::new(size(1280, 720));
    let mut pipeline = CpuPipeline::new(
        Box::new(OnnxModelProvider::new(
            std::env::temp_dir().join("chromafree-no-models"),
        )),
        settings,
    );
    assert!(matches!(
        pipeline.process(&camera_frame(), OutputFormat::Nv12),
        Err(CoreError::ModelNotFound(_))
    ));
}

#[test]
#[ignore = "requires models, a DirectX 12 GPU and CHROMAFREE_PERSON_IMAGE"]
fn full_cpu_pipeline_on_a_person_photo() {
    let models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models");
    let photo_path = std::env::var_os("CHROMAFREE_PERSON_IMAGE").expect("set CHROMAFREE_PERSON_IMAGE");
    let photo = RgbImage::load(Path::new(&photo_path))
        .unwrap()
        .to_nv12(size(1920, 1080), ColorMatrix::Bt709)
        .unwrap();
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pipeline");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut settings = PipelineSettings::new(size(1280, 720));
    for (name, effect) in [
        ("green", BackgroundEffect::Color(Rgb::GREEN_SCREEN)),
        ("blur", BackgroundEffect::Blur { strength: 0.6 }),
    ] {
        settings.effect = effect;
        let mut pipeline = CpuPipeline::new(Box::new(OnnxModelProvider::new(&models)), settings.clone());
        let mut last = None;
        for _ in 0..8 {
            last = Some(nv12(pipeline.process(&photo, OutputFormat::Nv12).unwrap()));
        }
        let frame = last.unwrap();
        let mut bgra = chromafree_core::BgraFrame::new(frame.size());
        nv12_to_bgra(&frame, None, settings.matrix(), &mut bgra).unwrap();
        let rgba = bgra
            .data()
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        image::RgbaImage::from_raw(1280, 720, rgba)
            .unwrap()
            .save(out_dir.join(format!("{name}.png")))
            .unwrap();
        println!("{name}: {}", pipeline.active_model().unwrap_or("none"));
    }
}
