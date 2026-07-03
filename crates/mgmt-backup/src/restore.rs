//! Restoring a snapshot. Restores are never done in place: we extract to a sibling staging
//! directory on the same filesystem, then swap it in with two atomic renames, keeping the old
//! tree as `<data_root>.pre-restore-<ts>` for manual cleanup.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use mgmt_core::{Error, Result};

use crate::snapshot::{read_manifest, Manifest};

/// Where a swap-in restore left things.
#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    /// The archive's manifest.
    pub manifest: Manifest,
    /// The previous data root, renamed aside and NOT deleted (delete it yourself once satisfied).
    pub pre_restore: PathBuf,
    /// How many files were written.
    pub files_restored: usize,
}

/// Reject archive paths that would escape the destination (absolute, or containing `..`).
fn safe_join(dest: &Path, arc_path: &str) -> Result<PathBuf> {
    let rel = Path::new(arc_path);
    for comp in rel.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(Error::Invalid(format!("unsafe path in snapshot: {arc_path}"))),
        }
    }
    Ok(dest.join(rel))
}

/// Extract the entire archive (including `config/` and `manifest.json`) under `dest_dir`,
/// preserving structure. Used by `mgmt restore --to <dir>` for inspection. Returns the manifest.
pub fn extract_all(archive: &Path, dest_dir: &Path) -> Result<Manifest> {
    std::fs::create_dir_all(dest_dir)?;
    extract_entries(archive, |arc_path| Some(arc_path.to_string()), dest_dir)?;
    read_manifest(archive)
}

/// Extract only the `data/…` subtree of the archive into `dest_data_root` (stripping the `data/`
/// prefix), so `dest_data_root` becomes a fresh data root.
fn extract_data(archive: &Path, dest_data_root: &Path) -> Result<usize> {
    std::fs::create_dir_all(dest_data_root)?;
    extract_entries(
        archive,
        |arc_path| arc_path.strip_prefix("data/").map(|s| s.to_string()),
        dest_data_root,
    )
}

/// Walk archive entries; for each, `remap` decides the destination-relative path (or `None` to
/// skip). Returns the number of files written.
fn extract_entries<F>(archive: &Path, remap: F, dest: &Path) -> Result<usize>
where
    F: Fn(&str) -> Option<String>,
{
    let file = std::fs::File::open(archive)?;
    let decoder = zstd::Decoder::new(file).map_err(Error::Io)?;
    let mut tar = tar::Archive::new(decoder);
    let mut written = 0;
    for entry in tar.entries().map_err(Error::Io)? {
        let mut entry = entry.map_err(Error::Io)?;
        let arc_path = entry.path().map_err(Error::Io)?.to_string_lossy().to_string();
        let Some(rel) = remap(&arc_path) else { continue };
        if rel.is_empty() {
            continue;
        }
        let out_path = safe_join(dest, &rel)?;
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        std::fs::write(&out_path, &bytes)?;
        written += 1;
    }
    Ok(written)
}

/// Verify `archive`, extract its `data/` subtree to a sibling staging dir, then swap it in for
/// `data_root` with two atomic renames. The caller is responsible for confirming with the user and
/// for refusing while services hold the vault. Returns the pre-restore path (never auto-deleted).
pub fn restore_swap(archive: &Path, data_root: &Path, now: DateTime<Utc>) -> Result<RestoreOutcome> {
    let manifest = crate::snapshot::verify_archive(archive)?;

    let ts = now.format("%Y%m%dT%H%M%SZ");
    let incoming = sibling(data_root, &format!(".incoming-{ts}"))?;
    let pre_restore = sibling(data_root, &format!(".pre-restore-{ts}"))?;

    // Clean any leftover staging from a previous aborted run.
    if incoming.exists() {
        std::fs::remove_dir_all(&incoming)?;
    }
    let files_restored = extract_data(archive, &incoming)?;

    // Swap: move the current tree aside, then move the new tree into place. If the second rename
    // fails, put the original back so we never leave the data root missing.
    if data_root.exists() {
        std::fs::rename(data_root, &pre_restore)?;
    }
    if let Err(e) = std::fs::rename(&incoming, data_root) {
        if pre_restore.exists() {
            let _ = std::fs::rename(&pre_restore, data_root);
        }
        return Err(Error::Io(e));
    }

    Ok(RestoreOutcome { manifest, pre_restore, files_restored })
}

/// Build a sibling path `<data_root><suffix>` next to `data_root` (same parent → same filesystem,
/// so the rename swap is atomic).
fn sibling(data_root: &Path, suffix: &str) -> Result<PathBuf> {
    let parent = data_root.parent().ok_or_else(|| Error::Invalid("data root has no parent directory".into()))?;
    let name = data_root
        .file_name()
        .ok_or_else(|| Error::Invalid("data root has no final component".into()))?
        .to_string_lossy();
    Ok(parent.join(format!("{name}{suffix}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{create_snapshot, MANIFEST_NAME};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-03T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn seed(root: &Path) {
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        std::fs::write(root.join("tasks/a.md"), "original-a").unwrap();
        std::fs::write(root.join("tasks/b.md"), "original-b").unwrap();
    }

    #[test]
    fn extract_all_round_trips_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        seed(&root);
        let archive = dir.path().join("snap.tar.zst");
        create_snapshot(&root, None, &archive, now()).unwrap();

        let out = dir.path().join("inspect");
        extract_all(&archive, &out).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("data/tasks/a.md")).unwrap(), "original-a");
        assert!(out.join(MANIFEST_NAME).exists());
    }

    #[test]
    fn swap_restores_and_keeps_pre_restore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        seed(&root);
        let archive = dir.path().join("snap.tar.zst");
        create_snapshot(&root, None, &archive, now()).unwrap();

        // Mutate the live tree after the snapshot.
        std::fs::write(root.join("tasks/a.md"), "MUTATED").unwrap();
        std::fs::write(root.join("tasks/c.md"), "new-since-snapshot").unwrap();

        let outcome = restore_swap(&archive, &root, now()).unwrap();
        // Restored content matches the snapshot, not the mutation.
        assert_eq!(std::fs::read_to_string(root.join("tasks/a.md")).unwrap(), "original-a");
        // File created after the snapshot is gone from the restored tree...
        assert!(!root.join("tasks/c.md").exists());
        // ...but preserved in the pre-restore copy.
        assert!(outcome.pre_restore.exists());
        assert_eq!(std::fs::read_to_string(outcome.pre_restore.join("tasks/c.md")).unwrap(), "new-since-snapshot");
    }

    #[test]
    fn rejects_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(safe_join(dir.path(), "../escape").is_err());
        assert!(safe_join(dir.path(), "/etc/passwd").is_err());
        assert!(safe_join(dir.path(), "data/ok.md").is_ok());
    }
}
