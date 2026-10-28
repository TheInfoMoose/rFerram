//! Embedded P2P Out-of-Band (OOB) Ed25519 key ingestion manager (`russh` & `ed25519-dalek`).
//!
//! Ingests authorized peer Ed25519 public keys directly into memory and `NistVault`
//! without relying on any OS-level OpenSSH daemon (`sshd`) or `~/.ssh/authorized_keys` file.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::public::Ed25519PublicKey;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey, parse_public_key_base64};
use russh::server::{Auth, ChannelOpenHandle, Config, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use rferram_common::{NistVault, VaultTokenModel};

/// Prefix for in-band authenticated OOB Ed25519 key rotation/ingestion frames over a `russh` session channel.
pub const OOB_INGEST_FRAME_PREFIX: &str = "OOB_INGEST_ED25519:";

/// Default validity horizon (30 days) for OOB-ingested peer authorization tokens in `NistVault`.
const DEFAULT_OOB_TOKEN_TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// Metadata and cryptographic material for an authorized peer Ed25519 public key ingested out-of-band.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestedPeerKey {
    /// Logical peer or user identifier.
    pub peer_id: String,
    /// Raw 32-byte RFC 8032 Ed25519 verifying key material.
    pub public_key_bytes: [u8; 32],
    /// Canonical OpenSSH public key string (`"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5..."`).
    pub openssh_public_key: String,
    /// Canonical SHA-256 fingerprint (`"SHA256:..."`).
    pub sha256_fingerprint: String,
}

/// Embedded P2P Out-of-Band (OOB) Ed25519 Key Ingestion Manager & `russh` Server Handler.
///
/// Operates entirely in-memory and through [`NistVault`] without touching OS `sshd`
/// or `~/.ssh/authorized_keys`.
#[derive(Clone)]
pub struct OobKeyIngestionManager {
    authorized_peers: Arc<RwLock<HashMap<String, IngestedPeerKey>>>,
    vault: Arc<NistVault>,
    host_key: Arc<PrivateKey>,
}

impl Default for OobKeyIngestionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl OobKeyIngestionManager {
    /// Creates a new embedded OOB Ed25519 key ingestion manager backed by the default [`NistVault`].
    pub fn new() -> Self {
        Self::with_vault(NistVault::default())
    }

    /// Creates a new embedded OOB Ed25519 key ingestion manager with a custom [`NistVault`] namespace.
    pub fn with_vault(vault: NistVault) -> Self {
        let host_key = match vault.get_or_create_ed25519_identity("oob_ssh_host") {
            Ok((signing_key, _verifying_key)) => {
                let keypair = Ed25519Keypair::from(&signing_key);
                PrivateKey::from(keypair)
            }
            Err(_) => {
                let seed: [u8; 32] =
                    std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(11));
                let keypair = Ed25519Keypair::from_seed(&seed);
                PrivateKey::from(keypair)
            }
        };

