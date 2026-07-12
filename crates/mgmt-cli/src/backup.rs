//! `mgmt backup` / `mgmt restore` — encrypted, provider-agnostic vault snapshots via rclone.
//!
//! A run builds a `tar.zst` snapshot of the data root (and, optionally, the config), uploads it
//! plus a small sidecar manifest to an rclone remote, then prunes old snapshots per the retention
//! policy. Point the remote at an `rclone crypt` wrapper for at-rest encryption — mgmt never holds
//! the passphrase.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use clap::Subcommand;

use mgmt_backup::{
    create_snapshot, extract_all, plan_prune, restore_swap, snapshot_name, verify_archive, hostname,
    Rclone, RetentionPolicy, SnapshotMeta,
};
use mgmt_config::{BackupCfg, Config};

#[derive(Subcommand)]
pub enum BackupCmd {
    /// Snapshot the vault, upload it to the remote, and prune per the retention policy.
    Run {
        /// Build the snapshot but print what would be uploaded/pruned instead of doing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// List snapshots on the remote (newest first).
    List,
    /// Apply the retention policy to the remote without taking a new snapshot.
    Prune {
        #[arg(long)]
        dry_run: bool,
    },
    /// Verify snapshots on the remote. With a name: download + full hash check. `--deep`: check all.
    Verify {
        /// Snapshot filename (or unique prefix) to deep-verify. Omit to check sidecars.
        name: Option<String>,
        /// Download and hash-verify every snapshot, not just sidecar presence.
        #[arg(long)]
        deep: bool,
    },
}

/// Resolve the enabled backup config or explain that backups are off.
fn backup_cfg(cfg: &Config) -> Result<&BackupCfg> {
    cfg.backup().context(
        "backups are disabled — add a `backup:` section with a `remote:` to your config.yaml",
    )
}

fn config_dir() -> Result<PathBuf> {
    let path = Config::default_path().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(path.parent().map(|p| p.to_path_buf()).unwrap_or_default())
}

fn state_dir(root: &Path) -> PathBuf {
    root.join(".state")
}

/// Join an rclone `remote:path` base with a filename.
fn remote_join(remote: &str, name: &str) -> String {
    format!("{}/{}", remote.trim_end_matches('/'), name)
}

/// Sidecar manifest filename for an archive (`mgmt-…-ts.tar.zst` → `mgmt-…-ts.manifest.json`).
fn sidecar_name(archive: &str) -> String {
    let stem = archive.strip_suffix(".tar.zst").unwrap_or(archive);
    format!("{stem}.manifest.json")
}

/// Parse the embedded UTC timestamp from a snapshot filename.
fn parse_snapshot_time(name: &str) -> Option<DateTime<Utc>> {
    let core = name.strip_suffix(".tar.zst")?;
    let ts = core.rsplit('-').next()?;
    NaiveDateTime::parse_from_str(ts, "%Y%m%dT%H%M%SZ").ok().map(|n| n.and_utc())
}

/// List `.tar.zst` snapshots on the remote as sortable metadata (newest first).
fn list_snapshots(rclone: &Rclone, remote: &str) -> Result<Vec<SnapshotMeta>> {
    let files = rclone.lsjson(remote).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut snaps: Vec<SnapshotMeta> = files
        .into_iter()
        .filter(|f| !f.is_dir && f.name.ends_with(".tar.zst"))
        .filter_map(|f| {
            let created_at = parse_snapshot_time(&f.name)?;
            Some(SnapshotMeta { name: f.name, created_at, size: f.size.max(0) as u64 })
        })
        .collect();
    snaps.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(snaps)
}

fn policy(cfg: &BackupCfg) -> RetentionPolicy {
    RetentionPolicy {
        keep_last: cfg.keep_last,
        keep_days: if cfg.keep_days == 0 { None } else { Some(cfg.keep_days) },
    }
}

pub fn run_backup(root: &Path, cfg: &Config, cmd: BackupCmd) -> Result<()> {
    let bcfg = backup_cfg(cfg)?;
    let rclone = Rclone::new(&bcfg.rclone_binary);
    match cmd {
        BackupCmd::Run { dry_run } => run(root, bcfg, &rclone, dry_run),
        BackupCmd::List => list(&rclone, bcfg),
        BackupCmd::Prune { dry_run } => prune(&rclone, bcfg, dry_run),
        BackupCmd::Verify { name, deep } => verify(&rclone, bcfg, name, deep),
    }
}

fn run(root: &Path, bcfg: &BackupCfg, rclone: &Rclone, dry_run: bool) -> Result<()> {
    rclone.preflight().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if !rclone.is_crypt_remote(&bcfg.remote) {
        eprintln!(
            "warning: remote '{}' does not look like an rclone `crypt` remote — snapshots may be \
             uploaded UNENCRYPTED. Configure a crypt remote to encrypt at rest.",
            bcfg.remote
        );
    }

    let _lock = BackupLock::acquire(&state_dir(root))?;
    let now = Utc::now();
    let name = snapshot_name(&hostname(), now);

    let staging = state_dir(root).join("backup-tmp");
    std::fs::create_dir_all(&staging)?;
    let archive = staging.join(&name);
    let sidecar = staging.join(sidecar_name(&name));

    let cfg_dir = if bcfg.include_config { Some(config_dir()?) } else { None };
    let manifest = create_snapshot(root, cfg_dir.as_deref(), &archive, now)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let manifest_json = serde_json::to_vec_pretty(&manifest)?;
    std::fs::write(&sidecar, &manifest_json)?;
    let size = std::fs::metadata(&archive)?.len();

    if dry_run {
        println!(
            "[dry-run] would upload {name} ({}) + sidecar to {}",
            human_size(size),
            bcfg.remote
        );
    } else {
        rclone
            .copyto(&archive, &remote_join(&bcfg.remote, &name))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        rclone
            .copyto(&sidecar, &remote_join(&bcfg.remote, &sidecar_name(&name)))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        println!(
            "backed up {} task(s), {} event(s) → {} ({})",
            manifest.task_count,
            manifest.event_count,
            name,
            human_size(size)
        );
    }

    // Clean local staging regardless.
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_file(&sidecar);

    prune(rclone, bcfg, dry_run)
}

fn list(rclone: &Rclone, bcfg: &BackupCfg) -> Result<()> {
    let snaps = list_snapshots(rclone, &bcfg.remote)?;
    if snaps.is_empty() {
        println!("no snapshots on {}", bcfg.remote);
        return Ok(());
    }
    let now = Utc::now();
    println!("{:<40} {:>10}  {}", "NAME", "SIZE", "AGE");
    for s in &snaps {
        println!("{:<40} {:>10}  {}", s.name, human_size(s.size), human_age(now - s.created_at));
    }
    Ok(())
}

fn prune(rclone: &Rclone, bcfg: &BackupCfg, dry_run: bool) -> Result<()> {
    let snaps = list_snapshots(rclone, &bcfg.remote)?;
    let doomed = plan_prune(&snaps, &policy(bcfg), Utc::now());
    if doomed.is_empty() {
        return Ok(());
    }
    for name in &doomed {
        if dry_run {
            println!("[dry-run] would prune {name}");
            continue;
        }
        rclone
            .deletefile(&remote_join(&bcfg.remote, name))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        // Best-effort: remove the paired sidecar too.
        let _ = rclone.deletefile(&remote_join(&bcfg.remote, &sidecar_name(name)));
        println!("pruned {name}");
    }
    Ok(())
}

fn verify(rclone: &Rclone, bcfg: &BackupCfg, name: Option<String>, deep: bool) -> Result<()> {
    let snaps = list_snapshots(rclone, &bcfg.remote)?;
    if snaps.is_empty() {
        println!("no snapshots on {}", bcfg.remote);
        return Ok(());
    }

    let targets: Vec<&SnapshotMeta> = match &name {
        Some(n) => vec![resolve(&snaps, n)?],
        None if deep => snaps.iter().collect(),
        None => {
            // Cheap check: every archive has a paired sidecar on the remote.
            let all = rclone.lsjson(&bcfg.remote).map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let present: std::collections::HashSet<String> = all.into_iter().map(|f| f.name).collect();
            let mut missing = 0;
            for s in &snaps {
                if !present.contains(&sidecar_name(&s.name)) {
                    println!("missing sidecar for {}", s.name);
                    missing += 1;
                }
            }
            println!("{} snapshot(s), {missing} missing sidecar(s)", snaps.len());
            return Ok(());
        }
    };

    let tmp = tempdir()?;
    for s in targets {
        let local = tmp.path().join(&s.name);
        rclone
            .fetch(&remote_join(&bcfg.remote, &s.name), &local)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        match verify_archive(&local) {
            Ok(m) => println!("ok  {} ({} files)", s.name, m.files.len()),
            Err(e) => println!("BAD {} — {e}", s.name),
        }
        let _ = std::fs::remove_file(&local);
    }
    Ok(())
}

/// Resolve a snapshot by exact name, `latest`, or unique prefix.
fn resolve<'a>(snaps: &'a [SnapshotMeta], name: &str) -> Result<&'a SnapshotMeta> {
    if name == "latest" {
        return snaps.first().context("no snapshots on the remote");
    }
    if let Some(exact) = snaps.iter().find(|s| s.name == name) {
        return Ok(exact);
    }
    let matches: Vec<&SnapshotMeta> = snaps.iter().filter(|s| s.name.starts_with(name)).collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!("no snapshot matching '{name}'"),
        _ => bail!("'{name}' is ambiguous ({} snapshots match)", matches.len()),
    }
}

