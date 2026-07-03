//! Reconciliation planning. Pure logic with two flavours:
//!
//! * [`plan_sync`] — the original two-way, remote-wins planner (still used by the CalDAV path).
//! * [`plan_sync3`] — a true three-way planner used by the native `mgmt` sync. Given a persisted
//!   *base* snapshot (the state at the last sync), the current local items, and the current remote
//!   listing, it detects edits and deletes on *either* side and only flags a real `Conflict` when
//!   both changed. This is what makes the native poll bidirectional: a single poller pushes its own
//!   changes and pulls the remote's in one pass.

use mgmt_core::Uid;
use serde::{Deserialize, Serialize};

/// A local item's sync identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRef {
    pub uid: Uid,
    pub href: Option<String>,
    pub etag: Option<String>,
}

/// A resource as seen on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    pub href: String,
    pub etag: Option<String>,
}

/// One unit of sync work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp {
    /// Upload a local item that has no server copy yet.
    Push(Uid),
    /// Download a remote resource (new remotely, or changed since we last saw it).
    Pull(String),
    /// Remove a local item whose server copy has disappeared.
    DeleteLocal(Uid),
}

/// Compute the set of operations to bring local and remote into agreement.
pub fn plan_sync(local: &[LocalRef], remote: &[RemoteRef]) -> Vec<SyncOp> {
    let mut ops = Vec::new();

    let remote_by_href = |href: &str| remote.iter().find(|r| r.href == href);
    let local_hrefs: Vec<&str> = local.iter().filter_map(|l| l.href.as_deref()).collect();

    for l in local {
        match &l.href {
            None => ops.push(SyncOp::Push(l.uid.clone())),
            Some(href) => match remote_by_href(href) {
                None => ops.push(SyncOp::DeleteLocal(l.uid.clone())),
                Some(r) => {
                    if r.etag != l.etag {
                        ops.push(SyncOp::Pull(href.clone()));
                    }
                }
            },
        }
    }

    for r in remote {
        if !local_hrefs.contains(&r.href.as_str()) {
            ops.push(SyncOp::Pull(r.href.clone()));
        }
    }

    ops
}

// ---- three-way (native bidirectional) planning ----------------------------------------------

/// The last-synced state of one item, persisted between passes as the merge *base*. Serialized to
/// `<user_root>/.state/sync/<pairing>/<collection>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseRef {
    pub uid: Uid,
    pub href: String,
    /// The remote ETag observed at the last successful sync.
    #[serde(default)]
    pub remote_etag: Option<String>,
    /// Our own hash of the clean-serialized local body at the last successful sync.
    pub local_hash: String,
}

/// Current local state of one item for the three-way planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalItem {
    pub uid: Uid,
    pub href: Option<String>,
    /// Last-synced remote ETag carried on the item (`SyncMeta.etag`); used as a fallback base the
    /// first time a pre-existing (two-way-synced) vault is reconciled three-way.
    pub etag: Option<String>,
    /// Hash of the item's *current* clean-serialized body.
    pub hash: String,
}

/// One unit of bidirectional sync work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp3 {
    /// Upload a local item that has no server copy (new locally, or re-create after a remote delete
    /// we chose not to honor because the item was edited locally).
    PushCreate(Uid),
    /// Upload a locally-edited item over its existing server copy (PUT with `If-Match`).
    PushUpdate(Uid, String),
    /// Download a remote resource (new remotely, or changed since the base).
    Pull(String),
    /// Remove a local item whose server copy was deleted (and which we hadn't edited).
    DeleteLocal(Uid),
    /// Remove a server resource that was deleted locally (and which the remote hadn't edited).
    DeleteRemote(String),
    /// Both sides changed since the base — resolve by comparing `modified` timestamps in the driver.
    Conflict(Uid, String),
}

