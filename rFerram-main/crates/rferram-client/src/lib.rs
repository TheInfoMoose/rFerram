//! `rferram-client`: Remote Viewer Session Pipeline Engine (v2 - Cross-Platform including macOS, Windows & Linux/COSMIC)
//!
//! Coordinates QUIC stream demuxing, video frame decoding (`wgpu 30.0.1` Metal / Vulkan / DirectX 12),
//! audio output playback (`cpal` / CoreAudio / WASAPI / PipeWire), native peripheral input capture,
//! dynamic feedback control loops (RTT, sequence loss & FPS monitoring), and virtual channels
//! (Clipboard & ST v2 APFS/VSS/Linux snapshots).

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{debug, error, info, warn};

use rferram_protocol::{
    ControlMessage, ElementState, HandshakeMessage, InputEvent, KeyModifiers, MouseButtonCode,
    PixelFormat, VideoCodec, WireMessage,
};

/// Built-in WGSL Shader for Zero-Copy BGRA8 / NV12 Frame Presentation on Vulkan, Metal, and DX12.
pub const VIDEO_SURFACE_SHADER_WGSL: &str = r#"
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32((in_vertex_index << 1u) & 2u);
    let y = f32(in_vertex_index & 2u);
    out.clip_position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.tex_coords = vec2<f32>(x, y);
    return out;
}

@group(0) @binding(0)
var frame_tex: texture_2d<f32>;
@group(0) @binding(1)
var frame_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(frame_tex, frame_sampler, in.tex_coords);
}
"#;

/// Session configuration options supplied during viewer initialization.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub profile_id: String,
    pub host_address: String,
    pub vpn_adapter_binding: Option<String>,
    pub enable_audio: bool,
    pub enable_microphone: bool,
    pub enable_clipboard: bool,
    pub max_target_fps: u32,
    pub preferred_video_codec: VideoCodec,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            profile_id: String::from("default"),
            host_address: String::from("127.0.0.1:4433"),
            vpn_adapter_binding: None,
            enable_audio: true,
            enable_microphone: false,
            enable_clipboard: true,
            max_target_fps: 60,
            preferred_video_codec: VideoCodec::AV1,
        }
    }
}

/// Dynamic metrics captured during an active remote session.
#[derive(Debug, Clone, Default)]
pub struct SessionMetrics {
    pub current_fps: f32,
    pub rtt_ms: f32,
    pub decode_latency_us: u32,
    pub render_latency_us: u32,
    pub total_video_frames_received: u64,
    pub total_bytes_received: u64,
    pub packet_loss_rate: f32,
}

/// Represents the active state of a remote session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    Authenticating,
    NegotiatingTopology,
    Active,
    Reconnecting { attempt: u32, reason: String },
    Terminated { reason: String },
}

/// Decoded surface frame ready for WGPU Metal/Vulkan/DX12 texture binding and presentation.
#[derive(Debug, Clone)]
pub struct DecodedFrameSurface {
    pub frame_index: u64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
    pub timestamp_us: u64,
}

/// Main Session Controller for managing a single active remote desktop connection.
pub struct ClientSessionPipeline {
    pub config: SessionConfig,
    pub state: watch::Sender<SessionState>,
    pub metrics: Arc<Mutex<SessionMetrics>>,
    pub wire_tx: mpsc::Sender<WireMessage>,
    pub video_frame_tx: mpsc::Sender<DecodedFrameSurface>,
    pub input_event_tx: mpsc::Sender<InputEvent>,
}

