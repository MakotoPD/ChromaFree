use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use chromafree_ipc::{ObjectNames, OutputMode, PixelFormat, ProducerChannel};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const FPS: u32 = 30;
const SMOKE_SECONDS: f64 = 6.0;
const PRODUCER_STOP_MS: f64 = 3000.0;
const PRODUCER_VALUE: u8 = 150;
const PRODUCER_ALPHA: u8 = 128;

struct Sample {
    time_ms: f64,
    latency_ms: f64,
    first: u8,
    fourth: u8,
}

struct ProducerReport {
    consumer_formats: Vec<PixelFormat>,
}

fn build_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/vcam/Release")
}

fn field<T: std::str::FromStr>(line: &str, name: &str) -> T {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(name).and_then(|rest| rest.strip_prefix('=')))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("missing {name} in {line}"))
}

fn spawn_producer(stop: Arc<AtomicBool>) -> thread::JoinHandle<ProducerReport> {
    let mut channel = ProducerChannel::create(&ObjectNames::local()).unwrap();
    channel
        .set_output_mode(OutputMode {
            width: WIDTH,
            height: HEIGHT,
            fps_numerator: FPS,
            fps_denominator: 1,
        })
        .unwrap();
    thread::spawn(move || {
        let mut pixels = vec![0u8; PixelFormat::Bgra.frame_size(WIDTH, HEIGHT)];
        let mut consumer_formats = Vec::new();
        let period = Duration::from_secs(1) / FPS;
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let consumer = channel.consumer();
            if let Some(format) = consumer.format {
                if consumer_formats.last() != Some(&format) {
                    consumer_formats.push(format);
                }
                match format {
                    PixelFormat::Nv12 => pixels.fill(PRODUCER_VALUE),
                    PixelFormat::Bgra => {
                        for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
                            pixel.copy_from_slice(&[PRODUCER_VALUE, PRODUCER_VALUE, PRODUCER_VALUE, PRODUCER_ALPHA]);
                        }
                    }
                }
                channel.publish(format, WIDTH, HEIGHT, &pixels).unwrap();
            }
            let now = Instant::now();
            next = (next + period).max(now);
            thread::sleep(next - now);
        }
        drop(channel);
        ProducerReport { consumer_formats }
    })
}

fn run_smoke(format: &str) -> (Vec<Sample>, ProducerReport) {
    let dir = build_dir();
    let stop = Arc::new(AtomicBool::new(false));
    let producer = spawn_producer(Arc::clone(&stop));
    let mut child = Command::new(dir.join("vcam-source-smoke.exe"))
        .arg(dir.join("vcam-source.dll"))
        .arg(format)
        .arg(SMOKE_SECONDS.to_string())
        .stdout(Stdio::piped())
        .spawn()
        .expect("build vcam-source first: cmake --build build/vcam --config Release");

    let mut samples = Vec::new();
    let mut summary = None;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        if line.starts_with("type") {
            assert_eq!(field::<u32>(&line, "width"), WIDTH);
            assert_eq!(field::<u32>(&line, "height"), HEIGHT);
        } else if line.starts_with("sample") {
            let sample = Sample {
                time_ms: field(&line, "t"),
                latency_ms: field(&line, "latency"),
                first: field(&line, "b0"),
                fourth: field(&line, "b3"),
            };
            if sample.time_ms >= PRODUCER_STOP_MS {
                stop.store(true, Ordering::Relaxed);
            }
            samples.push(sample);
        } else if line.starts_with("summary") {
            summary = Some(line);
        }
    }
    stop.store(true, Ordering::Relaxed);
    assert!(child.wait().unwrap().success());
    assert!(summary.is_some());
    (samples, producer.join().unwrap())
}

fn check(format: &str, expected_consumer: PixelFormat, producer_bytes: (u8, u8), fallback_bytes: (u8, u8)) {
    let (samples, report) = run_smoke(format);
    assert_eq!(report.consumer_formats, vec![expected_consumer]);

    let expected = SMOKE_SECONDS * f64::from(FPS);
    let count = samples.len() as f64;
    assert!(
        count > expected * 0.9 && count < expected * 1.05,
        "{count} samples in {SMOKE_SECONDS}s, pacing expected about {expected}"
    );

    let produced: Vec<_> = samples
        .iter()
        .filter(|s| s.time_ms > 500.0 && s.time_ms < PRODUCER_STOP_MS - 200.0)
        .collect();
    assert!(produced.len() > 60);
    assert!(produced.iter().all(|s| (s.first, s.fourth) == producer_bytes));
    let latency = produced.iter().map(|s| s.latency_ms).sum::<f64>() / produced.len() as f64;
    let max_latency = produced.iter().map(|s| s.latency_ms).fold(0.0, f64::max);
    println!("{format}: {count} samples, producer frame latency avg={latency:.2}ms max={max_latency:.2}ms");
    assert!(latency < 10.0, "average latency {latency:.2}ms");

    let fallback: Vec<_> = samples
        .iter()
        .filter(|s| s.time_ms > PRODUCER_STOP_MS + 500.0)
        .collect();
    assert!(fallback.len() > 60);
    assert!(fallback.iter().all(|s| (s.first, s.fourth) == fallback_bytes));
}

#[test]
#[ignore]
fn virtual_camera_source_delivers_producer_frames_then_fallback() {
    check("nv12", PixelFormat::Nv12, (PRODUCER_VALUE, PRODUCER_VALUE), (16, 16));
    check("argb32", PixelFormat::Bgra, (PRODUCER_VALUE, PRODUCER_ALPHA), (0, 255));
}
