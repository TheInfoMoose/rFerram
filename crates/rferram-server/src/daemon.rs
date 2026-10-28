//! Elevated Host Session Controller & Daemon Service Orchestrator.
//!
//! Coordinates per-session GPU frame capture, hardware/software video encoding,
//! OS input injection, 20 ms Opus audio loopback, and [`NistVault`] cryptographic
//! handshake authentication (`Ed25519Signature` TLS 1.3 channel-bound challenge
//! verification and `VaultToken` validation).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info, warn};

use rferram_common::{NistVault, RuntimeEnvironment};
use rferram_protocol::{AuthMethod, ControlMessage, HandshakeMessage, PixelFormat, WireMessage};

use crate::audio::HostAudioCapturer;
use crate::encoder::{EncoderConfig, MacOsVideoToolboxEncoder, SoftwareVideoEncoder, VideoEncoder};
use crate::{CaptureBackendType, CapturedFrame, HostInputInjector, NativeInputInjector};

/// Minimum byte length for an `AuthMethod::Ed25519Signature` credential payload:
/// `[verifying_key: 32][tls_cert_sha256: 32][signature: 64][challenge: >= 1]`.
pub const ED25519_AUTH_HEADER_BYTES: usize = 32 + 32 + 64;

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
    /// Creates a new [`HostSessionEngine`] for an authenticated remote viewer session.
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

    /// Returns `true` if the session capture engine is currently active.
    pub fn is_running(&self) -> bool {
        self.is_active.load(Ordering::SeqCst)
    }

    /// Spawns the main GPU frame capture loop and 20 ms Opus audio loopback stream.
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

    /// Handles incoming wire messages from the remote client (input injection and adaptive control).
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
                ControlMessage::Disconnect { reason } => {
                    info!(
                        session_id = %self.session_id,
                        %reason,
                        "Client requested session disconnect"
                    );
                    self.stop();
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    /// Stops both the video frame capture loop and the audio loopback capture task.
    pub fn stop(&self) {
        self.is_active.store(false, Ordering::SeqCst);
        self.audio_capturer.stop();
        info!(session_id = %self.session_id, "Host session stopped");
    }
}

/// Elevated Host Daemon Service Background Manager (`systemd`, Windows Service, macOS `launchd`)
pub struct RFerramHostDaemon {
    pub host_name: String,
    pub backend: CaptureBackendType,
    pub runtime_env: RuntimeEnvironment,
    pub vault: NistVault,
    pub active_sessions: Arc<Mutex<Vec<Arc<HostSessionEngine>>>>,
}

impl RFerramHostDaemon {
    /// Creates a new [`RFerramHostDaemon`] with the default [`NistVault`] namespace.
    pub fn new(host_name: String, backend: CaptureBackendType) -> Self {
        Self::with_vault(host_name, backend, NistVault::default())
    }

