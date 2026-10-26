use serde::{Deserialize, Serialize};
use simply_transfer_crypto::VpnConfig;
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// Authentication configuration for a remote connection profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AuthConfig {
    /// Key identifier in SecureStorage or path to SSH private key.
    pub key_id: String,
    /// Optional expected server public key fingerprint for strict host verification.
    pub expected_host_key: Option<String>,
}

/// Decoupled connection profile storing credentials, host settings, and optional WAN VPN config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionProfile {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_config: AuthConfig,
    pub vpn_config: Option<VpnConfig>,
}

/// Type of transfer job executed over a ConnectionProfile.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TransferJobType {
    #[default]
    Sync,
    Backup,
}

/// Schedule configuration for recurring or manual transfer jobs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleConfig {
    /// E.g., "Single Transfer", "Continuous Sync", "Scheduled Transfer", "Manual", "Hourly", "Daily".
    pub mode: String,
    /// Optional interval in seconds for recurring jobs.
    pub interval_secs: Option<u64>,
    /// Optional window start (e.g., "09:00").
    pub window_start: Option<String>,
    /// Optional window end (e.g., "17:00").
    pub window_end: Option<String>,
    /// Optional sync frequency string (e.g., "Every 15 Minutes", "Every Hour", "Daily", "Weekly").
    #[serde(default)]
    pub sync_frequency: Option<String>,
    /// Optional active days string (e.g., "MTWThF") for scheduled transfers.
    #[serde(default)]
    pub active_days: Option<String>,
}

/// Represents a discrete transfer job bound to a specific `ConnectionProfile` (`1:N`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferJob {
    pub job_id: Uuid,
    pub connection_id: Uuid,
    pub name: String,
    pub job_type: TransferJobType,
    pub source_paths: Vec<PathBuf>,
    pub destination_path: PathBuf,
    /// Full list of (source, destination) row pairs for multi-row transfer setups.
    #[serde(default)]
    pub mappings: Vec<(PathBuf, PathBuf)>,
    pub schedule: Option<ScheduleConfig>,
    pub use_snapshots: bool,
}

/// Represents a single file in the transfer registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    /// The normalized relative path of the file.
    pub relative_path: String,
    /// The size of the file in bytes.
    pub size: u64,
    /// The SHA-256 hash of the file.
    pub hash: String,
}

/// The registry sent from the source to the destination in Phase 1, and managing connections & jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    pub files: HashMap<String, FileEntry>,
    #[serde(default)]
    pub connections: HashMap<Uuid, ConnectionProfile>,
    #[serde(default)]
    pub jobs: HashMap<Uuid, TransferJob>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            connections: HashMap::new(),
            jobs: HashMap::new(),
        }
    }

    pub fn add_file(&mut self, entry: FileEntry) {
        self.files.insert(entry.relative_path.clone(), entry);
    }

    pub fn upsert_connection(&mut self, profile: ConnectionProfile) {
        self.connections.insert(profile.id, profile);
    }

    pub fn get_connection(&self, conn_id: Uuid) -> Option<&ConnectionProfile> {
        self.connections.get(&conn_id)
    }

    pub fn remove_connection(&mut self, conn_id: Uuid) -> Option<ConnectionProfile> {
        self.jobs.retain(|_, job| job.connection_id != conn_id);
        self.connections.remove(&conn_id)
    }

    pub fn get_jobs_for_connection(&self, conn_id: Uuid) -> Vec<TransferJob> {
        let mut jobs: Vec<TransferJob> = self
            .jobs
            .values()
            .filter(|job| job.connection_id == conn_id)
            .cloned()
            .collect();
        jobs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.job_id.cmp(&b.job_id)));
        jobs
    }

    pub fn upsert_job(&mut self, job: TransferJob) {
        self.jobs.insert(job.job_id, job);
    }

    pub fn remove_job(&mut self, job_id: Uuid) -> Option<TransferJob> {
        self.jobs.remove(&job_id)
    }
}

/// The manifest returned by the destination detailing which files to transfer or skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferManifest {
    /// Files that are missing or differ and must be transferred.
    pub to_transfer: Vec<String>,
    /// Files that are identical and can be skipped.
    pub to_skip: Vec<String>,
}

