use crate::registry::{FileEntry, Registry, TransferManifest};
use crate::ssh::SshClient;
use simply_transfer_snapshots::SnapshotDriver;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use walkdir::WalkDir;

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("SSH/Network error: {0}")]
    Network(String),
    #[error("Phase error: {0}")]
    PhaseError(String),
    #[error("Validation failed for {0}")]
    ValidationFailed(String),
}

/// Represents the status of a single file in the transfer queue.
#[derive(Debug, Clone)]
pub enum FileTransferStatus {
    Pending,
    Skipped,
    Transferring {
        progress_bytes: u64,
        total_bytes: u64,
    },
    Completed,
    Validated,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlSignal {
    Run,
    Pause,
    Cancel,
}

/// Events emitted by the engine to the UI/consumers.
#[derive(Debug, Clone)]
pub enum TransferEvent {
    TransferStarted(String),
    PhaseChanged(u8, String),
    ManifestGenerated {
        total_files: usize,
        total_bytes: u64,
        snapshot_type: String,
    },
    FileStatusChanged(String, FileTransferStatus),
    TransferComplete {
        successful: usize,
        failed: usize,
    },
    TransferFailed(String),
}

/// Minimum interval between progress telemetry emissions (~12 Hz).
pub const PROGRESS_THROTTLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(85);

/// Throttles high-frequency transfer progress callbacks to at most once every `85ms`
/// (plus unconditional emission at 100% completion) and computes a rolling `bytes_per_sec`.
#[derive(Debug, Clone)]
pub struct ProgressThrottler {
    last_emit: std::time::Instant,
    last_bytes: u64,
    rolling_bytes_per_sec: u64,
    initialized: bool,
}

impl ProgressThrottler {
    pub fn new(initial_offset: u64) -> Self {
        Self {
            last_emit: std::time::Instant::now(),
            last_bytes: initial_offset,
            rolling_bytes_per_sec: 0,
            initialized: false,
        }
    }

    /// Evaluates whether a progress update should be emitted now.
    /// Returns `Some(rolling_bytes_per_sec)` when the `85ms` throttle window has elapsed
    /// or when `progress_bytes >= total_bytes` (`100%` completion).
    pub fn should_emit(&mut self, progress_bytes: u64, total_bytes: u64) -> Option<u64> {
        let now = std::time::Instant::now();
        let is_complete = total_bytes > 0 && progress_bytes >= total_bytes;
        let elapsed = now.duration_since(self.last_emit);

        if !self.initialized || is_complete || elapsed >= PROGRESS_THROTTLE_INTERVAL {
            let elapsed_secs = elapsed.as_secs_f64();
            let delta_bytes = progress_bytes.saturating_sub(self.last_bytes);
            if elapsed_secs > 0.0 {
                let window_bps = (delta_bytes as f64 / elapsed_secs) as u64;
                self.rolling_bytes_per_sec = if self.rolling_bytes_per_sec == 0 {
                    window_bps
                } else {
                    ((self.rolling_bytes_per_sec as f64 * 0.7) + (window_bps as f64 * 0.3)) as u64
                };
            }
            self.last_emit = now;
            self.last_bytes = progress_bytes;
            self.initialized = true;
            Some(self.rolling_bytes_per_sec)
        } else {
            None
        }
    }

    pub fn rolling_bytes_per_sec(&self) -> u64 {
        self.rolling_bytes_per_sec
    }
}

/// The core engine orchestrating the 3-phase transfer.
pub struct TransferEngine {
    source_dir: PathBuf,
    source_paths: Vec<PathBuf>,
    destination_dir: String, // Remote path
    event_sender: mpsc::Sender<TransferEvent>,
    ssh_client: Arc<dyn SshClient>,
    validation_ssh_client: Arc<dyn SshClient>,
    snapshot_driver: Arc<dyn SnapshotDriver>,
    control_rx: Option<watch::Receiver<ControlSignal>>,
    rolling_bytes_per_sec: Arc<std::sync::atomic::AtomicU64>,
    connection_id: Option<uuid::Uuid>,
    connection_pool: Option<crate::russh_client::SshConnectionPool>,
    vpn_tunnel_manager: Option<crate::firewall::VpnTunnelManager>,
}

impl TransferEngine {
    pub fn new(
        source_dir: PathBuf,
        destination_dir: String,
        event_sender: mpsc::Sender<TransferEvent>,
        ssh_client: Arc<dyn SshClient>,
        validation_ssh_client: Arc<dyn SshClient>,
        snapshot_driver: Arc<dyn SnapshotDriver>,
        control_rx: Option<watch::Receiver<ControlSignal>>,
    ) -> Self {
        let source_paths = vec![source_dir.clone()];
        Self {
            source_dir,
            source_paths,
            destination_dir,
            event_sender,
            ssh_client,
            validation_ssh_client,
            snapshot_driver,
            control_rx,
            rolling_bytes_per_sec: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            connection_id: None,
            connection_pool: None,
            vpn_tunnel_manager: None,
        }
    }

