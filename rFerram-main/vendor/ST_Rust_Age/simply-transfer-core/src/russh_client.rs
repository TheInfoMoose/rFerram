use crate::ssh::{SshClient, SshError};
use async_trait::async_trait;
use russh::{client::Config, client::Handle};
use std::path::Path;
use std::sync::Arc;
use tokio::fs;

#[derive(Clone)]
struct ConnectionInfo {
    host: String,
    port: u16,
    username: String,
    private_key_pem: String,
}

#[derive(Default)]
pub struct RusshClient {
    handle: tokio::sync::Mutex<Option<Handle<ClientHandler>>>,
    remote_host: tokio::sync::Mutex<Option<String>>,
    remote_port: tokio::sync::Mutex<u16>,
    conn_info: tokio::sync::Mutex<Option<ConnectionInfo>>,
}

struct ClientHandler;

impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true) // Accept all keys for now
    }
}

impl RusshClient {
    pub fn new() -> Self {
        Self::default()
    }

    fn make_config() -> Arc<Config> {
        let limits = russh::Limits {
            rekey_time_limit: std::time::Duration::from_secs(86400),
            rekey_read_limit: 1024 * 1024 * 1024 * 100, // 100 GB
            rekey_write_limit: 1024 * 1024 * 1024 * 100, // 100 GB
        };

        let config = Config {
            inactivity_timeout: None,
            window_size: 1024 * 1024 * 100, // 100MB window
            limits,
            ..Default::default()
        };
        Arc::new(config)
    }

    async fn do_auth(
        handle: &mut Handle<ClientHandler>,
        username: &str,
        private_key_pem: &str,
    ) -> Result<(), SshError> {
        let key_pair = russh::keys::PrivateKey::from_openssh(private_key_pem)
            .map_err(|e| SshError::AuthenticationFailed(e.to_string()))?;

        let private_key_with_hash =
            russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key_pair), None);

        let auth_res = handle
            .authenticate_publickey(username, private_key_with_hash)
            .await
            .map_err(|e| SshError::AuthenticationFailed(e.to_string()))?;

        match auth_res {
            russh::client::AuthResult::Success => Ok(()),
            _ => Err(SshError::AuthenticationFailed(
                "Key rejected by server".to_string(),
            )),
        }
    }

    async fn open_channel_with_retry(
        &self,
    ) -> Result<russh::Channel<russh::client::Msg>, SshError> {
        let mut handle_guard = self.handle.lock().await;

        if let Some(handle) = handle_guard.as_mut() {
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                handle.channel_open_session(),
            )
            .await
            {
                Ok(Ok(channel)) => return Ok(channel),
                Ok(Err(e)) => {
                    tracing::warn!("Channel open failed ({}), attempting reconnect...", e);
                }
                Err(_) => {
                    tracing::warn!("Channel open timed out, attempting reconnect...");
                }
            }
        }

        let info = self.conn_info.lock().await.clone().ok_or_else(|| {
            SshError::ConnectionFailed("No saved connection information for auto-reconnect".into())
        })?;

        tracing::info!(
            "Attempting SSH auto-reconnect to {}:{}",
            info.host,
            info.port
        );
        let config = Self::make_config();
        let mut new_handle = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            russh::client::connect(config, (info.host.as_str(), info.port), ClientHandler),
        )
        .await
        .map_err(|_| {
            SshError::ConnectionFailed(format!(
                "Connection to {}:{} timed out after 10s (check firewall port 2222 or VPN)",
                info.host, info.port
            ))
        })?
        .map_err(|e| SshError::ConnectionFailed(format!("Reconnect failed: {}", e)))?;

        Self::do_auth(&mut new_handle, &info.username, &info.private_key_pem).await?;

        let channel = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            new_handle.channel_open_session(),
        )
        .await
        .map_err(|_| {
            SshError::CommandExecutionFailed(
                "Timeout opening session channel after reconnect".into(),
            )
        })?
        .map_err(|e| {
            SshError::CommandExecutionFailed(format!(
                "Failed to open session channel after reconnect: {}",
                e
            ))
        })?;

        *handle_guard = Some(new_handle);
        tracing::info!("SSH auto-reconnect succeeded!");
        Ok(channel)
    }

    /// Returns true if an active, open SSH session handle is present.
    pub async fn is_connected(&self) -> bool {
        let guard = self.handle.lock().await;
        guard.as_ref().is_some_and(|h| !h.is_closed())
    }

    /// Opens a dedicated multiplexed SSH channel (`channel_open_session`) on this connection,
    /// allowing concurrent jobs bound to the same `connection_id` to operate independently
    /// without re-handshaking or colliding.
    pub async fn open_dedicated_session_channel(
        &self,
    ) -> Result<russh::Channel<russh::client::Msg>, SshError> {
        self.open_channel_with_retry().await
    }

    /// Queries mounted volumes/drives on the remote server over this SSH session.
    pub async fn list_remote_volumes(
        &self,
    ) -> Result<Vec<simply_transfer_snapshots::VolumeInfo>, SshError> {
        list_remote_volumes_via_ssh(self).await
    }
}

