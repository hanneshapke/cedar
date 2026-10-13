//! Integration credentials saved through the admin API, encrypted at rest with
//! XChaCha20-Poly1305 under `CEDAR_SECRET_KEY`. Each value has a fresh random nonce, and its name
//! is bound in as associated data, so a ciphertext can't be moved to another name.

use crate::store::Store;
use anyhow::{anyhow, Result};
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde_json::Value;
use sqlx::Row;

pub const BRAVE_API_KEY: &str = "brave_api_key";
pub const GOOGLE_REFRESH_TOKEN: &str = "google_refresh_token";
pub const LLM_BASE_URL: &str = "llm_base_url";
pub const LLM_API_KEY: &str = "llm_api_key";

#[derive(Clone)]
pub struct SecretStore {
    store: Store,
    cipher: XChaCha20Poly1305,
}

impl SecretStore {
    pub fn new(store: Store, key: &[u8; 32]) -> Self {
        Self {
            store,
            cipher: XChaCha20Poly1305::new(key.into()),
        }
    }

    /// Encrypt and save `value` under `name`, with `meta` (shown to admins, so never secret).
    pub async fn put(&self, name: &str, value: &str, meta: &Value) -> Result<()> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: value.as_bytes(),
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("encrypting {name}"))?;
        sqlx::query(
            "INSERT INTO secrets (name, nonce, ciphertext, meta, updated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT (name) DO UPDATE SET nonce = excluded.nonce, ciphertext = excluded.ciphertext, \
             meta = excluded.meta, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(nonce.as_slice())
        .bind(ciphertext)
        .bind(meta.to_string())
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    /// The decrypted value, or `None` if nothing is saved under `name`. Fails if the value can't
    /// be decrypted (a different `CEDAR_SECRET_KEY`, or a tampered row).
    pub async fn get(&self, name: &str) -> Result<Option<String>> {
        let Some(row) = sqlx::query("SELECT nonce, ciphertext FROM secrets WHERE name = ?")
            .bind(name)
            .fetch_optional(self.store.pool())
            .await?
        else {
            return Ok(None);
        };
        let nonce: Vec<u8> = row.try_get("nonce")?;
        let ciphertext: Vec<u8> = row.try_get("ciphertext")?;
        if nonce.len() != 24 {
            return Err(anyhow!("stored {name} has a malformed nonce"));
        }
        let plain = self
            .cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| {
                anyhow!("cannot decrypt the saved {name}: was CEDAR_SECRET_KEY changed?")
            })?;
        Ok(Some(String::from_utf8(plain)?))
    }

    /// The non-secret details saved with `name`, if it is saved.
    pub async fn meta(&self, name: &str) -> Result<Option<Value>> {
        let row = sqlx::query("SELECT meta FROM secrets WHERE name = ?")
            .bind(name)
            .fetch_optional(self.store.pool())
            .await?;
        row.map(|r| Ok(serde_json::from_str(r.try_get("meta")?)?))
            .transpose()
    }

    /// Remove `name`. Returns whether anything was saved under it.
    pub async fn delete(&self, name: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM secrets WHERE name = ?")
            .bind(name)
            .execute(self.store.pool())
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

/// The last four characters of a secret, to recognise it by.
pub fn last4(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn values_round_trip_and_are_stored_encrypted() {
        let store = Store::connect_memory().await.unwrap();
        let secrets = SecretStore::new(store.clone(), &[7; 32]);
        assert_eq!(secrets.get(BRAVE_API_KEY).await.unwrap(), None);

        secrets
            .put(BRAVE_API_KEY, "brave-key-1234", &json!({ "last4": "1234" }))
            .await
            .unwrap();
        assert_eq!(
            secrets.get(BRAVE_API_KEY).await.unwrap().as_deref(),
            Some("brave-key-1234")
        );
        assert_eq!(
            secrets.meta(BRAVE_API_KEY).await.unwrap(),
            Some(json!({ "last4": "1234" }))
        );
        let raw: Vec<u8> = sqlx::query_scalar("SELECT ciphertext FROM secrets")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("brave-key"));

        // Replacing keeps one row.
        secrets
            .put(BRAVE_API_KEY, "other", &json!({}))
            .await
            .unwrap();
        assert_eq!(
            secrets.get(BRAVE_API_KEY).await.unwrap().as_deref(),
            Some("other")
        );

        assert!(secrets.delete(BRAVE_API_KEY).await.unwrap());
        assert!(!secrets.delete(BRAVE_API_KEY).await.unwrap());
        assert_eq!(secrets.get(BRAVE_API_KEY).await.unwrap(), None);
    }

    #[tokio::test]
    async fn another_key_or_another_name_cannot_decrypt() {
        let store = Store::connect_memory().await.unwrap();
        SecretStore::new(store.clone(), &[1; 32])
            .put(GOOGLE_REFRESH_TOKEN, "token", &json!({}))
            .await
            .unwrap();
        let err = SecretStore::new(store.clone(), &[2; 32])
            .get(GOOGLE_REFRESH_TOKEN)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("CEDAR_SECRET_KEY"), "{err}");

        // A ciphertext copied to another name fails too.
        sqlx::query(
            "INSERT INTO secrets SELECT ?, nonce, ciphertext, meta, updated_at FROM secrets",
        )
        .bind(BRAVE_API_KEY)
        .execute(store.pool())
        .await
        .unwrap();
        assert!(SecretStore::new(store, &[1; 32])
            .get(BRAVE_API_KEY)
            .await
            .is_err());
    }

    #[test]
    fn last4_handles_short_values() {
        assert_eq!(last4("abcdef"), "cdef");
        assert_eq!(last4("ab"), "ab");
    }
}