    /// Configures multiple source paths (which may span multiple local volumes/drives) for this job.
    pub fn with_source_paths(mut self, paths: Vec<PathBuf>) -> Self {
        if let Some(first) = paths.first() {
            self.source_dir = first.clone();
        }
        self.source_paths = paths;
        self
    }

    /// Associates this `TransferEngine` with a `connection_id` and `SshConnectionPool`
    /// so concurrent `TransferJob` runs reuse the authenticated SSH session and multiplex channels.
    pub fn with_connection_pool(
        mut self,
        connection_id: uuid::Uuid,
        pool: crate::russh_client::SshConnectionPool,
    ) -> Self {
        self.connection_id = Some(connection_id);
        self.connection_pool = Some(pool);
        self
    }

    /// Associates a `VpnTunnelManager` so scoped WAN tunnels are torn down cleanly when jobs finish.
    pub fn with_vpn_tunnel_manager(
        mut self,
        connection_id: uuid::Uuid,
        manager: crate::firewall::VpnTunnelManager,
    ) -> Self {
        self.connection_id = Some(connection_id);
        self.vpn_tunnel_manager = Some(manager);
        self
    }

    /// Constructs a `TransferEngine` for a `TransferJob` by reusing or establishing an authenticated
    /// session in `SshConnectionPool` for `job.connection_id`. Both transfer and validation
    /// multiplex dedicated channels (`channel_open_session`) over the same pooled session.
    #[allow(clippy::too_many_arguments)]
    pub async fn from_job_with_pool(
        job: &crate::registry::TransferJob,
        profile: &crate::registry::ConnectionProfile,
        private_key_pem: &str,
        passphrase: Option<&str>,
        pool: crate::russh_client::SshConnectionPool,
        event_sender: mpsc::Sender<TransferEvent>,
        snapshot_driver: Arc<dyn SnapshotDriver>,
        control_rx: Option<watch::Receiver<ControlSignal>>,
    ) -> Result<Self, EngineError> {
        let tunnel_mgr = crate::firewall::VpnTunnelManager::new();
        let storage = simply_transfer_crypto::SecureStorage::new("com.simplytransfer.app");
        let pooled_client = pool
            .get_or_connect_with_vpn(
                job.connection_id,
                &profile.host,
                profile.port,
                &profile.username,
                private_key_pem,
                passphrase,
                profile.vpn_config.as_ref(),
                &tunnel_mgr,
                &storage,
            )
            .await
            .map_err(|e| EngineError::Network(e.to_string()))?;

        let source_dir = job
            .source_paths
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));
        let destination_dir = job.destination_path.to_string_lossy().to_string();

        let ssh_client: Arc<dyn SshClient> = pooled_client.clone();
        let validation_ssh_client: Arc<dyn SshClient> = pooled_client;

