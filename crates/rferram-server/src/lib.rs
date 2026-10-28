//! `rferram-server`: Elevated Remote Host Capture Engine & System Daemon
//!
//! Provides zero-copy GPU frame capture across operating systems:
//! - Wayland DMABUF / PipeWire / COSMIC Compositor via Smithay & Wayland protocols (Linux: Debian, RHEL, SUSE, Arch)
//! - Windows DXGI Desktop Duplication API (Windows)
//! - ScreenCaptureKit zero-copy Metal GPU capture & VideoToolbox HW encoding (macOS)
//! - X11 SHM / XFIXES legacy fallback
//!
//! Handles hardware video encoding (AV1 via hardware/rav1e, H.265, H.264, Apple VideoToolbox),
//! native OS input injection (Wayland virtual pointer/keys, Windows SendInput, macOS CGEvent),
//! duplex audio loopback capture (PipeWire, WASAPI, macOS CoreAudio),
//! and elevated daemon session orchestration (`systemd`, Windows Service, macOS `launchd`).

use anyhow::Result;

use rferram_protocol::{InputEvent, PixelFormat};

pub mod audio;
pub use audio::{
    HostAudioCapturer, OPUS_CHANNELS, OPUS_FRAME_INTERVAL_MS, OPUS_PAYLOAD_BYTES,
    OPUS_SAMPLE_RATE_HZ,
};

pub mod backends;
pub use backends::{
    CG_EVENT_FLAG_MASK_ALTERNATE, CG_EVENT_FLAG_MASK_COMMAND, CG_EVENT_FLAG_MASK_CONTROL,
    CG_EVENT_FLAG_MASK_SHIFT, CV_PIXEL_FORMAT_32BGRA, CV_PIXEL_FORMAT_32RGBA, CV_PIXEL_FORMAT_420F,
    CV_PIXEL_FORMAT_420V, CV_PIXEL_FORMAT_Y420, CV_PIXEL_FORMAT_Y444, CgMouseEventKind,
    DRM_FORMAT_ABGR8888, DRM_FORMAT_ARGB8888, DRM_FORMAT_NV12, DRM_FORMAT_XBGR8888,
    DRM_FORMAT_XRGB8888, DXGI_FORMAT_B8G8R8A8_UNORM_RAW, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB_RAW,
    DXGI_FORMAT_NV12_RAW, DXGI_FORMAT_R8G8B8A8_UNORM_RAW, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB_RAW,
    DmaBufPlaneDescriptor, DxgiSubresourceLayout, EVDEV_BTN_EXTRA, EVDEV_BTN_LEFT,
    EVDEV_BTN_MIDDLE, EVDEV_BTN_RIGHT, EVDEV_BTN_SIDE, FrameCapturer, GbmDmabufSurface,
    IoSurfacePlaneDescriptor, LinuxCaptureController, LinuxInputController, MacOsCgEventInjector,
    MacOsIoSurfaceDescriptor, MacOsScreenCaptureKitCapturer, NormalizedPointerCoords,
    TranslatedCgEvent, TranslatedWin32Input, VK_CONTROL_CODE, VK_LWIN_CODE, VK_MENU_CODE,
    VK_SHIFT_CODE, WIN32_KEYEVENTF_EXTENDEDKEY, WIN32_KEYEVENTF_KEYUP, WIN32_KEYEVENTF_SCANCODE,
    WIN32_KEYEVENTF_UNICODE, WIN32_MOUSEEVENTF_ABSOLUTE, WIN32_MOUSEEVENTF_HWHEEL,
    WIN32_MOUSEEVENTF_LEFTDOWN, WIN32_MOUSEEVENTF_LEFTUP, WIN32_MOUSEEVENTF_MIDDLEDOWN,
    WIN32_MOUSEEVENTF_MIDDLEUP, WIN32_MOUSEEVENTF_MOVE, WIN32_MOUSEEVENTF_RIGHTDOWN,
    WIN32_MOUSEEVENTF_RIGHTUP, WIN32_MOUSEEVENTF_VIRTUALDESK, WIN32_MOUSEEVENTF_WHEEL,
    WIN32_MOUSEEVENTF_XDOWN, WIN32_MOUSEEVENTF_XUP, WIN32_WHEEL_DELTA, WIN32_XBUTTON1,
    WIN32_XBUTTON2, WaylandDmabufCapturer, WaylandVirtualInputInjector, WindowsDxgiCapturer,
    WindowsDxgiSurfaceDescriptor, WindowsSendInputInjector, X11_BUTTON_BACK, X11_BUTTON_FORWARD,
    X11_BUTTON_LEFT, X11_BUTTON_MIDDLE, X11_BUTTON_RIGHT, X11_BUTTON_SCROLL_DOWN,
    X11_BUTTON_SCROLL_LEFT, X11_BUTTON_SCROLL_RIGHT, X11_BUTTON_SCROLL_UP, X11ShmCapturer,
    X11XTestInputInjector, XKB_EVDEV_OFFSET, encode_win32_modifier_transitions,
    encode_xkb_modifiers_mask, evdev_to_x11_keycode, map_cv_pixel_format_to_wire,
    map_drm_fourcc_to_pixel_format, map_dxgi_format_to_wire, map_scancode_to_macos_keycode,
    map_scancode_to_win32_vk, mouse_button_to_evdev, mouse_button_to_x11_detail,
    normalize_pointer_coordinates, normalize_win32_absolute_coords,
    normalize_win32_virtual_desktop_coords, translate_input_event_to_cg,
    translate_input_event_to_win32, translate_key_to_evdev_scancode,
    translate_modifiers_to_cg_flags,
};

