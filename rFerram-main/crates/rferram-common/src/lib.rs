//! `rferram-common`
//!
//! Central domain models, session configuration profiles, OS/desktop environment
//! detection (Linux Debian/RHEL/SUSE/Arch, COSMIC Desktop, Windows, macOS), and
//! NIST-compliant credential vault abstractions backed by `keyring` and `simply-transfer-crypto`.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use simply_transfer_crypto::{CryptoError, SecureStorage};
use std::fmt;
use thiserror::Error;
use zeroize::Zeroize;

/// Errors produced by `rferram-common` vault and profile operations.
#[derive(Debug, Error)]
pub enum CommonError {
    #[error("Vault storage error: {0}")]
    Vault(#[from] CryptoError),
    #[error("Cryptographic key/signature error: {0}")]
    Crypto(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

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
        }
        PlatformDistribution::LinuxGeneric
    }
}

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

/// NIST-Compliant Credential Vault backed by OS Keyring (`keyring`) and
/// `simply-transfer-crypto::SecureStorage` (AES-256-GCM fallback).
pub struct NistVault {
    storage: SecureStorage,
}

impl Default for NistVault {
    fn default() -> Self {
        Self::new("com.theinfomoose.rferram")
    }
}

impl NistVault {
    pub fn new(service_namespace: &str) -> Self {
        Self {
            storage: SecureStorage::new(service_namespace),
        }
    }

    /// Stores a `VaultTokenModel` under `profile_id` in the OS keystore / encrypted vault.
    pub fn store_session_token(
        &self,
        profile_id: &str,
        token: &VaultTokenModel,
    ) -> Result<(), CommonError> {
        let key = format!("token_{profile_id}");
        let json = serde_json::to_string(token)?;
        self.storage.set_secret(&key, json)?;
        Ok(())
    }

    /// Retrieves a `VaultTokenModel` for `profile_id`.
    pub fn get_session_token(&self, profile_id: &str) -> Result<VaultTokenModel, CommonError> {
        let key = format!("token_{profile_id}");
        let mut raw = self.storage.get_secret(&key)?;
        let parsed = serde_json::from_str::<VaultTokenModel>(&raw);
        raw.zeroize();
        Ok(parsed?)
    }

    /// Deletes a stored session token.
    pub fn delete_session_token(&self, profile_id: &str) -> Result<(), CommonError> {
        let key = format!("token_{profile_id}");
        self.storage.delete_secret(&key)?;
        Ok(())
    }

    /// Generates or loads a persistent Ed25519 identity keypair for P2P handshakes.
    pub fn get_or_create_ed25519_identity(
        &self,
        identity_id: &str,
    ) -> Result<(SigningKey, VerifyingKey), CommonError> {
        let key_name = format!("ed25519_{identity_id}");
        if let Ok(mut hex_seed) = self.storage.get_secret(&key_name) {
            let bytes = hex::decode(hex_seed.trim())
                .map_err(|e| CommonError::Crypto(format!("Invalid stored Ed25519 hex: {e}")))?;
            hex_seed.zeroize();
            let seed: [u8; 32] = bytes
                .try_into()
                .map_err(|_| CommonError::Crypto("Ed25519 seed must be 32 bytes".to_string()))?;
            let signing = SigningKey::from_bytes(&seed);
            let verifying = signing.verifying_key();
            return Ok((signing, verifying));
        }

        let seed: [u8; 32] = rand::random();
        let signing = SigningKey::from_bytes(&seed);
        let verifying = signing.verifying_key();
        self.storage.set_secret(&key_name, hex::encode(seed))?;
        Ok((signing, verifying))
    }

    /// Signs a handshake challenge combined with a peer TLS certificate SHA-256 fingerprint.
    pub fn sign_channel_bound_challenge(
        signing_key: &SigningKey,
        challenge: &[u8],
        tls_cert_sha256: &[u8; 32],
    ) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"rFerram-v1-ChannelBinding:");
        hasher.update(challenge);
        hasher.update(tls_cert_sha256);
        let digest = hasher.finalize();
        signing_key.sign(&digest).to_bytes().to_vec()
    }

    /// Verifies an Ed25519 signature over the channel-bound challenge.
    pub fn verify_channel_bound_challenge(
        verifying_key_bytes: &[u8; 32],
        challenge: &[u8],
        tls_cert_sha256: &[u8; 32],
        signature_bytes: &[u8],
    ) -> Result<(), CommonError> {
        use sha2::{Digest, Sha256};
        let verifying_key = VerifyingKey::from_bytes(verifying_key_bytes)
            .map_err(|e| CommonError::Crypto(format!("Invalid Ed25519 public key: {e}")))?;
        let sig = Signature::from_slice(signature_bytes)
            .map_err(|e| CommonError::Crypto(format!("Invalid Ed25519 signature: {e}")))?;

        let mut hasher = Sha256::new();
        hasher.update(b"rFerram-v1-ChannelBinding:");
        hasher.update(challenge);
        hasher.update(tls_cert_sha256);
        let digest = hasher.finalize();

        verifying_key
            .verify(&digest, &sig)
            .map_err(|e| CommonError::Crypto(format!("Ed25519 challenge verification failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ed25519_channel_bound_challenge_roundtrip() {
        let vault = NistVault::new("test.rferram.common");
        let (signing, verifying) = vault
            .get_or_create_ed25519_identity("unit_test_peer")
            .expect("Failed to create Ed25519 identity");

        let challenge = b"random-32-byte-challenge-payload";
        let cert_hash = [0x42u8; 32];

        let signature = NistVault::sign_channel_bound_challenge(&signing, challenge, &cert_hash);
        assert_eq!(signature.len(), 64);

        NistVault::verify_channel_bound_challenge(
            &verifying.to_bytes(),
            challenge,
            &cert_hash,
            &signature,
        )
        .expect("Channel-bound signature verification should succeed");

        let _ = vault.storage.delete_secret("ed25519_unit_test_peer");
    }

    #[test]
    fn test_runtime_environment_detection() {
        let env = RuntimeEnvironment::detect();
        if cfg!(target_os = "linux") {
            assert!(matches!(
                env.platform,
                PlatformDistribution::LinuxDebian
                    | PlatformDistribution::LinuxRhel
                    | PlatformDistribution::LinuxSuse
                    | PlatformDistribution::LinuxArch
                    | PlatformDistribution::LinuxGeneric
            ));
        }
    }
}
