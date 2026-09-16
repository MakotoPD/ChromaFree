use anyhow::Result;
use bgcam_app::config::Config;

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    let path = Config::default_path()?;
    let config = Config::load(&path)?;
    tracing::info!(path = %path.display(), ?config, "configuration loaded");
    Ok(())
}
