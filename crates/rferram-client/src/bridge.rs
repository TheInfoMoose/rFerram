//! QUIC Transport Session Bridge (`QuicSessionBridge`).
//!
//! Connects [`ClientSessionPipeline`] `mpsc` channels directly to [`rferram_net::QuicConnectionManager`],
//! routing ultra-low-latency `WireMessage::Input` and `ControlMessage::FrameAck` packets over
//! unordered QUIC UDP datagrams (`send_datagram` / `recv_datagram`) and length-prefixed
//! `Handshake`, `Topology`, `Video`, `Audio`, `Channel`, and reliable `Control` messages
//! (such as `LossReport`) over QUIC unidirectional streams (`send_uni_message` / `recv_uni_message`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use rferram_net::QuicConnectionManager;
use rferram_protocol::{ControlMessage, InputEvent, WireMessage};
use tokio::sync::{mpsc, watch};
use tracing::{debug, warn};

use crate::render::DecodedFrameSurface;
use crate::session::{ClientSessionPipeline, SessionConfig, SessionState};

/// Default channel depth for inbound/outbound wire messages bridged between
/// [`ClientSessionPipeline`] and [`QuicConnectionManager`].
pub const BRIDGE_WIRE_CHANNEL_CAPACITY: usize = 256;

/// Transport bridge coupling a [`ClientSessionPipeline`] to an active [`QuicConnectionManager`].
pub struct QuicSessionBridge {
    pub pipeline: ClientSessionPipeline,
    pub connection: Arc<QuicConnectionManager>,
    pub is_running: Arc<AtomicBool>,
}

impl QuicSessionBridge {
    /// Returns `true` if `msg` must be transmitted as an unframed QUIC UDP datagram
    /// (`WireMessage::Input` and `ControlMessage::FrameAck`), or `false` if it must be
    /// length-prefixed over a QUIC stream.
    pub const fn is_datagram_message(msg: &WireMessage) -> bool {
        matches!(
            msg,
            WireMessage::Input(_) | WireMessage::Control(ControlMessage::FrameAck { .. })
        )
    }

    /// Dispatches a single outbound [`WireMessage`] over `connection`, routing datagram-eligible
    /// messages via [`QuicConnectionManager::send_datagram`] and stream-framed messages via
    /// [`QuicConnectionManager::send_uni_message`].
    pub async fn dispatch_outbound(
        connection: &QuicConnectionManager,
        msg: &WireMessage,
    ) -> Result<()> {
        if Self::is_datagram_message(msg) {
            connection
                .send_datagram(msg)
                .context("Failed to transmit outbound QUIC datagram message")
        } else {
            connection
                .send_uni_message(msg)
                .await
                .context("Failed to transmit outbound QUIC unidirectional stream message")
        }
    }

    /// Spawns a [`ClientSessionPipeline`] and binds its inbound/outbound wire channels to
    /// the provided [`QuicConnectionManager`].
    pub fn spawn(
        config: SessionConfig,
        connection: Arc<QuicConnectionManager>,
    ) -> (
        Self,
        watch::Receiver<SessionState>,
        mpsc::Receiver<DecodedFrameSurface>,
        mpsc::Sender<InputEvent>,
    ) {
        let (inbound_tx, inbound_rx) = mpsc::channel(BRIDGE_WIRE_CHANNEL_CAPACITY);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(BRIDGE_WIRE_CHANNEL_CAPACITY);

        let (pipeline, state_rx, video_rx, input_tx) =
            ClientSessionPipeline::spawn(config, inbound_rx, outbound_tx);

        let is_running = Arc::new(AtomicBool::new(true));

        // Task 1: Outbound pump (ClientSessionPipeline -> QuicConnectionManager)
        let conn_outbound = connection.clone();
        let running_outbound = is_running.clone();
        tokio::spawn(async move {
            while running_outbound.load(Ordering::Relaxed) {
                let Some(msg) = outbound_rx.recv().await else {
                    break;
                };
                if let Err(e) = Self::dispatch_outbound(&conn_outbound, &msg).await {
                    debug!(error = %e, "Outbound QUIC bridge dispatch terminated");
                    break;
                }
            }
            running_outbound.store(false, Ordering::Relaxed);
        });

        // Task 2: Inbound QUIC datagram receiver loop (QuicConnectionManager -> ClientSessionPipeline)
        let conn_dgram = connection.clone();
        let inbound_tx_dgram = inbound_tx.clone();
        let running_dgram = is_running.clone();
        tokio::spawn(async move {
            while running_dgram.load(Ordering::Relaxed) {
                match conn_dgram.recv_datagram().await {
                    Ok(msg) => {
                        if inbound_tx_dgram.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        debug!(error = %e, "Inbound QUIC datagram loop ended");
                        break;
                    }
                }
            }
        });

        // Task 3: Inbound QUIC unidirectional stream receiver loop (QuicConnectionManager -> ClientSessionPipeline)
        let conn_uni = connection.clone();
        let inbound_tx_uni = inbound_tx;
        let running_uni = is_running.clone();
        tokio::spawn(async move {
            while running_uni.load(Ordering::Relaxed) {
                match conn_uni.recv_uni_message().await {
                    Ok(msg) => {
                        if inbound_tx_uni.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        if running_uni.load(Ordering::Relaxed) {
                            warn!(error = %e, "Inbound QUIC unidirectional stream loop ended");
                        }
                        break;
                    }
                }
            }
        });

        let bridge = Self {
            pipeline,
            connection,
            is_running,
        };

        (bridge, state_rx, video_rx, input_tx)
    }

