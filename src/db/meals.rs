use chrono::{DateTime, Utc};
use sqlx::PgPool;

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
}