        Self {
            authorized_peers: Arc::new(RwLock::new(HashMap::new())),
            vault: Arc::new(vault),
            host_key: Arc::new(host_key),
        }
    }

    /// Returns the embedded host's OpenSSH-formatted Ed25519 public key for peer pinning.
    pub fn host_public_key_openssh(&self) -> Result<String> {
        self.host_key
            .public_key()
            .to_openssh()
            .context("Failed to encode embedded host Ed25519 public key to OpenSSH format")
    }

    /// Builds an embedded [`russh::server::Config`] advertising public-key authentication only
    /// and using the in-memory Ed25519 host key.
    pub fn build_russh_server_config(&self) -> Arc<Config> {
        let config = Config {
            auth_rejection_time: Duration::from_millis(50),
            auth_rejection_time_initial: Some(Duration::from_millis(0)),
            methods: MethodSet::from(&[MethodKind::PublicKey][..]),
            keys: vec![(*self.host_key).clone()],
            inactivity_timeout: Some(Duration::from_secs(120)),
            nodelay: true,
            ..Default::default()
        };
        Arc::new(config)
    }

    /// Registers a peer's raw 32-byte Ed25519 verifying key in memory and persists an authorization
    /// record inside [`NistVault`].
    pub async fn register_peer_verifying_key(
        &self,
        peer_id: &str,
        public_key_bytes: [u8; 32],
    ) -> Result<IngestedPeerKey> {
        let trimmed_peer = peer_id.trim();
        if trimmed_peer.is_empty() {
            bail!("Peer identifier cannot be empty");
        }

        // Validate that the 32-byte key is a valid Ed25519 curve point via ed25519-dalek conversion.
        let ed_pub = Ed25519PublicKey(public_key_bytes);
        validate_ed25519_public_key_point(&ed_pub)?;

        let ssh_pub = PublicKey::from(ed_pub);
        let openssh_public_key = ssh_pub
            .to_openssh()
            .context("Failed to format Ed25519 public key as OpenSSH string")?;
        let sha256_fingerprint = ssh_pub.fingerprint(HashAlg::Sha256).to_string();

        let ingested = IngestedPeerKey {
            peer_id: trimmed_peer.to_string(),
            public_key_bytes,
            openssh_public_key: openssh_public_key.clone(),
            sha256_fingerprint: sha256_fingerprint.clone(),
        };

        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(DEFAULT_OOB_TOKEN_TTL_SECS);

        let token = VaultTokenModel {
            session_token: openssh_public_key,
            auth_method: "Ed25519+OOB-SSH".to_string(),
            expires_at,
        };

        self.vault
            .store_session_token(&Self::vault_profile_key(trimmed_peer), &token)
            .context("Failed to store OOB peer Ed25519 key token in NistVault")?;

        {
            let mut peers = self.authorized_peers.write().await;
            peers.insert(trimmed_peer.to_string(), ingested.clone());
        }

        info!(
            peer_id = %trimmed_peer,
            fingerprint = %sha256_fingerprint,
            "Ingested peer Ed25519 public key into OOB memory registry and NistVault"
        );

        Ok(ingested)
    }

    /// Parses an OpenSSH Ed25519 public key string (or raw base64 blob) and ingests it for `peer_id`.
    pub async fn register_peer_openssh_key(
        &self,
        peer_id: &str,
        openssh_key: &str,
    ) -> Result<IngestedPeerKey> {
        let parsed = parse_ed25519_ssh_public_key(openssh_key)?;
        self.register_peer_ssh_public_key(peer_id, &parsed).await
    }

    /// Extracts the 32-byte Ed25519 key from a [`russh::keys::PublicKey`] and registers it for `peer_id`.
    pub async fn register_peer_ssh_public_key(
        &self,
        peer_id: &str,
        public_key: &PublicKey,
    ) -> Result<IngestedPeerKey> {
        if public_key.algorithm() != Algorithm::Ed25519 {
            bail!(
                "Unsupported OOB SSH key algorithm {:?}; only Ed25519 is permitted",
                public_key.algorithm()
            );
        }

        let ed25519_data = public_key
            .key_data()
            .ed25519()
            .ok_or_else(|| anyhow!("Missing Ed25519 key payload in SSH PublicKey"))?;

        self.register_peer_verifying_key(peer_id, ed25519_data.0)
            .await
    }

    /// Revokes an authorized peer's Ed25519 key from memory and deletes its [`NistVault`] entry.
    pub async fn revoke_peer_key(&self, peer_id: &str) -> Result<bool> {
        let removed = {
            let mut peers = self.authorized_peers.write().await;
            peers.remove(peer_id.trim()).is_some()
        };
        let _ = self
            .vault
            .delete_session_token(&Self::vault_profile_key(peer_id.trim()));
        Ok(removed)
    }

    /// Looks up a registered peer's [`IngestedPeerKey`] by `peer_id`.
    pub async fn get_peer_key(&self, peer_id: &str) -> Option<IngestedPeerKey> {
        let peers = self.authorized_peers.read().await;
        peers.get(peer_id.trim()).cloned()
    }

    /// Returns a snapshot of all currently ingested peer Ed25519 keys.
    pub async fn list_authorized_peers(&self) -> Vec<IngestedPeerKey> {
        let peers = self.authorized_peers.read().await;
        peers.values().cloned().collect()
    }

    /// Checks whether a [`russh::keys::PublicKey`] presented during an SSH handshake matches an
    /// ingested Ed25519 peer key in memory.
    pub async fn is_ssh_key_authorized(&self, public_key: &PublicKey) -> bool {
        if public_key.algorithm() != Algorithm::Ed25519 {
            return false;
        }
        let Some(ed_pk) = public_key.key_data().ed25519() else {
            return false;
        };
        let fp = public_key.fingerprint(HashAlg::Sha256).to_string();

        let peers = self.authorized_peers.read().await;
        peers
            .values()
            .any(|peer| peer.public_key_bytes == ed_pk.0 && peer.sha256_fingerprint == fp)
    }

    /// Verifies a TLS 1.3 channel-bound Ed25519 handshake challenge signed by `peer_id` using
    /// the peer's OOB-ingested public key and [`NistVault::verify_channel_bound_challenge`].
    pub async fn verify_peer_channel_bound_challenge(
        &self,
        peer_id: &str,
        challenge: &[u8],
        tls_cert_sha256: &[u8; 32],
        signature_bytes: &[u8],
    ) -> Result<()> {
        let peer = self
            .get_peer_key(peer_id)
            .await
            .ok_or_else(|| anyhow!("No OOB Ed25519 public key registered for peer '{peer_id}'"))?;

        NistVault::verify_channel_bound_challenge(
            &peer.public_key_bytes,
            challenge,
            tls_cert_sha256,
            signature_bytes,
        )
        .context("OOB peer Ed25519 channel-bound challenge verification failed")
    }

    fn vault_profile_key(peer_id: &str) -> String {
        format!("oob_peer_{peer_id}")
    }
}

