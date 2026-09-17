use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chromafree_core::{BackgroundEffect, ColorMatrix, FrameSize, OutputFormat, PipelineSettings, Rgb, RgbImage};
use chromafree_gpu::GpuPipeline;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIFactory1,
};
use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::core::Interface;

fn memory_mb() -> (f64, f64, f64) {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    }
    .as_bool();
    let vram = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }
        .and_then(|f| unsafe { f.EnumAdapters1(0) })
        .and_then(|a| a.cast::<IDXGIAdapter3>())
        .and_then(|a| {
            let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            unsafe { a.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }.map(|_| info.CurrentUsage)
        })
        .unwrap_or(0);
    let mb = |v: usize| v as f64 / 1_048_576.0;
    if ok {
        (
            mb(counters.WorkingSetSize),
            mb(counters.PrivateUsage),
            vram as f64 / 1_048_576.0,
        )
    } else {
        (0.0, 0.0, vram as f64 / 1_048_576.0)
    }
}

fn process_cpu_time() -> Duration {
    let (mut creation, mut exit, mut kernel, mut user) = Default::default();
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) }.is_err() {
        return Duration::ZERO;
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let models = std::env::var_os("CHROMAFREE_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models"));
    let photo_path = std::env::var_os("CHROMAFREE_PERSON_IMAGE").ok_or("set CHROMAFREE_PERSON_IMAGE")?;
    let camera = RgbImage::load(Path::new(&photo_path))?.to_nv12(FrameSize::new(1920, 1080)?, ColorMatrix::Bt709)?;
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(8);

    let mut settings = PipelineSettings::new(FrameSize::new(1280, 720)?);
    let mut pipeline = GpuPipeline::new(0, &models, settings.clone())?;
    println!("adapter: {}", pipeline.adapter_name());
    let cases = [
        ("passthrough", BackgroundEffect::Passthrough, OutputFormat::Nv12),
        (
            "green screen",
            BackgroundEffect::Color(Rgb::GREEN_SCREEN),
            OutputFormat::Nv12,
        ),
        ("blur 50%", BackgroundEffect::Blur { strength: 0.5 }, OutputFormat::Nv12),
        (
            "transparent",
            BackgroundEffect::Transparent {
                fallback: Rgb::GREEN_SCREEN,
            },
            OutputFormat::Bgra,
        ),
    ];
    for (name, effect, format) in cases {
        settings.effect = effect;
        pipeline.apply_settings(settings.clone())?;
        for _ in 0..30 {
            pipeline.process(&camera, format)?;
        }
        let period = Duration::from_secs(1) / 30;
        let cpu_start = process_cpu_time();
        let start = Instant::now();
        let mut next = start;
        let mut timings = Vec::new();
        while start.elapsed() < Duration::from_secs(seconds) {
            let frame_start = Instant::now();
            pipeline.process(&camera, format)?;
            timings.push(frame_start.elapsed().as_secs_f64() * 1000.0);
            next += period;
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        let cpu_ms = (process_cpu_time() - cpu_start).as_secs_f64() * 1000.0;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        timings.sort_by(f64::total_cmp);
        println!(
            "{name:<13} model {:<45} frames {:>4}  median {:>5.2} ms  p95 {:>5.2} ms  CPU {:.2} ms/frame ({:.1}% of one core)  working set {:.0} MB  private {:.0} MB  VRAM {:.0} MB",
            pipeline.active_model().unwrap_or("-"),
            timings.len(),
            timings[timings.len() / 2],
            timings[timings.len() * 95 / 100],
            cpu_ms / timings.len() as f64,
            cpu_ms / elapsed_ms * 100.0,
            memory_mb().0,
            memory_mb().1,
            memory_mb().2
        );
    }
    Ok(())
}
