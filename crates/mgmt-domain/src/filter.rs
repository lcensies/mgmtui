//! Task filtering and sorting. Pure predicates over `Task`, used by the list/kanban/inbox
//! views to narrow and order what the user sees.

use crate::{Priority, Task, TaskStatus};

/// A conjunctive filter: every populated field must match. Empty filter matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub project: Option<String>,
    pub area: Option<String>,
    pub tag: Option<String>,
    pub status: Option<TaskStatus>,
    /// Case-insensitive substring match against the title.
    pub text: Option<String>,
    /// When true, hide Done and Cancelled tasks.
    pub only_open: bool,
}

impl Filter {
    pub fn matches(&self, t: &Task) -> bool {
        if let Some(p) = &self.project {
            if t.project.as_deref() != Some(p.as_str()) {
                return false;
            }
        }
        if let Some(a) = &self.area {
            if t.area.as_deref() != Some(a.as_str()) {
                return false;
            }
        }
        if let Some(tag) = &self.tag {
            if !t.tags.iter().any(|x| x == tag) {
                return false;
            }
        }
        if let Some(s) = self.status {
            if t.status != s {
                return false;
            }
        }
        if let Some(text) = &self.text {
            if !t.title.to_lowercase().contains(&text.to_lowercase()) {
                return false;
            }
        }
        if self.only_open && !t.status.is_open() {
            return false;
        }
        true
    }
}

/// Ordering applied after filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    DueDate,
    Priority,
    Title,
    Created,
}

impl SortMode {
    /// Sort a slice in place according to this mode. Stable, with sensible tie-breaks
    /// (items lacking the sort key go last).
    pub fn apply(self, tasks: &mut [Task]) {
        match self {
            SortMode::DueDate => tasks.sort_by(|a, b| opt_min_first(a.due, b.due)),
            SortMode::Priority => {
                // High priority first, so reverse the natural Priority ordering.
                tasks.sort_by(|a, b| rank(b.priority).cmp(&rank(a.priority)));
            }
            SortMode::Title => tasks.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            SortMode::Created => tasks.sort_by(|a, b| opt_min_first(a.created, b.created)),
        }
    }
}

fn rank(p: Priority) -> u8 {
    match p {
        Priority::None => 0,
        Priority::Low => 1,
        Priority::Medium => 2,
        Priority::High => 3,
    }
}

/// Compare two `Option<T>` so that `Some` values sort ascending before any `None`.
fn opt_min_first<T: Ord>(a: Option<T>, b: Option<T>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filter_matches_anything() {
        assert!(Filter::default().matches(&Task::new("hi")));
    }

    #[test]
    fn project_filter_is_exact() {
        let f = Filter {
            project: Some("wng".into()),
            ..Default::default()
        };
        assert!(f.matches(&Task::new("a").with_project("wng")));
        assert!(!f.matches(&Task::new("b").with_project("other")));
        assert!(!f.matches(&Task::new("c")));
    }

    #[test]
    fn only_open_hides_done() {
        let f = Filter {
            only_open: true,
            ..Default::default()
        };
        let mut t = Task::new("x");
        t.status = TaskStatus::Done;
        assert!(!f.matches(&t));
        t.status = TaskStatus::Todo;
        assert!(f.matches(&t));
    }

    #[test]
    fn priority_sort_puts_high_first() {
        let mut hi = Task::new("hi");
        hi.priority = Priority::High;
        let lo = Task::new("lo");
        let mut v = vec![lo, hi];
        SortMode::Priority.apply(&mut v);
        assert_eq!(v[0].title, "hi");
    }

    #[test]
    fn due_sort_puts_dateless_last() {
        let mut soon = Task::new("soon");
        soon.due = Some(chrono::Utc::now());
        let none = Task::new("none");
        let mut v = vec![none, soon];
        SortMode::DueDate.apply(&mut v);
        assert_eq!(v[0].title, "soon");
    }
}
