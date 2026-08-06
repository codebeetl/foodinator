use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq)]
pub struct Consumer {
    pub id: i64,
    pub name: String,
    pub active: bool,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn list_all(pool: &PgPool) -> sqlx::Result<Vec<Consumer>> {
    sqlx::query_as!(
        Consumer,
        r#"SELECT id, name, active, is_default, created_at, updated_at FROM consumers
         ORDER BY name COLLATE "C""#
    )
    .fetch_all(pool)
    .await
}

pub async fn insert(pool: &PgPool, name: &str) -> sqlx::Result<Consumer> {
    sqlx::query_as!(
        Consumer,
        "INSERT INTO consumers (name) VALUES ($1) \
         RETURNING id, name, active, is_default, created_at, updated_at",
        name
    )
    .fetch_one(pool)
    .await
}

pub async fn set_default(pool: &PgPool, id: i64, is_default: bool) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE consumers SET is_default = $1, updated_at = now() WHERE id = $2",
        is_default,
        id
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "./migrations")]
    async fn insert_then_list_all_returns_the_new_consumer(pool: PgPool) -> sqlx::Result<()> {
        let created = insert(&pool, "Alice").await?;
        assert_eq!(created.name, "Alice");
        assert!(created.active);
        assert!(
            !created.is_default,
            "new consumers should not default to attending"
        );

        let all = list_all(&pool).await?;
        assert_eq!(all, vec![created]);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn set_default_toggles_the_flag(pool: PgPool) -> sqlx::Result<()> {
        let alice = insert(&pool, "Alice").await?;

        set_default(&pool, alice.id, true).await?;
        let all = list_all(&pool).await?;
        assert!(all[0].is_default);

        set_default(&pool, alice.id, false).await?;
        let all = list_all(&pool).await?;
        assert!(!all[0].is_default);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_all_returns_consumers_ordered_alphabetically(pool: PgPool) -> sqlx::Result<()> {
        insert(&pool, "Bob").await?;
        insert(&pool, "Alice").await?;

        let all = list_all(&pool).await?;

        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "Alice");
        assert_eq!(all[1].name, "Bob");

        Ok(())
    }
}
