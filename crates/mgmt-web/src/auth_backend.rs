//! The `axum-login` authentication backend: a single admin user whose credentials live in the
//! [`CredStore`]. The web UI is session-based (this backend); the native sync protocol is
//! bearer-token based (handled separately in [`crate::middleware`]).

use std::net::IpAddr;

use async_trait::async_trait;
use axum_login::{AuthUser, AuthnBackend, UserId};
use chrono::Utc;

use crate::auth::CredStore;

/// The authenticated admin. `auth_hash` is the password-hash bytes; axum-login compares it on every
/// request so changing the admin password invalidates existing sessions.
#[derive(Clone)]
pub struct AdminUser {
    pub id: String,
    auth_hash: Vec<u8>,
}

// A hand-written Debug that never prints the auth hash.
impl std::fmt::Debug for AdminUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminUser").field("id", &self.id).finish_non_exhaustive()
    }
}

impl AuthUser for AdminUser {
    type Id = String;

    fn id(&self) -> Self::Id {
        self.id.clone()
    }

    fn session_auth_hash(&self) -> &[u8] {
        &self.auth_hash
    }
}

/// Login credentials submitted to `POST /api/auth/login`.
#[derive(Clone)]
pub struct Credentials {
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
        Backend { creds }
    }

    pub fn admin_user(&self) -> AdminUser {
        AdminUser {
            id: mgmt_store::ADMIN_USER.to_string(),
            auth_hash: self.creds.admin_auth_hash(),
        }
    }
}

#[async_trait]
impl AuthnBackend for Backend {
    type User = AdminUser;
    type Credentials = Credentials;
    type Error = std::convert::Infallible;

    async fn authenticate(&self, creds: Self::Credentials) -> Result<Option<Self::User>, Self::Error> {
        let now = Utc::now();
        if self.creds.verify_credentials(&creds.password, creds.totp.as_deref(), now) {
            self.creds.clear_attempts(creds.ip);
            Ok(Some(self.admin_user()))
        } else {
            self.creds.record_failure(creds.ip, now);
            Ok(None)
        }
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        if user_id == mgmt_store::ADMIN_USER && self.creds.enabled() {
            Ok(Some(self.admin_user()))
        } else {
            Ok(None)
        }
    }
}

/// The `AuthSession` extractor specialized to our backend.
pub type AuthSession = axum_login::AuthSession<Backend>;
