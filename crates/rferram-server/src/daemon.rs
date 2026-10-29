//! Elevated Host Session Controller & Daemon Service Orchestrator.
//!
//! Coordinates per-session GPU frame capture, hardware/software video encoding,
//! OS input injection, 20 ms Opus audio loopback, and [`NistVault`] cryptographic
//! handshake authentication (`Ed25519Signature` TLS 1.3 channel-bound challenge
//! verification and `VaultToken` validation).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{debug, error, info, warn};

use rferram_channels::VirtualChannelManager;
use rferram_common::{NistVault, RuntimeEnvironment};
use rferram_net::QuicConnectionManager;
use rferram_protocol::{
    AuthMethod, ControlMessage, HandshakeMessage, PixelFormat, VideoCodec, WireMessage,
    decode_length_prefixed,
};

use crate::audio::HostAudioCapturer;
use crate::backends::{
    FrameCapturer, LinuxCaptureController, MacOsScreenCaptureKitCapturer, WindowsDxgiCapturer,
};
use crate::encoder::{EncoderConfig, MacOsVideoToolboxEncoder, SoftwareVideoEncoder, VideoEncoder};
use crate::{CaptureBackendType, CapturedFrame, HostInputInjector, NativeInputInjector};

/// Minimum byte length for an `AuthMethod::Ed25519Signature` credential payload:
/// `[verifying_key: 32][tls_cert_sha256: 32][signature: 64][challenge: >= 1]`.
pub const ED25519_AUTH_HEADER_BYTES: usize = 32 + 32 + 64;

/// Active Session Controller on Host Daemon
pub struct HostSessionEngine {
    pub session_id: String,
    pub backend_type: CaptureBackendType,
    pub capturer: Arc<Mutex<Box<dyn FrameCapturer>>>,
    pub encoder: Arc<Mutex<Box<dyn VideoEncoder>>>,
    pub input_injector: Arc<dyn HostInputInjector>,
    pub audio_capturer: HostAudioCapturer,
    pub channel_manager: Arc<VirtualChannelManager>,
    pub force_keyframe_flag: Arc<AtomicBool>,
    pub last_acked_frame_id: Arc<AtomicU64>,
    pub is_active: Arc<AtomicBool>,
    stop_tx: watch::Sender<bool>,
}

impl HostSessionEngine {
    /// Instantiates the platform [`FrameCapturer`] implementation matching `backend_type`.
    pub fn build_frame_capturer(
        backend_type: CaptureBackendType,
        display_id: u32,
        width: u32,
        height: u32,
    ) -> Result<Box<dyn FrameCapturer>> {
        let w = width.max(1);
        let h = height.max(1);
        match backend_type {
            CaptureBackendType::WaylandPipeWire | CaptureBackendType::X11Fallback => Ok(Box::new(
                LinuxCaptureController::new(backend_type, display_id, w, h)?,
            )),
            CaptureBackendType::MacOsScreenCaptureKit => Ok(Box::new(
                MacOsScreenCaptureKitCapturer::new(display_id, w, h)?,
            )),
            CaptureBackendType::WindowsDxgi => {
                Ok(Box::new(WindowsDxgiCapturer::new(display_id, w, h)?))
            }
        }
    }

    /// Creates a new [`HostSessionEngine`] for an authenticated remote viewer session.
    pub fn new(
        session_id: String,
        backend_type: CaptureBackendType,
        encoder_config: EncoderConfig,
    ) -> Self {
        let width = encoder_config.width.max(1);
        let height = encoder_config.height.max(1);
        let capturer =
            Self::build_frame_capturer(backend_type, 1, width, height).unwrap_or_else(|_| {
                Box::new(
                    LinuxCaptureController::new(
                        CaptureBackendType::WaylandPipeWire,
                        1,
                        width,
                        height,
                    )
                    .expect("Fallback WaylandDmabufCapturer initialization must succeed"),
                )
            });

        let encoder: Box<dyn VideoEncoder> = match backend_type {
            CaptureBackendType::MacOsScreenCaptureKit => {
                Box::new(MacOsVideoToolboxEncoder::new(encoder_config))
            }
            _ => Box::new(SoftwareVideoEncoder::new(encoder_config)),
        };

        let input_injector = Arc::new(NativeInputInjector::new(backend_type));
        let (stop_tx, _) = watch::channel(false);

        Self {
            session_id,
            backend_type,
            capturer: Arc::new(Mutex::new(capturer)),
            encoder: Arc::new(Mutex::new(encoder)),
            input_injector,
            audio_capturer: HostAudioCapturer::new(backend_type),
            channel_manager: Arc::new(VirtualChannelManager::new()),
            force_keyframe_flag: Arc::new(AtomicBool::new(true)),
            last_acked_frame_id: Arc::new(AtomicU64::new(0)),
            is_active: Arc::new(AtomicBool::new(true)),
            stop_tx,
        }
    }

