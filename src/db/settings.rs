use chrono::{DateTime, NaiveTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq)]
pub struct AppSettings {
    pub default_start_time: NaiveTime,
    pub default_duration_minutes: i32,
    pub updated_at: DateTime<Utc>,
}

pub async fn get(pool: &PgPool) -> sqlx::Result<AppSettings> {
    sqlx::query_as!(
        AppSettings,
        "SELECT default_start_time, default_duration_minutes, updated_at \
         FROM app_settings WHERE id = 1"
    )
    .fetch_one(pool)
    .await
}

pub async fn update(
    pool: &PgPool,
    default_start_time: NaiveTime,
    default_duration_minutes: i32,
) -> sqlx::Result<AppSettings> {
    sqlx::query_as!(
        AppSettings,
        "UPDATE app_settings SET default_start_time = $1, default_duration_minutes = $2, \
         updated_at = now() WHERE id = 1 \
         RETURNING default_start_time, default_duration_minutes, updated_at",
        default_start_time,
        default_duration_minutes
    )
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "./migrations")]
    async fn settings_start_with_the_migrations_defaults(pool: PgPool) -> sqlx::Result<()> {
        let settings = get(&pool).await?;

        assert_eq!(
            settings.default_start_time,
            NaiveTime::from_hms_opt(18, 30, 0).unwrap()
        );
        assert_eq!(settings.default_duration_minutes, 30);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn updating_settings_persists(pool: PgPool) -> sqlx::Result<()> {
        let new_start = NaiveTime::from_hms_opt(19, 0, 0).unwrap();

        let updated = update(&pool, new_start, 45).await?;
        assert_eq!(updated.default_start_time, new_start);
        assert_eq!(updated.default_duration_minutes, 45);

        let fetched = get(&pool).await?;
        assert_eq!(fetched, updated);

        Ok(())
    }
}
