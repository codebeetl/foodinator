use std::sync::Arc;

use chrono_tz::Tz;
use sqlx::PgPool;

use crate::db::settings::{self, AppSettings, HaConfig};
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
    // Env-var fallbacks for GCal config, overridden per-field by app_settings.
    pub gcal_env_client_id: Option<String>,
    pub gcal_env_client_secret: Option<String>,
    // Override for the OAuth redirect URI (e.g. tunnel URL).
    pub gcal_redirect_uri: Option<String>,
    // Override for the OAuth token endpoint - None uses Google's real one;
    // tests point this at a wiremock server.
    pub gcal_token_url: Option<String>,
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

    /// The nav-chrome context shared by every page (theme + which optional
    /// features are configured). Reads app_settings fresh every call, same
    /// rationale as `ha_client` - a Settings-page edit should take effect on
    /// the very next request. Handlers that already fetched app_settings for
    /// their own logic should use `PageContext::from_state` instead of this,
    /// to avoid a second settings read.
    pub async fn page_context(&self) -> PageContext {
        let app_settings = settings::get(&self.pool)
            .await
            .expect("failed to load settings");
        PageContext::from_state(self, &app_settings)
    }
}

/// The fields every page template needs from app_settings (its theme, and
/// whether the optional HA/display features are configured), collected in one
/// place so the six page handlers don't each re-derive them. base.html reads
/// them off this struct via `ctx.*`.
#[derive(Debug, Clone)]
pub struct PageContext {
    pub ha_configured: bool,
    pub display_configured: bool,
    pub theme: String,
}

impl PageContext {
    /// Builds the context from already-loaded settings. `ha_configured` is
    /// derived from `resolve_ha_config` directly (not by building a client),
    /// since "is HA configured" is exactly "does the config resolve" - same
    /// result, no client allocation and no settings round-trip.
    pub fn from_state(state: &AppState, settings: &AppSettings) -> Self {
        PageContext {
            ha_configured: settings::resolve_ha_config(
                settings,
                state.ha_env_url.as_deref(),
                state.ha_env_token.as_deref(),
                state.ha_env_calendar_entity_id.as_deref(),
            )
            .is_some(),
            display_configured: state.display_token.is_some(),
            theme: settings.theme.clone(),
        }
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
        gcal_env_client_id: Some("test-gcal-client-id".to_string()),
        gcal_env_client_secret: Some("test-gcal-secret".to_string()),
        gcal_redirect_uri: None,
        gcal_token_url: None,
        admin_username: "admin".to_string(),
        admin_password: "hunter2".to_string(),
        household_tz: chrono_tz::UTC,
        display_token: None,
    }
}
