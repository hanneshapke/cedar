//! Static bearer-key authentication. Runs are scoped to the key that created them, or, when a
//! service key acts for one of its users (`X-Cedar-User`), to that user.

use crate::error::AppError;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// The header a service key (`CEDAR_SERVICE_KEYS`) names the user it acts for in: a UUID.
pub const USER_HEADER: &str = "x-cedar-user";

/// Whose a run, a memory or a record is: the API key the request authenticated with (a short
/// hash, never the key itself), or the user a service key acted for (the user's UUID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner(pub String);

impl Owner {
    /// The owner a service key's user has: the UUID, lowercase and hyphenated. All service keys
    /// share these, so a user keeps what is theirs when the key in front of them is replaced.
    pub fn user(id: Uuid) -> Self {
        Owner(id.as_hyphenated().to_string())
    }

    /// Whether this is a user a service key acted for, rather than a key.
    pub fn is_user(owner: &str) -> bool {
        owner.len() == 36 && Uuid::parse_str(owner).is_ok()
    }
}

/// Whether the request's key may act for users (it is in `CEDAR_SERVICE_KEYS`): name one in
/// `X-Cedar-User`, and follow every user's finished runs (`GET /v1/events?users=all`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Service(pub bool);

/// Whether the request's key may use the admin routes (it is in `CEDAR_ADMIN_KEYS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admin(pub bool);

#[derive(Clone)]
pub struct AuthKeys(Arc<Vec<Key>>);

struct Key {
    owner: Owner,
    digest: [u8; 32],
    admin: bool,
    service: bool,
}

impl AuthKeys {
    pub fn new(keys: &[String]) -> Self {
        Self::with_admins(keys, &[])
    }

    /// `admins` is the subset of `keys` that may use the admin routes.
    pub fn with_admins(keys: &[String], admins: &[String]) -> Self {
        Self::with_roles(keys, admins, &[])
    }

    /// `services` is the subset of `keys` that may act for users.
    pub fn with_roles(keys: &[String], admins: &[String], services: &[String]) -> Self {
        let entries = keys
            .iter()
            .map(|key| {
                let digest: [u8; 32] = Sha256::digest(key.as_bytes()).into();
                Key {
                    owner: Owner(hex::encode(&digest[..8])),
                    digest,
                    admin: admins.contains(key),
                    service: services.contains(key),
                }
            })
            .collect();
        Self(Arc::new(entries))
    }

    pub fn authenticate(&self, token: &str) -> Option<Owner> {
        self.authenticate_role(token).map(|(owner, _)| owner)
    }

    /// Compare digests rather than raw keys so the comparison is constant-time regardless of the
    /// key's length, and check every key so timing doesn't reveal which one matched.
    pub fn authenticate_role(&self, token: &str) -> Option<(Owner, Admin)> {
        self.authenticate_roles(token).map(|(owner, admin, _)| (owner, admin))
    }

    pub fn authenticate_roles(&self, token: &str) -> Option<(Owner, Admin, Service)> {
        let presented: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut found = None;
        for key in self.0.iter() {
            if bool::from(presented.ct_eq(&key.digest)) {
                found = Some((key.owner.clone(), Admin(key.admin), Service(key.service)));
            }
        }
        found
    }
}

pub async fn require_auth(
    State(keys): State<AuthKeys>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim());
    let (mut owner, admin, service) = token
        .and_then(|t| keys.authenticate_roles(t))
        .ok_or(AppError::Unauthorized)?;
    // A service key acting for one of its users: everything the request reads and writes is that
    // user's. Without the header it acts as itself, like any other key. A key that isn't a
    // service key is refused rather than answered as itself, which would mix users up silently.
    if let Some(raw) = req.headers().get(USER_HEADER) {
        if !service.0 {
            return Err(AppError::Forbidden(
                "this API key may not act for users (see CEDAR_SERVICE_KEYS)",
            ));
        }
        let user = raw
            .to_str()
            .ok()
            .and_then(|v| Uuid::parse_str(v.trim()).ok())
            .ok_or_else(|| AppError::BadRequest(format!("{USER_HEADER} must be a UUID")))?;
        owner = Owner::user(user);
    }
    req.extensions_mut().insert(owner);
    req.extensions_mut().insert(admin);
    req.extensions_mut().insert(service);
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
    fn only_admin_keys_are_admins() {
        let keys = AuthKeys::with_admins(&["alpha".into(), "beta".into()], &["beta".into()]);
        assert_eq!(keys.authenticate_role("alpha").unwrap().1, Admin(false));
        assert_eq!(keys.authenticate_role("beta").unwrap().1, Admin(true));
        assert!(keys.authenticate_role("gamma").is_none());
    }

    #[test]
    fn only_service_keys_are_services() {
        let keys = AuthKeys::with_roles(&["alpha".into(), "beta".into()], &[], &["beta".into()]);
        assert_eq!(keys.authenticate_roles("alpha").unwrap().2, Service(false));
        assert_eq!(keys.authenticate_roles("beta").unwrap().2, Service(true));
        // A user's owner can't be mistaken for a key's.
        let user = Owner::user(Uuid::nil());
        assert_eq!(user.0, "00000000-0000-0000-0000-000000000000");
        assert_ne!(user.0.len(), keys.authenticate("alpha").unwrap().0.len());
    }

    #[test]
    fn owner_id_does_not_contain_the_key() {
        let keys = AuthKeys::new(&["super-secret-key".into()]);
        let owner = keys.authenticate("super-secret-key").unwrap();
        assert_eq!(owner.0.len(), 16);
        assert!(!owner.0.contains("secret"));
    }
}
