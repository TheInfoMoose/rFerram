//! `rferram-channels`: Out-of-Band Virtual Channel Handlers
//!
//! Provides modular handlers for bi-directional virtual channel features:
//! - Cross-platform native clipboard synchronization (`arboard 3.7.0` for text, PNG/RGBA images, and file lists)
//! - `ST_Rust_Age` snapshot-backed file transfer operations (`simply-transfer-core`, `simply-transfer-crypto`, & `simply-transfer-snapshots`)
//! - Streaming SHA-256 byte-level hash validation (`simply_transfer_crypto::hash`)
//! - Embedded P2P out-of-band (OOB) Ed25519 SSH key ingestion (`russh` & `ed25519-dalek`)
//! - Native device and smartcard passthrough
//! - Privileged OS session elevation requests (UAC / polkit / sudo / launchd)

pub mod clipboard;
pub mod elevation;
pub mod oob_ssh;
pub mod passthrough;
pub mod snapshot_transfer;

pub use clipboard::*;
pub use elevation::*;
pub use oob_ssh::*;
pub use passthrough::*;
pub use snapshot_transfer::*;

use anyhow::Result;

use rferram_protocol::VirtualChannelMessage;

/// Unified Orchestration Manager for all Virtual Channels
#[derive(Default)]
pub struct VirtualChannelManager {
    pub clipboard_handler: ClipboardChannelHandler,
    pub st_handler: SimplyTransferChannelHandler,
    pub oob_ssh_manager: OobKeyIngestionManager,
    pub device_handler: DevicePassthroughHandler,
    pub elevation_handler: ElevationChannelHandler,
}

impl VirtualChannelManager {
    pub fn new() -> Self {
        Self {
            clipboard_handler: ClipboardChannelHandler::new(),
            st_handler: SimplyTransferChannelHandler::new(),
            oob_ssh_manager: OobKeyIngestionManager::new(),
            device_handler: DevicePassthroughHandler::new(),
            elevation_handler: ElevationChannelHandler::new(),
        }
    }

    /// Route incoming virtual channel messages to specialized handlers
    pub async fn route_channel_message(&self, msg: VirtualChannelMessage) -> Result<()> {
        match msg {
            VirtualChannelMessage::Clipboard(payload) => {
                self.clipboard_handler
                    .handle_remote_clipboard(payload)
                    .await?;
            }
            VirtualChannelMessage::SimplyTransfer(st_bytes) => {
                let _ = self.st_handler.handle_incoming_st_bytes(&st_bytes).await?;
            }
            VirtualChannelMessage::DevicePassthrough { device_id, data } => {
                self.device_handler
                    .handle_device_data(&device_id, &data)
                    .await?;
            }
            VirtualChannelMessage::RequestElevation => {
                self.elevation_handler.handle_elevation_request().await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rferram_protocol::{ClipboardPayload, WireMessage};
    use simply_transfer_crypto::hash::compute_sha256_bytes;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_route_channel_message_all_variants() {
        let manager = VirtualChannelManager::new();

        // 1. VirtualChannelMessage::Clipboard (Text, Image, and Files payloads)
        manager
            .route_channel_message(VirtualChannelMessage::Clipboard(ClipboardPayload::Text(
                "rFerram virtual channel clipboard test".to_string(),
            )))
            .await
            .expect("Routing ClipboardPayload::Text should succeed");

        manager
            .route_channel_message(VirtualChannelMessage::Clipboard(ClipboardPayload::Image {
                width: 2,
                height: 2,
                format: "raw/rgba8".to_string(),
                data: vec![255u8; 16],
            }))
            .await
            .expect("Routing ClipboardPayload::Image should succeed");

        manager
            .route_channel_message(VirtualChannelMessage::Clipboard(ClipboardPayload::Files(
                vec!["/srv/devshare/report.pdf".to_string()],
            )))
            .await
            .expect("Routing ClipboardPayload::Files should succeed");

        // 2. VirtualChannelMessage::SimplyTransfer (init header + raw chunk)
        let (st_tx, mut st_rx) = mpsc::channel(4);
        let transfer_id = manager
            .st_handler
            .initiate_snapshot_transfer("/tmp".to_string(), "/var/tmp/dest".to_string(), st_tx)
            .await
            .expect("Snapshot transfer initiation should succeed");

        let init_wire = st_rx
            .recv()
            .await
            .expect("Should receive ST_INIT wire frame");
        let WireMessage::Channel(st_init_msg) = init_wire else {
            panic!("Expected WireMessage::Channel for SimplyTransfer init");
        };
        manager
            .route_channel_message(st_init_msg)
            .await
            .expect("Routing VirtualChannelMessage::SimplyTransfer init should succeed");

        let chunk_payload = b"snapshot-file-chunk-bytes-0123456789".to_vec();
        let expected_chunk_sha = compute_sha256_bytes(&chunk_payload);
        manager
            .route_channel_message(VirtualChannelMessage::SimplyTransfer(chunk_payload.clone()))
            .await
            .expect("Routing VirtualChannelMessage::SimplyTransfer chunk should succeed");

        let active_sessions = manager.st_handler.active_sessions().await;
        assert_eq!(active_sessions.len(), 1);
        assert_eq!(active_sessions[0].transfer_id, transfer_id);
        assert_eq!(
            active_sessions[0].transferred_bytes,
            chunk_payload.len() as u64
        );
        assert_eq!(
            active_sessions[0].sha256_digest.as_deref(),
            Some(expected_chunk_sha.as_str())
        );

        // 3. VirtualChannelMessage::DevicePassthrough
        let usb_payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x42];
        manager
            .route_channel_message(VirtualChannelMessage::DevicePassthrough {
                device_id: "smartcard-reader-0".to_string(),
                data: usb_payload.clone(),
            })
            .await
            .expect("Routing VirtualChannelMessage::DevicePassthrough should succeed");

        let device_frames = manager.device_handler.recorded_frames().await;
        assert_eq!(device_frames.len(), 1);
        assert_eq!(device_frames[0].device_id, "smartcard-reader-0");
        assert_eq!(device_frames[0].payload_bytes, usb_payload.len());
        assert_eq!(
            device_frames[0].sha256_digest,
            compute_sha256_bytes(&usb_payload)
        );

        // 4. VirtualChannelMessage::RequestElevation
        assert_eq!(manager.elevation_handler.requests_received(), 0);
        manager
            .route_channel_message(VirtualChannelMessage::RequestElevation)
            .await
            .expect("Routing VirtualChannelMessage::RequestElevation should succeed");
        assert_eq!(manager.elevation_handler.requests_received(), 1);
    }
}
