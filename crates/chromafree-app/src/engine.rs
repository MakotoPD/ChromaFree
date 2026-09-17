use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chromafree_capture::{CaptureFormat, MediaFoundation};
use chromafree_core::{ColorMatrix, OutputFormat, PipelineOutput, RgbImage};
use chromafree_ipc::{ObjectNames, OutputMode, PixelFormat, ProducerChannel, ProducerWaker};

use crate::camera::{CameraOpenError, CameraProvider, OpenedCamera};
use crate::config::{CameraConfig, Config, EffectMode};
use crate::processor::Processor;
use crate::virtual_camera::VirtualCamera;

const RETRY_DELAY: Duration = Duration::from_secs(2);
const STATS_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq)]
pub enum EngineState {
    Idle,
    Running,
    NoCamera,
    CameraMissing(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum VirtualCameraState {
    Disabled,
    Registered,
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct EngineStatus {
    pub state: EngineState,
    pub virtual_camera: VirtualCameraState,
    pub camera: Option<(String, CaptureFormat)>,
    pub consumer_format: Option<PixelFormat>,
    pub backend: Option<String>,
    pub model: Option<String>,
    pub fps: f32,
    pub processing_ms: f32,
    pub latency_ms: Option<f32>,
    pub warning: Option<String>,
}

impl Default for EngineStatus {
    fn default() -> Self {
        Self {
            state: EngineState::Idle,
            virtual_camera: VirtualCameraState::Disabled,
            camera: None,
            consumer_format: None,
            backend: None,
            model: None,
            fps: 0.0,
            processing_ms: 0.0,
            latency_ms: None,
            warning: None,
        }
    }
}

pub trait EngineObserver: Send + 'static {
    fn status_changed(&self, status: &EngineStatus);
    fn preview(&self, output: &PipelineOutput<'_>, matrix: ColorMatrix);
}

pub struct EngineOptions {
    pub names: ObjectNames,
    pub models_dir: PathBuf,
    pub register_virtual_camera: bool,
    pub camera_close_delay: Duration,
}

pub enum EngineCommand {
    Apply(Box<Config>),
    SetPreview(bool),
    Shutdown,
}

pub struct Engine {
    commands: Sender<EngineCommand>,
    waker: ProducerWaker,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(
        options: EngineOptions,
        config: Config,
        cameras: impl CameraProvider,
        observer: impl EngineObserver,
    ) -> Result<Self> {
        let channel = ProducerChannel::create(&options.names).context("creating shared memory")?;
        channel.set_output_mode(output_mode(&config))?;
        let waker = channel.waker();
        let (commands, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("chromafree-engine".to_owned())
            .spawn(move || {
                let worker = Worker::new(
                    options,
                    config,
                    channel,
                    receiver,
                    Box::new(cameras),
                    Box::new(observer),
                );
                worker.run();
            })?;
        Ok(Self {
            commands,
            waker,
            thread: Some(thread),
        })
    }

    pub fn send(&self, command: EngineCommand) {
        if self.commands.send(command).is_ok() {
            self.waker.wake();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.send(EngineCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Session {
    camera: OpenedCamera,
    processor: Processor,
}

struct Stats {
    started: Instant,
    frames: u32,
    processing: Duration,
    latency: Duration,
    latency_samples: u32,
}

impl Stats {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            frames: 0,
            processing: Duration::ZERO,
            latency: Duration::ZERO,
            latency_samples: 0,
        }
    }
}

struct Worker {
    options: EngineOptions,
    config: Config,
    channel: ProducerChannel,
    commands: Receiver<EngineCommand>,
    cameras: Box<dyn CameraProvider>,
    observer: Box<dyn EngineObserver>,
    status: EngineStatus,
    session: Option<Session>,
    virtual_camera: Option<VirtualCamera>,
    background: Option<(PathBuf, Arc<RgbImage>)>,
    preview_enabled: bool,
    unused_since: Option<Instant>,
    stats: Stats,
}

impl Worker {
    fn new(
        options: EngineOptions,
        config: Config,
        channel: ProducerChannel,
        commands: Receiver<EngineCommand>,
        cameras: Box<dyn CameraProvider>,
        observer: Box<dyn EngineObserver>,
    ) -> Self {
        Self {
            options,
            config,
            channel,
            commands,
            cameras,
            observer,
            status: EngineStatus::default(),
            session: None,
            virtual_camera: None,
            background: None,
            preview_enabled: false,
            unused_since: None,
            stats: Stats::new(),
        }
    }

    fn run(mut self) {
        let _media_foundation = match MediaFoundation::startup() {
            Ok(guard) => Some(guard),
            Err(error) => {
                tracing::error!(%error, "Media Foundation startup failed");
                self.status.state = EngineState::Error(error.to_string());
                None
            }
        };
        self.register_virtual_camera();
        self.load_background();
        self.notify();

        loop {
            if !self.drain_commands() {
                break;
            }
            let consumer = self.channel.consumer();
            if self.status.consumer_format != consumer.format {
                self.status.consumer_format = consumer.format;
                self.notify();
            }
            if !consumer.active && !self.preview_enabled {
                self.wait_while_unused();
                continue;
            }
            self.unused_since = None;
            if self.session.is_none() && !self.open_session() {
                self.channel.set_active(false);
                self.channel.wait_for_consumer_change(RETRY_DELAY);
                continue;
            }
            let format = match consumer.format {
                Some(PixelFormat::Bgra) => OutputFormat::Bgra,
                Some(PixelFormat::Nv12) => OutputFormat::Nv12,
                None if self.config.effect.mode == EffectMode::Transparent => OutputFormat::Bgra,
                None => OutputFormat::Nv12,
            };
            if let Err(error) = self.process_frame(format, consumer.active) {
                tracing::warn!(error = format!("{error:#}"), "frame processing failed");
                self.session = None;
                self.channel.set_active(false);
                self.status.state = EngineState::Error(format!("{error:#}"));
                self.notify();
                self.channel.wait_for_consumer_change(RETRY_DELAY);
            }
        }
        self.session = None;
        self.virtual_camera = None;
        self.channel.set_active(false);
    }

    fn drain_commands(&mut self) -> bool {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                EngineCommand::Apply(config) => self.apply_config(*config),
                EngineCommand::SetPreview(enabled) => self.preview_enabled = enabled,
                EngineCommand::Shutdown => return false,
            }
        }
        true
    }

    fn wait_while_unused(&mut self) {
        if self.session.is_none() {
            self.channel.set_active(false);
            if self.status.state == EngineState::Running {
                self.status.state = EngineState::Idle;
                self.status.fps = 0.0;
                self.notify();
            }
            self.channel.wait_for_consumer_change(Duration::MAX);
            return;
        }
        let since = *self.unused_since.get_or_insert_with(Instant::now);
        let remaining = self.options.camera_close_delay.saturating_sub(since.elapsed());
        if remaining.is_zero() {
            tracing::info!("no consumer, closing the camera");
            self.session = None;
            self.unused_since = None;
            self.status.state = EngineState::Idle;
            self.status.camera = None;
            self.status.fps = 0.0;
            self.notify();
            return;
        }
        self.channel.set_active(false);
        self.channel.wait_for_consumer_change(remaining);
    }

    fn open_session(&mut self) -> bool {
        let camera = match self.cameras.open(&self.config.camera, self.config.output.fps) {
            Ok(camera) => camera,
            Err(error) => {
                let state = match error {
                    CameraOpenError::NoCamera => EngineState::NoCamera,
                    CameraOpenError::Missing(name) => EngineState::CameraMissing(name),
                    CameraOpenError::Other(error) => EngineState::Error(format!("{error:#}")),
                };
                if self.status.state != state {
                    tracing::warn!(?state, "camera unavailable");
                    self.status.state = state;
                    self.notify();
                }
                return false;
            }
        };
        let settings = match self.config.pipeline_settings(self.background_image()) {
            Ok(settings) => settings,
            Err(error) => {
                self.status.state = EngineState::Error(format!("{error:#}"));
                self.notify();
                return false;
            }
        };
        let processor = Processor::create(&self.options.models_dir, settings);
        self.status.state = EngineState::Running;
        self.status.camera = Some((camera.name.clone(), camera.format));
        self.status.backend = Some(processor.backend());
        self.status.warning = None;
        self.stats = Stats::new();
        self.session = Some(Session { camera, processor });
        self.notify();
        true
    }

    fn process_frame(&mut self, format: OutputFormat, consumer_active: bool) -> Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        let frame = session.camera.source.read()?;
        let started = Instant::now();
        let matrix = session.processor.settings().matrix();
        let output = session.processor.process(frame, format)?;
        match &output {
            PipelineOutput::Nv12(frame) if consumer_active => {
                let size = frame.size();
                self.channel.publish_parts(
                    PixelFormat::Nv12,
                    size.width(),
                    size.height(),
                    &[frame.luma(), frame.chroma()],
                )?;
            }
            PipelineOutput::Bgra(frame) if consumer_active => {
                let size = frame.size();
                self.channel
                    .publish(PixelFormat::Bgra, size.width(), size.height(), frame.data())?;
            }
            _ => self.channel.set_active(false),
        }
        if self.preview_enabled {
            self.observer.preview(&output, matrix);
        }
        let model = session.processor.active_model().map(str::to_owned);
        self.stats.frames += 1;
        self.stats.processing += started.elapsed();
        if let Some(age) = session.camera.source.capture_age() {
            self.stats.latency += age;
            self.stats.latency_samples += 1;
        }
        let elapsed = self.stats.started.elapsed();
        if elapsed >= STATS_INTERVAL {
            self.status.fps = self.stats.frames as f32 / elapsed.as_secs_f32();
            self.status.processing_ms = self.stats.processing.as_secs_f32() * 1000.0 / self.stats.frames as f32;
            self.status.latency_ms = (self.stats.latency_samples > 0)
                .then(|| self.stats.latency.as_secs_f32() * 1000.0 / self.stats.latency_samples as f32);
            self.status.model = model;
            self.stats = Stats::new();
            self.notify();
        }
        Ok(())
    }

    fn apply_config(&mut self, config: Config) {
        let previous = std::mem::replace(&mut self.config, config);
        if previous.output != self.config.output {
            self.publish_output_mode();
            if self.virtual_camera.take().is_some() {
                self.register_virtual_camera();
            }
        }
        if previous.effect.image_path != self.config.effect.image_path
            || previous.effect.mode != self.config.effect.mode
        {
            self.load_background();
        }
        if camera_changed(&previous.camera, &self.config.camera) {
            self.session = None;
            self.status.state = EngineState::Idle;
            self.notify();
            return;
        }
        let settings = match self.config.pipeline_settings(self.background_image()) {
            Ok(settings) => settings,
            Err(error) => {
                self.status.warning = Some(format!("{error:#}"));
                self.notify();
                return;
            }
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.processor.runs_on(settings.segmentation.device) {
            session.processor = Processor::create(&self.options.models_dir, settings);
            self.status.backend = Some(session.processor.backend());
        } else if let Err(error) = session.processor.apply_settings(settings) {
            self.status.warning = Some(format!("{error:#}"));
        }
        self.notify();
    }

    fn publish_output_mode(&mut self) {
        if let Err(error) = self.channel.set_output_mode(output_mode(&self.config)) {
            self.status.warning = Some(error.to_string());
        }
    }

    fn register_virtual_camera(&mut self) {
        if !self.options.register_virtual_camera {
            return;
        }
        self.virtual_camera = None;
        match VirtualCamera::create() {
            Ok(camera) => {
                self.virtual_camera = Some(camera);
                self.status.virtual_camera = VirtualCameraState::Registered;
            }
            Err(error) => {
                tracing::error!(error = format!("{error:#}"), "virtual camera unavailable");
                self.status.virtual_camera = VirtualCameraState::Unavailable(format!("{error:#}"));
            }
        }
        self.notify();
    }

    fn load_background(&mut self) {
        let wanted = match (&self.config.effect.mode, &self.config.effect.image_path) {
            (EffectMode::Image, Some(path)) => path.clone(),
            _ => {
                self.background = None;
                return;
            }
        };
        if self.background.as_ref().is_some_and(|(path, _)| *path == wanted) {
            return;
        }
        match RgbImage::load(&wanted) {
            Ok(image) => self.background = Some((wanted, Arc::new(image))),
            Err(error) => {
                self.background = None;
                self.status.warning = Some(format!("{}: {error}", wanted.display()));
            }
        }
    }

    fn background_image(&self) -> Option<Arc<RgbImage>> {
        self.background.as_ref().map(|(_, image)| Arc::clone(image))
    }

    fn notify(&self) {
        self.observer.status_changed(&self.status);
    }
}

fn output_mode(config: &Config) -> OutputMode {
    OutputMode {
        width: config.output.width,
        height: config.output.height,
        fps_numerator: config.output.fps,
        fps_denominator: 1,
    }
}

fn camera_changed(previous: &CameraConfig, current: &CameraConfig) -> bool {
    previous.symbolic_link != current.symbolic_link || previous.format != current.format
}