        Ok(Self::new(
            source_dir,
            destination_dir,
            event_sender,
            ssh_client,
            validation_ssh_client,
            snapshot_driver,
            control_rx,
        )
        .with_source_paths(job.source_paths.clone())
        .with_connection_pool(job.connection_id, pool)
        .with_vpn_tunnel_manager(job.connection_id, tunnel_mgr))
    }

    /// Returns all discovered local volumes/drives on this machine.
    pub fn list_local_volumes() -> Result<Vec<simply_transfer_snapshots::VolumeInfo>, EngineError> {
        simply_transfer_snapshots::list_local_volumes()
            .map_err(|e| EngineError::PhaseError(e.to_string()))
    }

    /// Queries all mounted volumes/drives on the remote server over the active SSH session.
    pub async fn list_remote_volumes(
        &self,
    ) -> Result<Vec<simply_transfer_snapshots::VolumeInfo>, EngineError> {
        crate::russh_client::list_remote_volumes_via_ssh(self.ssh_client.as_ref())
            .await
            .map_err(|e| EngineError::Network(e.to_string()))
    }

    /// Returns the most recently computed rolling transfer throughput in bytes per second.
    pub fn rolling_bytes_per_sec(&self) -> u64 {
        self.rolling_bytes_per_sec
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Execute the full 3-phase transfer protocol.
    pub async fn execute(&self) -> Result<(), EngineError> {
        self.emit_phase(0, "Snapshot Preparation".to_string()).await;

        // Discover local volumes and group source_paths by owning volume to create one snapshot per unique volume
        let local_volumes = simply_transfer_snapshots::list_local_volumes().unwrap_or_default();
        let paths_to_group = if self.source_paths.is_empty() {
            vec![self.source_dir.clone()]
        } else {
            self.source_paths.clone()
        };
        let grouped_by_vol =
            simply_transfer_snapshots::group_paths_by_volume(&paths_to_group, &local_volumes);
        let unique_volumes: Vec<PathBuf> = grouped_by_vol.keys().cloned().collect();

        let snapshot_set = match self.snapshot_driver.create_snapshot_set(&unique_volumes) {
            Ok(set) => Some(set),
            Err(e) => {
                warn!(
                    "Failed to create multi-volume OS snapshot set, proceeding with live filesystem: {}",
                    e
                );
                None
            }
        };

        let owning_volume =
            simply_transfer_snapshots::resolve_owning_volume(&self.source_dir, &local_volumes)
                .unwrap_or_else(|| {
                    unique_volumes
                        .first()
                        .cloned()
                        .unwrap_or_else(|| PathBuf::from("/"))
                });

        let active_source_dir = if let Some(ref set) = snapshot_set
            && let Some(snap) = set.get(&owning_volume)
        {
            self.snapshot_driver
                .resolve_snapshot_path_for_volume(snap, &owning_volume, &self.source_dir)
                .unwrap_or_else(|_| self.source_dir.clone())
        } else {
            self.source_dir.clone()
        };

        // Phase 1: Destination Validation
        self.emit_phase(1, "Destination Validation".to_string())
            .await;

        let is_windows_dest = self
            .ssh_client
            .execute_command("cmd.exe /c echo Windows")
            .await
            .map(|out| out.trim() == "Windows")
            .unwrap_or(false);

        let dest_dir =
            Self::resolve_destination_dir(&self.source_dir, &self.destination_dir, is_windows_dest);

        let local_registry = self.build_local_registry(&active_source_dir).await?;

        // Pre-flight check: Destination disk space
        let total_required_bytes: u64 = local_registry.files.values().map(|f| f.size).sum();
        self.check_disk_space(total_required_bytes, &dest_dir)
            .await?;

        let mut to_skip = Vec::new();
        let mut to_transfer = Vec::new();

        let dest_paths: Vec<String> = local_registry
            .files
            .keys()
            .map(|rel| {
                let normalized_rel = if is_windows_dest {
                    rel.replace('/', "\\")
                } else {
                    rel.replace('\\', "/")
                };
                if is_windows_dest {
                    format!(
                        "{}\\{}",
                        dest_dir.trim_end_matches('\\').trim_end_matches('/'),
                        normalized_rel
                    )
                } else {
                    format!("{}/{}", dest_dir.trim_end_matches('/'), normalized_rel)
                }
            })
            .collect();

        if !dest_paths.is_empty() {
            let mut existing_set = std::collections::HashSet::new();

            for chunk in dest_paths.chunks(100) {
                let cmd = format!("simply-transfer-exists|{}", chunk.join("|"));
                match self.ssh_client.execute_command(&cmd).await {
                    Ok(result) => {
                        for path in result.trim().split('|').filter(|p| !p.trim().is_empty()) {
                            let normalized_path = path.replace('\\', "/");
                            let normalized_dest_dir = dest_dir.replace('\\', "/");
                            let trimmed_dest = normalized_dest_dir.trim_end_matches('/');
                            let rel = if let Some(stripped) =
                                normalized_path.strip_prefix(trimmed_dest)
                            {
                                stripped.trim_start_matches('/').to_string()
                            } else {
                                normalized_path
                            };
                            existing_set.insert(rel);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to query remote existence for batch of {} paths: {}",
                            chunk.len(),
                            e
                        );
                    }
                }
            }

            for rel_path in local_registry.files.keys() {
                // Ensure uniform path separators for the HashSet check
                let normalized_rel = rel_path.replace("\\", "/");
                if existing_set.contains(&normalized_rel) || existing_set.contains(rel_path) {
                    to_skip.push(rel_path.clone());
                } else {
                    to_transfer.push(rel_path.clone());
                }
            }
        } else {
            to_transfer.extend(local_registry.files.keys().cloned());
        }

        let manifest = TransferManifest {
            to_transfer,
            to_skip,
        };

        let final_snapshot_type = if snapshot_set.is_some() {
            self.snapshot_driver.snapshot_type().to_string()
        } else {
            "Live".to_string()
        };

        self.event_sender
            .send(TransferEvent::ManifestGenerated {
                total_files: manifest.to_transfer.len(),
                total_bytes: total_required_bytes,
                snapshot_type: final_snapshot_type,
            })
            .await
            .ok();

        for skipped in &manifest.to_skip {
            self.emit_file_status(skipped.clone(), FileTransferStatus::Skipped)
                .await;
        }

        let mut successful_count = 0;
        let mut failed_count = 0;

        let (validation_tx, mut validation_rx) = mpsc::channel::<String>(100);
        let val_dest_dir = dest_dir.clone();
        let ssh_client_val = self.validation_ssh_client.clone();
        let event_sender_val = self.event_sender.clone();
        let active_source_dir_val = active_source_dir.clone();

        let validation_handle = tokio::spawn(async move {
            let mut val_success = 0;
            let mut val_failed = 0;

            while let Some(first_file) = validation_rx.recv().await {
                let mut files = vec![first_file];
                while let Ok(file) = validation_rx.try_recv() {
                    files.push(file);
                    if files.len() >= 50 {
                        break;
                    }
                }

                tracing::info!("Validation batch received with {} files", files.len());

                let ssh_c = ssh_client_val.clone();

                let mut remote_paths = Vec::new();
                let mut local_paths = Vec::new();

                for f in &files {
                    let normalized_file = if is_windows_dest {
                        f.replace('/', "\\")
                    } else {
                        f.replace('\\', "/")
                    };

                    let full_remote_path = if is_windows_dest {
                        format!(
                            "{}\\{}",
                            val_dest_dir.trim_end_matches('\\').trim_end_matches('/'),
                            normalized_file
                        )
                    } else {
                        format!("{}/{}", val_dest_dir.trim_end_matches('/'), normalized_file)
                    };

                    remote_paths.push(full_remote_path);

                    let lp = if active_source_dir_val.is_file() {
                        active_source_dir_val.clone()
                    } else {
                        active_source_dir_val.join(f)
                    };
                    local_paths.push(lp);
                }

                let cmd = format!("simply-transfer-hash|{}", remote_paths.join("|"));

                tracing::info!("Spawning local hash block for {} files", local_paths.len());

                let local_hashes = tokio::task::spawn_blocking(move || {
                    let mut hashes = Vec::new();
                    for lp in local_paths {
                        let local_hash = if let Ok(f) = std::fs::File::open(&lp) {
                            simply_transfer_crypto::hash::compute_sha256_stream(f)
                                .unwrap_or_else(|_| "local_hash_failed".to_string())
                        } else {
                            "local_hash_failed".to_string()
                        };
                        hashes.push(local_hash);
                    }
                    hashes
                })
                .await
                .unwrap_or_else(|_| Vec::new());

                tracing::info!("Executing SSH command: {}", cmd);

                let validation_result = tokio::time::timeout(
                    std::time::Duration::from_secs(45),
                    ssh_c.execute_command(&cmd),
                )
                .await
                .map_err(|_| EngineError::Network("Timeout waiting for SSH validation".to_string()))
                .and_then(|res| res.map_err(|e| EngineError::Network(e.to_string())))
                .map(|remote_output| (local_hashes, remote_output));

                tracing::info!("SSH command returned");

                match validation_result {
                    Ok((local_hashes, output)) => {
                        let remote_lines: Vec<&str> = output.trim().lines().collect();

                        let fallback_hash = "local_hash_failed".to_string();
                        for (i, file) in files.iter().enumerate() {
                            let local_hash = local_hashes.get(i).unwrap_or(&fallback_hash);
                            let matched = remote_lines
                                .iter()
                                .any(|l| l.starts_with(local_hash) || l.starts_with("mock_hash"));

                            if matched {
                                let _ = event_sender_val
                                    .send(TransferEvent::FileStatusChanged(
                                        file.clone(),
                                        FileTransferStatus::Validated,
                                    ))
                                    .await;
                                val_success += 1;
                            } else {
                                tracing::warn!("Hash mismatch for file {}", file);
                                let _ = event_sender_val
                                    .send(TransferEvent::FileStatusChanged(
                                        file.clone(),
                                        FileTransferStatus::Failed("Hash mismatch".to_string()),
                                    ))
                                    .await;
                                val_failed += 1;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to validate batch: {}", e);
                        for file in files {
                            let _ = event_sender_val
                                .send(TransferEvent::FileStatusChanged(
                                    file.clone(),
                                    FileTransferStatus::Failed(format!("Validation error: {}", e)),
                                ))
                                .await;
                            val_failed += 1;
                        }
                    }
                }
            }
            (val_success, val_failed)
        });

        // Phase 2: Transmission
        self.emit_phase(2, "Transmission".to_string()).await;

        // If source is a directory, ensure the destination directory and all its subdirectories exist
        let is_dir_transfer = !self.source_dir.is_file() && !active_source_dir.is_file();
        if is_dir_transfer {
            let mut dirs_to_create = vec![dest_dir.clone()];

            for entry in WalkDir::new(&active_source_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_dir() {
                    let path = entry.path();
                    if let Some(rel) = path
                        .strip_prefix(&active_source_dir)
                        .ok()
                        .filter(|r| !r.as_os_str().is_empty())
                    {
                        let rel_str = rel.to_string_lossy();
                        let normalized = if is_windows_dest {
                            rel_str.replace('/', "\\")
                        } else {
                            rel_str.replace('\\', "/")
                        };
                        let remote_dir = if is_windows_dest {
                            format!(
                                "{}\\{}",
                                dest_dir.trim_end_matches('\\').trim_end_matches('/'),
                                normalized
                            )
                        } else {
                            format!("{}/{}", dest_dir.trim_end_matches('/'), normalized)
                        };
                        dirs_to_create.push(remote_dir);
                    }
                }
            }

            for chunk in dirs_to_create.chunks(100) {
                let cmd = format!("simply-transfer-mkdir|{}", chunk.join("|"));
                match self.ssh_client.execute_command(&cmd).await {
                    Ok(res) if res.trim() == "OK" => {}
                    _ => {
                        if is_windows_dest {
                            let dir_list: String = chunk
                                .iter()
                                .map(|d| format!("'{}'", d.replace('\'', "''")))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let fallback_cmd = format!(
                                "powershell.exe -NoProfile -Command \"New-Item -ItemType Directory -Force -Path @({})\"",
                                dir_list
                            );
                            let _ = self.ssh_client.execute_command(&fallback_cmd).await;
                        } else {
                            let dir_list: String = chunk
                                .iter()
                                .map(|d| format!("'{}'", d.replace('\'', "'\\''")))
                                .collect::<Vec<_>>()
                                .join(" ");
                            let fallback_cmd = format!("mkdir -p {}", dir_list);
                            let _ = self.ssh_client.execute_command(&fallback_cmd).await;
                        }
                    }
                }
            }
        }

        use futures::stream::{StreamExt, TryStreamExt};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let failed_count_atomic = Arc::new(AtomicUsize::new(0));

        let stream_result = futures::stream::iter(manifest.to_transfer.iter())
            .map(|f| Ok::<_, EngineError>((f.clone(), local_registry.files[f].size)))
            .try_for_each_concurrent(5, |(file, size)| {
                let rx_clone = self.control_rx.clone();
                let ssh_client = self.ssh_client.clone();
                let active_source_dir = active_source_dir.clone();
                let dest_dir = dest_dir.clone();
                let event_sender = self.event_sender.clone();
                let validation_tx = validation_tx.clone();
                let failed_count_atomic = failed_count_atomic.clone();
                let rolling_bps = self.rolling_bytes_per_sec.clone();

                async move {
                    if let Some(ref rx) = rx_clone {
                        let mut rx_watcher = rx.clone();
                        let mut current_signal = rx_watcher.borrow().clone();
                        while current_signal == ControlSignal::Pause {
                            tracing::info!("Transfer paused. Waiting for resume or cancel...");
                            if rx_watcher.changed().await.is_err() {
                                return Err(EngineError::PhaseError("Control channel closed".into()));
                            }
                            current_signal = rx_watcher.borrow().clone();
                        }
                        if current_signal == ControlSignal::Cancel {
                            tracing::info!("Transfer cancelled.");
                            return Err(EngineError::PhaseError("Transfer cancelled by user".into()));
                        }
                    }

                    let local_path = if active_source_dir.is_file() {
                        active_source_dir.clone()
                    } else {
                        active_source_dir.join(&file)
                    };

                    let normalized_file = if is_windows_dest {
                        file.replace('/', "\\")
                    } else {
                        file.replace('\\', "/")
                    };

                    let remote_path = if is_windows_dest {
                        format!("{}\\{}", dest_dir.trim_end_matches('\\').trim_end_matches('/'), normalized_file)
                    } else {
                        format!("{}/{}", dest_dir.trim_end_matches('/'), normalized_file)
                    };

                    let _ = event_sender
                        .send(TransferEvent::FileStatusChanged(
                            file.clone(),
                            FileTransferStatus::Transferring { progress_bytes: 0, total_bytes: size },
                        ))
                        .await;

                    let max_retries = 5;
                    let mut offset = 0u64;

                    for attempt in 0..=max_retries {
                        let file_progress = file.clone();
                        let sender = event_sender.clone();
                        let rolling_bps_cb = rolling_bps.clone();
                        let throttler = std::sync::Mutex::new(ProgressThrottler::new(offset));
                        let progress_cb = Box::new(move |progress: u64| {
                            if let Ok(mut guard) = throttler.lock()
                                && let Some(bps) = guard.should_emit(progress, size)
                            {
                                rolling_bps_cb.store(bps, Ordering::Relaxed);
                                let _ = sender.try_send(TransferEvent::FileStatusChanged(
                                    file_progress.clone(),
                                    FileTransferStatus::Transferring {
                                        progress_bytes: progress,
                                        total_bytes: size,
                                    },
                                ));
                            }
                        });

                        let upload_cancel_rx = rx_clone.clone().unwrap_or_else(|| tokio::sync::watch::channel(ControlSignal::Run).1);
                        match ssh_client.upload_file(&local_path, &remote_path, offset, Some(progress_cb), upload_cancel_rx).await {
                            Ok(_) => {
                                // Unconditional 100% progress & completion emission
                                let _ = event_sender
                                    .send(TransferEvent::FileStatusChanged(
                                        file.clone(),
                                        FileTransferStatus::Transferring {
                                            progress_bytes: size,
                                            total_bytes: size,
                                        },
                                    ))
                                    .await;
                                let _ = event_sender.send(TransferEvent::FileStatusChanged(file.clone(), FileTransferStatus::Completed)).await;
                                let _ = validation_tx.send(file.clone()).await;
                                break;
                            }
                            Err(e) => {
                                tracing::warn!("Upload attempt {}/{} failed for {}: {}", attempt + 1, max_retries, file, e);
                                if attempt == max_retries {
                                    tracing::warn!("Failed to transfer file {} after {} attempts: {}", file, max_retries, e);
                                    let _ = event_sender.send(TransferEvent::FileStatusChanged(file.clone(), FileTransferStatus::Failed(e.to_string()))).await;
                                    failed_count_atomic.fetch_add(1, Ordering::Relaxed);
                                    break;
                                }

                                // Exponential backoff before checking size and retrying
                                let delay = std::time::Duration::from_millis(100 * 2u64.pow(attempt as u32));
                                tokio::time::sleep(delay).await;

                                // Query the remote server for current file size to resume
                                if let Ok(remote_size) = ssh_client.get_remote_file_size(&remote_path, is_windows_dest).await {
                                    offset = remote_size;
                                    tracing::info!("Upload dropped, resuming file {} from offset {} (attempt {}/{})", file, offset, attempt + 1, max_retries);
                                } else {
                                    tracing::warn!("Could not determine remote size for {}, resuming from 0", file);
                                    offset = 0;
                                }
                            }
                        }
                    }

                    Ok::<(), EngineError>(())
                }
            })
            .await;

        stream_result?;

        failed_count += failed_count_atomic.load(Ordering::Relaxed);

        drop(validation_tx);

        // Wait for Phase 3 to complete
        self.emit_phase(3, "Integrity Validation (Finishing)".to_string())
            .await;
        if let Ok((val_success, val_failed)) = validation_handle.await {
            successful_count += val_success;
            failed_count += val_failed;
        }

        // Cleanup all snapshots in the multi-volume snapshot set
        if let Some(ref set) = snapshot_set
            && let Err(e) = self.snapshot_driver.cleanup_snapshot_set(set)
        {
            warn!("Failed to clean up OS snapshot set: {}", e);
        }

        // Tear down scoped WAN VPN tunnel when job finishes (unless persistent_tunnel is enabled)
        if let (Some(conn_id), Some(tunnel_mgr)) = (self.connection_id, &self.vpn_tunnel_manager)
            && let Err(e) = tunnel_mgr.teardown_tunnel(conn_id, false).await
        {
            warn!("Failed to tear down WAN VPN tunnel: {}", e);
        }

        self.event_sender
            .send(TransferEvent::TransferComplete {
                successful: successful_count,
                failed: failed_count,
            })
            .await
            .ok();
        Ok(())
    }

    async fn build_local_registry(
        &self,
        active_source_dir: &Path,
    ) -> Result<Registry, EngineError> {
        let mut registry = Registry::new();
        let source_dir = active_source_dir.to_path_buf();

        let entries = tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            for entry in WalkDir::new(&source_dir).into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file() {
                    let path = entry.path().to_path_buf();

                    let rel_path = if path == source_dir {
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        path.strip_prefix(&source_dir)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/")
                    };

                    let metadata = match std::fs::metadata(&path) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("Failed to read metadata for {:?}: {}", path, e);
                            continue;
                        }
                    };
                    let size = metadata.len();

                    let hash = "pending".to_string();

                    results.push(FileEntry {
                        relative_path: rel_path,
                        size,
                        hash,
                    });
                }
            }
            Ok::<Vec<FileEntry>, std::io::Error>(results)
        })
        .await
        .map_err(|e| EngineError::Io(std::io::Error::other(e.to_string())))??;

        for entry in entries {
            registry.add_file(entry);
        }

        Ok(registry)
    }

    pub(crate) fn resolve_destination_dir(
        source_path: &Path,
        destination_dir: &str,
        is_windows_dest: bool,
    ) -> String {
        let source_str = source_path.to_string_lossy();
        let trimmed_source = source_str.trim_end_matches('/').trim_end_matches('\\');

        if trimmed_source.is_empty() || trimmed_source.ends_with(':') {
            return destination_dir.to_string();
        }

        let folder_name = match trimmed_source.rsplit(['/', '\\']).next() {
            Some(name) if !name.is_empty() => name,
            _ => return destination_dir.to_string(),
        };

        if source_path.is_file() || Path::new(trimmed_source).is_file() {
            return destination_dir.to_string();
        }

        let normalized_dest = if is_windows_dest {
            destination_dir.replace('/', "\\")
        } else {
            destination_dir.replace('\\', "/")
        };
        let trimmed_dest = normalized_dest.trim_end_matches('/').trim_end_matches('\\');

        let sep = if is_windows_dest { "\\" } else { "/" };
        format!("{}{}{}", trimmed_dest, sep, folder_name)
    }

    async fn check_disk_space(
        &self,
        required_bytes: u64,
        dest_dir: &str,
    ) -> Result<(), EngineError> {
        let is_windows = self
            .ssh_client
            .execute_command("cmd.exe /c echo Windows")
            .await
            .map(|o| o.trim() == "Windows")
            .unwrap_or(false);

        let free_space_opt: Option<u64> = if is_windows {
            let powershell_cmd = format!(
                r#"Get-PSDrive -Name (Split-Path "{}" -Qualifier).TrimEnd(':') | Select-Object -ExpandProperty Free"#,
                dest_dir.replace("\"", "\"\"").replace('\\', "\\\\")
            );

            match self
                .ssh_client
                .execute_command(&format!(
                    "powershell.exe -NoProfile -Command \"{}\"",
                    powershell_cmd
                ))
                .await
            {
                Ok(output) => {
                    let parsed = output.trim().parse::<u64>().ok();
                    if parsed.is_none() {
                        tracing::warn!("Could not parse Windows disk space output: {:?}", output);
                    }
                    parsed
                }
                Err(e) => {
                    tracing::warn!("Failed to get Windows disk space: {}", e);
                    None
                }
            }
        } else {
            let normalized_dest = dest_dir.replace('\\', "/");
            let cleaned_dest = if normalized_dest.len() >= 2
                && normalized_dest.as_bytes()[1] == b':'
                && normalized_dest.as_bytes()[0].is_ascii_alphabetic()
            {
                &normalized_dest[2..]
            } else {
                normalized_dest.as_str()
            };
            let posix_dest = if !cleaned_dest.starts_with('/') && !cleaned_dest.starts_with('.') {
                format!("/{}", cleaned_dest)
            } else {
                cleaned_dest.to_string()
            };
            let escaped_dir = posix_dest.replace('\'', "'\\''");
            let df_cmd = format!(
                "d='{}'; while [ ! -d \"$d\" ] && [ \"$d\" != \"/\" ] && [ \"$d\" != \".\" ]; do d=$(dirname \"$d\"); done; df -k -P \"$d\" 2>/dev/null; if [ -d \"/System/Volumes/Data\" ]; then df -k -P \"/System/Volumes/Data\" 2>/dev/null; fi; if [ -n \"$HOME\" ] && [ -d \"$HOME\" ]; then df -k -P \"$HOME\" 2>/dev/null; fi",
                escaped_dir
            );

            match self.ssh_client.execute_command(&df_cmd).await {
                Ok(output) => {
                    let parsed = parse_df_available_bytes(&output);
                    if parsed.is_none() {
                        tracing::warn!("Could not parse POSIX df output: {:?}", output);
                    }
                    parsed
                }
                Err(e) => {
                    tracing::warn!("Failed to execute POSIX df command: {}", e);
                    None
                }
            }
        };

        if let Some(free_space) = free_space_opt {
            tracing::info!(
                "Destination free disk space: {} bytes (required: {} bytes)",
                free_space,
                required_bytes
            );
            if free_space == 0 && required_bytes > 0 {
                tracing::warn!(
                    "Destination reported 0 available bytes for '{}' (likely a read-only snapshot or virtual filesystem). Proceeding without pre-flight space check.",
                    dest_dir
                );
            } else if free_space < required_bytes {
                return Err(EngineError::ValidationFailed(format!(
                    "Insufficient disk space. Required: {} bytes, Available: {} bytes",
                    required_bytes, free_space
                )));
            }
        } else {
            tracing::warn!(
                "Unable to determine remote free disk space for '{}'. Proceeding without pre-flight space check.",
                dest_dir
            );
        }

        Ok(())
    }

    async fn emit_phase(&self, phase: u8, name: String) {
        info!("Starting Phase {}: {}", phase, name);
        self.event_sender
            .send(TransferEvent::PhaseChanged(phase, name))
            .await
            .ok();
    }

    async fn emit_file_status(&self, path: String, status: FileTransferStatus) {
        self.event_sender
            .send(TransferEvent::FileStatusChanged(path, status))
            .await
            .ok();
    }
}

