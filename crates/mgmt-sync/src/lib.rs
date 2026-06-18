//! Sync engine for mgmt: two-way reconcile against CalDAV, plus management of the bundled
//! rustical server and pre/post-sync hooks.

mod engine;
mod hooks;
mod reconcile;
mod rustical;

pub use engine::{SyncReport, sync_events, sync_tasks};
pub use hooks::run_hook;
pub use reconcile::{LocalRef, RemoteRef, SyncOp, plan_sync};
pub use rustical::RusticalConfig;

// Re-export the client surface so callers depend on one crate for sync.
pub use mgmt_dav::{Auth, CalDavClient};
