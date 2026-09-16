use std::hint::black_box;

use bgcam_core::{
    Background, BackgroundBlur, BgraFrame, ColorMatrix, FrameSize, MaskParams, MaskRefiner, ModelInputBuilder,
    Nv12Frame, Nv12Scaler, Orientation, Rgb, Rotation, ScaleMode, TensorLayout, Yuv, blend_nv12, nv12_to_bgra,
};
use criterion::{Criterion, criterion_group, criterion_main};
use half::f16;

fn size(width: u32, height: u32) -> FrameSize {
    FrameSize::new(width, height).unwrap()
}

fn textured_frame(frame_size: FrameSize) -> Nv12Frame {
    let width = frame_size.width() as usize;
    let luma = (0..frame_size.luma_len())
        .map(|i| (16 + ((i % width) ^ (i / width)) % 220) as u8)
        .collect();
    let chroma = (0..frame_size.chroma_len()).map(|i| (64 + i % 128) as u8).collect();
    Nv12Frame::from_planes(frame_size, luma, chroma).unwrap()
}

const RESOLUTIONS: [(u32, u32); 2] = [(1280, 720), (1920, 1080)];

fn orientation(c: &mut Criterion) {
    let mut group = c.benchmark_group("orientation");
    for (w, h) in RESOLUTIONS {
        let source = textured_frame(size(w, h));
        for (name, orientation) in [
            (
                "mirror",
                Orientation {
                    mirror_horizontal: true,
                    ..Default::default()
                },
            ),
            (
                "rotate90",
                Orientation {
                    rotation: Rotation::Clockwise90,
                    ..Default::default()
                },
            ),
        ] {
            let mut target = Nv12Frame::new(orientation.output_size(source.size()));
            group.bench_function(format!("{name}_{w}x{h}"), |b| {
                b.iter(|| orientation.apply(black_box(&source), &mut target).unwrap())
            });
        }
    }
    group.finish();
}

fn scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale");
    for ((sw, sh), (tw, th)) in [
        ((1920, 1080), (1280, 720)),
        ((1280, 720), (1920, 1080)),
        ((1280, 960), (1280, 720)),
    ] {
        let source = textured_frame(size(sw, sh));
        let mut scaler = Nv12Scaler::new(source.size(), size(tw, th), ScaleMode::Fill);
        let mut target = Nv12Frame::new(size(tw, th));
        group.bench_function(format!("{sw}x{sh}_to_{tw}x{th}"), |b| {
            b.iter(|| scaler.scale(black_box(&source), &mut target).unwrap())
        });
    }
    group.finish();
}

fn model_input(c: &mut Criterion) {
    let mut group = c.benchmark_group("model_input");
    let source = textured_frame(size(1280, 720));
    for (mw, mh) in [(640, 360), (1280, 720)] {
        let mut builder =
            ModelInputBuilder::<f16>::new(source.size(), size(mw, mh), TensorLayout::Nchw, ColorMatrix::Bt709);
        group.bench_function(format!("rvm_f16_720p_to_{mw}x{mh}"), |b| {
            b.iter(|| {
                black_box(builder.build(black_box(&source)).unwrap());
            })
        });
    }
    let mut mediapipe =
        ModelInputBuilder::<f32>::new(source.size(), size(256, 144), TensorLayout::Nhwc, ColorMatrix::Bt709);
    group.bench_function("mediapipe_f32_720p_to_256x144", |b| {
        b.iter(|| {
            black_box(mediapipe.build(black_box(&source)).unwrap());
        })
    });
    group.finish();
}

fn mask(c: &mut Criterion) {
    let mut group = c.benchmark_group("mask_refine");
    for ((mw, mh), (ow, oh), params, label) in [
        ((640, 360), (1280, 720), MaskParams::RECURRENT_MODEL, "rvm"),
        ((1280, 720), (1280, 720), MaskParams::RECURRENT_MODEL, "rvm"),
        ((256, 144), (1280, 720), MaskParams::MEMORYLESS_MODEL, "mediapipe"),
    ] {
        let alpha: Vec<f16> = (0..mw * mh).map(|i| f16::from_f32((i % 97) as f32 / 96.0)).collect();
        let mut refiner = MaskRefiner::new(mw, mh, size(ow, oh), params);
        group.bench_function(format!("{label}_{mw}x{mh}_to_{ow}x{oh}"), |b| {
            b.iter(|| {
                black_box(refiner.refine(black_box(&alpha)).unwrap().luma.len());
            })
        });
    }
    group.finish();
}

fn blur(c: &mut Criterion) {
    let mut group = c.benchmark_group("blur");
    for (w, h) in RESOLUTIONS {
        let source = textured_frame(size(w, h));
        for strength in [0.0f32, 0.5, 1.0] {
            let mut blur = BackgroundBlur::new(source.size(), strength);
            let mut target = Nv12Frame::new(source.size());
            group.bench_function(format!("strength{:.0}_{w}x{h}", strength * 100.0), |b| {
                b.iter(|| blur.apply(black_box(&source), &mut target).unwrap())
            });
        }
    }
    group.finish();
}

fn composite(c: &mut Criterion) {
    let mut group = c.benchmark_group("composite");
    for (w, h) in RESOLUTIONS {
        let frame_size = size(w, h);
        let foreground = textured_frame(frame_size);
        let background = Nv12Frame::filled(frame_size, Yuv { y: 90, u: 100, v: 150 });
        let luma_mask: Vec<u8> = (0..frame_size.luma_len()).map(|i| (i % 256) as u8).collect();
        let chroma_mask: Vec<u8> = (0..frame_size.chroma_len() / 2).map(|i| (i % 256) as u8).collect();
        let mut target = Nv12Frame::new(frame_size);
        let green = ColorMatrix::Bt709.to_yuv(Rgb::GREEN_SCREEN);
        group.bench_function(format!("solid_{w}x{h}"), |b| {
            b.iter(|| {
                blend_nv12(
                    &foreground,
                    Background::Solid(green),
                    &luma_mask,
                    &chroma_mask,
                    &mut target,
                )
                .unwrap()
            })
        });
        group.bench_function(format!("frame_{w}x{h}"), |b| {
            b.iter(|| {
                blend_nv12(
                    &foreground,
                    Background::Frame(&background),
                    &luma_mask,
                    &chroma_mask,
                    &mut target,
                )
                .unwrap()
            })
        });
        let mut bgra = BgraFrame::new(frame_size);
        group.bench_function(format!("nv12_to_bgra_alpha_{w}x{h}"), |b| {
            b.iter(|| nv12_to_bgra(&foreground, Some(&luma_mask), ColorMatrix::Bt709, &mut bgra).unwrap())
        });
    }
    group.finish();
}

criterion_group!(benches, orientation, scaling, model_input, mask, blur, composite);
criterion_main!(benches);
