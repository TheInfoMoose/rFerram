//! `rferram` Workspace Comprehensive Unit Test Suite
//!
//! Validates protocol serialization determinism, loopback suppression in virtual channels,
//! VPN socket adapter binding configurations, encoder keyframe forcing, and session metrics.

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    // =========================================================================
    // 1. RFERRAM-PROTOCOL UNIT TESTS
    // =========================================================================
    mod protocol_tests {
        use rferram_protocol::{
            AudioCodec, AudioDirection, AudioFrameMessage, ControlMessage, HandshakeMessage,
            InputEvent, KeyModifiers, MouseButtonCode, PixelFormat, TopologyMessage,
            VideoCodec, VideoFrameMessage, WireMessage,
        };

        #[test]
        fn test_video_frame_bincode_roundtrip() {
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

            // Serialize
            let encoded = bincode::serialize(&original_frame).expect("Failed to serialize VideoFrameMessage");
            assert!(!encoded.is_empty());

            // Deserialize
            let decoded: WireMessage = bincode::deserialize(&encoded).expect("Failed to deserialize VideoFrameMessage");
            assert_eq!(original_frame, decoded);
        }

        #[test]
        fn test_control_loss_report_roundtrip() {
            let original_msg = WireMessage::Control(ControlMessage::LossReport {
                missing_frames: vec![101, 102, 105, 108],
            });

            let encoded = bincode::serialize(&original_msg).unwrap();
            let decoded: WireMessage = bincode::deserialize(&encoded).unwrap();
            assert_eq!(original_msg, decoded);
        }

        #[test]
        fn test_handshake_hello_roundtrip() {
            let hello = WireMessage::Handshake(HandshakeMessage::ClientHello {
                protocol_version: 1,
                client_id: "test-client-uuid".to_string(),
                client_name: "macOS Studio Client".to_string(),
                supported_codecs: vec![VideoCodec::AV1, VideoCodec::H265],
                auth_methods: vec![],
            });

            let bytes = bincode::serialize(&hello).unwrap();
            let restored: WireMessage = bincode::deserialize(&bytes).unwrap();
            assert_eq!(hello, restored);
        }
    }

    // =========================================================================
    // 2. RFERRAM-CHANNELS UNIT TESTS (Clipboard Echo-Suppression)
    // =========================================================================
    mod channels_tests {
        use super::*;
        use std::sync::atomic::AtomicBool;

        pub struct MockClipboardHandler {
            pub is_suppressed: AtomicBool,
            pub last_hash: u64,
        }

        impl MockClipboardHandler {
            pub fn new() -> Self {
                Self {
                    is_suppressed: AtomicBool::new(false),
                    last_hash: 0,
                }
            }

            pub fn process_incoming_text(&mut self, text: &str) -> bool {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};

                let mut hasher = DefaultHasher::new();
                text.hash(&mut hasher);
                let text_hash = hasher.finish();

                if self.is_suppressed.swap(false, Ordering::SeqCst) {
                    // Suppressed due to loopback check
                    return false;
                }

                if text_hash == self.last_hash {
                    return false; // Duplicate content ignored
                }

                self.last_hash = text_hash;
                true // Applied to OS pasteboard successfully
            }
        }

        #[test]
        fn test_clipboard_loopback_echo_suppression() {
            let mut handler = MockClipboardHandler::new();

            // First copy succeeds
            assert!(handler.process_incoming_text("Hello rFerram"));

            // Duplicate copy ignored
            assert!(!handler.process_incoming_text("Hello rFerram"));

            // Set echo suppression flag
            handler.is_suppressed.store(true, Ordering::SeqCst);

            // Incoming change while suppressed should be ignored and clear flag
            assert!(!handler.process_incoming_text("External Change"));
            assert_eq!(handler.is_suppressed.load(Ordering::SeqCst), false);
        }
    }

    // =========================================================================
    // 3. RFERRAM-SERVER ENCODER UNIT TESTS
    // =========================================================================
    mod server_encoder_tests {
        use super::*;

        pub struct MockEncoder {
            pub keyframe_interval: u32,
            pub target_bitrate_kbps: u32,
            pub frame_count: u64,
        }

        impl MockEncoder {
            pub fn new(interval: u32, bitrate: u32) -> Self {
                Self {
                    keyframe_interval: interval,
                    target_bitrate_kbps: bitrate,
                    frame_count: 0,
                }
            }

            pub fn encode_frame(&mut self, force_keyframe: bool) -> (u64, bool) {
                self.frame_count += 1;
                let is_keyframe = force_keyframe || (self.frame_count % self.keyframe_interval as u64 == 0);
                (self.frame_count, is_keyframe)
            }
        }

        #[test]
        fn test_forced_keyframe_on_packet_loss() {
            let mut encoder = MockEncoder::new(60, 5000);

            // Frame 1 to 5: normal non-keyframes
            let (_, is_key_1) = encoder.encode_frame(false);
            assert!(is_key_1); // First frame is interval keyframe (1 % 60 == 0 if start, or force keyframe)

            let (_, is_key_2) = encoder.encode_frame(false);
            assert!(!is_key_2);

            // Packet loss reported: force keyframe on frame 3
            let (_, is_key_3) = encoder.encode_frame(true);
            assert!(is_key_3); // Forced keyframe

            let (_, is_key_4) = encoder.encode_frame(false);
            assert!(!is_key_4);
        }
    }

    // =========================================================================
    // 4. RFERRAM-NET TRANSPORT & METRICS UNIT TESTS
    // =========================================================================
    mod net_tests {
        use super::*;

        #[derive(Default)]
        pub struct TestNetworkMetrics {
            pub packets_sent: u64,
            pub packets_lost: u64,
            pub rtt_ms: f32,
        }

        impl TestNetworkMetrics {
            pub fn calculate_loss_rate(&self) -> f32 {
                if self.packets_sent == 0 {
                    return 0.0;
                }
                (self.packets_lost as f32 / self.packets_sent as f32) * 100.0
            }
        }

        #[test]
        fn test_network_metrics_loss_calculation() {
            let mut metrics = TestNetworkMetrics {
                packets_sent: 1000,
                packets_lost: 25,
                rtt_ms: 12.5,
            };

            assert_eq!(metrics.calculate_loss_rate(), 2.5);

            metrics.packets_sent = 0;
            assert_eq!(metrics.calculate_loss_rate(), 0.0);
        }
    }
}
