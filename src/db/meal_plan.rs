use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq)]
pub struct MealPlanEntry {
    pub id: i64,
    pub entry_date: NaiveDate,
    pub meal_id: i64,
    pub notes: Option<String>,
    pub start_time_override: Option<NaiveTime>,
    pub duration_minutes_override: Option<i32>,
    pub guest_names: Vec<String>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A meal alongside how a given set of attendees feels about it. Never
/// excludes - the picker UI flags liked/disliked meals rather than hiding
/// them, since a hard exclusion could leave the list empty.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MealSuitability {
    pub id: i64,
    pub name: String,
    pub liked_by_attendee: bool,
    // Names (not just a bool) so the UI can say who specifically dislikes
    // it, e.g. "disliked by Alice" - liked attendees aren't named, only
    // whether any exist (see the picker UI's thumbs-up/down/shrug emoji).
    pub disliked_by_attendee_names: Vec<String>,
}

/// Inserts or updates the single meal_plan_entries row for `entry_date`,
/// un-deleting it if it was previously soft-deleted.
pub async fn upsert_entry(
    pool: &PgPool,
    entry_date: NaiveDate,
    meal_id: i64,
    notes: Option<&str>,
    start_time_override: Option<NaiveTime>,
    duration_minutes_override: Option<i32>,
    guest_names: &[String],
) -> sqlx::Result<MealPlanEntry> {
    sqlx::query_as!(
        MealPlanEntry,
        "INSERT INTO meal_plan_entries \
           (entry_date, meal_id, notes, start_time_override, duration_minutes_override, guest_names) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (entry_date) DO UPDATE SET \
           meal_id = EXCLUDED.meal_id, \
           notes = EXCLUDED.notes, \
           start_time_override = EXCLUDED.start_time_override, \
           duration_minutes_override = EXCLUDED.duration_minutes_override, \
           guest_names = EXCLUDED.guest_names, \
           deleted_at = NULL, \
           updated_at = now() \
         RETURNING id, entry_date, meal_id, notes, start_time_override, \
           duration_minutes_override, guest_names, deleted_at, created_at, updated_at",
        entry_date,
        meal_id,
        notes,
        start_time_override,
        duration_minutes_override,
        guest_names
    )
    .fetch_one(pool)
    .await
}

/// Soft-deletes the entry for `entry_date`, if a non-deleted one exists.
/// A no-op (not an error) when there's nothing active to delete.
pub async fn soft_delete(pool: &PgPool, entry_date: NaiveDate) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE meal_plan_entries SET deleted_at = now(), updated_at = now() \
         WHERE entry_date = $1 AND deleted_at IS NULL",
        entry_date
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_by_date(
    pool: &PgPool,
    entry_date: NaiveDate,
) -> sqlx::Result<Option<MealPlanEntry>> {
    sqlx::query_as!(
        MealPlanEntry,
        "SELECT id, entry_date, meal_id, notes, start_time_override, \
           duration_minutes_override, guest_names, deleted_at, created_at, updated_at \
         FROM meal_plan_entries WHERE entry_date = $1",
        entry_date
    )
    .fetch_optional(pool)
    .await
}

/// Replaces the full attendance list for a meal_plan_entry.
pub async fn set_attendance(
    pool: &PgPool,
    meal_plan_entry_id: i64,
    consumer_ids: &[i64],
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query!(
        "DELETE FROM meal_attendance WHERE meal_plan_entry_id = $1",
        meal_plan_entry_id
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        "INSERT INTO meal_attendance (meal_plan_entry_id, consumer_id) \
         SELECT $1, * FROM UNNEST($2::bigint[])",
        meal_plan_entry_id,
        consumer_ids
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await
}

pub async fn get_attendance(pool: &PgPool, meal_plan_entry_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT consumer_id FROM meal_attendance WHERE meal_plan_entry_id = $1 \
         ORDER BY consumer_id",
        meal_plan_entry_id
    )
    .fetch_all(pool)
    .await
}

