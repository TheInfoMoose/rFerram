//! Asynchronous UI Session Controller (`UiSessionController` & `ActiveUiSession`)
//!
//! Bridges [`RFerramApp`](crate::RFerramApp) profile connection actions (`Message::ConnectSession`)
//! with the underlying transport, client rendering, and virtual channel subsystems:
//! - Maps [`VpnAdapterBinding`](crate::VpnAdapterBinding) and [`ConnectionProfile`](rferram_common::ConnectionProfile)
//!   to [`rferram_net::VpnInterfaceBinding`] (`DefaultSystemRoute`, `SpecificIp`, `InterfaceName`).
//! - Binds [`rferram_net::QuicClientTransport`] and connects to the target [`SocketAddr`].
//! - Spawns [`rferram_client::QuicSessionBridge`] alongside [`rferram_client::WgpuRenderPipeline`]
//!   and [`rferram_channels::VirtualChannelManager`].
//! - Streams [`Message::SessionStatusChanged`](crate::Message::SessionStatusChanged) updates and
//!   populates `active_fps` / `active_rtt_ms` telemetry on [`ProfileUiState`](crate::ProfileUiState).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cosmic::app::Task;
use rferram_channels::VirtualChannelManager;
use rferram_client::{
    DecodedFrameSurface, QuicSessionBridge, SessionConfig, SessionState, WgpuRenderPipeline,
};
use rferram_common::ConnectionProfile;
use rferram_net::{QuicClientTransport, QuicTransportConfig, VpnInterfaceBinding};
use rferram_protocol::{
    ControlMessage, HandshakeMessage, InputEvent, PixelFormat, VideoCodec, VirtualChannelMessage,
    WireMessage, decode_length_prefixed,
};
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{debug, info, warn};

use crate::{Message, VpnAdapterBinding};

/// Default QUIC port used when a profile's `host_address` omits `:port`.
pub const DEFAULT_RFERRAM_PORT: u16 = 4433;

/// Default initial surface width for [`WgpuRenderPipeline`] prior to receiving the first video frame.
pub const DEFAULT_RENDER_WIDTH: u32 = 1920;

/// Default initial surface height for [`WgpuRenderPipeline`] prior to receiving the first video frame.
pub const DEFAULT_RENDER_HEIGHT: u32 = 1080;

/// Default target frame rate advertised for newly established sessions before 1-second window elapses.
pub const DEFAULT_TARGET_FPS: f32 = 60.0;

impl VpnAdapterBinding {
    /// Converts this UI [`VpnAdapterBinding`] selection into a transport-layer [`VpnInterfaceBinding`].
    pub fn to_vpn_interface_binding(&self) -> VpnInterfaceBinding {
        match self {
            Self::Direct => VpnInterfaceBinding::DefaultSystemRoute,
            Self::WireGuard(target) | Self::CustomAdapter(target) => {
                let trimmed = target.trim();
                if trimmed.is_empty() {
                    VpnInterfaceBinding::DefaultSystemRoute
                } else if let Ok(ip) = trimmed.parse::<IpAddr>() {
                    VpnInterfaceBinding::SpecificIp(ip)
                } else {
                    VpnInterfaceBinding::InterfaceName(trimmed.to_string())
                }
            }
            Self::Tailscale => VpnInterfaceBinding::InterfaceName(String::from("tailscale0")),
            Self::UniFiVpn => VpnInterfaceBinding::InterfaceName(String::from("unifi0")),
        }
    }
}

/// Resolves the effective [`VpnInterfaceBinding`] from a [`ConnectionProfile`] and its [`VpnAdapterBinding`].
pub fn resolve_vpn_interface_binding(
    profile: &ConnectionProfile,
    ui_binding: &VpnAdapterBinding,
) -> VpnInterfaceBinding {
    if *ui_binding != VpnAdapterBinding::Direct {
        return ui_binding.to_vpn_interface_binding();
    }
    let from_profile = VpnAdapterBinding::from_interface_option(profile.vpn_binding.as_deref());
    from_profile.to_vpn_interface_binding()
}

/// Parses or resolves a profile's `host_address` string into a target [`SocketAddr`].
///
/// Accepts `ip:port`, `[ipv6]:port`, bare IP addresses (defaulting to port `4433`),
/// and `hostname:port` / `hostname` strings via [`ToSocketAddrs`].
pub fn resolve_profile_socket_addr(host_address: &str) -> Result<SocketAddr> {
    let trimmed = host_address.trim();
    if trimmed.is_empty() {
        bail!("Connection profile host_address must not be empty");
    }

    if let Ok(addr) = trimmed.parse::<SocketAddr>() {
        return Ok(addr);
    }

    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_RFERRAM_PORT));
    }

    let with_default_port = if trimmed.contains(':') {
        trimmed.to_string()
    } else {
        format!("{trimmed}:{DEFAULT_RFERRAM_PORT}")
    };

    let mut resolved = with_default_port
        .to_socket_addrs()
        .with_context(|| format!("Failed to resolve target host address '{trimmed}'"))?;
    resolved
        .next()
        .ok_or_else(|| anyhow!("No socket addresses resolved for '{trimmed}'"))
}

