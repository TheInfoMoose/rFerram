use crate::CryptoError;
use crate::SecureStorage;
use rand_core_06::OsRng;
use ssh_key::PrivateKey;

pub struct KeyPairManager {
    storage: SecureStorage,
}

impl KeyPairManager {
    pub fn new(service_name: &str) -> Self {
        Self {
            storage: SecureStorage::new(service_name),
        }
    }

    /// Generates a new Ed25519 keypair, securely stores the OpenSSH private key PEM keyed by the public key,
    /// and returns the public key as a hex string (or OpenSSH string).
    pub fn generate_and_store(&self) -> Result<String, CryptoError> {
        let mut rng = OsRng;
        let priv_key = PrivateKey::random(&mut rng, ssh_key::Algorithm::Ed25519)
            .map_err(|e| CryptoError::Other(e.to_string()))?;

        let pub_key = priv_key
            .public_key()
            .to_openssh()
            .map_err(|e| CryptoError::Other(e.to_string()))?;
        let priv_pem = priv_key
            .to_openssh(ssh_key::LineEnding::LF)
            .map_err(|e| CryptoError::Other(e.to_string()))?;

        // Use the base64 part of the public key as the identifier to store the private key
        let parts: Vec<&str> = pub_key.split_whitespace().collect();
        let pub_id = if parts.len() >= 2 { parts[1] } else { &pub_key };

        self.storage.set_secret(pub_id, priv_pem.to_string())?;

        Ok(pub_id.to_string())
    }

    /// Retrieves the OpenSSH private key PEM string for a given connection_id (public key base64)
    pub fn get_private_key_pem(&self, connection_id: &str) -> Result<String, CryptoError> {
        self.storage.get_secret(connection_id)
    }

    /// Removes a keypair for a given connection_id
    pub fn delete_key(&self, connection_id: &str) -> Result<(), CryptoError> {
        self.storage.delete_secret(connection_id)
    }
}

/// Parsed representation of a standard WireGuard `.conf` configuration file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WireGuardConf {
    // [Interface]
    pub private_key: String,
    pub address: String,
    pub dns: Option<String>,
    pub mtu: Option<u16>,
    // [Peer]
    pub public_key: String,
    pub preshared_key: Option<String>,
    pub endpoint: String,
    pub allowed_ips: Vec<String>,
    pub persistent_keepalive: Option<u16>,
}

impl WireGuardConf {
    /// Parses a WireGuard `.conf` string into its `[Interface]` and `[Peer]` attributes.
    pub fn parse(conf_text: &str) -> Result<Self, CryptoError> {
        let mut section = "";
        let mut private_key: Option<String> = None;
        let mut address: Option<String> = None;
        let mut dns: Option<String> = None;
        let mut mtu: Option<u16> = None;

        let mut public_key: Option<String> = None;
        let mut preshared_key: Option<String> = None;
        let mut endpoint: Option<String> = None;
        let mut allowed_ips: Vec<String> = Vec::new();
        let mut persistent_keepalive: Option<u16> = None;

        for raw_line in conf_text.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = &line[1..line.len() - 1];
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let val = v.trim();

            if section.eq_ignore_ascii_case("Interface") {
                match key.to_ascii_lowercase().as_str() {
                    "privatekey" => private_key = Some(val.to_string()),
                    "address" => address = Some(val.to_string()),
                    "dns" => dns = Some(val.to_string()),
                    "mtu" => {
                        mtu = Some(val.parse::<u16>().map_err(|e| {
                            CryptoError::Other(format!("Invalid MTU '{}': {}", val, e))
                        })?)
                    }
                    _ => {}
                }
            } else if section.eq_ignore_ascii_case("Peer") {
                match key.to_ascii_lowercase().as_str() {
                    "publickey" => public_key = Some(val.to_string()),
                    "presharedkey" => preshared_key = Some(val.to_string()),
                    "endpoint" => endpoint = Some(val.to_string()),
                    "allowedips" => {
                        allowed_ips = val
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    "persistentkeepalive" => {
                        persistent_keepalive = Some(val.parse::<u16>().map_err(|e| {
                            CryptoError::Other(format!(
                                "Invalid PersistentKeepalive '{}': {}",
                                val, e
                            ))
                        })?)
                    }
                    _ => {}
                }
            }
        }

        Ok(Self {
            private_key: private_key.ok_or_else(|| {
                CryptoError::Other("Missing [Interface] PrivateKey in WireGuard .conf".into())
            })?,
            address: address.ok_or_else(|| {
                CryptoError::Other("Missing [Interface] Address in WireGuard .conf".into())
            })?,
            dns,
            mtu,
            public_key: public_key.ok_or_else(|| {
                CryptoError::Other("Missing [Peer] PublicKey in WireGuard .conf".into())
            })?,
            preshared_key,
            endpoint: endpoint.ok_or_else(|| {
                CryptoError::Other("Missing [Peer] Endpoint in WireGuard .conf".into())
            })?,
            allowed_ips,
            persistent_keepalive,
        })
    }
}

/// Encrypted-at-rest WireGuard VPN configuration stored on a `ConnectionProfile`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VpnConfig {
    pub enabled: bool,
    pub address: String,
    pub dns: Option<String>,
    pub mtu: Option<u16>,
    pub peer_public_key: String,
    pub endpoint: String,
    pub allowed_ips: Vec<String>,
    pub persistent_keepalive: Option<u16>,
    pub encrypted_private_key: String,
    pub encrypted_preshared_key: Option<String>,
    #[serde(default)]
    pub persistent_tunnel: bool,
}

