use std::io::BufRead;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use chromafree_app::camera::MediaFoundationCameras;
use chromafree_app::config::Config;
use chromafree_app::desktop::SingleInstance;
use chromafree_app::engine::{Engine, EngineObserver, EngineOptions, EngineStatus};
use chromafree_app::gui;
use chromafree_core::BgraFrame;
use chromafree_ipc::ObjectNames;

const CAMERA_CLOSE_DELAY: Duration = Duration::from_secs(5);

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
            warning = status.warning.as_deref().unwrap_or("-"),
            "status"
        );
    }

    fn preview(&self, _frame: &BgraFrame) {}
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

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
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
        gui::run(config, config_path, models_dir)
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
    std::io::stdin().lock().read_line(&mut String::new())?;
    drop(engine);
    Ok(())
}