pub fn run_restore(
    root: &Path,
    cfg: &Config,
    name: String,
    yes: bool,
    to: Option<PathBuf>,
) -> Result<()> {
    let bcfg = backup_cfg(cfg)?;
    let rclone = Rclone::new(&bcfg.rclone_binary);
    let snaps = list_snapshots(&rclone, &bcfg.remote)?;
    let target = resolve(&snaps, &name)?.name.clone();

    let tmp = tempdir()?;
    let local = tmp.path().join(&target);
    println!("downloading {target}…");
    rclone
        .fetch(&remote_join(&bcfg.remote, &target), &local)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // Inspection restore: extract elsewhere, never touch the live vault.
    if let Some(dest) = to {
        extract_all(&local, &dest).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        println!("extracted {target} to {}", dest.display());
        return Ok(());
    }

    // In-place restore: gate hard.
    let lock = state_dir(root).join("backup.lock");
    if lock.exists() {
        bail!(
            "a backup appears to be running (lock at {}) — wait for it or remove the stale lock",
            lock.display()
        );
    }
    if !yes {
        bail!(
            "refusing to overwrite {} without --yes (the current tree will be kept as \
             <data_root>.pre-restore-<ts>)",
            root.display()
        );
    }

    let outcome = restore_swap(&local, root, Utc::now()).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    println!(
        "restored {} file(s) from {target}. Previous tree kept at {} (delete it once satisfied).",
        outcome.files_restored,
        outcome.pre_restore.display()
    );
    Ok(())
}

