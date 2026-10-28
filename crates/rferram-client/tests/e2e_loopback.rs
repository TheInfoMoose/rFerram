//! End-to-End Local QUIC Loopback Integration Tests (`rferram-client` <-> `rferram-net` <-> Host Session Engine).
//!
//! Validates:
//! 1. [`QuicClientTransport`] and [`ClientSessionPipeline`] / [`QuicSessionBridge`] connecting to
//!    [`QuicServerListener`] and [`HostSessionEngine`] over local `127.0.0.1:0` QUIC loopback.
//! 2. TLS 1.3 leaf certificate SHA-256 fingerprint capture and [`NistVault`] Ed25519 channel-bound
//!    challenge signing/verification (`ClientHello` -> `AuthRequest` -> `AuthResponse` -> `SessionState::Active`),
//!    including MITM certificate mismatch rejection.
//! 3. Unidirectional QUIC stream video frame delivery (`VideoFrameMessage` -> `DecodedFrameSurface`),
//!    sequence gap detection (`ControlMessage::LossReport`), and instant IDR keyframe recovery.
//! 4. Ultra-low-latency QUIC UDP datagram feedback (`ControlMessage::FrameAck` and `InputEvent`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rferram_client::{
    ClientSessionPipeline, DecodedFrameSurface, QuicSessionBridge, SessionConfig, SessionState,
};
use rferram_common::NistVault;
use rferram_net::{
    QuicClientTransport, QuicConnectionManager, QuicServerListener, QuicTransportConfig,
};
use rferram_protocol::{
    AuthMethod, ControlMessage, ElementState, HandshakeMessage, InputEvent, MouseButtonCode,
    PixelFormat, VideoCodec, VideoFrameMessage, WireMessage,
};
use tokio::sync::{Mutex, mpsc};

/// Header length for `AuthMethod::Ed25519Signature` credentials:
/// `[verifying_key: 32][tls_cert_sha256: 32][signature: 64][challenge: >= 1]`.
const ED25519_AUTH_HEADER_BYTES: usize = 32 + 32 + 64;

/// Host-side session engine driving handshake verification, video frame encoding,
/// keyframe recovery, and datagram `FrameAck` / `InputEvent` ingestion over a
/// [`QuicConnectionManager`].
pub struct HostSessionEngine {
    pub session_id: String,
    pub server_cert_sha256: [u8; 32],
    pub force_keyframe_flag: Arc<AtomicBool>,
    pub is_active: Arc<AtomicBool>,
    pub frame_counter: AtomicU64,
    pub acked_frames: Arc<Mutex<Vec<(u64, u32)>>>,
    pub injected_inputs: Arc<Mutex<Vec<InputEvent>>>,
}

