use russh::keys::{HashAlg, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Handler, Server, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc};

pub type OobVerifyCallback = Arc<dyn Fn(String, String, String) + Send + Sync>;
pub type OobActionCallback = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone)]
pub struct TransferServer {
    pub authorized_keys_file_path: PathBuf,
    pub authorized_keys: Arc<tokio::sync::RwLock<HashMap<String, PublicKey>>>,
    file_writers: Arc<Mutex<HashMap<ChannelId, mpsc::Sender<Vec<u8>>>>>,
    pub event_sender: Option<tokio::sync::broadcast::Sender<crate::engine::TransferEvent>>,
    pub oob_verify_callback: Option<OobVerifyCallback>,
    pub oob_action_callback: Option<OobActionCallback>,
}

impl Default for TransferServer {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferServer {
    pub fn new() -> Self {
        let home_dir = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let keys_file_path = PathBuf::from(format!(
            "{}/.simply-transfer/p2p_authorized_keys.json",
            home_dir
        ));

        let mut loaded_keys = if keys_file_path.exists() {
            if let Ok(file) = File::open(&keys_file_path) {
                let reader = BufReader::new(file);
                if let Ok(keys_map) = serde_json::from_reader::<_, HashMap<String, String>>(reader)
                {
                    let mut public_keys = HashMap::new();
                    for (stored_fp, key_base64) in keys_map {
                        if let Ok(public_key) = parse_public_key(&key_base64) {
                            let canonical_fp = public_key.fingerprint(HashAlg::Sha256).to_string();
                            let raw_fp = canonical_fp
                                .strip_prefix("SHA256:")
                                .unwrap_or(&canonical_fp)
                                .to_string();
                            public_keys.insert(stored_fp.clone(), public_key.clone());
                            public_keys.insert(canonical_fp, public_key.clone());
                            public_keys.insert(raw_fp, public_key);
                        }
                    }
                    public_keys
                } else {
                    HashMap::new()
                }
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        // Also ingest any keys already present in standard OS authorized_keys files
        let mut os_key_files = vec![
            PathBuf::from(&home_dir)
                .join(".ssh")
                .join("authorized_keys"),
        ];
        if cfg!(windows) {
            os_key_files.push(PathBuf::from(
                "C:\\ProgramData\\ssh\\administrators_authorized_keys",
            ));
        }
        for key_file in os_key_files {
            if let Ok(contents) = fs::read_to_string(&key_file) {
                for line in contents.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    if let Ok(public_key) = parse_public_key(trimmed) {
                        let canonical_fp = public_key.fingerprint(HashAlg::Sha256).to_string();
                        let raw_fp = canonical_fp
                            .strip_prefix("SHA256:")
                            .unwrap_or(&canonical_fp)
                            .to_string();
                        loaded_keys.insert(canonical_fp, public_key.clone());
                        loaded_keys.insert(raw_fp, public_key);
                    }
                }
            }
        }

        TransferServer {
            authorized_keys_file_path: keys_file_path,
            authorized_keys: Arc::new(tokio::sync::RwLock::new(loaded_keys)),
            file_writers: Arc::new(Mutex::new(HashMap::new())),
            event_sender: None,
            oob_verify_callback: None,
            oob_action_callback: None,
        }
    }

    pub fn set_event_sender(
        &mut self,
        sender: tokio::sync::broadcast::Sender<crate::engine::TransferEvent>,
    ) {
        self.event_sender = Some(sender);
    }

    pub fn set_oob_callbacks(
        &mut self,
        on_verify: OobVerifyCallback,
        on_action: OobActionCallback,
    ) {
        self.oob_verify_callback = Some(on_verify);
        self.oob_action_callback = Some(on_action);
    }

    pub async fn add_authorized_key(&self, key: PublicKey) {
        let canonical_fp = key.fingerprint(HashAlg::Sha256).to_string();
        let raw_fp = canonical_fp
            .strip_prefix("SHA256:")
            .unwrap_or(&canonical_fp)
            .to_string();

        {
            let mut keys = self.authorized_keys.write().await;
            keys.insert(canonical_fp.clone(), key.clone());
            keys.insert(raw_fp, key.clone());
        }

        let keys = self.authorized_keys.read().await;

        let path = &self.authorized_keys_file_path;
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut keys_map: HashMap<String, String> = HashMap::new();
        for public_key in keys.values() {
            let fp = public_key.fingerprint(HashAlg::Sha256).to_string();
            if let Ok(openssh) = public_key.to_openssh() {
                keys_map.insert(fp, openssh);
            }
        }

        if let Ok(file) = File::create(path) {
            let mut writer = BufWriter::new(file);
            let _ = serde_json::to_writer_pretty(&mut writer, &keys_map);
            let _ = writer.flush();

            #[cfg(unix)]
            {
                if let Ok(mut perms) = fs::metadata(path).map(|m| m.permissions()) {
                    perms.set_mode(0o600);
                    let _ = fs::set_permissions(path, perms);
                }
            }
        }
    }

    pub async fn add_authorized_key_from_str(&self, key_str: &str) -> bool {
        if let Ok(key) = parse_public_key(key_str) {
            self.add_authorized_key(key).await;
            true
        } else {
            false
        }
    }

    /// Purge authorized keys from memory and `p2p_authorized_keys.json` that are not in `active_key_strs`
    /// (while preserving keys present in the OS `~/.ssh/authorized_keys` file).
    pub async fn purge_unauthorized_keys(&self, active_key_strs: &[String]) -> usize {
        let mut allowed_fps = std::collections::HashSet::new();
        for k_str in active_key_strs {
            if let Ok(pk) = parse_public_key(k_str) {
                let canonical_fp = pk.fingerprint(HashAlg::Sha256).to_string();
                let raw_fp = canonical_fp
                    .strip_prefix("SHA256:")
                    .unwrap_or(&canonical_fp)
                    .to_string();
                allowed_fps.insert(canonical_fp);
                allowed_fps.insert(raw_fp);
            }
        }

        let mut keys = self.authorized_keys.write().await;
        let mut unique_before = std::collections::HashSet::new();
        for pk in keys.values() {
            unique_before.insert(pk.fingerprint(HashAlg::Sha256).to_string());
        }

        keys.retain(|fp, pk| {
            let canonical = pk.fingerprint(HashAlg::Sha256).to_string();
            allowed_fps.contains(fp) || allowed_fps.contains(&canonical)
        });

        let mut unique_after = std::collections::HashSet::new();
        let mut keys_map: HashMap<String, String> = HashMap::new();
        for public_key in keys.values() {
            let fp = public_key.fingerprint(HashAlg::Sha256).to_string();
            unique_after.insert(fp.clone());
            if let Ok(openssh) = public_key.to_openssh() {
                keys_map.insert(fp, openssh);
            }
        }

        let path = &self.authorized_keys_file_path;
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(file) = File::create(path) {
            let mut writer = BufWriter::new(file);
            let _ = serde_json::to_writer_pretty(&mut writer, &keys_map);
            let _ = writer.flush();
        }

        unique_before.len().saturating_sub(unique_after.len())
    }
}

impl Server for TransferServer {
    type Handler = Self;

    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        self.clone()
    }
}

impl Handler for TransferServer {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();
        let raw_fp = fingerprint.strip_prefix("SHA256:").unwrap_or(&fingerprint);

