//! Static bearer-key authentication. Runs are scoped to the key that created them.

use crate::error::AppError;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Identifies the API key a request authenticated with: a short hash, never the key itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner(pub String);

#[derive(Clone)]
pub struct AuthKeys(Arc<Vec<(Owner, [u8; 32])>>);

impl AuthKeys {
    pub fn new(keys: &[String]) -> Self {
        let entries = keys
            .iter()
            .map(|key| {
                let digest: [u8; 32] = Sha256::digest(key.as_bytes()).into();
                (Owner(hex::encode(&digest[..8])), digest)
            })
            .collect();
        Self(Arc::new(entries))
    }

    /// Compare digests rather than raw keys so the comparison is constant-time regardless of the
    /// key's length, and check every key so timing doesn't reveal which one matched.
    pub fn authenticate(&self, token: &str) -> Option<Owner> {
        let presented: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut found = None;
        for (owner, digest) in self.0.iter() {
            if bool::from(presented.ct_eq(digest)) {
                found = Some(owner.clone());
            }
        }
        found
    }
}

pub async fn require_auth(State(keys): State<AuthKeys>, mut req: Request, next: Next) -> Result<Response, AppError> {
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim());
    let owner = token.and_then(|t| keys.authenticate(t)).ok_or(AppError::Unauthorized)?;
    req.extensions_mut().insert(owner);
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_configured_keys() {
        let keys = AuthKeys::new(&["alpha".into(), "beta".into()]);
        let a = keys.authenticate("alpha").unwrap();
        let b = keys.authenticate("beta").unwrap();
        assert_ne!(a, b);
        assert_eq!(keys.authenticate("alpha"), Some(a));
        assert!(keys.authenticate("gamma").is_none());
        assert!(keys.authenticate("").is_none());
        assert!(keys.authenticate("alph").is_none());
    }

    #[test]
    fn owner_id_does_not_contain_the_key() {
        let keys = AuthKeys::new(&["super-secret-key".into()]);
        let owner = keys.authenticate("super-secret-key").unwrap();
        assert_eq!(owner.0.len(), 16);
        assert!(!owner.0.contains("secret"));
    }
}
