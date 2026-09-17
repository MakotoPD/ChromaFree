use std::time::Instant;

use chromafree_capture::{CameraReader, Encoding, FrameSource, MediaFoundation, camera_formats, list_cameras};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

const FRAMES: usize = 90;

fn process_cpu_ms() -> f64 {
    let (mut creation, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) }.unwrap();
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    (ticks(kernel) + ticks(user)) as f64 / 10_000.0
}

#[test]
#[ignore]
fn reads_frames_from_the_first_camera() {
    let _mf = MediaFoundation::startup().unwrap();
    let cameras = list_cameras().unwrap();
    for camera in &cameras {
        println!("camera {} {}", camera.name, camera.symbolic_link);
    }
    let camera = cameras.first().expect("no camera connected");
    let formats = camera_formats(&camera.symbolic_link).unwrap();
    for format in &formats {
        println!("  {format}");
    }

    let mut chosen = Vec::new();
    for encoding in [Encoding::Nv12, Encoding::Yuy2, Encoding::Mjpeg] {
        if let Some(format) = formats
            .iter()
            .filter(|f| f.encoding == encoding && f.width <= 1920 && f.fps() >= 29.0)
            .max_by_key(|f| f.width * f.height)
        {
            chosen.push(*format);
        }
    }
    assert!(!chosen.is_empty());

    for format in chosen {
        let mut reader = CameraReader::open(&camera.symbolic_link, &format).unwrap();
        reader.read().unwrap();
        let cpu_started = process_cpu_ms();
        let started = Instant::now();
        let (mut luma_min, mut luma_max, mut chroma_min, mut chroma_max) = (255u8, 0u8, 255u8, 0u8);
        let mut ages = Vec::with_capacity(FRAMES);
        for _ in 0..FRAMES {
            reader.read().unwrap();
            ages.push(
                reader
                    .capture_age()
                    .expect("camera frames carry timestamps")
                    .as_secs_f64()
                    * 1000.0,
            );
            let frame = reader.read().unwrap();
            luma_min = luma_min.min(*frame.luma().iter().min().unwrap());
            luma_max = luma_max.max(*frame.luma().iter().max().unwrap());
            chroma_min = chroma_min.min(*frame.chroma().iter().min().unwrap());
            chroma_max = chroma_max.max(*frame.chroma().iter().max().unwrap());
        }
        let elapsed = started.elapsed().as_secs_f64();
        let cpu = process_cpu_ms() - cpu_started;
        println!(
            "{format}: {:.1} fps, process cpu {:.2} ms/frame including range scan, luma {luma_min}..{luma_max} chroma {chroma_min}..{chroma_max}",
            FRAMES as f64 / elapsed,
            cpu / FRAMES as f64
        );
        ages.sort_by(f64::total_cmp);
        println!(
            "  capture age at read: median {:.1} ms, p95 {:.1} ms",
            ages[ages.len() / 2],
            ages[ages.len() * 95 / 100]
        );
        assert!(ages[ages.len() / 2] < 1000.0, "timestamps are not on the system clock");
        assert_eq!(reader.size().width(), format.width);
    }
}