/// A private staging directory, actually removed on drop. `tempfile` gives it an unpredictable
/// name and 0700 perms — a fixed `/tmp/mgmt-restore-<pid>` path could be pre-created by another
/// local user, and the downloaded vault archive would otherwise be left behind.
fn tempdir() -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new().prefix("mgmt-restore-").tempdir()?)
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

fn human_age(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// Exclusive backup lock at `<state_dir>/backup.lock`, released on drop. A stale lock (owning PID
/// no longer alive) is reclaimed.
struct BackupLock {
    path: PathBuf,
}

impl BackupLock {
    fn acquire(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir)?;
        let path = state_dir.join("backup.lock");
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let _ = write!(f, "{}", std::process::id());
                Ok(BackupLock { path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&path) {
                    std::fs::remove_file(&path)?;
                    return BackupLock::acquire(state_dir);
                }
                bail!("another backup is running (lock at {})", path.display())
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for BackupLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A lock is stale if its PID is unreadable or the process is no longer alive (Linux `/proc`).
fn lock_is_stale(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else { return true };
    let Ok(pid) = text.trim().parse::<u32>() else { return true };
    !Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_name_pairs_with_archive() {
        assert_eq!(sidecar_name("mgmt-host-20260703T120000Z.tar.zst"), "mgmt-host-20260703T120000Z.manifest.json");
    }

    #[test]
    fn parses_snapshot_timestamp() {
        let t = parse_snapshot_time("mgmt-my-host-20260703T120000Z.tar.zst").unwrap();
        assert_eq!(t.format("%Y-%m-%dT%H:%M:%SZ").to_string(), "2026-07-03T12:00:00Z");
        assert!(parse_snapshot_time("not-a-snapshot.txt").is_none());
    }

    #[test]
    fn remote_join_trims_slashes() {
        assert_eq!(remote_join("crypt:mgmt/", "x.tar.zst"), "crypt:mgmt/x.tar.zst");
        assert_eq!(remote_join("crypt:mgmt", "x.tar.zst"), "crypt:mgmt/x.tar.zst");
    }

    #[test]
    fn resolve_handles_latest_exact_and_prefix() {
        let n = Utc::now();
        let snaps = vec![
            SnapshotMeta { name: "mgmt-h-20260703T120000Z.tar.zst".into(), created_at: n, size: 1 },
            SnapshotMeta { name: "mgmt-h-20260702T120000Z.tar.zst".into(), created_at: n - chrono::Duration::days(1), size: 1 },
        ];
        assert_eq!(resolve(&snaps, "latest").unwrap().name, snaps[0].name);
        assert_eq!(resolve(&snaps, &snaps[1].name).unwrap().name, snaps[1].name);
        assert_eq!(resolve(&snaps, "mgmt-h-20260702").unwrap().name, snaps[1].name);
        assert!(resolve(&snaps, "mgmt-h-2026").is_err()); // ambiguous
        assert!(resolve(&snaps, "nope").is_err());
    }
}