/// Queries mounted volumes/drives on a remote server over any `SshClient`.
pub async fn list_remote_volumes_via_ssh(
    ssh: &dyn SshClient,
) -> Result<Vec<simply_transfer_snapshots::VolumeInfo>, SshError> {
    if let Ok(native_out) = ssh.execute_command("simply-transfer-volumes").await {
        let mut native_volumes = Vec::new();
        for line in native_out.lines() {
            let parts: Vec<&str> = line.trim().split('|').collect();
            if parts.len() == 5 {
                native_volumes.push(simply_transfer_snapshots::VolumeInfo {
                    mount_point: std::path::PathBuf::from(parts[0]),
                    label: parts[1].to_string(),
                    fs_type: parts[2].to_string(),
                    total_bytes: parts[3].parse::<u64>().unwrap_or(0),
                    available_bytes: parts[4].parse::<u64>().unwrap_or(0),
                });
            }
        }
        if !native_volumes.is_empty() {
            return Ok(native_volumes);
        }
    }

    let is_windows = ssh
        .execute_command("cmd.exe /c echo Windows")
        .await
        .map(|out| out.trim() == "Windows")
        .unwrap_or(false);

    if is_windows {
        let cmd = "powershell -NoProfile -NonInteractive -Command \"[System.IO.DriveInfo]::GetDrives() | Where-Object { $_.IsReady } | ForEach-Object { $r = $_.Name; $f = [uint64]$_.AvailableFreeSpace; $t = [uint64]$_.TotalSize; $fs = if ($_.DriveFormat) { $_.DriveFormat } else { 'NTFS' }; $d = if ($_.VolumeLabel) { $_.VolumeLabel } else { $_.Name.TrimEnd('\\') }; Write-Output ($r + '|' + $d + '|' + $fs + '|' + $t + '|' + $f) }\"";
        let output = ssh.execute_command(cmd).await?;
        let mut volumes = Vec::new();
        for line in output.lines() {
            let parts: Vec<&str> = line.trim().split('|').collect();
            if parts.len() == 5 {
                volumes.push(simply_transfer_snapshots::VolumeInfo {
                    mount_point: std::path::PathBuf::from(parts[0]),
                    label: parts[1].to_string(),
                    fs_type: parts[2].to_string(),
                    total_bytes: parts[3].parse::<u64>().unwrap_or(0),
                    available_bytes: parts[4].parse::<u64>().unwrap_or(0),
                });
            }
        }
        if volumes.is_empty() {
            volumes.push(simply_transfer_snapshots::VolumeInfo {
                mount_point: std::path::PathBuf::from("C:\\"),
                label: "Local Disk (C:)".to_string(),
                fs_type: "NTFS".to_string(),
                total_bytes: 0,
                available_bytes: 0,
            });
        }
        Ok(volumes)
    } else {
        let output = ssh.execute_command("df -k -P 2>/dev/null").await?;
        let mut volumes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut root_stats = (0u64, 0u64);
        for line in output.lines().skip(1) {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if let Some(cap_idx) = tokens.iter().position(|t| t.ends_with('%'))
                && cap_idx >= 3
                && cap_idx + 1 < tokens.len()
            {
                let dev = tokens[0];
                let total_kb = tokens[cap_idx - 3].parse::<u64>().unwrap_or(0);
                let avail_kb = tokens[cap_idx - 1].parse::<u64>().unwrap_or(0);
                let total_bytes = total_kb.saturating_mul(1024);
                let avail_bytes = avail_kb.saturating_mul(1024);
                let mount_str = tokens[cap_idx + 1..].join(" ");
                if mount_str == "/" {
                    root_stats = (total_bytes, avail_bytes);
                }
                if mount_str.starts_with("/proc")
                    || mount_str.starts_with("/sys")
                    || mount_str.starts_with("/dev")
                    || (mount_str.starts_with("/run") && !mount_str.starts_with("/run/media"))
                    || mount_str.starts_with("/snap")
                    || mount_str.starts_with("/boot")
                    || dev == "tmpfs"
                    || dev == "devtmpfs"
                    || dev == "overlay"
                    || dev == "efivarfs"
                {
                    continue;
                }
                if seen.insert(mount_str.clone()) {
                    volumes.push(simply_transfer_snapshots::VolumeInfo {
                        mount_point: std::path::PathBuf::from(&mount_str),
                        label: format!("{} ({})", mount_str, dev),
                        fs_type: "posix".to_string(),
                        total_bytes,
                        available_bytes: avail_bytes,
                    });
                }
            }
        }
        if let Ok(mnt_out) = ssh
            .execute_command(
                "for d in /mnt/* /media/*; do [ -d \"$d\" ] && echo \"$d\"; done 2>/dev/null",
            )
            .await
        {
            for line in mnt_out.lines() {
                let mount_str = line.trim().to_string();
                if mount_str.is_empty() || mount_str.starts_with("/mnt/st_snap_") {
                    continue;
                }
                if seen.insert(mount_str.clone()) {
                    volumes.push(simply_transfer_snapshots::VolumeInfo {
                        mount_point: std::path::PathBuf::from(&mount_str),
                        label: mount_str.clone(),
                        fs_type: "posix".to_string(),
                        total_bytes: root_stats.0,
                        available_bytes: root_stats.1,
                    });
                }
            }
        }
        if volumes.is_empty() {
            volumes.push(simply_transfer_snapshots::VolumeInfo {
                mount_point: std::path::PathBuf::from("/"),
                label: "Root (/)".to_string(),
                fs_type: "posix".to_string(),
                total_bytes: 0,
                available_bytes: 0,
            });
        }
        Ok(volumes)
    }
}

