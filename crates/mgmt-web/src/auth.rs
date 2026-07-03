//! Authentication for the public web server: an Argon2id password (+ optional TOTP) guarding
//! browser sessions, plus static bearer tokens for the desktop sync client. Credentials live in a
//! separate `web-auth.yaml` (never in the shareable config); sessions are hashed at rest and
//! persisted so a restart doesn't log the phone out.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

const SESSION_COOKIE: &str = "mgmt_session";
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

/// On-disk credentials file (`web-auth.yaml`). All fields optional; an absent password means auth
/// is disabled (the server runs open, only sane on loopback).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totp_secret: Option<String>,
    pub api_tokens: Vec<TokenEntry>,
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

struct Session {
    expires: DateTime<Utc>,
}

struct Attempt {
    count: u32,
    window_start: DateTime<Utc>,
    locked_until: Option<DateTime<Utc>>,
}

/// Persisted session record (only the hashed id + expiry; the raw id is the cookie value).
#[derive(Serialize, Deserialize)]
struct StoredSession {
    hash: String,
    expires: DateTime<Utc>,
}

/// Live authentication state shared across requests.
pub struct AuthState {
    file: AuthFile,
    auth_path: PathBuf,
    sessions: Mutex<HashMap<String, Session>>,
    sessions_path: PathBuf,
    rate: Mutex<HashMap<IpAddr, Attempt>>,
    public_origin: Option<String>,
    ttl: Duration,
    secure_cookie: bool,
}

impl AuthState {
    /// Build from the credentials file and session store paths.
    pub fn load(
        auth_path: PathBuf,
        sessions_path: PathBuf,
        public_origin: Option<String>,
        ttl_days: u64,
    ) -> Result<Self> {
        let file = AuthFile::load(&auth_path)?;
        let sessions = load_sessions(&sessions_path);
        let secure_cookie = public_origin.as_deref().map(|o| o.starts_with("https://")).unwrap_or(false);
        Ok(AuthState {
            file,
            auth_path,
            sessions: Mutex::new(sessions),
            sessions_path,
            rate: Mutex::new(HashMap::new()),
            public_origin,
            ttl: Duration::days(ttl_days.max(1) as i64),
            secure_cookie,
        })
    }

    /// A disabled auth state (open server) — used in tests and loopback dev.
    pub fn disabled() -> Self {
        AuthState {
            file: AuthFile::default(),
            auth_path: PathBuf::new(),
            sessions: Mutex::new(HashMap::new()),
            sessions_path: PathBuf::new(),
            rate: Mutex::new(HashMap::new()),
            public_origin: None,
            ttl: Duration::days(30),
            secure_cookie: false,
        }
    }

    /// Whether a password is configured (auth enforced).
    pub fn enabled(&self) -> bool {
        self.file.password_hash.is_some()
    }

    pub fn public_origin(&self) -> Option<&str> {
        self.public_origin.as_deref()
    }

    // ---- login / sessions ----------------------------------------------------------

    /// Verify credentials and, on success, return a fresh session cookie header value.
    /// `totp` is required iff a TOTP secret is enrolled. Rate-limited per IP.
    pub fn login(&self, ip: IpAddr, password: &str, totp: Option<&str>, now: DateTime<Utc>) -> LoginResult {
        if let Some(until) = self.locked_until(ip, now) {
            return LoginResult::RateLimited((until - now).num_seconds().max(1));
        }
        let pw_ok = self
            .file
            .password_hash
            .as_deref()
            .map(|h| verify_password(h, password))
            .unwrap_or(false);
        let totp_ok = match &self.file.totp_secret {
            Some(secret) => totp.map(|c| verify_totp(secret, c, now)).unwrap_or(false),
            None => true,
        };
        if pw_ok && totp_ok {
            self.clear_attempts(ip);
            LoginResult::Ok(self.issue_session(now))
        } else {
            self.record_failure(ip, now);
            LoginResult::Denied
        }
    }