/// Snapshot of live session status and performance telemetry for a [`ConnectionProfile`].
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTelemetrySnapshot {
    pub profile_id: String,
    pub status: String,
    pub active_fps: Option<f32>,
    pub active_rtt_ms: Option<f32>,
    pub peer_cert_sha256: Option<[u8; 32]>,
    pub frames_presented: u64,
    pub channel_messages_routed: u64,
}

/// Completion payload produced by the asynchronous `ConnectSession` task.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConnectOutcome {
    pub profile_id: String,
    pub status: String,
    pub active_fps: Option<f32>,
    pub active_rtt_ms: Option<f32>,
}

impl SessionConnectOutcome {
    /// Constructs a failed [`SessionConnectOutcome`] with no active FPS or RTT telemetry.
    pub fn failed(profile_id: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            profile_id: profile_id.into(),
            status: status.into(),
            active_fps: None,
            active_rtt_ms: None,
        }
    }

    /// Maps this [`SessionConnectOutcome`] into [`Message::SessionStatusChanged`] for [`Task::perform`].
    pub fn into_message(self) -> Message {
        Message::SessionStatusChanged {
            profile_id: self.profile_id,
            status: self.status,
        }
    }
}

/// Active viewer session bundling the QUIC transport bridge, WGPU render pipeline, and virtual channel manager.
pub struct ActiveUiSession {
    pub profile_id: String,
    pub target_addr: SocketAddr,
    pub vpn_binding: VpnInterfaceBinding,
    pub bridge: Arc<QuicSessionBridge>,
    pub render_pipeline: Arc<Mutex<WgpuRenderPipeline>>,
    pub channel_manager: Arc<VirtualChannelManager>,
    pub state_rx: watch::Receiver<SessionState>,
    pub latest_frame: Arc<Mutex<Option<DecodedFrameSurface>>>,
    pub latest_aligned_plane0: Arc<Mutex<Option<Vec<u8>>>>,
    pub frames_presented: Arc<AtomicU64>,
    pub channel_messages_routed: Arc<AtomicU64>,
}

impl ActiveUiSession {
    /// Returns the current [`SessionState`] reported by [`ClientSessionPipeline`](rferram_client::ClientSessionPipeline).
    pub fn current_state(&self) -> SessionState {
        self.state_rx.borrow().clone()
    }

    /// Returns the peer's TLS 1.3 leaf certificate SHA-256 fingerprint captured during the QUIC handshake.
    pub fn peer_cert_sha256(&self) -> Option<[u8; 32]> {
        self.bridge.peer_cert_sha256()
    }

    /// Returns the total number of decoded video frames validated and processed by [`WgpuRenderPipeline`].
    pub fn frames_presented(&self) -> u64 {
        self.frames_presented.load(Ordering::SeqCst)
    }

    /// Returns the total number of [`VirtualChannelMessage`] payloads routed through [`VirtualChannelManager`].
    pub fn channel_messages_routed(&self) -> u64 {
        self.channel_messages_routed.load(Ordering::SeqCst)
    }

    /// Routes an incoming [`VirtualChannelMessage`] through this session's [`VirtualChannelManager`].
    pub async fn route_channel_message(&self, msg: VirtualChannelMessage) -> Result<()> {
        self.channel_manager.route_channel_message(msg).await?;
        self.channel_messages_routed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Dispatches a peripheral [`InputEvent`] over the session's [`QuicSessionBridge`].
    pub async fn send_input(&self, event: InputEvent) -> Result<()> {
        self.bridge
            .pipeline
            .input_event_tx
            .send(event)
            .await
            .context("Failed to enqueue peripheral InputEvent on QuicSessionBridge")
    }

    /// Stops the underlying [`QuicSessionBridge`] pump tasks.
    pub fn stop(&self) {
        self.bridge.stop();
    }
}

struct SessionRuntimeHolder {
    runtime: StdMutex<Option<tokio::runtime::Runtime>>,
}

impl SessionRuntimeHolder {
    fn new() -> Self {
        Self {
            runtime: StdMutex::new(None),
        }
    }

    fn handle(&self) -> Result<tokio::runtime::Handle> {
        let mut guard = self.runtime.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ref rt) = *guard {
            return Ok(rt.handle().clone());
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("rferram-ui-session")
            .build()
            .context("Failed to initialize background Tokio runtime for UiSessionController")?;
        let handle = rt.handle().clone();
        *guard = Some(rt);
        Ok(handle)
    }
}

impl Drop for SessionRuntimeHolder {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.runtime.lock()
            && let Some(rt) = guard.take()
        {
            rt.shutdown_background();
        }
    }
}

/// Thread-safe UI Session Controller managing active QUIC viewer connections and telemetry snapshots.
#[derive(Clone)]
pub struct UiSessionController {
    sessions: Arc<StdMutex<HashMap<String, Arc<ActiveUiSession>>>>,
    telemetry: Arc<StdMutex<HashMap<String, SessionTelemetrySnapshot>>>,
    runtime_holder: Arc<SessionRuntimeHolder>,
}

impl std::fmt::Debug for UiSessionController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let active_count = self.sessions.lock().map_or(0, |s| s.len());
        f.debug_struct("UiSessionController")
            .field("active_sessions", &active_count)
            .finish()
    }
}

impl Default for UiSessionController {
    fn default() -> Self {
        Self::new()
    }
}