/// Pools active authenticated SSH client handles by `connection_id: Uuid` so multiple
/// `TransferJob` instances (e.g., hourly syncs and daily backups) share a single SSH session
/// and multiplex dedicated channels via `channel_open_session`.
#[derive(Clone, Default)]
pub struct SshConnectionPool {
    sessions: Arc<tokio::sync::RwLock<std::collections::HashMap<uuid::Uuid, Arc<RusshClient>>>>,
}

impl SshConnectionPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Retrieves an existing connected `RusshClient` for `connection_id`, or establishes and
    /// authenticates a new session if none is active.
    pub async fn get_or_connect(
        &self,
        connection_id: uuid::Uuid,
        host: &str,
        port: u16,
        username: &str,
        private_key_pem: &str,
        passphrase: Option<&str>,
    ) -> Result<Arc<RusshClient>, SshError> {
        {
            let read_guard = self.sessions.read().await;
            if let Some(existing) = read_guard.get(&connection_id)
                && existing.is_connected().await
            {
                tracing::info!(
                    "Reusing pooled SSH session for connection_id={} ({}:{})",
                    connection_id,
                    host,
                    port
                );
                return Ok(existing.clone());
            }
        }

        let mut write_guard = self.sessions.write().await;
        if let Some(existing) = write_guard.get(&connection_id)
            && existing.is_connected().await
        {
            return Ok(existing.clone());
        }

        let mut client = RusshClient::new();
        client.connect(host, port).await?;
        client
            .authenticate_publickey(username, private_key_pem, passphrase)
            .await?;

        let arc_client = Arc::new(client);
        write_guard.insert(connection_id, arc_client.clone());
        Ok(arc_client)
    }

    /// Retrieves or establishes a pooled SSH session, automatically bringing up the scoped
    /// WireGuard WAN tunnel first if `vpn_config` is enabled.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_or_connect_with_vpn(
        &self,
        connection_id: uuid::Uuid,
        host: &str,
        port: u16,
        username: &str,
        private_key_pem: &str,
        passphrase: Option<&str>,
        vpn_config: Option<&simply_transfer_crypto::VpnConfig>,
        tunnel_mgr: &crate::firewall::VpnTunnelManager,
        storage: &simply_transfer_crypto::SecureStorage,
    ) -> Result<Arc<RusshClient>, SshError> {
        if let Some(cfg) = vpn_config
            && cfg.enabled
        {
            tunnel_mgr
                .bring_up_tunnel(connection_id, cfg, storage, host)
                .await
                .map_err(SshError::ConnectionFailed)?;
        }
        self.get_or_connect(
            connection_id,
            host,
            port,
            username,
            private_key_pem,
            passphrase,
        )
        .await
    }

    /// Inserts an already-authenticated `RusshClient` into the pool under `connection_id`.
    pub async fn insert(&self, connection_id: uuid::Uuid, client: Arc<RusshClient>) {
        self.sessions.write().await.insert(connection_id, client);
    }

    /// Looks up a pooled `RusshClient` by `connection_id`.
    pub async fn get(&self, connection_id: uuid::Uuid) -> Option<Arc<RusshClient>> {
        self.sessions.read().await.get(&connection_id).cloned()
    }

    /// Removes a pooled `RusshClient` for `connection_id`.
    pub async fn remove(&self, connection_id: uuid::Uuid) -> Option<Arc<RusshClient>> {
        self.sessions.write().await.remove(&connection_id)
    }
}