    fn issue_session(&self, now: DateTime<Utc>) -> String {
        let mut raw = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut raw);
        let token = data_encoding::BASE64URL_NOPAD.encode(&raw);
        let hash = sha256_hex(token.as_bytes());
        let expires = now + self.ttl;
        self.sessions.lock().unwrap().insert(hash, Session { expires });
        self.persist_sessions();
        token
    }

    /// Validate a session cookie value; refresh its expiry (rolling TTL). Returns true if valid.
    pub fn validate_session(&self, token: &str, now: DateTime<Utc>) -> bool {
        let hash = sha256_hex(token.as_bytes());
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(&hash) {
            Some(s) if s.expires > now => {
                s.expires = now + self.ttl; // rolling refresh
                true
            }
            Some(_) => {
                sessions.remove(&hash); // expired
                false
            }
            None => false,
        }
    }

    /// Drop a session (logout).
    pub fn logout(&self, token: &str) {
        let hash = sha256_hex(token.as_bytes());
        self.sessions.lock().unwrap().remove(&hash);
        self.persist_sessions();
    }

    /// Validate a bearer token against the provisioned API tokens (constant-time).
    pub fn validate_bearer(&self, token: &str) -> bool {
        let hash = sha256_hex(token.as_bytes());
        self.file.api_tokens.iter().any(|t| constant_time_eq(t.hash.as_bytes(), hash.as_bytes()))
    }

    // ---- cookie helpers ------------------------------------------------------------

    pub fn cookie_name(&self) -> &'static str {
        SESSION_COOKIE
    }

    /// A `Set-Cookie` value establishing the session.
    pub fn set_cookie(&self, token: &str) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!(
            "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{secure}",
            self.ttl.num_seconds()
        )
    }

    /// A `Set-Cookie` value clearing the session.
    pub fn clear_cookie(&self) -> String {
        format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
    }

    // ---- rate limiting -------------------------------------------------------------

    fn locked_until(&self, ip: IpAddr, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let rate = self.rate.lock().unwrap();
        rate.get(&ip).and_then(|a| a.locked_until).filter(|&until| until > now)
    }

    fn record_failure(&self, ip: IpAddr, now: DateTime<Utc>) {
        let mut rate = self.rate.lock().unwrap();
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

    fn clear_attempts(&self, ip: IpAddr) {
        self.rate.lock().unwrap().remove(&ip);
    }

    // ---- session persistence -------------------------------------------------------

    fn persist_sessions(&self) {
        if self.sessions_path.as_os_str().is_empty() {
            return;
        }
        let sessions = self.sessions.lock().unwrap();
        let stored: Vec<StoredSession> =
            sessions.iter().map(|(h, s)| StoredSession { hash: h.clone(), expires: s.expires }).collect();
        if let Some(parent) = self.sessions_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&stored) {
            let tmp = self.sessions_path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.sessions_path);
            }
        }
    }

    // ---- credential management (used by the `mgmt web` CLI) -------------------------

    pub fn auth_path(&self) -> &Path {
        &self.auth_path
    }
}

fn load_sessions(path: &Path) -> HashMap<String, Session> {
    let mut out = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(stored) = serde_json::from_str::<Vec<StoredSession>>(&text) {
            for s in stored {
                out.insert(s.hash, Session { expires: s.expires });
            }
        }
    }
    out
}

/// Result of a login attempt.
pub enum LoginResult {
    /// Success — carries the raw session token to set as a cookie.
    Ok(String),
    /// Bad password or TOTP.
    Denied,
    /// Too many attempts; carries seconds until the lockout lifts.
    RateLimited(i64),
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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_round_trips() {
        let h = hash_password("hunter2").unwrap();
        assert!(verify_password(&h, "hunter2"));
        assert!(!verify_password(&h, "wrong"));
    }

    #[test]
    fn totp_matches_known_vector_shape() {
        let (secret, uri) = new_totp_secret();
        assert!(uri.contains("otpauth://totp/"));
        // The code generated for `now` must verify at `now`.
        let now = Utc::now();
        let cleaned = secret.clone();
        let key = data_encoding::BASE32_NOPAD.decode(cleaned.as_bytes()).unwrap();
        let code = hotp(&key, now.timestamp() as u64 / 30);
        assert!(verify_totp(&secret, &code, now));
        assert!(!verify_totp(&secret, "000000", now) || code == "000000");
    }

    #[test]
    fn bearer_tokens_validate() {
        let (token, hash) = new_api_token();
        let mut file = AuthFile::default();
        file.password_hash = Some(hash_password("x").unwrap());
        file.api_tokens.push(TokenEntry { name: "laptop".into(), hash });
        let auth = AuthState {
            file,
            auth_path: PathBuf::new(),
            sessions: Mutex::new(HashMap::new()),
            sessions_path: PathBuf::new(),
            rate: Mutex::new(HashMap::new()),
            public_origin: None,
            ttl: Duration::days(30),
            secure_cookie: false,
        };
        assert!(auth.validate_bearer(&token));
        assert!(!auth.validate_bearer("nope"));
    }

    #[test]
    fn login_success_issues_validatable_session() {
        let mut file = AuthFile::default();
        file.password_hash = Some(hash_password("pw").unwrap());
        let auth = AuthState {
            file,
            auth_path: PathBuf::new(),
            sessions: Mutex::new(HashMap::new()),
            sessions_path: PathBuf::new(),
            rate: Mutex::new(HashMap::new()),
            public_origin: None,
            ttl: Duration::days(30),
            secure_cookie: false,
        };
        let now = Utc::now();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let token = match auth.login(ip, "pw", None, now) {
            LoginResult::Ok(t) => t,
            _ => panic!("login should succeed"),
        };
        assert!(auth.validate_session(&token, now));
        assert!(matches!(auth.login(ip, "bad", None, now), LoginResult::Denied));
    }

    #[test]
    fn rate_limit_kicks_in_after_repeated_failures() {
        let mut file = AuthFile::default();
        file.password_hash = Some(hash_password("pw").unwrap());
        let auth = AuthState {
            file,
            auth_path: PathBuf::new(),
            sessions: Mutex::new(HashMap::new()),
            sessions_path: PathBuf::new(),
            rate: Mutex::new(HashMap::new()),
            public_origin: None,
            ttl: Duration::days(30),
            secure_cookie: false,
        };
        let now = Utc::now();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..LOGIN_MAX_ATTEMPTS {
            assert!(matches!(auth.login(ip, "bad", None, now), LoginResult::Denied));
        }
        assert!(matches!(auth.login(ip, "pw", None, now), LoginResult::RateLimited(_)));
    }
}
