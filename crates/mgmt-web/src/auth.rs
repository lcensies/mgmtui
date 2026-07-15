//! Credential model + primitives for the web server.
//!
//! Sessions and login cookies are handled by [`tower_sessions`] + [`axum_login`] (see
//! [`crate::auth_backend`]); this module owns the *credential store*: [`CredStore`], an async facade
//! over the SQLite-backed [`crate::authdb::Db`] (admin credential, managed users, hashed tokens,
//! login rate limiting, TOTP replay guard) plus Argon2 password + TOTP verification. Credentials
//! live in `<data_root>/.state/web-auth.db` (never in the shareable config), migrated once from a
//! legacy `web-auth.yaml`.

use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use mgmt_core::{Error, Result};

use crate::db::{CredRepo, SqliteRepo};

const LOGIN_WINDOW_SECS: i64 = 60;
const LOGIN_MAX_ATTEMPTS: u32 = 5;
const LOGIN_LOCKOUT_SECS: i64 = 300;

/// One provisioned API token (the raw token is shown once; only its SHA-256 is stored).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenEntry {
    pub name: String,
    /// Lowercase hex SHA-256 of the token.
    pub hash: String,
}

/// A managed user. Each owns an isolated vault at `<data_root>/users/<id>` reached over `/api/sync`
/// with one of its scoped bearer `tokens`, and can also log into the web UI directly with `email` +
/// a per-user `password_hash` set by accepting an invite. `is_admin` marks the provisioned admin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebUser {
    /// Stable id and vault directory name (`users/<id>`).
    pub id: String,
    /// Human-friendly display name.
    #[serde(default)]
    pub name: String,
    /// Login identifier (email). Required for per-user web login + invite delivery.
    #[serde(default)]
    pub email: Option<String>,
    /// Per-user web-login password (Argon2 PHC). Absent until the user accepts their invite (or a
    /// sync-only user with no web login).
    #[serde(default)]
    pub password_hash: Option<String>,
    /// Optional per-user TOTP secret (2FA is opt-in per user).
    #[serde(default)]
    pub totp_secret: Option<String>,
    /// A pending one-time invite token (SHA-256 hex). Present until the user sets a password.
    #[serde(default)]
    pub invite_token: Option<String>,
    /// Whether this user is the deployment admin (owns other users).
    #[serde(default)]
    pub is_admin: bool,
    /// Scoped sync tokens (only their SHA-256 is stored; the raw token is shown once).
    #[serde(default)]
    pub tokens: Vec<TokenEntry>,
}

impl WebUser {
    /// True when this user can log into the web UI (has a password set).
    pub fn can_login(&self) -> bool {
        self.password_hash.is_some()
    }
}

/// A Google Cloud OAuth client (Web-application type) used by the web "Connect Google" flow. The
/// user creates it once in Google Cloud and registers `<public_origin>/api/oauth/google/callback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleOAuth {
    pub client_id: String,
    pub client_secret: String,
}

/// Legacy on-disk credentials file (`web-auth.yaml`). Retained only to import into the DB on first
/// start (and for the migration test); the DB is the source of truth thereafter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totp_secret: Option<String>,
    pub api_tokens: Vec<TokenEntry>,
    pub users: Vec<WebUser>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_oauth: Option<GoogleOAuth>,
}

impl AuthFile {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(AuthFile::default());
        }
        let text = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&text).map_err(|e| Error::Parse(format!("parsing {}: {e}", path.display())))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_yaml::to_string(self).map_err(|e| Error::Other(format!("serializing auth file: {e}")))?;
        mgmt_config::write_private(path, &text)
    }
}

/// The shared credential store: an async SQLite-backed [`Db`] plus the process-level open/setup
/// flag. Cheaply cloneable (it is an `Arc` internally) so both the axum-login backend and the
/// request guard can hold it.
#[derive(Clone)]
pub struct CredStore {
    inner: Arc<Inner>,
}

struct Inner {
    repo: Arc<dyn CredRepo>,
    /// When true the server runs unauthenticated (loopback dev / `--no-auth`). When false and no
    /// password is set, the server is in first-run *setup* mode (locked until claimed). This is a
    /// runtime flag from CLI args, not persistent data, so it stays in memory.
    allow_open: AtomicBool,
}

