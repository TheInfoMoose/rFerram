use crate::{Snapshot, SnapshotDriver, SnapshotError};
use std::path::{Path, PathBuf};
use std::process::Command;
pub struct LinuxSnapshotDriver;

impl Default for LinuxSnapshotDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxSnapshotDriver {
    pub fn new() -> Self {
        Self
    }
}

impl SnapshotDriver for LinuxSnapshotDriver {
    fn snapshot_type(&self) -> &'static str {
        "LVM"
    }

    fn create_snapshot(&self, volume_path: &Path) -> Result<Snapshot, SnapshotError> {
        let snap_id = format!(
            "st_snap_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        let mount_point = PathBuf::from(format!("/mnt/{}", snap_id));

        // Create mount directory
        std::fs::create_dir_all(&mount_point).map_err(|e| {
            SnapshotError::CreationFailed(format!("Failed to create mount directory: {}", e))
        })?;

        // Create LVM snapshot
        let output = Command::new("lvcreate")
            .args([
                "-s",
                "-n",
                &snap_id,
                "-l",
                "100%FREE",
                volume_path.to_str().unwrap_or(""),
            ])
            .output()
            .map_err(|e| {
                SnapshotError::CreationFailed(format!("Failed to execute lvcreate: {}", e))
            })?;

        if !output.status.success() {
            return Err(SnapshotError::CreationFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        // Mount the snapshot (assuming the vg name can be extracted, we'll try a generic mount by finding the newly created block device)
        // Note: In a production app, the volume group parsing requires more complex logic.
        let dev_path = format!("/dev/mapper/*-{}", snap_id.replace("_", "--"));

        let mount_output = Command::new("sh")
            .args([
                "-c",
                &format!("mount {} {}", dev_path, mount_point.display()),
            ])
            .output()
            .map_err(|e| {
                SnapshotError::CreationFailed(format!("Failed to execute mount: {}", e))
            })?;

        if !mount_output.status.success() {
            return Err(SnapshotError::CreationFailed(
                String::from_utf8_lossy(&mount_output.stderr).to_string(),
            ));
        }

        Ok(Snapshot {
            id: snap_id,
            mount_point,
        })
    }

    fn resolve_snapshot_path(
        &self,
        snapshot: &Snapshot,
        original_path: &Path,
    ) -> Result<PathBuf, SnapshotError> {
        Ok(snapshot
            .mount_point
            .join(original_path.strip_prefix("/").unwrap_or(original_path)))
    }

    fn cleanup_snapshot(&self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        // Unmount
        let umount_output = Command::new("umount")
            .arg(&snapshot.mount_point)
            .output()
            .map_err(|e| {
                SnapshotError::CleanupFailed(format!("Failed to execute umount: {}", e))
            })?;

        if !umount_output.status.success() {
            tracing::warn!(
                "Failed to unmount snapshot: {}",
                String::from_utf8_lossy(&umount_output.stderr)
            );
        }

        // Remove snapshot volume (using wildcards via sh)
        let dev_path = format!("/dev/mapper/*-{}", snapshot.id.replace("_", "--"));
        let rm_output = Command::new("sh")
            .args(["-c", &format!("lvremove -f {}", dev_path)])
            .output()
            .map_err(|e| {
                SnapshotError::CleanupFailed(format!("Failed to execute lvremove: {}", e))
            })?;

        if !rm_output.status.success() {
            return Err(SnapshotError::CleanupFailed(
                String::from_utf8_lossy(&rm_output.stderr).to_string(),
            ));
        }

        // Remove mount directory
        let _ = std::fs::remove_dir(&snapshot.mount_point);

        Ok(())
    }
}

const IGNORED_FS_TYPES: &[&str] = &[
    "proc",
    "sysfs",
    "tmpfs",
    "devtmpfs",
    "cgroup",
    "cgroup2",
    "overlay",
    "squashfs",
    "devpts",
    "securityfs",
    "pstore",
    "efivarfs",
    "bpf",
    "autofs",
    "mqueue",
    "hugetlbfs",
    "debugfs",
    "tracefs",
    "fusectl",
    "configfs",
    "ramfs",
    "binfmt_misc",
    "nsfs",
    "rpc_pipefs",
    "fuse.portal",
    "fuse.gvfsd-fuse",
];

fn query_df_for_path(path: &Path) -> Option<(u64, u64)> {
    let output = Command::new("df")
        .args(["-k", "-P"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().skip(1) {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if let Some(cap_idx) = tokens.iter().position(|t| t.ends_with('%'))
            && cap_idx >= 3
        {
            let total_kb = tokens[cap_idx - 3].parse::<u64>().unwrap_or(0);
            let avail_kb = tokens[cap_idx - 1].parse::<u64>().unwrap_or(0);
            return Some((total_kb.saturating_mul(1024), avail_kb.saturating_mul(1024)));
        }
    }
    None
}

pub fn list_local_volumes() -> Result<Vec<crate::VolumeInfo>, SnapshotError> {
    let mounts_content = std::fs::read_to_string("/proc/mounts").map_err(|e| {
        SnapshotError::ResolutionFailed(format!("Failed to read /proc/mounts: {}", e))
    })?;

    // Map mount_point -> (total_bytes, available_bytes) from `df -k -P`
    let mut df_stats: std::collections::HashMap<String, (u64, u64)> =
        std::collections::HashMap::new();
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
                df_stats.insert(
                    mount_str,
                    (total_kb.saturating_mul(1024), avail_kb.saturating_mul(1024)),
                );
            }
        }
    }

    let mut volumes = Vec::new();
    let mut seen_mounts = std::collections::HashSet::new();

    for line in mounts_content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let device = parts[0];
        let mount_raw = parts[1].replace("\\040", " ");
        let fs_type = parts[2];

        if IGNORED_FS_TYPES.contains(&fs_type) {
            continue;
        }
        if mount_raw.starts_with("/proc")
            || mount_raw.starts_with("/sys")
            || mount_raw.starts_with("/dev")
            || (mount_raw.starts_with("/run") && !mount_raw.starts_with("/run/media"))
            || mount_raw.starts_with("/snap")
            || mount_raw.starts_with("/boot")
        {
            continue;
        }

        if !seen_mounts.insert(mount_raw.clone()) {
            continue;
        }

        let (total_bytes, available_bytes) = df_stats
            .get(&mount_raw)
            .copied()
            .or_else(|| query_df_for_path(Path::new(&mount_raw)))
            .unwrap_or((0, 0));
        let label = if mount_raw == "/" {
            format!("Root ({})", device)
        } else {
            let name = Path::new(&mount_raw)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| mount_raw.clone());
            format!("{} ({})", name, device)
        };

        volumes.push(crate::VolumeInfo {
            mount_point: PathBuf::from(&mount_raw),
            label,
            fs_type: fs_type.to_string(),
            total_bytes,
            available_bytes,
        });
    }

    // Also discover drive directories under /mnt and /media that may not appear as separate mounts
    for base_dir in ["/mnt", "/media"] {
        if let Ok(entries) = std::fs::read_dir(base_dir) {
            let mut dir_entries: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect();
            dir_entries.sort();
            for entry_path in dir_entries {
                let mount_str = entry_path.to_string_lossy().to_string();
                if mount_str.starts_with("/mnt/st_snap_") {
                    continue;
                }
                if seen_mounts.insert(mount_str.clone()) {
                    let (total_bytes, available_bytes) = df_stats
                        .get(&mount_str)
                        .copied()
                        .or_else(|| query_df_for_path(&entry_path))
                        .or_else(|| df_stats.get("/").copied())
                        .unwrap_or((0, 0));
                    let name = entry_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| mount_str.clone());
                    volumes.push(crate::VolumeInfo {
                        mount_point: entry_path,
                        label: format!("{} ({})", name, mount_str),
                        fs_type: "posix".to_string(),
                        total_bytes,
                        available_bytes,
                    });
                }
            }
        }
    }

    if !seen_mounts.contains("/") {
        let (total_bytes, available_bytes) = df_stats
            .get("/")
            .copied()
            .or_else(|| query_df_for_path(Path::new("/")))
            .unwrap_or((0, 0));
        volumes.insert(
            0,
            crate::VolumeInfo {
                mount_point: PathBuf::from("/"),
                label: "Root (/)".to_string(),
                fs_type: "rootfs".to_string(),
                total_bytes,
                available_bytes,
            },
        );
    }

    Ok(volumes)
}
