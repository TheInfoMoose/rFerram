//! Integration tests for `rferram-protocol` binary `postcard` wire envelopes,
//! 4-byte little-endian length-prefixed framing, and `MAX_WIRE_FRAME_BYTES` bounds checks.

use rferram_protocol::{
    AudioCodec, AudioDirection, AudioFrameMessage, AuthMethod, ClipboardPayload, ColorSubsampling,
    ControlMessage, ElementState, HandshakeMessage, InputEvent, KeyModifiers, MAX_WIRE_FRAME_BYTES,
    MonitorInfo, MouseButtonCode, PixelFormat, ProtocolError, TopologyMessage, TouchPhase,
    VideoCodec, VideoFrameMessage, VirtualChannelMessage, WheelUnit, WireMessage,
    decode_length_prefixed, decode_wire_message, encode_length_prefixed, encode_wire_message,
};

/// Helper that validates both raw datagram (`encode_wire_message` / `decode_wire_message`)
/// and 4-byte little-endian stream (`encode_length_prefixed` / `decode_length_prefixed`)
/// roundtrip serialization for a given [`WireMessage`].
fn assert_wire_and_framed_roundtrip(msg: &WireMessage) {
    let raw_bytes = encode_wire_message(msg).expect("encode_wire_message should succeed");
    assert!(!raw_bytes.is_empty());
    assert!(raw_bytes.len() <= MAX_WIRE_FRAME_BYTES);

    let decoded_raw = decode_wire_message(&raw_bytes).expect("decode_wire_message should succeed");
    assert_eq!(&decoded_raw, msg);

    let framed_bytes = encode_length_prefixed(msg).expect("encode_length_prefixed should succeed");
    assert_eq!(framed_bytes.len(), 4 + raw_bytes.len());

    let declared_len = u32::from_le_bytes([
        framed_bytes[0],
        framed_bytes[1],
        framed_bytes[2],
        framed_bytes[3],
    ]) as usize;
    assert_eq!(declared_len, raw_bytes.len());
    assert_eq!(&framed_bytes[4..], raw_bytes.as_slice());

    let decoded_framed =
        decode_length_prefixed(&framed_bytes).expect("decode_length_prefixed should succeed");
    assert_eq!(&decoded_framed, msg);

    // Ensure trailing stream bytes after a complete frame do not corrupt decoding of the first frame
    let mut with_trailing = framed_bytes.clone();
    with_trailing.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
    let decoded_with_trailing = decode_length_prefixed(&with_trailing)
        .expect("decode_length_prefixed should ignore trailing stream bytes");
    assert_eq!(&decoded_with_trailing, msg);
}

#[test]
fn test_handshake_all_variants_roundtrip() {
    let variants = vec![
        WireMessage::Handshake(HandshakeMessage::ClientHello {
            protocol_version: 1,
            client_id: "client-ed25519-001".to_string(),
            client_name: "COSMIC Workstation Viewer".to_string(),
            supported_codecs: vec![
                VideoCodec::AV1,
                VideoCodec::H265,
                VideoCodec::H264,
                VideoCodec::RawBgra,
            ],
            auth_methods: vec![
                AuthMethod::Ed25519Signature,
                AuthMethod::VaultToken,
                AuthMethod::SystemUserPassword,
            ],
        }),
        WireMessage::Handshake(HandshakeMessage::ServerHello {
            server_version: 1,
            server_id: "host-daemon-node-42".to_string(),
            selected_codec: VideoCodec::AV1,
            auth_challenge: vec![0x42; 32],
        }),
        WireMessage::Handshake(HandshakeMessage::AuthRequest {
            auth_method: AuthMethod::Ed25519Signature,
            credentials: vec![0xAB; 64],
        }),
        WireMessage::Handshake(HandshakeMessage::AuthResponse {
            success: true,
            session_token: Some("tok_nist_aes256gcm_9f8e7d6c".to_string()),
            error_message: None,
            requires_elevation: false,
        }),
        WireMessage::Handshake(HandshakeMessage::AuthResponse {
            success: false,
            session_token: None,
            error_message: Some("Channel-bound Ed25519 signature mismatch".to_string()),
            requires_elevation: true,
        }),
    ];

    for msg in &variants {
        assert_wire_and_framed_roundtrip(msg);
    }
}