pub mod daemon;
pub use daemon::{ED25519_AUTH_HEADER_BYTES, HostSessionEngine, RFerramHostDaemon};

pub mod encoder;
pub use encoder::{EncoderConfig, MacOsVideoToolboxEncoder, SoftwareVideoEncoder, VideoEncoder};

/// Operating System Display & Frame Capture Backend
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureBackendType {
    /// Wayland DMABUF / PipeWire capture managed via Smithay / COSMIC backend (Linux)
    WaylandPipeWire,
    /// Windows DXGI Desktop Duplication API (Windows)
    WindowsDxgi,
    /// macOS ScreenCaptureKit zero-copy frame capture (macOS 12.3+)
    MacOsScreenCaptureKit,
    /// X11 SHM / XFIXES fallback (Legacy Linux)
    X11Fallback,
}

/// Raw Frame Buffer captured from Host GPU / Display Subsystem
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub frame_index: u64,
    pub timestamp_us: u64,
    pub display_id: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: PixelFormat,
    pub data: Vec<u8>,
    pub is_dmabuf_or_metal_handle: bool,
}

/// Abstract Native Host Input Injector (Wayland Smithay, Windows SendInput & macOS CGEvent)
pub trait HostInputInjector: Send + Sync {
    fn inject_input(&self, event: &InputEvent) -> Result<()>;
}

/// Cross-Platform Native Input Injector Implementation
pub struct NativeInputInjector {
    backend: CaptureBackendType,
    linux_controller: Option<LinuxInputController>,
    macos_injector: Option<MacOsCgEventInjector>,
    windows_injector: Option<WindowsSendInputInjector>,
}

impl NativeInputInjector {
    pub fn new(backend: CaptureBackendType) -> Self {
        let linux_controller = match backend {
            CaptureBackendType::WaylandPipeWire | CaptureBackendType::X11Fallback => {
                LinuxInputController::new(backend, 2560, 1440).ok()
            }
            _ => None,
        };
        let macos_injector = match backend {
            CaptureBackendType::MacOsScreenCaptureKit => {
                Some(MacOsCgEventInjector::new_live(2560, 1440))
            }
            _ => None,
        };
        let windows_injector = match backend {
            CaptureBackendType::WindowsDxgi => Some(WindowsSendInputInjector::new_live(2560, 1440)),
            _ => None,
        };
        Self {
            backend,
            linux_controller,
            macos_injector,
            windows_injector,
        }
    }
}

