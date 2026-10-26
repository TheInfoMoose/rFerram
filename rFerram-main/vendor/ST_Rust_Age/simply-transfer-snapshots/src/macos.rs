use crate::{Snapshot, SnapshotDriver, SnapshotError};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::warn;

pub struct ApfsSnapshotDriver;

impl Default for ApfsSnapshotDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl ApfsSnapshotDriver {
    pub fn new() -> Self {
        Self
    }
}

impl SnapshotDriver for ApfsSnapshotDriver {
    fn snapshot_type(&self) -> &'static str {
        "APFS"
    }

    fn create_snapshot(&self, _volume_path: &Path) -> Result<Snapshot, SnapshotError> {
        warn!("APFS snapshotting requires elevated privileges (tmutil / diskutil).");

        // Example: `tmutil localsnapshot` or `diskutil apfs snapshot /`
        let output = Command::new("tmutil")
            .arg("localsnapshot")
            .output()
            .map_err(|e| SnapshotError::CreationFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapshotError::CreationFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        // Parse stdout for the snapshot name, e.g., "Created local snapshot with date: 2026-09-25-103000"
        let output_str = String::from_utf8_lossy(&output.stdout);
        let id = output_str
            .split_whitespace()
            .last()
            .unwrap_or("unknown")
            .to_string();

        Ok(Snapshot {
            id,
            // APFS local snapshots are accessible via /Volumes/com.apple.TimeMachine.localsnapshots/
            mount_point: PathBuf::from(format!(
                "/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb/Mac/{}/Macintosh HD",
                "id-placeholder"
            )),
        })
    }

    fn resolve_snapshot_path(
        &self,
        _snapshot: &Snapshot,
        original_path: &Path,
    ) -> Result<PathBuf, SnapshotError> {
        // TODO: proper resolution against the mounted APFS snapshot point
        Ok(original_path.to_path_buf())
    }

    fn cleanup_snapshot(&self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        // Example: `tmutil deletelocalsnapshots <date>`
        let status = Command::new("tmutil")
            .arg("deletelocalsnapshots")
            .arg(&snapshot.id)
            .status()
            .map_err(|e| SnapshotError::CleanupFailed(e.to_string()))?;

        if !status.success() {
            return Err(SnapshotError::CleanupFailed(
                "tmutil failed to delete snapshot".to_string(),
            ));
        }
        Ok(())
    }
}

pub fn list_local_volumes() -> Result<Vec<crate::VolumeInfo>, SnapshotError> {
    let mut volumes = Vec::new();
    if let Ok(output) = Command::new("df").args(["-k", "-P"]).output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if let Some(cap_idx) = tokens.iter().position(|t| t.ends_with('%'))
                && cap_idx >= 3
                && cap_idx + 1 < tokens.len()
            {
                let total_kb = tokens[cap_idx - 3].parse::<u64>().unwrap_or(0);
                let avail_kb = tokens[cap_idx - 1].parse::<u64>().unwrap_or(0);
                let mount_str = tokens[cap_idx + 1..].join(" ");
                if mount_str == "/" || mount_str.starts_with("/Volumes/") {
                    volumes.push(crate::VolumeInfo {
                        mount_point: PathBuf::from(&mount_str),
                        label: mount_str.clone(),
                        fs_type: "APFS".to_string(),
                        total_bytes: total_kb.saturating_mul(1024),
                        available_bytes: avail_kb.saturating_mul(1024),
                    });
                }
            }
        }
    }
    if volumes.is_empty() {
        volumes.push(crate::VolumeInfo {
            mount_point: PathBuf::from("/"),
            label: "Macintosh HD (/)".to_string(),
            fs_type: "APFS".to_string(),
            total_bytes: 0,
            available_bytes: 0,
        });
    }
    Ok(volumes)
}
