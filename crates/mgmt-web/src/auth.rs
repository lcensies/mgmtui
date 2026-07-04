//! Credential model + primitives for the web server.
//!
//! Sessions and login cookies are handled by [`tower_sessions`] + [`axum_login`] (see
//! [`crate::auth_backend`] and [`crate::session_store`]); this module owns only the *credential
//! store*: the on-disk `web-auth.yaml` model, Argon2 password + TOTP verification, per-IP login
//! rate limiting, and the bearer-token → user-id resolution the native sync protocol needs.
//! Credentials live in `web-auth.yaml` (never in the shareable config).

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use mgmt_core::{Error, Result};

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

/// An admin-managed user. Each user owns an isolated vault at `<data_root>/users/<id>` reached over
/// `/api/sync` with one of its scoped bearer `tokens`. There is no per-user web-login password: the
/// single `password_hash` above is the admin's, and the web UI operates on the admin's own vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebUser {
    /// Stable id and vault directory name (`users/<id>`).
    pub id: String,
    /// Human-friendly display name.
    #[serde(default)]
    pub name: String,
    /// Scoped sync tokens (only their SHA-256 is stored; the raw token is shown once).
    #[serde(default)]
    pub tokens: Vec<TokenEntry>,
}

/// A Google Cloud OAuth client (Web-application type) used by the web "Connect Google" flow. The
/// user creates it once in Google Cloud and registers `<public_origin>/api/oauth/google/callback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleOAuth {
    pub client_id: String,
    pub client_secret: String,
}

/// On-disk credentials file (`web-auth.yaml`). All fields optional; an absent password means auth
/// is disabled (the server runs open, only sane on loopback).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totp_secret: Option<String>,
    /// Admin-owned bearer tokens (resolve to the `admin` vault). Kept for back-compat with
    /// single-user setups provisioned by `mgmt web token-new`.
    pub api_tokens: Vec<TokenEntry>,
    /// Additional admin-created users, each with an isolated vault + scoped sync tokens.
    pub users: Vec<WebUser>,
    /// The Google OAuth client for the web "Connect Google" flow (set by an admin).
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
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_yaml::to_string(self).map_err(|e| Error::Other(format!("serializing auth file: {e}")))?;
        std::fs::write(path, text)?;
        // Best-effort 0600 — credentials should not be world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

struct Attempt {
    count: u32,
    window_start: DateTime<Utc>,
    locked_until: Option<DateTime<Utc>>,
}

/// The shared credential store: the on-disk model behind a lock (so the admin password can be
/// claimed at first-run and users managed at runtime), a per-IP login rate limiter, and the
/// open/setup mode flag. Cheaply cloneable (it is an `Arc` internally) so both the axum-login
/// backend and the request guard can hold it.
#[derive(Clone)]
pub struct CredStore {
    inner: Arc<Inner>,
}

struct Inner {
    file: RwLock<AuthFile>,
    auth_path: PathBuf,
    rate: Mutex<HashMap<IpAddr, Attempt>>,
    /// When true the server runs unauthenticated (loopback dev / `--no-auth`). When false and no
    /// password is set, the server is in first-run *setup* mode (locked until claimed).
    allow_open: AtomicBool,
}

impl CredStore {
    /// Load the credential file from `auth_path`.
    pub fn load(auth_path: PathBuf) -> Result<Self> {
        let file = AuthFile::load(&auth_path)?;
        Ok(CredStore {
            inner: Arc::new(Inner {
                file: RwLock::new(file),
                auth_path,
                rate: Mutex::new(HashMap::new()),
                allow_open: AtomicBool::new(false),
            }),
        })
    }

    /// A disabled (open) store — used in tests and loopback dev.
    pub fn disabled() -> Self {
        CredStore {
            inner: Arc::new(Inner {
                file: RwLock::new(AuthFile::default()),
                auth_path: PathBuf::new(),
                rate: Mutex::new(HashMap::new()),
                allow_open: AtomicBool::new(true),
            }),
        }
    }

    /// Set whether a passwordless server may run unauthenticated (loopback / `--no-auth`).
    pub fn set_open_mode(&self, open: bool) {
        self.inner.allow_open.store(open, Ordering::SeqCst);
    }

    /// Whether a password is configured (auth enforced).
    pub fn enabled(&self) -> bool {
        self.inner.file.read().unwrap().password_hash.is_some()
    }

    /// Whether the server currently runs unauthenticated (no password + open mode).
    pub fn is_open(&self) -> bool {
        self.inner.allow_open.load(Ordering::SeqCst) && !self.enabled()
    }

    /// Whether the server is waiting for an admin to be claimed via the first-run setup flow.
    pub fn needs_setup(&self) -> bool {
        !self.enabled() && !self.inner.allow_open.load(Ordering::SeqCst)
    }

    // ---- bearer resolution (native sync) -------------------------------------------