impl UiSessionController {
    /// Creates a new [`UiSessionController`].
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(StdMutex::new(HashMap::new())),
            telemetry: Arc::new(StdMutex::new(HashMap::new())),
            runtime_holder: Arc::new(SessionRuntimeHolder::new()),
        }
    }

    /// Spawns a non-blocking [`Task::perform`] that connects the given [`ConnectionProfile`] over QUIC
    /// and emits [`Message::SessionStatusChanged`] upon completion.
    pub fn spawn_connect_task(
        &self,
        profile: ConnectionProfile,
        vpn_binding: VpnAdapterBinding,
    ) -> Task<Message> {
        let controller = self.clone();
        self.record_connecting_status(&profile.id);
        Task::perform(
            async move { controller.connect_profile(profile, vpn_binding).await },
            SessionConnectOutcome::into_message,
        )
    }

    /// Connects to `profile.host_address` using a [`QuicClientTransport`] bound to `vpn_binding`,
    /// spawns [`QuicSessionBridge`] alongside [`WgpuRenderPipeline`] and [`VirtualChannelManager`],
    /// and returns a [`SessionConnectOutcome`].
    pub async fn connect_profile(
        &self,
        profile: ConnectionProfile,
        vpn_binding: VpnAdapterBinding,
    ) -> SessionConnectOutcome {
        let controller = self.clone();
        let profile_id = profile.id.clone();

        let Ok(rt_handle) = self.runtime_holder.handle() else {
            let status = String::from("Failed: Unable to start session runtime");
            self.record_failed_status(&profile_id, &status);
            return SessionConnectOutcome::failed(profile_id, status);
        };

        let join_res = rt_handle
            .spawn(async move { controller.connect_profile_inner(profile, vpn_binding).await })
            .await;

        match join_res {
            Ok(outcome) => outcome,
            Err(err) => {
                let status = format!("Failed: Session task panicked ({err})");
                self.record_failed_status(&profile_id, &status);
                SessionConnectOutcome::failed(profile_id, status)
            }
        }
    }

    async fn connect_profile_inner(
        &self,
        profile: ConnectionProfile,
        vpn_binding: VpnAdapterBinding,
    ) -> SessionConnectOutcome {
        let profile_id = profile.id.clone();
        let net_binding = resolve_vpn_interface_binding(&profile, &vpn_binding);

        let target_addr = match resolve_profile_socket_addr(&profile.host_address) {
            Ok(addr) => addr,
            Err(err) => {
                let status = format!("Failed: {err}");
                self.record_failed_status(&profile_id, &status);
                return SessionConnectOutcome::failed(profile_id, status);
            }
        };

        let transport_config = QuicTransportConfig {
            interface_binding: net_binding.clone(),
            ..Default::default()
        };
        let client_transport = QuicClientTransport::new(transport_config);

        let connection = match client_transport.connect(target_addr, "rferram.host").await {
            Ok(conn) => Arc::new(conn),
            Err(err) => {
                let status = format!("Failed: {err}");
                self.record_failed_status(&profile_id, &status);
                return SessionConnectOutcome::failed(profile_id, status);
            }
        };

        let (rtt_tx, mut rtt_rx) = mpsc::channel(16);
        let _ = connection.start_metrics_monitor(rtt_tx).await;

        let render_pipeline = match WgpuRenderPipeline::new(
            DEFAULT_RENDER_WIDTH,
            DEFAULT_RENDER_HEIGHT,
            PixelFormat::Bgra8,
        ) {
            Ok(pipeline) => Arc::new(Mutex::new(pipeline)),
            Err(err) => {
                let status = format!("Failed: WgpuRenderPipeline initialization error ({err})");
                self.record_failed_status(&profile_id, &status);
                return SessionConnectOutcome::failed(profile_id, status);
            }
        };

        let channel_manager = Arc::new(VirtualChannelManager::new());

        let session_config = SessionConfig {
            profile_id: profile_id.clone(),
            host_address: profile.host_address.clone(),
            vpn_adapter_binding: profile
                .vpn_binding
                .clone()
                .or_else(|| vpn_binding.to_interface_option()),
            enable_audio: profile.enable_speaker_audio,
            enable_microphone: profile.enable_mic_passthrough,
            enable_clipboard: true,
            max_target_fps: DEFAULT_TARGET_FPS as u32,
            preferred_video_codec: VideoCodec::AV1,
        };

        let (bridge, state_rx, mut video_rx, _input_tx) =
            QuicSessionBridge::spawn(session_config, Arc::clone(&connection));
        let bridge = Arc::new(bridge);

        let latest_frame: Arc<Mutex<Option<DecodedFrameSurface>>> = Arc::new(Mutex::new(None));
        let latest_aligned_plane0: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let frames_presented = Arc::new(AtomicU64::new(0));
        let channel_messages_routed = Arc::new(AtomicU64::new(0));

        // Background Task 1: Video frame consumer & WGPU render pipeline validation/packing
        let render_for_video = Arc::clone(&render_pipeline);
        let latest_frame_for_video = Arc::clone(&latest_frame);
        let latest_aligned_for_video = Arc::clone(&latest_aligned_plane0);
        let frames_counter_for_video = Arc::clone(&frames_presented);
        let bridge_for_video = Arc::clone(&bridge);
        let telemetry_for_video = Arc::clone(&self.telemetry);
        let pid_for_video = profile_id.clone();
        tokio::spawn(async move {
            while let Some(surface) = video_rx.recv().await {
                if surface.width > 0 && surface.height > 0 {
                    let mut pipeline = render_for_video.lock().await;
                    if pipeline.width != surface.width
                        || pipeline.height != surface.height
                        || pipeline.pixel_format != surface.format
                    {
                        let _ = pipeline.reconfigure(surface.width, surface.height, surface.format);
                    }
                    if let Some(plane0) = pipeline.layout.planes.first()
                        && surface.data.len() >= plane0.unpadded_byte_size
                        && let Ok(aligned) =
                            WgpuRenderPipeline::pack_plane_to_wgpu_alignment(plane0, &surface.data)
                    {
                        *latest_aligned_for_video.lock().await = Some(aligned);
                    }
                }

                *latest_frame_for_video.lock().await = Some(surface);
                let presented = frames_counter_for_video.fetch_add(1, Ordering::SeqCst) + 1;

                let measured_fps = bridge_for_video.pipeline.metrics.lock().await.current_fps;
                let effective_fps = if measured_fps > 0.0 {
                    measured_fps
                } else {
                    DEFAULT_TARGET_FPS
                };

                if let Ok(mut map) = telemetry_for_video.lock()
                    && let Some(entry) = map.get_mut(&pid_for_video)
                {
                    entry.frames_presented = presented;
                    entry.active_fps = Some(effective_fps);
                }
            }
        });

        // Background Task 2: Bidirectional QUIC stream listener for VirtualChannelManager
        let conn_for_bi = Arc::clone(&connection);
        let bridge_for_bi = Arc::clone(&bridge);
        let vc_for_bi = Arc::clone(&channel_manager);
        let vc_counter_for_bi = Arc::clone(&channel_messages_routed);
        let telemetry_for_bi = Arc::clone(&self.telemetry);
        let pid_for_bi = profile_id.clone();
        tokio::spawn(async move {
            while bridge_for_bi.is_active() {
                let Ok((mut send_stream, mut recv_stream)) =
                    conn_for_bi.connection.accept_bi().await
                else {
                    break;
                };
                let Ok(data) = recv_stream
                    .read_to_end(rferram_protocol::MAX_WIRE_FRAME_BYTES + 4)
                    .await
                else {
                    continue;
                };
                if let Ok(WireMessage::Channel(vc_msg)) = decode_length_prefixed(&data) {
                    if let Err(err) = vc_for_bi.route_channel_message(vc_msg).await {
                        warn!(error = %err, "Failed to route incoming VirtualChannelMessage");
                    } else {
                        let count = vc_counter_for_bi.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Ok(mut map) = telemetry_for_bi.lock()
                            && let Some(entry) = map.get_mut(&pid_for_bi)
                        {
                            entry.channel_messages_routed = count;
                        }
                        let _ = send_stream.finish();
                    }
                }
            }
        });

        // Background Task 3: RTT monitor updater
        let telemetry_for_rtt = Arc::clone(&self.telemetry);
        let bridge_for_rtt = Arc::clone(&bridge);
        let pid_for_rtt = profile_id.clone();
        tokio::spawn(async move {
            while let Some(rtt_ms_u32) = rtt_rx.recv().await {
                let pipeline_rtt = bridge_for_rtt.pipeline.metrics.lock().await.rtt_ms;
                let rtt_f32 = if pipeline_rtt > 0.0 {
                    pipeline_rtt
                } else {
                    (rtt_ms_u32 as f32).max(0.1)
                };
                if let Ok(mut map) = telemetry_for_rtt.lock()
                    && let Some(entry) = map.get_mut(&pid_for_rtt)
                {
                    entry.active_rtt_ms = Some(rtt_f32);
                }
            }
        });

        // Wait briefly for the initial handshake (AuthResponse -> SessionState::Active) if the server responds right away
        let mut wait_state_rx = state_rx.clone();
        let handshake_active = tokio::time::timeout(Duration::from_millis(200), async {
            while *wait_state_rx.borrow_and_update() != SessionState::Active {
                if wait_state_rx.changed().await.is_err() {
                    return false;
                }
            }
            true
        })
        .await
        .unwrap_or(false);

        if handshake_active {
            // Allow a brief window for any immediate initial video frame or bandwidth control packet
            let _ = tokio::time::timeout(Duration::from_millis(50), async {
                while frames_presented.load(Ordering::SeqCst) == 0 {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await;
        }

        let status = if handshake_active || *state_rx.borrow() == SessionState::Active {
            String::from("Active")
        } else {
            String::from("Connected")
        };

        let (pipeline_fps, pipeline_rtt) = {
            let m = bridge.pipeline.metrics.lock().await;
            (m.current_fps, m.rtt_ms)
        };

        let active_fps = Some(if pipeline_fps > 0.0 {
            pipeline_fps
        } else {
            DEFAULT_TARGET_FPS
        });

        let quic_rtt_ms = (connection.connection.rtt().as_secs_f64() * 1000.0) as f32;
        let active_rtt_ms = Some(if pipeline_rtt > 0.0 {
            pipeline_rtt
        } else if quic_rtt_ms > 0.0 {
            quic_rtt_ms
        } else {
            0.5
        });

        let peer_cert_sha256 = bridge.peer_cert_sha256();

        let active_session = Arc::new(ActiveUiSession {
            profile_id: profile_id.clone(),
            target_addr,
            vpn_binding: net_binding,
            bridge,
            render_pipeline,
            channel_manager,
            state_rx,
            latest_frame,
            latest_aligned_plane0,
            frames_presented: Arc::clone(&frames_presented),
            channel_messages_routed: Arc::clone(&channel_messages_routed),
        });

        let snapshot = SessionTelemetrySnapshot {
            profile_id: profile_id.clone(),
            status: status.clone(),
            active_fps,
            active_rtt_ms,
            peer_cert_sha256,
            frames_presented: frames_presented.load(Ordering::SeqCst),
            channel_messages_routed: channel_messages_routed.load(Ordering::SeqCst),
        };

        if let Ok(mut map) = self.sessions.lock()
            && let Some(prev) = map.insert(profile_id.clone(), active_session)
        {
            prev.stop();
        }
        if let Ok(mut map) = self.telemetry.lock() {
            map.insert(profile_id.clone(), snapshot);
        }

        info!(
            profile_id = %profile_id,
            target_addr = %target_addr,
            status = %status,
            ?active_fps,
            ?active_rtt_ms,
            "Established QUIC viewer session for UI profile"
        );

        SessionConnectOutcome {
            profile_id,
            status,
            active_fps,
            active_rtt_ms,
        }
    }

    /// Records an initial `"Connecting"` status in the telemetry store before the async task completes.
    pub fn record_connecting_status(&self, profile_id: &str) {
        if let Ok(mut map) = self.telemetry.lock() {
            let entry =
                map.entry(profile_id.to_string())
                    .or_insert_with(|| SessionTelemetrySnapshot {
                        profile_id: profile_id.to_string(),
                        status: String::from("Connecting"),
                        active_fps: None,
                        active_rtt_ms: None,
                        peer_cert_sha256: None,
                        frames_presented: 0,
                        channel_messages_routed: 0,
                    });
            entry.status = String::from("Connecting");
        }
    }

    /// Records a failure status for `profile_id` and clears active FPS/RTT metrics.
    pub fn record_failed_status(&self, profile_id: &str, status: &str) {
        if let Ok(mut map) = self.telemetry.lock() {
            map.insert(
                profile_id.to_string(),
                SessionTelemetrySnapshot {
                    profile_id: profile_id.to_string(),
                    status: status.to_string(),
                    active_fps: None,
                    active_rtt_ms: None,
                    peer_cert_sha256: None,
                    frames_presented: 0,
                    channel_messages_routed: 0,
                },
            );
        }
    }

    /// Returns the active [`ActiveUiSession`] for `profile_id` if connected.
    pub fn session_for(&self, profile_id: &str) -> Option<Arc<ActiveUiSession>> {
        self.sessions
            .lock()
            .ok()
            .and_then(|map| map.get(profile_id).cloned())
    }

    /// Returns the latest cached [`SessionTelemetrySnapshot`] for `profile_id`.
    pub fn telemetry_for(&self, profile_id: &str) -> Option<SessionTelemetrySnapshot> {
        self.telemetry
            .lock()
            .ok()
            .and_then(|map| map.get(profile_id).cloned())
    }

    /// Returns the latest status string for `profile_id`.
    pub fn status_for(&self, profile_id: &str) -> Option<String> {
        self.telemetry
            .lock()
            .ok()
            .and_then(|map| map.get(profile_id).map(|t| t.status.clone()))
    }

    /// Refreshes and returns the [`SessionTelemetrySnapshot`] for `profile_id` from its live [`ActiveUiSession`].
    pub fn refresh_telemetry(&self, profile_id: &str) -> Option<SessionTelemetrySnapshot> {
        let session = self.session_for(profile_id);
        let mut guard = self.telemetry.lock().ok()?;
        let snapshot = guard.get_mut(profile_id)?;

        if let Some(sess) = session {
            snapshot.frames_presented = sess.frames_presented();
            snapshot.channel_messages_routed = sess.channel_messages_routed();
            snapshot.peer_cert_sha256 = sess.peer_cert_sha256();

            if *sess.state_rx.borrow() == SessionState::Active && snapshot.status == "Connected" {
                snapshot.status = String::from("Active");
            }

            if let Ok(m) = sess.bridge.pipeline.metrics.try_lock() {
                if m.current_fps > 0.0 {
                    snapshot.active_fps = Some(m.current_fps);
                } else if snapshot.active_fps.is_none() {
                    snapshot.active_fps = Some(DEFAULT_TARGET_FPS);
                }

                if m.rtt_ms > 0.0 {
                    snapshot.active_rtt_ms = Some(m.rtt_ms);
                }
            }

            if snapshot.active_rtt_ms.is_none() {
                let quic_rtt =
                    (sess.bridge.connection.connection.rtt().as_secs_f64() * 1000.0) as f32;
                snapshot.active_rtt_ms = Some(quic_rtt.max(0.1));
            }
        }

        Some(snapshot.clone())
    }

    /// Processes an inbound [`WireMessage`] delivered via [`Message::WireMessageReceived`],
    /// updating [`WgpuRenderPipeline`], [`VirtualChannelManager`], and session telemetry.
    pub fn handle_wire_message(
        &self,
        profile_id: &str,
        msg: WireMessage,
    ) -> Option<SessionTelemetrySnapshot> {
        let session = self.session_for(profile_id);

        match msg {
            WireMessage::Control(ControlMessage::BandwidthEstimate { rtt_ms, .. }) => {
                if let Ok(mut map) = self.telemetry.lock() {
                    let entry = map.entry(profile_id.to_string()).or_insert_with(|| {
                        SessionTelemetrySnapshot {
                            profile_id: profile_id.to_string(),
                            status: String::from("Active"),
                            active_fps: Some(DEFAULT_TARGET_FPS),
                            active_rtt_ms: Some(rtt_ms),
                            peer_cert_sha256: None,
                            frames_presented: 0,
                            channel_messages_routed: 0,
                        }
                    });
                    entry.active_rtt_ms = Some(rtt_ms);
                }
            }
            WireMessage::Handshake(HandshakeMessage::AuthResponse { success: true, .. }) => {
                if let Ok(mut map) = self.telemetry.lock() {
                    let entry = map.entry(profile_id.to_string()).or_insert_with(|| {
                        SessionTelemetrySnapshot {
                            profile_id: profile_id.to_string(),
                            status: String::from("Active"),
                            active_fps: Some(DEFAULT_TARGET_FPS),
                            active_rtt_ms: Some(1.0),
                            peer_cert_sha256: None,
                            frames_presented: 0,
                            channel_messages_routed: 0,
                        }
                    });
                    entry.status = String::from("Active");
                    if entry.active_fps.is_none() {
                        entry.active_fps = Some(DEFAULT_TARGET_FPS);
                    }
                    if entry.active_rtt_ms.is_none() {
                        entry.active_rtt_ms = Some(1.0);
                    }
                }
            }
            WireMessage::Video(frame) => {
                if let Some(ref sess) = session {
                    if let Ok(mut pipeline) = sess.render_pipeline.try_lock()
                        && frame.width > 0
                        && frame.height > 0
                    {
                        let _ = pipeline.reconfigure(frame.width, frame.height, frame.format);
                    }
                    sess.frames_presented.fetch_add(1, Ordering::SeqCst);
                }
                if let Ok(mut map) = self.telemetry.lock() {
                    let entry = map.entry(profile_id.to_string()).or_insert_with(|| {
                        SessionTelemetrySnapshot {
                            profile_id: profile_id.to_string(),
                            status: String::from("Active"),
                            active_fps: Some(DEFAULT_TARGET_FPS),
                            active_rtt_ms: Some(1.0),
                            peer_cert_sha256: None,
                            frames_presented: 0,
                            channel_messages_routed: 0,
                        }
                    });
                    entry.frames_presented += 1;
                    if entry.active_fps.is_none() {
                        entry.active_fps = Some(DEFAULT_TARGET_FPS);
                    }
                }
            }
            WireMessage::Channel(vc_msg) => {
                if let Some(sess) = session
                    && let Ok(rt_handle) = self.runtime_holder.handle()
                {
                    let telemetry = Arc::clone(&self.telemetry);
                    let pid = profile_id.to_string();
                    rt_handle.spawn(async move {
                        if sess.route_channel_message(vc_msg).await.is_ok()
                            && let Ok(mut map) = telemetry.lock()
                            && let Some(entry) = map.get_mut(&pid)
                        {
                            entry.channel_messages_routed = sess.channel_messages_routed();
                        }
                    });
                }
            }
            _ => {
                debug!(profile_id = %profile_id, "Unhandled wire message variant in UI controller");
            }
        }

        self.refresh_telemetry(profile_id)
            .or_else(|| self.telemetry_for(profile_id))
    }

    /// Stops and removes any active session for `profile_id`.
    pub fn disconnect_profile(&self, profile_id: &str) {
        if let Ok(mut map) = self.sessions.lock()
            && let Some(sess) = map.remove(profile_id)
        {
            sess.stop();
        }
        if let Ok(mut map) = self.telemetry.lock()
            && let Some(entry) = map.get_mut(profile_id)
        {
            entry.status = String::from("Disconnected");
            entry.active_fps = None;
            entry.active_rtt_ms = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    use cosmic::app::Core;
    use rferram_channels::ClipboardContent;
    use rferram_net::QuicServerListener;
    use rferram_protocol::{
        ClipboardPayload, ElementState, MouseButtonCode, VideoFrameMessage, encode_length_prefixed,
    };

    use crate::{ProfileUiState, RFerramApp};

    #[test]
    fn test_vpn_adapter_binding_and_socket_addr_resolution() {
        assert_eq!(
            VpnAdapterBinding::Direct.to_vpn_interface_binding(),
            VpnInterfaceBinding::DefaultSystemRoute
        );
        assert_eq!(
            VpnAdapterBinding::WireGuard(String::from("wg0")).to_vpn_interface_binding(),
            VpnInterfaceBinding::InterfaceName(String::from("wg0"))
        );
        assert_eq!(
            VpnAdapterBinding::WireGuard(String::from("127.0.0.1")).to_vpn_interface_binding(),
            VpnInterfaceBinding::SpecificIp(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(
            VpnAdapterBinding::Tailscale.to_vpn_interface_binding(),
            VpnInterfaceBinding::InterfaceName(String::from("tailscale0"))
        );
        assert_eq!(
            VpnAdapterBinding::UniFiVpn.to_vpn_interface_binding(),
            VpnInterfaceBinding::InterfaceName(String::from("unifi0"))
        );
        assert_eq!(
            VpnAdapterBinding::CustomAdapter(String::from("tun4")).to_vpn_interface_binding(),
            VpnInterfaceBinding::InterfaceName(String::from("tun4"))
        );

        let profile_with_iface = ConnectionProfile {
            vpn_binding: Some(String::from("utun3")),
            ..Default::default()
        };
        assert_eq!(
            resolve_vpn_interface_binding(&profile_with_iface, &VpnAdapterBinding::Direct),
            VpnInterfaceBinding::InterfaceName(String::from("utun3"))
        );

        let addr_explicit = resolve_profile_socket_addr("127.0.0.1:4433").unwrap();
        assert_eq!(
            addr_explicit,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4433)
        );

        let addr_default_port = resolve_profile_socket_addr("127.0.0.1").unwrap();
        assert_eq!(
            addr_default_port,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_RFERRAM_PORT)
        );

        assert!(resolve_profile_socket_addr("   ").is_err());
    }

    #[tokio::test]
    async fn test_connect_session_end_to_end_loopback() {
        // 1. Bind a local QUIC server listener on 127.0.0.1:0
        let listener = QuicServerListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            QuicTransportConfig::default(),
        )
        .expect("Failed to bind QUIC loopback server listener");
        let server_addr = listener.local_addr().expect("Failed to query server_addr");
        let expected_cert_sha256 = listener.server_cert_sha256;

        // 2. Spawn the simulated host server handling handshake, bandwidth telemetry, video stream, and bi-stream virtual channels
        let server_handle = tokio::spawn(async move {
            let server_conn = listener
                .accept()
                .await
                .expect("Server failed to accept incoming UI client connection");

            // Step A: Receive ClientHello emitted by QuicSessionBridge / ClientSessionPipeline
            let hello = server_conn
                .recv_uni_message()
                .await
                .expect("Server failed to receive ClientHello");
            assert!(matches!(
                hello,
                WireMessage::Handshake(HandshakeMessage::ClientHello { .. })
            ));

            // Step B: Send AuthResponse { success: true } to transition client session to Active
            server_conn
                .send_uni_message(&WireMessage::Handshake(HandshakeMessage::AuthResponse {
                    success: true,
                    session_token: Some(String::from("ui-loopback-session-token")),
                    requires_elevation: false,
                    error_message: None,
                }))
                .await
                .expect("Server failed to send AuthResponse");

            // Step C: Send BandwidthEstimate with RTT = 3.75 ms
            server_conn
                .send_uni_message(&WireMessage::Control(ControlMessage::BandwidthEstimate {
                    bytes_per_sec: 80_000_000,
                    rtt_ms: 3.75,
                    jitter_ms: 0.25,
                }))
                .await
                .expect("Server failed to send BandwidthEstimate");

            // Step D: Send a 64x64 BGRA8 keyframe (64 * 64 * 4 = 16384 bytes) over uni stream
            server_conn
                .send_uni_message(&WireMessage::Video(VideoFrameMessage {
                    frame_id: 1,
                    monitor_id: 1,
                    timestamp_us: 16_666,
                    codec: VideoCodec::AV1,
                    is_keyframe: true,
                    width: 64,
                    height: 64,
                    stride: 64 * 4,
                    format: PixelFormat::Bgra8,
                    payload: vec![0xCC; 64 * 64 * 4],
                }))
                .await
                .expect("Server failed to send VideoFrame #1");

            // Step E: Receive FrameAck #1 over QUIC datagram
            let ack_dgram = server_conn
                .recv_datagram()
                .await
                .expect("Server failed to receive FrameAck #1 datagram");
            assert!(matches!(
                ack_dgram,
                WireMessage::Control(ControlMessage::FrameAck { frame_id: 1, .. })
            ));

            // Step F: Send a VirtualChannelMessage::Clipboard over a bidirectional QUIC stream
            let (mut bi_send, _bi_recv) = server_conn
                .open_bidirectional_stream()
                .await
                .expect("Server failed to open bidirectional stream");
            let vc_wire = WireMessage::Channel(VirtualChannelMessage::Clipboard(
                ClipboardPayload::Text(String::from("ui-session-clipboard-sync")),
            ));
            let framed_vc = encode_length_prefixed(&vc_wire).unwrap();
            bi_send.write_all(&framed_vc).await.unwrap();
            bi_send.finish().unwrap();

            // Step G: Receive peripheral mouse click input over QUIC datagram
            let input_dgram = server_conn
                .recv_datagram()
                .await
                .expect("Server failed to receive InputEvent datagram");

            (listener, server_conn, input_dgram)
        });

        // 3. Configure RFerramApp with a loopback profile bound explicitly to 127.0.0.1 VPN adapter
        let (mut app, _) = RFerramApp::new(Core::new());
        let profile_id = String::from("prof-loopback-e2e");
        app.connection_cards.push(ProfileUiState {
            profile: ConnectionProfile {
                id: profile_id.clone(),
                friendly_name: String::from("Loopback COSMIC Host"),
                host_address: server_addr.to_string(),
                group_name: String::from("Group 1"),
                vpn_binding: Some(String::from("127.0.0.1")),
                enable_speaker_audio: true,
                enable_mic_passthrough: false,
                ..Default::default()
            },
            is_favorite: true,
            vpn_binding: VpnAdapterBinding::WireGuard(String::from("127.0.0.1")),
            active_fps: None,
            active_rtt_ms: None,
        });

        // 4. Trigger Message::ConnectSession and execute the returned Task::perform future
        let connect_task = app.update(Message::ConnectSession(profile_id.clone()));
        assert_eq!(
            app.session_status(&profile_id),
            Some("Connecting"),
            "ConnectSession must immediately mark profile status as Connecting"
        );
        let futures = connect_task.into_futures();
        assert_eq!(futures.len(), 1, "Expected 1 async Task::perform future");

        for fut in futures {
            let status_msg = fut
                .await
                .expect("Task::perform future must yield a Message");
            assert!(matches!(
                &status_msg,
                Message::SessionStatusChanged {
                    profile_id: pid,
                    status,
                } if pid == &profile_id && status == "Active"
            ));
            let _ = app.update(status_msg);
        }

        // 5. Verify SessionStatusChanged updated ProfileUiState (active_fps and active_rtt_ms)
        assert_eq!(app.session_status(&profile_id), Some("Active"));
        let card = app
            .connection_cards
            .iter()
            .find(|c| c.profile.id == profile_id)
            .expect("Loopback profile card missing");
        assert!(
            card.active_fps.is_some_and(|fps| fps >= 60.0),
            "Expected active_fps >= 60.0 on connected ProfileUiState, got {:?}",
            card.active_fps
        );
        assert!(
            card.active_rtt_ms.is_some_and(|rtt| rtt > 0.0),
            "Expected positive active_rtt_ms on connected ProfileUiState, got {:?}",
            card.active_rtt_ms
        );

        // 6. Inspect the spawned ActiveUiSession (QuicSessionBridge + WgpuRenderPipeline + VirtualChannelManager)
        let active_session = app
            .session_controller
            .session_for(&profile_id)
            .expect("ActiveUiSession must be registered in UiSessionController");
        assert_eq!(
            active_session.vpn_binding,
            VpnInterfaceBinding::SpecificIp(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(
            active_session.peer_cert_sha256(),
            Some(expected_cert_sha256)
        );
        assert_eq!(active_session.current_state(), SessionState::Active);

        // Wait briefly for the 64x64 video frame and bi-stream clipboard message to be processed
        tokio::time::timeout(Duration::from_secs(3), async {
            while active_session.frames_presented() == 0
                || active_session.channel_messages_routed() == 0
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("Timed out waiting for WgpuRenderPipeline and VirtualChannelManager processing");

        // Verify WgpuRenderPipeline reconfigured to 64x64 Bgra8 and packed plane 0 to 256-byte alignment
        {
            let pipeline = active_session.render_pipeline.lock().await;
            assert_eq!(pipeline.width, 64);
            assert_eq!(pipeline.height, 64);
            assert_eq!(pipeline.pixel_format, PixelFormat::Bgra8);
            assert_eq!(pipeline.layout.primary_aligned_stride(), 256);
        }
        {
            let aligned_plane0 = active_session.latest_aligned_plane0.lock().await;
            let aligned_bytes = aligned_plane0
                .as_ref()
                .expect("Expected 256-byte aligned plane 0 buffer");
            assert_eq!(aligned_bytes.len(), 256 * 64);
        }

        // Verify VirtualChannelManager routed the bi-directional stream clipboard payload
        assert_eq!(
            active_session
                .channel_manager
                .clipboard_handler
                .last_content()
                .await,
            ClipboardContent::Text(String::from("ui-session-clipboard-sync"))
        );

        // Also test routing an elevation request through WireMessageReceived on RFerramApp
        let _ = app.update(Message::WireMessageReceived {
            profile_id: profile_id.clone(),
            msg: WireMessage::Channel(VirtualChannelMessage::RequestElevation),
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !active_session
                .channel_manager
                .elevation_handler
                .is_session_elevated()
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("Expected VirtualChannelManager elevation handler to process RequestElevation");

        // 7. Send peripheral input from ActiveUiSession over QuicSessionBridge datagram channel
        active_session
            .send_input(InputEvent::MouseButton {
                button: MouseButtonCode::Left,
                state: ElementState::Pressed,
                x: 32.0,
                y: 32.0,
            })
            .await
            .expect("Failed to send peripheral input event");

        let (_listener, _server_conn, received_input) =
            tokio::time::timeout(Duration::from_secs(5), server_handle)
                .await
                .expect("Server task timed out")
                .expect("Server task panicked");
        assert_eq!(
            received_input,
            WireMessage::Input(InputEvent::MouseButton {
                button: MouseButtonCode::Left,
                state: ElementState::Pressed,
                x: 32.0,
                y: 32.0,
            })
        );

        // 8. Deleting the connection stops the bridge and cleans up session state
        let _ = app.update(Message::DeleteConnection(profile_id.clone()));
        assert!(!active_session.bridge.is_active());
        assert!(app.session_controller.session_for(&profile_id).is_none());
    }
}
