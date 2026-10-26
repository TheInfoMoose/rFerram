//! `rferram-protocol`
//!
//! Memory-safe, zero-copy wire protocol definitions and `postcard` serialization frames
//! for communication between the `rferram-client` viewer and `rferram-server` host daemon
//! over `rferram-net` (QUIC/UDP).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum permitted wire payload size (16 MiB) to prevent memory exhaustion DoS.
pub const MAX_WIRE_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Protocol serialization and framing errors.
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Postcard wire serialization error: {0}")]
    Serialization(#[from] postcard::Error),
    #[error("Wire frame length {0} bytes exceeds maximum allowed {MAX_WIRE_FRAME_BYTES} bytes")]
    FrameTooLarge(usize),
    #[error("Incomplete length-prefixed wire frame header")]
    IncompleteHeader,
}

/// Serializes a [`WireMessage`] into a compact binary `postcard` payload.
pub fn encode_wire_message(msg: &WireMessage) -> Result<Vec<u8>, ProtocolError> {
    let bytes = postcard::to_stdvec(msg)?;
    if bytes.len() > MAX_WIRE_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge(bytes.len()));
    }
    Ok(bytes)
}

/// Deserializes a [`WireMessage`] from a binary `postcard` slice with size bounds checking.
pub fn decode_wire_message(bytes: &[u8]) -> Result<WireMessage, ProtocolError> {
    if bytes.len() > MAX_WIRE_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge(bytes.len()));
    }
    Ok(postcard::from_bytes(bytes)?)
}

/// Encodes a [`WireMessage`] with a 4-byte little-endian length prefix for QUIC streams.
pub fn encode_length_prefixed(msg: &WireMessage) -> Result<Vec<u8>, ProtocolError> {
    let payload = encode_wire_message(msg)?;
    let len = payload.len() as u32;
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&len.to_le_bytes());
    framed.extend_from_slice(&payload);
    Ok(framed)
}

/// Decodes a 4-byte little-endian length-prefixed [`WireMessage`] buffer.
pub fn decode_length_prefixed(framed: &[u8]) -> Result<WireMessage, ProtocolError> {
    if framed.len() < 4 {
        return Err(ProtocolError::IncompleteHeader);
    }
    let len = u32::from_le_bytes([framed[0], framed[1], framed[2], framed[3]]) as usize;
    if len > MAX_WIRE_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge(len));
    }
    if framed.len() < 4 + len {
        return Err(ProtocolError::IncompleteHeader);
    }
    decode_wire_message(&framed[4..4 + len])
}

/// Main protocol envelope wrapping all messaging streams over QUIC multiplexed channels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WireMessage {
    /// Initial session negotiation, handshake, and credential validation.
    Handshake(HandshakeMessage),
    /// Multi-monitor display layout, DPI scaling, and geometry updates.
    Topology(TopologyMessage),
    /// Encoded video frame headers, metadata, and payload slices.
    Video(VideoFrameMessage),
    /// Bi-directional low-latency audio sync (Host speaker / Client mic).
    Audio(AudioFrameMessage),
    /// High-priority raw input events (Mouse, Keyboard, Touch, Gamepad).
    Input(InputEvent),
    /// Adaptive stream control, frame acknowledgments, and congestion metrics.
    Control(ControlMessage),
    /// Out-of-band virtual channel payloads (Clipboard, ST file sync, Passthrough).
    Channel(VirtualChannelMessage),
}

