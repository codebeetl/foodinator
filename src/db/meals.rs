use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::db::meal_plan::MealSuitability;

#[derive(Debug, Clone, PartialEq)]
pub struct Meal {
    pub id: i64,
    pub name: String,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn list_all(pool: &PgPool) -> sqlx::Result<Vec<Meal>> {
    sqlx::query_as!(
        Meal,
        "SELECT id, name, active, created_at, updated_at FROM meals ORDER BY id"
    )
    .fetch_all(pool)
    .await
}

pub async fn insert(pool: &PgPool, name: &str) -> sqlx::Result<Meal> {
    sqlx::query_as!(
        Meal,
        "INSERT INTO meals (name) VALUES ($1) \
         RETURNING id, name, active, created_at, updated_at",
        name
    )
    .fetch_one(pool)
    .await
}

pub async fn get(pool: &PgPool, id: i64) -> sqlx::Result<Option<Meal>> {
    sqlx::query_as!(
        Meal,
        "SELECT id, name, active, created_at, updated_at FROM meals WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await
}

pub async fn update(pool: &PgPool, id: i64, name: &str, active: bool) -> sqlx::Result<Meal> {
    sqlx::query_as!(
        Meal,
        "UPDATE meals SET name = $2, active = $3, updated_at = now() WHERE id = $1 \
         RETURNING id, name, active, created_at, updated_at",
        id,
        name,
        active
    )
    .fetch_one(pool)
    .await
}

/// Trigram-ranked search for the plan-day meal picker: exact match first, then
/// prefix match, then similarity - tolerant of typos, unlike plain ILIKE.
/// `attendee_ids` feeds the same disliked-by-attendee flag as suitability_for_attendees.
pub async fn search(
    pool: &PgPool,
    q: &str,
    attendee_ids: &[i64],
    limit: i64,
) -> sqlx::Result<Vec<MealSuitability>> {
    sqlx::query_as!(
        MealSuitability,
        r#"SELECT m.id, m.name, EXISTS (
             SELECT 1 FROM consumer_meal_preferences p
             WHERE p.meal_id = m.id
               AND p.preference = 'dislike'
               AND p.consumer_id = ANY($2::bigint[])
           ) AS "disliked_by_attendee!"
         FROM meals m
         WHERE m.active AND (m.name ILIKE '%' || $1 || '%' OR similarity(m.name, $1) > 0.2)
         ORDER BY
           CASE
             WHEN lower(m.name) = lower($1) THEN 0
             WHEN m.name ILIKE $1 || '%' THEN 1
             ELSE 2
           END,
           similarity(m.name, $1) DESC,
           m.name COLLATE "C"
         LIMIT $3"#,
        q,
        attendee_ids,
        limit
    )
    .fetch_all(pool)
    .await
}

/// The picker's default list before anything has been typed: no ranking signal
/// to go on, so alphabetical is the only defensible order.
pub async fn list_top(
    pool: &PgPool,
    attendee_ids: &[i64],
    limit: i64,
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
         ORDER BY m.name COLLATE "C"
         LIMIT $2"#,
        attendee_ids,
        limit
    )
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "./migrations")]
    async fn insert_then_list_all_returns_the_new_meal(pool: PgPool) -> sqlx::Result<()> {
        let created = insert(&pool, "Spaghetti Bolognese").await?;
        assert_eq!(created.name, "Spaghetti Bolognese");
        assert!(created.active);

        let all = list_all(&pool).await?;
        assert_eq!(all, vec![created]);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn update_renames_and_can_deactivate_a_meal(pool: PgPool) -> sqlx::Result<()> {
        let created = insert(&pool, "Tacos").await?;

        let updated = update(&pool, created.id, "Fish Tacos", false).await?;

        assert_eq!(updated.name, "Fish Tacos");
        assert!(!updated.active);

        let fetched = get(&pool, created.id).await?.expect("meal should exist");
        assert_eq!(fetched, updated);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn search_ranks_exact_then_prefix_then_typo_tolerant_similarity(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        insert(&pool, "Tacos").await?;
        insert(&pool, "Taco Salad").await?;
        insert(&pool, "Spaghetti Bolognese").await?;

        let results = search(&pool, "Tacos", &[], 10).await?;
        assert_eq!(
            results.iter().map(|m| &m.name).collect::<Vec<_>>(),
            vec!["Tacos", "Taco Salad"],
            "exact match should rank above a mere prefix match"
        );

        let typo_results = search(&pool, "spagetti", &[], 10).await?;
        assert_eq!(
            typo_results.iter().map(|m| &m.name).collect::<Vec<_>>(),
            vec!["Spaghetti Bolognese"],
            "trigram similarity should tolerate a typo"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn search_excludes_inactive_meals_and_flags_disliked_ones(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = crate::db::consumers::insert(&pool, "Alice").await?;
        let tacos = insert(&pool, "Tacos").await?;
        crate::db::preferences::set(&pool, alice.id, tacos.id, Some("dislike")).await?;
        let retired = insert(&pool, "Tacos Deluxe").await?;
        update(&pool, retired.id, "Tacos Deluxe", false).await?;

        let results = search(&pool, "Tacos", &[alice.id], 10).await?;

        assert_eq!(results.len(), 1, "inactive meal should be excluded");
        assert_eq!(results[0].name, "Tacos");
        assert!(results[0].disliked_by_attendee);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_top_returns_active_meals_alphabetically_up_to_the_limit(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        insert(&pool, "Tacos").await?;
        insert(&pool, "Pasta").await?;
        insert(&pool, "Curry").await?;

        let results = list_top(&pool, &[], 2).await?;

        assert_eq!(
            results.iter().map(|m| &m.name).collect::<Vec<_>>(),
            vec!["Curry", "Pasta"],
            "alphabetical order, capped at the limit"
        );

        Ok(())
    }
}
