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

/// A managed user. Each owns an isolated vault at `<data_root>/users/<id>` reached over `/api/sync`
/// with one of its scoped bearer `tokens`, and (unlike the legacy sync-only model) can also log
/// into the web UI directly with `email` + a per-user `password_hash` set by accepting an invite.
/// `is_admin` marks the provisioned admin; the top-level `AuthFile.password_hash` remains the
/// admin's password for backward compatibility, so an admin `WebUser` may carry no `password_hash`.
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
    /// The last TOTP time step each user successfully authenticated with, so a sniffed code
    /// cannot be replayed inside the ±1-step acceptance window.
    totp_last_step: Mutex<HashMap<String, i64>>,
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
                totp_last_step: Mutex::new(HashMap::new()),
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
                totp_last_step: Mutex::new(HashMap::new()),
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

    /// Whether a TOTP secret is enrolled (2FA is opt-in; off unless explicitly enabled).
    pub fn totp_enrolled(&self) -> bool {
        self.inner.file.read().unwrap().totp_secret.is_some()
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
        self.verify_user_credentials(mgmt_store::ADMIN_USER, password, totp, now)
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

    // ---- per-user web login (email + password, set via invite) -------------------

    /// Look up a non-admin user by login identifier (email, case-insensitive).
    pub fn user_by_email(&self, email: &str) -> Option<WebUser> {
        let e = email.trim().to_lowercase();
        self.inner
            .file
            .read()
            .unwrap()
            .users
            .iter()
            .find(|u| u.email.as_deref().map(|s| s.to_lowercase()).as_deref() == Some(e.as_str()))
            .cloned()
    }

    /// Find a user (admin or not) by id. The admin is synthesized from the top-level fields so the
    /// login/session path treats admin and regular users uniformly.
    pub fn user_by_id(&self, id: &str) -> Option<WebUser> {
        if id == mgmt_store::ADMIN_USER {
            let f = self.inner.file.read().unwrap();
            return Some(WebUser {
                id: mgmt_store::ADMIN_USER.to_string(),
                name: "admin".into(),
                email: None,
                password_hash: f.password_hash.clone(),
                totp_secret: f.totp_secret.clone(),
                invite_token: None,
                is_admin: true,
                tokens: f.api_tokens.clone(),
            });
        }
        self.inner.file.read().unwrap().users.iter().find(|u| u.id == id).cloned()
    }

    /// Verify a user's password (+ TOTP iff enrolled, with replay protection). For the admin,
    /// the top-level password; for others, their own `password_hash`.
    pub fn verify_user_credentials(&self, id: &str, password: &str, totp: Option<&str>, now: DateTime<Utc>) -> bool {
        let (pw_hash, totp_secret) = {
            let file = self.inner.file.read().unwrap();
            if id == mgmt_store::ADMIN_USER {
                (file.password_hash.clone(), file.totp_secret.clone())
            } else {
                match file.users.iter().find(|u| u.id == id) {
                    Some(u) => (u.password_hash.clone(), u.totp_secret.clone()),
                    None => return false,
                }
            }
        };
        // Password first: a failed password must not consume the TOTP step, or an attacker
        // could burn a legitimate user's current code without knowing the password.
        if !pw_hash.as_deref().map(|h| verify_password(h, password)).unwrap_or(false) {
            return false;
        }
        match totp_secret {
            Some(secret) => self.consume_totp(id, &secret, totp, now),
            None => true,
        }
    }

    /// TOTP check with replay protection: the code must verify AND its time step must be newer
    /// than the last step this user consumed, so a sniffed code can't be replayed in the window.
    fn consume_totp(&self, id: &str, secret: &str, code: Option<&str>, now: DateTime<Utc>) -> bool {
        let Some(step) = code.and_then(|c| totp_step(secret, c, now)) else { return false };
        let mut last = self.inner.totp_last_step.lock().unwrap();
        match last.get(id) {
            Some(&prev) if step <= prev => false,
            _ => {
                last.insert(id.to_string(), step);
                true
            }
        }
    }

    /// Session-auth hash for a user (their password-hash bytes), so a password change invalidates
    /// their sessions. For the admin, the top-level hash.
    pub fn user_auth_hash(&self, id: &str) -> Vec<u8> {
        if id == mgmt_store::ADMIN_USER {
            return self.admin_auth_hash();
        }
        self.inner
            .file
            .read()
            .unwrap()
            .users
            .iter()
            .find(|u| u.id == id)
            .and_then(|u| u.password_hash.as_deref().map(|h| h.as_bytes().to_vec()))
            .unwrap_or_default()
    }

    /// Whether a user has a pending (unaccepted) invite.
    pub fn has_pending_invite(&self, id: &str) -> bool {
        self.inner.file.read().unwrap().users.iter().any(|u| u.id == id && u.invite_token.is_some())
    }

    /// Mint a one-time invite token for `id` and return the raw token (its SHA-256 is stored). The
    /// invitee presents it to `/api/auth/invite/accept` to set their password.
    pub fn mint_invite(&self, id: &str) -> Result<String> {
        let (raw, hash) = new_api_token();
        let ok = self.mutate_file(|f| match f.users.iter_mut().find(|u| u.id == id) {
            Some(u) => {
                u.invite_token = Some(hash);
                true
            }
            None => false,
        })?;
        if ok { Ok(raw) } else { Err(Error::NotFound(format!("user {id}"))) }
    }

    /// Consume an invite token and set the user's password, returning the user id. Atomic + one-shot:
    /// the stored (hashed) token is cleared on success or a mismatch.
    pub fn accept_invite(&self, token: &str, password: &str) -> Result<String> {
        let hash = sha256_hex(token.as_bytes());
        let phc = hash_password(password)?;
        let id = self.mutate_file(|f| {
            f.users
                .iter_mut()
                .find(|u| u.invite_token.as_deref().map(|t| constant_time_eq(t.as_bytes(), hash.as_bytes())).unwrap_or(false))
                .map(|u| {
                    u.password_hash = Some(phc);
                    u.invite_token = None;
                    u.id.clone()
                })
        })?;
        id.ok_or_else(|| Error::NotFound("invalid or expired invite token".into()))
    }

    /// Validate an invite token without consuming it: return the user it's for (None if invalid).
    pub fn invite_owner(&self, token: &str) -> Option<WebUser> {
        let hash = sha256_hex(token.as_bytes());
        self.inner
            .file
            .read()
            .unwrap()
            .users
            .iter()
            .find(|u| u.invite_token.as_deref().map(|t| constant_time_eq(t.as_bytes(), hash.as_bytes())).unwrap_or(false))
            .cloned()
    }

    // ---- first-run setup + runtime credential management ---------------------------

    /// Claim the admin account: set the password (and optionally enroll a TOTP secret), persisting
    /// to `web-auth.yaml`. Claiming is atomic — it fails if an admin password is already set, so
    /// first-run setup can only ever succeed once per deployment (even under concurrent requests).
    /// Runtime password *changes* go through `mgmt web setpass`, not this path.
    pub fn set_admin_password(&self, password: &str, totp_secret: Option<&str>) -> Result<()> {
        let hash = hash_password(password)?;
        {
            let mut file = self.inner.file.write().unwrap();
            if file.password_hash.is_some() {
                return Err(Error::Other("admin already configured".into()));
            }
            file.password_hash = Some(hash);
            if let Some(secret) = totp_secret {
                file.totp_secret = Some(secret.to_string());
            }
        }
        self.save_file()
    }

    /// Change a user's (or the admin's) password at runtime, persisting to `web-auth.yaml`.
    /// The caller must have verified the current credentials first. Existing sessions are
    /// invalidated automatically (their `session_auth_hash` no longer matches).
    pub fn change_password(&self, id: &str, new_password: &str) -> Result<()> {
        let hash = hash_password(new_password)?;
        let changed = self.mutate_file(|f| {
            if id == mgmt_store::ADMIN_USER {
                f.password_hash = Some(hash);
                return true;
            }
            match f.users.iter_mut().find(|u| u.id == id) {
                Some(u) => {
                    u.password_hash = Some(hash);
                    true
                }
                None => false,
            }
        })?;
        if changed { Ok(()) } else { Err(Error::NotFound(format!("user {id}"))) }
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
        // Evict entries whose window and lockout have both lapsed, so an attacker cycling source
        // addresses can't grow the map without bound.
        rate.retain(|_, a| {
            (now - a.window_start).num_seconds() <= LOGIN_WINDOW_SECS
                || a.locked_until.map(|until| until > now).unwrap_or(false)
        });
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

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}
