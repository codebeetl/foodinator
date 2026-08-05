use chrono::{NaiveDate, NaiveTime};
use sqlx::PgPool;

use crate::ha::sync::is_within_sync_horizon;

/// A meal_plan_entry eligible to sync (non-deleted, not yet successfully
/// synced per the ha_calendar_sync ledger), with everything needed to build
/// the HA event.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncCandidate {
    pub meal_plan_entry_id: i64,
    pub entry_date: NaiveDate,
    pub meal_name: String,
    pub notes: Option<String>,
    pub start_time_override: Option<NaiveTime>,
    pub duration_minutes_override: Option<i32>,
    pub attendee_names: Vec<String>,
}

/// Non-deleted, not-yet-successfully-synced entries within `horizon_days` of
/// `today` - the ledger (not HA) is the source of truth for what's already
/// synced, since HA's REST API has no update/delete service to correct a
/// previously-pushed event.
pub async fn list_syncable(
    pool: &PgPool,
    today: NaiveDate,
    horizon_days: i64,
) -> sqlx::Result<Vec<SyncCandidate>> {
    struct Row {
        id: i64,
        entry_date: NaiveDate,
        meal_name: String,
        notes: Option<String>,
        start_time_override: Option<NaiveTime>,
        duration_minutes_override: Option<i32>,
        attendee_names: Vec<String>,
    }

    let rows = sqlx::query_as!(
        Row,
        r#"SELECT mpe.id, mpe.entry_date, m.name AS meal_name, mpe.notes,
             mpe.start_time_override, mpe.duration_minutes_override,
             array_remove(array_agg(c.name ORDER BY c.name), NULL) AS "attendee_names!"
           FROM meal_plan_entries mpe
           JOIN meals m ON m.id = mpe.meal_id
           LEFT JOIN meal_attendance ma ON ma.meal_plan_entry_id = mpe.id
           LEFT JOIN consumers c ON c.id = ma.consumer_id
           LEFT JOIN ha_calendar_sync hcs ON hcs.meal_plan_entry_id = mpe.id
           WHERE mpe.deleted_at IS NULL
             AND hcs.synced_at IS NULL
             AND mpe.entry_date >= $1
           GROUP BY mpe.id, m.name
           ORDER BY mpe.entry_date"#,
        today
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter(|row| is_within_sync_horizon(row.entry_date, today, horizon_days))
        .map(|row| SyncCandidate {
            meal_plan_entry_id: row.id,
            entry_date: row.entry_date,
            meal_name: row.meal_name,
            notes: row.notes,
            start_time_override: row.start_time_override,
            duration_minutes_override: row.duration_minutes_override,
            attendee_names: row.attendee_names,
        })
        .collect())
}

/// Records a successful push: the ledger is now the reason this entry won't
/// be re-pushed on a future sync run.
pub async fn record_synced(
    pool: &PgPool,
    meal_plan_entry_id: i64,
    content_hash: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ha_calendar_sync (meal_plan_entry_id, content_hash, synced_at, last_error) \
         VALUES ($1, $2, now(), NULL) \
         ON CONFLICT (meal_plan_entry_id) \
         DO UPDATE SET content_hash = $2, synced_at = now(), last_error = NULL",
        meal_plan_entry_id,
        content_hash
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Records a failed push attempt, leaving the entry eligible to retry on the
/// next sync run.
pub async fn record_failed(
    pool: &PgPool,
    meal_plan_entry_id: i64,
    content_hash: &str,
    error: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ha_calendar_sync (meal_plan_entry_id, content_hash, synced_at, last_error) \
         VALUES ($1, $2, NULL, $3) \
         ON CONFLICT (meal_plan_entry_id) \
         DO UPDATE SET content_hash = $2, synced_at = NULL, last_error = $3",
        meal_plan_entry_id,
        content_hash,
        error
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{consumers, meal_plan, meals};

    #[sqlx::test(migrations = "./migrations")]
    async fn list_syncable_includes_attendees_and_excludes_out_of_horizon_and_deleted(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();

        let in_range = meal_plan::upsert_entry(
            &pool,
            today + chrono::Duration::days(1),
            tacos.id,
            Some("notes"),
            None,
            None,
        )
        .await?;
        meal_plan::set_attendance(&pool, in_range.id, &[alice.id]).await?;

        meal_plan::upsert_entry(
            &pool,
            today + chrono::Duration::days(30),
            tacos.id,
            None,
            None,
            None,
        )
        .await?;

        let deleted_date = today + chrono::Duration::days(2);
        meal_plan::upsert_entry(&pool, deleted_date, tacos.id, None, None, None).await?;
        meal_plan::soft_delete(&pool, deleted_date).await?;

        let candidates = list_syncable(&pool, today, 14).await?;

        assert_eq!(candidates.len(), 1, "only the in-range, non-deleted entry");
        assert_eq!(candidates[0].meal_plan_entry_id, in_range.id);
        assert_eq!(candidates[0].meal_name, "Tacos");
        assert_eq!(candidates[0].attendee_names, vec!["Alice".to_string()]);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn recording_a_sync_removes_the_entry_from_future_candidate_lists(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let entry = meal_plan::upsert_entry(&pool, today, tacos.id, None, None, None).await?;

        record_synced(&pool, entry.id, "hash-1").await?;

        let candidates = list_syncable(&pool, today, 14).await?;
        assert!(candidates.is_empty(), "already-synced entries are excluded");

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn recording_a_failure_keeps_the_entry_eligible_for_retry(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let entry = meal_plan::upsert_entry(&pool, today, tacos.id, None, None, None).await?;

        record_failed(&pool, entry.id, "hash-1", "connection refused").await?;

        let candidates = list_syncable(&pool, today, 14).await?;
        assert_eq!(candidates.len(), 1, "a failed attempt should still retry");

        Ok(())
    }
}
