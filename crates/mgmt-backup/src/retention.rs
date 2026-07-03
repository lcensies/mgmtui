//! Pure retention planning: given the snapshots present on a remote and a policy, decide which
//! ones to delete. No I/O — fully unit-tested.

use chrono::{DateTime, Duration, Utc};

/// Which snapshots to keep. A snapshot survives if it is among the newest `keep_last` **and**
/// (when `keep_days` is set) younger than that many days — so `keep_last` bounds the count and
/// `keep_days` is a maximum age cap. The newest snapshot is always kept. Both conditions must hold,
/// which keeps the remote file count bounded (frequent backups don't accumulate for the whole
/// age window).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// Keep at most this many of the most recent snapshots.
    pub keep_last: usize,
    /// Also drop any snapshot older than this many days, even if within `keep_last` (`None` = no
    /// age cap).
    pub keep_days: Option<u32>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        RetentionPolicy { keep_last: 14, keep_days: Some(180) }
    }
}

/// A snapshot as discovered on the remote (parsed from its sidecar / name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMeta {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub size: u64,
}

/// Return the names of snapshots that should be pruned, newest-first ordering applied internally.
/// The most recent snapshot is always retained even if the policy would otherwise drop it.
pub fn plan_prune(snaps: &[SnapshotMeta], policy: &RetentionPolicy, now: DateTime<Utc>) -> Vec<String> {
    let mut sorted: Vec<&SnapshotMeta> = snaps.iter().collect();
    // Newest first. Ties broken by name so the result is deterministic.
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.name.cmp(&b.name)));

    let keep_last = policy.keep_last.max(1); // never prune everything
    let age_cutoff = policy.keep_days.map(|d| now - Duration::days(d as i64));

    let mut doomed = Vec::new();
    for (idx, snap) in sorted.iter().enumerate() {
        let within_count = idx < keep_last;
        let within_age = age_cutoff.map(|cut| snap.created_at >= cut).unwrap_or(true);
        // Index 0 (newest) is always kept; otherwise both the count and age bounds must hold.
        if idx == 0 || (within_count && within_age) {
            continue;
        }
        doomed.push(snap.name.clone());
    }
    doomed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(name: &str, days_ago: i64, now: DateTime<Utc>) -> SnapshotMeta {
        SnapshotMeta { name: name.into(), created_at: now - Duration::days(days_ago), size: 100 }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-03T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn empty_prunes_nothing() {
        let policy = RetentionPolicy::default();
        assert!(plan_prune(&[], &policy, now()).is_empty());
    }

    #[test]
    fn keeps_at_least_newest_even_with_zero_keep_last() {
        let n = now();
        let snaps = vec![snap("a", 0, n), snap("b", 1000, n)];
        let policy = RetentionPolicy { keep_last: 0, keep_days: None };
        let doomed = plan_prune(&snaps, &policy, n);
        assert_eq!(doomed, vec!["b".to_string()]); // newest survives, old one pruned
    }

    #[test]
    fn keep_last_bounds_count() {
        let n = now();
        // 5 snapshots, keep_last 3, no age window -> oldest 2 pruned.
        let snaps: Vec<_> = (0..5).map(|i| snap(&format!("s{i}"), i as i64, n)).collect();
        let policy = RetentionPolicy { keep_last: 3, keep_days: None };
        let doomed = plan_prune(&snaps, &policy, n);
        assert_eq!(doomed, vec!["s3".to_string(), "s4".to_string()]);
    }

    #[test]
    fn age_caps_retention_even_within_count() {
        let n = now();
        // Generous keep_last, but a 30-day age cap drops the 200-day-old snapshot.
        let snaps = vec![snap("recent", 10, n), snap("old", 200, n), snap("newest", 0, n)];
        let policy = RetentionPolicy { keep_last: 5, keep_days: Some(30) };
        let doomed = plan_prune(&snaps, &policy, n);
        assert_eq!(doomed, vec!["old".to_string()]);
    }

    #[test]
    fn count_bounds_even_within_age() {
        let n = now();
        // All within the age window, but keep_last caps the count so the tail is pruned.
        let snaps: Vec<_> = (0..5).map(|i| snap(&format!("s{i}"), i as i64, n)).collect();
        let policy = RetentionPolicy { keep_last: 2, keep_days: Some(180) };
        let doomed = plan_prune(&snaps, &policy, n);
        assert_eq!(doomed, vec!["s2".to_string(), "s3".to_string(), "s4".to_string()]);
    }

    #[test]
    fn nothing_pruned_when_all_within_both_windows() {
        let n = now();
        let snaps: Vec<_> = (0..3).map(|i| snap(&format!("s{i}"), i as i64, n)).collect();
        let policy = RetentionPolicy { keep_last: 14, keep_days: Some(180) };
        assert!(plan_prune(&snaps, &policy, n).is_empty());
    }
}