impl CredStore {
    /// Open the SQLite credential store at `db_path`, importing a legacy `web-auth.yaml` (at
    /// `legacy`) once when the store has no admin yet.
    pub async fn open(db_path: &Path, legacy: Option<&Path>) -> Result<Self> {
        let repo = SqliteRepo::open(db_path).await?;
        if let Some(yaml) = legacy {
            let file = AuthFile::load(yaml)?;
            if repo.import_legacy(&file).await? {
                println!("imported legacy web-auth.yaml into {} (the yaml is left as a backup)", db_path.display());
            }
        }
        Ok(Self::with_repo(Arc::new(repo), false))
    }

    /// A disabled (open) in-memory store — used in tests and loopback dev with no persistence.
    pub async fn disabled() -> Result<Self> {
        Ok(Self::with_repo(Arc::new(SqliteRepo::memory().await?), true))
    }

    /// Wrap any [`CredRepo`] backend. The auth logic depends only on the trait, so a different
    /// store (Postgres, a test fake) drops in here.
    pub fn with_repo(repo: Arc<dyn CredRepo>, allow_open: bool) -> Self {
        CredStore { inner: Arc::new(Inner { repo, allow_open: AtomicBool::new(allow_open) }) }
    }

    /// Set whether a passwordless server may run unauthenticated (loopback / `--no-auth`).
    pub fn set_open_mode(&self, open: bool) {
        self.inner.allow_open.store(open, Ordering::SeqCst);
    }

    fn allow_open(&self) -> bool {
        self.inner.allow_open.load(Ordering::SeqCst)
    }

    /// Whether a password is configured (auth enforced).
    pub async fn enabled(&self) -> bool {
        self.inner.repo.admin_password_hash().await.ok().flatten().is_some()
    }

    /// Whether the server currently runs unauthenticated (no password + open mode).
    pub async fn is_open(&self) -> bool {
        self.allow_open() && !self.enabled().await
    }

    /// Whether the server is waiting for an admin to be claimed via the first-run setup flow.
    pub async fn needs_setup(&self) -> bool {
        !self.allow_open() && !self.enabled().await
    }

    /// Whether a TOTP secret is enrolled on the admin (2FA is opt-in; off unless explicitly set).
    pub async fn totp_enrolled(&self) -> bool {
        self.inner.repo.admin_totp_secret().await.ok().flatten().is_some()
    }

    // ---- bearer resolution (native sync) -------------------------------------------

    /// Resolve a bearer token to the id of the user that owns it. Admin-owned tokens map to
    /// [`mgmt_store::ADMIN_USER`]; a user's scoped token maps to that user's id.
    pub async fn resolve_bearer(&self, token: &str) -> Option<String> {
        let hash = sha256_hex(token.as_bytes());
        self.inner.repo.resolve_token(&hash).await.ok().flatten()
    }

    // ---- password / TOTP verification (used by the axum-login backend) -------------

    /// Verify the admin password (+ TOTP iff enrolled). No rate limiting here — the login handler
    /// wraps this with [`CredStore::locked_secs`]/[`record_failure`]/[`clear_attempts`].
    pub async fn verify_credentials(&self, password: &str, totp: Option<&str>, now: DateTime<Utc>) -> bool {
        self.verify_user_credentials(mgmt_store::ADMIN_USER, password, totp, now).await
    }

    /// The admin's session-auth hash (the password-hash bytes), so changing the password
    /// invalidates existing sessions (axum-login's `session_auth_hash`).
    pub async fn admin_auth_hash(&self) -> Vec<u8> {
        self.user_auth_hash(mgmt_store::ADMIN_USER).await
    }

    // ---- per-user web login (email + password, set via invite) -------------------

    /// Look up a non-admin user by login identifier (email, case-insensitive).
    pub async fn user_by_email(&self, email: &str) -> Option<WebUser> {
        self.inner.repo.user_by_email(email).await.ok().flatten()
    }

    /// Find a user (admin or not) by id. The admin is synthesized so the login/session path treats
    /// admin and regular users uniformly.
    pub async fn user_by_id(&self, id: &str) -> Option<WebUser> {
        self.inner.repo.user_by_id(id).await.ok().flatten()
    }

    /// Verify a user's password (+ TOTP iff enrolled, with replay protection). For the admin, the
    /// top-level password; for others, their own `password_hash`.
    pub async fn verify_user_credentials(&self, id: &str, password: &str, totp: Option<&str>, now: DateTime<Utc>) -> bool {
        let Some((pw_hash, totp_secret)) = self.inner.repo.user_creds(id).await.ok().flatten() else {
            return false;
        };
        // Password first: a failed password must not consume the TOTP step, or an attacker could
        // burn a legitimate user's current code without knowing the password.
        if !pw_hash.as_deref().map(|h| verify_password(h, password)).unwrap_or(false) {
            return false;
        }
        match totp_secret {
            Some(secret) => self.consume_totp(id, &secret, totp, now).await,
            None => true,
        }
    }