#[async_trait]
impl SshClient for RusshClient {
    async fn connect(&mut self, host: &str, port: u16) -> Result<(), SshError> {
        let mut handle_guard = self.handle.lock().await;
        if handle_guard.is_some() {
            return Ok(());
        }

        *self.remote_host.lock().await = Some(host.to_string());
        *self.remote_port.lock().await = port;

        let config = Self::make_config();
        let client = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            russh::client::connect(config, (host, port), ClientHandler),
        )
        .await
        .map_err(|_| {
            SshError::ConnectionFailed(format!(
                "SSH connection to {}:{} timed out after 10s (ensure Simply Transfer is open on remote device, port {}/tcp is allowed in firewall, or Preflight VPN is connected)",
                host, port, port
            ))
        })?
        .map_err(|e| SshError::ConnectionFailed(e.to_string()))?;

        *handle_guard = Some(client);
        Ok(())
    }

    async fn authenticate_publickey(
        &mut self,
        username: &str,
        private_key_pem: &str,
        _passphrase: Option<&str>,
    ) -> Result<(), SshError> {
        let mut handle_guard = self.handle.lock().await;
        let handle = handle_guard
            .as_mut()
            .ok_or_else(|| SshError::ConnectionFailed("Not connected".to_string()))?;

        Self::do_auth(handle, username, private_key_pem).await?;

        // Save connection info for auto-reconnect
        let host = self.remote_host.lock().await.clone().unwrap_or_default();
        let port = *self.remote_port.lock().await;
        *self.conn_info.lock().await = Some(ConnectionInfo {
            host,
            port: if port == 0 { 2222 } else { port },
            username: username.to_string(),
            private_key_pem: private_key_pem.to_string(),
        });

        Ok(())
    }

    async fn get_remote_file_size(
        &self,
        remote_path: &str,
        is_windows_dest: bool,
    ) -> Result<u64, SshError> {
        let exec_request = if is_windows_dest {
            // Check if file exists first to avoid error spam
            format!(
                "powershell -NoProfile -Command \"if (Test-Path '{}') {{ (Get-Item '{}').length }} else {{ 0 }}\"",
                remote_path.replace("'", "''"),
                remote_path.replace("'", "''")
            )
        } else {
            let escaped = remote_path.replace("'", "'\\''");
            format!(
                "f='{}'; if [ -f \"$f\" ]; then stat -c%s \"$f\" 2>/dev/null || stat -f%z \"$f\" 2>/dev/null || wc -c < \"$f\" 2>/dev/null || echo 0; else echo 0; fi",
                escaped
            )
        };

        let output = self.execute_command(&exec_request).await?;
        Ok(output.trim().parse::<u64>().unwrap_or(0))
    }

    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        offset: u64,
        progress_callback: Option<Box<dyn Fn(u64) + Send>>,
        mut cancel_rx: tokio::sync::watch::Receiver<crate::engine::ControlSignal>,
    ) -> Result<(), SshError> {
        let mut channel = self.open_channel_with_retry().await?;

        let subsystem_name = format!("data-quic|{}|{}", offset, remote_path);
        channel
            .request_subsystem(true, &subsystem_name)
            .await
            .map_err(|e| {
                SshError::CommandExecutionFailed(format!(
                    "Failed to request subsystem {}: {}",
                    subsystem_name, e
                ))
            })?;

        let mut response_str = String::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                russh::ChannelMsg::Data { ref data } => {
                    response_str = String::from_utf8_lossy(data).to_string();
                    break;
                }
                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => {
                    break;
                }
                _ => {}
            }
        }
        let _ = channel.close().await;

        if response_str.is_empty() {
            return Err(SshError::FileTransferFailed(
                "No QUIC port provided by server".into(),
            ));
        }

        let parts: Vec<&str> = response_str.split('|').collect();
        if parts.len() != 3 {
            return Err(SshError::FileTransferFailed(
                "Invalid QUIC connection response".into(),
            ));
        }
        let port: u16 = parts[0].parse().unwrap_or(0);
        let token: u64 = parts[1].parse().unwrap_or(0);

        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let cert_der = STANDARD
            .decode(parts[2])
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;

        let mut root_cert_store = rustls::RootCertStore::empty();
        root_cert_store
            .add(rustls::pki_types::CertificateDer::from(cert_der))
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;

        let _ = rustls::crypto::ring::default_provider().install_default();

        let mut client_crypto = rustls::ClientConfig::builder()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth();
        client_crypto.alpn_protocols = vec![b"simply-transfer".to_vec()];

        let quic_client_config = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;

        let mut client_endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;
        client_endpoint.set_default_client_config(quinn::ClientConfig::new(std::sync::Arc::new(
            quic_client_config,
        )));

        let remote_host_opt = self.remote_host.lock().await.clone();
        let remote_host = remote_host_opt.as_deref().unwrap_or("127.0.0.1");
        let connect_addr: std::net::SocketAddr = format!("{}:{}", remote_host, port)
            .parse()
            .map_err(|e| SshError::FileTransferFailed(format!("Invalid socket address: {}", e)))?;

        let connection = client_endpoint
            .connect(connect_addr, "localhost")
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?
            .await
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;
        let mut bi = connection
            .open_bi()
            .await
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;

        use tokio::io::AsyncReadExt;
        bi.0.write_all(&token.to_le_bytes())
            .await
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;

        let mut file = fs::File::open(local_path)
            .await
            .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;

        if offset > 0 {
            use tokio::io::AsyncSeekExt;
            file.seek(tokio::io::SeekFrom::Start(offset))
                .await
                .map_err(|e| SshError::FileTransferFailed(e.to_string()))?;
        }

        let mut buffer = vec![0u8; 1024 * 1024 * 4]; // 4MB chunks
        let mut progress = offset;

        loop {
            tokio::select! {
                res = file.read(&mut buffer) => {
                    match res {
                        Ok(0) => break,
                        Ok(n) => {
                            if bi.0.write_all(&buffer[..n]).await.is_err() {
                                return Err(SshError::FileTransferFailed("Connection dropped".into()));
                            }
                            progress += n as u64;
                            if let Some(cb) = &progress_callback {
                                cb(progress);
                            }
                        }
                        Err(e) => return Err(SshError::FileTransferFailed(e.to_string())),
                    }
                }
                _ = cancel_rx.changed() => {
                    let signal = cancel_rx.borrow().clone();
                    match signal {
                        crate::engine::ControlSignal::Cancel => {
                            let _ = bi.0.finish();
                            connection.close(0u32.into(), b"cancelled");
                            return Err(SshError::FileTransferFailed("Transfer cancelled".into()));
                        }
                        crate::engine::ControlSignal::Pause => {
                            // Wait for either Resume or Cancel
                            loop {
                                tokio::select! {
                                    _ = cancel_rx.changed() => {
                                        let pause_signal = cancel_rx.borrow().clone();
                                        match pause_signal {
                                            crate::engine::ControlSignal::Cancel => {
                                                let _ = bi.0.finish();
                                                connection.close(0u32.into(), b"cancelled");
                                                return Err(SshError::FileTransferFailed("Transfer cancelled".into()));
                                            }
                                            crate::engine::ControlSignal::Run => break,
                                            _ => continue, // Ignore other signals
                                        }
                                    }
                                }
                            }
                        }
                        _ => continue, // Ignore other signals
                    }
                }
            }
        }

        let _ = bi.0.finish();

        let mut ack_buf = [0u8; 2];
        if bi.1.read_exact(&mut ack_buf).await.is_ok() {
            if &ack_buf != b"OK" {
                tracing::warn!("Did not receive valid application-layer ACK");
                connection.close(0u32.into(), b"failed");
                return Err(SshError::FileTransferFailed(
                    "Invalid application-layer ACK".into(),
                ));
            }
        } else {
            tracing::warn!("Failed to read application-layer ACK");
            connection.close(0u32.into(), b"failed");
            return Err(SshError::FileTransferFailed(
                "Failed to read application-layer ACK".into(),
            ));
        }

        connection.close(0u32.into(), b"done");

        Ok(())
    }

    async fn execute_command(&self, command: &str) -> Result<String, SshError> {
        let mut channel = self.open_channel_with_retry().await?;

        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            channel.exec(true, command),
        )
        .await
        .map_err(|_| SshError::CommandExecutionFailed("Timeout executing command".to_string()))?
        .map_err(|e| SshError::CommandExecutionFailed(e.to_string()))?;

        let mut output = String::new();
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(600), channel.wait()).await {
                Ok(Some(msg)) => match msg {
                    russh::ChannelMsg::Data { ref data } => {
                        output.push_str(&String::from_utf8_lossy(data));
                    }
                    russh::ChannelMsg::Eof | russh::ChannelMsg::Close => {
                        break;
                    }
                    _ => {}
                },
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!("Timeout waiting for SSH command output");
                    break;
                }
            }
        }

        let _ = channel.close().await;

        Ok(output)
    }
}
