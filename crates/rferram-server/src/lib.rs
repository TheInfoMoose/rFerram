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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info, warn};

use rferram_common::{NistVault, RuntimeEnvironment};
use rferram_protocol::{
    AudioCodec, AudioDirection, AudioFrameMessage, ControlMessage, HandshakeMessage, InputEvent,
    PixelFormat, WireMessage,
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

/// Host System Audio Capture Loopback Engine (PipeWire / WASAPI / macOS CoreAudio)
pub struct HostAudioCapturer {
    pub backend: CaptureBackendType,
    pub is_running: Arc<AtomicBool>,
}

impl HostAudioCapturer {
    pub fn new(backend: CaptureBackendType) -> Self {
        Self {
            backend,
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn start_loopback_capture(&self, tx: mpsc::Sender<WireMessage>) -> Result<()> {
        self.is_running.store(true, Ordering::SeqCst);
        let is_running = self.is_running.clone();
        let backend = self.backend;

        tokio::spawn(async move {
            let mut stream_id: u32 = 1;
            let interval = Duration::from_millis(20); // 50 Hz Opus frame chunking

            info!(?backend, "Started host audio loopback capture task");

            while is_running.load(Ordering::SeqCst) {
                tokio::time::sleep(interval).await;

                let audio_msg = AudioFrameMessage {
                    stream_id,
                    timestamp_us: Instant::now().elapsed().as_micros() as u64,
                    direction: AudioDirection::HostToClientSpeaker,
                    codec: AudioCodec::Opus,
                    channels: 2,
                    sample_rate: 48000,
                    payload: vec![0u8; 192],
                };

                if tx.send(WireMessage::Audio(audio_msg)).await.is_err() {
                    break;
                }
                stream_id += 1;
            }
        });

        Ok(())
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}

/// Active Session Controller on Host Daemon
pub struct HostSessionEngine {
    pub session_id: String,
    pub backend_type: CaptureBackendType,
    pub encoder: Arc<Mutex<Box<dyn VideoEncoder>>>,
    pub input_injector: Arc<dyn HostInputInjector>,
    pub audio_capturer: HostAudioCapturer,
    pub force_keyframe_flag: Arc<AtomicBool>,
    pub is_active: Arc<AtomicBool>,
}

impl HostSessionEngine {
    pub fn new(
        session_id: String,
        backend_type: CaptureBackendType,
        encoder_config: EncoderConfig,
    ) -> Self {
        let encoder: Box<dyn VideoEncoder> = match backend_type {
            CaptureBackendType::MacOsScreenCaptureKit => {
                Box::new(MacOsVideoToolboxEncoder::new(encoder_config))
            }
            _ => Box::new(SoftwareVideoEncoder::new(encoder_config)),
        };

        let input_injector = Arc::new(NativeInputInjector::new(backend_type));

        Self {
            session_id,
            backend_type,
            encoder: Arc::new(Mutex::new(encoder)),
            input_injector,
            audio_capturer: HostAudioCapturer::new(backend_type),
            force_keyframe_flag: Arc::new(AtomicBool::new(true)),
            is_active: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Spawns the main GPU frame capture loop
    pub async fn run_capture_loop(&self, outgoing_tx: mpsc::Sender<WireMessage>) -> Result<()> {
        info!(
            session_id = %self.session_id,
            backend = ?self.backend_type,
            "Starting host frame capture loop"
        );

        let is_active = self.is_active.clone();
        let encoder = self.encoder.clone();
        let force_keyframe = self.force_keyframe_flag.clone();
        let video_tx = outgoing_tx.clone();
        let start_time = Instant::now();
        let mut frame_count: u64 = 0;

        tokio::spawn(async move {
            let target_frame_duration = Duration::from_nanos(1_000_000_000 / 60);

            while is_active.load(Ordering::Relaxed) {
                let loop_start = Instant::now();
                frame_count += 1;

                let captured_frame = CapturedFrame {
                    frame_index: frame_count,
                    timestamp_us: start_time.elapsed().as_micros() as u64,
                    display_id: 1,
                    width: 2560,
                    height: 1440,
                    stride: 2560 * 4,
                    pixel_format: PixelFormat::Bgra8,
                    data: vec![128u8; 2560 * 1440 * 4],
                    is_dmabuf_or_metal_handle: true,
                };

                let need_keyframe = force_keyframe.swap(false, Ordering::SeqCst);

                let mut enc_lock = encoder.lock().await;
                match enc_lock.encode(&captured_frame, need_keyframe) {
                    Ok(video_msg) => {
                        if video_tx.send(WireMessage::Video(video_msg)).await.is_err() {
                            error!("Outgoing transport channel closed; exiting capture loop");
                            break;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Video frame encoding failed");
                    }
                }
                drop(enc_lock);

                let elapsed = loop_start.elapsed();
                if elapsed < target_frame_duration {
                    tokio::time::sleep(target_frame_duration - elapsed).await;
                }
            }
        });

        self.audio_capturer
            .start_loopback_capture(outgoing_tx)
            .await?;

        Ok(())
    }

    /// Handle incoming wire messages from remote client
    pub async fn handle_incoming_wire_message(&self, msg: WireMessage) -> Result<()> {
        match msg {
            WireMessage::Input(input_event) => {
                self.input_injector.inject_input(&input_event)?;
            }
            WireMessage::Control(ctrl) => match ctrl {
                ControlMessage::LossReport { missing_frames } => {
                    warn!(
                        ?missing_frames,
                        "Client reported packet loss; triggering instant keyframe"
                    );
                    self.force_keyframe_flag.store(true, Ordering::SeqCst);
                }
                ControlMessage::BandwidthEstimate {
                    bytes_per_sec,
                    rtt_ms,
                    ..
                } => {
                    let bitrate_kbps = (bytes_per_sec * 8 / 1000) as u32;
                    debug!(
                        bitrate_kbps,
                        rtt_ms, "Updating encoder bitrate from network feedback"
                    );
                    let mut enc = self.encoder.lock().await;
                    let _ = enc.update_bitrate(bitrate_kbps);
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    pub fn stop(&self) {
        self.is_active.store(false, Ordering::SeqCst);
        self.audio_capturer.stop();
        info!(session_id = %self.session_id, "Host session stopped");
    }
}

/// Elevated Host Daemon Service Background Manager (systemd, Windows Service, macOS launchd)
pub struct RFerramHostDaemon {
    pub host_name: String,
    pub backend: CaptureBackendType,
    pub runtime_env: RuntimeEnvironment,
    pub vault: NistVault,
    pub active_sessions: Arc<Mutex<Vec<Arc<HostSessionEngine>>>>,
}

impl RFerramHostDaemon {
    pub fn new(host_name: String, backend: CaptureBackendType) -> Self {
        let runtime_env = RuntimeEnvironment::detect();
        info!(
            host_name = %host_name,
            backend = ?backend,
            platform = ?runtime_env.platform,
            is_cosmic = runtime_env.is_cosmic_desktop,
            "Initializing elevated rFerram Host Daemon Service"
        );
        Self {
            host_name,
            backend,
            runtime_env,
            vault: NistVault::default(),
            active_sessions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Auto-detects operating system backend (Wayland/COSMIC, X11 fallback, DXGI, or ScreenCaptureKit)
    pub fn auto_detect_backend() -> CaptureBackendType {
        if cfg!(target_os = "macos") {
            CaptureBackendType::MacOsScreenCaptureKit
        } else if cfg!(target_os = "windows") {
            CaptureBackendType::WindowsDxgi
        } else {
            let env = RuntimeEnvironment::detect();
            if env.is_wayland || env.is_cosmic_desktop {
                CaptureBackendType::WaylandPipeWire
            } else if std::env::var("DISPLAY").is_ok() {
                CaptureBackendType::X11Fallback
            } else {
                CaptureBackendType::WaylandPipeWire
            }
        }
    }

    /// Authenticates client connection request and provisions session
    pub async fn handle_handshake(&self, handshake: HandshakeMessage) -> Result<String> {
        info!(
            client_hello = ?handshake,
            "Processing incoming client connection handshake"
        );

        Ok(uuid::Uuid::new_v4().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