#[test]
fn test_topology_all_variants_roundtrip() {
    let primary_monitor = MonitorInfo {
        monitor_id: 0,
        name: "DP-1 (COSMIC Primary)".to_string(),
        x: 0,
        y: 0,
        width: 3840,
        height: 2160,
        scale_factor: 1.5,
        refresh_rate_hz: 144,
        is_primary: true,
    };
    let secondary_monitor = MonitorInfo {
        monitor_id: 1,
        name: "HDMI-A-1 (Left Portrait)".to_string(),
        x: -1440,
        y: -240,
        width: 1440,
        height: 2560,
        scale_factor: 1.0,
        refresh_rate_hz: 60,
        is_primary: false,
    };

    let variants = vec![
        WireMessage::Topology(TopologyMessage::DisplayLayout {
            monitors: vec![primary_monitor.clone(), secondary_monitor.clone()],
        }),
        WireMessage::Topology(TopologyMessage::ResizeRequest {
            monitor_id: 0,
            width: 2560,
            height: 1440,
            scale_factor: 1.25,
        }),
        WireMessage::Topology(TopologyMessage::DisplayAdded(secondary_monitor)),
        WireMessage::Topology(TopologyMessage::DisplayRemoved { monitor_id: 1 }),
    ];

    for msg in &variants {
        assert_wire_and_framed_roundtrip(msg);
    }
}

#[test]
fn test_video_frame_all_codecs_and_pixel_formats_roundtrip() {
    let codecs = [
        VideoCodec::AV1,
        VideoCodec::H265,
        VideoCodec::H264,
        VideoCodec::RawBgra,
    ];
    let formats = [
        PixelFormat::NV12,
        PixelFormat::YUV420P,
        PixelFormat::YUV444P,
        PixelFormat::Rgba8,
        PixelFormat::Bgra8,
    ];

    let mut frame_id = 1;
    for codec in codecs {
        for format in formats {
            let msg = WireMessage::Video(VideoFrameMessage {
                frame_id,
                monitor_id: (frame_id % 2) as u32,
                timestamp_us: 16_666 * frame_id,
                codec,
                is_keyframe: frame_id % 3 == 0,
                width: 1920,
                height: 1080,
                stride: 1920 * 4,
                format,
                payload: vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
            });
            assert_wire_and_framed_roundtrip(&msg);
            frame_id += 1;
        }
    }
}

#[test]
fn test_audio_frame_all_directions_and_codecs_roundtrip() {
    let directions = [
        AudioDirection::HostToClientSpeaker,
        AudioDirection::ClientToHostMicrophone,
    ];
    let codecs = [AudioCodec::Opus, AudioCodec::Pcm16Le];

    let mut stream_id = 1;
    for direction in directions {
        for codec in codecs {
            let msg = WireMessage::Audio(AudioFrameMessage {
                stream_id,
                timestamp_us: 20_000 * u64::from(stream_id),
                direction,
                codec,
                channels: 2,
                sample_rate: 48_000,
                payload: [0x01, 0x02, 0xFE, 0xFF].repeat(40),
            });
            assert_wire_and_framed_roundtrip(&msg);
            stream_id += 1;
        }
    }
}

#[test]
fn test_input_event_all_variants_roundtrip() {
    let buttons = [
        MouseButtonCode::Left,
        MouseButtonCode::Right,
        MouseButtonCode::Middle,
        MouseButtonCode::Back,
        MouseButtonCode::Forward,
        MouseButtonCode::Other(8),
    ];
    let states = [ElementState::Pressed, ElementState::Released];
    let touch_phases = [
        TouchPhase::Started,
        TouchPhase::Moved,
        TouchPhase::Ended,
        TouchPhase::Cancelled,
    ];

    let mut events = vec![
        WireMessage::Input(InputEvent::MouseMove {
            monitor_id: 0,
            x: 1280.5,
            y: 720.25,
            is_absolute: true,
        }),
        WireMessage::Input(InputEvent::MouseMove {
            monitor_id: 1,
            x: -14.0,
            y: 8.5,
            is_absolute: false,
        }),
        WireMessage::Input(InputEvent::MouseWheel {
            delta_x: 0.0,
            delta_y: -120.0,
            unit: WheelUnit::Line,
        }),
        WireMessage::Input(InputEvent::MouseWheel {
            delta_x: 15.5,
            delta_y: 42.25,
            unit: WheelUnit::Pixel,
        }),
        WireMessage::Input(InputEvent::KeyEvent {
            key_code: 0x0026,
            scancode: 30,
            state: ElementState::Pressed,
            modifiers: KeyModifiers {
                shift: true,
                ctrl: true,
                alt: false,
                logo: true,
            },
        }),
        WireMessage::Input(InputEvent::KeyEvent {
            key_code: 0x0026,
            scancode: 30,
            state: ElementState::Released,
            modifiers: KeyModifiers::default(),
        }),
        WireMessage::Input(InputEvent::TextInput {
            text: "rFerram Zero-Copy Wayland 🚀 — 日本語テスト".to_string(),
        }),
    ];

    for button in buttons {
        for state in states {
            events.push(WireMessage::Input(InputEvent::MouseButton {
                button,
                state,
                x: 512.0,
                y: 384.0,
            }));
        }
    }

    for (idx, phase) in touch_phases.into_iter().enumerate() {
        events.push(WireMessage::Input(InputEvent::TouchEvent {
            touch_id: idx as u64,
            phase,
            x: 300.0 + (idx as f64 * 12.5),
            y: 600.0 - (idx as f64 * 5.0),
        }));
    }

    for msg in &events {
        assert_wire_and_framed_roundtrip(msg);
    }
}

