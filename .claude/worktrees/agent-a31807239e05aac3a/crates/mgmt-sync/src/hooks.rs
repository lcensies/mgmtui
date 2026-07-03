//! Pre/post-sync hooks — executable scripts in the hooks directory, mirroring calcurse's
//! hook model. A missing hook is simply skipped.

use std::path::Path;
use std::process::Command;

use mgmt_core::{Error, Result};

/// Run `<hooks_dir>/<name>` if it exists. Returns `Ok(true)` if a hook ran successfully,
/// `Ok(false)` if no such hook exists, and an error if the hook ran but failed.
pub fn run_hook(hooks_dir: &Path, name: &str) -> Result<bool> {
    let path = hooks_dir.join(name);
    if !path.exists() {
        return Ok(false);
    }
    let status = Command::new(&path)
        .current_dir(hooks_dir)
        .status()
        .map_err(Error::Io)?;
    if status.success() {
        Ok(true)
    } else {
        Err(Error::Other(format!("hook {name} exited with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_hook_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!run_hook(dir.path(), "pre-sync").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn successful_hook_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let hook = dir.path().join("pre-sync");
        std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(run_hook(dir.path(), "pre-sync").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn failing_hook_errors() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let hook = dir.path().join("post-sync");
        std::fs::write(&hook, "#!/bin/sh\nexit 3\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(run_hook(dir.path(), "post-sync").is_err());
    }
}
