//! Collections — named groups of events or tasks, optionally backed by a remote CalDAV
//! collection for sync or by a read-only ICS/webcal subscription.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionKind {
    /// Holds `VEVENT` items (appointments, meetings).
    Events,
    /// Holds `VTODO` items (tasks).
    Tasks,
}

/// Where a collection's contents come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RemoteSource {
    /// Two-way CalDAV mirror.
    CalDav {
        /// Collection URL on the CalDAV server.
        url: String,
        /// Name of the account credentials block in config (resolved at sync time).
        account: String,
        /// Last-seen collection ctag, for cheap change detection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ctag: Option<String>,
        /// Last sync-token (RFC 6578) for incremental sync.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sync_token: Option<String>,
    },
    /// Read-only subscription to an ICS/webcal URL, refetched wholesale by the daemon.
    Ics {
        /// Normalised (http/https) feed URL — see [`normalize_feed_url`].
        url: String,
        #[serde(default = "default_refresh_minutes")]
        refresh_minutes: u32,
    },
}

fn default_refresh_minutes() -> u32 {
    60
}

/// Normalise a subscription URL: `webcal://`/`webcals://` are ICS feeds fetched over https.
/// Returns `None` for anything that is not http(s) afterwards — the fetcher must never be
/// pointed at `file://` or another scheme by a pasted URL — and for hosts that only exist on
/// the server itself (the daemon refetches on a timer, so a feed URL is an SSRF vector).
pub fn normalize_feed_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (scheme, rest) = raw.split_once("://")?;
    if rest.is_empty() || rest.starts_with('/') {
        return None; // no host
    }
    if is_local_host(rest) {
        return None;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "webcal" | "webcals" => Some(format!("https://{rest}")),
        "http" | "https" => Some(format!("{}://{rest}", scheme.to_ascii_lowercase())),
        _ => None,
    }
}

/// Loopback, link-local (cloud metadata) or unqualified hosts, taken from the part of a URL
/// after `://`. ponytail: a textual check, not a DNS resolution — a hostname that *resolves* to
/// 127.0.0.1 gets through, and private ranges (10/8, 172.16/12, 192.168/16) are not rejected at
/// all; the endpoint is admin-only, so that is the accepted ceiling. Upgrade path: resolve the
/// host and filter the socket addrs (loopback + private + link-local).
fn is_local_host(rest: &str) -> bool {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(), // [::1]:8080
        None => host.split(':').next().unwrap_or_default(),
    }
    .to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "metadata.google.internal"
        || host.starts_with("127.")
        || host.starts_with("0.")
        || host.starts_with("169.254.")
        || host == "::1"
        || host.starts_with("fe80:")
        || !host.contains('.') && !host.contains(':') // unqualified, resolves via local search
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
    /// Secret for the unauthenticated `GET /api/feed/<token>.ics` endpoint. `None` = no feed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed_token: Option<String>,
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
            feed_token: None,
        }
    }

    pub fn is_synced(&self) -> bool {
        self.remote.is_some()
    }

    /// `(url, refresh_minutes)` when this collection is a read-only ICS subscription.
    pub fn subscription(&self) -> Option<(&str, u32)> {
        match &self.remote {
            Some(RemoteSource::Ics { url, refresh_minutes }) => Some((url, *refresh_minutes)),
            _ => None,
        }
    }

    /// Subscribed collections are mirrors: local edits would be wiped by the next refresh.
    pub fn is_read_only(&self) -> bool {
        self.subscription().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcal_normalises_to_https_and_other_schemes_are_rejected() {
        assert_eq!(normalize_feed_url("webcal://ex.org/h.ics").unwrap(), "https://ex.org/h.ics");
        assert_eq!(normalize_feed_url(" WEBCALS://ex.org/h.ics ").unwrap(), "https://ex.org/h.ics");
        assert_eq!(normalize_feed_url("http://ex.org/h.ics").unwrap(), "http://ex.org/h.ics");
        for bad in ["file:///etc/passwd", "ftp://ex.org/h.ics", "/h.ics", "https://", "https:///h.ics"] {
            assert!(normalize_feed_url(bad).is_none(), "{bad} must be rejected");
        }
    }

    /// The daemon refetches feeds on a timer, so a feed URL must not reach the host's own network.
    #[test]
    fn loopback_and_link_local_feeds_are_rejected() {
        let local_hosts = [
            "127.0.0.1:8080",
            "localhost",
            "LOCALHOST:5000",
            "[::1]:8080",
            "169.254.169.254",
            "metadata.google.internal",
            "0.0.0.0",
            "intranet",
            "user@127.0.0.1",
        ];
        for host in local_hosts {
            let url = format!("http://{host}/h.ics");
            assert!(normalize_feed_url(&url).is_none(), "{url} must be rejected");
        }
        assert!(normalize_feed_url("https://ex.org:8443/h.ics").is_some());
    }

    #[test]
    fn subscription_round_trips_and_is_read_only() {
        let mut c = Collection::local("holidays", CollectionKind::Events);
        c.remote = Some(RemoteSource::Ics { url: "https://ex.org/h.ics".into(), refresh_minutes: 30 });
        c.feed_token = Some("tok".into());
        let yaml = serde_yaml::to_string(&c).unwrap();
        let back: Collection = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.subscription(), Some(("https://ex.org/h.ics", 30)));
        assert!(back.is_read_only());
        assert!(!Collection::local("work", CollectionKind::Events).is_read_only());
    }
}