// ============================================================================
// 1. HANDSHAKE & SESSION NEGOTIATION
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HandshakeMessage {
    ClientHello {
        protocol_version: u32,
        client_id: String,
        client_name: String,
        supported_codecs: Vec<VideoCodec>,
        auth_methods: Vec<AuthMethod>,
    },
    ServerHello {
        server_version: u32,
        server_id: String,
        selected_codec: VideoCodec,
        auth_challenge: Vec<u8>,
    },
    AuthRequest {
        auth_method: AuthMethod,
        credentials: Vec<u8>,
    },
    AuthResponse {
        success: bool,
        session_token: Option<String>,
        error_message: Option<String>,
        requires_elevation: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMethod {
    Ed25519Signature,
    VaultToken,
    SystemUserPassword,
}

// ============================================================================
// 2. DISPLAY TOPOLOGY & GEOMETRY
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TopologyMessage {
    /// Full snapshot of host monitor configuration.
    DisplayLayout {
        monitors: Vec<MonitorInfo>,
    },
    /// Request from client to resize or rescale a remote monitor session.
    ResizeRequest {
        monitor_id: u32,
        width: u32,
        height: u32,
        scale_factor: f32,
    },
    DisplayAdded(MonitorInfo),
    DisplayRemoved {
        monitor_id: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MonitorInfo {
    pub monitor_id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
    pub refresh_rate_hz: u32,
    pub is_primary: bool,
}

// ============================================================================
// 3. VIDEO STREAMING
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoFrameMessage {
    pub frame_id: u64,
    pub monitor_id: u32,
    pub timestamp_us: u64,
    pub codec: VideoCodec,
    pub is_keyframe: bool,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VideoCodec {
    AV1,
    H265,
    H264,
    RawBgra,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PixelFormat {
    NV12,
    YUV420P,
    YUV444P,
    Rgba8,
    Bgra8,
}

// ============================================================================
// 4. DUPLEX AUDIO SYNC
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioFrameMessage {
    pub stream_id: u32,
    pub timestamp_us: u64,
    pub direction: AudioDirection,
    pub codec: AudioCodec,
    pub channels: u8,
    pub sample_rate: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AudioDirection {
    HostToClientSpeaker,
    ClientToHostMicrophone,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AudioCodec {
    Opus,
    Pcm16Le,
}

// ============================================================================
// 5. LOW-LATENCY INPUT SUBSYSTEM
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InputEvent {
    MouseMove {
        monitor_id: u32,
        x: f64,
        y: f64,
        is_absolute: bool,
    },
    MouseButton {
        button: MouseButtonCode,
        state: ElementState,
        x: f64,
        y: f64,
    },
    MouseWheel {
        delta_x: f32,
        delta_y: f32,
        unit: WheelUnit,
    },
    KeyEvent {
        key_code: u32,
        scancode: u32,
        state: ElementState,
        modifiers: KeyModifiers,
    },
    TouchEvent {
        touch_id: u64,
        phase: TouchPhase,
        x: f64,
        y: f64,
    },
    TextInput {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MouseButtonCode {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ElementState {
    Pressed,
    Released,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WheelUnit {
    Line,
    Pixel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

// ============================================================================
// 6. ADAPTIVE CONTROL & FEEDBACK
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ControlMessage {
    /// Viewer confirms receipt and decode completion of a video frame.
    FrameAck {
        frame_id: u64,
        monitor_id: u32,
        render_delay_us: u32,
    },
    /// Loss notification triggering instant keyframe generation.
    LossReport {
        missing_frames: Vec<u64>,
    },
    /// Network metrics for QUIC bitrate adjustment.
    BandwidthEstimate {
        bytes_per_sec: u64,
        rtt_ms: f32,
        jitter_ms: f32,
    },
    SetTargetFps(u32),
    SetQuality {
        quantizer: u8,
        color_subsampling: ColorSubsampling,
    },
    KeepAlive,
    Disconnect {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ColorSubsampling {
    FourTwoZero,
    FourFourFour,
}

// ============================================================================
// 7. VIRTUAL CHANNELS (Clipboard, File Transfer, Passthrough)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VirtualChannelMessage {
    Clipboard(ClipboardPayload),
    SimplyTransfer(Vec<u8>), // Wraps ST_Rust_Age snapshot/transfer packets
    DevicePassthrough { device_id: String, data: Vec<u8> },
    RequestElevation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClipboardPayload {
    Text(String),
    Image {
        width: u32,
        height: u32,
        format: String, // e.g. "image/png" or "raw/rgba8"
        data: Vec<u8>,
    },
    Files(Vec<String>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_frame_postcard_roundtrip() {
        let original_frame = WireMessage::Video(VideoFrameMessage {
            frame_id: 1024,
            monitor_id: 0,
            timestamp_us: 1_600_000_000,
            codec: VideoCodec::AV1,
            is_keyframe: true,
            width: 3840,
            height: 2160,
            stride: 3840 * 4,
            format: PixelFormat::NV12,
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE],
        });

        let encoded =
            encode_wire_message(&original_frame).expect("Failed to serialize VideoFrameMessage");
        assert!(!encoded.is_empty());

        let decoded =
            decode_wire_message(&encoded).expect("Failed to deserialize VideoFrameMessage");
        assert_eq!(original_frame, decoded);
    }

    #[test]
    fn test_control_loss_report_roundtrip() {
        let original_msg = WireMessage::Control(ControlMessage::LossReport {
            missing_frames: vec![101, 102, 105, 108],
        });

        let encoded = encode_length_prefixed(&original_msg).unwrap();
        let decoded = decode_length_prefixed(&encoded).unwrap();
        assert_eq!(original_msg, decoded);
    }

    #[test]
    fn test_handshake_hello_roundtrip() {
        let hello = WireMessage::Handshake(HandshakeMessage::ClientHello {
            protocol_version: 1,
            client_id: "test-client-uuid".to_string(),
            client_name: "macOS Studio Client".to_string(),
            supported_codecs: vec![VideoCodec::AV1, VideoCodec::H265],
            auth_methods: vec![AuthMethod::VaultToken, AuthMethod::Ed25519Signature],
        });

        let bytes = encode_wire_message(&hello).unwrap();
        let restored = decode_wire_message(&bytes).unwrap();
        assert_eq!(hello, restored);
    }
}
