use std::io::BufRead;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bgcam_ipc::layout::BGCAM_FRAME_CAPACITY;
use bgcam_ipc::{
    IpcError, ObjectNames, OutputMode, PixelFormat, ProducerChannel, ReaderChannel, qpc_frequency, qpc_now,
};
use windows::Win32::Media::MediaFoundation::{
    MF_VERSION, MFCreateVirtualCamera, MFSTARTUP_FULL, MFShutdown, MFStartup, MFVirtualCameraAccess_CurrentUser,
    MFVirtualCameraLifetime_Session, MFVirtualCameraType_SoftwareCameraSource,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;

const CAMERA_NAME: &str = "bgcam";
const CAMERA_CLSID: &str = "{4525794B-703E-444B-A81D-2B278B2AD0E4}";
const REPORT_INTERVAL: Duration = Duration::from_secs(5);
const WAIT_SLICE: Duration = Duration::from_secs(1);

const USAGE: &str = "usage:
  bgcam-probe produce [--size WxH] [--fps N] [--format nv12|bgra] [--seconds S]
  bgcam-probe read [--format nv12|bgra] [--seconds S]
  bgcam-probe camera";

struct Options {
    width: u32,
    height: u32,
    fps: u32,
    format: Option<PixelFormat>,
    seconds: Option<f64>,
}

fn parse_format(value: &str) -> Result<PixelFormat> {
    match value {
        "nv12" => Ok(PixelFormat::Nv12),
        "bgra" => Ok(PixelFormat::Bgra),
        _ => bail!("unknown format {value}, expected nv12 or bgra"),
    }
}

fn parse_options(args: &[String]) -> Result<Options> {
    let mut options = Options {
        width: 1280,
        height: 720,
        fps: 30,
        format: None,
        seconds: None,
    };
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args.next().with_context(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--size" => {
                let (width, height) = value.split_once('x').context("size must look like 1280x720")?;
                options.width = width.parse()?;
                options.height = height.parse()?;
            }
            "--fps" => options.fps = value.parse::<u32>()?.max(1),
            "--format" => options.format = Some(parse_format(value)?),
            "--seconds" => options.seconds = Some(value.parse()?),
            _ => bail!("unknown option {flag}\n{USAGE}"),
        }
    }
    Ok(options)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        println!("{USAGE}");
        return Ok(());
    };
    let options = parse_options(&args[1..])?;
    match command.as_str() {
        "produce" => produce(&options),
        "read" => read(&options),
        "camera" => camera(),
        _ => bail!("unknown command {command}\n{USAGE}"),
    }
}

fn running(started: Instant, options: &Options) -> bool {
    options
        .seconds
        .is_none_or(|seconds| started.elapsed().as_secs_f64() < seconds)
}

struct Window {
    started: Instant,
    count: u64,
    sum_ms: f64,
    max_ms: f64,
}

impl Window {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            count: 0,
            sum_ms: 0.0,
            max_ms: 0.0,
        }
    }

    fn record(&mut self, milliseconds: f64) {
        self.count += 1;
        self.sum_ms += milliseconds;
        self.max_ms = self.max_ms.max(milliseconds);
    }

    fn take_if_due(&mut self) -> Option<(f64, f64, f64)> {
        let elapsed = self.started.elapsed();
        if elapsed < REPORT_INTERVAL {
            return None;
        }
        let fps = self.count as f64 / elapsed.as_secs_f64();
        let average = self.sum_ms / self.count.max(1) as f64;
        let report = (fps, average, self.max_ms);
        *self = Self::new();
        Some(report)
    }
}

fn draw_pattern(format: PixelFormat, width: u32, height: u32, frame_number: u64, pixels: &mut [u8]) {
    let (width, height) = (width as usize, height as usize);
    let bar_start = (frame_number as usize * 8) % width;
    let bar = bar_start..(bar_start + width / 16);
    match format {
        PixelFormat::Nv12 => {
            for (y, row) in pixels[..width * height].chunks_exact_mut(width).enumerate() {
                for (x, value) in row.iter_mut().enumerate() {
                    *value = if bar.contains(&x) {
                        235
                    } else {
                        (16 + (x + y) * 200 / (width + height)) as u8
                    };
                }
            }
            pixels[width * height..width * height * 3 / 2].fill(128);
        }
        PixelFormat::Bgra => {
            for (y, row) in pixels[..width * height * 4].chunks_exact_mut(width * 4).enumerate() {
                for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
                    let shade = if bar.contains(&x) { 255 } else { (x * 255 / width) as u8 };
                    let alpha = if y < height / 2 { 255 } else { (x * 255 / width) as u8 };
                    pixel.copy_from_slice(&[shade, (y * 255 / height) as u8, 255 - shade, alpha]);
                }
            }
        }
    }
    pixels[..8].copy_from_slice(&frame_number.to_le_bytes());
}

