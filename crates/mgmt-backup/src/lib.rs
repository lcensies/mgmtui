//! Encrypted, provider-agnostic vault backups for mgmt.
//!
//! A backup is a single `tar.zst` snapshot of the data root (plus, optionally, the config) with
//! an embedded [`Manifest`] listing every file and its SHA-256. Snapshots are pushed to an
//! **rclone** remote — point that remote at an `rclone crypt` wrapper and encryption is handled
//! entirely by rclone, so mgmt never stores a passphrase and stays provider-agnostic.
//!
//! The crate deliberately depends only on `mgmt-core` (+ tar/zstd/sha2/serde/chrono): it shells
//! out to `rclone` the same way `mgmt-sync` shells out to `rustical`, and it never pulls in the
//! CalDAV/tokio stack.
//!
//! Layout: [`snapshot`] builds/verifies archives, [`retention`] is the pure prune planner,
//! [`rclone`] wraps the external binary, and [`restore`] extracts archives safely (staging +
//! atomic rename, never in place).

mod rclone;
mod restore;
mod retention;
mod snapshot;

pub use rclone::{RemoteFile, Rclone};
pub use restore::{extract_all, restore_swap, RestoreOutcome};
pub use retention::{plan_prune, RetentionPolicy, SnapshotMeta};
pub use snapshot::{
    create_snapshot, hostname, snapshot_name, verify_archive, FileEntry, Manifest, FORMAT_VERSION,
    MANIFEST_NAME,
};
