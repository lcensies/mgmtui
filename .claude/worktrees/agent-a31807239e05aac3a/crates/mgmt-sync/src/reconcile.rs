//! Two-way reconciliation planning. Pure logic: given the local items (each possibly
//! carrying the `href`/`etag` it last synced with) and the current remote listing, decide
//! what to push, pull, or delete. Remote wins on etag conflicts — the same default
//! calcurse's caldav helper uses.

use mgmt_core::Uid;

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
