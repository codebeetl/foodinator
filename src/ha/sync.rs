use chrono::NaiveDate;
use sha2::{Digest, Sha256};

const MARKER_PREFIX: &str = "foodinator:entry=";

/// Deterministic fingerprint of the fields we push to HA, so a re-run can tell
/// whether a previously-synced entry has actually changed.
pub fn content_hash(summary: &str, description: &str, start: &str, end: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(summary.as_bytes());
    hasher.update(0u8.to_le_bytes());
    hasher.update(description.as_bytes());
    hasher.update(0u8.to_le_bytes());
    hasher.update(start.as_bytes());
    hasher.update(0u8.to_le_bytes());
    hasher.update(end.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Embedded in an HA event's description so we can recognize it as ours on read-back,
/// since HA's REST API has no update/delete service to key off of an ha_uid alone.
pub fn build_marker(meal_plan_entry_id: i64) -> String {
    format!("{MARKER_PREFIX}{meal_plan_entry_id}")
}

/// Recovers the meal_plan_entry_id from a description previously produced by
/// `build_marker`. Returns None if the marker is missing or corrupted (e.g. a human
/// hand-edited the event in HA's UI) - callers surface that as a manual-cleanup case.
pub fn extract_marker_entry_id(description: &str) -> Option<i64> {
    description
        .lines()
        .find_map(|line| line.strip_prefix(MARKER_PREFIX))
        .and_then(|rest| rest.trim().parse().ok())
}

/// Only entries within `horizon_days` of today are eligible to sync, so most edits to
/// a meal-plan entry happen before anything was ever pushed to HA (HA has no update
/// service, so an already-pushed entry can't be corrected - only avoided).
pub fn is_within_sync_horizon(entry_date: NaiveDate, today: NaiveDate, horizon_days: i64) -> bool {
    let delta = (entry_date - today).num_days();
    (0..=horizon_days).contains(&delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_changes_when_any_field_changes() {
        let base = content_hash(
            "Dinner",
            "desc",
            "2026-08-10T18:00:00Z",
            "2026-08-10T19:00:00Z",
        );
        let changed_summary = content_hash(
            "Lunch",
            "desc",
            "2026-08-10T18:00:00Z",
            "2026-08-10T19:00:00Z",
        );
        let changed_start = content_hash(
            "Dinner",
            "desc",
            "2026-08-11T18:00:00Z",
            "2026-08-10T19:00:00Z",
        );

        assert_ne!(base, changed_summary);
        assert_ne!(base, changed_start);
    }

    #[test]
    fn content_hash_is_stable_for_identical_input() {
        let a = content_hash(
            "Dinner",
            "desc",
            "2026-08-10T18:00:00Z",
            "2026-08-10T19:00:00Z",
        );
        let b = content_hash(
            "Dinner",
            "desc",
            "2026-08-10T18:00:00Z",
            "2026-08-10T19:00:00Z",
        );
        assert_eq!(a, b);
    }

    #[test]
    fn content_hash_does_not_collide_across_field_boundaries() {
        // Without a separator, ("ab", "c") and ("a", "bc") would hash identically.
        let a = content_hash("ab", "c", "", "");
        let b = content_hash("a", "bc", "", "");
        assert_ne!(a, b);
    }

    #[test]
    fn marker_round_trips_through_a_description() {
        let marker = build_marker(42);
        let description = format!("Alice's dinner\n\n{marker}");

        assert_eq!(extract_marker_entry_id(&description), Some(42));
    }

    #[test]
    fn extract_marker_entry_id_returns_none_when_marker_is_missing_or_corrupted() {
        assert_eq!(extract_marker_entry_id("just a plain description"), None);
        assert_eq!(
            extract_marker_entry_id("foodinator:entry=not-a-number"),
            None
        );
    }

    #[test]
    fn horizon_includes_today_and_excludes_beyond_the_window() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();

        assert!(is_within_sync_horizon(today, today, 7));
        assert!(is_within_sync_horizon(
            today + chrono::Duration::days(7),
            today,
            7
        ));
        assert!(!is_within_sync_horizon(
            today + chrono::Duration::days(8),
            today,
            7
        ));
        assert!(!is_within_sync_horizon(
            today - chrono::Duration::days(1),
            today,
            7
        ));
    }
}
