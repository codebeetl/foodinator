use chrono::{DateTime, NaiveTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq)]
pub struct AppSettings {
    pub default_start_time: NaiveTime,
    pub default_duration_minutes: i32,
    // NULL means "fall back to the HA_URL/HA_TOKEN/HA_CALENDAR_ENTITY_ID env
    // vars" - see resolve_ha_config.
    pub ha_url: Option<String>,
    pub ha_token: Option<String>,
    pub ha_calendar_entity_id: Option<String>,
    // 0=Monday .. 6=Sunday, matching chrono's Weekday::num_days_from_monday().
    pub week_start_weekday: i16,
    // "light", "dark", or "auto" - enforced by a CHECK constraint in the DB.
    pub theme: String,
    pub updated_at: DateTime<Utc>,
}

/// The fully-resolved set of HA connection details, once both DB overrides
/// and env-var fallbacks have been merged.
#[derive(Debug, Clone, PartialEq)]
pub struct HaConfig {
    pub url: String,
    pub token: String,
    pub calendar_entity_id: String,
}

/// Merges per-field DB overrides over env-var fallbacks. HA is only
/// considered configured if every field resolves to something - a partially
/// configured integration is treated the same as an unconfigured one.
pub fn resolve_ha_config(
    settings: &AppSettings,
    env_url: Option<&str>,
    env_token: Option<&str>,
    env_calendar_entity_id: Option<&str>,
) -> Option<HaConfig> {
    let url = settings
        .ha_url
        .clone()
        .or_else(|| env_url.map(str::to_string))?;
    let token = settings
        .ha_token
        .clone()
        .or_else(|| env_token.map(str::to_string))?;
    let calendar_entity_id = settings
        .ha_calendar_entity_id
        .clone()
        .or_else(|| env_calendar_entity_id.map(str::to_string))?;
    Some(HaConfig {
        url,
        token,
        calendar_entity_id,
    })
}

pub async fn get(pool: &PgPool) -> sqlx::Result<AppSettings> {
    sqlx::query_as!(
        AppSettings,
        "SELECT default_start_time, default_duration_minutes, \
           ha_url, ha_token, ha_calendar_entity_id, week_start_weekday, theme, updated_at \
         FROM app_settings WHERE id = 1"
    )
    .fetch_one(pool)
    .await
}

pub async fn update(
    pool: &PgPool,
    default_start_time: NaiveTime,
    default_duration_minutes: i32,
    week_start_weekday: i16,
    theme: &str,
) -> sqlx::Result<AppSettings> {
    sqlx::query_as!(
        AppSettings,
        "UPDATE app_settings SET default_start_time = $1, default_duration_minutes = $2, \
         week_start_weekday = $3, theme = $4, updated_at = now() WHERE id = 1 \
         RETURNING default_start_time, default_duration_minutes, \
           ha_url, ha_token, ha_calendar_entity_id, week_start_weekday, theme, updated_at",
        default_start_time,
        default_duration_minutes,
        week_start_weekday,
        theme
    )
    .fetch_one(pool)
    .await
}

