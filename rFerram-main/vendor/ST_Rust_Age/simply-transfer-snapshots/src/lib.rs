use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SnapshotError {
    #[error("Snapshot creation failed: {0}")]
    CreationFailed(String),
    #[error("Path resolution failed: {0}")]
    ResolutionFailed(String),
    #[error("Snapshot cleanup failed: {0}")]
    CleanupFailed(String),
    #[error("Unsupported platform or file system: {0}")]
    Unsupported(String),
    #[error("Elevated privileges required for this operation")]
    PrivilegeRequired,
}

/// Metadata describing a discovered local or remote volume/drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    pub mount_point: PathBuf,
    pub label: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Enumerates all non-pseudo local volumes/drives on the current system.
pub fn list_local_volumes() -> Result<Vec<VolumeInfo>, SnapshotError> {
    #[cfg(target_os = "windows")]
    {
        windows::list_local_volumes()
    }
    #[cfg(target_os = "linux")]
    {
        linux::list_local_volumes()
    }
    #[cfg(target_os = "macos")]
    {
        macos::list_local_volumes()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        Ok(vec![VolumeInfo {
            mount_point: PathBuf::from("/"),
            label: "Root".to_string(),
            fs_type: "unknown".to_string(),
            total_bytes: 0,
            available_bytes: 0,
        }])
    }
}

/// Resolves which volume in `volumes` owns `path`, using case-insensitive drive prefix matching
/// on Windows-style paths (`C:\...`) and longest mount-point prefix matching on Linux/POSIX paths.
pub fn resolve_owning_volume(path: &Path, volumes: &[VolumeInfo]) -> Option<PathBuf> {
    let path_str = path.to_string_lossy();

    // Check Windows-style drive prefix (e.g., "C:\..." or "D:/...")
    if path_str.len() >= 2 && path_str.as_bytes()[1] == b':' {
        let drive_prefix = path_str[..2].to_ascii_uppercase();
        for vol in volumes {
            let vol_str = vol.mount_point.to_string_lossy();
            if vol_str.len() >= 2
                && vol_str.as_bytes()[1] == b':'
                && vol_str[..2].eq_ignore_ascii_case(&drive_prefix)
            {
                return Some(vol.mount_point.clone());
            }
        }
    }

    // POSIX longest mount-point prefix matching
    let mut best_match: Option<&PathBuf> = None;
    let mut best_len: usize = 0;

    for vol in volumes {
        if path.starts_with(&vol.mount_point) {
            let comp_len = vol.mount_point.as_os_str().len();
            if comp_len >= best_len {
                best_len = comp_len;
                best_match = Some(&vol.mount_point);
            }
        }
    }

    best_match.cloned()
}

/// Groups a list of source paths by their owning volume mount point.
pub fn group_paths_by_volume(
    paths: &[PathBuf],
    volumes: &[VolumeInfo],
) -> HashMap<PathBuf, Vec<PathBuf>> {
    let mut grouped: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for path in paths {
        let owning = resolve_owning_volume(path, volumes).unwrap_or_else(|| {
            if cfg!(windows) {
                PathBuf::from("C:\\")
            } else {
                PathBuf::from("/")
            }
        });
        grouped.entry(owning).or_default().push(path.clone());
    }
    grouped
}

/// Represents an active OS-level snapshot.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// A platform-specific identifier for the snapshot.
    /// (e.g., VSS Shadow Copy ID, APFS snapshot name, or LVM volume name)
    pub id: String,
    /// The mount point or prefix of the snapshot volume.
    pub mount_point: PathBuf,
}

/// A common trait abstracting native OS shadow-copy/snapshotting mechanisms.
pub trait SnapshotDriver: Send + Sync {
    /// Returns the type/name of the snapshot technology used (e.g. "VSS", "APFS", "Live")
    fn snapshot_type(&self) -> &'static str;

    /// Create a point-in-time snapshot of the volume containing the given path.
    fn create_snapshot(&self, volume_path: &Path) -> Result<Snapshot, SnapshotError>;

    /// Translate a standard file path into its snapshot-relative equivalent
    /// so it can be safely read even if exclusively locked by an application.
    fn resolve_snapshot_path(
        &self,
        snapshot: &Snapshot,
        original_path: &Path,
    ) -> Result<PathBuf, SnapshotError>;

    /// Release and clean up the snapshot from the operating system.
    fn cleanup_snapshot(&self, snapshot: &Snapshot) -> Result<(), SnapshotError>;

    /// Creates a coordinated set of snapshots across multiple unique volume mount points.
    fn create_snapshot_set(
        &self,
        volume_paths: &[PathBuf],
    ) -> Result<HashMap<PathBuf, Snapshot>, SnapshotError> {
        let mut set = HashMap::new();
        for vol in volume_paths {
            let snap = self.create_snapshot(vol)?;
            set.insert(vol.clone(), snap);
        }
        Ok(set)
    }

