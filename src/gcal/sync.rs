use sha2::{Digest, Sha256};

/// Compute a content hash for idempotency. Same approach as the HA sync:
/// hash (summary, description, start_rfc3339, end_rfc3339) to detect changes
/// between sync runs.
pub fn content_hash(
    summary: &str,
    description: &str,
    start_rfc3339: &str,
    end_rfc3339: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(summary.as_bytes());
    hasher.update(b"\0");
    hasher.update(description.as_bytes());
    hasher.update(b"\0");
    hasher.update(start_rfc3339.as_bytes());
    hasher.update(b"\0");
    hasher.update(end_rfc3339.as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build the event description, embedding the entry ID marker for
/// reconciliation (same pattern as ha/sync.rs).
pub fn build_description(
    attendee_names: &[String],
    notes: Option<&str>,
    meal_plan_entry_id: i64,
) -> String {
    let mut parts = Vec::new();

    if !attendee_names.is_empty() {
        parts.push(format!("Attendees: {}", attendee_names.join(", ")));
    }
    if let Some(notes) = notes {
        if !notes.is_empty() {
            parts.push(format!("Notes: {notes}"));
        }
    }

    let body = parts.join("\n");
    format!("{body}\n\nfoodinator:entry={meal_plan_entry_id}")
}

/// Extract the foodinator entry ID from a Google Calendar event description.
pub fn extract_marker_entry_id(description: &str) -> Option<i64> {
    for line in description.lines().rev() {
        if let Some(id_str) = line.strip_prefix("foodinator:entry=") {
            return id_str.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_deterministic() {
        let h1 = content_hash(
            "Tacos",
            "desc",
            "2026-08-10T18:00:00Z",
            "2026-08-10T18:30:00Z",
        );
        let h2 = content_hash(
            "Tacos",
            "desc",
            "2026-08-10T18:00:00Z",
            "2026-08-10T18:30:00Z",
        );
        assert_eq!(h1, h2);
    }

    #[test]
    fn content_hash_changes_on_summary_change() {
        let h1 = content_hash(
            "Tacos",
            "desc",
            "2026-08-10T18:00:00Z",
            "2026-08-10T18:30:00Z",
        );
        let h2 = content_hash(
            "Pizza",
            "desc",
            "2026-08-10T18:00:00Z",
            "2026-08-10T18:30:00Z",
        );
        assert_ne!(h1, h2);
    }

    #[test]
    fn build_description_includes_attendees_and_notes_and_marker() {
        let desc = build_description(
            &["Alice".to_string(), "Bob".to_string()],
            Some("bring hot sauce"),
            42,
        );
        assert!(desc.contains("Attendees: Alice, Bob"));
        assert!(desc.contains("Notes: bring hot sauce"));
        assert!(desc.contains("foodinator:entry=42"));
    }

    #[test]
    fn build_description_with_no_notes() {
        let desc = build_description(&["Alice".to_string()], None, 1);
        assert!(desc.contains("Attendees: Alice"));
        assert!(!desc.contains("Notes:"));
        assert!(desc.contains("foodinator:entry=1"));
    }

    #[test]
    fn extract_marker_entry_id_from_valid_description() {
        let desc = "Attendees: Alice\nNotes: stuff\n\nfoodinator:entry=99";
        assert_eq!(extract_marker_entry_id(desc), Some(99));
    }

    #[test]
    fn extract_marker_entry_id_returns_none_when_absent() {
        assert_eq!(extract_marker_entry_id("no marker here"), None);
    }
}
