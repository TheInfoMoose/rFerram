//! Privileged OS session elevation handler (`ElevationChannelHandler`) for Windows UAC,
//! Linux `polkit`/`sudo`, and macOS `launchd`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::info;

use rferram_common::{PlatformDistribution, RuntimeEnvironment};
use rferram_protocol::{VirtualChannelMessage, WireMessage};

/// Operating system privilege elevation mechanism selected for the host environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationMechanism {
    /// Linux `polkit` (`pkexec`) or `sudo` privilege escalation.
    LinuxPolkitOrSudo,
    /// Windows User Account Control (`UAC` / administrator token elevation).
    WindowsUac,
    /// macOS `launchd` privileged helper / Authorization Services.
    MacOsLaunchd,
}

impl ElevationMechanism {
    /// Resolves the appropriate [`ElevationMechanism`] for a [`PlatformDistribution`].
    pub fn for_platform(platform: PlatformDistribution) -> Self {
        match platform {
            PlatformDistribution::Windows => Self::WindowsUac,
            PlatformDistribution::MacOs => Self::MacOsLaunchd,
            PlatformDistribution::LinuxDebian
            | PlatformDistribution::LinuxRhel
            | PlatformDistribution::LinuxSuse
            | PlatformDistribution::LinuxArch
            | PlatformDistribution::LinuxGeneric => Self::LinuxPolkitOrSudo,
        }
    }
}

/// Privileged OS Elevation Request Handler (Windows UAC, Linux polkit/sudo, macOS launchd)
#[derive(Clone)]
pub struct ElevationChannelHandler {
    mechanism: ElevationMechanism,
    requests_sent: Arc<AtomicU64>,
    requests_received: Arc<AtomicU64>,
}

impl Default for ElevationChannelHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ElevationChannelHandler {
    /// Creates a new [`ElevationChannelHandler`] detecting the current OS [`ElevationMechanism`].
    pub fn new() -> Self {
        let env = RuntimeEnvironment::detect();
        Self {
            mechanism: ElevationMechanism::for_platform(env.platform),
            requests_sent: Arc::new(AtomicU64::new(0)),
            requests_received: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns the detected OS [`ElevationMechanism`] for this handler.
    pub fn mechanism(&self) -> ElevationMechanism {
        self.mechanism
    }

    /// Returns the total number of outbound elevation requests sent by this handler.
    pub fn requests_sent(&self) -> u64 {
        self.requests_sent.load(Ordering::SeqCst)
    }

    /// Returns the total number of inbound elevation requests processed by this handler.
    pub fn requests_received(&self) -> u64 {
        self.requests_received.load(Ordering::SeqCst)
    }

    /// Sends a [`VirtualChannelMessage::RequestElevation`] control frame to the remote host daemon.
    pub async fn request_host_elevation(
        &self,
        outgoing_tx: mpsc::Sender<WireMessage>,
    ) -> Result<()> {
        info!(
            mechanism = ?self.mechanism,
            "Sending elevation request token to host daemon"
        );
        let msg = WireMessage::Channel(VirtualChannelMessage::RequestElevation);
        outgoing_tx.send(msg).await?;
        self.requests_sent.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Processes an incoming [`VirtualChannelMessage::RequestElevation`] on the host daemon.
    pub async fn handle_elevation_request(&self) -> Result<()> {
        let count = self.requests_received.fetch_add(1, Ordering::SeqCst) + 1;
        info!(
            mechanism = ?self.mechanism,
            request_count = count,
            "Host daemon received remote UAC / polkit / sudo elevation request"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_elevation_request_send_and_handle() {
        let handler = ElevationChannelHandler::new();
        assert_eq!(handler.requests_sent(), 0);
        assert_eq!(handler.requests_received(), 0);

        let (tx, mut rx) = mpsc::channel(4);
        handler
            .request_host_elevation(tx)
            .await
            .expect("Outbound elevation request should succeed");
        assert_eq!(handler.requests_sent(), 1);

        let msg = rx.recv().await.expect("Should emit RequestElevation frame");
        assert_eq!(
            msg,
            WireMessage::Channel(VirtualChannelMessage::RequestElevation)
        );

        handler
            .handle_elevation_request()
            .await
            .expect("Inbound elevation request handling should succeed");
        assert_eq!(handler.requests_received(), 1);
    }

    #[test]
    fn test_elevation_mechanism_mapping() {
        assert_eq!(
            ElevationMechanism::for_platform(PlatformDistribution::LinuxArch),
            ElevationMechanism::LinuxPolkitOrSudo
        );
        assert_eq!(
            ElevationMechanism::for_platform(PlatformDistribution::Windows),
            ElevationMechanism::WindowsUac
        );
        assert_eq!(
            ElevationMechanism::for_platform(PlatformDistribution::MacOs),
            ElevationMechanism::MacOsLaunchd
        );
    }
}
