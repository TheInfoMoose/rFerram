use crate::{Snapshot, SnapshotDriver, SnapshotError};
use std::path::{Path, PathBuf};

fn hidden_powershell() -> std::process::Command {
    let mut cmd = std::process::Command::new("powershell");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cmd
}

pub struct VssSnapshotDriver;

impl Default for VssSnapshotDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl VssSnapshotDriver {
    pub fn new() -> Self {
        Self
    }
}

impl SnapshotDriver for VssSnapshotDriver {
    fn snapshot_type(&self) -> &'static str {
        "VSS"
    }

    fn create_snapshot(&self, volume_path: &Path) -> Result<Snapshot, SnapshotError> {
        let drive = volume_path
            .to_string_lossy()
            .chars()
            .take(2)
            .collect::<String>()
            + "\\";

        let script = format!(
            "$class = Get-WmiObject -List Win32_ShadowCopy; \
             $res = $class.Create('{}', 'ClientAccessible'); \
             if ($res.ReturnValue -ne 0) {{ exit 1 }}; \
             $shadow = Get-WmiObject Win32_ShadowCopy | Where-Object {{ $_.ID -eq $res.ShadowID }}; \
             Write-Output ($shadow.ID + '|' + $shadow.DeviceObject)",
            drive
        );

        let output = hidden_powershell()
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|e| SnapshotError::CreationFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapshotError::CreationFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = output_str.trim().split('|').collect();
        if parts.len() != 2 {
            return Err(SnapshotError::CreationFailed(
                "Failed to parse WMI ShadowCopy output".to_string(),
            ));
        }

        Ok(Snapshot {
            id: parts[0].to_string(),
            mount_point: PathBuf::from(parts[1]),
        })
    }

    fn resolve_snapshot_path(
        &self,
        snapshot: &Snapshot,
        original_path: &Path,
    ) -> Result<PathBuf, SnapshotError> {
        let path_str = original_path.to_string_lossy();
        if path_str.len() > 2 && path_str.chars().nth(1) == Some(':') {
            let relative = &path_str[2..];
            let relative = relative.trim_start_matches('\\');
            Ok(snapshot.mount_point.join(relative))
        } else {
            Ok(original_path.to_path_buf())
        }
    }

    fn cleanup_snapshot(&self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        let script = format!(
            "$shadow = Get-WmiObject Win32_ShadowCopy | Where-Object {{ $_.ID -eq '{}' }}; \
             if ($shadow) {{ $shadow.Delete() }}",
            snapshot.id
        );

        let output = hidden_powershell()
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|e| SnapshotError::CleanupFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SnapshotError::CleanupFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    fn create_snapshot_set(
        &self,
        volume_paths: &[PathBuf],
    ) -> Result<std::collections::HashMap<PathBuf, Snapshot>, SnapshotError> {
        let mut snapshots = std::collections::HashMap::new();
        for vol in volume_paths {
            let snap = self.create_snapshot(vol)?;
            snapshots.insert(vol.clone(), snap);
        }
        Ok(snapshots)
    }
}

pub fn list_local_volumes() -> Result<Vec<crate::VolumeInfo>, SnapshotError> {
    let script = "[System.IO.DriveInfo]::GetDrives() | Where-Object { $_.IsReady } | ForEach-Object { \
        $root = $_.Name; \
        $free = [uint64]$_.AvailableFreeSpace; \
        $total = [uint64]$_.TotalSize; \
        $fs = if ($_.DriveFormat) { $_.DriveFormat } else { 'NTFS' }; \
        $desc = if ($_.VolumeLabel) { $_.VolumeLabel } else { $_.Name.TrimEnd('\\') }; \
        Write-Output ($root + '|' + $desc + '|' + $fs + '|' + $total + '|' + $free) \
    }";

    let mut volumes = Vec::new();
    if let Ok(output) = hidden_powershell()
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.trim().split('|').collect();
            if parts.len() == 5 {
                let mount_point = PathBuf::from(parts[0]);
                let label = parts[1].to_string();
                let fs_type = parts[2].to_string();
                let total_bytes = parts[3].parse::<u64>().unwrap_or(0);
                let available_bytes = parts[4].parse::<u64>().unwrap_or(0);
                volumes.push(crate::VolumeInfo {
                    mount_point,
                    label,
                    fs_type,
                    total_bytes,
                    available_bytes,
                });
            }
        }
    }

    if volumes.is_empty() {
        for letter in b'C'..=b'Z' {
            let root_str = format!("{}:\\", letter as char);
            let root_path = PathBuf::from(&root_str);
            if root_path.exists() {
                volumes.push(crate::VolumeInfo {
                    mount_point: root_path,
                    label: format!("Local Disk ({}:)", letter as char),
                    fs_type: "NTFS".to_string(),
                    total_bytes: 0,
                    available_bytes: 0,
                });
            }
        }
    }

    Ok(volumes)
}
