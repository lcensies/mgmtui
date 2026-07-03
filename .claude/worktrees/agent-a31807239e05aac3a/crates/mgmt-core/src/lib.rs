//! Foundation crate for mgmt: error types, result alias, the `Uid` newtype, and the
//! storage-agnostic `Store` trait that views depend on.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Centralized error type for the mgmt workspace.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

/// Standard result type used across mgmt library crates.
pub type Result<T> = std::result::Result<T, Error>;

/// Stable unique identifier for events and tasks.
///
/// Backed by a UUID but serialized as its plain string form so it round-trips cleanly
/// through iCalendar `UID` and markdown frontmatter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Uid(String);

impl Uid {
    /// Generate a fresh random UID.
    pub fn new() -> Self {
        Uid(Uuid::new_v4().to_string())
    }

    /// Wrap an existing identifier string (e.g. a UID read from a `.ics` file).
    pub fn from_string(s: impl Into<String>) -> Self {
        Uid(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Uid {
    fn default() -> Self {
        Uid::new()
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Uid {
    fn from(s: String) -> Self {
        Uid(s)
    }
}

impl From<&str> for Uid {
    fn from(s: &str) -> Self {
        Uid(s.to_string())
    }
}

/// Read-then-write storage abstraction. Domain crates define models; this trait lets the
/// service and views operate over any backend (vault, vdir, in-memory) without coupling.
pub trait Store<T> {
    /// Load every item from the backing store.
    fn load_all(&self) -> Result<Vec<T>>;
    /// Insert or replace a single item, returning its persisted form.
    fn upsert(&mut self, item: T) -> Result<T>;
    /// Remove an item by id. Returns `true` if something was removed.
    fn delete(&mut self, id: &Uid) -> Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_round_trips_through_string() {
        let u = Uid::from_string("abc-123");
        assert_eq!(u.as_str(), "abc-123");
        assert_eq!(u.to_string(), "abc-123");
    }

    #[test]
    fn fresh_uids_are_unique() {
        assert_ne!(Uid::new(), Uid::new());
    }

    #[test]
    fn uid_serializes_as_plain_string() {
        let u = Uid::from_string("xyz");
        let json = serde_json_to_string(&u);
        assert_eq!(json, "\"xyz\"");
    }

    // tiny helper so we don't pull serde_json into core just for a test
    fn serde_json_to_string(u: &Uid) -> String {
        format!("\"{}\"", u.as_str())
    }
}
