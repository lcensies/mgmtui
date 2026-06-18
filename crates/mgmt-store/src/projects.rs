//! Project registry — a newline-delimited file of known project names so that empty
//! projects (no tasks yet) still persist. Human-editable.

use std::path::Path;

use mgmt_core::Result;

/// Load project names from `path`, returning an empty list if the file is absent.
pub fn load(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Persist project names to `path`, one per line, sorted and de-duplicated.
pub fn save(path: &Path, projects: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut names: Vec<&String> = projects.iter().collect();
    names.sort();
    names.dedup();
    let body: String = names.iter().map(|n| format!("{n}\n")).collect();
    std::fs::write(path, body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips_sorted_unique() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects");
        save(&path, &["wng".into(), "home".into(), "wng".into()]).unwrap();
        assert_eq!(load(&path).unwrap(), vec!["home".to_string(), "wng".to_string()]);
    }

    #[test]
    fn load_missing_is_empty() {
        assert!(load(&std::path::PathBuf::from("/no/such/projects")).unwrap().is_empty());
    }
}
