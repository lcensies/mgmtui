//! Building and verifying `tar.zst` snapshot archives.
//!
//! Archive layout:
//! ```text
//! data/<...>          # every file under the data root (tasks/, calendars/, projects/, .trash/, .state/)
//! config/config.yaml  # optional, when include_config is set
//! manifest.json       # format, timestamps, counts, and a SHA-256 per file
//! ```

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use mgmt_core::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Snapshot format version (bump on breaking archive-layout changes).
pub const FORMAT_VERSION: u32 = 1;
/// Name of the manifest entry at the archive root.
pub const MANIFEST_NAME: &str = "manifest.json";

const ZSTD_LEVEL: i32 = 3;

/// One file recorded in a snapshot's manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    /// Path within the archive, e.g. `data/tasks/<uid>.md`.
    pub path: String,
    pub size: u64,
    /// Lowercase hex SHA-256 of the file bytes.
    pub sha256: String,
}

/// Metadata embedded in each snapshot (also uploaded as a sidecar so listing avoids downloads).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub created_at: DateTime<Utc>,
    pub mgmt_version: String,
    pub hostname: String,
    pub task_count: usize,
    pub event_count: usize,
    pub project_count: usize,
    pub files: Vec<FileEntry>,
}

/// A sortable, host-tagged snapshot filename: `mgmt-<host>-YYYYMMDDTHHMMSSZ.tar.zst`.
pub fn snapshot_name(host: &str, now: DateTime<Utc>) -> String {
    format!("mgmt-{}-{}.tar.zst", sanitize(host), now.format("%Y%m%dT%H%M%SZ"))
}

/// Best-effort machine hostname for tagging snapshots. Falls back to `HOSTNAME`, `/etc/hostname`,
/// then `"host"`.
pub fn hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.trim().is_empty() {
            return h.trim().to_string();
        }
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        if !h.trim().is_empty() {
            return h.trim().to_string();
        }
    }
    "host".to_string()
}

fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '-' })
        .collect();
    if out.is_empty() { "host".into() } else { out }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Recursively collect `(archive_path, absolute_path)` pairs under `root`, prefixing each archive
/// path with `prefix`. Skips the backup staging dir, the backup lockfile, and `*.tmp` atomic-write
/// leftovers so a live-tree snapshot stays clean.
fn collect(root: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut stack = vec![(root.to_path_buf(), prefix.to_string())];
    while let Some((dir, arc_prefix)) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            let arc = format!("{arc_prefix}/{name}");
            if path.is_dir() {
                // Never recurse into our own staging directory.
                if name == "backup-tmp" {
                    continue;
                }
                stack.push((path, arc));
            } else {
                if name == "backup.lock" || name.ends_with(".tmp") {
                    continue;
                }
                out.push((arc, path));
            }
        }
    }
    out.sort();
    Ok(())
}

/// Create a snapshot archive at `out_archive` covering `data_root` (as `data/…`) and, when
/// `config_dir` is `Some`, its `config.yaml` and `web-auth.yaml` (as `config/…`). Returns the
/// manifest (also embedded in the archive).
pub fn create_snapshot(
    data_root: &Path,
    config_dir: Option<&Path>,
    out_archive: &Path,
    now: DateTime<Utc>,
) -> Result<Manifest> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    collect(data_root, "data", &mut entries)?;

    // Config is bundled selectively (never secrets we don't own): config.yaml + web-auth.yaml.
    if let Some(cfg_dir) = config_dir {
        for file in ["config.yaml", "web-auth.yaml"] {
            let p = cfg_dir.join(file);
            if p.is_file() {
                entries.push((format!("config/{file}"), p));
            }
        }
    }

    let (task_count, event_count, project_count) = counts(data_root);

    if let Some(parent) = out_archive.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(out_archive)?;
    let encoder = zstd::Encoder::new(file, ZSTD_LEVEL).map_err(Error::Io)?;
    let mut builder = tar::Builder::new(encoder);

    let mtime = now.timestamp().max(0) as u64;
    let mut manifest_files = Vec::with_capacity(entries.len());
    for (arc_path, abs) in &entries {
        let bytes = std::fs::read(abs)?;
        let sha = hex(&Sha256::digest(&bytes));
        append_file(&mut builder, arc_path, &bytes, mtime)?;
        manifest_files.push(FileEntry { path: arc_path.clone(), size: bytes.len() as u64, sha256: sha });
    }

    let manifest = Manifest {
        format: FORMAT_VERSION,
        created_at: now,
        mgmt_version: env!("CARGO_PKG_VERSION").to_string(),
        hostname: hostname(),
        task_count,
        event_count,
        project_count,
        files: manifest_files,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| Error::Other(format!("serializing manifest: {e}")))?;
    append_file(&mut builder, MANIFEST_NAME, &manifest_bytes, mtime)?;

    // Finish the tar (writes the trailer) then flush the zstd stream.
    let encoder = builder.into_inner().map_err(Error::Io)?;
    encoder.finish().map_err(Error::Io)?;
    Ok(manifest)
}

fn append_file<W: std::io::Write>(builder: &mut tar::Builder<W>, arc_path: &str, bytes: &[u8], mtime: u64) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(mtime);
    header.set_cksum();
    builder.append_data(&mut header, arc_path, bytes).map_err(Error::Io)?;
    Ok(())
}

