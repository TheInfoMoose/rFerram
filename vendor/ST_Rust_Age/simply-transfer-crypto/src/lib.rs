pub mod hash;
pub mod token;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use keyring::Entry;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use thiserror::Error;
use zeroize::Zeroize;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Keyring error: {0}")]
    KeyringError(#[from] keyring::Error),
    #[error("Value not found")]
    NotFound,
    #[error("Other error: {0}")]
    Other(String),
}

/// Process-wide lock serializing every read-modify-write cycle on the shared vault file.
static VAULT_LOCK: Mutex<()> = Mutex::new(());

fn lock_vault() -> MutexGuard<'static, ()> {
    VAULT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Abstract storage for secrets (like SSH passphrases or profile secrets)
/// backed by the native OS keyring with an authenticated AES-256-GCM encrypted vault fallback.
pub struct SecureStorage {
    service_name: String,
}

impl SecureStorage {
    /// Create a new SecureStorage instance for a specific service namespace.
    /// E.g. "com.simplytransfer.app"
    pub fn new(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_string(),
        }
    }

    fn get_entry(&self, account: &str) -> Result<Entry, CryptoError> {
        Entry::new(&self.service_name, account).map_err(CryptoError::KeyringError)
    }

    fn get_vault_path(&self) -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let primary_dir = PathBuf::from(&home).join(".simply-transfer");
        let dir = if std::fs::create_dir_all(&primary_dir).is_ok() {
            primary_dir
        } else {
            let fallback_dir = std::env::temp_dir().join("simply-transfer-vault");
            let _ = std::fs::create_dir_all(&fallback_dir);
            fallback_dir
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        if self.service_name == "com.simplytransfer.app" {
            dir.join("vault.enc")
        } else {
            let sanitized: String = self
                .service_name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            dir.join(format!("vault_{}.enc", sanitized))
        }
    }

    fn derive_vault_key(&self) -> [u8; 32] {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "simply-transfer-user".to_string());
        let mut hasher = Sha256::new();
        hasher.update(b"SimplyTransfer:NIST-SP-800-57:VaultKey:");
        hasher.update(self.service_name.as_bytes());
        hasher.update(b":");
        hasher.update(user.as_bytes());
        let result = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&result);
        key
    }

    fn load_encrypted_vault(&self) -> HashMap<String, String> {
        let path = self.get_vault_path();
        if !path.exists() {
            return HashMap::new();
        }
        let Ok(data) = std::fs::read(&path) else {
            return HashMap::new();
        };
        if data.len() < 12 {
            return HashMap::new();
        }
        let key = self.derive_vault_key();
        let cipher = Aes256Gcm::new_from_slice(&key).expect("32 bytes key");
        let nonce = Nonce::from_slice(&data[0..12]);
        let ciphertext = &data[12..];
        if let Ok(mut decrypted_bytes) = cipher.decrypt(nonce, ciphertext)
            && let Ok(json_str) = String::from_utf8(decrypted_bytes.clone())
        {
            decrypted_bytes.zeroize();
            if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&json_str) {
                return map;
            }
        }
        HashMap::new()
    }

    fn save_encrypted_vault(&self, map: &HashMap<String, String>) -> Result<(), CryptoError> {
        let path = self.get_vault_path();
        let json_bytes = serde_json::to_vec(map).map_err(|e| CryptoError::Other(e.to_string()))?;
        let key = self.derive_vault_key();
        let cipher = Aes256Gcm::new_from_slice(&key).expect("32 bytes key");
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, json_bytes.as_ref())
            .map_err(|e| CryptoError::Other(format!("AES-GCM encryption error: {}", e)))?;

        let mut payload = Vec::with_capacity(12 + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);

        // Write to a temp file in the same directory, then atomically rename into place
        // so readers never observe a partially written vault.
        let tmp_path = path.with_extension(format!("enc.tmp.{}", std::process::id()));
        std::fs::write(&tmp_path, &payload).map_err(|e| CryptoError::Other(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
        }
        if let Err(e) = std::fs::rename(&tmp_path, &path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(CryptoError::Other(e.to_string()));
        }
        Ok(())
    }

    /// Store a sensitive string securely. Synchronizes with both the OS keyring and the AES-256-GCM encrypted vault fallback
    /// so transient D-Bus Secret Service sessions (e.g. on Arch/CachyOS Linux) never lose keys across threads or restarts.
    pub fn set_secret(&self, account: &str, mut secret: String) -> Result<(), CryptoError> {
        let _guard = lock_vault();
        if let Ok(entry) = self.get_entry(account) {
            let _ = entry.set_password(&secret);
        }

        let mut vault = self.load_encrypted_vault();
        vault.insert(account.to_string(), secret.clone());
        let res = self.save_encrypted_vault(&vault);
        secret.zeroize();
        res
    }

    /// Retrieve a sensitive string. The caller is responsible for zeroizing it after use.
    pub fn get_secret(&self, account: &str) -> Result<String, CryptoError> {
        let _guard = lock_vault();
        if let Ok(entry) = self.get_entry(account)
            && let Ok(pwd) = entry.get_password()
            && !pwd.trim().is_empty()
        {
            return Ok(pwd);
        }
        let vault = self.load_encrypted_vault();
        if let Some(secret) = vault.get(account)
            && !secret.trim().is_empty()
        {
            return Ok(secret.clone());
        }
        Err(CryptoError::NotFound)
    }

    /// Delete a secret securely.
    pub fn delete_secret(&self, account: &str) -> Result<(), CryptoError> {
        let _guard = lock_vault();
        if let Ok(entry) = self.get_entry(account) {
            let _ = entry.delete_credential();
        }
        let mut vault = self.load_encrypted_vault();
        if vault.remove(account).is_some() {
            let _ = self.save_encrypted_vault(&vault);
        }
        Ok(())
    }

    /// Purge all stored secrets that are not referenced in `active_accounts`.
    /// Returns the number of orphaned keys purged.
    pub fn purge_orphaned_keys(&self, active_accounts: &[String]) -> Result<usize, CryptoError> {
        let _guard = lock_vault();
        let mut vault = self.load_encrypted_vault();
        let mut to_remove = Vec::new();
        for key in vault.keys() {
            let is_managed_key = key.starts_with("key_")
                || key.starts_with("AAAAC3NzaC1lZDI1NTE5")
                || key.starts_with("ssh-ed25519")
                || key.starts_with("wg_");
            if is_managed_key && !active_accounts.iter().any(|a| a == key) {
                to_remove.push(key.clone());
            }
        }
        let purged_count = to_remove.len();
        for key in &to_remove {
            if let Ok(entry) = self.get_entry(key) {
                let _ = entry.delete_credential();
            }
            vault.remove(key);
        }
        if purged_count > 0 {
            self.save_encrypted_vault(&vault)?;
        }
        Ok(purged_count)
    }

    /// Encrypts a sensitive value at rest using AES-256-GCM and returns a base64-encoded nonce+ciphertext.
    pub fn encrypt_value(&self, plaintext: &str) -> Result<String, CryptoError> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let key = self.derive_vault_key();
        let cipher = Aes256Gcm::new_from_slice(&key).expect("32 bytes key");
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| CryptoError::Other(format!("AES-GCM encryption error: {}", e)))?;

        let mut payload = Vec::with_capacity(12 + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);
        Ok(STANDARD.encode(&payload))
    }

    /// Decrypts a base64-encoded AES-256-GCM nonce+ciphertext produced by `encrypt_value`.
    pub fn decrypt_value(&self, encoded: &str) -> Result<String, CryptoError> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let data = STANDARD
            .decode(encoded)
            .map_err(|e| CryptoError::Other(format!("Base64 decode error: {}", e)))?;
        if data.len() < 12 {
            return Err(CryptoError::Other("Ciphertext too short".to_string()));
        }
        let key = self.derive_vault_key();
        let cipher = Aes256Gcm::new_from_slice(&key).expect("32 bytes key");
        let nonce = Nonce::from_slice(&data[0..12]);
        let ciphertext = &data[12..];
        let decrypted = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| CryptoError::Other(format!("AES-GCM decryption error: {}", e)))?;
        String::from_utf8(decrypted).map_err(|e| CryptoError::Other(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secure_storage() {
        let storage = SecureStorage::new("test.simplytransfer.crypto");
        let account = "test_user";

        let _ = storage.delete_secret(account);

        assert!(storage.get_secret(account).is_err());

        storage
            .set_secret(account, "super_secret".to_string())
            .unwrap();
        let retrieved = storage.get_secret(account).unwrap();
        assert_eq!(retrieved, "super_secret");

        storage.delete_secret(account).unwrap();
        assert!(storage.get_secret(account).is_err());

        // Test purge_orphaned_keys
        storage
            .set_secret("key_active_1", "secret_active".to_string())
            .unwrap();
        storage
            .set_secret("key_orphan_1", "secret_orphan".to_string())
            .unwrap();
        let purged = storage
            .purge_orphaned_keys(&["key_active_1".to_string()])
            .unwrap();
        assert!(purged >= 1);
        assert!(storage.get_secret("key_orphan_1").is_err());
        assert_eq!(storage.get_secret("key_active_1").unwrap(), "secret_active");
        storage.delete_secret("key_active_1").unwrap();
    }
}
pub mod keys;
pub use keys::{VpnConfig, WireGuardConf};
