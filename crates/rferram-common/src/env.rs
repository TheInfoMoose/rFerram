//! OS & desktop environment detection (`PlatformDistribution`, `RuntimeEnvironment`).

use serde::{Deserialize, Serialize};

/// Target Linux Distribution Family or Operating System Platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlatformDistribution {
    LinuxDebian,
    LinuxRhel,
    LinuxSuse,
    LinuxArch,
    LinuxGeneric,
    Windows,
    MacOs,
}

/// Runtime Desktop & OS Environment Metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEnvironment {
    pub platform: PlatformDistribution,
    pub is_cosmic_desktop: bool,
    pub is_wayland: bool,
}

impl RuntimeEnvironment {
    /// Detects the current operating system, Linux distribution family (`/etc/os-release`),
    /// Wayland session status, and whether the COSMIC Desktop Environment is active.
    pub fn detect() -> Self {
        let is_cosmic_desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .or_else(|_| std::env::var("DESKTOP_SESSION"))
            .map(|val| val.to_ascii_uppercase().contains("COSMIC"))
            .unwrap_or(false);

        let is_wayland = std::env::var("WAYLAND_DISPLAY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            || std::env::var("XDG_SESSION_TYPE")
                .map(|v| v.eq_ignore_ascii_case("wayland"))
                .unwrap_or(false);

        let platform = Self::detect_platform();

        Self {
            platform,
            is_cosmic_desktop,
            is_wayland,
        }
    }

    fn detect_platform() -> PlatformDistribution {
        if cfg!(target_os = "windows") {
            return PlatformDistribution::Windows;
        }
        if cfg!(target_os = "macos") {
            return PlatformDistribution::MacOs;
        }
        if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
            return Self::parse_os_release(&os_release);
        }
        PlatformDistribution::LinuxGeneric
    }

    /// Parses `/etc/os-release` content into a `PlatformDistribution`.
    pub fn parse_os_release(os_release: &str) -> PlatformDistribution {
        let lower = os_release.to_ascii_lowercase();
        if lower.contains("arch") || lower.contains("cachyos") || lower.contains("manjaro") {
            return PlatformDistribution::LinuxArch;
        }
        if lower.contains("debian")
            || lower.contains("ubuntu")
            || lower.contains("pop")
            || lower.contains("mint")
        {
            return PlatformDistribution::LinuxDebian;
        }
        if lower.contains("rhel")
            || lower.contains("fedora")
            || lower.contains("centos")
            || lower.contains("almalinux")
            || lower.contains("rocky")
        {
            return PlatformDistribution::LinuxRhel;
        }
        if lower.contains("suse") || lower.contains("opensuse") || lower.contains("sles") {
            return PlatformDistribution::LinuxSuse;
        }
        PlatformDistribution::LinuxGeneric
    }
}
