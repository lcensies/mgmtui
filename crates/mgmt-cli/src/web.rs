//! `mgmt web` — serve the HTTP/JSON API + PWA, and manage its credentials (`web-auth.yaml`).

use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;

use anyhow::{bail, Context as _, Result};
use clap::Subcommand;

use mgmt_config::Config;
use mgmt_web::auth::{hash_password, new_api_token, new_totp_secret, AuthFile, TokenEntry};

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

fn load_auth() -> Result<AuthFile> {
    AuthFile::load(&auth_path()?).map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn save_auth(file: &AuthFile) -> Result<()> {
    file.save(&auth_path()?).map_err(|e| anyhow::anyhow!(e.to_string()))
}

pub fn run_web(root: &PathBuf, cfg: Config, cmd: Option<WebCmd>) -> Result<()> {
    match cmd.unwrap_or(WebCmd::Serve { bind: None, assets_dir: None, no_auth: false }) {
        WebCmd::Serve { bind, assets_dir, no_auth } => serve(root, cfg, bind, assets_dir, no_auth),
        WebCmd::Setpass => setpass(),
        WebCmd::TotpEnroll => totp_enroll(),
        WebCmd::TotpDisable => totp_disable(),
        WebCmd::TokenNew { name } => token_new(name),
        WebCmd::TokenList => token_list(),
        WebCmd::TokenRevoke { name } => token_revoke(name),
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

fn setpass() -> Result<()> {
    let pw = read_password()?;
    let mut file = load_auth()?;
    file.password_hash = Some(hash_password(&pw).map_err(|e| anyhow::anyhow!(e.to_string()))?);
    save_auth(&file)?;
    println!("web password set ({}).", auth_path()?.display());
    Ok(())
}

fn totp_enroll() -> Result<()> {
    let (secret, uri) = new_totp_secret();
    let mut file = load_auth()?;
    file.totp_secret = Some(secret.clone());
    save_auth(&file)?;
    println!("TOTP enrolled. Add this to your authenticator app:");
    println!("  {uri}");
    println!("  (secret: {secret})");
    Ok(())
}

fn totp_disable() -> Result<()> {
    let mut file = load_auth()?;
    file.totp_secret = None;
    save_auth(&file)?;
    println!("TOTP disabled.");
    Ok(())
}

fn token_new(name: String) -> Result<()> {
    let (token, hash) = new_api_token();
    let mut file = load_auth()?;
    file.api_tokens.retain(|t| t.name != name); // replace a same-named token
    file.api_tokens.push(TokenEntry { name: name.clone(), hash });
    save_auth(&file)?;
    println!("token '{name}' created — shown once, store it now:");
    println!("  {token}");
    Ok(())
}

fn token_list() -> Result<()> {
    let file = load_auth()?;
    if file.api_tokens.is_empty() {
        println!("no API tokens");
    } else {
        for t in &file.api_tokens {
            println!("{}", t.name);
        }
    }
    Ok(())
}

fn token_revoke(name: String) -> Result<()> {
    let mut file = load_auth()?;
    let before = file.api_tokens.len();
    file.api_tokens.retain(|t| t.name != name);
    if file.api_tokens.len() == before {
        bail!("no token named '{name}'");
    }
    save_auth(&file)?;
    println!("revoked token '{name}'.");
    Ok(())
}
