//! Live QUIC Diagnostics, Atomic Byte/Datagram Counters & RTT Performance Tracking
//!
//! Provides [`NetworkMetrics`] for lock-free tracking of connection Round-Trip Time (RTT),
//! datagram/stream throughput counters, and packet loss rate calculation.

use std::sync::atomic::AtomicU64;

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

impl NetworkMetrics {
    pub fn calculate_loss_rate(packets_sent: u64, packets_lost: u64) -> f32 {
        if packets_sent == 0 {
            return 0.0;
        }
        (packets_lost as f32 / packets_sent as f32) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_metrics_loss_calculation() {
        assert_eq!(NetworkMetrics::calculate_loss_rate(1000, 25), 2.5);
        assert_eq!(NetworkMetrics::calculate_loss_rate(0, 25), 0.0);
    }
}
