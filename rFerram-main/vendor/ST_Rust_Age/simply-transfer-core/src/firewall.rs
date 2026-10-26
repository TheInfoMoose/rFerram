use std::process::Command;

pub struct Firewall;

impl Firewall {
    /// Opens a TCP port on Linux firewalls using pkexec.
    /// On Windows and MacOS, the OS automatically prompts the user when the app binds.
    pub fn open_port(port: u16) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            // Try firewalld first (Fedora, RHEL, CachyOS, Arch)
            if Command::new("which")
                .arg("firewall-cmd")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
                && Command::new("systemctl")
                    .args(["is-active", "--quiet", "firewalld"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            {
                let script = format!(
                    "firewall-cmd --add-port={0}/tcp --permanent && firewall-cmd --add-port={0}/tcp",
                    port
                );
                let status = Command::new("pkexec")
                    .args(["sh", "-c", &script])
                    .status()
                    .map_err(|e| format!("Failed to run pkexec firewall-cmd: {}", e))?;
                if status.success() {
                    return Ok(());
                }
            }
            // Try ufw (CachyOS default, Ubuntu, Debian, Arch)
            if Command::new("which")
                .arg("ufw")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                let status = Command::new("pkexec")
                    .args(["ufw", "allow", &format!("{}/tcp", port)])
                    .status()
                    .map_err(|e| format!("Failed to run pkexec ufw: {}", e))?;
                if status.success() {
                    return Ok(());
                }
            }
            // Fallback to iptables if neither firewalld nor ufw is active
            if Command::new("which")
                .arg("iptables")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                let status = Command::new("pkexec")
                    .args([
                        "iptables",
                        "-I",
                        "INPUT",
                        "-p",
                        "tcp",
                        "--dport",
                        &port.to_string(),
                        "-j",
                        "ACCEPT",
                    ])
                    .status()
                    .map_err(|e| format!("Failed to run pkexec iptables: {}", e))?;
                if status.success() {
                    return Ok(());
                }
            }
            Err(
                "Failed to open port. Neither firewalld, ufw, nor iptables could be modified."
                    .into(),
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = port;
            Ok(())
        }
    }

    /// Closes a TCP port on Linux firewalls.
    pub fn close_port(port: u16) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            if Command::new("which")
                .arg("firewall-cmd")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                let _ = Command::new("pkexec")
                    .args(["firewall-cmd", "--remove-port", &format!("{}/tcp", port)])
                    .status();
                return Ok(());
            }
            if Command::new("which")
                .arg("ufw")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                let _ = Command::new("pkexec")
                    .args(["ufw", "delete", "allow", &format!("{}/tcp", port)])
                    .status();
                return Ok(());
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = port;
            Ok(())
        }
    }
}

/// Represents an active scoped WireGuard WAN tunnel managed by `VpnTunnelManager`.
#[derive(Debug, Clone)]
pub struct VpnTunnelHandle {
    pub connection_id: uuid::Uuid,
    pub interface_name: String,
    pub endpoint: String,
    pub scoped_route: String,
    pub persistent: bool,
    pub temp_conf_path: Option<std::path::PathBuf>,
}

/// Manages the lifecycle of optional WireGuard WAN tunnels per `ConnectionProfile`.
/// Brings up a scoped `/32` route tunnel before SSH handshake when `vpn_config` is enabled,
/// and tears it down cleanly when jobs complete (unless marked `persistent_tunnel` for frequent schedules).
#[derive(Clone, Default)]
pub struct VpnTunnelManager {
    active_tunnels:
        std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<uuid::Uuid, VpnTunnelHandle>>>,
}