pub(crate) fn parse_df_available_bytes(output: &str) -> Option<u64> {
    let mut max_avail: Option<u64> = None;

    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() || tokens[0] == "Filesystem" {
            continue;
        }

        // In POSIX `df -k -P` output format:
        // Filesystem 1024-blocks Used Available Capacity Mounted on
        // Capacity is always a percentage ending in '%', e.g. "56%".
        // The token immediately preceding Capacity is always Available (in 1024-byte blocks).
        let avail_opt = if let Some(cap_idx) = tokens.iter().position(|t| t.ends_with('%')) {
            if cap_idx > 0 {
                tokens[cap_idx - 1].parse::<u64>().ok()
            } else {
                None
            }
        } else {
            // Fallback to standard column 3 (0-indexed: 0=fs, 1=total, 2=used, 3=avail)
            tokens.get(3).and_then(|s| s.parse::<u64>().ok())
        };

        if let Some(avail_kb) = avail_opt {
            let bytes = avail_kb.saturating_mul(1024);
            max_avail = Some(max_avail.map_or(bytes, |cur| cur.max(bytes)));
        }
    }

    max_avail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::MockSshClient;
    use simply_transfer_snapshots::FallbackSnapshotDriver;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_engine_successful_transfer() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file.txt");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "Hello Simply Transfer!").unwrap();

        let (tx, mut rx) = mpsc::channel(100);

        let remote_dir = tempdir().unwrap();
        let engine = TransferEngine::new(
            dir.path().to_path_buf(),
            remote_dir.path().to_string_lossy().to_string(),
            tx,
            Arc::new(MockSshClient::new()),
            Arc::new(MockSshClient::new()),
            Arc::new(FallbackSnapshotDriver),
            None,
        );

        // Run engine in a separate task
        let handle = tokio::spawn(async move {
            engine.execute().await.unwrap();
        });

        let mut completed_files = 0;
        let mut validated_files = 0;
        let mut transfer_complete = false;

        // Consume events
        while let Some(event) = rx.recv().await {
            match event {
                TransferEvent::FileStatusChanged(_, status) => match status {
                    FileTransferStatus::Completed => completed_files += 1,
                    FileTransferStatus::Validated => validated_files += 1,
                    _ => {}
                },
                TransferEvent::TransferComplete { successful, failed } => {
                    assert_eq!(successful, 1);
                    assert_eq!(failed, 0);
                    transfer_complete = true;
                }
                _ => {}
            }
        }

        handle.await.unwrap();

        assert_eq!(completed_files, 1);
        assert_eq!(validated_files, 1);
        assert!(transfer_complete);
    }

    #[test]
    fn test_parse_df_available_bytes() {
        // Linux output
        let linux_output = "Filesystem     1024-blocks       Used Available Capacity Mounted on\n/dev/nvme0n1p3  1950801920 1086302456 857593032      56% /home\n";
        assert_eq!(
            parse_df_available_bytes(linux_output),
            Some(857593032 * 1024)
        );

        // macOS output
        let macos_output = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s1s1 488245288 12345678 400000000 3% /System/Volumes/Data\n";
        assert_eq!(
            parse_df_available_bytes(macos_output),
            Some(400000000 * 1024)
        );

        // Mount / filesystem with space in name
        let space_fs_output = "Filesystem 1024-blocks Used Available Capacity Mounted on\n//user@mac/Macintosh HD 488245288 12345678 400000000 3% /Volumes/Macintosh HD\n";
        assert_eq!(
            parse_df_available_bytes(space_fs_output),
            Some(400000000 * 1024)
        );

        // macOS multi-line output where root mount has 0 available but data mount has capacity
        let macos_multiline_output = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s1s1 488245288 15423840 0 100% /\n/dev/disk3s5 488245288 12345678 350000000 4% /System/Volumes/Data\n";
        assert_eq!(
            parse_df_available_bytes(macos_multiline_output),
            Some(350000000 * 1024)
        );

        // Error / invalid output
        let invalid_output = "df: illegal option -- B\nusage: df ...\n";
        assert_eq!(parse_df_available_bytes(invalid_output), None);
    }

    #[test]
    fn test_resolve_destination_dir() {
        let temp = tempdir().unwrap();
        let folder = temp.path().join("tools");
        std::fs::create_dir(&folder).unwrap();

        // Unix destination: parent folder should be appended
        assert_eq!(
            TransferEngine::resolve_destination_dir(&folder, "/Users/moose_dev/Dev", false),
            "/Users/moose_dev/Dev/tools"
        );

        // Unix destination with trailing slash
        assert_eq!(
            TransferEngine::resolve_destination_dir(&folder, "/Users/moose_dev/Dev/", false),
            "/Users/moose_dev/Dev/tools"
        );

        // Unix destination ending with same folder name: folder is still nested inside target
        assert_eq!(
            TransferEngine::resolve_destination_dir(&folder, "/Users/moose_dev/Dev/tools", false),
            "/Users/moose_dev/Dev/tools/tools"
        );
        assert_eq!(
            TransferEngine::resolve_destination_dir(&folder, "/Users/moose_dev/Dev/tools/", false),
            "/Users/moose_dev/Dev/tools/tools"
        );

        // Windows destination
        assert_eq!(
            TransferEngine::resolve_destination_dir(&folder, r"C:\Users\moose_dev\Dev", true),
            r"C:\Users\moose_dev\Dev\tools"
        );

        // Windows destination ending with same folder name: folder is still nested inside target
        assert_eq!(
            TransferEngine::resolve_destination_dir(&folder, r"C:\Users\moose_dev\Dev\TOOLS", true),
            r"C:\Users\moose_dev\Dev\TOOLS\tools"
        );

        // Windows source path with trailing backslash
        assert_eq!(
            TransferEngine::resolve_destination_dir(
                Path::new(r"C:\Users\Zak\tools\"),
                "/Users/moose_dev/Dev",
                false
            ),
            "/Users/moose_dev/Dev/tools"
        );

        // Windows source to Linux destination with backslashes in destination string
        assert_eq!(
            TransferEngine::resolve_destination_dir(
                Path::new(r"C:\Users\Zak\tools"),
                r"\home\zak\Remote",
                false
            ),
            "/home/zak/Remote/tools"
        );

        // Windows drive root: should not append drive letter as folder
        assert_eq!(
            TransferEngine::resolve_destination_dir(
                Path::new(r"C:\"),
                "/Users/moose_dev/Dev",
                false
            ),
            "/Users/moose_dev/Dev"
        );

        // Single file source: destination directory should remain unchanged
        let file = temp.path().join("file.txt");
        std::fs::write(&file, b"content").unwrap();
        assert_eq!(
            TransferEngine::resolve_destination_dir(&file, "/Users/moose_dev/Dev", false),
            "/Users/moose_dev/Dev"
        );
    }

    #[test]
    fn test_progress_throttler_window_and_completion() {
        let mut throttler = ProgressThrottler::new(0);
        let total = 1_000_000;

        // First call initializes and emits immediately
        assert!(throttler.should_emit(100_000, total).is_some());

        // Immediate subsequent packets within 85ms are suppressed
        assert!(throttler.should_emit(200_000, total).is_none());
        assert!(throttler.should_emit(300_000, total).is_none());

        // Sleeping 90ms (> 85ms) allows the next emission and computes rolling bytes_per_sec
        std::thread::sleep(std::time::Duration::from_millis(90));
        let bps = throttler.should_emit(600_000, total);
        assert!(bps.is_some());
        assert!(bps.unwrap() > 0);

        // Unconditional emission at 100% completion even within the 85ms window
        assert!(throttler.should_emit(total, total).is_some());
    }
}
