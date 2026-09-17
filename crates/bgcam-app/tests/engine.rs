use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bgcam_app::camera::{CameraOpenError, CameraProvider, OpenedCamera};
use bgcam_app::config::{CameraConfig, Config, DeviceConfig, EffectMode};
use bgcam_app::engine::{Engine, EngineCommand, EngineObserver, EngineOptions, EngineState, EngineStatus};
use bgcam_capture::{CaptureError, CaptureFormat, Encoding, FrameSource, SyntheticSource};
use bgcam_core::{BgraFrame, FrameSize, Nv12Frame};
use bgcam_ipc::{ObjectNames, OutputMode, PixelFormat, ReaderChannel};

const CAMERA_WIDTH: u32 = 640;
const CAMERA_HEIGHT: u32 = 360;
const CLOSE_DELAY: Duration = Duration::from_millis(300);

#[derive(Default)]
struct Counters {
    opened: AtomicUsize,
    live: AtomicUsize,
}

struct CountedSource {
    inner: SyntheticSource,
    counters: Arc<Counters>,
}

impl FrameSource for CountedSource {
    fn size(&self) -> FrameSize {
        self.inner.size()
    }

    fn read(&mut self) -> Result<&Nv12Frame, CaptureError> {
        self.inner.read()
    }
}

impl Drop for CountedSource {
    fn drop(&mut self) {
        self.counters.live.fetch_sub(1, Ordering::SeqCst);
    }
}

struct SyntheticCameras(Arc<Counters>);

impl CameraProvider for SyntheticCameras {
    fn open(&mut self, _camera: &CameraConfig, output_fps: u32) -> Result<OpenedCamera, CameraOpenError> {
        self.0.opened.fetch_add(1, Ordering::SeqCst);
        self.0.live.fetch_add(1, Ordering::SeqCst);
        let size = FrameSize::new(CAMERA_WIDTH, CAMERA_HEIGHT).map_err(anyhow::Error::from)?;
        Ok(OpenedCamera {
            source: Box::new(CountedSource {
                inner: SyntheticSource::new(size, output_fps),
                counters: Arc::clone(&self.0),
            }),
            name: "synthetic".to_owned(),
            format: CaptureFormat {
                width: CAMERA_WIDTH,
                height: CAMERA_HEIGHT,
                fps_numerator: output_fps,
                fps_denominator: 1,
                encoding: Encoding::Nv12,
            },
        })
    }
}

#[derive(Default)]
struct Recorded {
    statuses: Vec<EngineStatus>,
    previews: usize,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Recorded>>);

impl EngineObserver for Recorder {
    fn status_changed(&self, status: &EngineStatus) {
        self.0.lock().unwrap().statuses.push(status.clone());
    }

    fn preview(&self, frame: &BgraFrame) {
        assert_eq!(frame.data()[3], 255);
        self.0.lock().unwrap().previews += 1;
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn test_config(width: u32, height: u32) -> Config {
    let mut config = Config::default();
    config.output.width = width;
    config.output.height = height;
    config.effect.mode = EffectMode::Passthrough;
    config.segmentation.device = DeviceConfig::Cpu;
    config
}

fn read_frame(reader: &ReaderChannel, buffer: &mut [u8], format: PixelFormat, size: (u32, u32)) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "no {format:?} {size:?} frame arrived");
        reader.heartbeat();
        if !reader.wait_for_frame(Duration::from_millis(200)) {
            continue;
        }
        if let Ok(Some(info)) = reader.read(buffer)
            && info.format == format
            && (info.width, info.height) == size
        {
            return;
        }
    }
}

#[test]
fn engine_follows_consumers_formats_preview_and_settings() {
    let names = ObjectNames::with_prefix(&format!("Local\\bgcam-engine-test-{}-", std::process::id()));
    let counters = Arc::new(Counters::default());
    let recorder = Recorder::default();
    let engine = Engine::start(
        EngineOptions {
            names: names.clone(),
            models_dir: std::env::temp_dir().join("bgcam-no-models"),
            register_virtual_camera: false,
            camera_close_delay: CLOSE_DELAY,
        },
        test_config(320, 180),
        SyntheticCameras(Arc::clone(&counters)),
        recorder.clone(),
    )
    .unwrap();

    let mut reader = ReaderChannel::open(&names).unwrap();
    assert_eq!(
        reader.output_mode(),
        Some(OutputMode {
            width: 320,
            height: 180,
            fps_numerator: 30,
            fps_denominator: 1
        })
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        counters.opened.load(Ordering::SeqCst),
        0,
        "camera opened without a consumer"
    );

    let mut buffer = vec![0u8; 1920 * 1080 * 4];
    reader.start(PixelFormat::Nv12).unwrap();
    read_frame(&reader, &mut buffer, PixelFormat::Nv12, (320, 180));
    assert!(
        buffer[..320].contains(&235),
        "moving bar missing from the published frame"
    );
    assert!(reader.producer_alive());

    reader.stop().unwrap();
    reader.start(PixelFormat::Bgra).unwrap();
    read_frame(&reader, &mut buffer, PixelFormat::Bgra, (320, 180));
    assert_eq!(
        counters.opened.load(Ordering::SeqCst),
        1,
        "format change reopened the camera"
    );

    engine.send(EngineCommand::Apply(Box::new(test_config(640, 360))));
    wait_until("new output mode", || {
        reader.output_mode().is_some_and(|mode| mode.width == 640)
    });
    read_frame(&reader, &mut buffer, PixelFormat::Bgra, (640, 360));
    let streaming = Instant::now();
    while streaming.elapsed() < Duration::from_millis(1300) {
        read_frame(&reader, &mut buffer, PixelFormat::Bgra, (640, 360));
    }

    reader.stop().unwrap();
    wait_until("producer to go inactive", || !reader.producer_alive());
    wait_until("camera to close after the delay", || {
        counters.live.load(Ordering::SeqCst) == 0
    });

    engine.send(EngineCommand::SetPreview(true));
    wait_until("preview frames", || recorder.0.lock().unwrap().previews >= 5);
    assert!(!reader.producer_alive(), "preview alone must not publish frames");
    engine.send(EngineCommand::SetPreview(false));
    wait_until("camera to close after preview", || {
        counters.live.load(Ordering::SeqCst) == 0
    });

    drop(engine);
    let recorded = recorder.0.lock().unwrap();
    assert!(
        recorded
            .statuses
            .iter()
            .any(|s| s.state == EngineState::Running && s.fps > 20.0)
    );
    assert!(recorded.statuses.iter().any(|s| s.backend.as_deref() == Some("CPU")));
    assert_eq!(recorded.statuses.last().map(|s| &s.state), Some(&EngineState::Idle));
}