    /// Returns the peer's TLS 1.3 leaf certificate SHA-256 digest captured during the QUIC handshake.
    pub fn peer_cert_sha256(&self) -> Option<[u8; 32]> {
        self.connection.peer_cert_sha256()
    }

    /// Returns `true` if the bridge pump tasks are active.
    pub fn is_active(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Signals all bridge pump tasks to stop.
    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rferram_net::{QuicClientTransport, QuicServerListener, QuicTransportConfig};
    use rferram_protocol::{
        AudioCodec, AudioDirection, AudioFrameMessage, ElementState, HandshakeMessage,
        MouseButtonCode, PixelFormat, VideoCodec, VideoFrameMessage,
    };

    #[test]
    fn test_datagram_vs_stream_message_classification() {
        let input_msg = WireMessage::Input(InputEvent::MouseMove {
            monitor_id: 1,
            x: 100.0,
            y: 200.0,
            is_absolute: true,
        });
        assert!(QuicSessionBridge::is_datagram_message(&input_msg));

        let ack_msg = WireMessage::Control(ControlMessage::FrameAck {
            frame_id: 7,
            monitor_id: 1,
            render_delay_us: 320,
        });
        assert!(QuicSessionBridge::is_datagram_message(&ack_msg));

        let loss_msg = WireMessage::Control(ControlMessage::LossReport {
            missing_frames: vec![5, 6],
        });
        assert!(!QuicSessionBridge::is_datagram_message(&loss_msg));

        let hello_msg = WireMessage::Handshake(HandshakeMessage::ClientHello {
            protocol_version: 1,
            client_id: String::from("c1"),
            client_name: String::from("test"),
            supported_codecs: vec![VideoCodec::AV1],
            auth_methods: vec![],
        });
        assert!(!QuicSessionBridge::is_datagram_message(&hello_msg));

        let audio_msg = WireMessage::Audio(AudioFrameMessage {
            stream_id: 1,
            timestamp_us: 20_000,
            direction: AudioDirection::HostToClientSpeaker,
            codec: AudioCodec::Opus,
            channels: 2,
            sample_rate: 48_000,
            payload: vec![0x11; 192],
        });
        assert!(!QuicSessionBridge::is_datagram_message(&audio_msg));
    }

    #[tokio::test]
    async fn test_quic_session_bridge_end_to_end_loopback() {
        let listener = QuicServerListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            QuicTransportConfig::default(),
        )
        .expect("Failed to bind QUIC server listener");
        let server_addr = listener.local_addr().unwrap();

        let server_handle = tokio::spawn(async move {
            let server_conn = listener.accept().await.expect("Server failed to accept");

            // 1. Receive ClientHello on unidirectional stream
            let hello = server_conn
                .recv_uni_message()
                .await
                .expect("Server failed to receive ClientHello");
            assert!(matches!(
                hello,
                WireMessage::Handshake(HandshakeMessage::ClientHello { .. })
            ));

            // 2. Send AuthResponse { success: true } over unidirectional stream
            server_conn
                .send_uni_message(&WireMessage::Handshake(HandshakeMessage::AuthResponse {
                    success: true,
                    session_token: Some(String::from("bridge-tok-01")),
                    error_message: None,
                    requires_elevation: false,
                }))
                .await
                .expect("Server failed to send AuthResponse");

            // 3. Send VideoFrame #1 over unidirectional stream and expect FrameAck over datagram
            server_conn
                .send_uni_message(&WireMessage::Video(VideoFrameMessage {
                    frame_id: 1,
                    monitor_id: 1,
                    timestamp_us: 16_666,
                    codec: VideoCodec::AV1,
                    is_keyframe: true,
                    width: 1280,
                    height: 720,
                    stride: 1280 * 4,
                    format: PixelFormat::Bgra8,
                    payload: vec![0xAA; 256],
                }))
                .await
                .expect("Server failed to send VideoFrame #1");

            let ack_dgram = server_conn
                .recv_datagram()
                .await
                .expect("Server failed to receive FrameAck datagram");
            assert!(matches!(
                ack_dgram,
                WireMessage::Control(ControlMessage::FrameAck { frame_id: 1, .. })
            ));

            // 4. Send VideoFrame #3 (skipping #2) to trigger LossReport on unidirectional stream
            server_conn
                .send_uni_message(&WireMessage::Video(VideoFrameMessage {
                    frame_id: 3,
                    monitor_id: 1,
                    timestamp_us: 49_998,
                    codec: VideoCodec::AV1,
                    is_keyframe: false,
                    width: 1280,
                    height: 720,
                    stride: 1280 * 4,
                    format: PixelFormat::Bgra8,
                    payload: vec![0xBB; 256],
                }))
                .await
                .expect("Server failed to send VideoFrame #3");

            let loss_uni = server_conn
                .recv_uni_message()
                .await
                .expect("Server failed to receive LossReport over uni stream");
            assert_eq!(
                loss_uni,
                WireMessage::Control(ControlMessage::LossReport {
                    missing_frames: vec![2],
                })
            );

            // Drain FrameAck #3 datagram
            let ack_3 = server_conn
                .recv_datagram()
                .await
                .expect("Server failed to receive FrameAck #3 datagram");
            assert!(matches!(
                ack_3,
                WireMessage::Control(ControlMessage::FrameAck { frame_id: 3, .. })
            ));

            // 5. Send speaker AudioFrameMessage over unidirectional stream
            server_conn
                .send_uni_message(&WireMessage::Audio(AudioFrameMessage {
                    stream_id: 9,
                    timestamp_us: 20_000,
                    direction: AudioDirection::HostToClientSpeaker,
                    codec: AudioCodec::Opus,
                    channels: 2,
                    sample_rate: 48_000,
                    payload: vec![0x7F; 192],
                }))
                .await
                .expect("Server failed to send AudioFrameMessage");

            // 6. Receive client mouse input over QUIC datagram
            let input_dgram = server_conn
                .recv_datagram()
                .await
                .expect("Server failed to receive InputEvent datagram");

            (listener, server_conn, input_dgram)
        });

        let client_transport = QuicClientTransport::new(QuicTransportConfig::default());
        let client_conn = Arc::new(
            client_transport
                .connect(server_addr, "rferram.host")
                .await
                .expect("Client failed to connect to QUIC listener"),
        );

        let (bridge, mut state_rx, mut video_rx, _input_tx) =
            QuicSessionBridge::spawn(SessionConfig::default(), client_conn);

        assert!(bridge.is_active());
        assert!(bridge.peer_cert_sha256().is_some());

        // Wait for session state to transition to Active upon AuthResponse
        while *state_rx.borrow_and_update() != SessionState::Active {
            state_rx.changed().await.unwrap();
        }
        assert_eq!(*state_rx.borrow(), SessionState::Active);

        // Receive both decoded video frames (#1 and #3)
        let f1 = video_rx.recv().await.unwrap();
        assert_eq!(f1.frame_index, 1);
        let f3 = video_rx.recv().await.unwrap();
        assert_eq!(f3.frame_index, 3);

        // Send a mouse button input event from the client pipeline (routed via QUIC datagram)
        bridge
            .pipeline
            .send_mouse_button(MouseButtonCode::Left, ElementState::Pressed, 640.0, 360.0)
            .await;

        let (_listener, _server_conn, received_input) = server_handle.await.unwrap();
        assert_eq!(
            received_input,
            WireMessage::Input(InputEvent::MouseButton {
                button: MouseButtonCode::Left,
                state: ElementState::Pressed,
                x: 640.0,
                y: 360.0,
            })
        );

        // Poll briefly for the asynchronous audio frame to be pushed into `pipeline.audio_buffer`
        for _ in 0..20 {
            if !bridge.pipeline.audio_buffer.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let mut audio_buf = bridge.pipeline.audio_buffer.lock().await;
        assert_eq!(audio_buf.len(), 1);
        let popped_audio = audio_buf.pop_frame().unwrap();
        assert_eq!(popped_audio.stream_id, 9);
        assert_eq!(popped_audio.payload.len(), 192);
        drop(audio_buf);

        bridge.stop();
        assert!(!bridge.is_active());
    }
}