impl ClientSessionPipeline {
    /// Spawns a new client session pipeline and hooks into transport & rendering channels.
    pub fn spawn(
        config: SessionConfig,
        wire_rx: mpsc::Receiver<WireMessage>,
        wire_tx: mpsc::Sender<WireMessage>,
    ) -> (
        Self,
        watch::Receiver<SessionState>,
        mpsc::Receiver<DecodedFrameSurface>,
        mpsc::Sender<InputEvent>,
    ) {
        let (state_tx, state_rx) = watch::channel(SessionState::Connecting);
        let (video_tx, video_rx) = mpsc::channel(128);
        let (input_tx, mut input_rx) = mpsc::channel(256);
        let metrics = Arc::new(Mutex::new(SessionMetrics::default()));

        let pipeline = Self {
            config: config.clone(),
            state: state_tx.clone(),
            metrics: metrics.clone(),
            wire_tx: wire_tx.clone(),
            video_frame_tx: video_tx.clone(),
            input_event_tx: input_tx.clone(),
        };

        // Task 1: Demux incoming wire messages from transport
        let wire_tx_feedback = wire_tx.clone();
        let metrics_demux = metrics.clone();
        let state_demux = state_tx;
        tokio::spawn(async move {
            Self::demux_loop(
                wire_rx,
                video_tx,
                wire_tx_feedback,
                metrics_demux,
                state_demux,
            )
            .await;
        });

        // Task 2: Outbound input capture event queue
        let wire_tx_input = wire_tx.clone();
        tokio::spawn(async move {
            while let Some(input) = input_rx.recv().await {
                let msg = WireMessage::Input(input);
                if let Err(e) = wire_tx_input.send(msg).await {
                    error!("Failed to dispatch input event over QUIC transport: {e}");
                    break;
                }
            }
        });

        // Task 3: Initial Handshake sequence over QUIC
        let wire_tx_handshake = wire_tx;
        tokio::spawn(async move {
            info!(
                "Initiating session handshake with target {}",
                config.host_address
            );
            let handshake = WireMessage::Handshake(HandshakeMessage::ClientHello {
                protocol_version: 1,
                client_id: String::from("rferram-client-v0.1"),
                client_name: String::from(get_platform_client_name()),
                supported_codecs: vec![
                    config.preferred_video_codec,
                    VideoCodec::H265,
                    VideoCodec::RawBgra,
                ],
                auth_methods: vec![
                    rferram_protocol::AuthMethod::VaultToken,
                    rferram_protocol::AuthMethod::Ed25519Signature,
                ],
            });
            let _ = wire_tx_handshake.send(handshake).await;
        });

        (pipeline, state_rx, video_rx, input_tx)
    }

    /// Asynchronous demuxing loop for incoming wire packets.
    async fn demux_loop(
        mut wire_rx: mpsc::Receiver<WireMessage>,
        video_tx: mpsc::Sender<DecodedFrameSurface>,
        wire_tx_feedback: mpsc::Sender<WireMessage>,
        metrics: Arc<Mutex<SessionMetrics>>,
        state_tx: watch::Sender<SessionState>,
    ) {
        let mut frame_count = 0u64;
        let mut expected_frame_id: Option<u64> = None;
        let mut last_fps_check = Instant::now();

        while let Some(msg) = wire_rx.recv().await {
            match msg {
                WireMessage::Handshake(h) => {
                    info!(?h, "Handshake update received");
                    if matches!(h, HandshakeMessage::AuthResponse { success: true, .. }) {
                        let _ = state_tx.send(SessionState::Active);
                    }
                }
                WireMessage::Topology(t) => {
                    info!(?t, "Host display topology updated");
                }
                WireMessage::Video(frame) => {
                    let frame_idx = frame.frame_id;
                    let timestamp_us = frame.timestamp_us;
                    let payload_len = frame.payload.len() as u64;

                    // Detect sequence gaps and emit instant LossReport for IDR keyframe recovery
                    if let Some(expected) = expected_frame_id
                        && frame_idx > expected
                    {
                        let missing: Vec<u64> = (expected..frame_idx).collect();
                        warn!(
                            ?missing,
                            "Detected video frame sequence gap; sending LossReport"
                        );
                        let _ = wire_tx_feedback
                            .send(WireMessage::Control(ControlMessage::LossReport {
                                missing_frames: missing,
                            }))
                            .await;
                    }
                    expected_frame_id = Some(frame_idx + 1);

                    let decode_start = Instant::now();
                    let decoded_surface = DecodedFrameSurface {
                        frame_index: frame_idx,
                        width: frame.width,
                        height: frame.height,
                        format: frame.format,
                        data: frame.payload,
                        timestamp_us,
                    };
                    let decode_dur = decode_start.elapsed().as_micros() as u32;

                    if video_tx.send(decoded_surface).await.is_ok() {
                        frame_count += 1;
                        let render_dur = decode_start.elapsed().as_micros() as u32;

                        {
                            let mut m = metrics.lock().await;
                            m.total_video_frames_received += 1;
                            m.total_bytes_received += payload_len;
                            m.decode_latency_us = decode_dur;
                            m.render_latency_us = render_dur;

                            if last_fps_check.elapsed() >= Duration::from_secs(1) {
                                m.current_fps =
                                    frame_count as f32 / last_fps_check.elapsed().as_secs_f32();
                                frame_count = 0;
                                last_fps_check = Instant::now();
                            }
                        }

                        let ack = WireMessage::Control(ControlMessage::FrameAck {
                            frame_id: frame_idx,
                            monitor_id: frame.monitor_id,
                            render_delay_us: render_dur,
                        });
                        let _ = wire_tx_feedback.send(ack).await;
                    }
                }
                WireMessage::Audio(audio) => {
                    debug!(
                        payload_bytes = audio.payload.len(),
                        codec = ?audio.codec,
                        "Low-latency audio frame received"
                    );
                }
                WireMessage::Control(ctrl) => match ctrl {
                    ControlMessage::BandwidthEstimate { rtt_ms, .. } => {
                        let mut m = metrics.lock().await;
                        m.rtt_ms = rtt_ms;
                    }
                    ControlMessage::LossReport { missing_frames } => {
                        warn!(?missing_frames, "Loss report received from host");
                    }
                    _ => {}
                },
                WireMessage::Channel(vc) => {
                    info!(?vc, "Virtual channel payload received");
                }
                WireMessage::Input(_) => {}
            }
        }
    }

