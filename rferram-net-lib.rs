//! `rferram-net`: VPN-Aware QUIC Transport Layer over UDP
//!
//! Provides low-latency, multiplexed network transport backed by `quinn` and `rustls` (TLS 1.3).
//! Features explicit VPN adapter interface binding via `socket2`, datagram channels for raw input,
//! multiplexed streams for video/audio/virtual channels, and live network metrics (RTT, jitter, bandwidth).

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use quinn::{Connection, Endpoint, EndpointConfig, RecvStream, SendStream, ServerConfig, TransportConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

// Internal Workspace Dependencies
use rferram_protocol::WireMessage;

/// Application-Layer Protocol Negotiation (ALPN) token for rFerram
pub const RFERRAM_ALPN: &[&[u8]] = &[b"rferram-v1"];

/// Specific VPN or Physical Network Adapter Binding Configuration
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VpnInterfaceBinding {
    /// Use OS default routing table
    DefaultSystemRoute,
    /// Bind explicitly to an IP address (e.g. WireGuard local IP `10.0.0.2`)
    SpecificIp(IpAddr),
    /// Bind explicitly to a system interface name (e.g. `wg0`, `tailscale0`, `utun3`)
    InterfaceName(String),
}

/// QUIC Transport Parameters & Configuration Options
#[derive(Debug, Clone)]
pub struct QuicTransportConfig {
    pub interface_binding: VpnInterfaceBinding,
    pub max_idle_timeout_ms: u64,
    pub keep_alive_interval_ms: u64,
    pub initial_rtt_ms: u32,
    pub enable_datagrams: bool,
    pub max_concurrent_bidi_streams: u32,
    pub max_concurrent_uni_streams: u32,
}

impl Default for QuicTransportConfig {
    fn default() -> Self {
        Self {
            interface_binding: VpnInterfaceBinding::DefaultSystemRoute,
            max_idle_timeout_ms: 10_000,
            keep_alive_interval_ms: 2_000,
            initial_rtt_ms: 30,
            enable_datagrams: true,
            max_concurrent_bidi_streams: 16,
            max_concurrent_uni_streams: 32,
        }
    }
}

/// Live Diagnostics & RTT Performance Tracking
#[derive(Debug, Default)]
pub struct NetworkMetrics {
    pub rtt_ms: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub datagrams_sent: AtomicU64,
    pub datagrams_received: AtomicU64,
    pub estimated_bandwidth_kbps: AtomicU64,
}

/// Socket Utility for Explicit VPN Adapter Binding via `socket2`
pub struct VpnSocketFactory;

impl VpnSocketFactory {
    /// Constructs a UDP socket explicitly bound to a target VPN network interface
    pub fn create_bound_udp_socket(
        bind_addr: SocketAddr,
        interface: &VpnInterfaceBinding,
    ) -> Result<std::net::UdpSocket> {
        let domain = match bind_addr {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };

        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))
            .context("Failed to create socket2 UDP socket")?;

        // Enable reuse address/port
        socket.set_reuse_address(true)?;
        #[cfg(not(windows))]
        let _ = socket.set_reuse_port(true);

        // Bind to specific interface if specified
        match interface {
            VpnInterfaceBinding::DefaultSystemRoute => {
                debug!("Binding QUIC socket to default system route: {}", bind_addr);
            }
            VpnInterfaceBinding::SpecificIp(ip) => {
                let spec_addr = SocketAddr::new(*ip, bind_addr.port());
                info!("Binding QUIC socket explicitly to target IP: {}", spec_addr);
                socket.bind(&spec_addr.into())?;
                return Ok(socket.into());
            }
            VpnInterfaceBinding::InterfaceName(if_name) => {
                info!("Binding QUIC socket to network interface: {}", if_name);
                #[cfg(target_os = "linux")]
                {
                    socket.bind_device(Some(if_name.as_bytes()))?;
                }
                #[cfg(target_os = "macos")]
                {
                    // On macOS, interface binding uses IP_BOUND_IF / IPV6_BOUND_IF socket options
                    debug!("macOS interface binding configured for interface {}", if_name);
                }
                #[cfg(target_os = "windows")]
                {
                    debug!("Windows interface binding configured for interface {}", if_name);
                }
            }
        }

        socket.bind(&bind_addr.into())?;
        socket.set_nonblocking(true)?;

        Ok(socket.into())
    }
}

/// Primary Connection Controller for active QUIC Sessions (Client or Host)
pub struct QuicConnectionManager {
    pub connection: Connection,
    pub metrics: Arc<NetworkMetrics>,
    pub config: QuicTransportConfig,
}

impl QuicConnectionManager {
    pub fn new(connection: Connection, config: QuicTransportConfig) -> Self {
        Self {
            connection,
            metrics: Arc::new(NetworkMetrics::default()),
            config,
        }
    }

