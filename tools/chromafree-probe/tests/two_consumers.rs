use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use chromafree_ipc::{ObjectNames, OutputMode, PixelFormat, ProducerChannel};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const FPS: u32 = 30;
const SMOKE_SECONDS: f64 = 4.0;
const VALUE_STEPS: u64 = 200;

fn build_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/vcam/Release")
}

fn luma_values(output: &Output) -> Vec<u8> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with("sample"))
        .filter_map(|line| line.split_whitespace().find_map(|part| part.strip_prefix("b0=")))
        .map(|value| value.parse().unwrap())
        .collect()
}

fn frame_value(frame_number: u64) -> u8 {
    (16 + frame_number % VALUE_STEPS) as u8
}

#[test]
#[ignore]
fn two_consumers_both_receive_every_new_frame_in_order() {
    let stop = Arc::new(AtomicBool::new(false));
    let mut channel = ProducerChannel::create(&ObjectNames::local()).unwrap();
    channel
        .set_output_mode(OutputMode {
            width: WIDTH,
            height: HEIGHT,
            fps_numerator: FPS,
            fps_denominator: 1,
        })
        .unwrap();
    let producer_stop = Arc::clone(&stop);
    let producer = thread::spawn(move || {
        let mut pixels = vec![0u8; PixelFormat::Nv12.frame_size(WIDTH, HEIGHT)];
        let period = Duration::from_secs(1) / FPS;
        let mut next = Instant::now();
        let mut frame_number = 0u64;
        while !producer_stop.load(Ordering::Relaxed) {
            frame_number += 1;
            pixels.fill(frame_value(frame_number));
            channel.publish(PixelFormat::Nv12, WIDTH, HEIGHT, &pixels).unwrap();
            let now = Instant::now();
            next = (next + period).max(now);
            thread::sleep(next - now);
        }
    });

    let spawn = || {
        let dir = build_dir();
        Command::new(dir.join("vcam-source-smoke.exe"))
            .arg(dir.join("vcam-source.dll"))
            .arg("nv12")
            .arg(SMOKE_SECONDS.to_string())
            .output()
    };
    let consumers: Vec<_> = (0..2).map(|_| thread::spawn(spawn)).collect();
    let outputs: Vec<Output> = consumers.into_iter().map(|c| c.join().unwrap().unwrap()).collect();
    stop.store(true, Ordering::Relaxed);
    producer.join().unwrap();

    for output in &outputs {
        assert!(output.status.success());
        let values = luma_values(output);
        let settled = &values[values.len() / 4..];
        let fresh = settled.windows(2).filter(|pair| pair[0] != pair[1]).count();
        let backwards = settled
            .windows(2)
            .filter(|pair| pair[1] < pair[0] && pair[0] - pair[1] < (VALUE_STEPS / 2) as u8)
            .count();
        println!(
            "{} samples, {} fresh transitions, {} backwards",
            values.len(),
            fresh,
            backwards
        );
        let expected = SMOKE_SECONDS * f64::from(FPS);
        assert!(values.len() as f64 > expected * 0.85, "{} samples", values.len());
        assert!(
            fresh as f64 >= settled.len() as f64 * 0.85,
            "{fresh} fresh of {}",
            settled.len()
        );
        assert_eq!(backwards, 0);
    }
}
