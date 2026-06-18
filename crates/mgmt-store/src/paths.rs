//! Filesystem layout and helpers shared by the vault and vdir stores.

use std::path::{Path, PathBuf};

use mgmt_core::{Error, Result};

/// Resolve the mgmt data root (`$XDG_DATA_HOME/mgmt`, falling back to `~/.local/share/mgmt`).
pub fn data_root() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "mgmt")
        .ok_or_else(|| Error::Other("cannot resolve home directory".into()))?;
    Ok(dirs.data_dir().to_path_buf())
}

/// The tasks vault directory under a data root.
pub fn tasks_dir(root: &Path) -> PathBuf {
    root.join("tasks")
}

/// The calendars (vdir) directory under a data root.
pub fn calendars_dir(root: &Path) -> PathBuf {
    root.join("calendars")
}

/// The newline-delimited file that records known project names (including empty ones).
pub fn projects_file(root: &Path) -> PathBuf {
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

/// Atomically write `contents` to `path` by writing a sibling temp file and renaming.
pub fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
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