    /// Resolves `original_path` located on `owning_volume` into its snapshot-rooted path.
    fn resolve_snapshot_path_for_volume(
        &self,
        snapshot: &Snapshot,
        owning_volume: &Path,
        original_path: &Path,
    ) -> Result<PathBuf, SnapshotError> {
        if snapshot.mount_point.as_os_str().is_empty() {
            return Ok(original_path.to_path_buf());
        }
        if let Ok(rel) = original_path.strip_prefix(owning_volume) {
            return Ok(snapshot.mount_point.join(rel));
        }
        self.resolve_snapshot_path(snapshot, original_path)
    }

    /// Releases all snapshots in a multi-volume snapshot set.
    fn cleanup_snapshot_set(
        &self,
        snapshots: &HashMap<PathBuf, Snapshot>,
    ) -> Result<(), SnapshotError> {
        let mut first_err = None;
        for snap in snapshots.values() {
            if let Err(e) = self.cleanup_snapshot(snap)
                && first_err.is_none()
            {
                first_err = Some(e);
            }
        }
        if let Some(e) = first_err {
            Err(e)
        } else {
            Ok(())
        }
    }
}

/// A fallback driver that simply returns the original path.
/// Used when elevated privileges are unavailable or snapshots are unsupported.
pub struct FallbackSnapshotDriver;

impl SnapshotDriver for FallbackSnapshotDriver {
    fn snapshot_type(&self) -> &'static str {
        "Live"
    }

    fn create_snapshot(&self, _volume_path: &Path) -> Result<Snapshot, SnapshotError> {
        Ok(Snapshot {
            id: "fallback".to_string(),
            mount_point: PathBuf::new(),
        })
    }

    fn resolve_snapshot_path(
        &self,
        _snapshot: &Snapshot,
        original_path: &Path,
    ) -> Result<PathBuf, SnapshotError> {
        Ok(original_path.to_path_buf())
    }

    fn cleanup_snapshot(&self, _snapshot: &Snapshot) -> Result<(), SnapshotError> {
        Ok(())
    }
}

// Platform specific module exports
#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

/// Factory function to get the appropriate driver for the current platform.
pub fn get_native_driver() -> Box<dyn SnapshotDriver> {
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::VssSnapshotDriver::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::ApfsSnapshotDriver::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxSnapshotDriver::new())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Box::new(FallbackSnapshotDriver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_local_volumes_contains_root() {
        let volumes = list_local_volumes().expect("list_local_volumes should succeed");
        assert!(
            !volumes.is_empty(),
            "Expected at least one local volume to be discovered"
        );
        #[cfg(not(target_os = "windows"))]
        {
            assert!(
                volumes.iter().any(|v| v.mount_point == Path::new("/")),
                "Expected root volume '/' to be present in {:?}",
                volumes
            );
        }
    }

    #[test]
    fn test_resolve_owning_volume_posix_and_windows() {
        let posix_volumes = vec![
            VolumeInfo {
                mount_point: PathBuf::from("/"),
                label: "Root".to_string(),
                fs_type: "ext4".to_string(),
                total_bytes: 100_000,
                available_bytes: 50_000,
            },
            VolumeInfo {
                mount_point: PathBuf::from("/mnt/data"),
                label: "Data".to_string(),
                fs_type: "xfs".to_string(),
                total_bytes: 500_000,
                available_bytes: 400_000,
            },
            VolumeInfo {
                mount_point: PathBuf::from("/mnt/data/archive"),
                label: "Archive".to_string(),
                fs_type: "btrfs".to_string(),
                total_bytes: 900_000,
                available_bytes: 800_000,
            },
        ];

        assert_eq!(
            resolve_owning_volume(Path::new("/home/user/docs"), &posix_volumes),
            Some(PathBuf::from("/"))
        );
        assert_eq!(
            resolve_owning_volume(Path::new("/mnt/data/projects/rust"), &posix_volumes),
            Some(PathBuf::from("/mnt/data"))
        );
        assert_eq!(
            resolve_owning_volume(Path::new("/mnt/data/archive/2026/db.sql"), &posix_volumes),
            Some(PathBuf::from("/mnt/data/archive"))
        );

        let win_volumes = vec![
            VolumeInfo {
                mount_point: PathBuf::from(r"C:\"),
                label: "OS".to_string(),
                fs_type: "NTFS".to_string(),
                total_bytes: 100_000,
                available_bytes: 50_000,
            },
            VolumeInfo {
                mount_point: PathBuf::from(r"D:\"),
                label: "Storage".to_string(),
                fs_type: "NTFS".to_string(),
                total_bytes: 500_000,
                available_bytes: 400_000,
            },
        ];

        assert_eq!(
            resolve_owning_volume(Path::new(r"d:\Backups\sql\prod.bak"), &win_volumes),
            Some(PathBuf::from(r"D:\"))
        );
        assert_eq!(
            resolve_owning_volume(Path::new(r"C:\Users\Admin\Documents"), &win_volumes),
            Some(PathBuf::from(r"C:\"))
        );
    }
}
