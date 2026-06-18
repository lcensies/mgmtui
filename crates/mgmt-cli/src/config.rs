//! Configuration loaded from `$XDG_CONFIG_HOME/mgmt/config.toml`. Describes CalDAV accounts
//! and the collections that mirror them.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::Deserialize;

use mgmt_sync::Auth;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    #[serde(default, rename = "account")]
    pub accounts: Vec<Account>,
    #[serde(default, rename = "collection")]
    pub collections: Vec<Collection>,
}

#[derive(Debug, Deserialize)]
pub struct Account {
    pub name: String,
    /// `basic`, `bearer`, or `none`.
    #[serde(default = "default_auth_kind")]
    pub auth: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

fn default_auth_kind() -> String {
    "basic".into()
}

#[derive(Debug, Deserialize)]
pub struct Collection {
    /// Local collection / vault-project name.
    pub name: String,
    /// `events` or `tasks`.
    pub kind: String,
    /// Remote CalDAV collection URL.
    pub url: String,
    /// Name of the account block to authenticate with.
    pub account: String,
}

impl Account {
    pub fn to_auth(&self) -> Result<Auth> {
        Ok(match self.auth.as_str() {
            "none" => Auth::None,
            "bearer" => Auth::Bearer {
                token: self.token.clone().context("account.token required for bearer auth")?,
            },
            "basic" => Auth::Basic {
                user: self.username.clone().context("account.username required for basic auth")?,
                password: self.password.clone().context("account.password required for basic auth")?,
            },
            other => anyhow::bail!("unknown auth kind: {other}"),
        })
    }
}

impl Config {
    /// Default config path: `$XDG_CONFIG_HOME/mgmt/config.toml`.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "mgmt").context("cannot resolve config dir")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load config from `path`, returning an empty config if it does not exist.
    pub fn load(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn account(&self, name: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_accounts_and_collections() {
        let toml = r#"
[[account]]
name = "home"
auth = "basic"
username = "u"
password = "p"

[[collection]]
name = "work"
kind = "events"
url = "http://localhost:4000/dav/cal/work/"
account = "home"
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.accounts.len(), 1);
        assert_eq!(cfg.collections.len(), 1);
        assert!(matches!(cfg.account("home").unwrap().to_auth().unwrap(), Auth::Basic { .. }));
        assert_eq!(cfg.collections[0].kind, "events");
    }

    #[test]
    fn missing_file_is_empty_config() {
        let cfg = Config::load(&PathBuf::from("/nonexistent/mgmt/config.toml")).unwrap();
        assert!(cfg.accounts.is_empty());
    }
}