impl VpnConfig {
    /// Creates a `VpnConfig` from a parsed `WireGuardConf`, encrypting `PrivateKey` and `PresharedKey` at rest.
    pub fn from_wireguard_conf(
        conf: &WireGuardConf,
        storage: &SecureStorage,
    ) -> Result<Self, CryptoError> {
        let encrypted_private_key = storage.encrypt_value(&conf.private_key)?;
        let encrypted_preshared_key = match &conf.preshared_key {
            Some(psk) => Some(storage.encrypt_value(psk)?),
            None => None,
        };

        // Also store under keyring/vault keyed by peer public key for secret lookup compatibility
        let key_acct = format!("wg_priv_{}", conf.public_key);
        let _ = storage.set_secret(&key_acct, conf.private_key.clone());
        if let Some(ref psk) = conf.preshared_key {
            let psk_acct = format!("wg_psk_{}", conf.public_key);
            let _ = storage.set_secret(&psk_acct, psk.clone());
        }

        Ok(Self {
            enabled: true,
            address: conf.address.clone(),
            dns: conf.dns.clone(),
            mtu: conf.mtu,
            peer_public_key: conf.public_key.clone(),
            endpoint: conf.endpoint.clone(),
            allowed_ips: conf.allowed_ips.clone(),
            persistent_keepalive: conf.persistent_keepalive,
            encrypted_private_key,
            encrypted_preshared_key,
            persistent_tunnel: false,
        })
    }

    /// Parses a WireGuard `.conf` string and encrypts its sensitive keys at rest.
    pub fn from_conf_str(conf_text: &str, storage: &SecureStorage) -> Result<Self, CryptoError> {
        let parsed = WireGuardConf::parse(conf_text)?;
        Self::from_wireguard_conf(&parsed, storage)
    }

    /// Decrypts the `PrivateKey` and optional `PresharedKey` for runtime tunnel setup.
    pub fn decrypt_keys(
        &self,
        storage: &SecureStorage,
    ) -> Result<(String, Option<String>), CryptoError> {
        let priv_key = storage.decrypt_value(&self.encrypted_private_key)?;
        let psk = match &self.encrypted_preshared_key {
            Some(enc_psk) => Some(storage.decrypt_value(enc_psk)?),
            None => None,
        };
        Ok((priv_key, psk))
    }

    /// Returns a masked representation of the peer's `PublicKey` suitable for UI display.
    pub fn masked_public_key(&self) -> String {
        let pk = self.peer_public_key.trim();
        if pk.len() <= 10 {
            pk.to_string()
        } else {
            format!("{}...{}", &pk[..6], &pk[pk.len() - 4..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wireguard_conf_parse_and_round_trip_encryption() {
        let sample_conf = r#"
[Interface]
PrivateKey = cHJpdmF0ZV9rZXlfYmFzZTY0X3Rlc3RfdmFsdWU=
Address = 10.66.66.2/32
DNS = 1.1.1.1
MTU = 1420

[Peer]
PublicKey = cHVibGljX2tleV9iYXNlNjRfdGVzdF92YWx1ZQ==
PresharedKey = cHJlc2hhcmVkX2tleV9iYXNlNjRfdGVzdF92YWx1ZQ==
Endpoint = vpn.example.com:51820
AllowedIPs = 10.66.66.1/32, 192.168.10.0/24
PersistentKeepalive = 25
"#;

        let parsed = WireGuardConf::parse(sample_conf).expect("should parse WireGuard .conf");
        assert_eq!(
            parsed.private_key,
            "cHJpdmF0ZV9rZXlfYmFzZTY0X3Rlc3RfdmFsdWU="
        );
        assert_eq!(parsed.address, "10.66.66.2/32");
        assert_eq!(parsed.dns.as_deref(), Some("1.1.1.1"));
        assert_eq!(parsed.mtu, Some(1420));
        assert_eq!(
            parsed.public_key,
            "cHVibGljX2tleV9iYXNlNjRfdGVzdF92YWx1ZQ=="
        );
        assert_eq!(
            parsed.preshared_key.as_deref(),
            Some("cHJlc2hhcmVkX2tleV9iYXNlNjRfdGVzdF92YWx1ZQ==")
        );
        assert_eq!(parsed.endpoint, "vpn.example.com:51820");
        assert_eq!(
            parsed.allowed_ips,
            vec!["10.66.66.1/32".to_string(), "192.168.10.0/24".to_string()]
        );
        assert_eq!(parsed.persistent_keepalive, Some(25));

        let storage = SecureStorage::new("test.simplytransfer.wg");
        let vpn_cfg =
            VpnConfig::from_wireguard_conf(&parsed, &storage).expect("should encrypt keys");

        // Ensure plaintext keys are not stored directly in VpnConfig
        assert_ne!(vpn_cfg.encrypted_private_key, parsed.private_key);
        assert_ne!(
            vpn_cfg.encrypted_preshared_key.as_deref(),
            parsed.preshared_key.as_deref()
        );

        // Verify round-trip decryption
        let (dec_priv, dec_psk) = vpn_cfg.decrypt_keys(&storage).expect("should decrypt keys");
        assert_eq!(dec_priv, parsed.private_key);
        assert_eq!(dec_psk, parsed.preshared_key);
    }
}
