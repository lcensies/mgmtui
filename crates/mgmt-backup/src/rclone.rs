//! Thin wrapper over the external `rclone` binary. mgmt stores only the remote *name*
//! (e.g. `crypt-b2:mgmt-backups`); the crypt passphrase and provider credentials live in
//! `rclone.conf`, so encryption and the choice of provider are entirely rclone's concern.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use mgmt_core::{Error, Result};
use serde::Deserialize;

/// A remote file as reported by `rclone lsjson`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteFile {
    pub name: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub is_dir: bool,
    #[serde(default)]
    pub mod_time: Option<String>,
}

/// Handle to the `rclone` executable.
#[derive(Debug, Clone)]
pub struct Rclone {
    pub binary: PathBuf,
}

impl Rclone {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Rclone { binary: binary.into() }
    }

    /// Run `rclone <args>`, mapping a missing binary to a friendly [`Error::NotFound`] (mirrors the
    /// rustical-spawn precedent) and a non-zero exit to [`Error::Other`] carrying stderr.
    fn run(&self, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new(&self.binary).args(args).output().map_err(|e| match e.kind() {
            ErrorKind::NotFound => Error::NotFound(format!(
                "rclone not found ('{}') — install rclone or set backup.rclone_binary",
                self.binary.display()
            )),
            _ => Error::Io(e),
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Other(format!(
                "rclone {} failed: {}",
                args.first().copied().unwrap_or(""),
                stderr.trim()
            )));
        }
        Ok(output)
    }

    /// Verify rclone is installed and runnable (`rclone version`).
    pub fn preflight(&self) -> Result<()> {
        self.run(&["version"]).map(|_| ())
    }

    /// Upload a single local file to an exact remote path (`rclone copyto <local> <remote>`).
    pub fn copyto(&self, local: &Path, remote_path: &str) -> Result<()> {
        let local = local.to_string_lossy();
        self.run(&["copyto", &local, remote_path]).map(|_| ())
    }

    /// List the files in a remote directory (`rclone lsjson --files-only <remote_dir>`).
    pub fn lsjson(&self, remote_dir: &str) -> Result<Vec<RemoteFile>> {
        let output = self.run(&["lsjson", "--files-only", remote_dir])?;
        serde_json::from_slice(&output.stdout)
            .map_err(|e| Error::Parse(format!("parsing rclone lsjson output: {e}")))
    }

    /// Delete a single remote file (`rclone deletefile <remote_path>`).
    pub fn deletefile(&self, remote_path: &str) -> Result<()> {
        self.run(&["deletefile", remote_path]).map(|_| ())
    }

    /// Download a single remote file to a local path (`rclone copyto <remote> <local>`).
    pub fn fetch(&self, remote_path: &str, local: &Path) -> Result<()> {
        let local = local.to_string_lossy();
        self.run(&["copyto", remote_path, &local]).map(|_| ())
    }

    /// Best-effort check that `remote` is an rclone *crypt* remote (so uploads are encrypted).
    /// Returns `Ok(false)` if it can't tell (e.g. rclone too old); never fails the backup.
    pub fn is_crypt_remote(&self, remote: &str) -> bool {
        // `remote` is like "crypt-b2:path" — the config section is the part before ':'.
        let Some(name) = remote.split(':').next() else { return false };
        let Ok(output) = self.run(&["config", "show", name]) else { return false };
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines().any(|l| {
            let l = l.trim();
            l.eq_ignore_ascii_case("type = crypt") || l.replace(' ', "").eq_ignore_ascii_case("type=crypt")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binary_is_not_found_error() {
        let rc = Rclone::new("definitely-not-a-real-binary-xyz");
        let err = rc.preflight().unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn parses_lsjson_shape() {
        let json = r#"[{"Path":"a.tar.zst","Name":"a.tar.zst","Size":1234,"IsDir":false,"ModTime":"2026-07-03T12:00:00Z"}]"#;
        let files: Vec<RemoteFile> = serde_json::from_str(json).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "a.tar.zst");
        assert_eq!(files[0].size, 1234);
        assert!(!files[0].is_dir);
    }
}
