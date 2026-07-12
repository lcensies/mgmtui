//! The `axum-login` authentication backend. The web UI is session-based (this backend); the native
//! sync protocol is bearer-token based (handled separately in [`crate::middleware`]).
//!
//! Multi-user: the backend authenticates by a login **identifier** (email) + password across every
//! managed user. An empty identifier (or `"admin"`) resolves to the deployment admin, whose password
//! lives in the top-level `AuthFile.password_hash` (backward compatible with single-admin setups);
//! any other identifier resolves to the [`WebUser`](crate::auth::WebUser) with that email. A user
//! can only log in once they have accepted their invite and set a password.

use std::net::IpAddr;

use async_trait::async_trait;
use axum_login::{AuthUser, AuthnBackend, UserId};
use chrono::Utc;

use crate::auth::CredStore;

/// The authenticated session user. `auth_hash` is the user's password-hash bytes; axum-login
/// compares it on every request so changing the password invalidates their existing sessions.
#[derive(Clone)]
pub struct SessionUser {
    pub id: String,
    auth_hash: Vec<u8>,
}

// A hand-written Debug that never prints the auth hash.
impl std::fmt::Debug for SessionUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionUser").field("id", &self.id).finish_non_exhaustive()
    }
}

impl AuthUser for SessionUser {
    type Id = String;

    fn id(&self) -> Self::Id {
        self.id.clone()
    }

    fn session_auth_hash(&self) -> &[u8] {
        &self.auth_hash
    }
}

/// Login credentials submitted to `POST /api/auth/login`. `email` is the login identifier (absent
/// or `"admin"` → the deployment admin); `password` + optional `totp` verify against that user.
#[derive(Clone)]
pub struct Credentials {
    pub email: Option<String>,
    pub password: String,
    pub totp: Option<String>,
    pub ip: IpAddr,
}

/// The axum-login backend, backed by the shared credential store.
#[derive(Clone)]
pub struct Backend {
    creds: CredStore,
}

impl Backend {
    pub fn new(creds: CredStore) -> Self {
        Self { creds }
    }

    /// Build a [`SessionUser`] for `id` (admin or a managed user).
    pub fn session_user(&self, id: &str) -> SessionUser {
        SessionUser { id: id.to_string(), auth_hash: self.creds.user_auth_hash(id) }
    }

    /// Convenience: the admin session user (used to log in a freshly-claimed admin at first-run).
    pub fn admin_user(&self) -> SessionUser {
        self.session_user(mgmt_store::ADMIN_USER)
    }
}

#[async_trait]
impl AuthnBackend for Backend {
    type User = SessionUser;
    type Credentials = Credentials;
    type Error = std::convert::Infallible;

    async fn authenticate(&self, creds: Self::Credentials) -> Result<Option<Self::User>, Self::Error> {
        let now = Utc::now();
        let ident = creds.email.as_deref().unwrap_or("").trim().to_lowercase();
        // Empty/"admin" identifier → the deployment admin (legacy single-admin login).
        let id = if ident.is_empty() || ident == mgmt_store::ADMIN_USER {
            mgmt_store::ADMIN_USER.to_string()
        } else {
            match self.creds.user_by_email(&ident) {
                // Only users who have set a password (accepted their invite) can log in.
                Some(u) if u.can_login() => u.id,
                _ => {
                    self.creds.record_failure(creds.ip, now);
                    return Ok(None);
                }
            }
        };
        if self.creds.verify_user_credentials(&id, &creds.password, creds.totp.as_deref(), now) {
            self.creds.clear_attempts(creds.ip);
            Ok(Some(self.session_user(&id)))
        } else {
            self.creds.record_failure(creds.ip, now);
            Ok(None)
        }
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        let id = user_id.as_ref();
        // The admin is a valid session target only while a password is configured.
        if id == mgmt_store::ADMIN_USER && self.creds.enabled() {
            return Ok(Some(self.session_user(id)));
        }
        // A managed user is valid while they keep a password set.
        if self.creds.user_by_id(id).map(|u| u.can_login()).unwrap_or(false) {
            return Ok(Some(self.session_user(id)));
        }
        Ok(None)
    }
}

/// The `AuthSession` extractor specialized to our backend.
pub type AuthSession = axum_login::AuthSession<Backend>;