#[test]
fn test_control_message_all_variants_roundtrip() {
    let variants = vec![
        WireMessage::Control(ControlMessage::FrameAck {
            frame_id: 4096,
            monitor_id: 0,
            render_delay_us: 1_850,
        }),
        WireMessage::Control(ControlMessage::LossReport {
            missing_frames: vec![4090, 4091, 4094],
        }),
        WireMessage::Control(ControlMessage::BandwidthEstimate {
            bytes_per_sec: 125_000_000,
            rtt_ms: 2.45,
            jitter_ms: 0.35,
        }),
        WireMessage::Control(ControlMessage::SetTargetFps(120)),
        WireMessage::Control(ControlMessage::SetQuality {
            quantizer: 18,
            color_subsampling: ColorSubsampling::FourTwoZero,
        }),
        WireMessage::Control(ControlMessage::SetQuality {
            quantizer: 10,
            color_subsampling: ColorSubsampling::FourFourFour,
        }),
        WireMessage::Control(ControlMessage::KeepAlive),
        WireMessage::Control(ControlMessage::Disconnect {
            reason: "User closed remote viewer session".to_string(),
        }),
    ];

    for msg in &variants {
        assert_wire_and_framed_roundtrip(msg);
    }
}

#[test]
fn test_virtual_channel_all_variants_roundtrip() {
    let variants = vec![
        WireMessage::Channel(VirtualChannelMessage::Clipboard(ClipboardPayload::Text(
            "Clipboard synchronization payload".to_string(),
        ))),
        WireMessage::Channel(VirtualChannelMessage::Clipboard(ClipboardPayload::Image {
            width: 64,
            height: 64,
            format: "raw/rgba8".to_string(),
            data: vec![0xFF; 64 * 64 * 4],
        })),
        WireMessage::Channel(VirtualChannelMessage::Clipboard(ClipboardPayload::Files(
            vec![
                "/home/user/Documents/report.pdf".to_string(),
                "/srv/devshare/archive.tar.zst".to_string(),
            ],
        ))),
        WireMessage::Channel(VirtualChannelMessage::SimplyTransfer(vec![
            0x53, 0x54, 0x5F, 0x41, 0x47, 0x45, 0x00, 0x01,
        ])),
        WireMessage::Channel(VirtualChannelMessage::DevicePassthrough {
            device_id: "usb:1050:0407-yubikey".to_string(),
            data: vec![0x90, 0x00],
        }),
        WireMessage::Channel(VirtualChannelMessage::RequestElevation),
    ];

    for msg in &variants {
        assert_wire_and_framed_roundtrip(msg);
    }
}

