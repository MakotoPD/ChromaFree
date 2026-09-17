#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::OpenOptions;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use chromafree_app::camera::MediaFoundationCameras;
use chromafree_app::config::Config;
use chromafree_app::desktop::SingleInstance;
use chromafree_app::engine::{Engine, EngineObserver, EngineOptions, EngineStatus};
use chromafree_app::gui;
use chromafree_app::virtual_camera::{install_system_camera, uninstall_system_camera};
use chromafree_capture::MediaFoundation;
use chromafree_core::{ColorMatrix, PipelineOutput};
use chromafree_ipc::ObjectNames;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::HSTRING;

const CAMERA_CLOSE_DELAY: Duration = Duration::from_secs(5);
const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

struct LogObserver;

impl EngineObserver for LogObserver {
    fn status_changed(&self, status: &EngineStatus) {
        tracing::info!(
            state = ?status.state,
            virtual_camera = ?status.virtual_camera,
            consumer = ?status.consumer_format,
            backend = status.backend.as_deref().unwrap_or("-"),
            model = status.model.as_deref().unwrap_or("-"),
            fps = status.fps,
            processing_ms = status.processing_ms,
            latency_ms = status.latency_ms.unwrap_or(-1.0),
            warning = status.warning.as_deref().unwrap_or("-"),
            "status"
        );
    }

    fn preview(&self, _output: &PipelineOutput<'_>, _matrix: ColorMatrix) {}
}

fn models_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("CHROMAFREE_MODELS_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let exe = std::env::current_exe().context("locating the executable")?;
    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join("models"))
        .find(|dir| dir.is_dir())
        .context("models directory not found next to the executable, set CHROMAFREE_MODELS_DIR")
}

fn init_logging() -> Option<PathBuf> {
    let directory = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("ChromaFree");
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join("chromafree.log");
    if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > LOG_ROTATE_BYTES) {
        let _ = std::fs::rename(&path, directory.join("chromafree.old.log"));
    }
    let file = OpenOptions::new().create(true).append(true).open(&path).ok()?;
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(Mutex::new(file).and(std::io::stderr))
        .init();
    Some(path)
}

fn main() {
    let log_path = init_logging();
    if log_path.is_none() {
        tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    }
    if let Some(action) = std::env::args().find_map(|arg| CameraAction::parse(&arg)) {
        if let Err(error) = action.run() {
            tracing::error!(error = format!("{error:#}"), "virtual camera setup failed");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run() {
        tracing::error!(error = format!("{error:#}"), "ChromaFree stopped");
        let log = log_path
            .map(|path| format!("\n\nLog: {}", path.display()))
            .unwrap_or_default();
        let text = HSTRING::from(format!("ChromaFree nie może działać:\n{error:#}{log}"));
        unsafe { MessageBoxW(None, &text, &HSTRING::from("ChromaFree"), MB_OK | MB_ICONERROR) };
    }
}

enum CameraAction {
    Install,
    Uninstall,
}

impl CameraAction {
    fn parse(argument: &str) -> Option<Self> {
        match argument {
            "--install-camera" => Some(Self::Install),
            "--uninstall-camera" => Some(Self::Uninstall),
            _ => None,
        }
    }

    fn run(self) -> Result<()> {
        let _media_foundation = MediaFoundation::startup()?;
        match self {
            Self::Install => install_system_camera(),
            Self::Uninstall => uninstall_system_camera(),
        }
    }
}

fn run() -> Result<()> {
    let Some(_instance) = SingleInstance::acquire()? else {
        tracing::warn!("ChromaFree is already running");
        return Ok(());
    };
    let config_path = Config::default_path()?;
    let config = Config::load(&config_path).unwrap_or_else(|error| {
        tracing::warn!(error = format!("{error:#}"), "configuration is invalid, using defaults");
        Config::default()
    });
    let models_dir = models_dir()?;
    tracing::info!(config = %config_path.display(), models = %models_dir.display(), "starting");
    if std::env::args().any(|arg| arg == "--headless") {
        run_headless(config, models_dir)
    } else {
        gui::run(
            config,
            config_path,
            models_dir,
            !std::env::args().any(|arg| arg == "--minimized"),
        )
    }
}

fn run_headless(config: Config, models_dir: PathBuf) -> Result<()> {
    let engine = Engine::start(
        EngineOptions {
            names: ObjectNames::local(),
            models_dir,
            register_virtual_camera: true,
            camera_close_delay: CAMERA_CLOSE_DELAY,
        },
        config,
        MediaFoundationCameras,
        LogObserver,
    )?;
    tracing::info!("running headless, press Enter to quit");
    if std::io::stdin().lock().read_line(&mut String::new())? == 0 {
        loop {
            std::thread::park();
        }
    }
    drop(engine);
    Ok(())
}
