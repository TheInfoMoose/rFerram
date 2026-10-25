//! `rferram-protocol`
//!
//! Memory-safe, zero-copy wire protocol definitions and serialization frames
//! for communication between the `rferram-client` viewer and `rferram-server` host daemon
//! over `rferram-net` (QUIC/UDP).

use serde::{Deserialize, Serialize};

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
    DevicePassthrough {
        device_id: String,
        data: Vec<u8>,
    },
    RequestElevation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClipboardPayload {
    Text(String),
    Image {
        format: String, // e.g. "image/png"
        data: Vec<u8>,
    },
    Files(Vec<String>),
}