    /// Resolve a bearer token to the id of the user that owns it (constant-time). Admin-owned
    /// `api_tokens` map to [`mgmt_store::ADMIN_USER`]; a user's scoped token maps to that user's id.
    pub fn resolve_bearer(&self, token: &str) -> Option<String> {
        let hash = sha256_hex(token.as_bytes());
        let matches = |t: &TokenEntry| constant_time_eq(t.hash.as_bytes(), hash.as_bytes());
        let file = self.inner.file.read().unwrap();
        if file.api_tokens.iter().any(matches) {
            return Some(mgmt_store::ADMIN_USER.to_string());
        }
        file.users.iter().find(|u| u.tokens.iter().any(matches)).map(|u| u.id.clone())
    }

    // ---- password / TOTP verification (used by the axum-login backend) -------------

    /// Verify the admin password (+ TOTP iff enrolled). No rate limiting here — the login handler
    /// wraps this with [`CredStore::locked_secs`]/[`record_failure`]/[`clear_attempts`].
    pub fn verify_credentials(&self, password: &str, totp: Option<&str>, now: DateTime<Utc>) -> bool {
        let file = self.inner.file.read().unwrap();
        let pw_ok = file.password_hash.as_deref().map(|h| verify_password(h, password)).unwrap_or(false);
        let totp_ok = match &file.totp_secret {
            Some(secret) => totp.map(|c| verify_totp(secret, c, now)).unwrap_or(false),
            None => true,
        };
        pw_ok && totp_ok
    }

    /// The admin's session-auth hash (the password-hash bytes), so changing the password
    /// invalidates existing sessions (axum-login's `session_auth_hash`).
    pub fn admin_auth_hash(&self) -> Vec<u8> {
        self.inner
            .file
            .read()
            .unwrap()
            .password_hash
            .as_deref()
            .map(|h| h.as_bytes().to_vec())
            .unwrap_or_default()
    }

    // ---- first-run setup + runtime credential management ---------------------------

    /// Claim the admin account: set the password (and optionally enroll a TOTP secret), persisting
    /// to `web-auth.yaml`.
    pub fn set_admin_password(&self, password: &str, totp_secret: Option<&str>) -> Result<()> {
        {
            let mut file = self.inner.file.write().unwrap();
            file.password_hash = Some(hash_password(password)?);
            if let Some(secret) = totp_secret {
                file.totp_secret = Some(secret.to_string());
            }
        }
        self.save_file()
    }

    /// Snapshot the current on-disk credential model (for the admin API to render the user list).
    pub fn snapshot(&self) -> AuthFile {
        self.inner.file.read().unwrap().clone()
    }

    /// Mutate the credential file under the write lock and persist it. The closure returns any value
    /// the caller wants back (e.g. a freshly-minted raw token).
    pub fn mutate_file<T>(&self, f: impl FnOnce(&mut AuthFile) -> T) -> Result<T> {
        let out = {
            let mut file = self.inner.file.write().unwrap();
            f(&mut file)
        };
        self.save_file()?;
        Ok(out)
    }

    /// Persist the credential file to `auth_path` (0600). A no-op when there is no path.
    fn save_file(&self) -> Result<()> {
        if self.inner.auth_path.as_os_str().is_empty() {
            return Ok(());
        }
        self.inner.file.read().unwrap().save(&self.inner.auth_path)
    }

    pub fn auth_path(&self) -> &Path {
        &self.inner.auth_path
    }

    // ---- rate limiting -------------------------------------------------------------

    /// Seconds until the lockout for `ip` lifts, if it is currently locked out.
    pub fn locked_secs(&self, ip: IpAddr, now: DateTime<Utc>) -> Option<i64> {
        let rate = self.inner.rate.lock().unwrap();
        rate.get(&ip)
            .and_then(|a| a.locked_until)
            .filter(|&until| until > now)
            .map(|until| (until - now).num_seconds().max(1))
    }

    pub fn record_failure(&self, ip: IpAddr, now: DateTime<Utc>) {
        let mut rate = self.inner.rate.lock().unwrap();
        let a = rate.entry(ip).or_insert(Attempt { count: 0, window_start: now, locked_until: None });
        if (now - a.window_start).num_seconds() > LOGIN_WINDOW_SECS {
            a.count = 0;
            a.window_start = now;
        }
        a.count += 1;
        if a.count >= LOGIN_MAX_ATTEMPTS {
            a.locked_until = Some(now + Duration::seconds(LOGIN_LOCKOUT_SECS));
            a.count = 0;
            a.window_start = now;
        }
    }

    pub fn clear_attempts(&self, ip: IpAddr) {
        self.inner.rate.lock().unwrap().remove(&ip);
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
    let cleaned = secret_b32.trim().replace(' ', "").replace('=', "").to_uppercase();
    let Ok(key) = data_encoding::BASE32_NOPAD.decode(cleaned.as_bytes()) else {
        return false;
    };
    let step = now.timestamp() / 30;
    let want = code.trim();
    [-1i64, 0, 1].iter().any(|w| hotp(&key, (step + w) as u64) == want)
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

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}
