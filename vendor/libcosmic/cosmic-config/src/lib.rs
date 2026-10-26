//! `cosmic-config` — COSMIC Desktop Environment Configuration & Theme Persistence
//!
//! Stores per-key atomic configuration files under `~/.config/cosmic/<app_id>/v<version>/`
//! on Linux (Debian, RHEL, SUSE, Arch, Pop!_OS), `%APPDATA%\cosmic\<app_id>\v<version>\` on Windows,
//! and `~/Library/Application Support/cosmic/<app_id>/v<version>/` on macOS.

use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error accessing COSMIC config: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error in COSMIC config: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct Config {
    app_id: String,
    version: u64,
    base_dir: PathBuf,
}

impl Config {
    pub fn new(app_id: &str, version: u64) -> Result<Self, ConfigError> {
        let base_dir = Self::resolve_config_root()
            .join(app_id)
            .join(format!("v{version}"));
        let _ = fs::create_dir_all(&base_dir);
        Ok(Self {
            app_id: app_id.to_string(),
            version,
            base_dir,
        })
    }

    pub fn resolve_config_root() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.trim().is_empty()
        {
            return PathBuf::from(xdg).join("cosmic");
        }
        #[cfg(target_os = "windows")]
        if let Ok(appdata) = std::env::var("APPDATA")
            && !appdata.trim().is_empty()
        {
            return PathBuf::from(appdata).join("cosmic");
        }
        #[cfg(target_os = "macos")]
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("cosmic");
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home).join(".config").join("cosmic");
        }
        std::env::temp_dir().join("cosmic")
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<T, ConfigError> {
        let path = self.base_dir.join(key);
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<(), ConfigError> {
        let _ = fs::create_dir_all(&self.base_dir);
        let path = self.base_dir.join(key);
        let tmp_path = self
            .base_dir
            .join(format!(".{key}.tmp.{}", std::process::id()));
        let serialized = serde_json::to_string_pretty(value)?;
        fs::write(&tmp_path, serialized)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }
}