/// Compute the bidirectional operations to converge local and remote, given the last-synced `base`.
///
/// `remote_changed` compares the current remote ETag against the base's; `local_changed` compares
/// the current local body hash against the base's. When there is no base entry for an item yet
/// (e.g. the first three-way pass over a vault previously synced two-way), we fall back to the
/// item's carried `etag` for the remote comparison and assume the local side is unchanged — so the
/// first pass behaves like the old remote-wins two-way sync rather than flagging everything.
pub fn plan_sync3(base: &[BaseRef], local: &[LocalItem], remote: &[RemoteRef]) -> Vec<SyncOp3> {
    let mut ops = Vec::new();
    let base_by_href = |h: &str| base.iter().find(|b| b.href == h);
    let remote_by_href = |h: &str| remote.iter().find(|r| r.href == h);
    let local_hrefs: Vec<&str> = local.iter().filter_map(|l| l.href.as_deref()).collect();

    for l in local {
        let Some(href) = l.href.as_deref() else {
            ops.push(SyncOp3::PushCreate(l.uid.clone())); // never synced
            continue;
        };
        let based = base_by_href(href);
        match remote_by_href(href) {
            None => {
                // Remote copy is gone. Honor the delete unless we edited it locally since the base
                // (then keep the local edit by re-creating it on the server).
                let local_changed = based.map(|b| b.local_hash != l.hash).unwrap_or(false);
                if local_changed {
                    ops.push(SyncOp3::PushCreate(l.uid.clone()));
                } else {
                    ops.push(SyncOp3::DeleteLocal(l.uid.clone()));
                }
            }
            Some(r) => {
                let (base_etag, base_hash) = match based {
                    Some(b) => (b.remote_etag.as_deref(), Some(b.local_hash.as_str())),
                    None => (l.etag.as_deref(), None),
                };
                let remote_changed = base_etag != r.etag.as_deref();
                let local_changed = base_hash.map(|h| h != l.hash).unwrap_or(false);
                match (local_changed, remote_changed) {
                    (false, false) => {}
                    (true, false) => ops.push(SyncOp3::PushUpdate(l.uid.clone(), href.to_string())),
                    (false, true) => ops.push(SyncOp3::Pull(href.to_string())),
                    (true, true) => ops.push(SyncOp3::Conflict(l.uid.clone(), href.to_string())),
                }
            }
        }
    }

    for r in remote {
        if local_hrefs.contains(&r.href.as_str()) {
            continue;
        }
        match base_by_href(&r.href) {
            // We synced it before and it's gone locally → propagate our delete, unless the remote
            // also changed it since the base (then keep the remote edit).
            Some(b) => {
                if b.remote_etag.as_deref() != r.etag.as_deref() {
                    ops.push(SyncOp3::Pull(r.href.clone()));
                } else {
                    ops.push(SyncOp3::DeleteRemote(r.href.clone()));
                }
            }
            // Brand new on the remote.
            None => ops.push(SyncOp3::Pull(r.href.clone())),
        }
    }

    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(uid: &str, href: Option<&str>, etag: Option<&str>) -> LocalRef {
        LocalRef {
            uid: Uid::from_string(uid),
            href: href.map(String::from),
            etag: etag.map(String::from),
        }
    }

    fn remote(href: &str, etag: Option<&str>) -> RemoteRef {
        RemoteRef {
            href: href.to_string(),
            etag: etag.map(String::from),
        }
    }

    #[test]
    fn unsynced_local_is_pushed() {
        let ops = plan_sync(&[local("a", None, None)], &[]);
        assert_eq!(ops, vec![SyncOp::Push(Uid::from_string("a"))]);
    }

    #[test]
    fn new_remote_is_pulled() {
        let ops = plan_sync(&[], &[remote("/x.ics", Some("e1"))]);
        assert_eq!(ops, vec![SyncOp::Pull("/x.ics".into())]);
    }

    #[test]
    fn unchanged_etag_is_a_noop() {
        let ops = plan_sync(
            &[local("a", Some("/a.ics"), Some("e1"))],
            &[remote("/a.ics", Some("e1"))],
        );
        assert!(ops.is_empty());
    }

    #[test]
    fn changed_etag_pulls_remote() {
        let ops = plan_sync(
            &[local("a", Some("/a.ics"), Some("e1"))],
            &[remote("/a.ics", Some("e2"))],
        );
        assert_eq!(ops, vec![SyncOp::Pull("/a.ics".into())]);
    }

    #[test]
    fn vanished_remote_deletes_local() {
        let ops = plan_sync(&[local("a", Some("/a.ics"), Some("e1"))], &[]);
        assert_eq!(ops, vec![SyncOp::DeleteLocal(Uid::from_string("a"))]);
    }

    #[test]
    fn mixed_scenario() {
        let local = vec![
            local("keep", Some("/keep.ics"), Some("e1")),
            local("changed", Some("/changed.ics"), Some("old")),
            local("gone", Some("/gone.ics"), Some("e1")),
            local("new-local", None, None),
        ];
        let remote = vec![
            remote("/keep.ics", Some("e1")),
            remote("/changed.ics", Some("new")),
            remote("/new-remote.ics", Some("e9")),
        ];
        let ops = plan_sync(&local, &remote);
        assert!(ops.contains(&SyncOp::Pull("/changed.ics".into())));
        assert!(ops.contains(&SyncOp::DeleteLocal(Uid::from_string("gone"))));
        assert!(ops.contains(&SyncOp::Push(Uid::from_string("new-local"))));
        assert!(ops.contains(&SyncOp::Pull("/new-remote.ics".into())));
        assert!(!ops.iter().any(|op| matches!(op, SyncOp::Pull(h) if h == "/keep.ics")));
    }
}

#[cfg(test)]
mod three_way_tests {
    use super::*;

