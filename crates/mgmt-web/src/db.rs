//! Credential persistence layer — a swappable repository behind the [`CredRepo`] trait, with a
//! SQLite implementation ([`SqliteRepo`]).
//!
//! The auth *logic* (Argon2, TOTP, rate-limit policy, open/setup mode) lives in
//! [`crate::auth::CredStore`] and depends only on this trait, so the storage backend can be
//! replaced (Postgres, an HTTP service, an in-memory fake for tests) without touching auth code.
//! `CredRepo` deals purely in stored rows/values; it holds no policy.

use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::Row;

use mgmt_core::{Error, Result};

use crate::auth::{constant_time_eq, AuthFile, GoogleOAuth, TokenEntry, WebUser};

/// The storage interface the credential logic depends on. All methods are data operations — no
/// hashing, no policy. Times are unix seconds (`i64`). Implementations must be `Send + Sync`.
#[async_trait]
pub trait CredRepo: Send + Sync {
    // admin credential (single logical row)
    async fn admin_password_hash(&self) -> Result<Option<String>>;
    async fn admin_totp_secret(&self) -> Result<Option<String>>;
    /// Set the admin password iff none is set yet (atomic). Returns whether it was applied.
    async fn set_admin_password_if_unset(&self, hash: &str, totp: Option<&str>) -> Result<bool>;
    /// Set (or clear, with `None`) the admin's TOTP secret.
    async fn set_admin_totp(&self, secret: Option<&str>) -> Result<()>;

    // password / totp lookups
    /// `(password_hash, totp_secret)` for a user (admin or managed); `None` if absent.
    async fn user_creds(&self, id: &str) -> Result<Option<(Option<String>, Option<String>)>>;
    async fn set_password(&self, id: &str, hash: &str) -> Result<bool>;

    // users
    async fn user_by_id(&self, id: &str) -> Result<Option<WebUser>>;
    async fn user_by_email(&self, email: &str) -> Result<Option<WebUser>>;
    async fn list_users(&self) -> Result<Vec<WebUser>>;
    async fn user_exists(&self, id: &str) -> Result<bool>;
    async fn email_exists(&self, email: &str) -> Result<bool>;
    async fn add_user(&self, u: &WebUser) -> Result<()>;
    async fn delete_user(&self, id: &str) -> Result<bool>;

    // tokens (hashed)
    async fn resolve_token(&self, hash: &str) -> Result<Option<String>>;
    async fn add_token(&self, owner: &str, t: &TokenEntry) -> Result<bool>;
    async fn revoke_token(&self, owner: &str, name: &str) -> Result<bool>;

    // invites
    async fn set_user_invite(&self, id: &str, hash: &str) -> Result<bool>;
    async fn has_pending_invite(&self, id: &str) -> Result<bool>;
    async fn user_by_invite(&self, hash: &str) -> Result<Option<WebUser>>;
    async fn accept_invite(&self, hash: &str, new_pw_hash: &str) -> Result<Option<String>>;

    // google oauth client
    async fn google_oauth(&self) -> Result<Option<GoogleOAuth>>;
    async fn set_google_oauth(&self, g: &GoogleOAuth) -> Result<()>;

    // login rate limiting
    async fn locked_secs(&self, ip: &str, now: i64) -> Result<Option<i64>>;
    async fn record_failure(&self, ip: &str, now: i64, window: i64, max: u32, lockout: i64) -> Result<()>;
    async fn clear_attempts(&self, ip: &str) -> Result<()>;

    // TOTP replay guard: accept `step` only if newer than the last consumed for `id`.
    async fn consume_totp_step(&self, id: &str, step: i64) -> Result<bool>;
}

