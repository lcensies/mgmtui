//! Collections — named groups of events or tasks, optionally backed by a remote CalDAV
//! collection for sync.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionKind {
    /// Holds `VEVENT` items (appointments, meetings).
    Events,
    /// Holds `VTODO` items (tasks).
    Tasks,
}

/// A remote CalDAV collection this local collection mirrors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSource {
    /// Collection URL on the CalDAV server.
    pub url: String,
    /// Name of the account credentials block in config (resolved at sync time).
    pub account: String,
    /// Last-seen collection ctag, for cheap change detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctag: Option<String>,
    /// Last sync-token (RFC 6578) for incremental sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    /// Stable local id and on-disk directory name (e.g. "work").
    pub id: String,
    pub kind: CollectionKind,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteSource>,
}

impl Collection {
    pub fn local(id: impl Into<String>, kind: CollectionKind) -> Self {
        let id = id.into();
        Collection {
            display_name: id.clone(),
            id,
            kind,
            color: None,
            remote: None,
        }
    }

    pub fn is_synced(&self) -> bool {
        self.remote.is_some()
    }
}
