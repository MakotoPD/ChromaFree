use std::hint::black_box;

use chromafree_core::segmentation::AlphaMask;
use chromafree_core::{
    BackgroundEffect, ColorMatrix, CoreError, CpuPipeline, FrameSize, MaskParams, ModelProvider, Nv12Frame,
    OutputFormat, PipelineSettings, Rgb, SegmentationModel, SegmentationSettings,
};
use criterion::{Criterion, criterion_group, criterion_main};

struct FixedMask {
    size: FrameSize,
    alpha: Vec<u8>,
}

impl SegmentationModel for FixedMask {
    fn name(&self) -> &str {
        "fixed mask"
    }

    fn mask_size(&self) -> FrameSize {
        self.size
    }

    fn default_mask_params(&self) -> MaskParams {
        MaskParams::RECURRENT_MODEL
    }

    fn segment(&mut self, _frame: &Nv12Frame, _matrix: ColorMatrix) -> Result<AlphaMask<'_>, CoreError> {
        Ok(AlphaMask {
            width: self.size.width(),
            height: self.size.height(),
            data: &self.alpha,
        })
    }

    fn reset(&mut self) {}
}

struct FixedMaskProvider;

impl ModelProvider for FixedMaskProvider {
    fn load(
        &self,
        _settings: &SegmentationSettings,
        frame: FrameSize,
    ) -> Result<Box<dyn SegmentationModel>, CoreError> {
        let size = FrameSize::new(frame.width() / 2, frame.height() / 2)?;
        let alpha = (0..size.luma_len()).map(|i| (i % 256) as u8).collect();
        Ok(Box::new(FixedMask { size, alpha }))
    }
}

fn camera_frame() -> Nv12Frame {
    let size = FrameSize::new(1920, 1080).unwrap();
    let luma = (0..size.luma_len())
        .map(|i| (16 + ((i % 1920) ^ (i / 1920)) % 220) as u8)
        .collect();
    let chroma = (0..size.chroma_len()).map(|i| (64 + i % 128) as u8).collect();
    Nv12Frame::from_planes(size, luma, chroma).unwrap()
}

fn pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("cpu_pipeline_1080p_camera_to_720p_without_inference");
    let camera = camera_frame();
    for (name, effect, format) in [
        ("passthrough_nv12", BackgroundEffect::Passthrough, OutputFormat::Nv12),
        (
            "green_nv12",
            BackgroundEffect::Color(Rgb::GREEN_SCREEN),
            OutputFormat::Nv12,
        ),
        (
            "blur50_nv12",
            BackgroundEffect::Blur { strength: 0.5 },
            OutputFormat::Nv12,
        ),
        (
            "transparent_bgra",
            BackgroundEffect::Transparent {
                fallback: Rgb::GREEN_SCREEN,
            },
            OutputFormat::Bgra,
        ),
    ] {
        let mut settings = PipelineSettings::new(FrameSize::new(1280, 720).unwrap());
        settings.effect = effect;
        let mut pipeline = CpuPipeline::new(Box::new(FixedMaskProvider), settings);
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(pipeline.process(black_box(&camera), format).is_ok());
            })
        });
    }
    group.finish();
}

criterion_group!(benches, pipeline);
criterion_main!(benches);
