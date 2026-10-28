//! Native OS device and smartcard passthrough handler (`DevicePassthroughHandler`).

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, bail};
use simply_transfer_crypto::hash::compute_sha256_bytes;
use tokio::sync::{Mutex, mpsc};
use tracing::debug;

use rferram_protocol::{VirtualChannelMessage, WireMessage};

/// Recorded telemetry metadata for a processed device/smartcard passthrough frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePassthroughFrame {
    /// Logical peripheral or smartcard device identifier.
    pub device_id: String,
    /// Number of raw payload bytes delivered in the passthrough frame.
    pub payload_bytes: usize,
    /// Hex-encoded SHA-256 digest of the device payload.
    pub sha256_digest: String,
    /// Monotonic timestamp when the frame was ingested.
    pub received_at: Instant,
}

/// Native OS Device Passthrough Handler (Smartcards, USB redirection)
#[derive(Clone)]
pub struct DevicePassthroughHandler {
    recorded_frames: Arc<Mutex<Vec<DevicePassthroughFrame>>>,
}

impl Default for DevicePassthroughHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl DevicePassthroughHandler {
    /// Creates a new [`DevicePassthroughHandler`] instance.
    pub fn new() -> Self {
        Self {
            recorded_frames: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns a snapshot of all device passthrough frames processed by this handler.
    pub async fn recorded_frames(&self) -> Vec<DevicePassthroughFrame> {
        self.recorded_frames.lock().await.clone()
    }

    /// Returns recorded passthrough frames matching a specific `device_id`.
    pub async fn frames_for_device(&self, device_id: &str) -> Vec<DevicePassthroughFrame> {
        self.recorded_frames
            .lock()
            .await
            .iter()
            .filter(|f| f.device_id == device_id)
            .cloned()
            .collect()
    }

    /// Clears all recorded passthrough telemetry frames.
    pub async fn clear_recorded_frames(&self) {
        self.recorded_frames.lock().await.clear();
    }

    /// Emits a device passthrough payload frame over the outgoing wire transport channel.
    pub async fn send_device_data(
        &self,
        device_id: impl Into<String>,
        data: Vec<u8>,
        outgoing_tx: mpsc::Sender<WireMessage>,
    ) -> Result<()> {
        let device_id = device_id.into();
        if device_id.trim().is_empty() {
            bail!("Device passthrough identifier cannot be empty");
        }
        debug!(
            device_id = %device_id,
            bytes = data.len(),
            "Sending outbound device passthrough frame"
        );
        let msg =
            WireMessage::Channel(VirtualChannelMessage::DevicePassthrough { device_id, data });
        outgoing_tx.send(msg).await?;
        Ok(())
    }

    /// Processes an incoming device or smartcard passthrough payload frame.
    pub async fn handle_device_data(&self, device_id: &str, data: &[u8]) -> Result<()> {
        let trimmed_id = device_id.trim();
        if trimmed_id.is_empty() {
            bail!("Device passthrough identifier cannot be empty");
        }
        let sha256_digest = compute_sha256_bytes(data);
        debug!(
            device_id = %trimmed_id,
            bytes = data.len(),
            sha256 = %sha256_digest,
            "Processing device passthrough frame"
        );

        let frame = DevicePassthroughFrame {
            device_id: trimmed_id.to_string(),
            payload_bytes: data.len(),
            sha256_digest,
            received_at: Instant::now(),
        };
        self.recorded_frames.lock().await.push(frame);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_device_passthrough_send_and_handle() {
        let handler = DevicePassthroughHandler::new();
        let (tx, mut rx) = mpsc::channel(4);

        let sample_apdu = vec![0x00, 0xA4, 0x04, 0x00, 0x07, 0xA0, 0x00, 0x00, 0x03, 0x08];
        handler
            .send_device_data("yubikey-piv-slot9a", sample_apdu.clone(), tx)
            .await
            .expect("Sending device passthrough frame should succeed");

        let emitted = rx.recv().await.expect("Should receive WireMessage");
        match emitted {
            WireMessage::Channel(VirtualChannelMessage::DevicePassthrough { device_id, data }) => {
                assert_eq!(device_id, "yubikey-piv-slot9a");
                assert_eq!(data, sample_apdu);
                handler
                    .handle_device_data(&device_id, &data)
                    .await
                    .expect("Handling device passthrough data should succeed");
            }
            other => panic!("Unexpected wire message variant: {other:?}"),
        }

        let recorded = handler.recorded_frames().await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].device_id, "yubikey-piv-slot9a");
        assert_eq!(recorded[0].payload_bytes, sample_apdu.len());
        assert_eq!(
            recorded[0].sha256_digest,
            compute_sha256_bytes(&sample_apdu)
        );
    }

    #[tokio::test]
    async fn test_device_passthrough_filtering_and_validation() {
        let handler = DevicePassthroughHandler::new();
        let (tx, _rx) = mpsc::channel(4);

        assert!(
            handler
                .send_device_data("   ", vec![0x01], tx)
                .await
                .is_err(),
            "Empty device_id should fail outbound validation"
        );
        assert!(
            handler.handle_device_data("", &[0x01]).await.is_err(),
            "Empty device_id should fail inbound validation"
        );

        handler
            .handle_device_data("usb-smartcard-0", &[0xAA, 0xBB])
            .await
            .unwrap();
        handler
            .handle_device_data("usb-security-key-1", &[0xCC, 0xDD, 0xEE])
            .await
            .unwrap();
        handler
            .handle_device_data("usb-smartcard-0", &[0xFF])
            .await
            .unwrap();

        assert_eq!(handler.recorded_frames().await.len(), 3);
        let smartcard_frames = handler.frames_for_device("usb-smartcard-0").await;
        assert_eq!(smartcard_frames.len(), 2);
        assert_eq!(smartcard_frames[0].payload_bytes, 2);
        assert_eq!(smartcard_frames[1].payload_bytes, 1);

        handler.clear_recorded_frames().await;
        assert!(handler.recorded_frames().await.is_empty());
    }
}