impl HostSessionEngine {
    /// Creates a new [`HostSessionEngine`] bound to the listener's TLS 1.3 certificate SHA-256 digest.
    pub fn new(session_id: impl Into<String>, server_cert_sha256: [u8; 32]) -> Self {
        Self {
            session_id: session_id.into(),
            server_cert_sha256,
            force_keyframe_flag: Arc::new(AtomicBool::new(true)),
            is_active: Arc::new(AtomicBool::new(true)),
            frame_counter: AtomicU64::new(0),
            acked_frames: Arc::new(Mutex::new(Vec::new())),
            injected_inputs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Builds a channel-bound `AuthMethod::Ed25519Signature` [`HandshakeMessage::AuthRequest`].
    pub fn build_ed25519_auth_request(
        verifying_key: &[u8; 32],
        tls_cert_sha256: &[u8; 32],
        challenge: &[u8],
        signature: &[u8],
    ) -> HandshakeMessage {
        let mut credentials = Vec::with_capacity(ED25519_AUTH_HEADER_BYTES + challenge.len());
        credentials.extend_from_slice(verifying_key);
        credentials.extend_from_slice(tls_cert_sha256);
        credentials.extend_from_slice(signature);
        credentials.extend_from_slice(challenge);

        HandshakeMessage::AuthRequest {
            auth_method: AuthMethod::Ed25519Signature,
            credentials,
        }
    }

    /// Validates incoming [`HandshakeMessage::ClientHello`] and channel-bound
    /// [`HandshakeMessage::AuthRequest`] messages against the host's TLS 1.3 leaf certificate digest.
    pub fn handle_handshake(&self, handshake: &HandshakeMessage) -> Result<HandshakeMessage> {
        match handshake {
            HandshakeMessage::ClientHello {
                protocol_version,
                client_id,
                supported_codecs,
                auth_methods,
                ..
            } => {
                if *protocol_version == 0 {
                    bail!("Unsupported protocol version 0");
                }
                if client_id.trim().is_empty() {
                    bail!("ClientHello client_id must not be empty");
                }
                if supported_codecs.is_empty() || auth_methods.is_empty() {
                    bail!("ClientHello must advertise codecs and auth methods");
                }
                Ok(HandshakeMessage::ServerHello {
                    server_version: 1,
                    server_id: self.session_id.clone(),
                    selected_codec: supported_codecs[0],
                    auth_challenge: b"rferram-e2e-loopback-challenge-32b".to_vec(),
                })
            }
            HandshakeMessage::AuthRequest {
                auth_method: AuthMethod::Ed25519Signature,
                credentials,
            } => {
                if credentials.len() <= ED25519_AUTH_HEADER_BYTES {
                    bail!("Ed25519 AuthRequest credentials too short");
                }
                let verifying_key: [u8; 32] = credentials[0..32]
                    .try_into()
                    .map_err(|_| anyhow!("Invalid verifying key slice"))?;
                let presented_cert_sha256: [u8; 32] = credentials[32..64]
                    .try_into()
                    .map_err(|_| anyhow!("Invalid cert hash slice"))?;
                let signature = &credentials[64..128];
                let challenge = &credentials[128..];

                if presented_cert_sha256 != self.server_cert_sha256 {
                    bail!("TLS 1.3 channel-binding certificate SHA-256 mismatch");
                }

                NistVault::verify_channel_bound_challenge(
                    &verifying_key,
                    challenge,
                    &self.server_cert_sha256,
                    signature,
                )
                .context("Ed25519 channel-bound challenge verification failed")?;

                Ok(HandshakeMessage::AuthResponse {
                    success: true,
                    session_token: Some(format!("session-token-{}", self.session_id)),
                    requires_elevation: false,
                    error_message: None,
                })
            }
            other => bail!("Unexpected handshake message: {other:?}"),
        }
    }

    /// Encodes and transmits the next video frame over a QUIC unidirectional stream.
    pub async fn send_encoded_video_frame(
        &self,
        conn: &QuicConnectionManager,
        frame_id_override: Option<u64>,
        width: u32,
        height: u32,
    ) -> Result<VideoFrameMessage> {
        let seq = self.frame_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let frame_id = frame_id_override.unwrap_or(seq);
        let forced = self.force_keyframe_flag.swap(false, Ordering::SeqCst);
        let is_keyframe = forced || frame_id == 1;

        let frame = VideoFrameMessage {
            frame_id,
            monitor_id: 1,
            timestamp_us: frame_id * 16_666,
            codec: VideoCodec::AV1,
            is_keyframe,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Bgra8,
            payload: vec![(frame_id as u8) ^ 0xA5; 1024],
        };

        conn.send_uni_message(&WireMessage::Video(frame.clone()))
            .await
            .context("HostSessionEngine failed to send VideoFrameMessage over uni stream")?;

        Ok(frame)
    }

    /// Processes an incoming control or input message from the client.
    pub async fn handle_incoming_wire_message(&self, msg: WireMessage) -> Result<()> {
        match msg {
            WireMessage::Control(ControlMessage::FrameAck {
                frame_id,
                render_delay_us,
                ..
            }) => {
                self.acked_frames
                    .lock()
                    .await
                    .push((frame_id, render_delay_us));
            }
            WireMessage::Control(ControlMessage::LossReport { missing_frames }) => {
                if !missing_frames.is_empty() {
                    self.force_keyframe_flag.store(true, Ordering::SeqCst);
                }
            }
            WireMessage::Control(ControlMessage::Disconnect { .. }) => {
                self.is_active.store(false, Ordering::SeqCst);
            }
            WireMessage::Input(event) => {
                self.injected_inputs.lock().await.push(event);
            }
            _ => {}
        }
        Ok(())
    }
}

#[tokio::test]
async fn test_e2e_quic_loopback_ed25519_handshake_video_and_frame_ack() {
    // 1. Bind QuicServerListener on local loopback ephemeral port
    let transport_cfg = QuicTransportConfig::default();
    let listener = QuicServerListener::bind("127.0.0.1:0".parse().unwrap(), transport_cfg.clone())
        .expect("Failed to bind QuicServerListener on 127.0.0.1:0");
    let server_addr = listener.local_addr().expect("Failed to query local_addr");
    let expected_server_cert_sha256 = listener.server_cert_sha256;

    let host_engine = Arc::new(HostSessionEngine::new(
        "host-e2e-session-01",
        expected_server_cert_sha256,
    ));
    let host_engine_for_server = host_engine.clone();

    // 2. Spawn the Host Server task managing QuicServerListener + HostSessionEngine
    let server_task = tokio::spawn(async move {
        let server_conn = listener
            .accept()
            .await
            .expect("QuicServerListener failed to accept client connection");

        assert_eq!(
            server_conn.peer_cert_sha256(),
            Some(expected_server_cert_sha256)
        );

        // Step A: Receive initial ClientHello emitted automatically by ClientSessionPipeline::spawn
        let hello_wire = server_conn
            .recv_uni_message()
            .await
            .expect("Server failed to receive initial ClientHello");
        let WireMessage::Handshake(client_hello) = hello_wire else {
            panic!("Expected WireMessage::Handshake(ClientHello), got {hello_wire:?}");
        };
        let server_hello = host_engine_for_server
            .handle_handshake(&client_hello)
            .expect("HostSessionEngine failed to validate ClientHello");
        server_conn
            .send_uni_message(&WireMessage::Handshake(server_hello))
            .await
            .expect("Server failed to send ServerHello");

        // Step B: Receive channel-bound Ed25519 AuthRequest from client and verify via NistVault
        let auth_wire = server_conn
            .recv_uni_message()
            .await
            .expect("Server failed to receive Ed25519 AuthRequest");
        let WireMessage::Handshake(auth_req) = auth_wire else {
            panic!("Expected WireMessage::Handshake(AuthRequest), got {auth_wire:?}");
        };
        let auth_resp = host_engine_for_server
            .handle_handshake(&auth_req)
            .expect("HostSessionEngine failed to verify channel-bound Ed25519 AuthRequest");
        server_conn
            .send_uni_message(&WireMessage::Handshake(auth_resp))
            .await
            .expect("Server failed to send AuthResponse");

        // Step C: Stream VideoFrame #1 (initial IDR keyframe) and await FrameAck #1 datagram
        let sent_f1 = host_engine_for_server
            .send_encoded_video_frame(&server_conn, Some(1), 2560, 1440)
            .await
            .expect("Failed to send frame 1");
        assert!(sent_f1.is_keyframe, "Frame 1 must be an IDR keyframe");

        let ack_1 = server_conn
            .recv_datagram()
            .await
            .expect("Server failed to receive FrameAck #1 datagram");
        host_engine_for_server
            .handle_incoming_wire_message(ack_1)
            .await
            .unwrap();

        // Step D: Stream VideoFrame #2 (normal inter-frame) and await FrameAck #2 datagram
        let sent_f2 = host_engine_for_server
            .send_encoded_video_frame(&server_conn, Some(2), 2560, 1440)
            .await
            .expect("Failed to send frame 2");
        assert!(!sent_f2.is_keyframe, "Frame 2 should be an inter-frame");

        let ack_2 = server_conn
            .recv_datagram()
            .await
            .expect("Server failed to receive FrameAck #2 datagram");
        host_engine_for_server
            .handle_incoming_wire_message(ack_2)
            .await
            .unwrap();

        // Step E: Simulate packet loss by sending VideoFrame #4 (skipping #3).
        // ClientSessionPipeline must emit LossReport { missing_frames: [3] } on a uni stream
        // AND FrameAck #4 on a datagram.
        let sent_f4 = host_engine_for_server
            .send_encoded_video_frame(&server_conn, Some(4), 2560, 1440)
            .await
            .expect("Failed to send frame 4");
        assert!(!sent_f4.is_keyframe);

        let loss_msg = server_conn
            .recv_uni_message()
            .await
            .expect("Server failed to receive LossReport over uni stream");
        assert_eq!(
            loss_msg,
            WireMessage::Control(ControlMessage::LossReport {
                missing_frames: vec![3],
            })
        );
        host_engine_for_server
            .handle_incoming_wire_message(loss_msg)
            .await
            .unwrap();
        assert!(
            host_engine_for_server
                .force_keyframe_flag
                .load(Ordering::SeqCst),
            "LossReport must set force_keyframe_flag on HostSessionEngine"
        );

        let ack_4 = server_conn
            .recv_datagram()
            .await
            .expect("Server failed to receive FrameAck #4 datagram");
        host_engine_for_server
            .handle_incoming_wire_message(ack_4)
            .await
            .unwrap();

        // Step F: Send VideoFrame #5 and verify it is forced to be an IDR keyframe
        let sent_f5 = host_engine_for_server
            .send_encoded_video_frame(&server_conn, Some(5), 2560, 1440)
            .await
            .expect("Failed to send recovery frame 5");
        assert!(
            sent_f5.is_keyframe,
            "Frame 5 following LossReport must be forced to an IDR keyframe"
        );

        let ack_5 = server_conn
            .recv_datagram()
            .await
            .expect("Server failed to receive FrameAck #5 datagram");
        host_engine_for_server
            .handle_incoming_wire_message(ack_5)
            .await
            .unwrap();

        // Step G: Receive client peripheral InputEvent over QUIC datagram
        let input_dgram = server_conn
            .recv_datagram()
            .await
            .expect("Server failed to receive InputEvent datagram");
        host_engine_for_server
            .handle_incoming_wire_message(input_dgram)
            .await
            .unwrap();

        (listener, server_conn)
    });

    // 3. Connect QuicClientTransport to QuicServerListener
    let client_transport = QuicClientTransport::new(transport_cfg);
    let client_conn = Arc::new(
        client_transport
            .connect(server_addr, "rferram.host")
            .await
            .expect("QuicClientTransport failed to connect to QuicServerListener"),
    );

    // Validate captured peer TLS 1.3 certificate SHA-256 digest matches server certificate
    let captured_cert_sha256 = client_conn
        .peer_cert_sha256()
        .expect("Client must capture peer TLS 1.3 leaf certificate SHA-256 digest");
    assert_eq!(captured_cert_sha256, expected_server_cert_sha256);

    // 4. Spawn QuicSessionBridge (wrapping ClientSessionPipeline)
    let session_cfg = SessionConfig {
        profile_id: String::from("e2e-loopback-profile"),
        host_address: server_addr.to_string(),
        preferred_video_codec: VideoCodec::AV1,
        ..Default::default()
    };
    let (bridge, mut state_rx, mut video_rx, _input_tx) =
        QuicSessionBridge::spawn(session_cfg, client_conn.clone());

    // 5. Generate client Ed25519 identity via NistVault and sign TLS channel-bound challenge
    let vault = NistVault::new("test.rferram.client.e2e_loopback");
    let (signing_key, verifying_key) = vault
        .get_or_create_ed25519_identity("e2e_loopback_viewer")
        .expect("Failed to create Ed25519 identity in NistVault");
    let challenge = b"rferram-e2e-loopback-challenge-32b";
    let signature =
        NistVault::sign_channel_bound_challenge(&signing_key, challenge, &captured_cert_sha256);

    // Also verify that a tampered/MITM certificate fingerprint is rejected by HostSessionEngine
    let mitm_req = HostSessionEngine::build_ed25519_auth_request(
        &verifying_key.to_bytes(),
        &[0xFF; 32],
        challenge,
        &signature,
    );
    assert!(
        host_engine.handle_handshake(&mitm_req).is_err(),
        "Mismatched TLS certificate SHA-256 must fail channel-bound verification"
    );

    // Send valid channel-bound Ed25519 AuthRequest through ClientSessionPipeline's wire_tx
    let valid_auth_req = HostSessionEngine::build_ed25519_auth_request(
        &verifying_key.to_bytes(),
        &captured_cert_sha256,
        challenge,
        &signature,
    );
    bridge
        .pipeline
        .wire_tx
        .send(WireMessage::Handshake(valid_auth_req))
        .await
        .expect("Failed to enqueue Ed25519 AuthRequest on ClientSessionPipeline");

    // 6. Wait for ClientSessionPipeline to transition to SessionState::Active upon AuthResponse
    tokio::time::timeout(Duration::from_secs(5), async {
        while *state_rx.borrow_and_update() != SessionState::Active {
            state_rx.changed().await.unwrap();
        }
    })
    .await
    .expect("Timed out waiting for SessionState::Active");
    assert_eq!(*state_rx.borrow(), SessionState::Active);

    // 7. Receive all 4 delivered video frames (#1, #2, #4, #5) from ClientSessionPipeline
    let mut received_frames: Vec<DecodedFrameSurface> = Vec::with_capacity(4);
    for _ in 0..4 {
        let surface = tokio::time::timeout(Duration::from_secs(5), video_rx.recv())
            .await
            .expect("Timed out waiting for DecodedFrameSurface")
            .expect("video_rx channel closed unexpectedly");
        received_frames.push(surface);
    }

    let received_ids: Vec<u64> = received_frames.iter().map(|f| f.frame_index).collect();
    assert_eq!(received_ids, vec![1, 2, 4, 5]);
    for surface in &received_frames {
        assert_eq!(surface.width, 2560);
        assert_eq!(surface.height, 1440);
        assert_eq!(surface.format, PixelFormat::Bgra8);
        assert_eq!(surface.data.len(), 1024);
    }

    // 8. Dispatch a mouse button input event from ClientSessionPipeline over QUIC datagram
    bridge
        .pipeline
        .send_mouse_button(MouseButtonCode::Left, ElementState::Pressed, 1280.0, 720.0)
        .await;

    // Wait for server task to complete all verification steps
    let (_listener, server_conn) = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("Server task timed out")
        .expect("Server task panicked");

    // 9. Validate FrameAck datagrams and InputEvents recorded by HostSessionEngine
    let acked = host_engine.acked_frames.lock().await;
    let acked_ids: Vec<u64> = acked.iter().map(|(id, _)| *id).collect();
    assert_eq!(acked_ids, vec![1, 2, 4, 5]);

    let inputs = host_engine.injected_inputs.lock().await;
    assert_eq!(inputs.len(), 1);
    assert_eq!(
        inputs[0],
        InputEvent::MouseButton {
            button: MouseButtonCode::Left,
            state: ElementState::Pressed,
            x: 1280.0,
            y: 720.0,
        }
    );

    // 10. Validate ClientSessionPipeline and QUIC transport metrics
    let client_metrics = bridge.pipeline.metrics.lock().await;
    assert_eq!(client_metrics.total_video_frames_received, 4);
    assert_eq!(client_metrics.total_bytes_received, 4 * 1024);
    drop(client_metrics);

    // 4 FrameAck datagrams + 1 InputEvent datagram = 5 datagrams sent by client
    assert_eq!(
        client_conn.metrics.datagrams_sent.load(Ordering::Relaxed),
        5
    );
    assert_eq!(
        server_conn
            .metrics
            .datagrams_received
            .load(Ordering::Relaxed),
        5
    );

    // Also verify direct ClientSessionPipeline::spawn wiring with mpsc channels
    let (h2c_tx, h2c_rx) = mpsc::channel(8);
    let (c2h_tx, mut c2h_rx) = mpsc::channel(8);
    let (_direct_pipeline, _direct_state, _direct_video, _direct_input) =
        ClientSessionPipeline::spawn(SessionConfig::default(), h2c_rx, c2h_tx);
    let direct_hello = c2h_rx.recv().await.expect("Expected direct ClientHello");
    assert!(matches!(
        direct_hello,
        WireMessage::Handshake(HandshakeMessage::ClientHello { .. })
    ));
    drop(h2c_tx);

    bridge.stop();
    assert!(!bridge.is_active());
}
