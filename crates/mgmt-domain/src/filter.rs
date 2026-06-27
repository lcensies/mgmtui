//! Task filtering and sorting. Pure predicates over `Task`, used by the list/kanban/inbox
//! views to narrow and order what the user sees. Kept time- and config-free: callers translate
//! "today"/"open" into concrete values (a date window, the set of open status ids) before
//! building a [`Filter`], so `matches` stays a deterministic predicate.

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::{Priority, Task};

/// A conjunctive filter: every populated field must match. Empty filter matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub project: Option<String>,
    pub area: Option<String>,
    pub tag: Option<String>,
    /// Exact status id match.
    pub status: Option<String>,
    /// Case-insensitive substring match against the title.
    pub text: Option<String>,
    /// When set, only tasks whose status id is in this set pass (the "open" statuses). Built by
    /// the caller from the active [`crate::Workflow`].
    pub open_statuses: Option<Vec<String>>,
    /// Inbox semantics: only tasks with no project.
    pub no_project: bool,
    /// Lower bound (inclusive) on `calendar_date()`.
    pub date_from: Option<DateTime<Utc>>,
    /// Upper bound (exclusive) on `calendar_date()`.
    pub date_to: Option<DateTime<Utc>>,
}

impl Filter {
    pub fn matches(&self, t: &Task) -> bool {
        if let Some(p) = &self.project {
            if t.project.as_deref() != Some(p.as_str()) {
                return false;
            }
        }
        if self.no_project && t.project.is_some() {
            return false;
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
        if let Some(s) = &self.status {
            if &t.status != s {
                return false;
            }
        }
        if let Some(open) = &self.open_statuses {
            if !open.iter().any(|s| s == &t.status) {
                return false;
            }
        }
        if let Some(text) = &self.text {
            if !t.title.to_lowercase().contains(&text.to_lowercase()) {
                return false;
            }
        }
        if self.date_from.is_some() || self.date_to.is_some() {
            let Some(d) = t.calendar_date() else { return false };
            if let Some(from) = self.date_from {
                if d < from {
                    return false;
                }
            }
            if let Some(to) = self.date_to {
                if d >= to {
                    return false;
                }
            }
        }
        true
    }
}

/// TickTick / Microsoft To Do-style smart lists for the Tasks view. Each translates to a
/// concrete [`Filter`] given the current day and the workflow's open status set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartView {
    /// Open tasks with no project.
    Inbox,
    /// Open tasks due/scheduled today or overdue.
    Today,
    /// Open tasks due/scheduled within the next 7 days.
    Next7,
    /// Every open task.
    All,
}

impl SmartView {
    pub const ALL: [SmartView; 4] = [SmartView::Inbox, SmartView::Today, SmartView::Next7, SmartView::All];

    pub fn label(self) -> &'static str {
        match self {
            SmartView::Inbox => "Inbox",
            SmartView::Today => "Today",
            SmartView::Next7 => "Next 7 days",
            SmartView::All => "All",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SmartView::Inbox => "inbox",
            SmartView::Today => "today",
            SmartView::Next7 => "next7",
            SmartView::All => "all",
        }
    }

    pub fn from_id(id: &str) -> Option<SmartView> {
        SmartView::ALL.into_iter().find(|v| v.id() == id)
    }

    /// Build the concrete filter for this view as of `today`, restricted to the given open
    /// status ids.
    pub fn to_filter(self, today: NaiveDate, open_statuses: Vec<String>) -> Filter {
        let start = today.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let end_of_today = start + Duration::days(1);
        let mut f = Filter { open_statuses: Some(open_statuses), ..Default::default() };
        match self {
            SmartView::Inbox => f.no_project = true,
            SmartView::Today => f.date_to = Some(end_of_today),
            SmartView::Next7 => {
                f.date_from = Some(start);
                f.date_to = Some(start + Duration::days(7));
            }
            // "All" is the catch-all: show every task, completed included.
            SmartView::All => f.open_statuses = None,
        }
        f
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

    /// A short human label for the sort mode (used in the Tasks panel title and status line).
    pub fn label(self) -> &'static str {
        match self {
            SortMode::DueDate => "due date",
            SortMode::Priority => "priority",
            SortMode::Title => "title",
            SortMode::Created => "created",
        }
    }

    /// The next sort mode, cycling DueDate → Priority → Title → Created → DueDate.
    pub fn next(self) -> SortMode {
        match self {
            SortMode::DueDate => SortMode::Priority,
            SortMode::Priority => SortMode::Title,
            SortMode::Title => SortMode::Created,
            SortMode::Created => SortMode::DueDate,
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
    fn open_statuses_hides_done() {
        let f = Filter {
            open_statuses: Some(vec!["todo".into(), "doing".into()]),
            ..Default::default()
        };
        let mut t = Task::new("x");
        t.status = "done".into();
        assert!(!f.matches(&t));
        t.status = "todo".into();
        assert!(f.matches(&t));
    }

    #[test]
    fn inbox_excludes_projects() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();
        let f = SmartView::Inbox.to_filter(today, vec!["todo".into()]);
        assert!(f.matches(&Task::new("loose")));
        assert!(!f.matches(&Task::new("filed").with_project("wng")));
    }

    #[test]
    fn today_includes_overdue_and_today_excludes_future() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();
        let f = SmartView::Today.to_filter(today, vec!["todo".into()]);
        let mut overdue = Task::new("overdue");
        overdue.due = Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap().and_hms_opt(9, 0, 0).unwrap().and_utc());
        let mut future = Task::new("future");
        future.due = Some(NaiveDate::from_ymd_opt(2026, 6, 25).unwrap().and_hms_opt(9, 0, 0).unwrap().and_utc());
        assert!(f.matches(&overdue));
        assert!(!f.matches(&future));
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
