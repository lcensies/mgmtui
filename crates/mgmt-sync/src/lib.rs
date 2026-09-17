//! Sync engine for mgmt: two-way reconcile against CalDAV, plus management of the bundled
//! rustical server and pre/post-sync hooks.

mod engine;
mod hooks;
mod http;
mod ics;
mod pairing;
mod reconcile;
mod rustical;

pub use engine::{SyncReport, sync_events, sync_tasks};
pub use hooks::run_hook;
pub use http::{sync_events_http, sync_tasks_http, HttpRemote};
pub use ics::{fetch_ics, refresh_all, refresh_subscription, replace_collection};
pub use pairing::{run_pairing, Pairing, Pairings};
pub use reconcile::{
    plan_sync, plan_sync3, BaseRef, LocalItem, LocalRef, RemoteRef, SyncOp, SyncOp3,
};
pub use rustical::RusticalConfig;

// Re-export the client surface so callers depend on one crate for sync.
pub use mgmt_dav::{Auth, CalDavClient};
