//! Filesystem layout and helpers shared by the vault and vdir stores.

use std::path::{Path, PathBuf};

use mgmt_core::{Error, Result};

/// Resolve the mgmt data root (`$XDG_DATA_HOME/mgmt`, falling back to `~/.local/share/mgmt`).
pub fn data_root() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "mgmt")
        .ok_or_else(|| Error::Other("cannot resolve home directory".into()))?;
    Ok(dirs.data_dir().to_path_buf())
}

/// The implicit id of the single web-login user whose vault the local CLI/TUI and the web UI share.
pub const ADMIN_USER: &str = "admin";

/// The directory holding one isolated vault subtree per user (multi-user layout).
pub fn users_dir(root: &Path) -> PathBuf {
    root.join("users")
}

/// A specific user's isolated vault root, `<data_root>/users/<id>`.
pub fn user_root(root: &Path, id: &str) -> PathBuf {
    users_dir(root).join(id)
}

/// The effective vault root for local (single-user) CLI/TUI/daemon operations. Once a data root has
/// been migrated to the multi-user layout (a `users/` directory exists) this is `users/admin`;
/// otherwise it is the legacy root itself, so pre-migration installs keep working unchanged.
pub fn local_vault_root(root: &Path) -> PathBuf {
    if users_dir(root).exists() {
        user_root(root, ADMIN_USER)
    } else {
        root.to_path_buf()
    }
}

/// Migrate a legacy single-vault data root into the multi-user layout: move `tasks/`, `calendars/`,
/// `projects/`, and `.trash/` under `users/admin/`. Idempotent (a no-op once `users/admin` exists)
/// and safe on an empty root (it just establishes the layout). Returns `true` when legacy vault
/// data was actually moved. Global web state (`.state/web-sessions.json`) intentionally stays at the
/// data-root level, so it is not moved.
pub fn migrate_to_multiuser(root: &Path) -> Result<bool> {
    let admin = user_root(root, ADMIN_USER);
    if admin.exists() {
        return Ok(false); // already migrated
    }
    let movable = ["tasks", "calendars", "projects", ".trash"];
    let moved = movable.iter().any(|d| root.join(d).exists());
    std::fs::create_dir_all(&admin)?;
    for d in movable {
        let from = root.join(d);
        if from.exists() {
            std::fs::rename(&from, admin.join(d))?;
        }
    }
    Ok(moved)
}

/// The tasks vault directory under a data root.
pub fn tasks_dir(root: &Path) -> PathBuf {
    root.join("tasks")
}

/// The calendars (vdir) directory under a data root.
pub fn calendars_dir(root: &Path) -> PathBuf {
    root.join("calendars")
}

/// The directory holding one markdown file per project (portable, hand-editable).
pub fn projects_dir(root: &Path) -> PathBuf {
    root.join("projects")
}

/// Make a UID safe to use as a filename stem (UUIDs pass through unchanged; imported UIDs
/// containing path separators or other awkward bytes are sanitized).
pub fn safe_stem(uid: &str) -> String {
    let mut out: String = uid
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Atomically write `contents` to `path` by writing a sibling temp file and renaming. The temp
/// name is unique per process *and* per call — the daemon, CLI, and web server write the same
/// vault concurrently, and a shared `<file>.tmp` would let one writer publish another's torn
/// bytes under the final name.
pub fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.{}-{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::write(&tmp, contents)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Recursively collect files under `root` whose extension equals `ext` (case-insensitive).
/// Returns an empty vec if `root` does not exist.
pub fn collect_files(root: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case(ext)).unwrap_or(false) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_stem_sanitizes_separators() {
        assert_eq!(safe_stem("abc-123"), "abc-123");
        assert_eq!(safe_stem("a/b@c.d"), "a_b_c.d");
        assert_eq!(safe_stem(""), "_");
    }

    #[test]
    fn migrate_moves_legacy_vault_under_users_admin() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // A legacy single-vault layout.
        std::fs::create_dir_all(tasks_dir(root)).unwrap();
        std::fs::write(tasks_dir(root).join("a.md"), "x").unwrap();
        assert_eq!(local_vault_root(root), root, "pre-migration root is the legacy root");

        let moved = migrate_to_multiuser(root).unwrap();
        assert!(moved, "reports it moved legacy data");
        let admin = user_root(root, ADMIN_USER);
        assert!(admin.join("tasks").join("a.md").exists(), "task moved under users/admin");
        assert!(!tasks_dir(root).exists(), "legacy tasks dir is gone");
        assert_eq!(local_vault_root(root), admin, "post-migration root is users/admin");

        // Idempotent.
        assert!(!migrate_to_multiuser(root).unwrap(), "second migrate is a no-op");
    }

    #[test]
    fn atomic_write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("x.md");
        atomic_write(&p, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
        // no leftover temp files
        let leftovers: Vec<_> = collect_files(dir.path(), "tmp").unwrap();
        assert!(leftovers.is_empty());
    }
}
