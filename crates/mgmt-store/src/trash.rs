//! Trash store: soft-deleted tasks and projects kept under `<root>/.trash/` so an accidental
//! delete can be restored. Items are stored exactly as the live stores serialize them (task
//! `<uid>.md`, project `<name>.md`), so restoring is just moving the markdown back.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mgmt_core::{Result, Uid};
use mgmt_domain::{Project, Task};

use crate::paths;

pub struct TrashStore {
    dir: PathBuf,
}

impl TrashStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        TrashStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn tasks_dir(&self) -> PathBuf {
        self.dir.join("tasks")
    }

    fn projects_dir(&self) -> PathBuf {
        self.dir.join("projects")
    }

    fn task_path(&self, uid: &Uid) -> PathBuf {
        self.tasks_dir().join(format!("{}.md", paths::safe_stem(uid.as_str())))
    }

    fn project_path(&self, name: &str) -> PathBuf {
        self.projects_dir().join(format!("{}.md", paths::safe_stem(name)))
    }

    /// Move a task into the trash (idempotent: overwrites any prior copy of the same uid).
    pub fn trash_task(&self, task: &Task) -> Result<()> {
        let text = mgmt_markdown::serialize_task(task)?;
        paths::atomic_write(&self.task_path(&task.uid), &text)
    }

    /// Move a project into the trash (idempotent).
    pub fn trash_project(&self, project: &Project) -> Result<()> {
        let text = mgmt_markdown::serialize_project(project)?;
        paths::atomic_write(&self.project_path(&project.name), &text)
    }

    /// Read a trashed task without removing it.
    pub fn read_task(&self, uid: &Uid) -> Result<Option<Task>> {
        read_parsed(&self.task_path(uid), |text| mgmt_markdown::parse_task(text).ok())
    }

    /// Read a trashed project without removing it.
    pub fn read_project(&self, name: &str) -> Result<Option<Project>> {
        read_parsed(&self.project_path(name), |text| mgmt_markdown::parse_project(text).ok())
    }

    /// Permanently remove a trashed task. Returns whether a file was removed.
    pub fn purge_task(&self, uid: &Uid) -> Result<bool> {
        remove_if_exists(&self.task_path(uid))
    }

    /// Permanently remove a trashed project. Returns whether a file was removed.
    pub fn purge_project(&self, name: &str) -> Result<bool> {
        remove_if_exists(&self.project_path(name))
    }

    /// All trashed tasks, most-recently-trashed first.
    pub fn load_tasks(&self) -> Result<Vec<Task>> {
        load_sorted(&self.tasks_dir(), |text| mgmt_markdown::parse_task(text).ok())
    }

    /// All trashed projects, most-recently-trashed first.
    pub fn load_projects(&self) -> Result<Vec<Project>> {
        load_sorted(&self.projects_dir(), |text| mgmt_markdown::parse_project(text).ok())
    }

    /// Whether the trash holds nothing.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(paths::collect_files(&self.tasks_dir(), "md")?.is_empty()
            && paths::collect_files(&self.projects_dir(), "md")?.is_empty())
    }

    /// Empty the entire trash.
    pub fn empty(&self) -> Result<()> {
        for dir in [self.tasks_dir(), self.projects_dir()] {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
        }
        Ok(())
    }
}

fn remove_if_exists(path: &Path) -> Result<bool> {
    if path.exists() {
        std::fs::remove_file(path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn read_parsed<T>(path: &Path, parse: impl Fn(&str) -> Option<T>) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    Ok(parse(&text))
}

/// Load and parse every `.md` file under `dir`, newest-trashed first (by mtime).
fn load_sorted<T>(dir: &Path, parse: impl Fn(&str) -> Option<T>) -> Result<Vec<T>> {
    let mut entries: Vec<(SystemTime, T)> = Vec::new();
    for file in paths::collect_files(dir, "md")? {
        let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        let text = std::fs::read_to_string(&file)?;
        if let Some(item) = parse(&text) {
            entries.push((mtime, item));
        }
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(entries.into_iter().map(|(_, item)| item).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, TrashStore) {
        let dir = tempfile::tempdir().unwrap();
        let trash = TrashStore::new(dir.path().join(".trash"));
        (dir, trash)
    }

    #[test]
    fn task_round_trips_through_trash() {
        let (_d, trash) = store();
        let t = Task::new("buy milk");
        trash.trash_task(&t).unwrap();
        let loaded = trash.load_tasks().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "buy milk");
        let read = trash.read_task(&t.uid).unwrap().unwrap();
        assert_eq!(read.uid, t.uid);
        assert!(trash.purge_task(&t.uid).unwrap());
        assert!(trash.load_tasks().unwrap().is_empty());
    }

    #[test]
    fn project_round_trips_and_empty_clears_all() {
        let (_d, trash) = store();
        let t = Task::new("x");
        trash.trash_task(&t).unwrap();
        trash.trash_project(&Project::new("wng")).unwrap();
        assert!(!trash.is_empty().unwrap());
        assert_eq!(trash.load_projects().unwrap().len(), 1);
        trash.empty().unwrap();
        assert!(trash.is_empty().unwrap());
        assert!(trash.load_tasks().unwrap().is_empty());
    }
}
