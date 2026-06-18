//! Vault store: tasks as `<uid>.md` files under a directory tree.

use std::path::{Path, PathBuf};

use mgmt_core::{Result, Store, Uid};
use mgmt_domain::Task;

use crate::paths;

pub struct VaultStore {
    root: PathBuf,
}

impl VaultStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        VaultStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, uid: &Uid) -> PathBuf {
        self.root.join(format!("{}.md", paths::safe_stem(uid.as_str())))
    }
}

impl Store<Task> for VaultStore {
    fn load_all(&self) -> Result<Vec<Task>> {
        let mut tasks = Vec::new();
        for file in paths::collect_files(&self.root, "md")? {
            let text = std::fs::read_to_string(&file)?;
            // Skip non-task markdown (no frontmatter) rather than failing the whole load.
            if let Ok(task) = mgmt_markdown::parse_task(&text) {
                tasks.push(task);
            }
        }
        Ok(tasks)
    }

    fn upsert(&mut self, item: Task) -> Result<Task> {
        let text = mgmt_markdown::serialize_task(&item)?;
        paths::atomic_write(&self.path_for(&item.uid), &text)?;
        Ok(item)
    }

    fn delete(&mut self, id: &Uid) -> Result<bool> {
        let path = self.path_for(id);
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mgmt_domain::TaskStatus;

    #[test]
    fn upsert_load_delete_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = VaultStore::new(dir.path());

        let mut t = Task::new("Write tests");
        t.uid = Uid::from_string("t1");
        t.status = TaskStatus::Doing;
        store.upsert(t.clone()).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "Write tests");
        assert_eq!(loaded[0].status, TaskStatus::Doing);

        assert!(store.delete(&t.uid).unwrap());
        assert!(store.load_all().unwrap().is_empty());
        assert!(!store.delete(&t.uid).unwrap());
    }

    #[test]
    fn upsert_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = VaultStore::new(dir.path());
        let mut t = Task::new("v1");
        t.uid = Uid::from_string("same");
        store.upsert(t.clone()).unwrap();
        t.title = "v2".into();
        store.upsert(t.clone()).unwrap();
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "v2");
    }
}
