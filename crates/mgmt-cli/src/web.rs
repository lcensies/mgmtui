//! `mgmt web` — serve the HTTP/JSON API + PWA, and manage its credentials (`web-auth.yaml`).

use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use clap::Subcommand;

use mgmt_config::Config;
use mgmt_web::auth::{new_api_token, new_totp_secret, TokenEntry};

#[derive(Subcommand)]
pub enum WebCmd {
    /// Serve the API + PWA (the default when `mgmt web` is run with no subcommand).
    Serve {
        /// Address to bind (overrides config `web.bind`).
        #[arg(long)]
        bind: Option<String>,
        /// Serve a built SPA from this directory (SPA fallback to index.html).
        #[arg(long, value_name = "DIR")]
        assets_dir: Option<PathBuf>,
        /// Run without a password even on a non-loopback bind (only sane behind a trusted network).
        #[arg(long)]
        no_auth: bool,
    },
    /// Set (or replace) the web password. Reads `MGMT_WEB_PASSWORD`, else prompts on stdin.
    Setpass,
    /// Enroll TOTP two-factor: generate a secret and print its otpauth:// URI.
    TotpEnroll,
    /// Remove TOTP two-factor.
    TotpDisable,
    /// Create a new API token for the desktop sync client (printed once).
    TokenNew {
        /// A label for the token (e.g. `laptop`).
        name: String,
    },
    /// List provisioned API tokens.
    TokenList,
    /// Revoke an API token by name.
    TokenRevoke {
        name: String,
    },
}

fn config_dir() -> Result<PathBuf> {
    let path = Config::default_path().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(path.parent().map(|p| p.to_path_buf()).unwrap_or_default())
}

fn auth_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("web-auth.yaml"))
}

fn anyerr(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!(e.to_string())
}

/// Open the SQLite credential store for a data root (importing a legacy `web-auth.yaml` once). The
/// credential subcommands run synchronously, so they own a small runtime and `block_on`.
fn open_creds(root: &Path) -> Result<(tokio::runtime::Runtime, mgmt_web::auth::CredStore)> {
    let rt = tokio::runtime::Runtime::new()?;
    let db = mgmt_web::auth_db_path(root);
    let legacy = auth_path()?;
    let creds = rt.block_on(mgmt_web::auth::CredStore::open(&db, Some(&legacy))).map_err(anyerr)?;
    Ok((rt, creds))
}

pub fn run_web(root: &PathBuf, cfg: Config, cmd: Option<WebCmd>) -> Result<()> {
    match cmd.unwrap_or(WebCmd::Serve { bind: None, assets_dir: None, no_auth: false }) {
        WebCmd::Serve { bind, assets_dir, no_auth } => serve(root, cfg, bind, assets_dir, no_auth),
        WebCmd::Setpass => setpass(root),
        WebCmd::TotpEnroll => totp_enroll(root),
        WebCmd::TotpDisable => totp_disable(root),
        WebCmd::TokenNew { name } => token_new(root, name),
        WebCmd::TokenList => token_list(root),
        WebCmd::TokenRevoke { name } => token_revoke(root, name),
    }
}

fn serve(root: &PathBuf, cfg: Config, bind: Option<String>, assets_dir: Option<PathBuf>, no_auth: bool) -> Result<()> {
    let web = cfg.web();
    let bind_str = bind.unwrap_or_else(|| web.bind.clone());
    let addr: SocketAddr = bind_str
        .to_socket_addrs()
        .with_context(|| format!("invalid bind address '{bind_str}'"))?
        .next()
        .with_context(|| format!("could not resolve bind address '{bind_str}'"))?;
    let public_origin = if web.public_origin.trim().is_empty() {
        None
    } else {
        Some(web.public_origin.clone())
    };
    let opts = mgmt_web::WebOptions {
        bind: addr,
        assets_dir,
        auth_file: auth_path()?,
        caldav_file: config_dir()?.join("caldav.yaml"),
        public_origin,
        session_ttl_days: web.session_ttl_days,
        no_auth,
    };
    mgmt_web::run(root.clone(), cfg, opts).map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn read_password() -> Result<String> {
    if let Ok(p) = std::env::var("MGMT_WEB_PASSWORD") {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    print!("New web password: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let pw = line.trim().to_string();
    if pw.is_empty() {
        bail!("password is empty");
    }
    Ok(pw)
}

fn setpass(root: &Path) -> Result<()> {
    let pw = read_password()?;
    let (rt, creds) = open_creds(root)?;
    rt.block_on(creds.force_set_admin_password(&pw)).map_err(anyerr)?;
    println!("web password set. Restart a running `mgmt web` for it to take effect.");
    Ok(())
}

fn totp_enroll(root: &Path) -> Result<()> {
    let (secret, uri) = new_totp_secret();
    let (rt, creds) = open_creds(root)?;
    rt.block_on(creds.set_admin_totp(Some(&secret))).map_err(anyerr)?;
    println!("TOTP enrolled. Add this to your authenticator app:");
    println!("  {uri}");
    println!("  (secret: {secret})");
    Ok(())
}

fn totp_disable(root: &Path) -> Result<()> {
    let (rt, creds) = open_creds(root)?;
    rt.block_on(creds.set_admin_totp(None)).map_err(anyerr)?;
    println!("TOTP disabled.");
    Ok(())
}

fn token_new(root: &Path, name: String) -> Result<()> {
    let (token, hash) = new_api_token();
    let (rt, creds) = open_creds(root)?;
    // Replaces a same-named token (INSERT OR REPLACE keyed on owner+name).
    rt.block_on(creds.add_token(mgmt_store::ADMIN_USER, &TokenEntry { name: name.clone(), hash })).map_err(anyerr)?;
    println!("token '{name}' created — shown once, store it now:");
    println!("  {token}");
    Ok(())
}

fn token_list(root: &Path) -> Result<()> {
    let (rt, creds) = open_creds(root)?;
    let tokens = rt.block_on(creds.admin_tokens()).map_err(anyerr)?;
    if tokens.is_empty() {
        println!("no API tokens");
    } else {
        for t in &tokens {
            println!("{}", t.name);
        }
    }
    Ok(())
}

fn token_revoke(root: &Path, name: String) -> Result<()> {
    let (rt, creds) = open_creds(root)?;
    if !rt.block_on(creds.revoke_token(mgmt_store::ADMIN_USER, &name)).map_err(anyerr)? {
        bail!("no token named '{name}'");
    }
    println!("revoked token '{name}'.");
    Ok(())
}
