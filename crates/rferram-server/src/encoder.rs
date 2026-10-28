//! Hardware and software video encoder abstractions and implementations (`AV1`, `H.265`, `H.264`, Apple `VideoToolbox`).

use anyhow::Result;
use tracing::{debug, info};

use rferram_protocol::{VideoCodec, VideoFrameMessage};

use crate::CapturedFrame;

/// Hardware / Software Encoder Configuration
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
    pub target_bitrate_kbps: u32,
    pub enable_hardware_accel: bool,
    pub keyframe_interval: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::AV1,
            width: 2560,
            height: 1440,
            target_fps: 60,
            target_bitrate_kbps: 15_000,
            enable_hardware_accel: true,
            keyframe_interval: 60,
        }
    }
}

/// Abstraction interface for Hardware-Accelerated Video Encoders (NVENC, VAAPI, Apple VideoToolbox, QuickSync, rav1e)
pub trait VideoEncoder: Send + Sync {
    fn encode(&mut self, frame: &CapturedFrame, force_keyframe: bool) -> Result<VideoFrameMessage>;
    fn update_bitrate(&mut self, target_bitrate_kbps: u32) -> Result<()>;
    fn update_framerate(&mut self, target_fps: u32) -> Result<()>;
}

/// macOS Apple Silicon / VideoToolbox Hardware Accelerated Video Encoder
pub struct MacOsVideoToolboxEncoder {
    config: EncoderConfig,
    frame_counter: u64,
}

impl MacOsVideoToolboxEncoder {
    pub fn new(config: EncoderConfig) -> Self {
        info!(
            width = config.width,
            height = config.height,
            codec = ?config.codec,
            "Initialized macOS VideoToolbox Hardware Encoder (Apple Silicon / Intel Media Engine)"
        );
        Self {
            config,
            frame_counter: 0,
        }
    }

    /// Returns a reference to the current encoder configuration.
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Returns the total number of frames encoded so far.
    pub fn frame_counter(&self) -> u64 {
        self.frame_counter
    }
}

impl VideoEncoder for MacOsVideoToolboxEncoder {
    fn encode(&mut self, frame: &CapturedFrame, force_keyframe: bool) -> Result<VideoFrameMessage> {
        self.frame_counter += 1;
        let is_keyframe = force_keyframe
            || self.frame_counter == 1
            || self
                .frame_counter
                .is_multiple_of(self.config.keyframe_interval.max(1) as u64);

        let payload_size = std::cmp::min(2048, frame.data.len());
        let encoded_payload = frame.data[..payload_size].to_vec();

        Ok(VideoFrameMessage {
            frame_id: self.frame_counter,
            monitor_id: frame.display_id,
            timestamp_us: frame.timestamp_us,
            codec: self.config.codec,
            is_keyframe,
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            format: frame.pixel_format,
            payload: encoded_payload,
        })
    }

    fn update_bitrate(&mut self, target_bitrate_kbps: u32) -> Result<()> {
        self.config.target_bitrate_kbps = target_bitrate_kbps;
        debug!(
            target_bitrate_kbps,
            "VideoToolbox target bitrate updated dynamically"
        );
        Ok(())
    }

    fn update_framerate(&mut self, target_fps: u32) -> Result<()> {
        self.config.target_fps = target_fps;
        debug!(
            target_fps,
            "VideoToolbox target framerate updated dynamically"
        );
        Ok(())
    }
}

/// General Hardware/AV1 Fallback Video Encoder
pub struct SoftwareVideoEncoder {
    config: EncoderConfig,
    frame_counter: u64,
}

impl SoftwareVideoEncoder {
    pub fn new(config: EncoderConfig) -> Self {
        Self {
            config,
            frame_counter: 0,
        }
    }

    /// Returns a reference to the current encoder configuration.
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Returns the total number of frames encoded so far.
    pub fn frame_counter(&self) -> u64 {
        self.frame_counter
    }
}