/// Given a set of attendee consumer IDs, returns every active meal annotated
/// with how those attendees feel about it.
pub async fn suitability_for_attendees(
    pool: &PgPool,
    attendee_ids: &[i64],
) -> sqlx::Result<Vec<MealSuitability>> {
    sqlx::query_as!(
        MealSuitability,
        r#"SELECT m.id, m.name, EXISTS (
             SELECT 1 FROM consumer_meal_preferences p
             WHERE p.meal_id = m.id
               AND p.preference = 'like'
               AND p.consumer_id = ANY($1::bigint[])
           ) AS "liked_by_attendee!",
           COALESCE(
             (SELECT array_agg(c.name ORDER BY c.name COLLATE "C")
              FROM consumer_meal_preferences p
              JOIN consumers c ON c.id = p.consumer_id
              WHERE p.meal_id = m.id
                AND p.preference = 'dislike'
                AND p.consumer_id = ANY($1::bigint[])),
             ARRAY[]::text[]
           ) AS "disliked_by_attendee_names!"
         FROM meals m
         WHERE m.active
         ORDER BY m.name COLLATE "C""#,
        attendee_ids
    )
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{consumers, meals, preferences};

    #[sqlx::test(migrations = "./migrations")]
    async fn upserting_the_same_date_twice_updates_rather_than_duplicates(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = meals::insert(&pool, "Tacos").await?;
        let pasta = meals::insert(&pool, "Pasta").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();

        let first = upsert_entry(&pool, date, tacos.id, Some("first"), None, None, &[]).await?;
        assert_eq!(first.meal_id, tacos.id);

        let second =
            upsert_entry(&pool, date, pasta.id, Some("second"), None, Some(45), &[]).await?;
        assert_eq!(second.id, first.id, "same date should update the same row");
        assert_eq!(second.meal_id, pasta.id);
        assert_eq!(second.notes.as_deref(), Some("second"));
        assert_eq!(second.duration_minutes_override, Some(45));

        let fetched = get_by_date(&pool, date).await?.expect("entry should exist");
        assert_eq!(fetched, second);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn soft_delete_clears_the_day_and_can_be_replanned(pool: PgPool) -> sqlx::Result<()> {
        let meal = meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let created = upsert_entry(&pool, date, meal.id, None, None, None, &[]).await?;

        soft_delete(&pool, date).await?;

        let fetched = get_by_date(&pool, date).await?.expect("row still exists");
        assert!(fetched.deleted_at.is_some());

        let replanned = upsert_entry(&pool, date, meal.id, None, None, None, &[]).await?;
        assert_eq!(
            replanned.id, created.id,
            "re-planning the same date should reuse the row"
        );
        assert!(replanned.deleted_at.is_none(), "upserting should un-delete");

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn guest_names_round_trip_and_default_to_empty(pool: PgPool) -> sqlx::Result<()> {
        let meal = meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();

        let no_guests = upsert_entry(&pool, date, meal.id, None, None, None, &[]).await?;
        assert!(no_guests.guest_names.is_empty());

        let guests = vec!["Aunt Jane".to_string(), "Uncle Bob".to_string()];
        let with_guests = upsert_entry(&pool, date, meal.id, None, None, None, &guests).await?;
        assert_eq!(with_guests.guest_names, guests);

        let fetched = get_by_date(&pool, date).await?.expect("entry should exist");
        assert_eq!(fetched.guest_names, guests);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn set_attendance_replaces_the_full_list(pool: PgPool) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let bob = consumers::insert(&pool, "Bob").await?;
        let meal = meals::insert(&pool, "Tacos").await?;
        let entry = upsert_entry(
            &pool,
            NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
            meal.id,
            None,
            None,
            None,
            &[],
        )
        .await?;

        set_attendance(&pool, entry.id, &[alice.id, bob.id]).await?;
        let mut attendance = get_attendance(&pool, entry.id).await?;
        attendance.sort();
        assert_eq!(attendance, vec![alice.id, bob.id]);

        set_attendance(&pool, entry.id, &[alice.id]).await?;
        let attendance = get_attendance(&pool, entry.id).await?;
        assert_eq!(attendance, vec![alice.id], "replacing should drop bob");

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn suitability_flags_disliked_meals_without_excluding_them(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = meals::insert(&pool, "Tacos").await?;
        let pasta = meals::insert(&pool, "Pasta").await?;
        preferences::set(&pool, alice.id, tacos.id, Some("dislike")).await?;

        let suitability = suitability_for_attendees(&pool, &[alice.id]).await?;

        assert_eq!(
            suitability,
            vec![
                MealSuitability {
                    id: pasta.id,
                    name: "Pasta".to_string(),
                    liked_by_attendee: false,
                    disliked_by_attendee_names: vec![],
                },
                MealSuitability {
                    id: tacos.id,
                    name: "Tacos".to_string(),
                    liked_by_attendee: false,
                    disliked_by_attendee_names: vec!["Alice".to_string()],
                },
            ],
            "disliked meal must still be present, just flagged"
        );

        Ok(())
    }
}
