use std::sync::Arc;

use chrono_tz::Tz;
use sqlx::PgPool;

use crate::db::settings::{self, HaConfig};
use crate::ha::CalendarSync;

/// Builds a CalendarSync from a resolved HaConfig. Indirected through a
/// factory (rather than a fixed client) so the client always reflects the
/// currently-resolved config - which can change at runtime via Settings -
/// and so tests can swap in Noop/Failing implementations without needing a
/// real HA config to resolve.
pub type HaClientFactory = Arc<dyn Fn(HaConfig) -> Arc<dyn CalendarSync> + Send + Sync>;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub ha_client_factory: HaClientFactory,
    // Env-var fallbacks for HA config, overridden per-field by app_settings.
    pub ha_env_url: Option<String>,
    pub ha_env_token: Option<String>,
    pub ha_env_calendar_entity_id: Option<String>,
    pub admin_username: String,
    pub admin_password: String,
    pub household_tz: Tz,
    pub display_token: Option<String>,
}

impl AppState {
    /// Resolves the current HA config (DB override merged over env
    /// fallback) and builds a client from it - None if HA isn't configured.
    /// Reads app_settings fresh every call so a Settings-page edit takes
    /// effect on the very next request, no restart needed.
    pub async fn ha_client(&self) -> Option<Arc<dyn CalendarSync>> {
        let app_settings = settings::get(&self.pool)
            .await
            .expect("failed to load settings");
        let config = settings::resolve_ha_config(
            &app_settings,
            self.ha_env_url.as_deref(),
            self.ha_env_token.as_deref(),
            self.ha_env_calendar_entity_id.as_deref(),
        )?;
        Some((self.ha_client_factory)(config))
    }

    /// The saved theme preference ("light", "dark", or "auto"). Reads fresh
    /// every call, same rationale as `ha_client` - a Settings-page edit
    /// should take effect on the very next request.
    pub async fn theme(&self) -> String {
        settings::get(&self.pool)
            .await
            .expect("failed to load settings")
            .theme
    }
}

#[cfg(test)]
pub fn test_app_state(pool: PgPool) -> AppState {
    use crate::ha::test_support::NoopCalendarSync;
    AppState {
        pool,
        ha_client_factory: Arc::new(|_config| Arc::new(NoopCalendarSync)),
        ha_env_url: Some("http://test-ha.local".to_string()),
        ha_env_token: Some("test-token".to_string()),
        ha_env_calendar_entity_id: Some("calendar.test".to_string()),
        admin_username: "admin".to_string(),
        admin_password: "hunter2".to_string(),
        household_tz: chrono_tz::UTC,
        display_token: None,
    }
}