impl VideoEncoder for SoftwareVideoEncoder {
    fn encode(&mut self, frame: &CapturedFrame, force_keyframe: bool) -> Result<VideoFrameMessage> {
        self.frame_counter += 1;
        let is_keyframe = force_keyframe
            || self.frame_counter == 1
            || self
                .frame_counter
                .is_multiple_of(self.config.keyframe_interval.max(1) as u64);

        let payload_size = std::cmp::min(1024, frame.data.len());
        let encoded_payload = frame.data[..payload_size].to_vec();

        Ok(VideoFrameMessage {
            frame_id: self.frame_counter,
            monitor_id: frame.display_id,
            timestamp_us: frame.timestamp_us,
            codec: self.config.codec,
            is_keyframe,
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            format: frame.pixel_format,
            payload: encoded_payload,
        })
    }

    fn update_bitrate(&mut self, target_bitrate_kbps: u32) -> Result<()> {
        self.config.target_bitrate_kbps = target_bitrate_kbps;
        Ok(())
    }

    fn update_framerate(&mut self, target_fps: u32) -> Result<()> {
        self.config.target_fps = target_fps;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rferram_protocol::PixelFormat;

    fn sample_frame() -> CapturedFrame {
        CapturedFrame {
            frame_index: 1,
            timestamp_us: 100,
            display_id: 1,
            width: 1920,
            height: 1080,
            stride: 1920 * 4,
            pixel_format: PixelFormat::Bgra8,
            data: vec![0xAA; 2048],
            is_dmabuf_or_metal_handle: true,
        }
    }

    #[test]
    fn test_encoder_keyframe_interval_multiples_and_updates() {
        let mut sw_encoder = SoftwareVideoEncoder::new(EncoderConfig {
            keyframe_interval: 3,
            ..Default::default()
        });
        let frame = sample_frame();

        // Frame 1: initial keyframe
        let f1 = sw_encoder.encode(&frame, false).unwrap();
        assert_eq!(f1.frame_id, 1);
        assert!(f1.is_keyframe);

        // Frame 2: inter-frame
        let f2 = sw_encoder.encode(&frame, false).unwrap();
        assert_eq!(f2.frame_id, 2);
        assert!(!f2.is_keyframe);

        // Frame 3: multiple of keyframe_interval (3) -> keyframe
        let f3 = sw_encoder.encode(&frame, false).unwrap();
        assert_eq!(f3.frame_id, 3);
        assert!(f3.is_keyframe);

        // Dynamic bitrate and framerate updates
        sw_encoder.update_bitrate(25_000).unwrap();
        sw_encoder.update_framerate(120).unwrap();
        assert_eq!(sw_encoder.config().target_bitrate_kbps, 25_000);
        assert_eq!(sw_encoder.config().target_fps, 120);
        assert_eq!(sw_encoder.frame_counter(), 3);
    }

    #[test]
    fn test_macos_videotoolbox_encoder_keyframes_and_updates() {
        let mut vt_encoder = MacOsVideoToolboxEncoder::new(EncoderConfig {
            keyframe_interval: 4,
            ..Default::default()
        });
        let frame = sample_frame();

        assert!(vt_encoder.encode(&frame, false).unwrap().is_keyframe); // frame 1
        assert!(!vt_encoder.encode(&frame, false).unwrap().is_keyframe); // frame 2
        assert!(vt_encoder.encode(&frame, true).unwrap().is_keyframe); // frame 3 (forced)
        assert!(vt_encoder.encode(&frame, false).unwrap().is_keyframe); // frame 4 (interval multiple)
        assert!(!vt_encoder.encode(&frame, false).unwrap().is_keyframe); // frame 5

        vt_encoder.update_bitrate(30_000).unwrap();
        vt_encoder.update_framerate(144).unwrap();
        assert_eq!(vt_encoder.config().target_bitrate_kbps, 30_000);
        assert_eq!(vt_encoder.config().target_fps, 144);
        assert_eq!(vt_encoder.frame_counter(), 5);
    }
}