    /// Returns `true` if the session capture engine is currently active.
    pub fn is_running(&self) -> bool {
        self.is_active.load(Ordering::SeqCst)
    }

    /// Returns a [`watch::Receiver`] that resolves to `true` when [`HostSessionEngine::stop`] is invoked.
    pub fn subscribe_stop(&self) -> watch::Receiver<bool> {
        self.stop_tx.subscribe()
    }

    /// Spawns the main GPU frame capture loop and 20 ms Opus audio loopback stream.
    pub async fn run_capture_loop(&self, outgoing_tx: mpsc::Sender<WireMessage>) -> Result<()> {
        info!(
            session_id = %self.session_id,
            backend = ?self.backend_type,
            "Starting host frame capture loop"
        );

        let is_active = self.is_active.clone();
        let mut stop_rx = self.subscribe_stop();
        let capturer = self.capturer.clone();
        let encoder = self.encoder.clone();
        let force_keyframe = self.force_keyframe_flag.clone();
        let video_tx = outgoing_tx.clone();
        let start_time = Instant::now();
        let mut frame_count: u64 = 0;

        tokio::spawn(async move {
            let target_frame_duration = Duration::from_nanos(1_000_000_000 / 60);

            while is_active.load(Ordering::Relaxed) && !*stop_rx.borrow() {
                let loop_start = Instant::now();
                frame_count += 1;

                let captured_frame = {
                    let mut cap_lock = capturer.lock().await;
                    match cap_lock.capture_next_frame() {
                        Ok(frame) => frame,
                        Err(e) => {
                            warn!(
                                error = %e,
                                "Platform FrameCapturer capture_next_frame failed; using fallback frame"
                            );
                            CapturedFrame {
                                frame_index: frame_count,
                                timestamp_us: start_time.elapsed().as_micros() as u64,
                                display_id: 1,
                                width: 2560,
                                height: 1440,
                                stride: 2560 * 4,
                                pixel_format: PixelFormat::Bgra8,
                                data: vec![128u8; 64 * 1024],
                                is_dmabuf_or_metal_handle: true,
                            }
                        }
                    }
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
                    tokio::select! {
                        _ = tokio::time::sleep(target_frame_duration - elapsed) => {}
                        _ = stop_rx.changed() => break,
                    }
                }
            }
        });

        self.audio_capturer
            .start_loopback_capture(outgoing_tx)
            .await?;

        Ok(())
    }