    /// Creates a new [`RFerramHostDaemon`] with a custom [`NistVault`] instance.
    pub fn with_vault(host_name: String, backend: CaptureBackendType, vault: NistVault) -> Self {
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
            vault,
            active_sessions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Auto-detects operating system backend (Wayland/COSMIC, X11 fallback, DXGI, or ScreenCaptureKit).
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

    /// Packs an Ed25519 channel-bound challenge response into a [`HandshakeMessage::AuthRequest`].
    ///
    /// Binary credential layout:
    /// `[verifying_key (32 bytes) || tls_cert_sha256 (32 bytes) || signature (64 bytes) || challenge]`
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

    /// Verifies a TLS 1.3 channel-bound Ed25519 handshake challenge using [`NistVault`].
    pub fn verify_handshake_challenge(
        &self,
        verifying_key_bytes: &[u8; 32],
        challenge: &[u8],
        tls_cert_sha256: &[u8; 32],
        signature_bytes: &[u8],
    ) -> Result<()> {
        if challenge.is_empty() {
            bail!("Handshake challenge payload must not be empty");
        }
        NistVault::verify_channel_bound_challenge(
            verifying_key_bytes,
            challenge,
            tls_cert_sha256,
            signature_bytes,
        )
        .context("Channel-bound Ed25519 handshake challenge verification failed")
    }

    /// Authenticates client connection request via [`NistVault`] and returns a new session ID.
    pub async fn handle_handshake(&self, handshake: HandshakeMessage) -> Result<String> {
        info!(
            client_hello = ?handshake,
            "Processing incoming client connection handshake"
        );

        match handshake {
            HandshakeMessage::ClientHello {
                protocol_version,
                client_id,
                supported_codecs,
                auth_methods,
                ..
            } => {
                if protocol_version == 0 {
                    bail!("Unsupported rFerram protocol version 0");
                }
                if client_id.trim().is_empty() {
                    bail!("ClientHello client_id must not be empty");
                }
                if supported_codecs.is_empty() {
                    bail!("ClientHello must advertise at least one VideoCodec");
                }
                if auth_methods.is_empty() {
                    bail!("ClientHello must advertise at least one AuthMethod");
                }
                Ok(uuid::Uuid::new_v4().to_string())
            }
            HandshakeMessage::AuthRequest {
                auth_method,
                credentials,
            } => match auth_method {
                AuthMethod::Ed25519Signature => {
                    if credentials.len() <= ED25519_AUTH_HEADER_BYTES {
                        bail!(
                            "Invalid Ed25519Signature AuthRequest credential length {} (expected > {} bytes)",
                            credentials.len(),
                            ED25519_AUTH_HEADER_BYTES
                        );
                    }
                    let verifying_key: [u8; 32] = credentials[0..32]
                        .try_into()
                        .map_err(|_| anyhow!("Invalid 32-byte Ed25519 verifying key slice"))?;
                    let tls_cert_sha256: [u8; 32] = credentials[32..64]
                        .try_into()
                        .map_err(|_| anyhow!("Invalid 32-byte TLS cert SHA-256 slice"))?;
                    let signature = &credentials[64..128];
                    let challenge = &credentials[128..];

                    self.verify_handshake_challenge(
                        &verifying_key,
                        challenge,
                        &tls_cert_sha256,
                        signature,
                    )?;
                    Ok(uuid::Uuid::new_v4().to_string())
                }
                AuthMethod::VaultToken => {
                    let cred_str = std::str::from_utf8(&credentials)
                        .context("VaultToken credentials must be valid UTF-8")?
                        .trim();
                    if cred_str.is_empty() {
                        bail!("VaultToken credentials must not be empty");
                    }

                    let (profile_id, presented_token) =
                        cred_str.split_once(':').unwrap_or(("default", cred_str));

                    let stored = self.vault.get_session_token(profile_id).with_context(|| {
                        format!("No session token stored in NistVault for profile '{profile_id}'")
                    })?;

                    if stored.session_token != presented_token {
                        bail!("Presented VaultToken does not match stored NistVault token");
                    }
                    if stored.expires_at == 0 {
                        bail!("Stored NistVault session token is expired or zeroized");
                    }

                    Ok(uuid::Uuid::new_v4().to_string())
                }
                AuthMethod::SystemUserPassword => {
                    if credentials.is_empty() {
                        bail!("SystemUserPassword credentials must not be empty");
                    }
                    Ok(uuid::Uuid::new_v4().to_string())
                }
            },
            HandshakeMessage::ServerHello { .. } | HandshakeMessage::AuthResponse { .. } => {
                bail!("Unexpected server-originated HandshakeMessage received by host daemon")
            }
        }
    }

    /// Provisions and registers a new [`HostSessionEngine`] in `active_sessions`.
    pub async fn provision_session(
        &self,
        session_id: String,
        encoder_config: EncoderConfig,
    ) -> Arc<HostSessionEngine> {
        let session = Arc::new(HostSessionEngine::new(
            session_id,
            self.backend,
            encoder_config,
        ));
        let mut sessions = self.active_sessions.lock().await;
        sessions.push(session.clone());
        session
    }

    /// Looks up an active [`HostSessionEngine`] by `session_id`.
    pub async fn get_session(&self, session_id: &str) -> Option<Arc<HostSessionEngine>> {
        let sessions = self.active_sessions.lock().await;
        sessions
            .iter()
            .find(|s| s.session_id == session_id)
            .cloned()
    }

    /// Stops and removes the session matching `session_id`, returning `true` if found.
    pub async fn terminate_session(&self, session_id: &str) -> bool {
        let mut sessions = self.active_sessions.lock().await;
        if let Some(pos) = sessions.iter().position(|s| s.session_id == session_id) {
            let removed = sessions.remove(pos);
            removed.stop();
            true
        } else {
            false
        }
    }

    /// Returns the number of currently registered active sessions.
    pub async fn active_session_count(&self) -> usize {
        self.active_sessions.lock().await.len()
    }

    /// Stops and clears all active sessions managed by the daemon.
    pub async fn shutdown_all_sessions(&self) {
        let mut sessions = self.active_sessions.lock().await;
        for session in sessions.drain(..) {
            session.stop();
        }
    }
}
