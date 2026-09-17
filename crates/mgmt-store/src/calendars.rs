//! Calendar metadata sidecar (`<vault>/.state/calendars.yaml`).
//!
//! Subscription URLs and feed tokens have no iCalendar home, so they live beside the vdir as a
//! list of [`Collection`]s. Only calendars that carry such metadata appear here; a plain local
//! calendar is just a directory under `calendars/`.

use std::path::{Path, PathBuf};

use mgmt_core::{Error, Result};
use mgmt_domain::Collection;

use crate::paths;

pub fn calendars_meta_path(vault_root: &Path) -> PathBuf {
    vault_root.join(".state").join("calendars.yaml")
}

/// Load the sidecar; a missing (or unreadable) file means "no calendar metadata".
pub fn load_calendars(vault_root: &Path) -> Result<Vec<Collection>> {
    let path = calendars_meta_path(vault_root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)?;
    serde_yaml::from_str(&text).map_err(|e| Error::Parse(format!("parsing {}: {e}", path.display())))
}

/// Write the sidecar. It holds feed tokens (bearer secrets), so it is kept owner-only.
pub fn save_calendars(vault_root: &Path, cols: &[Collection]) -> Result<()> {
    let path = calendars_meta_path(vault_root);
    let text = serde_yaml::to_string(cols).map_err(|e| Error::Other(format!("serializing calendars.yaml: {e}")))?;
    paths::atomic_write(&path, &text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mgmt_domain::{CollectionKind, RemoteSource};

    #[test]
    fn round_trips_and_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_calendars(dir.path()).unwrap().is_empty());
        let mut c = Collection::local("holidays", CollectionKind::Events);
        c.remote = Some(RemoteSource::Ics { url: "https://ex.org/h.ics".into(), refresh_minutes: 60 });
        c.feed_token = Some("secret".into());
        save_calendars(dir.path(), std::slice::from_ref(&c)).unwrap();
        assert_eq!(load_calendars(dir.path()).unwrap(), vec![c]);
    }
}