/// Updates the HA override fields. `ha_url`/`ha_calendar_entity_id` are set
/// verbatim (None clears the override back to the env-var fallback), and
/// since HA only works with every field resolved (see `resolve_ha_config`),
/// blanking either one is already enough to disable the integration.
/// `ha_token` stays a special case: the settings form never echoes the real
/// token back (it's a password field), so a blank submission there means
/// "leave whatever's stored untouched," not "clear it" - there's no way to
/// distinguish "user left it blank on purpose" from "field just renders
/// blank every time" otherwise, and disabling doesn't require clearing it
/// anyway since blanking the URL or calendar entity ID already does that.
pub async fn update_ha(
    pool: &PgPool,
    ha_url: Option<&str>,
    ha_token: Option<&str>,
    ha_calendar_entity_id: Option<&str>,
) -> sqlx::Result<AppSettings> {
    sqlx::query_as!(
        AppSettings,
        "UPDATE app_settings SET \
           ha_url = $1, \
           ha_token = COALESCE($2, ha_token), \
           ha_calendar_entity_id = $3, \
           updated_at = now() \
         WHERE id = 1 \
         RETURNING default_start_time, default_duration_minutes, \
           ha_url, ha_token, ha_calendar_entity_id, week_start_weekday, theme, updated_at",
        ha_url,
        ha_token,
        ha_calendar_entity_id
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

        let updated = update(&pool, new_start, 45, 2, "dark").await?;
        assert_eq!(updated.default_start_time, new_start);
        assert_eq!(updated.default_duration_minutes, 45);
        assert_eq!(updated.week_start_weekday, 2);
        assert_eq!(updated.theme, "dark");

        let fetched = get(&pool).await?;
        assert_eq!(fetched, updated);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn settings_default_to_auto_theme(pool: PgPool) -> sqlx::Result<()> {
        let settings = get(&pool).await?;
        assert_eq!(settings.theme, "auto");
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn an_invalid_theme_is_rejected_by_the_database_check_constraint(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let start = NaiveTime::from_hms_opt(18, 30, 0).unwrap();
        let result = update(&pool, start, 30, 5, "purple").await;
        assert!(
            result.is_err(),
            "a theme outside light/dark/auto should be rejected"
        );
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn settings_default_to_saturday_as_the_week_start(pool: PgPool) -> sqlx::Result<()> {
        let settings = get(&pool).await?;
        assert_eq!(settings.week_start_weekday, 5);
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn settings_start_with_no_ha_override(pool: PgPool) -> sqlx::Result<()> {
        let settings = get(&pool).await?;

        assert_eq!(settings.ha_url, None);
        assert_eq!(settings.ha_token, None);
        assert_eq!(settings.ha_calendar_entity_id, None);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn update_ha_sets_url_and_entity_id_verbatim_but_keeps_token_when_blank(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let first = update_ha(
            &pool,
            Some("http://homeassistant.local:8123"),
            Some("secret-token"),
            Some("calendar.foodinator"),
        )
        .await?;
        assert_eq!(
            first.ha_url.as_deref(),
            Some("http://homeassistant.local:8123")
        );
        assert_eq!(first.ha_token.as_deref(), Some("secret-token"));

        let second = update_ha(
            &pool,
            Some("http://homeassistant.local:8124"),
            None,
            Some("calendar.foodinator"),
        )
        .await?;
        assert_eq!(
            second.ha_url.as_deref(),
            Some("http://homeassistant.local:8124"),
            "url should update"
        );
        assert_eq!(
            second.ha_token.as_deref(),
            Some("secret-token"),
            "blank token submission should leave the stored token untouched"
        );

        let cleared = update_ha(&pool, None, None, Some("calendar.foodinator")).await?;
        assert_eq!(cleared.ha_url, None, "None should clear the url override");
        assert_eq!(
            cleared.ha_token.as_deref(),
            Some("secret-token"),
            "token still can't be cleared by a blank submission - blanking url already disables"
        );

        Ok(())
    }

    #[test]
    fn resolve_ha_config_requires_every_field_to_resolve() {
        let base = AppSettings {
            default_start_time: NaiveTime::from_hms_opt(18, 30, 0).unwrap(),
            default_duration_minutes: 30,
            ha_url: None,
            ha_token: None,
            ha_calendar_entity_id: None,
            week_start_weekday: 5,
            theme: "auto".to_string(),
            updated_at: Utc::now(),
        };

        assert_eq!(resolve_ha_config(&base, None, None, None), None);
        assert_eq!(
            resolve_ha_config(&base, Some("http://ha.local"), None, Some("calendar.x")),
            None,
            "a partially configured integration should not resolve"
        );

        let fully_from_env = resolve_ha_config(
            &base,
            Some("http://ha.local"),
            Some("env-token"),
            Some("calendar.x"),
        );
        assert_eq!(
            fully_from_env,
            Some(HaConfig {
                url: "http://ha.local".to_string(),
                token: "env-token".to_string(),
                calendar_entity_id: "calendar.x".to_string(),
            })
        );

        let overridden = AppSettings {
            ha_url: Some("http://ha-override.local".to_string()),
            ..base
        };
        let resolved = resolve_ha_config(
            &overridden,
            Some("http://ha.local"),
            Some("env-token"),
            Some("calendar.x"),
        )
        .unwrap();
        assert_eq!(
            resolved.url, "http://ha-override.local",
            "DB override should win over the env default"
        );
    }
}
