//! `rferram-ui` Native COSMIC Desktop & Cross-Platform Viewer Entrypoint

use anyhow::Result;
use cosmic::Application;
use cosmic::app::Core;
use rferram_ui::RFerramApp;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let (app, _task) = RFerramApp::init(Core::new(), ());
    info!(
        app_id = RFerramApp::APP_ID,
        platform = ?app.runtime_env.platform,
        is_cosmic_desktop = app.runtime_env.is_cosmic_desktop,
        is_wayland = app.runtime_env.is_wayland,
        profiles = app.connection_cards.len(),
        "Initialized rFerram UI Shell"
    );

    Ok(())
}
