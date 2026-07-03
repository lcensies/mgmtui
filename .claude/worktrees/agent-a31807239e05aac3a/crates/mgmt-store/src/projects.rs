//! Project store — one markdown file per project under `<root>/projects/`, mirroring the task
//! vault so projects are just as portable and hand-editable.
//!
//! A legacy newline-delimited `<root>/projects` *file* (the old registry) is migrated to the
//! directory form on first load: its names become bare `<name>.md` files and the file is removed.

use std::path::{Path, PathBuf};

use mgmt_core::Result;
use mgmt_domain::Project;

use crate::paths;

pub struct ProjectStore {
    dir: PathBuf,
}

impl ProjectStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        ProjectStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.md", paths::safe_stem(name)))
    }

    /// On-disk path of a project's markdown file (whether or not it exists yet).
    pub fn project_path(&self, name: &str) -> PathBuf {
        self.path_for(name)
    }

    /// Load all projects, migrating a legacy newline `projects` file first if present.
    pub fn load_all(&self) -> Result<Vec<Project>> {
        self.migrate_legacy()?;
        let mut projects = Vec::new();
        for file in paths::collect_files(&self.dir, "md")? {
            let text = std::fs::read_to_string(&file)?;
            if let Ok(p) = mgmt_markdown::parse_project(&text) {
                projects.push(p);
            }
        }
        Ok(projects)
    }

    pub fn upsert(&self, project: &Project) -> Result<()> {
        let text = mgmt_markdown::serialize_project(project)?;
        paths::atomic_write(&self.path_for(&project.name), &text)?;
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<bool> {
        let path = self.path_for(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// If `<root>/projects` exists as a *file* (the old registry), read its names, remove it, and
    /// recreate each as a project markdown file in the directory of the same name.
    fn migrate_legacy(&self) -> Result<()> {
        if self.dir.is_file() {
            let text = std::fs::read_to_string(&self.dir)?;
            let names: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            std::fs::remove_file(&self.dir)?;
            for name in names {
                self.upsert(&Project::new(name))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_load_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::new(dir.path().join("projects"));
        store.upsert(&Project::new("wng").with_color("blue")).unwrap();
        store.upsert(&Project::new("home")).unwrap();
        let mut loaded = store.load_all().unwrap();
        loaded.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].name, "wng");
        assert_eq!(loaded[1].color.as_deref(), Some("blue"));
        assert!(store.delete("home").unwrap());
        assert_eq!(store.load_all().unwrap().len(), 1);
    }

    #[test]
    fn migrates_legacy_newline_file() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        std::fs::write(&projects, "wng\nhome\n").unwrap();
        let store = ProjectStore::new(&projects);
        let mut loaded = store.load_all().unwrap();
        loaded.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = loaded.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["home", "wng"]);
        // the legacy file is gone, replaced by a directory
        assert!(projects.is_dir());
    }

    #[test]
    fn load_missing_is_empty() {
        let store = ProjectStore::new(PathBuf::from("/no/such/projects"));
        assert!(store.load_all().unwrap().is_empty());
    }
}
