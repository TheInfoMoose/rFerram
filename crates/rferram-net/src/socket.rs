//! Explicit VPN & Network Interface Discovery and UDP Socket Binding (`if-addrs` & `socket2`)
//!
//! Provides cross-platform network interface enumeration for BYON VPN adapters (`wg*`,
//! `tailscale*`, `utun*`, `tun*`, `unifi*`, `wireguard*`) and constructs non-blocking UDP
//! sockets bound via `SO_BINDTODEVICE` on Linux with graceful unprivileged IP fallback.

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result};
use socket2::{Domain, Protocol, Socket, Type};
use tracing::{debug, info, warn};

/// Discovered VPN or Physical Network Interface Metadata
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredNetworkAdapter {
    pub name: String,
    pub ip: IpAddr,
    pub is_vpn_candidate: bool,
}

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

/// Socket Utility for Explicit VPN Adapter Discovery and Binding via `if-addrs` & `socket2`
pub struct VpnSocketFactory;

impl VpnSocketFactory {
    /// Returns `true` if the interface name matches known VPN tunnel prefixes or substrings
    /// (`wg*`, `tailscale*`, `utun*`, `tun*`, `*unifi*`, `*wireguard*`).
    pub fn is_vpn_interface_name(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        lower.starts_with("wg")
            || lower.starts_with("tailscale")
            || lower.starts_with("utun")
            || lower.starts_with("tun")
            || lower.contains("unifi")
            || lower.contains("wireguard")
    }

    /// Enumerates active network interfaces and highlights VPN candidates (`wg*`, `tailscale*`, `utun*`, `tun*`, `unifi*`).
    pub fn discover_vpn_adapters() -> Result<Vec<DiscoveredNetworkAdapter>> {
        let interfaces =
            if_addrs::get_if_addrs().context("Failed to enumerate OS network interfaces")?;
        let mut out = Vec::new();
        for iface in interfaces {
            if iface.is_loopback() {
                continue;
            }
            let is_vpn_candidate = Self::is_vpn_interface_name(&iface.name);
            out.push(DiscoveredNetworkAdapter {
                name: iface.name,
                ip: iface.addr.ip(),
                is_vpn_candidate,
            });
        }
        Ok(out)
    }

    /// Resolves the first matching IP address for a given interface name matching family `is_ipv4`.
    pub fn resolve_interface_ip(if_name: &str, is_ipv4: bool) -> Option<IpAddr> {
        let interfaces = if_addrs::get_if_addrs().ok()?;
        interfaces
            .into_iter()
            .find(|iface| iface.name == if_name && iface.addr.ip().is_ipv4() == is_ipv4)
            .map(|iface| iface.addr.ip())
    }

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

        socket.set_reuse_address(true)?;
        #[cfg(not(windows))]
        let _ = socket.set_reuse_port(true);

        match interface {
            VpnInterfaceBinding::DefaultSystemRoute => {
                debug!("Binding QUIC socket to default system route: {}", bind_addr);
            }
            VpnInterfaceBinding::SpecificIp(ip) => {
                let spec_addr = SocketAddr::new(*ip, bind_addr.port());
                info!("Binding QUIC socket explicitly to target IP: {}", spec_addr);
                socket.bind(&spec_addr.into())?;
                socket.set_nonblocking(true)?;
                return Ok(socket.into());
            }
            VpnInterfaceBinding::InterfaceName(if_name) => {
                info!("Binding QUIC socket to network interface: {}", if_name);
                #[cfg(target_os = "linux")]
                {
                    if let Err(e) = socket.bind_device(Some(if_name.as_bytes())) {
                        warn!(
                            interface = %if_name,
                            error = %e,
                            "SO_BINDTODEVICE requires CAP_NET_RAW or root; falling back to interface IP binding"
                        );
                        if let Some(ip) = Self::resolve_interface_ip(if_name, bind_addr.is_ipv4()) {
                            let spec_addr = SocketAddr::new(ip, bind_addr.port());
                            socket.bind(&spec_addr.into())?;
                            socket.set_nonblocking(true)?;
                            return Ok(socket.into());
                        }
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    if let Some(ip) = Self::resolve_interface_ip(if_name, bind_addr.is_ipv4()) {
                        let spec_addr = SocketAddr::new(ip, bind_addr.port());
                        socket.bind(&spec_addr.into())?;
                        socket.set_nonblocking(true)?;
                        return Ok(socket.into());
                    }
                }
            }
        }

        socket.bind(&bind_addr.into())?;
        socket.set_nonblocking(true)?;

        Ok(socket.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_vpn_interface_name_classification() {
        assert!(VpnSocketFactory::is_vpn_interface_name("wg0"));
        assert!(VpnSocketFactory::is_vpn_interface_name("WG-corp"));
        assert!(VpnSocketFactory::is_vpn_interface_name("tailscale0"));
        assert!(VpnSocketFactory::is_vpn_interface_name("utun3"));
        assert!(VpnSocketFactory::is_vpn_interface_name("tun0"));
        assert!(VpnSocketFactory::is_vpn_interface_name("unifi-teleport0"));
        assert!(VpnSocketFactory::is_vpn_interface_name("wireguard- peer"));

        assert!(!VpnSocketFactory::is_vpn_interface_name("eth0"));
        assert!(!VpnSocketFactory::is_vpn_interface_name("enp5s0"));
        assert!(!VpnSocketFactory::is_vpn_interface_name("wlan0"));
        assert!(!VpnSocketFactory::is_vpn_interface_name("lo"));
    }

    #[test]
    fn test_discover_vpn_adapters_excludes_loopback() {
        let adapters = VpnSocketFactory::discover_vpn_adapters()
            .expect("Adapter discovery via if-addrs should succeed");
        for adapter in &adapters {
            assert!(
                !adapter.ip.is_loopback(),
                "Loopback address {} should be filtered out from discovered adapters",
                adapter.ip
            );
            assert_eq!(
                adapter.is_vpn_candidate,
                VpnSocketFactory::is_vpn_interface_name(&adapter.name)
            );
        }
    }

    #[test]
    fn test_create_bound_udp_socket_modes() {
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        // 1. DefaultSystemRoute binding
        let default_sock = VpnSocketFactory::create_bound_udp_socket(
            bind_addr,
            &VpnInterfaceBinding::DefaultSystemRoute,
        )
        .expect("Failed to create UDP socket with DefaultSystemRoute");
        let local_default = default_sock.local_addr().unwrap();
        assert_eq!(local_default.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(local_default.port(), 0);

        // 2. SpecificIp binding
        let specific_sock = VpnSocketFactory::create_bound_udp_socket(
            "0.0.0.0:0".parse().unwrap(),
            &VpnInterfaceBinding::SpecificIp(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        )
        .expect("Failed to create UDP socket with SpecificIp");
        let local_specific = specific_sock.local_addr().unwrap();
        assert_eq!(local_specific.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(local_specific.port(), 0);

        // 3. InterfaceName binding (exercises Linux SO_BINDTODEVICE + unprivileged IP fallback)
        let iface_sock = VpnSocketFactory::create_bound_udp_socket(
            bind_addr,
            &VpnInterfaceBinding::InterfaceName("lo".to_string()),
        )
        .expect("Failed to create UDP socket with InterfaceName fallback");
        let local_iface = iface_sock.local_addr().unwrap();
        assert_eq!(local_iface.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(local_iface.port(), 0);
    }
}