impl Server for OobKeyIngestionManager {
    type Handler = Self;

    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler {
        debug!(
            ?peer_addr,
            "Accepted incoming P2P OOB SSH client connection"
        );
        self.clone()
    }
}

impl Handler for OobKeyIngestionManager {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> std::result::Result<Auth, Self::Error> {
        if self.is_ssh_key_authorized(public_key).await {
            debug!(user = %user, "OOB SSH pre-auth public key probe accepted");
            Ok(Auth::Accept)
        } else {
            warn!(user = %user, "OOB SSH pre-auth public key probe rejected");
            Ok(Auth::reject())
        }
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> std::result::Result<Auth, Self::Error> {
        if self.is_ssh_key_authorized(public_key).await {
            let fp = public_key.fingerprint(HashAlg::Sha256);
            info!(
                user = %user,
                fingerprint = %fp,
                "Authenticated P2P OOB SSH peer via in-memory Ed25519 key registry"
            );
            Ok(Auth::Accept)
        } else {
            warn!(
                user = %user,
                "Rejected P2P OOB SSH authentication attempt with unauthorized key"
            );
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> std::result::Result<(), Self::Error> {
        debug!(channel_id = %channel.id(), "Opened P2P OOB SSH session channel");
        reply.accept().await;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> std::result::Result<(), Self::Error> {
        if let Ok(text) = std::str::from_utf8(data)
            && let Some(payload) = text.trim().strip_prefix(OOB_INGEST_FRAME_PREFIX)
            && let Some((peer_id, key_str)) = payload.split_once(':')
        {
            match self.register_peer_openssh_key(peer_id, key_str).await {
                Ok(ingested) => {
                    let ack = format!("OOB_OK:{}\n", ingested.sha256_fingerprint);
                    let _ = session.data(channel, ack.into_bytes());
                }
                Err(e) => {
                    warn!(error = %e, "Failed to ingest peer Ed25519 key over OOB SSH channel");
                    let err_msg = b"OOB_ERR:invalid_ed25519_key\n".to_vec();
                    let _ = session.data(channel, err_msg);
                }
            }
        }
        Ok(())
    }
}

/// Parses an Ed25519 [`PublicKey`] from either a full OpenSSH line (`"ssh-ed25519 AAAAC3Nza..."`)
/// or a raw base64 key blob.
pub fn parse_ed25519_ssh_public_key(input: &str) -> Result<PublicKey> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("Empty SSH public key string");
    }

    let pub_key = PublicKey::from_openssh(trimmed)
        .or_else(|_| PublicKey::from_openssh(&format!("ssh-ed25519 {trimmed}")))
        .or_else(|_| parse_public_key_base64(trimmed))
        .context("Failed to parse OpenSSH Ed25519 public key")?;

    if pub_key.algorithm() != Algorithm::Ed25519 {
        bail!("Expected ssh-ed25519 key, found {:?}", pub_key.algorithm());
    }

    Ok(pub_key)
}

/// Validates that an [`Ed25519PublicKey`] represents a valid Edwards curve point using `ed25519-dalek`.
fn validate_ed25519_public_key_point(ed_pub: &Ed25519PublicKey) -> Result<()> {
    // Check point decompression via NistVault's ed25519-dalek VerifyingKey parser:
    // an invalid curve point fails with "Invalid Ed25519 public key", whereas a valid point
    // with a dummy signature fails later at "Ed25519 challenge verification failed".
    if let Err(err) =
        NistVault::verify_channel_bound_challenge(&ed_pub.0, b"point-check", &[0u8; 32], &[0u8; 64])
    {
        let msg = err.to_string();
        if msg.contains("Invalid Ed25519 public key") {
            bail!("{msg}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_peer_ed25519_key_registration_and_verification() {
        let vault = NistVault::new("test.rferram.channels.oob_ssh");
        let (peer_signing, peer_verifying) = vault
            .get_or_create_ed25519_identity("oob_unit_test_peer_alpha")
            .expect("Should generate peer Ed25519 identity");

        let manager =
            OobKeyIngestionManager::with_vault(NistVault::new("test.rferram.channels.oob_ssh_mgr"));

        let ingested = manager
            .register_peer_verifying_key("peer-alpha", peer_verifying.to_bytes())
            .await
            .expect("Peer Ed25519 verifying key registration should succeed");

        assert_eq!(ingested.peer_id, "peer-alpha");
        assert_eq!(ingested.public_key_bytes, peer_verifying.to_bytes());
        assert!(ingested.openssh_public_key.starts_with("ssh-ed25519 "));
        assert!(ingested.sha256_fingerprint.starts_with("SHA256:"));

        // Verify in-memory lookup and list
        let fetched = manager
            .get_peer_key("peer-alpha")
            .await
            .expect("Registered peer key should be retrievable");
        assert_eq!(fetched, ingested);
        assert_eq!(manager.list_authorized_peers().await.len(), 1);

        // Verify TLS 1.3 channel-bound challenge verification using the ingested peer key
        let challenge = b"oob-p2p-session-challenge-nonce-32b";
        let tls_cert_hash = [0x5Au8; 32];
        let signature =
            NistVault::sign_channel_bound_challenge(&peer_signing, challenge, &tls_cert_hash);

        manager
            .verify_peer_channel_bound_challenge(
                "peer-alpha",
                challenge,
                &tls_cert_hash,
                &signature,
            )
            .await
            .expect("Valid peer signature must pass OOB verification");

        // Tampered TLS certificate fingerprint must be rejected
        let wrong_cert_hash = [0xFFu8; 32];
        assert!(
            manager
                .verify_peer_channel_bound_challenge(
                    "peer-alpha",
                    challenge,
                    &wrong_cert_hash,
                    &signature,
                )
                .await
                .is_err(),
            "Mismatched TLS certificate hash must fail verification"
        );

        // Revoking the peer key removes authorization
        assert!(
            manager
                .revoke_peer_key("peer-alpha")
                .await
                .expect("Revocation should succeed")
        );
        assert!(manager.get_peer_key("peer-alpha").await.is_none());
        assert!(
            manager
                .verify_peer_channel_bound_challenge(
                    "peer-alpha",
                    challenge,
                    &tls_cert_hash,
                    &signature,
                )
                .await
                .is_err(),
            "Revoked peer must fail verification"
        );
    }

    #[tokio::test]
    async fn test_openssh_key_parsing_and_russh_handler_auth() {
        let vault = NistVault::new("test.rferram.channels.oob_ssh_handler");
        let (_peer_signing, peer_verifying) = vault
            .get_or_create_ed25519_identity("oob_unit_test_peer_beta")
            .expect("Should generate peer Ed25519 identity");
        let (_other_signing, other_verifying) = vault
            .get_or_create_ed25519_identity("oob_unit_test_peer_unregistered")
            .expect("Should generate unregistered Ed25519 identity");

        let peer_ssh_pub = PublicKey::from(Ed25519PublicKey::from(&peer_verifying));
        let unregistered_ssh_pub = PublicKey::from(Ed25519PublicKey::from(&other_verifying));
        let openssh_str = peer_ssh_pub
            .to_openssh()
            .expect("Should encode Ed25519 public key to OpenSSH string");

        let mut manager = OobKeyIngestionManager::with_vault(NistVault::new(
            "test.rferram.channels.oob_ssh_handler_mgr",
        ));

        // Unregistered key is rejected by russh Handler::auth_publickey
        let initial_auth = manager
            .auth_publickey("peer-beta", &peer_ssh_pub)
            .await
            .unwrap();
        assert!(matches!(initial_auth, Auth::Reject { .. }));

        // Register via OpenSSH string
        let ingested = manager
            .register_peer_openssh_key("peer-beta", &openssh_str)
            .await
            .expect("Should parse and ingest OpenSSH Ed25519 key");
        assert_eq!(ingested.public_key_bytes, peer_verifying.to_bytes());

        // Registered key is accepted by russh Handler::auth_publickey_offered & auth_publickey
        let offered_auth = manager
            .auth_publickey_offered("peer-beta", &peer_ssh_pub)
            .await
            .unwrap();
        assert_eq!(offered_auth, Auth::Accept);

        let final_auth = manager
            .auth_publickey("peer-beta", &peer_ssh_pub)
            .await
            .unwrap();
        assert_eq!(final_auth, Auth::Accept);

        // Unregistered peer key is still rejected
        let other_auth = manager
            .auth_publickey("peer-unregistered", &unregistered_ssh_pub)
            .await
            .unwrap();
        assert!(matches!(other_auth, Auth::Reject { .. }));

        // Verify server config builds cleanly with embedded host key
        let server_cfg = manager.build_russh_server_config();
        assert_eq!(server_cfg.keys.len(), 1);
        assert!(
            manager
                .host_public_key_openssh()
                .expect("Host public key should format")
                .starts_with("ssh-ed25519 ")
        );

        let _ = manager.revoke_peer_key("peer-beta").await;
    }
}