    fn base(uid: &str, href: &str, etag: &str, hash: &str) -> BaseRef {
        BaseRef {
            uid: Uid::from_string(uid),
            href: href.into(),
            remote_etag: Some(etag.into()),
            local_hash: hash.into(),
        }
    }
    fn litem(uid: &str, href: Option<&str>, etag: Option<&str>, hash: &str) -> LocalItem {
        LocalItem {
            uid: Uid::from_string(uid),
            href: href.map(String::from),
            etag: etag.map(String::from),
            hash: hash.into(),
        }
    }
    fn rref(href: &str, etag: &str) -> RemoteRef {
        RemoteRef { href: href.into(), etag: Some(etag.into()) }
    }

    #[test]
    fn unchanged_both_sides_is_a_noop() {
        let b = vec![base("a", "/a.md", "e1", "h1")];
        let l = vec![litem("a", Some("/a.md"), Some("e1"), "h1")];
        let r = vec![rref("/a.md", "e1")];
        assert!(plan_sync3(&b, &l, &r).is_empty());
    }

    #[test]
    fn new_local_is_created_new_remote_is_pulled() {
        let ops = plan_sync3(&[], &[litem("a", None, None, "h1")], &[rref("/b.md", "e1")]);
        assert!(ops.contains(&SyncOp3::PushCreate(Uid::from_string("a"))));
        assert!(ops.contains(&SyncOp3::Pull("/b.md".into())));
    }

    #[test]
    fn local_only_edit_pushes_update() {
        let b = vec![base("a", "/a.md", "e1", "h1")];
        let l = vec![litem("a", Some("/a.md"), Some("e1"), "h2")]; // hash changed locally
        let r = vec![rref("/a.md", "e1")]; // remote etag unchanged
        assert_eq!(plan_sync3(&b, &l, &r), vec![SyncOp3::PushUpdate(Uid::from_string("a"), "/a.md".into())]);
    }

    #[test]
    fn remote_only_edit_pulls() {
        let b = vec![base("a", "/a.md", "e1", "h1")];
        let l = vec![litem("a", Some("/a.md"), Some("e1"), "h1")]; // local unchanged
        let r = vec![rref("/a.md", "e2")]; // remote etag changed
        assert_eq!(plan_sync3(&b, &l, &r), vec![SyncOp3::Pull("/a.md".into())]);
    }

    #[test]
    fn both_edited_is_a_conflict() {
        let b = vec![base("a", "/a.md", "e1", "h1")];
        let l = vec![litem("a", Some("/a.md"), Some("e1"), "h2")];
        let r = vec![rref("/a.md", "e2")];
        assert_eq!(plan_sync3(&b, &l, &r), vec![SyncOp3::Conflict(Uid::from_string("a"), "/a.md".into())]);
    }

    #[test]
    fn local_delete_propagates_to_remote() {
        // In base + remote, absent from local, remote unchanged → delete on the server.
        let b = vec![base("a", "/a.md", "e1", "h1")];
        let r = vec![rref("/a.md", "e1")];
        assert_eq!(plan_sync3(&b, &[], &r), vec![SyncOp3::DeleteRemote("/a.md".into())]);
    }

    #[test]
    fn remote_delete_propagates_to_local() {
        // In base + local (unedited), absent from remote → delete locally.
        let b = vec![base("a", "/a.md", "e1", "h1")];
        let l = vec![litem("a", Some("/a.md"), Some("e1"), "h1")];
        assert_eq!(plan_sync3(&b, &l, &[]), vec![SyncOp3::DeleteLocal(Uid::from_string("a"))]);
    }

    #[test]
    fn local_edit_beats_remote_delete_by_recreating() {
        let b = vec![base("a", "/a.md", "e1", "h1")];
        let l = vec![litem("a", Some("/a.md"), Some("e1"), "h2")]; // edited locally
        assert_eq!(plan_sync3(&b, &l, &[]), vec![SyncOp3::PushCreate(Uid::from_string("a"))]);
    }

    #[test]
    fn remote_edit_beats_local_delete_by_pulling() {
        let b = vec![base("a", "/a.md", "e1", "h1")];
        let r = vec![rref("/a.md", "e2")]; // remote changed after our local delete
        assert_eq!(plan_sync3(&b, &[], &r), vec![SyncOp3::Pull("/a.md".into())]);
    }

    #[test]
    fn first_pass_without_base_falls_back_to_remote_wins() {
        // A vault previously synced two-way: items carry href+etag but there is no base snapshot.
        // Remote etag differs → pull (like the old behaviour), not a spurious conflict.
        let l = vec![litem("a", Some("/a.md"), Some("e1"), "h1")];
        let r = vec![rref("/a.md", "e2")];
        assert_eq!(plan_sync3(&[], &l, &r), vec![SyncOp3::Pull("/a.md".into())]);
    }
}
