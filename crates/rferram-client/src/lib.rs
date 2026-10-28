//! `rferram-client`: Remote Viewer Session Pipeline Engine (v2 - Cross-Platform including macOS, Windows & Linux/COSMIC)
//!
//! Coordinates QUIC stream demuxing, video frame decoding (`wgpu 30.0.1` Metal / Vulkan / DirectX 12),
//! audio output playback (`cpal` / CoreAudio / WASAPI / PipeWire), native peripheral input capture,
//! dynamic feedback control loops (RTT, sequence loss & FPS monitoring), and virtual channels
//! (Clipboard & ST v2 APFS/VSS/Linux snapshots).

pub mod audio;
pub mod bridge;
pub mod render;
pub mod session;

pub use audio::*;
pub use bridge::*;
pub use render::*;
pub use session::*;