    /// Helper to inject mouse move events into the input channel
    pub async fn send_mouse_move(&self, monitor_id: u32, x: f64, y: f64, is_absolute: bool) {
        let _ = self
            .input_event_tx
            .send(InputEvent::MouseMove {
                monitor_id,
                x,
                y,
                is_absolute,
            })
            .await;
    }

    /// Helper to inject mouse button events
    pub async fn send_mouse_button(
        &self,
        button: MouseButtonCode,
        state: ElementState,
        x: f64,
        y: f64,
    ) {
        let _ = self
            .input_event_tx
            .send(InputEvent::MouseButton {
                button,
                state,
                x,
                y,
            })
            .await;
    }

    /// Helper to inject keypress events with platform modifier awareness
    pub async fn send_key_event(
        &self,
        key_code: u32,
        scancode: u32,
        state: ElementState,
        modifiers: KeyModifiers,
    ) {
        let _ = self
            .input_event_tx
            .send(InputEvent::KeyEvent {
                key_code,
                scancode,
                state,
                modifiers,
            })
            .await;
    }
}

pub fn get_platform_client_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "rFerram macOS Client (Metal)"
    } else if cfg!(target_os = "windows") {
        "rFerram Windows Client (DirectX 12)"
    } else {
        "rFerram Linux Client (Wayland/COSMIC)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rferram_protocol::VideoFrameMessage;

    #[tokio::test]
    async fn test_client_pipeline_handshake_and_loss_detection() {
        let (host_to_client_tx, host_to_client_rx) = mpsc::channel(32);
        let (client_to_host_tx, mut client_to_host_rx) = mpsc::channel(32);

        let (pipeline, _state_rx, mut video_rx, _input_tx) = ClientSessionPipeline::spawn(
            SessionConfig::default(),
            host_to_client_rx,
            client_to_host_tx,
        );

        // 1. Expect initial ClientHello from pipeline
        let hello = client_to_host_rx.recv().await.unwrap();
        assert!(matches!(
            hello,
            WireMessage::Handshake(HandshakeMessage::ClientHello { .. })
        ));

        // 2. Send Frame 1 and Frame 3 (skipping Frame 2) to trigger LossReport
        host_to_client_tx
            .send(WireMessage::Video(VideoFrameMessage {
                frame_id: 1,
                monitor_id: 1,
                timestamp_us: 1000,
                codec: VideoCodec::AV1,
                is_keyframe: true,
                width: 1920,
                height: 1080,
                stride: 1920 * 4,
                format: PixelFormat::Bgra8,
                payload: vec![0xAA; 64],
            }))
            .await
            .unwrap();

        let decoded_1 = video_rx.recv().await.unwrap();
        assert_eq!(decoded_1.frame_index, 1);

        let ack_1 = client_to_host_rx.recv().await.unwrap();
        assert!(matches!(
            ack_1,
            WireMessage::Control(ControlMessage::FrameAck { frame_id: 1, .. })
        ));

        host_to_client_tx
            .send(WireMessage::Video(VideoFrameMessage {
                frame_id: 3,
                monitor_id: 1,
                timestamp_us: 2000,
                codec: VideoCodec::AV1,
                is_keyframe: false,
                width: 1920,
                height: 1080,
                stride: 1920 * 4,
                format: PixelFormat::Bgra8,
                payload: vec![0xBB; 64],
            }))
            .await
            .unwrap();

        let loss_msg = client_to_host_rx.recv().await.unwrap();
        assert_eq!(
            loss_msg,
            WireMessage::Control(ControlMessage::LossReport {
                missing_frames: vec![2],
            })
        );

        let _ = pipeline.metrics.lock().await;
    }
}