fn produce(options: &Options) -> Result<()> {
    let mut channel = ProducerChannel::create(&ObjectNames::local()).context("creating shared memory")?;
    channel.set_output_mode(OutputMode {
        width: options.width,
        height: options.height,
        fps_numerator: options.fps,
        fps_denominator: 1,
    })?;
    println!(
        "output mode {}x{}@{}, waiting for a consumer",
        options.width, options.height, options.fps
    );

    let mut pixels = vec![0u8; PixelFormat::Bgra.frame_size(options.width, options.height)];
    let period = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    let started = Instant::now();
    let mut frame_number = 0u64;
    let mut window = Window::new();
    let mut last_consumer = None;
    let mut next_frame = Instant::now();
    while running(started, options) {
        let consumer = channel.consumer();
        if last_consumer != Some((consumer.active, consumer.format, consumer.count)) {
            println!(
                "consumer active={} format={:?} count={}",
                consumer.active, consumer.format, consumer.count
            );
            last_consumer = Some((consumer.active, consumer.format, consumer.count));
        }
        let Some(format) = options.format.or(consumer.format) else {
            channel.set_active(false);
            channel.wait_for_consumer_change(WAIT_SLICE);
            next_frame = Instant::now();
            continue;
        };

        frame_number += 1;
        draw_pattern(format, options.width, options.height, frame_number, &mut pixels);
        let publish_started = Instant::now();
        channel.publish(format, options.width, options.height, &pixels)?;
        window.record(publish_started.elapsed().as_secs_f64() * 1000.0);
        if let Some((fps, average, max)) = window.take_if_due() {
            println!("published fps={fps:.1} write avg={average:.2}ms max={max:.2}ms");
        }

        let now = Instant::now();
        next_frame = (next_frame + period).max(now);
        std::thread::sleep(next_frame - now);
    }
    Ok(())
}

fn read(options: &Options) -> Result<()> {
    let format = options.format.unwrap_or(PixelFormat::Nv12);
    let mut channel =
        ReaderChannel::open(&ObjectNames::local()).context("opening shared memory, is a producer running?")?;
    println!("output mode {:?}", channel.output_mode());
    channel.start(format)?;

    let mut buffer = vec![0u8; BGCAM_FRAME_CAPACITY as usize];
    let frequency = qpc_frequency();
    let started = Instant::now();
    let mut window = Window::new();
    let (mut last_frame, mut dropped, mut invalid, mut contended) = (0u64, 0u64, 0u64, 0u64);
    let mut producer_alive = None;
    while running(started, options) {
        channel.heartbeat();
        let alive = channel.producer_alive();
        if producer_alive != Some(alive) {
            println!("producer alive={alive}");
            producer_alive = Some(alive);
        }
        if !channel.wait_for_frame(WAIT_SLICE) {
            continue;
        }
        let info = match channel.read(&mut buffer) {
            Ok(Some(info)) => info,
            Ok(None) => continue,
            Err(IpcError::Contended) => {
                contended += 1;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let latency_ms = (qpc_now() - info.qpc) as f64 * 1000.0 / frequency as f64;
        window.record(latency_ms);
        let embedded = u64::from_le_bytes(buffer[..8].try_into()?);
        if embedded != info.frame_number || info.format != format {
            invalid += 1;
        }
        if last_frame != 0 && info.frame_number > last_frame + 1 {
            dropped += info.frame_number - last_frame - 1;
        }
        last_frame = info.frame_number;
        if let Some((fps, average, max)) = window.take_if_due() {
            println!(
                "read {}x{} {:?} fps={fps:.1} latency avg={average:.2}ms max={max:.2}ms dropped={dropped} invalid={invalid} contended={contended}",
                info.width, info.height, info.format
            );
        }
    }
    channel.stop()?;
    Ok(())
}

fn camera() -> Result<()> {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }?;
    let camera = unsafe {
        MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_Session,
            MFVirtualCameraAccess_CurrentUser,
            &HSTRING::from(CAMERA_NAME),
            &HSTRING::from(CAMERA_CLSID),
            None,
        )
    }
    .context("creating the virtual camera")?;
    unsafe { camera.Start(None) }.context("starting the virtual camera, is vcam-source.dll registered?")?;
    println!("virtual camera \"{CAMERA_NAME}\" is running, press Enter to remove it");
    std::io::stdin().lock().read_line(&mut String::new())?;
    unsafe { camera.Remove() }?;
    unsafe { MFShutdown() }?;
    Ok(())
}
