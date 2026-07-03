//! A small persistent [`SessionStore`] backed by a JSON file, so a server restart doesn't log the
//! phone out. tower-sessions ships an in-memory store and DB-backed stores; a local-first app wants
//! neither a lost-on-restart cache nor a database, so we implement the (tiny) store trait against a
//! single JSON file guarded by a mutex.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{Error, Result};
use tower_sessions::SessionStore;

#[derive(Clone, Debug)]
pub struct FileSessionStore {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    path: PathBuf,
    records: Mutex<HashMap<Id, Record>>,
}

impl FileSessionStore {
    /// Open (or create) the store at `path`, hydrating any persisted, non-expired sessions.
    pub fn new(path: PathBuf) -> Self {
        let records = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<Record>>(&t).ok())
            .map(|v| v.into_iter().map(|r| (r.id, r)).collect())
            .unwrap_or_default();
        FileSessionStore { inner: Arc::new(Inner { path, records: Mutex::new(records) }) }
    }

    /// Persist the current record set to disk (best-effort atomic via a temp file + rename).
    fn persist(&self, records: &HashMap<Id, Record>) -> Result<()> {
        if let Some(parent) = self.inner.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let list: Vec<&Record> = records.values().collect();
        let json = serde_json::to_string(&list).map_err(|e| Error::Encode(e.to_string()))?;
        let tmp = self.inner.path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| Error::Backend(e.to_string()))?;
        std::fs::rename(&tmp, &self.inner.path).map_err(|e| Error::Backend(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn save(&self, record: &Record) -> Result<()> {
        let mut records = self.inner.records.lock().unwrap();
        records.insert(record.id, record.clone());
        self.persist(&records)
    }

    async fn load(&self, session_id: &Id) -> Result<Option<Record>> {
        let mut records = self.inner.records.lock().unwrap();
        match records.get(session_id) {
            Some(r) if r.expiry_date > OffsetDateTime::now_utc() => Ok(Some(r.clone())),
            Some(_) => {
                // Expired — drop it so the file doesn't accrete dead sessions.
                records.remove(session_id);
                let _ = self.persist(&records);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, session_id: &Id) -> Result<()> {
        let mut records = self.inner.records.lock().unwrap();
        records.remove(session_id);
        self.persist(&records)
    }
}
