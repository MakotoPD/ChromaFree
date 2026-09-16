use std::time::{Duration, Instant};

use bgcam_core::{ColorMatrix, FrameSize, InferenceDevice, ModelVariant, Nv12Frame, RvmModel, SegmentationModel, Yuv};

fn main() {
    let dir = std::env::var_os("BGCAM_MODELS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models"));
    let variant = std::env::args()
        .nth(1)
        .and_then(|w| {
            let (a, b) = w.split_once('x')?;
            Some(ModelVariant::new(a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or(ModelVariant::new(640, 360));
    let mut model = RvmModel::load(&dir, variant, InferenceDevice::default()).unwrap();
    let frame = Nv12Frame::filled(FrameSize::new(1280, 720).unwrap(), Yuv { y: 120, u: 110, v: 140 });
    for _ in 0..30 {
        model.segment(&frame, ColorMatrix::Bt709).unwrap();
    }
    println!("READY");
    let period = Duration::from_secs(1) / 30;
    let start = Instant::now();
    let mut next = start;
    let mut t = Vec::new();
    while start.elapsed() < Duration::from_secs(10) {
        let s = Instant::now();
        model.segment(&frame, ColorMatrix::Bt709).unwrap();
        t.push(s.elapsed().as_secs_f64() * 1e3);
        next += period;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    t.sort_by(f64::total_cmp);
    println!(
        "{}x{} paced 30fps: frames {} median {:.2} ms p95 {:.2} ms",
        variant.width,
        variant.height,
        t.len(),
        t[t.len() / 2],
        t[t.len() * 95 / 100]
    );
}
