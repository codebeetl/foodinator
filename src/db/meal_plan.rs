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
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A meal alongside whether any of a given set of attendees dislikes it.
/// Never excludes - the picker UI flags disliked meals rather than hiding
/// them, since a hard exclusion could leave the list empty.
#[derive(Debug, Clone, PartialEq)]
pub struct MealSuitability {
    pub id: i64,
    pub name: String,
    pub disliked_by_attendee: bool,
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
) -> sqlx::Result<MealPlanEntry> {
    sqlx::query_as!(
        MealPlanEntry,
        "INSERT INTO meal_plan_entries \
           (entry_date, meal_id, notes, start_time_override, duration_minutes_override) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (entry_date) DO UPDATE SET \
           meal_id = EXCLUDED.meal_id, \
           notes = EXCLUDED.notes, \
           start_time_override = EXCLUDED.start_time_override, \
           duration_minutes_override = EXCLUDED.duration_minutes_override, \
           deleted_at = NULL, \
           updated_at = now() \
         RETURNING id, entry_date, meal_id, notes, start_time_override, \
           duration_minutes_override, deleted_at, created_at, updated_at",
        entry_date,
        meal_id,
        notes,
        start_time_override,
        duration_minutes_override
    )
    .fetch_one(pool)
    .await
}

pub async fn get_by_date(
    pool: &PgPool,
    entry_date: NaiveDate,
) -> sqlx::Result<Option<MealPlanEntry>> {
    sqlx::query_as!(
        MealPlanEntry,
        "SELECT id, entry_date, meal_id, notes, start_time_override, \
           duration_minutes_override, deleted_at, created_at, updated_at \
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
/// with whether any of those attendees dislikes it.
pub async fn suitability_for_attendees(
    pool: &PgPool,
    attendee_ids: &[i64],
) -> sqlx::Result<Vec<MealSuitability>> {
    sqlx::query_as!(
        MealSuitability,
        r#"SELECT m.id, m.name, EXISTS (
             SELECT 1 FROM consumer_meal_preferences p
             WHERE p.meal_id = m.id
               AND p.preference = 'dislike'
               AND p.consumer_id = ANY($1::bigint[])
           ) AS "disliked_by_attendee!"
         FROM meals m
         WHERE m.active
         ORDER BY m.name"#,
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

        let first = upsert_entry(&pool, date, tacos.id, Some("first"), None, None).await?;
        assert_eq!(first.meal_id, tacos.id);

        let second = upsert_entry(&pool, date, pasta.id, Some("second"), None, Some(45)).await?;
        assert_eq!(second.id, first.id, "same date should update the same row");
        assert_eq!(second.meal_id, pasta.id);
        assert_eq!(second.notes.as_deref(), Some("second"));
        assert_eq!(second.duration_minutes_override, Some(45));

        let fetched = get_by_date(&pool, date).await?.expect("entry should exist");
        assert_eq!(fetched, second);

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
                    disliked_by_attendee: false,
                },
                MealSuitability {
                    id: tacos.id,
                    name: "Tacos".to_string(),
                    disliked_by_attendee: true,
                },
            ],
            "disliked meal must still be present, just flagged"
        );

        Ok(())
    }
}