/// Count `.md` tasks, `.ics` events, and `.md` projects on disk (best-effort, for the manifest).
fn counts(data_root: &Path) -> (usize, usize, usize) {
    let tasks = count_ext(&data_root.join("tasks"), "md");
    let events = count_ext(&data_root.join("calendars"), "ics");
    let projects = count_ext(&data_root.join("projects"), "md");
    (tasks, events, projects)
}

fn count_ext(root: &Path, ext: &str) -> usize {
    let mut n = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case(ext)).unwrap_or(false) {
                n += 1;
            }
        }
    }
    n
}

/// Decode `archive`, read its manifest, and verify every listed file's size and SHA-256 against the
/// archive contents. Returns the manifest on success, or an error naming the first mismatch.
pub fn verify_archive(archive: &Path) -> Result<Manifest> {
    // First pass: pull the manifest out.
    let manifest = read_manifest(archive)?;

    // Second pass: hash every non-manifest entry and check it against the manifest.
    let file = File::open(archive)?;
    let decoder = zstd::Decoder::new(file).map_err(Error::Io)?;
    let mut tar = tar::Archive::new(decoder);
    let mut seen = std::collections::HashMap::new();
    for entry in tar.entries().map_err(Error::Io)? {
        let mut entry = entry.map_err(Error::Io)?;
        let path = entry.path().map_err(Error::Io)?.to_string_lossy().to_string();
        if path == MANIFEST_NAME {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        seen.insert(path, (bytes.len() as u64, hex(&Sha256::digest(&bytes))));
    }

    for f in &manifest.files {
        match seen.get(&f.path) {
            None => return Err(Error::Invalid(format!("snapshot missing file listed in manifest: {}", f.path))),
            Some((size, sha)) => {
                if *size != f.size || *sha != f.sha256 {
                    return Err(Error::Invalid(format!("snapshot file corrupt (size/hash mismatch): {}", f.path)));
                }
            }
        }
    }
    Ok(manifest)
}

/// Read just the `manifest.json` entry from an archive without verifying file hashes.
pub fn read_manifest(archive: &Path) -> Result<Manifest> {
    let file = File::open(archive)?;
    let decoder = zstd::Decoder::new(file).map_err(Error::Io)?;
    let mut tar = tar::Archive::new(decoder);
    for entry in tar.entries().map_err(Error::Io)? {
        let mut entry = entry.map_err(Error::Io)?;
        let path = entry.path().map_err(Error::Io)?.to_string_lossy().to_string();
        if path == MANIFEST_NAME {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return serde_json::from_slice(&bytes)
                .map_err(|e| Error::Parse(format!("parsing snapshot manifest: {e}")));
        }
    }
    Err(Error::Invalid("snapshot has no manifest.json".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(root: &Path) {
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        std::fs::create_dir_all(root.join("calendars/personal")).unwrap();
        std::fs::create_dir_all(root.join("projects")).unwrap();
        std::fs::write(root.join("tasks/a.md"), "---\ntitle: A\n---\nbody").unwrap();
        std::fs::write(root.join("tasks/b.md"), "---\ntitle: B\n---\n").unwrap();
        std::fs::write(root.join("calendars/personal/e.ics"), "BEGIN:VEVENT\nEND:VEVENT").unwrap();
        std::fs::write(root.join("projects/wng.md"), "---\nname: wng\n---\n").unwrap();
        // things that must be excluded:
        std::fs::create_dir_all(root.join(".state/backup-tmp")).unwrap();
        std::fs::write(root.join(".state/backup-tmp/junk"), "x").unwrap();
        std::fs::write(root.join(".state/backup.lock"), "123").unwrap();
        std::fs::write(root.join("tasks/a.md.tmp"), "half-written").unwrap();
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-03T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn snapshot_name_is_sortable_and_tagged() {
        let name = snapshot_name("my host!", now());
        assert_eq!(name, "mgmt-my-host--20260703T120000Z.tar.zst");
    }

    #[test]
    fn create_and_verify_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        seed(&root);
        let archive = dir.path().join("snap.tar.zst");

        let manifest = create_snapshot(&root, None, &archive, now()).unwrap();
        assert_eq!(manifest.task_count, 2);
        assert_eq!(manifest.event_count, 1);
        assert_eq!(manifest.project_count, 1);
        // excluded files are absent from the manifest
        assert!(manifest.files.iter().all(|f| !f.path.contains("backup-tmp")));
        assert!(manifest.files.iter().all(|f| !f.path.ends_with(".tmp")));
        assert!(manifest.files.iter().all(|f| !f.path.ends_with("backup.lock")));
        assert!(manifest.files.iter().any(|f| f.path == "data/tasks/a.md"));

        // verify passes on a good archive
        let verified = verify_archive(&archive).unwrap();
        assert_eq!(verified.files.len(), manifest.files.len());
    }

    #[test]
    fn verify_detects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        seed(&root);
        let archive = dir.path().join("snap.tar.zst");
        create_snapshot(&root, None, &archive, now()).unwrap();

        // Truncate the archive to corrupt it.
        let mut bytes = std::fs::read(&archive).unwrap();
        bytes.truncate(bytes.len() / 2);
        std::fs::write(&archive, &bytes).unwrap();
        assert!(verify_archive(&archive).is_err());
    }

    #[test]
    fn bundles_config_when_requested() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        seed(&root);
        let cfg = dir.path().join("config");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("config.yaml"), "statuses: []").unwrap();
        let archive = dir.path().join("snap.tar.zst");

        let manifest = create_snapshot(&root, Some(&cfg), &archive, now()).unwrap();
        assert!(manifest.files.iter().any(|f| f.path == "config/config.yaml"));
    }
}
