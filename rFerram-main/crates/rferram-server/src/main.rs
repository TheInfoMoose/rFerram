//! `rferram-server` Elevated Host Daemon Entrypoint

use anyhow::Result;
use rferram_net::{QuicServerListener, QuicTransportConfig};
use rferram_server::RFerramHostDaemon;
use std::net::SocketAddr;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let backend = RFerramHostDaemon::auto_detect_backend();
    let daemon = RFerramHostDaemon::new("rferram-host-node".to_string(), backend);

    let bind_addr: SocketAddr = std::env::var("RFERRAM_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:4433".to_string())
        .parse()?;

    let listener = QuicServerListener::bind(bind_addr, QuicTransportConfig::default())?;
    info!(
        local_addr = ?listener.local_addr()?,
        backend = ?daemon.backend,
        "rFerram Host Daemon listening for incoming QUIC connections"
    );

    if std::env::var("RFERRAM_SMOKE_TEST").is_ok() {
        return Ok(());
    }

    while let Some(conn) = listener.accept().await {
        info!(
            remote = %conn.connection.remote_address(),
            "Provisioned host session for remote viewer"
        );
    }

    Ok(())
}
