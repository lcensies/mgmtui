//! Vdir store: events as `<uid>.ics` files, one directory per collection.
//!
//! Layout: `<root>/<collection>/<uid>.ics`. The directory name is authoritative for an
//! event's `calendar` field, so moving a `.ics` between folders re-homes the event.

use std::path::{Path, PathBuf};

use mgmt_core::{Error, Result, Store, Uid};
use mgmt_domain::Event;

use crate::paths;

pub struct VdirStore {
    root: PathBuf,
}

impl VdirStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        VdirStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create a collection directory if it does not yet exist.
    pub fn ensure_collection(&self, name: &str) -> Result<()> {
        std::fs::create_dir_all(self.root.join(name))?;
        Ok(())
    }

    /// Names of the collections (subdirectories) currently on disk.
    pub fn collections(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    fn path_for(&self, ev: &Event) -> PathBuf {
        self.root
            .join(&ev.calendar)
            .join(format!("{}.ics", paths::safe_stem(ev.uid.as_str())))
    }

    /// Find the on-disk path of an event by uid, searching every collection. Used by the
    /// `$EDITOR` integration.
    pub fn find_path(&self, uid: &Uid) -> Result<Option<PathBuf>> {
        let stem = format!("{}.ics", paths::safe_stem(uid.as_str()));
        for collection in self.collections()? {
            let path = self.root.join(&collection).join(&stem);
            if path.exists() {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }
}

impl Store<Event> for VdirStore {
    fn load_all(&self) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        for collection in self.collections()? {
            let dir = self.root.join(&collection);
            for file in paths::collect_files(&dir, "ics")? {
                let text = std::fs::read_to_string(&file)?;
                if let Ok(ev) = mgmt_ical::event_from_ics(&text, &collection) {
                    events.push(ev);
                }
            }
        }
        Ok(events)
    }

    fn upsert(&mut self, item: Event) -> Result<Event> {
        if item.calendar.is_empty() {
            return Err(Error::Invalid("event has no calendar".into()));
        }
        let text = mgmt_ical::event_to_ics_local(&item);
        paths::atomic_write(&self.path_for(&item), &text)?;
        Ok(item)
    }

    fn delete(&mut self, id: &Uid) -> Result<bool> {
        // The UID alone does not tell us the collection, so search them all.
        let stem = format!("{}.ics", paths::safe_stem(id.as_str()));
        for collection in self.collections()? {
            let path = self.root.join(&collection).join(&stem);
            if path.exists() {
                std::fs::remove_file(&path)?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn sample(cal: &str, uid: &str) -> Event {
        let mut e = Event::new(
            cal,
            "Meeting",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 10, 0, 0).unwrap(),
        );
        e.uid = Uid::from_string(uid);
        e
    }

    #[test]
    fn events_round_trip_across_collections() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = VdirStore::new(dir.path());
        store.upsert(sample("work", "w1")).unwrap();
        store.upsert(sample("personal", "p1")).unwrap();

        let mut loaded = store.load_all().unwrap();
        loaded.sort_by(|a, b| a.uid.as_str().cmp(b.uid.as_str()));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].calendar, "personal");
        assert_eq!(loaded[1].calendar, "work");

        assert!(store.delete(&Uid::from_string("w1")).unwrap());
        assert_eq!(store.load_all().unwrap().len(), 1);
    }

    #[test]
    fn collections_lists_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let store = VdirStore::new(dir.path());
        store.ensure_collection("work").unwrap();
        store.ensure_collection("home").unwrap();
        assert_eq!(store.collections().unwrap(), vec!["home", "work"]);
    }
}
