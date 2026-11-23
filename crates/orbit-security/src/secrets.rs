use std::fmt;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand::RngCore;
use serde::Serialize;

use crate::SecurityError;

/// Opaque handle to secret material. Handles are safe to log and return to clients;
/// the value they point to never is.
pub type SecretRef = String;

/// Plaintext secret. `Debug` and `Display` are redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Serialize for SecretValue {
    fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom(
            "SecretValue must never be serialized",
        ))
    }
}

/// Storage abstraction for credentials.
#[async_trait::async_trait]
pub trait SecretStore: Send + Sync {
    async fn put(&self, reference: &str, value: SecretValue) -> Result<(), SecurityError>;
    async fn get(&self, reference: &str) -> Result<Option<SecretValue>, SecurityError>;
    async fn delete(&self, reference: &str) -> Result<(), SecurityError>;
}

/// AES-256-GCM envelope used by the default PostgreSQL secret store.
///
/// Format: `v1:<base64url(nonce || ciphertext)>`, with the secret reference bound as
/// associated data so ciphertext cannot be moved between rows.
pub struct SecretCipher {
    cipher: Aes256Gcm,
}

impl SecretCipher {
    pub fn from_base64_key(key: &str) -> Result<Self, SecurityError> {
        let raw = STANDARD
            .decode(key.trim())
            .map_err(|_| SecurityError::Secret("master key is not valid base64".into()))?;
        if raw.len() != 32 {
            return Err(SecurityError::Secret(format!(
                "master key must be 32 bytes, got {}",
                raw.len()
            )));
        }
        let key = Key::<Aes256Gcm>::from_slice(&raw);
        Ok(Self {
            cipher: Aes256Gcm::new(key),
        })
    }

    pub fn encrypt(&self, reference: &str, value: &SecretValue) -> Result<String, SecurityError> {
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(
                nonce,
                Payload {
                    msg: value.expose().as_bytes(),
                    aad: reference.as_bytes(),
                },
            )
            .map_err(|_| SecurityError::Secret("encryption failed".into()))?;
        let mut combined = Vec::with_capacity(12 + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);
        Ok(format!("v1:{}", STANDARD.encode(combined)))
    }

    pub fn decrypt(&self, reference: &str, sealed: &str) -> Result<SecretValue, SecurityError> {
        let encoded = sealed
            .strip_prefix("v1:")
            .ok_or_else(|| SecurityError::Secret("unsupported ciphertext version".into()))?;
        let combined = STANDARD
            .decode(encoded)
            .map_err(|_| SecurityError::Secret("invalid ciphertext encoding".into()))?;
        if combined.len() < 13 {
            return Err(SecurityError::Secret("ciphertext is too short".into()));
        }
        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let plaintext = self
            .cipher
            .decrypt(
                Nonce::from_slice(nonce_bytes),
                Payload {
                    msg: ciphertext,
                    aad: reference.as_bytes(),
                },
            )
            .map_err(|_| {
                SecurityError::Secret("decryption failed (wrong key or tampered)".into())
            })?;
        String::from_utf8(plaintext)
            .map(SecretValue::new)
            .map_err(|_| SecurityError::Secret("secret is not valid UTF-8".into()))
    }
}

impl fmt::Debug for SecretCipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretCipher(<key>)")
    }
}

/// In-memory store for tests and single-process development.
#[derive(Default, Debug)]
pub struct MemorySecretStore {
    cipher: Option<SecretCipher>,
    entries: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}

impl MemorySecretStore {
    pub fn new(cipher: Option<SecretCipher>) -> Self {
        Self {
            cipher,
            entries: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl SecretStore for MemorySecretStore {
    async fn put(&self, reference: &str, value: SecretValue) -> Result<(), SecurityError> {
        let stored = match &self.cipher {
            Some(cipher) => cipher.encrypt(reference, &value)?,
            None => value.expose().to_string(),
        };
        self.entries
            .lock()
            .expect("secret store lock")
            .insert(reference.to_string(), stored);
        Ok(())
    }

    async fn get(&self, reference: &str) -> Result<Option<SecretValue>, SecurityError> {
        let stored = self
            .entries
            .lock()
            .expect("secret store lock")
            .get(reference)
            .cloned();
        match stored {
            None => Ok(None),
            Some(value) => match &self.cipher {
                Some(cipher) => Ok(Some(cipher.decrypt(reference, &value)?)),
                None => Ok(Some(SecretValue::new(value))),
            },
        }
    }

    async fn delete(&self, reference: &str) -> Result<(), SecurityError> {
        self.entries
            .lock()
            .expect("secret store lock")
            .remove(reference);
        Ok(())
    }
}

pub fn new_secret_ref(prefix: &str) -> String {
    format!("{prefix}/{}", uuid::Uuid::now_v7())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> SecretCipher {
        // deterministic test key: 32 zero bytes
        SecretCipher::from_base64_key(&STANDARD.encode([0u8; 32])).unwrap()
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let cipher = cipher();
        let sealed = cipher
            .encrypt("provider/abc", &SecretValue::new("sk-test-123"))
            .unwrap();
        assert!(!sealed.contains("sk-test-123"));
        let opened = cipher.decrypt("provider/abc", &sealed).unwrap();
        assert_eq!(opened.expose(), "sk-test-123");
    }

    #[test]
    fn aad_binds_reference() {
        let cipher = cipher();
        let sealed = cipher
            .encrypt("provider/a", &SecretValue::new("value"))
            .unwrap();
        assert!(cipher.decrypt("provider/b", &sealed).is_err());
    }

    #[tokio::test]
    async fn memory_store_roundtrip() {
        let store = MemorySecretStore::new(Some(cipher()));
        store
            .put("conn/1", SecretValue::new("token"))
            .await
            .unwrap();
        assert_eq!(
            store.get("conn/1").await.unwrap().unwrap().expose(),
            "token"
        );
        store.delete("conn/1").await.unwrap();
        assert!(store.get("conn/1").await.unwrap().is_none());
    }
}