        let keys = self.authorized_keys.read().await;
        tracing::info!(
            "Received auth request for user '{}' with fingerprint: {}. Known keys: {:?}",
            user,
            fingerprint,
            keys.keys()
        );

        let is_authorized = keys.contains_key(&fingerprint)
            || keys.contains_key(raw_fp)
            || keys.values().any(|k| k == public_key);

        if is_authorized {
            tracing::info!("Key accepted for {}", user);
            Ok(Auth::Accept)
        } else {
            tracing::warn!("Key rejected for {}", user);
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<russh::server::Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        tracing::info!("Session channel opened: {:?}", channel.id());
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name.starts_with("data-quic|") {
            let payload = name.trim_start_matches("data-quic|");
            let (offset, remote_path) = if let Some((offset_str, path)) = payload.split_once('|') {
                if let Ok(off) = offset_str.parse::<u64>() {
                    (off, path.to_string())
                } else {
                    (0, payload.to_string())
                }
            } else {
                (0, payload.to_string())
            };

            tracing::info!(
                "Accepted QUIC binary transfer for: {} at offset {}",
                remote_path,
                offset
            );

            let _ = rustls::crypto::ring::default_provider().install_default();

            let token: u64 = rand::random();
            let cert = match rcgen::generate_simple_self_signed(vec!["localhost".into()]) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to generate QUIC certificate: {}", e);
                    return Ok(());
                }
            };
            let cert_der = cert.cert.der().to_vec();
            let priv_key = cert.signing_key.serialize_der();