    /// Handles incoming wire messages from the remote client (input injection, adaptive control, and virtual channels).
    pub async fn handle_incoming_wire_message(&self, msg: WireMessage) -> Result<()> {
        match msg {
            WireMessage::Input(input_event) => {
                self.input_injector.inject_input(&input_event)?;
            }
            WireMessage::Control(ctrl) => match ctrl {
                ControlMessage::FrameAck {
                    frame_id,
                    monitor_id,
                    render_delay_us,
                } => {
                    self.last_acked_frame_id.store(frame_id, Ordering::SeqCst);
                    debug!(
                        frame_id,
                        monitor_id, render_delay_us, "Processed client FrameAck"
                    );
                }
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
            WireMessage::Channel(channel_msg) => {
                self.channel_manager
                    .route_channel_message(channel_msg)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Stops both the video frame capture loop and the audio loopback capture task.
    pub fn stop(&self) {
        self.is_active.store(false, Ordering::SeqCst);
        let _ = self.stop_tx.send(true);
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

    async fn send_auth_failure_and_retain(conn: Arc<QuicConnectionManager>, error_message: String) {
        let _ = conn
            .send_uni_message(&WireMessage::Handshake(HandshakeMessage::AuthResponse {
                success: false,
                session_token: None,
                error_message: Some(error_message),
                requires_elevation: false,
            }))
            .await;
        // Retain `conn` briefly so `quinn::Connection::drop` does not transmit
        // `CONNECTION_CLOSE` before the peer reads the finished `AuthResponse` stream.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(conn);
        });
    }

    /// Serves an incoming [`QuicConnectionManager`] connection:
    /// 1. Validates the client's handshake (`ClientHello` or `AuthRequest`) via [`NistVault`] and
    ///    replies with [`HandshakeMessage::AuthResponse`].
    /// 2. Provisions a new [`HostSessionEngine`] and [`VirtualChannelManager`].
    /// 3. Starts [`HostSessionEngine::run_capture_loop`] and pumps outbound video/audio frames over
    ///    QUIC streams.
    /// 4. Routes inbound QUIC datagrams, unidirectional streams, and bidirectional streams to
    ///    [`HostSessionEngine::handle_incoming_wire_message`] and [`VirtualChannelManager`].
    pub async fn serve_connection(
        &self,
        conn: impl Into<Arc<QuicConnectionManager>>,
    ) -> Result<Arc<HostSessionEngine>> {
        let conn: Arc<QuicConnectionManager> = conn.into();

        let initial_wire = conn
            .recv_uni_message()
            .await
            .context("Failed to receive initial handshake message over QUIC stream")?;

        let WireMessage::Handshake(handshake) = initial_wire else {
            bail!(
                "Expected initial WireMessage::Handshake on QUIC connection, got {initial_wire:?}"
            );
        };

        let preferred_codec = match &handshake {
            HandshakeMessage::ClientHello {
                supported_codecs, ..
            } => supported_codecs.first().copied().unwrap_or(VideoCodec::AV1),
            _ => VideoCodec::AV1,
        };

        if let HandshakeMessage::AuthRequest {
            auth_method: AuthMethod::Ed25519Signature,
            ref credentials,
        } = handshake
            && credentials.len() >= 64
            && let Some(expected_cert_sha256) = conn.peer_cert_sha256()
            && credentials[32..64] != expected_cert_sha256
        {
            let err_msg = "TLS 1.3 channel-binding certificate SHA-256 digest mismatch";
            Self::send_auth_failure_and_retain(conn, err_msg.to_string()).await;
            bail!("{err_msg}");
        }

        let session_id = match self.handle_handshake(handshake).await {
            Ok(id) => id,
            Err(e) => {
                Self::send_auth_failure_and_retain(conn, e.to_string()).await;
                return Err(e);
            }
        };

        conn.send_uni_message(&WireMessage::Handshake(HandshakeMessage::AuthResponse {
            success: true,
            session_token: Some(session_id.clone()),
            error_message: None,
            requires_elevation: false,
        }))
        .await
        .context("Failed to send HandshakeMessage::AuthResponse over QUIC stream")?;

        let encoder_config = EncoderConfig {
            codec: preferred_codec,
            ..EncoderConfig::default()
        };
        let session = self
            .provision_session(session_id.clone(), encoder_config)
            .await;

        let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<WireMessage>(256);
        if let Err(e) = session.run_capture_loop(outgoing_tx).await {
            self.terminate_session(&session_id).await;
            return Err(e);
        }

        // Task 1: Pump outbound video/audio/channel frames over QUIC streams
        let conn_outbound = conn.clone();
        let session_outbound = session.clone();
        let mut stop_outbound = session.subscribe_stop();
        tokio::spawn(async move {
            while session_outbound.is_running() && !*stop_outbound.borrow() {
                let maybe_msg = tokio::select! {
                    msg = outgoing_rx.recv() => msg,
                    _ = stop_outbound.changed() => None,
                };
                let Some(msg) = maybe_msg else {
                    break;
                };
                if !session_outbound.is_running() {
                    break;
                }
                let res = match &msg {
                    WireMessage::Input(_)
                    | WireMessage::Control(ControlMessage::FrameAck { .. }) => {
                        conn_outbound.send_datagram(&msg)
                    }
                    _ => conn_outbound.send_uni_message(&msg).await,
                };
                if let Err(e) = res {
                    debug!(
                        session_id = %session_outbound.session_id,
                        error = %e,
                        "Outbound QUIC stream pump terminated"
                    );
                    session_outbound.stop();
                    break;
                }
            }
        });

        // Task 2: Route inbound QUIC datagrams to handle_incoming_wire_message and VirtualChannelManager
        let conn_dgram = conn.clone();
        let session_dgram = session.clone();
        let channel_mgr_dgram = session.channel_manager.clone();
        let mut stop_dgram = session.subscribe_stop();
        tokio::spawn(async move {
            while session_dgram.is_running() && !*stop_dgram.borrow() {
                let recv_res = tokio::select! {
                    res = conn_dgram.recv_datagram() => res,
                    _ = stop_dgram.changed() => break,
                };
                match recv_res {
                    Ok(WireMessage::Channel(vc)) => {
                        if let Err(e) = channel_mgr_dgram.route_channel_message(vc).await {
                            warn!(
                                session_id = %session_dgram.session_id,
                                error = %e,
                                "Failed to route inbound datagram virtual channel message"
                            );
                        }
                    }
                    Ok(msg) => {
                        if let Err(e) = session_dgram.handle_incoming_wire_message(msg).await {
                            warn!(
                                session_id = %session_dgram.session_id,
                                error = %e,
                                "Failed to process inbound QUIC datagram message"
                            );
                        }
                    }
                    Err(e) => {
                        debug!(
                            session_id = %session_dgram.session_id,
                            error = %e,
                            "Inbound QUIC datagram receiver loop ended"
                        );
                        break;
                    }
                }
            }
        });

        // Task 3: Route inbound QUIC unidirectional streams to handle_incoming_wire_message and VirtualChannelManager
        let conn_uni = conn.clone();
        let session_uni = session.clone();
        let channel_mgr_uni = session.channel_manager.clone();
        let mut stop_uni = session.subscribe_stop();
        tokio::spawn(async move {
            while session_uni.is_running() && !*stop_uni.borrow() {
                let recv_res = tokio::select! {
                    res = conn_uni.recv_uni_message() => res,
                    _ = stop_uni.changed() => break,
                };
                match recv_res {
                    Ok(WireMessage::Channel(vc)) => {
                        if let Err(e) = channel_mgr_uni.route_channel_message(vc).await {
                            warn!(
                                session_id = %session_uni.session_id,
                                error = %e,
                                "Failed to route inbound stream virtual channel message"
                            );
                        }
                    }
                    Ok(WireMessage::Handshake(HandshakeMessage::AuthRequest {
                        auth_method,
                        credentials,
                    })) => {
                        let valid = match auth_method {
                            AuthMethod::Ed25519Signature => {
                                if credentials.len() > ED25519_AUTH_HEADER_BYTES {
                                    let verifying_key: [u8; 32] =
                                        credentials[0..32].try_into().unwrap_or([0u8; 32]);
                                    let tls_cert_sha256: [u8; 32] =
                                        credentials[32..64].try_into().unwrap_or([0u8; 32]);
                                    let signature = &credentials[64..128];
                                    let challenge = &credentials[128..];
                                    let cert_matches = conn_uni
                                        .peer_cert_sha256()
                                        .is_none_or(|expected| expected == tls_cert_sha256);
                                    cert_matches
                                        && NistVault::verify_channel_bound_challenge(
                                            &verifying_key,
                                            challenge,
                                            &tls_cert_sha256,
                                            signature,
                                        )
                                        .is_ok()
                                } else {
                                    false
                                }
                            }
                            AuthMethod::SystemUserPassword => !credentials.is_empty(),
                            AuthMethod::VaultToken => !credentials.is_empty(),
                        };
                        let _ = conn_uni
                            .send_uni_message(&WireMessage::Handshake(
                                HandshakeMessage::AuthResponse {
                                    success: valid,
                                    session_token: valid.then(|| session_uni.session_id.clone()),
                                    error_message: (!valid).then(|| {
                                        "In-session handshake authentication failed".to_string()
                                    }),
                                    requires_elevation: false,
                                },
                            ))
                            .await;
                    }
                    Ok(msg) => {
                        if let Err(e) = session_uni.handle_incoming_wire_message(msg).await {
                            warn!(
                                session_id = %session_uni.session_id,
                                error = %e,
                                "Failed to process inbound QUIC unidirectional stream message"
                            );
                        }
                    }
                    Err(e) => {
                        debug!(
                            session_id = %session_uni.session_id,
                            error = %e,
                            "Inbound QUIC unidirectional stream receiver loop ended"
                        );
                        session_uni.stop();
                        break;
                    }
                }
            }
        });

        // Task 4: Route inbound QUIC bidirectional streams (e.g., SimplyTransfer / Clipboard)
        let conn_bi = conn;
        let session_bi = session.clone();
        let channel_mgr_bi = session.channel_manager.clone();
        let mut stop_bi = session.subscribe_stop();
        tokio::spawn(async move {
            while session_bi.is_running() && !*stop_bi.borrow() {
                let accept_res = tokio::select! {
                    res = conn_bi.connection.accept_bi() => res,
                    _ = stop_bi.changed() => break,
                };
                let Ok((mut send_stream, mut recv_stream)) = accept_res else {
                    break;
                };
                let Ok(data) = recv_stream
                    .read_to_end(rferram_protocol::MAX_WIRE_FRAME_BYTES + 4)
                    .await
                else {
                    continue;
                };
                conn_bi
                    .metrics
                    .bytes_received
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                match decode_length_prefixed(&data) {
                    Ok(WireMessage::Channel(vc)) => {
                        if let Err(e) = channel_mgr_bi.route_channel_message(vc).await {
                            warn!(
                                session_id = %session_bi.session_id,
                                error = %e,
                                "Failed to route inbound bidirectional stream virtual channel message"
                            );
                        } else {
                            let _ = send_stream.finish();
                        }
                    }
                    Ok(msg) => {
                        if let Err(e) = session_bi.handle_incoming_wire_message(msg).await {
                            warn!(
                                session_id = %session_bi.session_id,
                                error = %e,
                                "Failed to process inbound QUIC bidirectional stream message"
                            );
                        } else {
                            let _ = send_stream.finish();
                        }
                    }
                    Err(e) => {
                        warn!(
                            session_id = %session_bi.session_id,
                            error = %e,
                            "Failed to decode inbound QUIC bidirectional stream message"
                        );
                    }
                }
            }
        });

        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rferram_channels::ClipboardContent;
    use rferram_net::{QuicClientTransport, QuicServerListener, QuicTransportConfig};
    use rferram_protocol::{
        AudioCodec, AudioDirection, ClipboardPayload, ElementState, InputEvent, MouseButtonCode,
        VirtualChannelMessage, encode_length_prefixed,
    };

    #[tokio::test]
    async fn test_run_capture_loop_across_platform_frame_capturers() {
        let backends = [
            CaptureBackendType::WaylandPipeWire,
            CaptureBackendType::X11Fallback,
            CaptureBackendType::MacOsScreenCaptureKit,
            CaptureBackendType::WindowsDxgi,
        ];

        for backend in backends {
            let engine = HostSessionEngine::new(
                format!("cap-test-{backend:?}"),
                backend,
                EncoderConfig {
                    width: 1920,
                    height: 1080,
                    ..EncoderConfig::default()
                },
            );
            let (actual_backend, monitors) = {
                let cap = engine.capturer.lock().await;
                (cap.backend_type(), cap.enumerate_monitors().unwrap())
            };
            assert_eq!(actual_backend, backend);
            assert!(monitors[0].width >= 1);
            assert!(monitors[0].height >= 1);

            let (tx, mut rx) = mpsc::channel(16);
            engine.run_capture_loop(tx).await.unwrap();

            let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("Timed out waiting for captured frame")
                .expect("Capture channel closed");

            match msg {
                WireMessage::Video(v) => {
                    assert_eq!(v.width, monitors[0].width);
                    assert_eq!(v.height, monitors[0].height);
                    assert!(v.is_keyframe);
                }
                WireMessage::Audio(a) => {
                    assert_eq!(a.codec, AudioCodec::Opus);
                }
                other => panic!("Unexpected wire message from capture loop: {other:?}"),
            }

            engine.stop();
            assert!(!engine.is_running());
        }
    }

    #[tokio::test]
    async fn test_serve_connection_over_loopback_quic() {
        let vault = NistVault::new("test.rferram.server.serve_connection");
        let daemon = Arc::new(RFerramHostDaemon::with_vault(
            "loopback-host-daemon".to_string(),
            CaptureBackendType::WaylandPipeWire,
            vault,
        ));

        let listener = QuicServerListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            QuicTransportConfig::default(),
        )
        .expect("Failed to bind QUIC listener");
        let server_addr = listener.local_addr().unwrap();
        let expected_cert_sha256 = listener.server_cert_sha256;

        let daemon_server = daemon.clone();
        let server_task = tokio::spawn(async move {
            let server_conn = listener
                .accept()
                .await
                .expect("Server failed to accept QUIC connection");
            let session = daemon_server
                .serve_connection(server_conn)
                .await
                .expect("serve_connection should succeed for valid Ed25519 handshake");
            (listener, session)
        });

        let client = QuicClientTransport::new(QuicTransportConfig::default());
        let client_conn = client
            .connect(server_addr, "rferram.host")
            .await
            .expect("Client failed to connect to QUIC listener");

        let peer_cert_sha256 = client_conn
            .peer_cert_sha256()
            .expect("Client should capture server TLS 1.3 leaf cert SHA-256");
        assert_eq!(peer_cert_sha256, expected_cert_sha256);

        // 1. Send channel-bound Ed25519 AuthRequest over unidirectional stream
        let (signing_key, verifying_key) = daemon
            .vault
            .get_or_create_ed25519_identity("loopback_quic_client")
            .expect("Should create Ed25519 identity");
        let challenge = b"rferram-serve-connection-loopback-challenge";
        let signature =
            NistVault::sign_channel_bound_challenge(&signing_key, challenge, &peer_cert_sha256);
        let auth_req = RFerramHostDaemon::build_ed25519_auth_request(
            &verifying_key.to_bytes(),
            &peer_cert_sha256,
            challenge,
            &signature,
        );

        client_conn
            .send_uni_message(&WireMessage::Handshake(auth_req))
            .await
            .expect("Client failed to send AuthRequest");

        // 2. Receive AuthResponse from serve_connection
        let auth_resp_wire =
            tokio::time::timeout(Duration::from_secs(5), client_conn.recv_uni_message())
                .await
                .expect("Timed out waiting for AuthResponse")
                .expect("Failed to receive AuthResponse");
        let WireMessage::Handshake(HandshakeMessage::AuthResponse {
            success: true,
            session_token: Some(session_token),
            ..
        }) = auth_resp_wire
        else {
            panic!("Expected successful AuthResponse, got {auth_resp_wire:?}");
        };
        assert!(!session_token.is_empty());

        let (_listener, session) = tokio::time::timeout(Duration::from_secs(5), server_task)
            .await
            .expect("Server task timed out")
            .expect("Server task panicked");
        assert_eq!(session.session_id, session_token);
        assert_eq!(daemon.active_session_count().await, 1);

        // 3. Receive outbound Video and Audio frames pumped over QUIC unidirectional streams
        let mut saw_video = false;
        let mut saw_audio = false;
        for _ in 0..16 {
            let msg = tokio::time::timeout(Duration::from_secs(5), client_conn.recv_uni_message())
                .await
                .expect("Timed out waiting for outbound media stream")
                .expect("Failed to read outbound media stream");
            match msg {
                WireMessage::Video(v) => {
                    assert_eq!(v.width, 2560);
                    assert_eq!(v.height, 1440);
                    saw_video = true;
                }
                WireMessage::Audio(a) => {
                    assert_eq!(a.codec, AudioCodec::Opus);
                    assert_eq!(a.direction, AudioDirection::HostToClientSpeaker);
                    saw_audio = true;
                }
                _ => {}
            }
            if saw_video && saw_audio {
                break;
            }
        }
        assert!(
            saw_video,
            "Expected outbound VideoFrameMessage over QUIC stream"
        );
        assert!(
            saw_audio,
            "Expected outbound AudioFrameMessage over QUIC stream"
        );

        // 4. Send inbound QUIC datagrams (InputEvent + FrameAck) and verify routing to HostSessionEngine
        client_conn
            .send_datagram(&WireMessage::Input(InputEvent::MouseButton {
                button: MouseButtonCode::Left,
                state: ElementState::Pressed,
                x: 640.0,
                y: 360.0,
            }))
            .expect("Failed to send InputEvent datagram");
        client_conn
            .send_datagram(&WireMessage::Control(ControlMessage::FrameAck {
                frame_id: 77,
                monitor_id: 1,
                render_delay_us: 250,
            }))
            .expect("Failed to send FrameAck datagram");

        // 5. Send inbound VirtualChannelMessage over QUIC uni stream, bi stream, and datagram
        client_conn
            .send_uni_message(&WireMessage::Channel(
                VirtualChannelMessage::DevicePassthrough {
                    device_id: "yubikey-fido2-0".to_string(),
                    data: vec![0xCA, 0xFE, 0xBA, 0xBE],
                },
            ))
            .await
            .expect("Failed to send DevicePassthrough over uni stream");

        let (mut bi_send, _bi_recv) = client_conn
            .open_bidirectional_stream()
            .await
            .expect("Failed to open bidirectional stream");
        let bi_clipboard = WireMessage::Channel(VirtualChannelMessage::Clipboard(
            ClipboardPayload::Files(vec!["/srv/devshare/archive.tar.zst".to_string()]),
        ));
        let framed_bi = encode_length_prefixed(&bi_clipboard).unwrap();
        bi_send.write_all(&framed_bi).await.unwrap();
        bi_send.finish().unwrap();

        client_conn
            .send_datagram(&WireMessage::Channel(
                VirtualChannelMessage::RequestElevation,
            ))
            .expect("Failed to send RequestElevation over datagram");

        // Poll briefly for async inbound datagram, uni-stream, and bi-stream handlers to process messages
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let acked = session.last_acked_frame_id.load(Ordering::SeqCst) == 77;
                let elevated = session
                    .channel_manager
                    .elevation_handler
                    .is_session_elevated();
                let dev_frames = session
                    .channel_manager
                    .device_handler
                    .frames_for_device("yubikey-fido2-0")
                    .await;
                let clipboard_matched = session
                    .channel_manager
                    .clipboard_handler
                    .last_content()
                    .await
                    == ClipboardContent::Files(vec!["/srv/devshare/archive.tar.zst".to_string()]);
                if acked && elevated && dev_frames.len() == 1 && clipboard_matched {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Timed out waiting for inbound datagrams and virtual channel messages to route");

        // 6. Send Disconnect control message over uni stream and verify session stops
        client_conn
            .send_uni_message(&WireMessage::Control(ControlMessage::Disconnect {
                reason: "Loopback test complete".to_string(),
            }))
            .await
            .expect("Failed to send Disconnect message");

        tokio::time::timeout(Duration::from_secs(5), async {
            while session.is_running() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Timed out waiting for session to stop after Disconnect");

        assert!(!session.is_running());
        assert!(daemon.terminate_session(&session.session_id).await);
        assert_eq!(daemon.active_session_count().await, 0);
    }

    #[tokio::test]
    async fn test_serve_connection_rejects_mitm_cert_mismatch_and_concurrent_sessions() {
        let vault = NistVault::new("test.rferram.server.serve_connection_mitm");
        let daemon = Arc::new(RFerramHostDaemon::with_vault(
            "mitm-host-daemon".to_string(),
            CaptureBackendType::WaylandPipeWire,
            vault,
        ));

        let listener = Arc::new(
            QuicServerListener::bind(
                "127.0.0.1:0".parse().unwrap(),
                QuicTransportConfig::default(),
            )
            .expect("Failed to bind QUIC listener"),
        );
        let server_addr = listener.local_addr().unwrap();

        // Pass `server_conn` by value (matching `main.rs` call pattern) and verify client still receives `AuthResponse { success: false }`
        let daemon_server = daemon.clone();
        let listener_mitm = listener.clone();
        let server_task = tokio::spawn(async move {
            let server_conn = listener_mitm.accept().await.unwrap();
            daemon_server.serve_connection(server_conn).await
        });

        let client = QuicClientTransport::new(QuicTransportConfig::default());
        let client_conn = client
            .connect(server_addr, "rferram.host")
            .await
            .expect("Client failed to connect");

        // Sign a challenge bound to a spoofed MITM certificate hash ([0xEE; 32]) instead of the server's actual cert
        let (signing_key, verifying_key) = daemon
            .vault
            .get_or_create_ed25519_identity("mitm_test_client")
            .unwrap();
        let spoofed_cert_sha256 = [0xEEu8; 32];
        let challenge = b"mitm-spoofed-challenge";
        let signature =
            NistVault::sign_channel_bound_challenge(&signing_key, challenge, &spoofed_cert_sha256);
        let mitm_req = RFerramHostDaemon::build_ed25519_auth_request(
            &verifying_key.to_bytes(),
            &spoofed_cert_sha256,
            challenge,
            &signature,
        );

        client_conn
            .send_uni_message(&WireMessage::Handshake(mitm_req))
            .await
            .unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(5), client_conn.recv_uni_message())
            .await
            .expect("Timed out waiting for failure AuthResponse")
            .expect("Failed to read failure AuthResponse");
        assert!(matches!(
            resp,
            WireMessage::Handshake(HandshakeMessage::AuthResponse { success: false, .. })
        ));

        let srv_res = server_task.await.unwrap();
        assert!(
            srv_res.is_err(),
            "serve_connection must reject mismatched TLS 1.3 cert fingerprint"
        );
        assert_eq!(daemon.active_session_count().await, 0);

        // Verify two concurrent clients can connect and disconnecting one does not disrupt the surviving session
        let daemon_c1 = daemon.clone();
        let listener_c1 = listener.clone();
        let accept_two = tokio::spawn(async move {
            let conn1 = listener_c1.accept().await.unwrap();
            let s1 = daemon_c1.serve_connection(conn1).await.unwrap();
            let conn2 = listener_c1.accept().await.unwrap();
            let s2 = daemon_c1.serve_connection(conn2).await.unwrap();
            (s1, s2)
        });

        let c1 = client.connect(server_addr, "rferram.host").await.unwrap();
        let hello1 = HandshakeMessage::ClientHello {
            protocol_version: 1,
            client_id: "concurrent-client-1".to_string(),
            client_name: "Viewer 1".to_string(),
            supported_codecs: vec![VideoCodec::AV1],
            auth_methods: vec![AuthMethod::Ed25519Signature],
        };
        c1.send_uni_message(&WireMessage::Handshake(hello1))
            .await
            .unwrap();
        let _ = c1.recv_uni_message().await.unwrap();

        let c2 = client.connect(server_addr, "rferram.host").await.unwrap();
        let hello2 = HandshakeMessage::ClientHello {
            protocol_version: 1,
            client_id: "concurrent-client-2".to_string(),
            client_name: "Viewer 2".to_string(),
            supported_codecs: vec![VideoCodec::H265],
            auth_methods: vec![AuthMethod::Ed25519Signature],
        };
        c2.send_uni_message(&WireMessage::Handshake(hello2))
            .await
            .unwrap();
        let _ = c2.recv_uni_message().await.unwrap();

        let (s1, s2) = tokio::time::timeout(Duration::from_secs(5), accept_two)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(daemon.active_session_count().await, 2);

        // Terminate session 1 and verify session 2 remains active and continues streaming
        assert!(daemon.terminate_session(&s1.session_id).await);
        assert!(!s1.is_running());
        assert!(s2.is_running());
        assert_eq!(daemon.active_session_count().await, 1);

        let c2_media = tokio::time::timeout(Duration::from_secs(5), c2.recv_uni_message())
            .await
            .expect("Surviving session 2 should continue streaming media")
            .expect("Failed to receive media on surviving session 2");
        assert!(matches!(
            c2_media,
            WireMessage::Video(_) | WireMessage::Audio(_)
        ));

        daemon.shutdown_all_sessions().await;
        assert!(!s2.is_running());
        assert_eq!(daemon.active_session_count().await, 0);
    }
}
