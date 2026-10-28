//! Domain models for connection profiles, zeroized host credentials, and NIST session tokens.

use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// Remote Desktop Connection Profile shared across UI, Client, and Server crates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub friendly_name: String,
    pub group_name: String,
    pub host_address: String,
    #[serde(default)]
    pub vpn_binding: Option<String>,
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
    #[serde(default)]
    pub admin_session_takeover: bool,
    #[serde(default = "default_true")]
    pub ask_credentials_on_connect: bool,
    #[serde(default)]
    pub folder_shares: Vec<String>,
    #[serde(default = "default_true")]
    pub enable_speaker_audio: bool,
    #[serde(default)]
    pub enable_mic_passthrough: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            friendly_name: String::from("Remote Workstation"),
            group_name: String::from("Group 1"),
            host_address: String::from("127.0.0.1:4433"),
            vpn_binding: None,
            auto_reconnect: true,
            admin_session_takeover: false,
            ask_credentials_on_connect: true,
            folder_shares: Vec::new(),
            enable_speaker_audio: true,
            enable_mic_passthrough: false,
        }
    }
}

impl ConnectionProfile {
    pub fn name(&self) -> &str {
        &self.friendly_name
    }

    pub fn group(&self) -> &str {
        &self.group_name
    }
}

/// Zeroized Credential Container for Host or Session Authentication.
#[derive(Clone, Serialize, Deserialize)]
pub struct HostCredentials {
    pub username: String,
    pub secret: String,
    pub ed25519_public_key_hex: Option<String>,
}

impl Zeroize for HostCredentials {
    fn zeroize(&mut self) {
        self.username.zeroize();
        self.secret.zeroize();
    }
}

impl Drop for HostCredentials {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl fmt::Debug for HostCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostCredentials")
            .field("username", &self.username)
            .field("secret", &"[REDACTED]")
            .field("ed25519_public_key_hex", &self.ed25519_public_key_hex)
            .finish()
    }
}

/// Zeroized Vault Session Token Model (NIST SP 800-63B Compliant).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultTokenModel {
    pub session_token: String,
    pub auth_method: String,
    pub expires_at: u64,
}

impl Zeroize for VaultTokenModel {
    fn zeroize(&mut self) {
        self.session_token.zeroize();
        self.auth_method.zeroize();
        self.expires_at.zeroize();
    }
}

impl Drop for VaultTokenModel {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl fmt::Debug for VaultTokenModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultTokenModel")
            .field("session_token", &"[REDACTED]")
            .field("auth_method", &self.auth_method)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
