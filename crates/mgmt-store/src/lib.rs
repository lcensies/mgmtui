//! Local-first persistence for mgmt: a markdown vault for tasks and a vdir tree for events.
//!
//! Both stores implement [`mgmt_core::Store`] so the service layer can treat them uniformly.

mod paths;
mod projects;
mod trash;
mod vault;
mod vdir;

pub use paths::{
    atomic_write, calendars_dir, collect_files, data_root, local_vault_root, migrate_to_multiuser,
    projects_dir, safe_stem, tasks_dir, user_root, users_dir, ADMIN_USER,
};
pub use projects::ProjectStore;
pub use trash::TrashStore;
pub use vault::VaultStore;
pub use vdir::VdirStore;