    /// TOTP check with replay protection: the code must verify AND its time step must be newer than
    /// the last step this user consumed, so a sniffed code can't be replayed in the window.
    async fn consume_totp(&self, id: &str, secret: &str, code: Option<&str>, now: DateTime<Utc>) -> bool {
        let Some(step) = code.and_then(|c| totp_step(secret, c, now)) else { return false };
        self.inner.repo.consume_totp_step(id, step).await.unwrap_or(false)
    }

    /// Session-auth hash for a user (their password-hash bytes), so a password change invalidates
    /// their sessions.
    pub async fn user_auth_hash(&self, id: &str) -> Vec<u8> {
        self.inner
            .repo
            .user_creds(id)
            .await
            .ok()
            .flatten()
            .and_then(|(pw, _)| pw)
            .map(|h| h.into_bytes())
            .unwrap_or_default()
    }

    /// Whether a user has a pending (unaccepted) invite.
    pub async fn has_pending_invite(&self, id: &str) -> bool {
        self.inner.repo.has_pending_invite(id).await.unwrap_or(false)
    }

    /// Mint a one-time invite token for `id` and return the raw token (its SHA-256 is stored).
    pub async fn mint_invite(&self, id: &str) -> Result<String> {
        let (raw, hash) = new_api_token();
        if self.inner.repo.set_user_invite(id, &hash).await? {
            Ok(raw)
        } else {
            Err(Error::NotFound(format!("user {id}")))
        }
    }

    /// Consume an invite token and set the user's password, returning the user id.
    pub async fn accept_invite(&self, token: &str, password: &str) -> Result<String> {
        let hash = sha256_hex(token.as_bytes());
        let phc = hash_password(password)?;
        self.inner
            .repo
            .accept_invite(&hash, &phc)
            .await?
            .ok_or_else(|| Error::NotFound("invalid or expired invite token".into()))
    }

    /// Validate an invite token without consuming it: return the user it's for (None if invalid).
    pub async fn invite_owner(&self, token: &str) -> Option<WebUser> {
        let hash = sha256_hex(token.as_bytes());
        self.inner.repo.user_by_invite(&hash).await.ok().flatten()
    }

    // ---- first-run setup + runtime credential management ---------------------------

    /// Claim the admin account: set the password (and optionally enroll a TOTP secret). Atomic — it
    /// fails if an admin password is already set, so first-run setup can only ever succeed once.
    pub async fn set_admin_password(&self, password: &str, totp_secret: Option<&str>) -> Result<()> {
        let hash = hash_password(password)?;
        if self.inner.repo.set_admin_password_if_unset(&hash, totp_secret).await? {
            Ok(())
        } else {
            Err(Error::Other("admin already configured".into()))
        }
    }

    /// Set the admin password from a pre-computed Argon2 hash iff none is set (the
    /// `MGMT_WEB_PASSWORD_HASH` bootstrap path). Returns whether it was applied.
    pub async fn set_admin_password_hash(&self, hash: &str) -> Result<bool> {
        self.inner.repo.set_admin_password_if_unset(hash, None).await
    }

    /// Force-set the admin password (overwrite; the `mgmt web setpass` reset path).
    pub async fn force_set_admin_password(&self, password: &str) -> Result<()> {
        let hash = hash_password(password)?;
        self.inner.repo.set_password(mgmt_store::ADMIN_USER, &hash).await.map(|_| ())
    }

    /// Set (or clear, with `None`) the admin TOTP secret (`mgmt web totp-enroll/disable`).
    pub async fn set_admin_totp(&self, secret: Option<&str>) -> Result<()> {
        self.inner.repo.set_admin_totp(secret).await
    }

    /// The admin's provisioned API tokens (names only are meaningful; hashes are opaque).
    pub async fn admin_tokens(&self) -> Result<Vec<TokenEntry>> {
        Ok(self.inner.repo.user_by_id(mgmt_store::ADMIN_USER).await?.map(|u| u.tokens).unwrap_or_default())
    }

    /// Change a user's (or the admin's) password at runtime. The caller must have verified the
    /// current credentials first; existing sessions are invalidated (their auth hash changes).
    pub async fn change_password(&self, id: &str, new_password: &str) -> Result<()> {
        let hash = hash_password(new_password)?;
        if self.inner.repo.set_password(id, &hash).await? {
            Ok(())
        } else {
            Err(Error::NotFound(format!("user {id}")))
        }
    }