fn dberr(e: impl std::fmt::Display) -> Error {
    Error::Other(format!("web-auth db: {e}"))
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS admin (
    id                   INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash        TEXT,
    totp_secret          TEXT,
    google_client_id     TEXT,
    google_client_secret TEXT
);
INSERT OR IGNORE INTO admin (id) VALUES (1);

CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL DEFAULT '',
    email         TEXT,
    password_hash TEXT,
    totp_secret   TEXT,
    invite_token  TEXT,
    is_admin      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS tokens (
    owner TEXT NOT NULL,
    name  TEXT NOT NULL,
    hash  TEXT NOT NULL,
    PRIMARY KEY (owner, name)
);

CREATE TABLE IF NOT EXISTS rate_limit (
    ip           TEXT PRIMARY KEY,
    count        INTEGER NOT NULL,
    window_start INTEGER NOT NULL,
    locked_until INTEGER
);

CREATE TABLE IF NOT EXISTS totp_step (
    user_id TEXT PRIMARY KEY,
    step    INTEGER NOT NULL
);
"#;

/// A SQLite-backed [`CredRepo`] (sqlx pool). The DB lives at `<data_root>/.state/web-auth.db`.
pub struct SqliteRepo {
    pool: SqlitePool,
}

impl SqliteRepo {
    /// Open (creating if absent) the credential DB at `path`, running the idempotent schema.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new().max_connections(5).connect_with(opts).await.map_err(dberr)?;
        Self::migrate(&pool).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(SqliteRepo { pool })
    }

    /// An ephemeral in-memory DB (tests, disabled/open-mode store). A single connection so every
    /// query sees the same database.
    pub async fn memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:").map_err(dberr)?;
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await.map_err(dberr)?;
        Self::migrate(&pool).await?;
        Ok(SqliteRepo { pool })
    }

    async fn migrate(pool: &SqlitePool) -> Result<()> {
        for stmt in SCHEMA.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            sqlx::query(stmt).execute(pool).await.map_err(dberr)?;
        }
        Ok(())
    }

    /// Import a legacy `web-auth.yaml` once, when the DB has no admin configured yet. Returns
    /// whether anything was imported. SQLite-specific (a one-off migration), so it's inherent
    /// rather than part of [`CredRepo`].
    pub async fn import_legacy(&self, file: &AuthFile) -> Result<bool> {
        if self.admin_password_hash().await?.is_some() {
            return Ok(false);
        }
        if file.password_hash.is_none() && file.users.is_empty() && file.api_tokens.is_empty() {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await.map_err(dberr)?;
        sqlx::query("UPDATE admin SET password_hash = ?, totp_secret = ?, google_client_id = ?, google_client_secret = ? WHERE id = 1")
            .bind(&file.password_hash)
            .bind(&file.totp_secret)
            .bind(file.google_oauth.as_ref().map(|g| g.client_id.clone()))
            .bind(file.google_oauth.as_ref().map(|g| g.client_secret.clone()))
            .execute(&mut *tx).await.map_err(dberr)?;
        for t in &file.api_tokens {
            sqlx::query("INSERT OR REPLACE INTO tokens (owner, name, hash) VALUES (?, ?, ?)")
                .bind(mgmt_store::ADMIN_USER).bind(&t.name).bind(&t.hash)
                .execute(&mut *tx).await.map_err(dberr)?;
        }
        for u in &file.users {
            sqlx::query("INSERT OR REPLACE INTO users (id, name, email, password_hash, totp_secret, invite_token, is_admin) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .bind(&u.id).bind(&u.name).bind(u.email.as_ref().map(|e| e.to_lowercase()))
                .bind(&u.password_hash).bind(&u.totp_secret).bind(&u.invite_token).bind(u.is_admin as i64)
                .execute(&mut *tx).await.map_err(dberr)?;
            for t in &u.tokens {
                sqlx::query("INSERT OR REPLACE INTO tokens (owner, name, hash) VALUES (?, ?, ?)")
                    .bind(&u.id).bind(&t.name).bind(&t.hash)
                    .execute(&mut *tx).await.map_err(dberr)?;
            }
        }
        tx.commit().await.map_err(dberr)?;
        Ok(true)
    }

    async fn tokens_for(&self, owner: &str) -> Result<Vec<TokenEntry>> {
        let rows = sqlx::query("SELECT name, hash FROM tokens WHERE owner = ? ORDER BY name")
            .bind(owner).fetch_all(&self.pool).await.map_err(dberr)?;
        Ok(rows.into_iter().map(|r| TokenEntry { name: r.get("name"), hash: r.get("hash") }).collect())
    }

    async fn row_to_user(&self, r: &sqlx::sqlite::SqliteRow) -> Result<WebUser> {
        let id: String = r.get("id");
        let tokens = self.tokens_for(&id).await?;
        Ok(WebUser {
            name: r.get("name"),
            email: r.get("email"),
            password_hash: r.get("password_hash"),
            totp_secret: r.get("totp_secret"),
            invite_token: r.get("invite_token"),
            is_admin: r.get::<i64, _>("is_admin") != 0,
            tokens,
            id,
        })
    }
}

#[async_trait]
impl CredRepo for SqliteRepo {
    async fn admin_password_hash(&self) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT password_hash FROM admin WHERE id = 1").fetch_one(&self.pool).await.map_err(dberr)
    }

    async fn admin_totp_secret(&self) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT totp_secret FROM admin WHERE id = 1").fetch_one(&self.pool).await.map_err(dberr)
    }

    async fn set_admin_password_if_unset(&self, hash: &str, totp: Option<&str>) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(dberr)?;
        let current: Option<String> = sqlx::query_scalar("SELECT password_hash FROM admin WHERE id = 1")
            .fetch_one(&mut *tx).await.map_err(dberr)?;
        if current.is_some() {
            return Ok(false);
        }
        sqlx::query("UPDATE admin SET password_hash = ?, totp_secret = COALESCE(?, totp_secret) WHERE id = 1")
            .bind(hash).bind(totp).execute(&mut *tx).await.map_err(dberr)?;
        tx.commit().await.map_err(dberr)?;
        Ok(true)
    }

    async fn set_admin_totp(&self, secret: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE admin SET totp_secret = ? WHERE id = 1").bind(secret)
            .execute(&self.pool).await.map_err(dberr)?;
        Ok(())
    }

    async fn user_creds(&self, id: &str) -> Result<Option<(Option<String>, Option<String>)>> {
        if id == mgmt_store::ADMIN_USER {
            let row = sqlx::query("SELECT password_hash, totp_secret FROM admin WHERE id = 1")
                .fetch_one(&self.pool).await.map_err(dberr)?;
            return Ok(Some((row.get("password_hash"), row.get("totp_secret"))));
        }
        let row = sqlx::query("SELECT password_hash, totp_secret FROM users WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await.map_err(dberr)?;
        Ok(row.map(|r| (r.get::<Option<String>, _>("password_hash"), r.get::<Option<String>, _>("totp_secret"))))
    }

    async fn set_password(&self, id: &str, hash: &str) -> Result<bool> {
        if id == mgmt_store::ADMIN_USER {
            sqlx::query("UPDATE admin SET password_hash = ? WHERE id = 1").bind(hash)
                .execute(&self.pool).await.map_err(dberr)?;
            return Ok(true);
        }
        let r = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?").bind(hash).bind(id)
            .execute(&self.pool).await.map_err(dberr)?;
        Ok(r.rows_affected() > 0)
    }

    async fn user_by_id(&self, id: &str) -> Result<Option<WebUser>> {
        if id == mgmt_store::ADMIN_USER {
            let Some((pw, totp)) = self.user_creds(id).await? else { return Ok(None) };
            return Ok(Some(WebUser {
                id: mgmt_store::ADMIN_USER.to_string(),
                name: "admin".into(),
                email: None,
                password_hash: pw,
                totp_secret: totp,
                invite_token: None,
                is_admin: true,
                tokens: self.tokens_for(mgmt_store::ADMIN_USER).await?,
            }));
        }
        let row = sqlx::query("SELECT * FROM users WHERE id = ?").bind(id)
            .fetch_optional(&self.pool).await.map_err(dberr)?;
        match row {
            Some(r) => Ok(Some(self.row_to_user(&r).await?)),
            None => Ok(None),
        }
    }

    async fn user_by_email(&self, email: &str) -> Result<Option<WebUser>> {
        let e = email.trim().to_lowercase();
        let row = sqlx::query("SELECT * FROM users WHERE email = ?").bind(&e)
            .fetch_optional(&self.pool).await.map_err(dberr)?;
        match row {
            Some(r) => Ok(Some(self.row_to_user(&r).await?)),
            None => Ok(None),
        }
    }

    async fn list_users(&self) -> Result<Vec<WebUser>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY id").fetch_all(&self.pool).await.map_err(dberr)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(self.row_to_user(r).await?);
        }
        Ok(out)
    }

    async fn user_exists(&self, id: &str) -> Result<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ?").bind(id)
            .fetch_one(&self.pool).await.map_err(dberr)?;
        Ok(n > 0)
    }

    async fn email_exists(&self, email: &str) -> Result<bool> {
        let e = email.trim().to_lowercase();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email = ?").bind(&e)
            .fetch_one(&self.pool).await.map_err(dberr)?;
        Ok(n > 0)
    }

    async fn add_user(&self, u: &WebUser) -> Result<()> {
        sqlx::query("INSERT INTO users (id, name, email, password_hash, totp_secret, invite_token, is_admin) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(&u.id).bind(&u.name).bind(u.email.as_ref().map(|e| e.to_lowercase()))
            .bind(&u.password_hash).bind(&u.totp_secret).bind(&u.invite_token).bind(u.is_admin as i64)
            .execute(&self.pool).await.map_err(dberr)?;
        Ok(())
    }

    async fn delete_user(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(dberr)?;
        sqlx::query("DELETE FROM tokens WHERE owner = ?").bind(id).execute(&mut *tx).await.map_err(dberr)?;
        sqlx::query("DELETE FROM totp_step WHERE user_id = ?").bind(id).execute(&mut *tx).await.map_err(dberr)?;
        let r = sqlx::query("DELETE FROM users WHERE id = ?").bind(id).execute(&mut *tx).await.map_err(dberr)?;
        tx.commit().await.map_err(dberr)?;
        Ok(r.rows_affected() > 0)
    }

    async fn resolve_token(&self, hash: &str) -> Result<Option<String>> {
        // Constant-time compare over the stored hashes (they're SHA-256 of 256-bit tokens).
        let rows = sqlx::query("SELECT owner, hash FROM tokens").fetch_all(&self.pool).await.map_err(dberr)?;
        for r in &rows {
            let stored: String = r.get("hash");
            if constant_time_eq(stored.as_bytes(), hash.as_bytes()) {
                return Ok(Some(r.get("owner")));
            }
        }
        Ok(None)
    }

    async fn add_token(&self, owner: &str, t: &TokenEntry) -> Result<bool> {
        if owner != mgmt_store::ADMIN_USER && !self.user_exists(owner).await? {
            return Ok(false);
        }
        sqlx::query("INSERT OR REPLACE INTO tokens (owner, name, hash) VALUES (?, ?, ?)")
            .bind(owner).bind(&t.name).bind(&t.hash).execute(&self.pool).await.map_err(dberr)?;
        Ok(true)
    }

    async fn revoke_token(&self, owner: &str, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM tokens WHERE owner = ? AND name = ?").bind(owner).bind(name)
            .execute(&self.pool).await.map_err(dberr)?;
        Ok(r.rows_affected() > 0)
    }

    async fn set_user_invite(&self, id: &str, hash: &str) -> Result<bool> {
        let r = sqlx::query("UPDATE users SET invite_token = ? WHERE id = ?").bind(hash).bind(id)
            .execute(&self.pool).await.map_err(dberr)?;
        Ok(r.rows_affected() > 0)
    }

    async fn has_pending_invite(&self, id: &str) -> Result<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ? AND invite_token IS NOT NULL")
            .bind(id).fetch_one(&self.pool).await.map_err(dberr)?;
        Ok(n > 0)
    }

    async fn user_by_invite(&self, hash: &str) -> Result<Option<WebUser>> {
        let rows = sqlx::query("SELECT * FROM users WHERE invite_token IS NOT NULL")
            .fetch_all(&self.pool).await.map_err(dberr)?;
        for r in &rows {
            let stored: String = r.get("invite_token");
            if constant_time_eq(stored.as_bytes(), hash.as_bytes()) {
                return Ok(Some(self.row_to_user(r).await?));
            }
        }
        Ok(None)
    }

    async fn accept_invite(&self, hash: &str, new_pw_hash: &str) -> Result<Option<String>> {
        let mut tx = self.pool.begin().await.map_err(dberr)?;
        let rows = sqlx::query("SELECT id, invite_token FROM users WHERE invite_token IS NOT NULL")
            .fetch_all(&mut *tx).await.map_err(dberr)?;
        let mut target = None;
        for r in &rows {
            let stored: String = r.get("invite_token");
            if constant_time_eq(stored.as_bytes(), hash.as_bytes()) {
                target = Some(r.get::<String, _>("id"));
                break;
            }
        }
        let Some(id) = target else { return Ok(None) };
        sqlx::query("UPDATE users SET password_hash = ?, invite_token = NULL WHERE id = ?")
            .bind(new_pw_hash).bind(&id).execute(&mut *tx).await.map_err(dberr)?;
        tx.commit().await.map_err(dberr)?;
        Ok(Some(id))
    }

    async fn google_oauth(&self) -> Result<Option<GoogleOAuth>> {
        let row = sqlx::query("SELECT google_client_id, google_client_secret FROM admin WHERE id = 1")
            .fetch_one(&self.pool).await.map_err(dberr)?;
        let id: Option<String> = row.get("google_client_id");
        let secret: Option<String> = row.get("google_client_secret");
        Ok(match (id, secret) {
            (Some(client_id), Some(client_secret)) => Some(GoogleOAuth { client_id, client_secret }),
            _ => None,
        })
    }

    async fn set_google_oauth(&self, g: &GoogleOAuth) -> Result<()> {
        sqlx::query("UPDATE admin SET google_client_id = ?, google_client_secret = ? WHERE id = 1")
            .bind(&g.client_id).bind(&g.client_secret).execute(&self.pool).await.map_err(dberr)?;
        Ok(())
    }

    async fn locked_secs(&self, ip: &str, now: i64) -> Result<Option<i64>> {
        let until: Option<i64> = sqlx::query_scalar("SELECT locked_until FROM rate_limit WHERE ip = ?")
            .bind(ip).fetch_optional(&self.pool).await.map_err(dberr)?.flatten();
        Ok(until.filter(|&u| u > now).map(|u| (u - now).max(1)))
    }

    async fn record_failure(&self, ip: &str, now: i64, window: i64, max: u32, lockout: i64) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(dberr)?;
        // Evict lapsed entries so cycling source addresses can't grow the table without bound.
        sqlx::query("DELETE FROM rate_limit WHERE window_start < ? AND (locked_until IS NULL OR locked_until <= ?)")
            .bind(now - window).bind(now).execute(&mut *tx).await.map_err(dberr)?;
        let row = sqlx::query("SELECT count, window_start FROM rate_limit WHERE ip = ?")
            .bind(ip).fetch_optional(&mut *tx).await.map_err(dberr)?;
        let (mut count, mut win_start) = match &row {
            Some(r) => (r.get::<i64, _>("count"), r.get::<i64, _>("window_start")),
            None => (0, now),
        };
        if now - win_start > window {
            count = 0;
            win_start = now;
        }
        count += 1;
        let locked_until = if count as u32 >= max {
            count = 0;
            win_start = now;
            Some(now + lockout)
        } else {
            None
        };
        sqlx::query("INSERT INTO rate_limit (ip, count, window_start, locked_until) VALUES (?, ?, ?, ?)
                     ON CONFLICT(ip) DO UPDATE SET count = excluded.count, window_start = excluded.window_start, locked_until = excluded.locked_until")
            .bind(ip).bind(count).bind(win_start).bind(locked_until).execute(&mut *tx).await.map_err(dberr)?;
        tx.commit().await.map_err(dberr)?;
        Ok(())
    }

    async fn clear_attempts(&self, ip: &str) -> Result<()> {
        sqlx::query("DELETE FROM rate_limit WHERE ip = ?").bind(ip).execute(&self.pool).await.map_err(dberr)?;
        Ok(())
    }

    async fn consume_totp_step(&self, id: &str, step: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(dberr)?;
        let prev: Option<i64> = sqlx::query_scalar("SELECT step FROM totp_step WHERE user_id = ?")
            .bind(id).fetch_optional(&mut *tx).await.map_err(dberr)?;
        if matches!(prev, Some(p) if step <= p) {
            return Ok(false);
        }
        sqlx::query("INSERT INTO totp_step (user_id, step) VALUES (?, ?)
                     ON CONFLICT(user_id) DO UPDATE SET step = excluded.step")
            .bind(id).bind(step).execute(&mut *tx).await.map_err(dberr)?;
        tx.commit().await.map_err(dberr)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(name: &str, hash: &str) -> TokenEntry {
        TokenEntry { name: name.into(), hash: hash.into() }
    }

    #[tokio::test]
    async fn import_legacy_is_once_only_and_seeds_users_tokens_admin() {
        let db = SqliteRepo::memory().await.unwrap();
        let file = AuthFile {
            password_hash: Some("phc-admin".into()),
            totp_secret: Some("SECRET".into()),
            api_tokens: vec![tok("laptop", "h1")],
            users: vec![WebUser {
                id: "alice".into(), name: "Alice".into(), email: Some("Alice@Example.com".into()),
                password_hash: Some("phc-alice".into()), totp_secret: None, invite_token: None,
                is_admin: false, tokens: vec![tok("phone", "h2")],
            }],
            google_oauth: Some(GoogleOAuth { client_id: "cid".into(), client_secret: "csec".into() }),
        };
        assert!(db.import_legacy(&file).await.unwrap(), "first import applies");
        assert_eq!(db.admin_password_hash().await.unwrap().as_deref(), Some("phc-admin"));
        assert_eq!(db.admin_totp_secret().await.unwrap().as_deref(), Some("SECRET"));
        // Admin token resolves to the admin id; alice's token resolves to alice.
        assert_eq!(db.resolve_token("h1").await.unwrap().as_deref(), Some(mgmt_store::ADMIN_USER));
        assert_eq!(db.resolve_token("h2").await.unwrap().as_deref(), Some("alice"));
        // Email is lowercased for lookup.
        assert_eq!(db.user_by_email("alice@example.com").await.unwrap().unwrap().id, "alice");
        assert!(db.google_oauth().await.unwrap().is_some());

        // A second import is a no-op (admin already configured) — no double-shift of state.
        assert!(!db.import_legacy(&file).await.unwrap(), "second import is a no-op");
    }

    #[tokio::test]
    async fn set_admin_password_is_once_per_deployment() {
        let db = SqliteRepo::memory().await.unwrap();
        assert!(db.set_admin_password_if_unset("first", None).await.unwrap());
        assert!(!db.set_admin_password_if_unset("second", None).await.unwrap(), "cannot re-claim");
        assert_eq!(db.admin_password_hash().await.unwrap().as_deref(), Some("first"));
        // But a deliberate change (setpass path) overwrites.
        assert!(db.set_password(mgmt_store::ADMIN_USER, "reset").await.unwrap());
        assert_eq!(db.admin_password_hash().await.unwrap().as_deref(), Some("reset"));
    }

    #[tokio::test]
    async fn totp_replay_and_rate_limit_are_durable_in_the_store() {
        let db = SqliteRepo::memory().await.unwrap();
        // Replay guard: the same step can't be consumed twice; a newer one can.
        assert!(db.consume_totp_step("admin", 100).await.unwrap());
        assert!(!db.consume_totp_step("admin", 100).await.unwrap(), "replay rejected");
        assert!(!db.consume_totp_step("admin", 99).await.unwrap(), "older rejected");
        assert!(db.consume_totp_step("admin", 101).await.unwrap(), "newer accepted");

        // Rate limit: 3 failures within the window locks out; the lock is queryable.
        for _ in 0..3 {
            db.record_failure("1.2.3.4", 1000, 60, 3, 300).await.unwrap();
        }
        assert!(db.locked_secs("1.2.3.4", 1000).await.unwrap().is_some(), "locked after max attempts");
        db.clear_attempts("1.2.3.4").await.unwrap();
        assert!(db.locked_secs("1.2.3.4", 1000).await.unwrap().is_none(), "cleared on success");
    }
}