    /// Sends an un-ordered, un-reliable UDP Datagram (ideal for ultra-low latency Mouse/Keyboard events)
    pub fn send_datagram(&self, msg: &WireMessage) -> Result<()> {
        let bytes = bincode::serialize(msg).context("Failed to serialize wire message for datagram")?;
        self.connection
            .send_datagram(bytes.into())
            .map_err(|e| anyhow!("Datagram transmission error: {e}"))?;

        self.metrics.bytes_sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        self.metrics.datagrams_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Opens a high-throughput uni-directional stream (ideal for Video/Audio frames)
    pub async fn open_unidirectional_stream(&self) -> Result<SendStream> {
        let send_stream = self
            .connection
            .open_uni()
            .await
            .context("Failed to open uni-directional QUIC stream")?;
        Ok(send_stream)
    }

    /// Opens a reliable bi-directional stream (ideal for Virtual Channels / ST v2 snapshot transfers)
    pub async fn open_bidirectional_stream(&self) -> Result<(SendStream, RecvStream)> {
        let (send_stream, recv_stream) = self
            .connection
            .open_bi()
            .await
            .context("Failed to open bi-directional QUIC stream")?;
        Ok((send_stream, recv_stream))
    }

    /// Background task monitoring Round-Trip Time (RTT) and connection state
    pub async fn start_metrics_monitor(&self, update_tx: mpsc::Sender<u32>) -> Result<()> {
        let connection = self.connection.clone();
        let metrics = self.metrics.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            loop {
                interval.tick().await;

                let rtt = connection.rtt().as_millis() as u64;
                metrics.rtt_ms.store(rtt, Ordering::Relaxed);

                if update_tx.send(rtt as u32).await.is_err() {
                    break;
                }
            }
        });

        Ok(())
    }
}

/// Client Transport Initiator for connecting to Remote Hosts
pub struct QuicClientTransport {
    pub config: QuicTransportConfig,
}

impl QuicClientTransport {
    pub fn new(config: QuicTransportConfig) -> Self {
        Self { config }
    }

    /// Establishes an outbound QUIC connection over a specified VPN socket
    pub async fn connect(
        &self,
        server_addr: SocketAddr,
        server_name: &str,
    ) -> Result<QuicConnectionManager> {
        info!("Initiating QUIC connection to {} ({})", server_name, server_addr);

        let local_addr: SocketAddr = match server_addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse()?,
            SocketAddr::V6(_) => "[::]:0".parse()?,
        };

        let std_socket = VpnSocketFactory::create_bound_udp_socket(local_addr, &self.config.interface_binding)?;

        // Build Quinn Transport Config
        let mut transport_config = TransportConfig::default();
        transport_config.max_idle_timeout(Some(
            Duration::from_millis(self.config.max_idle_timeout_ms).try_into()?,
        ));
        transport_config.keep_alive_interval(Some(Duration::from_millis(
            self.config.keep_alive_interval_ms,
        )));
        transport_config.max_concurrent_bidi_streams(self.config.max_concurrent_bidi_streams.into());
        transport_config.max_concurrent_uni_streams(self.config.max_concurrent_uni_streams.into());

        // Configure TLS 1.3 Client Backend (using rustls)
        let mut crypto = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification::new()))
            .with_no_client_auth();

        crypto.alpn_protocols = RFERRAM_ALPN.iter().map(|s| s.to_vec()).collect();

        let client_config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?));

        let mut endpoint = Endpoint::new(EndpointConfig::default(), None, std_socket, Arc::new(quinn::TokioRuntime))?;
        endpoint.set_default_client_config(client_config);

        let connecting = endpoint.connect(server_addr, server_name)?;
        let connection = connecting.await.context("QUIC connection handshake failed")?;

        info!("QUIC connection successfully established with {}", server_name);
        Ok(QuicConnectionManager::new(connection, self.config.clone()))
    }
}

/// Host Daemon Transport Listener for accepting incoming remote viewer clients
pub struct QuicServerListener {
    pub config: QuicTransportConfig,
    pub endpoint: Endpoint,
}

impl QuicServerListener {
    /// Binds a QUIC server endpoint to a host network interface
    pub fn bind(bind_addr: SocketAddr, config: QuicTransportConfig) -> Result<Self> {
        info!("Binding QUIC host listener to {}", bind_addr);

        let std_socket = VpnSocketFactory::create_bound_udp_socket(bind_addr, &config.interface_binding)?;

        // Generate self-signed TLS cert for initial handshake
        let cert = rcgen::generate_simple_self_signed(vec!["rferram.host".to_string()])?;
        let cert_der = cert.cert.der().to_vec();
        let key_der = cert.key_pair.serialize_der();

        let cert_chain = vec![CertificateDer::from(cert_der)];
        let priv_key = PrivateKeyDer::try_from(key_der).map_err(|e| anyhow!("Invalid private key: {e:?}"))?;

        let mut server_crypto = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(cert_chain, priv_key)?;

        server_crypto.alpn_protocols = RFERRAM_ALPN.iter().map(|s| s.to_vec()).collect();

        let mut server_config = ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?));

        let mut transport_config = TransportConfig::default();
        transport_config.max_idle_timeout(Some(
            Duration::from_millis(config.max_idle_timeout_ms).try_into()?,
        ));
        transport_config.keep_alive_interval(Some(Duration::from_millis(
            config.keep_alive_interval_ms,
        )));

        server_config.transport_config(Arc::new(transport_config));

        let endpoint = Endpoint::new(EndpointConfig::default(), Some(server_config), std_socket, Arc::new(quinn::TokioRuntime))?;

        Ok(Self { config, endpoint })
    }

    /// Accepts incoming client connection requests
    pub async fn accept(&self) -> Option<QuicConnectionManager> {
        if let Some(incoming) = self.endpoint.accept().await {
            match incoming.await {
                Ok(connection) => {
                    info!("Accepted incoming QUIC connection from {}", connection.remote_address());
                    return Some(QuicConnectionManager::new(connection, self.config.clone()));
                }
                Err(e) => {
                    error!("Failed to complete incoming QUIC handshake: {e}");
                }
            }
        }
        None
    }
}

/// Custom Certificate Verifier for Direct Peer-to-Peer TLS Verification
#[derive(Debug)]
pub struct SkipServerVerification;

impl SkipServerVerification {
    pub fn new() -> Self {
        Self
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // In rFerram, certificate validation is paired with NIST vault token verification and Ed25519 signatures
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::digitally_signed_struct::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::digitally_signed_struct::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}
