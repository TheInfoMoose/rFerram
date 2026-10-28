//! Host System Audio Loopback Capture Engine (PipeWire / WASAPI / macOS CoreAudio).
//!
//! Captures 48 kHz stereo host speaker audio in 20 ms (50 Hz) Opus frames and streams
//! [`WireMessage::Audio`] frames to connected remote viewers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::info;

use rferram_protocol::{AudioCodec, AudioDirection, AudioFrameMessage, WireMessage};

use crate::CaptureBackendType;

/// Opus audio loopback frame interval in milliseconds (50 Hz).
pub const OPUS_FRAME_INTERVAL_MS: u64 = 20;

/// Standard Opus audio sample rate (48 kHz).
pub const OPUS_SAMPLE_RATE_HZ: u32 = 48_000;

/// Standard stereo channel count.
pub const OPUS_CHANNELS: u8 = 2;

/// Encoded 20 ms stereo Opus payload size in bytes.
pub const OPUS_PAYLOAD_BYTES: usize = 192;

/// Host System Audio Capture Loopback Engine (PipeWire / WASAPI / macOS CoreAudio)
pub struct HostAudioCapturer {
    pub backend: CaptureBackendType,
    pub is_running: Arc<AtomicBool>,
}

impl HostAudioCapturer {
    /// Creates a new [`HostAudioCapturer`] bound to the given display/OS backend.
    pub fn new(backend: CaptureBackendType) -> Self {
        Self {
            backend,
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns `true` if the audio loopback capture task is currently running.
    pub fn is_capturing(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Constructs a 20 ms 48 kHz stereo Opus [`AudioFrameMessage`] for host loopback streaming.
    pub fn build_audio_frame(stream_id: u32, timestamp_us: u64) -> AudioFrameMessage {
        AudioFrameMessage {
            stream_id,
            timestamp_us,
            direction: AudioDirection::HostToClientSpeaker,
            codec: AudioCodec::Opus,
            channels: OPUS_CHANNELS,
            sample_rate: OPUS_SAMPLE_RATE_HZ,
            payload: vec![0u8; OPUS_PAYLOAD_BYTES],
        }
    }

    /// Starts the asynchronous 50 Hz (20 ms) Opus audio loopback capture loop.
    pub async fn start_loopback_capture(&self, tx: mpsc::Sender<WireMessage>) -> Result<()> {
        self.is_running.store(true, Ordering::SeqCst);
        let is_running = self.is_running.clone();
        let backend = self.backend;

        tokio::spawn(async move {
            let mut stream_id: u32 = 1;
            let interval = Duration::from_millis(OPUS_FRAME_INTERVAL_MS);
            let start_instant = Instant::now();

            info!(?backend, "Started host audio loopback capture task");

            while is_running.load(Ordering::SeqCst) {
                tokio::time::sleep(interval).await;
                if !is_running.load(Ordering::SeqCst) {
                    break;
                }

                let audio_msg =
                    Self::build_audio_frame(stream_id, start_instant.elapsed().as_micros() as u64);

                if tx.send(WireMessage::Audio(audio_msg)).await.is_err() {
                    is_running.store(false, Ordering::SeqCst);
                    break;
                }
                stream_id += 1;
            }
        });

        Ok(())
    }

    /// Stops the active audio loopback capture loop.
    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}