impl TransferManifest {
    /// Compare a source registry against a destination registry to generate a manifest.
    pub fn compare(source: &Registry, destination: &Registry) -> Self {
        let mut to_transfer = Vec::new();
        let mut to_skip = Vec::new();

        for (path, src_entry) in &source.files {
            if let Some(dest_entry) = destination.files.get(path) {
                if src_entry.hash == dest_entry.hash && src_entry.size == dest_entry.size {
                    to_skip.push(path.clone());
                } else {
                    to_transfer.push(path.clone());
                }
            } else {
                to_transfer.push(path.clone());
            }
        }

        Self {
            to_transfer,
            to_skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_manifest_compare() {
        let mut source = Registry::new();
        let mut dest = Registry::new();

        // 1. Identical file -> should be skipped
        let file1 = FileEntry {
            relative_path: "file1.txt".to_string(),
            size: 100,
            hash: "abc".to_string(),
        };
        source.add_file(file1.clone());
        dest.add_file(file1.clone());

        // 2. Modified file (different hash) -> should be transferred
        let file2_src = FileEntry {
            relative_path: "file2.txt".to_string(),
            size: 200,
            hash: "def_new".to_string(),
        };
        let file2_dest = FileEntry {
            relative_path: "file2.txt".to_string(),
            size: 200,
            hash: "def_old".to_string(),
        };
        source.add_file(file2_src.clone());
        dest.add_file(file2_dest.clone());

        // 3. New file (in source, missing in dest) -> should be transferred
        let file3_src = FileEntry {
            relative_path: "new_file.txt".to_string(),
            size: 300,
            hash: "ghi".to_string(),
        };
        source.add_file(file3_src.clone());

        let manifest = TransferManifest::compare(&source, &dest);

        assert_eq!(manifest.to_skip.len(), 1);
        assert!(manifest.to_skip.contains(&"file1.txt".to_string()));

        assert_eq!(manifest.to_transfer.len(), 2);
        assert!(manifest.to_transfer.contains(&"file2.txt".to_string()));
        assert!(manifest.to_transfer.contains(&"new_file.txt".to_string()));
    }

    #[test]
    fn test_multiple_jobs_per_connection_and_vpn_profile() {
        let mut registry = Registry::new();
        let conn_id = Uuid::new_v4();
        let other_conn_id = Uuid::new_v4();

        let profile = ConnectionProfile {
            id: conn_id,
            name: "Production NAS".to_string(),
            host: "10.0.0.15".to_string(),
            port: 2222,
            username: "backup_svc".to_string(),
            auth_config: AuthConfig {
                key_id: "prod_nas_ed25519".to_string(),
                expected_host_key: None,
            },
            vpn_config: Some(VpnConfig {
                enabled: true,
                encrypted_private_key: "ENC_PRIV_HEX".to_string(),
                address: "10.99.0.2/24".to_string(),
                dns: Some("1.1.1.1".to_string()),
                mtu: Some(1420),
                peer_public_key: "PEER_PUB_KEY_B64".to_string(),
                encrypted_preshared_key: None,
                endpoint: "203.0.113.10:51820".to_string(),
                allowed_ips: vec!["10.0.0.15/32".to_string()],
                persistent_keepalive: Some(25),
                persistent_tunnel: false,
            }),
        };
        registry.upsert_connection(profile.clone());

        let hourly_sync = TransferJob {
            job_id: Uuid::new_v4(),
            connection_id: conn_id,
            name: "Hourly Workspace Sync".to_string(),
            job_type: TransferJobType::Sync,
            source_paths: vec![PathBuf::from("/data/workspace")],
            destination_path: PathBuf::from("/backups/workspace_sync"),
            mappings: vec![(
                PathBuf::from("/data/workspace"),
                PathBuf::from("/backups/workspace_sync"),
            )],
            schedule: Some(ScheduleConfig {
                mode: "Hourly".to_string(),
                interval_secs: Some(3600),
                window_start: Some("08:00".to_string()),
                window_end: Some("20:00".to_string()),
                sync_frequency: Some("Every Hour".to_string()),
                active_days: None,
            }),
            use_snapshots: false,
        };

        let daily_backup = TransferJob {
            job_id: Uuid::new_v4(),
            connection_id: conn_id,
            name: "Daily Full Snapshot Backup".to_string(),
            job_type: TransferJobType::Backup,
            source_paths: vec![
                PathBuf::from("/data/workspace"),
                PathBuf::from("/mnt/volume2/databases"),
            ],
            destination_path: PathBuf::from("/backups/daily_archive"),
            mappings: vec![
                (
                    PathBuf::from("/data/workspace"),
                    PathBuf::from("/backups/daily_archive"),
                ),
                (
                    PathBuf::from("/mnt/volume2/databases"),
                    PathBuf::from("/backups/daily_archive"),
                ),
            ],
            schedule: Some(ScheduleConfig {
                mode: "Daily".to_string(),
                interval_secs: Some(86400),
                window_start: Some("01:00".to_string()),
                window_end: Some("05:00".to_string()),
                sync_frequency: Some("Daily".to_string()),
                active_days: Some("MTWThF".to_string()),
            }),
            use_snapshots: true,
        };

        let unrelated_job = TransferJob {
            job_id: Uuid::new_v4(),
            connection_id: other_conn_id,
            name: "Other Server Job".to_string(),
            job_type: TransferJobType::Sync,
            source_paths: vec![PathBuf::from("/tmp/other")],
            destination_path: PathBuf::from("/remote/other"),
            mappings: vec![(PathBuf::from("/tmp/other"), PathBuf::from("/remote/other"))],
            schedule: None,
            use_snapshots: false,
        };

        registry.upsert_job(hourly_sync.clone());
        registry.upsert_job(daily_backup.clone());
        registry.upsert_job(unrelated_job.clone());

        let jobs_for_conn = registry.get_jobs_for_connection(conn_id);
        assert_eq!(jobs_for_conn.len(), 2);
        assert!(jobs_for_conn.contains(&hourly_sync));
        assert!(jobs_for_conn.contains(&daily_backup));

        // Verify removing one job keeps the other intact
        let removed = registry.remove_job(hourly_sync.job_id);
        assert_eq!(removed.as_ref(), Some(&hourly_sync));
        let remaining_jobs = registry.get_jobs_for_connection(conn_id);
        assert_eq!(remaining_jobs, vec![daily_backup]);
    }
}