#[test]
fn test_oversized_frame_rejected_on_encode() {
    // 1. Inner payload strictly larger than MAX_WIRE_FRAME_BYTES (caught by pre-serialization check)
    let oversized_msg = WireMessage::Channel(VirtualChannelMessage::SimplyTransfer(vec![
        0u8;
        MAX_WIRE_FRAME_BYTES
            + 1
    ]));

    let err = encode_wire_message(&oversized_msg)
        .expect_err("encode_wire_message must reject payload > MAX_WIRE_FRAME_BYTES");
    match err {
        ProtocolError::FrameTooLarge(len) => assert!(len > MAX_WIRE_FRAME_BYTES),
        other => panic!("Expected ProtocolError::FrameTooLarge, got {other:?}"),
    }

    let framed_err = encode_length_prefixed(&oversized_msg)
        .expect_err("encode_length_prefixed must reject payload > MAX_WIRE_FRAME_BYTES");
    match framed_err {
        ProtocolError::FrameTooLarge(len) => assert!(len > MAX_WIRE_FRAME_BYTES),
        other => panic!("Expected ProtocolError::FrameTooLarge, got {other:?}"),
    }

    // 2. Inner payload equal to MAX_WIRE_FRAME_BYTES, whose serialized envelope exceeds MAX_WIRE_FRAME_BYTES
    let boundary_msg = WireMessage::Channel(VirtualChannelMessage::SimplyTransfer(vec![
        0u8;
        MAX_WIRE_FRAME_BYTES
    ]));

    let boundary_err = encode_wire_message(&boundary_msg).expect_err(
        "encode_wire_message must reject message when envelope + payload > MAX_WIRE_FRAME_BYTES",
    );
    match boundary_err {
        ProtocolError::FrameTooLarge(len) => assert!(len > MAX_WIRE_FRAME_BYTES),
        other => panic!("Expected ProtocolError::FrameTooLarge, got {other:?}"),
    }
}

#[test]
fn test_oversized_frame_rejected_on_decode() {
    // 1. Raw datagram decode exceeding MAX_WIRE_FRAME_BYTES
    let oversized_slice = vec![0u8; MAX_WIRE_FRAME_BYTES + 1];
    let err = decode_wire_message(&oversized_slice)
        .expect_err("decode_wire_message must reject buffer > MAX_WIRE_FRAME_BYTES");
    match err {
        ProtocolError::FrameTooLarge(len) => assert_eq!(len, MAX_WIRE_FRAME_BYTES + 1),
        other => panic!("Expected ProtocolError::FrameTooLarge, got {other:?}"),
    }

    // 2. Length-prefixed header claiming MAX_WIRE_FRAME_BYTES + 1 (rejected immediately from 4-byte header)
    let forged_len = (MAX_WIRE_FRAME_BYTES as u32) + 1;
    let forged_header = forged_len.to_le_bytes();
    let header_err = decode_length_prefixed(&forged_header)
        .expect_err("decode_length_prefixed must reject header > MAX_WIRE_FRAME_BYTES");
    match header_err {
        ProtocolError::FrameTooLarge(len) => assert_eq!(len, MAX_WIRE_FRAME_BYTES + 1),
        other => panic!("Expected ProtocolError::FrameTooLarge, got {other:?}"),
    }

    // 3. Length-prefixed header claiming u32::MAX
    let max_u32_header = u32::MAX.to_le_bytes();
    let max_err = decode_length_prefixed(&max_u32_header)
        .expect_err("decode_length_prefixed must reject u32::MAX length header");
    match max_err {
        ProtocolError::FrameTooLarge(len) => assert_eq!(len, u32::MAX as usize),
        other => panic!("Expected ProtocolError::FrameTooLarge, got {other:?}"),
    }
}

#[test]
fn test_incomplete_and_corrupted_frames_rejected() {
    // 1. Fewer than 4 header bytes
    for len in 0..4 {
        let short_buf = &[0x01, 0x02, 0x03][..len];
        let err = decode_length_prefixed(short_buf)
            .expect_err("decode_length_prefixed must reject < 4 byte header");
        assert!(
            matches!(err, ProtocolError::IncompleteHeader),
            "Expected IncompleteHeader for {len}-byte input, got {err:?}"
        );
    }

    // 2. Header declares 32 bytes, but only 10 payload bytes follow
    let mut truncated = 32u32.to_le_bytes().to_vec();
    truncated.extend_from_slice(&[0u8; 10]);
    let trunc_err = decode_length_prefixed(&truncated)
        .expect_err("decode_length_prefixed must reject truncated payload");
    assert!(matches!(trunc_err, ProtocolError::IncompleteHeader));

    // 3. Empty/corrupted postcard payload
    let empty_err = decode_wire_message(&[]).expect_err("Empty slice is not a valid WireMessage");
    assert!(matches!(empty_err, ProtocolError::Serialization(_)));

    let invalid_variant_frame = {
        let bad_payload = [0xFF, 0xFF, 0xFF, 0xFF, 0x7F];
        let mut buf = (bad_payload.len() as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(&bad_payload);
        buf
    };
    let bad_err = decode_length_prefixed(&invalid_variant_frame)
        .expect_err("Invalid discriminant must fail with ProtocolError::Serialization");
    assert!(matches!(bad_err, ProtocolError::Serialization(_)));
}
