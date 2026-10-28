//! NIST-compliant credential vault (`NistVault`, `CommonError`) and TLS 1.3 channel-bound Ed25519 signatures.

use crate::models::VaultTokenModel;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use simply_transfer_crypto::{CryptoError, SecureStorage};
use thiserror::Error;
use zeroize::Zeroize;

/// Domain separation prefix for TLS 1.3 channel-bound Ed25519 handshake challenges.
pub const CHANNEL_BINDING_DOMAIN_PREFIX: &[u8] = b"rFerram-v1-ChannelBinding:";

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

/// NIST-Compliant Credential Vault backed by OS Keyring (`keyring-core`) and
/// `simply-transfer-crypto::SecureStorage` (AES-256-GCM encrypted vault).
pub struct NistVault {
    pub(crate) storage: SecureStorage,
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
            let mut bytes = hex::decode(hex_seed.trim())
                .map_err(|e| CommonError::Crypto(format!("Invalid stored Ed25519 hex: {e}")))?;
            hex_seed.zeroize();
            let mut seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                bytes.zeroize();
                CommonError::Crypto("Ed25519 seed must be 32 bytes".to_string())
            })?;
            bytes.zeroize();
            let signing = SigningKey::from_bytes(&seed);
            seed.zeroize();
            let verifying = signing.verifying_key();
            return Ok((signing, verifying));
        }

        let mut seed: [u8; 32] = rand::random();
        let signing = SigningKey::from_bytes(&seed);
        let verifying = signing.verifying_key();
        let hex_encoded = hex::encode(seed);
        seed.zeroize();
        self.storage.set_secret(&key_name, hex_encoded)?;
        Ok((signing, verifying))
    }

    /// Signs a handshake challenge combined with a peer TLS certificate SHA-256 fingerprint.
    pub fn sign_channel_bound_challenge(
        signing_key: &SigningKey,
        challenge: &[u8],
        tls_cert_sha256: &[u8; 32],
    ) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(CHANNEL_BINDING_DOMAIN_PREFIX);
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
        let verifying_key = VerifyingKey::from_bytes(verifying_key_bytes)
            .map_err(|e| CommonError::Crypto(format!("Invalid Ed25519 public key: {e}")))?;
        let sig = Signature::from_slice(signature_bytes)
            .map_err(|e| CommonError::Crypto(format!("Invalid Ed25519 signature: {e}")))?;

        let mut hasher = Sha256::new();
        hasher.update(CHANNEL_BINDING_DOMAIN_PREFIX);
        hasher.update(challenge);
        hasher.update(tls_cert_sha256);
        let digest = hasher.finalize();

        verifying_key
            .verify(&digest, &sig)
            .map_err(|e| CommonError::Crypto(format!("Ed25519 challenge verification failed: {e}")))
    }
}