    // ---- managed-user admin ops (used by the admin API) ----------------------------

    pub async fn list_users(&self) -> Result<Vec<WebUser>> {
        self.inner.repo.list_users().await
    }
    pub async fn user_exists(&self, id: &str) -> Result<bool> {
        self.inner.repo.user_exists(id).await
    }
    pub async fn email_exists(&self, email: &str) -> Result<bool> {
        self.inner.repo.email_exists(email).await
    }
    pub async fn add_user(&self, u: &WebUser) -> Result<()> {
        self.inner.repo.add_user(u).await
    }
    pub async fn delete_user(&self, id: &str) -> Result<bool> {
        self.inner.repo.delete_user(id).await
    }
    pub async fn add_token(&self, owner: &str, t: &TokenEntry) -> Result<bool> {
        self.inner.repo.add_token(owner, t).await
    }
    pub async fn revoke_token(&self, owner: &str, name: &str) -> Result<bool> {
        self.inner.repo.revoke_token(owner, name).await
    }
    pub async fn google_oauth(&self) -> Result<Option<GoogleOAuth>> {
        self.inner.repo.google_oauth().await
    }
    pub async fn set_google_oauth(&self, g: &GoogleOAuth) -> Result<()> {
        self.inner.repo.set_google_oauth(g).await
    }

    // ---- rate limiting -------------------------------------------------------------

    /// Seconds until the lockout for `ip` lifts, if it is currently locked out.
    pub async fn locked_secs(&self, ip: IpAddr, now: DateTime<Utc>) -> Option<i64> {
        self.inner.repo.locked_secs(&ip.to_string(), now.timestamp()).await.ok().flatten()
    }

    pub async fn record_failure(&self, ip: IpAddr, now: DateTime<Utc>) {
        let _ = self
            .inner
            .repo
            .record_failure(&ip.to_string(), now.timestamp(), LOGIN_WINDOW_SECS, LOGIN_MAX_ATTEMPTS, LOGIN_LOCKOUT_SECS)
            .await;
    }

    pub async fn clear_attempts(&self, ip: IpAddr) {
        let _ = self.inner.repo.clear_attempts(&ip.to_string()).await;
    }
}

// ---- primitives -------------------------------------------------------------------

/// Hash a password with Argon2id (PHC string form).
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Other(format!("hashing password: {e}")))
}

/// Verify a password against a stored Argon2 PHC hash.
pub fn verify_password(phc: &str, password: &str) -> bool {
    PasswordHash::new(phc)
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

/// Generate a fresh random API token (URL-safe) and its SHA-256 hex.
pub fn new_api_token() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = data_encoding::BASE64URL_NOPAD.encode(&raw);
    let hash = sha256_hex(token.as_bytes());
    (token, hash)
}

/// Generate a base32 TOTP secret and its `otpauth://` provisioning URI.
pub fn new_totp_secret() -> (String, String) {
    let mut raw = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let secret = data_encoding::BASE32_NOPAD.encode(&raw);
    let uri = format!(
        "otpauth://totp/mgmt:web?secret={secret}&issuer=mgmt&period=30&digits=6&algorithm=SHA1"
    );
    (secret, uri)
}

/// Verify a 6-digit TOTP code against a base32 secret, allowing ±1 time step of skew.
pub fn verify_totp(secret_b32: &str, code: &str, now: DateTime<Utc>) -> bool {
    totp_step(secret_b32, code, now).is_some()
}

/// Like [`verify_totp`], but return the matched time step so callers can reject replays of an
/// already-consumed code within the skew window.
fn totp_step(secret_b32: &str, code: &str, now: DateTime<Utc>) -> Option<i64> {
    let cleaned = secret_b32.trim().replace(' ', "").replace('=', "").to_uppercase();
    let key = data_encoding::BASE32_NOPAD.decode(cleaned.as_bytes()).ok()?;
    let step = now.timestamp() / 30;
    let want = code.trim();
    [-1i64, 0, 1].iter().map(|w| step + w).find(|&s| hotp(&key, s as u64) == want)
}

fn hotp(key: &[u8], counter: u64) -> String {
    let mut mac = <Hmac<Sha1>>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[hash.len() - 1] & 0x0f) as usize;
    let bin = ((hash[offset] & 0x7f) as u32) << 24
        | (hash[offset + 1] as u32) << 16
        | (hash[offset + 2] as u32) << 8
        | (hash[offset + 3] as u32);
    format!("{:06}", bin % 1_000_000)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}