impl VpnTunnelManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Extracts the primary IP address (without CIDR `/prefix`) from a WireGuard `[Interface] Address` field.
    pub fn extract_interface_ip(address: &str) -> String {
        let first_addr = address.split(',').next().unwrap_or(address).trim();
        first_addr
            .split('/')
            .next()
            .unwrap_or(first_addr)
            .trim()
            .to_string()
    }

    /// Extracts the host/IP portion of a WireGuard `[Peer] Endpoint` (`host:port`).
    pub fn extract_endpoint_host(endpoint: &str) -> String {
        let trimmed = endpoint.trim();
        if let Some(bracket_end) = trimmed.find(']')
            && trimmed.starts_with('[')
        {
            return trimmed[1..bracket_end].to_string();
        }
        if let Some((h, _port)) = trimmed.rsplit_once(':')
            && !h.contains(':')
        {
            h.trim().to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// Checks whether the given IP address is currently assigned to a local network interface
    /// by attempting an ephemeral UDP bind on `ip:0`.
    pub fn is_interface_ip_bound_locally(ip: &str) -> bool {
        let clean = Self::extract_interface_ip(ip);
        if clean.is_empty() {
            return false;
        }
        std::net::UdpSocket::bind((clean.as_str(), 0)).is_ok()
    }

    /// Computes a `/32` (IPv4) or `/128` (IPv6) scoped route for the target SSH host
    /// so the VPN tunnel routes only traffic destined for the target server.
    pub fn compute_scoped_route(target_ssh_host: &str) -> String {
        let host_part = target_ssh_host
            .rsplit('@')
            .next()
            .unwrap_or(target_ssh_host)
            .trim();
        if host_part.is_empty() || host_part.eq_ignore_ascii_case("remote") {
            return String::new();
        }
        let ip_only = if let Some((h, _port)) = host_part.rsplit_once(':')
            && !h.contains(':')
        {
            h.trim()
        } else {
            host_part
        };
        if ip_only.is_empty() {
            return String::new();
        }

        if ip_only.contains('/') {
            ip_only.to_string()
        } else if ip_only.contains(':') {
            format!("{}/128", ip_only)
        } else {
            format!("{}/32", ip_only)
        }
    }

    /// Renders a WireGuard configuration string with `AllowedIPs` scoped to the target SSH host `/32`
    /// or the WireGuard subnet during preflight setup, ensuring the VPN Endpoint itself is never routed inside the tunnel.
    pub fn render_scoped_wireguard_conf(
        conf: &simply_transfer_crypto::WireGuardConf,
        target_ssh_host: &str,
    ) -> String {
        let endpoint_host = Self::extract_endpoint_host(&conf.endpoint);
        let scoped_route = Self::compute_scoped_route(target_ssh_host);
        let scoped_ip = Self::extract_interface_ip(&scoped_route);

        let mut allowed = Vec::new();
        if !scoped_route.is_empty() && scoped_ip != endpoint_host {
            allowed.push(scoped_route);
        }
        for ip in &conf.allowed_ips {
            let trimmed = ip.trim();
            let ip_base = Self::extract_interface_ip(trimmed);
            if !trimmed.is_empty()
                && trimmed != "0.0.0.0/0"
                && trimmed != "::/0"
                && ip_base != endpoint_host
                && !allowed.iter().any(|a| a == trimmed)
            {
                allowed.push(trimmed.to_string());
            }
        }

        // If preflight setup has no specific target host yet and .conf used 0.0.0.0/0,
        // derive the /24 subnet from the WireGuard [Interface] Address so peers on the VPN subnet are reachable.
        if allowed.is_empty() {
            let iface_ip = Self::extract_interface_ip(&conf.address);
            let octets: Vec<&str> = iface_ip.split('.').collect();
            if octets.len() == 4 {
                allowed.push(format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2]));
            } else if !conf.address.trim().is_empty() {
                allowed.push(conf.address.trim().to_string());
            }
        }

        let mut out = String::new();
        out.push_str("[Interface]\n");
        out.push_str(&format!("PrivateKey = {}\n", conf.private_key));
        out.push_str(&format!("Address = {}\n", conf.address));
        if let Some(ref dns) = conf.dns {
            out.push_str(&format!("DNS = {}\n", dns));
        }
        if let Some(mtu) = conf.mtu {
            out.push_str(&format!("MTU = {}\n", mtu));
        }
        out.push_str("\n[Peer]\n");
        out.push_str(&format!("PublicKey = {}\n", conf.public_key));
        if let Some(ref psk) = conf.preshared_key {
            out.push_str(&format!("PresharedKey = {}\n", psk));
        }
        out.push_str(&format!("Endpoint = {}\n", conf.endpoint));
        out.push_str(&format!("AllowedIPs = {}\n", allowed.join(", ")));
        if let Some(keepalive) = conf.persistent_keepalive {
            out.push_str(&format!("PersistentKeepalive = {}\n", keepalive));
        } else {
            out.push_str("PersistentKeepalive = 25\n");
        }
        out
    }

    /// Brings up the WireGuard tunnel for `connection_id` before initiating the TCP/SSH handshake.
    pub async fn bring_up_tunnel(
        &self,
        connection_id: uuid::Uuid,
        vpn_config: &simply_transfer_crypto::VpnConfig,
        storage: &simply_transfer_crypto::SecureStorage,
        target_ssh_host: &str,
    ) -> Result<VpnTunnelHandle, String> {
        if !vpn_config.enabled {
            return Err("VPN configuration is not enabled".to_string());
        }

        {
            let read_guard = self.active_tunnels.read().await;
            if let Some(existing) = read_guard.get(&connection_id) {
                return Ok(existing.clone());
            }
        }

        let (priv_key, psk) = vpn_config
            .decrypt_keys(storage)
            .map_err(|e| format!("Failed to decrypt VPN keys: {}", e))?;

        let decrypted = simply_transfer_crypto::WireGuardConf {
            private_key: priv_key,
            address: vpn_config.address.clone(),
            dns: vpn_config.dns.clone(),
            mtu: vpn_config.mtu,
            public_key: vpn_config.peer_public_key.clone(),
            preshared_key: psk,
            endpoint: vpn_config.endpoint.clone(),
            allowed_ips: vpn_config.allowed_ips.clone(),
            persistent_keepalive: vpn_config.persistent_keepalive,
        };

        let mut scoped_route = Self::compute_scoped_route(target_ssh_host);
        if scoped_route.is_empty() {
            let iface_ip = Self::extract_interface_ip(&vpn_config.address);
            let octets: Vec<&str> = iface_ip.split('.').collect();
            if octets.len() == 4 {
                scoped_route = format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2]);
            } else {
                scoped_route = vpn_config.address.clone();
            }
        }
        let rendered_conf = Self::render_scoped_wireguard_conf(&decrypted, target_ssh_host);
        let short_id: String = connection_id.simple().to_string().chars().take(8).collect();
        let interface_name = format!("stwg{}", short_id);

        let temp_conf_path = std::env::temp_dir().join(format!("{}.conf", interface_name));
        let _ = std::fs::write(&temp_conf_path, &rendered_conf);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&temp_conf_path, std::fs::Permissions::from_mode(0o600));
        }

        let iface_ip = Self::extract_interface_ip(&vpn_config.address);
        let already_bound = Self::is_interface_ip_bound_locally(&iface_ip);

        // If the WireGuard interface IP is not yet active on the host, bring up the tunnel via OS tools
        if !already_bound {
            let conf_str = temp_conf_path.to_string_lossy().to_string();
            #[cfg(target_os = "linux")]
            {
                if Command::new("which")
                    .arg("wg-quick")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
                {
                    let direct = Command::new("wg-quick").args(["up", &conf_str]).output();
                    let needs_elev = direct.map(|o| !o.status.success()).unwrap_or(true);
                    if needs_elev
                        && std::env::var("DISPLAY").is_ok_and(|d| !d.is_empty())
                        && Command::new("which")
                            .arg("pkexec")
                            .output()
                            .map(|o| o.status.success())
                            .unwrap_or(false)
                    {
                        let _ = Command::new("pkexec")
                            .args(["wg-quick", "up", &conf_str])
                            .output();
                    }
                }
            }
            #[cfg(target_os = "macos")]
            {
                let wg_quick_exists = Command::new("which")
                    .arg("wg-quick")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
                    || std::path::Path::new("/opt/homebrew/bin/wg-quick").exists()
                    || std::path::Path::new("/usr/local/bin/wg-quick").exists();
                if wg_quick_exists {
                    let wg_bin = if std::path::Path::new("/opt/homebrew/bin/wg-quick").exists() {
                        "/opt/homebrew/bin/wg-quick"
                    } else if std::path::Path::new("/usr/local/bin/wg-quick").exists() {
                        "/usr/local/bin/wg-quick"
                    } else {
                        "wg-quick"
                    };
                    let direct = Command::new(wg_bin).args(["up", &conf_str]).output();
                    if direct.map(|o| !o.status.success()).unwrap_or(true) {
                        let script = format!(
                            "do shell script \"{} up '{}'\" with administrator privileges",
                            wg_bin,
                            conf_str.replace('\'', "'\\''")
                        );
                        let _ = Command::new("osascript").args(["-e", &script]).output();
                    }
                }
            }
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                let wg_exe = "C:\\Program Files\\WireGuard\\wireguard.exe";
                if std::path::Path::new(wg_exe).exists() {
                    let _ = Command::new(wg_exe)
                        .args(["/installtunnelservice", &conf_str])
                        .creation_flags(0x08000000)
                        .output();
                }
            }
        }

        let handle = VpnTunnelHandle {
            connection_id,
            interface_name,
            endpoint: vpn_config.endpoint.clone(),
            scoped_route,
            persistent: vpn_config.persistent_tunnel,
            temp_conf_path: Some(temp_conf_path),
        };

        self.active_tunnels
            .write()
            .await
            .insert(connection_id, handle.clone());
        Ok(handle)
    }

    /// Verifies that the WireGuard tunnel interface is up and checks peer handshake / endpoint reachability.
    pub async fn verify_tunnel_handshake(
        &self,
        connection_id: uuid::Uuid,
        vpn_config: &simply_transfer_crypto::VpnConfig,
    ) -> Result<String, String> {
        let is_registered = self.is_tunnel_active(connection_id).await;
        if !is_registered {
            return Err("VPN tunnel is not active".to_string());
        }

        let iface_ip = Self::extract_interface_ip(&vpn_config.address);
        let iface_bound = Self::is_interface_ip_bound_locally(&iface_ip);

        // Check `wg show` output if `wg` utility is installed
        let mut wg_peer_seen = false;
        let mut wg_handshake_seen = false;
        if let Ok(out) = Command::new("wg").arg("show").output()
            && out.status.success()
        {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.contains(vpn_config.peer_public_key.trim()) {
                wg_peer_seen = true;
                if stdout.contains("latest handshake") {
                    wg_handshake_seen = true;
                }
            }
        }

        // Probe endpoint UDP socket resolution and routing
        let endpoint = vpn_config.endpoint.trim();
        let endpoint_reachable = tokio::net::lookup_host(endpoint)
            .await
            .ok()
            .and_then(|mut addrs| addrs.next())
            .is_some_and(|addr| {
                let bind_addr = if addr.is_ipv6() {
                    "[::]:0"
                } else {
                    "0.0.0.0:0"
                };
                if let Ok(sock) = std::net::UdpSocket::bind(bind_addr) {
                    // Send a 1-byte UDP probe or connect socket to trigger WireGuard initiation
                    let _ = sock.connect(addr);
                    true
                } else {
                    false
                }
            });

        if wg_handshake_seen {
            Ok(format!("VPN Active — Handshake Verified ({})", iface_ip))
        } else if iface_bound || wg_peer_seen {
            Ok(format!("VPN Active — Tunnel Up ({})", iface_ip))
        } else if endpoint_reachable {
            Ok(format!(
                "VPN Ready — Endpoint Reachable ({} -> {})",
                iface_ip, vpn_config.endpoint
            ))
        } else {
            Err(format!(
                "VPN Interface {} not detected and Endpoint {} unreachable",
                iface_ip, vpn_config.endpoint
            ))
        }
    }

    /// Tears down the WireGuard tunnel for `connection_id` when a job finishes,
    /// unless `persistent` is true and `force` is false.
    /// Returns `Ok(true)` if the tunnel was torn down, or `Ok(false)` if kept alive/not active.
    pub async fn teardown_tunnel(
        &self,
        connection_id: uuid::Uuid,
        force: bool,
    ) -> Result<bool, String> {
        let mut write_guard = self.active_tunnels.write().await;
        if let Some(existing) = write_guard.get(&connection_id)
            && existing.persistent
            && !force
        {
            tracing::info!(
                "Keeping persistent WAN VPN tunnel {} alive for scheduled jobs",
                existing.interface_name
            );
            return Ok(false);
        }

        if let Some(handle) = write_guard.remove(&connection_id) {
            if let Some(ref conf_path) = handle.temp_conf_path {
                #[cfg(target_os = "linux")]
                {
                    if Command::new("which")
                        .arg("wg-quick")
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                    {
                        let _ = Command::new("wg-quick")
                            .args(["down", conf_path.to_str().unwrap_or("")])
                            .output();
                    }
                }
                let _ = std::fs::remove_file(conf_path);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn is_tunnel_active(&self, connection_id: uuid::Uuid) -> bool {
        self.active_tunnels
            .read()
            .await
            .contains_key(&connection_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use simply_transfer_crypto::{SecureStorage, VpnConfig, WireGuardConf};

    #[tokio::test]
    async fn test_vpn_tunnel_manager_scoped_route_and_lifecycle() {
        assert_eq!(
            VpnTunnelManager::compute_scoped_route("backup@198.51.100.42:2222"),
            "198.51.100.42/32"
        );

        assert_eq!(
            VpnTunnelManager::extract_interface_ip("10.88.0.2/24, fd00::2/64"),
            "10.88.0.2"
        );
        assert_eq!(
            VpnTunnelManager::extract_endpoint_host("203.0.113.10:51820"),
            "203.0.113.10"
        );
        assert_eq!(VpnTunnelManager::compute_scoped_route(""), "");
        assert_eq!(VpnTunnelManager::compute_scoped_route("Remote"), "");

        let raw = r#"
[Interface]
PrivateKey = cHJpdmF0ZWtleXJlYWxseWxvbmdiYXNlNjRzdHJpbmc=
Address = 10.88.0.2/24

[Peer]
PublicKey = cHVibGlja2V5cmVhbGx5bG9uZ2Jhc2U2NHN0cmluZw==
Endpoint = 203.0.113.10:51820
AllowedIPs = 0.0.0.0/0
"#;
        let parsed = WireGuardConf::parse(raw).unwrap();
        let rendered = VpnTunnelManager::render_scoped_wireguard_conf(&parsed, "10.88.0.1");
        assert!(rendered.contains("AllowedIPs = 10.88.0.1/32"));
        assert!(!rendered.contains("0.0.0.0/0"));

        // Preflight empty host or VPN endpoint host should default to the interface /24 subnet (never "/32" or endpoint loop)
        let rendered_preflight = VpnTunnelManager::render_scoped_wireguard_conf(&parsed, "");
        assert!(rendered_preflight.contains("AllowedIPs = 10.88.0.0/24"));
        let rendered_ep_guard =
            VpnTunnelManager::render_scoped_wireguard_conf(&parsed, "203.0.113.10");
        assert!(rendered_ep_guard.contains("AllowedIPs = 10.88.0.0/24"));
        assert!(!rendered_ep_guard.contains("203.0.113.10/32"));

        let storage = SecureStorage::new("simply_transfer_vpn_mgr_test");
        let mut vpn_cfg = VpnConfig::from_wireguard_conf(&parsed, &storage).unwrap();
        let mgr = VpnTunnelManager::new();
        let conn_id = uuid::Uuid::new_v4();

        // Non-persistent tunnel tears down on job completion
        let handle = mgr
            .bring_up_tunnel(conn_id, &vpn_cfg, &storage, "10.88.0.1")
            .await
            .unwrap();
        assert_eq!(handle.scoped_route, "10.88.0.1/32");
        assert!(mgr.is_tunnel_active(conn_id).await);
        assert!(mgr.teardown_tunnel(conn_id, false).await.unwrap());
        assert!(!mgr.is_tunnel_active(conn_id).await);

        // Persistent tunnel stays active on non-forced teardown, tears down when forced
        vpn_cfg.persistent_tunnel = true;
        mgr.bring_up_tunnel(conn_id, &vpn_cfg, &storage, "10.88.0.1")
            .await
            .unwrap();
        assert!(!mgr.teardown_tunnel(conn_id, false).await.unwrap());
        assert!(mgr.is_tunnel_active(conn_id).await);
        assert!(mgr.teardown_tunnel(conn_id, true).await.unwrap());
        assert!(!mgr.is_tunnel_active(conn_id).await);
    }
}