            let cert_chain = vec![rustls::pki_types::CertificateDer::from(cert_der.clone())];
            let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
                rustls::pki_types::PrivatePkcs8KeyDer::from(priv_key),
            );

            let mut server_crypto = match rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert_chain, key)
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to build QUIC server crypto: {}", e);
                    return Ok(());
                }
            };
            server_crypto.alpn_protocols = vec![b"simply-transfer".to_vec()];

            let quic_server_config =
                match quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Failed to convert rustls config: {}", e);
                        return Ok(());
                    }
                };
            let server_config =
                quinn::ServerConfig::with_crypto(std::sync::Arc::new(quic_server_config));
            let endpoint =
                match quinn::Endpoint::server(server_config, "0.0.0.0:0".parse().unwrap()) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("Failed to bind QUIC endpoint: {}", e);
                        return Ok(());
                    }
                };
            let port = endpoint.local_addr().unwrap().port();

            use base64::{Engine as _, engine::general_purpose::STANDARD};
            let cert_b64 = STANDARD.encode(&cert_der);

            let response = format!("{}|{}|{}", port, token, cert_b64);
            let _ = session.data(channel, response.into_bytes());
            let _ = session.eof(channel);
            let _ = session.close(channel);

            let event_sender_clone = self.event_sender.clone();

            tokio::spawn(async move {
                if let Some(conn) = endpoint.accept().await
                    && let Ok(connection) = conn.await
                {
                    if let Ok(mut bi) = connection.accept_bi().await {
                        let mut token_buf = [0u8; 8];
                        if bi.1.read_exact(&mut token_buf).await.is_ok() {
                            let received_token = u64::from_le_bytes(token_buf);
                            if received_token == token {
                                if let Some(sender) = &event_sender_clone {
                                    let _ =
                                        sender.send(crate::engine::TransferEvent::TransferStarted(
                                            "Incoming".to_string(),
                                        ));
                                }
                                let normalized_remote_path = if cfg!(windows) {
                                    remote_path.replace('/', "\\")
                                } else {
                                    remote_path.replace('\\', "/")
                                };
                                let path = std::path::Path::new(&normalized_remote_path);
                                if let Some(parent) = path.parent() {
                                    let _ = tokio::fs::create_dir_all(parent).await;
                                }

                                let file_result = if offset == 0 {
                                    tokio::fs::File::create(&normalized_remote_path).await
                                } else {
                                    tokio::fs::OpenOptions::new()
                                        .write(true)
                                        .open(&normalized_remote_path)
                                        .await
                                };

                                if let Ok(mut file) = file_result {
                                    if offset > 0 {
                                        use std::io::SeekFrom;
                                        use tokio::io::AsyncSeekExt;
                                        let _ = file.seek(SeekFrom::Start(offset)).await;
                                    }

                                    let mut buf = vec![0u8; 1024 * 1024];
                                    let mut total_bytes_written = offset;
                                    let mut throttler =
                                        crate::engine::ProgressThrottler::new(offset);

                                    if let Some(sender) = &event_sender_clone {
                                        let _ = sender.send(
                                            crate::engine::TransferEvent::FileStatusChanged(
                                                remote_path.clone(),
                                                crate::engine::FileTransferStatus::Transferring {
                                                    progress_bytes: total_bytes_written,
                                                    total_bytes: 0,
                                                },
                                            ),
                                        );
                                    }

                                    while let Ok(Some(n)) = bi.1.read(&mut buf).await {
                                        if file.write_all(&buf[..n]).await.is_err() {
                                            break;
                                        }
                                        total_bytes_written += n as u64;
                                        if let Some(sender) = &event_sender_clone
                                            && throttler
                                                .should_emit(total_bytes_written, 0)
                                                .is_some()
                                        {
                                            let _ = sender.send(crate::engine::TransferEvent::FileStatusChanged(
                                                remote_path.clone(),
                                                crate::engine::FileTransferStatus::Transferring {
                                                    progress_bytes: total_bytes_written,
                                                    total_bytes: 0,
                                                },
                                            ));
                                        }
                                    }
                                    let _ = file.flush().await;
                                    drop(file);

                                    if let Some(sender) = &event_sender_clone {
                                        let _ = sender.send(
                                            crate::engine::TransferEvent::FileStatusChanged(
                                                remote_path.clone(),
                                                crate::engine::FileTransferStatus::Completed,
                                            ),
                                        );
                                    }

                                    // Send application-layer ACK that the file has been successfully written and closed
                                    let _ = bi.0.write_all(b"OK").await;
                                    let _ = bi.0.finish();
                                }
                            }
                        }
                    }
                    let _ = connection.closed().await;
                }
            });

            Ok(())
        } else {
            tracing::info!("Rejected unknown subsystem: {}", name);
            Ok(())
        }
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        program: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let cmd_str = String::from_utf8_lossy(program).to_string();
        tracing::info!("Executing command via exec_request: {}", cmd_str);

        if cmd_str.trim() == "simply-transfer-volumes" {
            let handle = session.handle();
            tokio::spawn(async move {
                let payload = tokio::task::spawn_blocking(|| {
                    let mut lines = Vec::new();
                    if let Ok(vols) = simply_transfer_snapshots::list_local_volumes() {
                        for v in vols {
                            lines.push(format!(
                                "{}|{}|{}|{}|{}",
                                v.mount_point.to_string_lossy(),
                                v.label,
                                v.fs_type,
                                v.total_bytes,
                                v.available_bytes
                            ));
                        }
                    }
                    format!("{}\n", lines.join("\n"))
                })
                .await
                .unwrap_or_default();

                let _ = handle.data(channel, payload.into_bytes()).await;
                let _ = handle.exit_status_request(channel, 0).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }

        if cmd_str.starts_with("simply-transfer-list|") {
            let target_path = cmd_str
                .trim_start_matches("simply-transfer-list|")
                .trim()
                .to_string();
            let handle = session.handle();
            tokio::spawn(async move {
                let payload =
                    tokio::task::spawn_blocking(move || list_directory_entries(&target_path))
                        .await
                        .unwrap_or_default();

                let _ = handle.data(channel, payload.into_bytes()).await;
                let _ = handle.exit_status_request(channel, 0).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }

        if cmd_str.starts_with("simply-transfer-mkdir|") {
            let paths_str = cmd_str.trim_start_matches("simply-transfer-mkdir|");
            let paths: Vec<String> = paths_str.split('|').map(|s| s.to_string()).collect();

            let handle = session.handle();
            tokio::spawn(async move {
                for path in paths.iter().filter(|&p| !p.trim().is_empty()) {
                    let normalized = if cfg!(windows) {
                        path.replace('/', "\\")
                    } else {
                        path.replace('\\', "/")
                    };
                    let _ = tokio::fs::create_dir_all(&normalized).await;
                }
                let _ = handle.data(channel, b"OK\n".to_vec()).await;
                let _ = handle.exit_status_request(channel, 0).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }

        if cmd_str.starts_with("simply-transfer-exists|") {
            let paths_str = cmd_str.trim_start_matches("simply-transfer-exists|");
            let paths: Vec<String> = paths_str.split('|').map(|s| s.to_string()).collect();

            let handle = session.handle();
            tokio::spawn(async move {
                let mut existing_paths = Vec::new();
                for path in paths.iter().filter(|&p| !p.trim().is_empty()) {
                    let normalized = if cfg!(windows) {
                        path.replace('/', "\\")
                    } else {
                        path.replace('\\', "/")
                    };
                    if tokio::fs::metadata(&normalized).await.is_ok() {
                        existing_paths.push(path.clone());
                    }
                }
                let response = format!("{}\n", existing_paths.join("|"));
                let _ = handle.data(channel, response.into_bytes()).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }

        if cmd_str.starts_with("simply-transfer-hash|") {
            let paths_str = cmd_str.trim_start_matches("simply-transfer-hash|");
            let paths: Vec<String> = paths_str.split('|').map(|s| s.to_string()).collect();

            tracing::info!(
                "simply-transfer-hash| command received with {} files",
                paths.len()
            );

            let handle = session.handle();
            tokio::spawn(async move {
                let output_str = tokio::task::spawn_blocking(move || {
                    let mut out = String::new();
                    for path in paths.iter().filter(|&p| !p.trim().is_empty()) {
                        tracing::info!("Hashing remote file: {}", path);
                        let normalized = if cfg!(windows) {
                            path.replace('/', "\\")
                        } else {
                            path.replace('\\', "/")
                        };
                        let local_hash = if let Ok(f) = std::fs::File::open(&normalized) {
                            simply_transfer_crypto::hash::compute_sha256_stream(f)
                                .unwrap_or_else(|_| "hash_failed".to_string())
                        } else {
                            "hash_failed".to_string()
                        };
                        tracing::info!("Remote file {} hashed: {}", path, local_hash);
                        out.push_str(&format!("{}  {}\n", local_hash, path));
                    }
                    out
                })
                .await
                .unwrap_or_else(|_| String::new());

                tracing::info!(
                    "Sending SSH hash response back to client. Payload length: {}",
                    output_str.len()
                );

                let _ = handle.data(channel, output_str.into_bytes()).await;
                let _ = handle.exit_status_request(channel, 0).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }

        #[cfg(windows)]
        let output = {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("cmd")
                .arg("/Q")
                .arg("/c")
                .raw_arg(&cmd_str)
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .output()
        };
        #[cfg(not(windows))]
        let output = std::process::Command::new("sh")
            .args(["-c", &cmd_str])
            .output();

        match output {
            Ok(out) => {
                let _ = session.data(channel, out.stdout);
                let _ = session.extended_data(channel, 1, out.stderr);
                let _ = session.exit_status_request(channel, out.status.code().unwrap_or(0) as u32);
            }
            Err(e) => {
                tracing::error!("Failed to execute command: {}", e);
                let _ = session.exit_status_request(channel, 1);
            }
        }

        let _ = session.eof(channel);
        let _ = session.close(channel);
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.file_writers.lock().await.get(&channel) {
            let _ = tx.send(data.to_vec()).await;
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.file_writers.lock().await.remove(&channel);
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.file_writers.lock().await.remove(&channel);
        Ok(())
    }
}

pub async fn run_server(
    port: u16,
    server: TransferServer,
    host_key: russh::keys::PrivateKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = russh::server::Config {
        keys: vec![host_key],
        ..Default::default()
    };

    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("Starting SSH transfer server on {}", addr);

    let config = Arc::new(config);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    loop {
        let (mut socket, peer_addr) = listener.accept().await?;
        let config_clone = config.clone();
        let mut server_clone = server.clone();

        tokio::spawn(async move {
            let mut peek_buf = [0u8; 4];
            let is_oob_json = match tokio::time::timeout(
                std::time::Duration::from_millis(150),
                socket.peek(&mut peek_buf),
            )
            .await
            {
                Ok(Ok(n)) if n > 0 => {
                    let slice = &peek_buf[..n];
                    let trimmed = slice
                        .iter()
                        .skip_while(|&&b| b == b' ' || b == b'\t' || b == b'\r' || b == b'\n')
                        .copied()
                        .next();
                    trimmed == Some(b'{')
                }
                _ => false,
            };

            if is_oob_json {
                use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
                let (reader, mut writer) = socket.split();
                let mut buf_reader = tokio::io::BufReader::new(reader);
                let mut line = String::new();
                if tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    buf_reader.read_line(&mut line),
                )
                .await
                .is_ok_and(|r| r.is_ok())
                    && let Ok(req) = serde_json::from_str::<serde_json::Value>(&line)
                {
                    let action = req["action"].as_str().unwrap_or("");
                    if action == "verify" {
                        let dest_ip = peer_addr.ip().to_string();
                        let dest_user = req["user"]
                            .as_str()
                            .unwrap_or("simply-transfer")
                            .to_string();
                        let dest_pub = req
                            .get("pub_key")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        tracing::info!(
                            "Port 2222 OOB verify request from {}@{}",
                            dest_user,
                            dest_ip
                        );

                        if !dest_pub.is_empty()
                            && server_clone.add_authorized_key_from_str(&dest_pub).await
                        {
                            // Also append to ~/.ssh/authorized_keys for OS sshd compatibility
                            let home_dir = std::env::var("HOME")
                                .or_else(|_| std::env::var("USERPROFILE"))
                                .unwrap_or_else(|_| ".".to_string());
                            let ssh_dir = PathBuf::from(&home_dir).join(".ssh");
                            let _ = fs::create_dir_all(&ssh_dir);
                            let auth_keys = ssh_dir.join("authorized_keys");
                            let pub_key_line =
                                format!("ssh-ed25519 {} simply-transfer", dest_pub.trim());
                            let already_present = fs::read_to_string(&auth_keys)
                                .map(|c| c.contains(dest_pub.trim()))
                                .unwrap_or(false);
                            if !already_present
                                && let Ok(mut f) = fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(&auth_keys)
                            {
                                let _ = writeln!(f, "{}", pub_key_line);
                            }

                            let my_user = std::env::var("USER")
                                .or_else(|_| std::env::var("USERNAME"))
                                .unwrap_or_else(|_| "simply-transfer".to_string());
                            let res = format!("{{\"status\":\"ok\",\"user\":\"{}\"}}\n", my_user);
                            let _ = writer.write_all(res.as_bytes()).await;
                            let _ = writer.flush().await;

                            if let Some(cb) = &server_clone.oob_verify_callback {
                                cb(dest_ip, dest_user, dest_pub);
                            }
                            return;
                        }
                        let _ = writer.write_all(b"{\"status\":\"error\"}\n").await;
                        let _ = writer.flush().await;
                    } else if action == "commit" || action == "cancel" {
                        let _ = writer.write_all(b"{\"status\":\"ok\"}\n").await;
                        let _ = writer.flush().await;
                        if let Some(cb) = &server_clone.oob_action_callback {
                            cb(action.to_string());
                        }
                    }
                }
                return;
            }

            if let Err(e) = russh::server::run_stream(
                config_clone,
                socket,
                server_clone.new_client(Some(peer_addr)),
            )
            .await
            {
                tracing::error!("SSH server error: {}", e);
            }
        });
    }
}

pub fn generate_host_key() -> russh::keys::PrivateKey {
    russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
        .expect("Failed to generate Ed25519 host key")
}

pub fn list_directory_entries(target_path: &str) -> String {
    let trimmed = target_path.trim();
    if cfg!(windows) && (trimmed.is_empty() || trimmed == "/") {
        let mut drive_lines = Vec::new();
        for letter in b'A'..=b'Z' {
            let root = format!("{}:\\", letter as char);
            if std::path::Path::new(&root).exists() {
                drive_lines.push(format!("D|{}|{}", root, root));
            }
        }
        return format!("{}\n", drive_lines.join("\n"));
    }

    let effective_path = if trimmed.is_empty() { "/" } else { trimmed };
    let normalized = if cfg!(windows) {
        effective_path.replace('/', "\\")
    } else {
        effective_path.replace('\\', "/")
    };

    let read_res = fs::read_dir(&normalized);
    let entries = match read_res {
        Ok(rd) => rd,
        Err(e) => {
            tracing::warn!("list_directory_entries failed for '{}': {}", normalized, e);
            return format!("E|{}\n", e);
        }
    };

    let mut dirs: Vec<(String, String)> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
            continue;
        }
        let path_buf = entry.path();
        let is_dir = entry
            .file_type()
            .map(|ft| ft.is_dir() || (ft.is_symlink() && path_buf.is_dir()))
            .unwrap_or_else(|_| path_buf.is_dir());

        let full_path = if cfg!(windows) {
            path_buf.to_string_lossy().replace('/', "\\")
        } else {
            path_buf.to_string_lossy().replace('\\', "/")
        };

        if is_dir {
            dirs.push((name, full_path));
        } else {
            files.push((name, full_path));
        }
    }

    dirs.sort_by_key(|a| a.0.to_lowercase());
    files.sort_by_key(|a| a.0.to_lowercase());

    let mut out_lines = Vec::with_capacity(dirs.len() + files.len());
    for (name, full_path) in dirs {
        out_lines.push(format!("D|{}|{}", name, full_path));
    }
    for (name, full_path) in files {
        out_lines.push(format!("F|{}|{}", name, full_path));
    }

    if out_lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", out_lines.join("\n"))
    }
}

pub fn parse_public_key(key_str: &str) -> Result<PublicKey, String> {
    let clean = key_str.split(';').next().unwrap_or(key_str).trim();
    let actual_key = if clean.starts_with("st:") {
        if let Ok(token) = simply_transfer_crypto::token::ConnectionToken::parse(clean) {
            token.pub_key
        } else {
            clean.to_string()
        }
    } else {
        clean.to_string()
    };
    let trimmed = actual_key.trim();
    PublicKey::from_openssh(trimmed)
        .or_else(|_| PublicKey::from_openssh(&format!("ssh-ed25519 {}", trimmed)))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_transfer_server_auth_publickey_matching() {
        let mut server = TransferServer {
            authorized_keys_file_path: PathBuf::from("/nonexistent/keys.json"),
            authorized_keys: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            file_writers: Arc::new(Mutex::new(HashMap::new())),
            event_sender: None,
            oob_verify_callback: None,
            oob_action_callback: None,
        };

        let key = generate_host_key();
        let pub_key = key.public_key().clone();
        let canonical_fp = pub_key.fingerprint(HashAlg::Sha256).to_string();
        let raw_fp = canonical_fp
            .strip_prefix("SHA256:")
            .unwrap_or(&canonical_fp)
            .to_string();

        // Initially rejected
        let res = server.auth_publickey("test_user", &pub_key).await;
        assert!(matches!(res, Ok(Auth::Reject { .. })));

        // Test matching when legacy raw fingerprint is in map
        server
            .authorized_keys
            .write()
            .await
            .insert(raw_fp.clone(), pub_key.clone());
        let res = server.auth_publickey("test_user", &pub_key).await;
        assert!(matches!(res, Ok(Auth::Accept)));

        // Test matching when canonical fingerprint is in map
        server.authorized_keys.write().await.clear();
        server
            .authorized_keys
            .write()
            .await
            .insert(canonical_fp.clone(), pub_key.clone());
        let res = server.auth_publickey("test_user", &pub_key).await;
        assert!(matches!(res, Ok(Auth::Accept)));

        // Test matching via direct PublicKey equality fallback
        server.authorized_keys.write().await.clear();
        server
            .authorized_keys
            .write()
            .await
            .insert("arbitrary_key_id".to_string(), pub_key.clone());
        let res = server.auth_publickey("test_user", &pub_key).await;
        assert!(matches!(res, Ok(Auth::Accept)));
    }

    #[test]
    fn test_parse_public_key_formats() {
        let key = generate_host_key();
        let pub_key = key.public_key().clone();
        let openssh_str = pub_key.to_openssh().unwrap();
        let raw_b64 = openssh_str.strip_prefix("ssh-ed25519 ").unwrap();

        // 1. Standard openssh string
        let parsed = parse_public_key(&openssh_str).unwrap();
        assert_eq!(parsed, pub_key);

        // 2. Raw base64 string
        let parsed = parse_public_key(raw_b64).unwrap();
        assert_eq!(parsed, pub_key);

        // 3. Semicolon delimited token string
        let delimited = format!("{};sender_key_id", openssh_str);
        let parsed = parse_public_key(&delimited).unwrap();
        assert_eq!(parsed, pub_key);

        // 4. st: ConnectionToken string
        let token_str = simply_transfer_crypto::token::ConnectionToken::generate(
            "127.0.0.1",
            2222,
            &openssh_str,
            None,
        );
        let parsed = parse_public_key(&token_str).unwrap();
        assert_eq!(parsed, pub_key);
    }

    #[test]
    fn test_list_directory_entries() {
        let temp_dir = std::env::temp_dir().join(format!(
            "st_list_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(temp_dir.join("alpha_dir"));
        let _ = std::fs::write(temp_dir.join("beta_file.txt"), b"hello");
        let _ = std::fs::write(temp_dir.join(".hidden_file"), b"hidden");

        let output = list_directory_entries(&temp_dir.to_string_lossy());
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("D|alpha_dir|"));
        assert!(lines[1].starts_with("F|beta_file.txt|"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
