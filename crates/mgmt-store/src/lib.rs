//! Local-first persistence for mgmt: a markdown vault for tasks and a vdir tree for events.
//!
//! Both stores implement [`mgmt_core::Store`] so the service layer can treat them uniformly.

mod paths;
mod projects;
mod vault;
mod vdir;

pub use paths::{calendars_dir, data_root, projects_file, tasks_dir};
pub use projects::{load as load_projects, save as save_projects};
pub use vault::VaultStore;
pub use vdir::VdirStore;