impl HostInputInjector for NativeInputInjector {
    fn inject_input(&self, event: &InputEvent) -> Result<()> {
        match self.backend {
            CaptureBackendType::MacOsScreenCaptureKit => {
                if let Some(injector) = &self.macos_injector {
                    injector.inject_input(event)?;
                }
            }
            CaptureBackendType::WaylandPipeWire | CaptureBackendType::X11Fallback => {
                if let Some(ctrl) = &self.linux_controller {
                    ctrl.inject_input(event)?;
                }
            }
            CaptureBackendType::WindowsDxgi => {
                if let Some(injector) = &self.windows_injector {
                    injector.inject_input(event)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc;

    use rferram_common::{NistVault, VaultTokenModel};
    use rferram_protocol::{
        AudioCodec, AudioDirection, AuthMethod, ControlMessage, HandshakeMessage, VideoCodec,
        WireMessage,
    };

    #[tokio::test]
    async fn test_forced_keyframe_on_packet_loss() {
        let mut encoder = SoftwareVideoEncoder::new(EncoderConfig {
            keyframe_interval: 5,
            ..Default::default()
        });
        let dummy_frame = CapturedFrame {
            frame_index: 1,
            timestamp_us: 100,
            display_id: 1,
            width: 1920,
            height: 1080,
            stride: 1920 * 4,
            pixel_format: PixelFormat::Bgra8,
            data: vec![0u8; 2048],
            is_dmabuf_or_metal_handle: true,
        };

        // Frame 1 is an initial keyframe
        let msg1 = encoder.encode(&dummy_frame, false).unwrap();
        assert!(msg1.is_keyframe);

        // Frame 2 is a normal inter-frame
        let msg2 = encoder.encode(&dummy_frame, false).unwrap();
        assert!(!msg2.is_keyframe);

        // Frame 3 with force_keyframe = true (packet loss recovery)
        let msg3 = encoder.encode(&dummy_frame, true).unwrap();
        assert!(msg3.is_keyframe);

        // Frame 4 returns to normal inter-frame
        let msg4 = encoder.encode(&dummy_frame, false).unwrap();
        assert!(!msg4.is_keyframe);

        // Frame 5 hits keyframe_interval multiple (5)
        let msg5 = encoder.encode(&dummy_frame, false).unwrap();
        assert!(msg5.is_keyframe);

        // Verify ControlMessage::LossReport triggers force_keyframe_flag on HostSessionEngine
        let session = HostSessionEngine::new(
            "test-session".to_string(),
            CaptureBackendType::WaylandPipeWire,
            EncoderConfig::default(),
        );
        session.force_keyframe_flag.store(false, Ordering::SeqCst);
        session
            .handle_incoming_wire_message(WireMessage::Control(ControlMessage::LossReport {
                missing_frames: vec![2, 3],
            }))
            .await
            .unwrap();
        assert!(session.force_keyframe_flag.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_handshake_challenge_and_vault_token_verification() {
        let vault = NistVault::new("test.rferram.server.handshake");
        let daemon = RFerramHostDaemon::with_vault(
            "test-host-daemon".to_string(),
            CaptureBackendType::WaylandPipeWire,
            vault,
        );

        // 1. Valid ClientHello negotiation returns a session UUID
        let hello = HandshakeMessage::ClientHello {
            protocol_version: 1,
            client_id: "viewer-client-01".to_string(),
            client_name: "COSMIC Viewer".to_string(),
            supported_codecs: vec![VideoCodec::AV1, VideoCodec::H265],
            auth_methods: vec![AuthMethod::Ed25519Signature, AuthMethod::VaultToken],
        };
        let hello_session_id = daemon
            .handle_handshake(hello)
            .await
            .expect("Valid ClientHello should succeed");
        assert!(!hello_session_id.is_empty());

        // Reject ClientHello with protocol_version 0
        let bad_hello = HandshakeMessage::ClientHello {
            protocol_version: 0,
            client_id: "viewer-client-01".to_string(),
            client_name: "COSMIC Viewer".to_string(),
            supported_codecs: vec![VideoCodec::AV1],
            auth_methods: vec![AuthMethod::Ed25519Signature],
        };
        assert!(daemon.handle_handshake(bad_hello).await.is_err());

        // 2. Ed25519 TLS 1.3 channel-bound challenge verification
        let (signing_key, verifying_key) = daemon
            .vault
            .get_or_create_ed25519_identity("server_handshake_test_peer")
            .expect("Should generate Ed25519 identity");
        let challenge = b"rferram-server-nonce-challenge-32bytes";
        let tls_cert_sha256 = [0x5Au8; 32];
        let signature =
            NistVault::sign_channel_bound_challenge(&signing_key, challenge, &tls_cert_sha256);

        let auth_req = RFerramHostDaemon::build_ed25519_auth_request(
            &verifying_key.to_bytes(),
            &tls_cert_sha256,
            challenge,
            &signature,
        );
        let ed_session_id = daemon
            .handle_handshake(auth_req)
            .await
            .expect("Valid channel-bound Ed25519 AuthRequest should succeed");
        assert!(!ed_session_id.is_empty());

        // Tampered TLS certificate fingerprint must fail MITM channel binding check
        let wrong_cert_sha256 = [0xFFu8; 32];
        let mitm_req = RFerramHostDaemon::build_ed25519_auth_request(
            &verifying_key.to_bytes(),
            &wrong_cert_sha256,
            challenge,
            &signature,
        );
        assert!(
            daemon.handle_handshake(mitm_req).await.is_err(),
            "Mismatched TLS certificate fingerprint must be rejected"
        );

        // Tampered signature bytes must fail verification
        let mut bad_sig = signature.clone();
        bad_sig[0] ^= 0xFF;
        let bad_sig_req = RFerramHostDaemon::build_ed25519_auth_request(
            &verifying_key.to_bytes(),
            &tls_cert_sha256,
            challenge,
            &bad_sig,
        );
        assert!(
            daemon.handle_handshake(bad_sig_req).await.is_err(),
            "Tampered Ed25519 signature must be rejected"
        );

        // 3. NistVault session token authentication
        let token_model = VaultTokenModel {
            session_token: "nist-vault-secret-session-token-99".to_string(),
            auth_method: "Ed25519+P2P".to_string(),
            expires_at: 2_000_000_000,
        };
        daemon
            .vault
            .store_session_token("workstation_alpha", &token_model)
            .expect("Should store session token in NistVault");

        let valid_token_req = HandshakeMessage::AuthRequest {
            auth_method: AuthMethod::VaultToken,
            credentials: b"workstation_alpha:nist-vault-secret-session-token-99".to_vec(),
        };
        let token_session_id = daemon
            .handle_handshake(valid_token_req)
            .await
            .expect("Matching NistVault token should authenticate");
        assert!(!token_session_id.is_empty());

        let invalid_token_req = HandshakeMessage::AuthRequest {
            auth_method: AuthMethod::VaultToken,
            credentials: b"workstation_alpha:wrong-token-value".to_vec(),
        };
        assert!(
            daemon.handle_handshake(invalid_token_req).await.is_err(),
            "Mismatched NistVault token must be rejected"
        );

        let _ = daemon.vault.delete_session_token("workstation_alpha");
    }

    #[tokio::test]
    async fn test_daemon_session_lifecycle_and_audio_loopback() {
        let daemon = RFerramHostDaemon::with_vault(
            "lifecycle-host".to_string(),
            CaptureBackendType::WaylandPipeWire,
            NistVault::new("test.rferram.server.lifecycle"),
        );
        assert_eq!(daemon.active_session_count().await, 0);

        let hello = HandshakeMessage::ClientHello {
            protocol_version: 1,
            client_id: "client-lifecycle-1".to_string(),
            client_name: "Linux Viewer".to_string(),
            supported_codecs: vec![VideoCodec::AV1],
            auth_methods: vec![AuthMethod::Ed25519Signature],
        };
        let session_id_1 = daemon.handle_handshake(hello).await.unwrap();
        let session_1 = daemon
            .provision_session(session_id_1.clone(), EncoderConfig::default())
            .await;
        let session_2 = daemon
            .provision_session("session-2".to_string(), EncoderConfig::default())
            .await;

        assert_eq!(daemon.active_session_count().await, 2);
        assert!(session_1.is_running());
        assert!(session_2.is_running());
        assert!(daemon.get_session(&session_id_1).await.is_some());

        // Start capture + 20ms Opus audio loopback on session_1 and verify emitted frames
        let (wire_tx, mut wire_rx) = mpsc::channel(32);
        session_1.run_capture_loop(wire_tx).await.unwrap();
        assert!(session_1.audio_capturer.is_capturing());

        let mut saw_video = false;
        let mut saw_audio = false;
        for _ in 0..10 {
            if let Some(msg) = wire_rx.recv().await {
                match msg {
                    WireMessage::Video(v) => {
                        assert_eq!(v.width, 2560);
                        assert_eq!(v.height, 1440);
                        saw_video = true;
                    }
                    WireMessage::Audio(a) => {
                        assert_eq!(a.codec, AudioCodec::Opus);
                        assert_eq!(a.direction, AudioDirection::HostToClientSpeaker);
                        assert_eq!(a.channels, OPUS_CHANNELS);
                        assert_eq!(a.sample_rate, OPUS_SAMPLE_RATE_HZ);
                        assert_eq!(a.payload.len(), OPUS_PAYLOAD_BYTES);
                        saw_audio = true;
                    }
                    _ => {}
                }
                if saw_video && saw_audio {
                    break;
                }
            }
        }
        assert!(saw_video, "Capture loop should emit VideoFrameMessage");
        assert!(
            saw_audio,
            "Audio loopback should emit 20ms Opus AudioFrameMessage"
        );

        // Terminate session_1 and verify it stops and is removed from active_sessions
        assert!(daemon.terminate_session(&session_id_1).await);
        assert!(!session_1.is_running());
        assert!(!session_1.audio_capturer.is_capturing());
        assert_eq!(daemon.active_session_count().await, 1);
        assert!(!daemon.terminate_session("non-existent-session").await);

        // Shutdown all remaining sessions
        daemon.shutdown_all_sessions().await;
        assert!(!session_2.is_running());
        assert_eq!(daemon.active_session_count().await, 0);
    }
}
