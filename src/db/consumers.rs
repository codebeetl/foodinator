use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq)]
pub struct Consumer {
    pub id: i64,
    pub name: String,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn list_all(pool: &PgPool) -> sqlx::Result<Vec<Consumer>> {
    sqlx::query_as!(
        Consumer,
        "SELECT id, name, active, created_at, updated_at FROM consumers ORDER BY id"
    )
    .fetch_all(pool)
    .await
}

pub async fn insert(pool: &PgPool, name: &str) -> sqlx::Result<Consumer> {
    sqlx::query_as!(
        Consumer,
        "INSERT INTO consumers (name) VALUES ($1) \
         RETURNING id, name, active, created_at, updated_at",
        name
    )
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "./migrations")]
    async fn insert_then_list_all_returns_the_new_consumer(pool: PgPool) -> sqlx::Result<()> {
        let created = insert(&pool, "Alice").await?;
        assert_eq!(created.name, "Alice");
        assert!(created.active);

        let all = list_all(&pool).await?;
        assert_eq!(all, vec![created]);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_all_returns_consumers_ordered_by_id(pool: PgPool) -> sqlx::Result<()> {
        insert(&pool, "Bob").await?;
        insert(&pool, "Alice").await?;

        let all = list_all(&pool).await?;

        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "Bob");
        assert_eq!(all[1].name, "Alice");

        Ok(())
    }
}
