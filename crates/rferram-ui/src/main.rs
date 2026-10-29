//! `rferram-ui` Native COSMIC Desktop & Cross-Platform Viewer Entrypoint

use anyhow::{Result, anyhow, ensure};
use cosmic::Application;
use cosmic::app::Core;
use rferram_client::WgpuRenderPipeline;
use rferram_ui::{
    RFerramApp, frosted_glass_shader_descriptor, validate_frosted_glass_shader_source,
};
use tracing::info;

/// Validates the UI frosted glass WGSL shader and the client video surface WGSL shader descriptors.
fn validate_startup_shaders() -> Result<(&'static str, &'static str)> {
    ensure!(
        validate_frosted_glass_shader_source(),
        "Frosted glass WGSL shader source failed validation"
    );
    let frosted_descriptor = frosted_glass_shader_descriptor();
    let frosted_label = frosted_descriptor
        .label
        .ok_or_else(|| anyhow!("Frosted glass shader descriptor is missing a label"))?;
    match &frosted_descriptor.source {
        wgpu::ShaderSource::Wgsl(src) => ensure!(
            src.contains("fn fs_frosted_glass"),
            "Frosted glass shader descriptor is missing `fs_frosted_glass` entry point"
        ),
        _ => anyhow::bail!("Frosted glass shader descriptor must use WGSL source"),
    }

    let video_descriptor = WgpuRenderPipeline::validate_default_shader()?;
    let video_label = video_descriptor
        .label
        .ok_or_else(|| anyhow!("Video surface shader descriptor is missing a label"))?;

    Ok((frosted_label, video_label))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let (frosted_shader_label, video_shader_label) = validate_startup_shaders()?;

    let (app, _task) = RFerramApp::init(Core::new(), ());
    info!(
        app_id = RFerramApp::APP_ID,
        platform = ?app.runtime_env.platform,
        is_cosmic_desktop = app.runtime_env.is_cosmic_desktop,
        is_wayland = app.runtime_env.is_wayland,
        profiles = app.connection_cards.len(),
        frosted_glass_shader = frosted_shader_label,
        video_surface_shader = video_shader_label,
        "Initialized rFerram UI Shell"
    );

    if std::env::var("RFERRAM_SMOKE_TEST").is_ok() {
        return Ok(());
    }

    cosmic::app::run::<RFerramApp>(cosmic::app::Settings::default(), ())
        .map_err(|err| anyhow!(err))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_startup_shaders() {
        let (frosted_label, video_label) =
            validate_startup_shaders().expect("startup shader validation must succeed");
        assert_eq!(frosted_label, "rferram_frosted_glass_shader");
        assert_eq!(video_label, "rferram_video_surface_shader");
    }
}
