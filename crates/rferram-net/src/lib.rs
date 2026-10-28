//! `rferram-net`: VPN-Aware QUIC Transport Layer over UDP
//!
//! Provides low-latency, multiplexed network transport backed by `quinn` (`0.11.12` / `quinn-proto 0.11.19`)
//! and `rustls` (`0.23.46`, TLS 1.3). Features explicit VPN adapter interface discovery and binding via
//! `if-addrs` and `socket2`, datagram channels for raw input, multiplexed streams for video/audio/virtual
//! channels, TLS 1.3 peer certificate SHA-256 channel binding, and live network metrics.

pub mod endpoint;
pub mod manager;
pub mod metrics;
pub mod socket;

pub use endpoint::*;
pub use manager::*;
pub use metrics::*;
pub use socket::*;

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
