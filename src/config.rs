use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    // Optional: the Home Assistant integration is disabled unless all three
    // are present (env-var default, overridable per-field from Settings).
    pub ha_url: Option<String>,
    pub ha_token: Option<String>,
    pub ha_calendar_entity_id: Option<String>,
    // Optional: Google Calendar OAuth2 credentials (env-var default,
    // overridable per-field from Settings). Calendar ID is set via the
    // UI picker after OAuth consent.
    pub gcal_client_id: Option<String>,
    pub gcal_client_secret: Option<String>,
    pub admin_username: String,
    pub admin_password: String,
    pub app_tz: String,
    // Optional: gates the unauthenticated /display kiosk route. Absent means
    // the route is unavailable, same pattern as the HA integration.
    pub display_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("missing required environment variable: {0}")]
pub struct MissingEnvVar(pub String);

impl Config {
    pub fn from_env() -> Result<Self, MissingEnvVar> {
        Ok(Config {
            database_url: required("DATABASE_URL")?,
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
            ha_url: env::var("HA_URL").ok(),
            ha_token: env::var("HA_TOKEN").ok(),
            ha_calendar_entity_id: env::var("HA_CALENDAR_ENTITY_ID").ok(),
            gcal_client_id: env::var("GCAL_CLIENT_ID").ok(),
            gcal_client_secret: env::var("GCAL_CLIENT_SECRET").ok(),
            admin_username: required("ADMIN_USERNAME")?,
            admin_password: required("ADMIN_PASSWORD")?,
            app_tz: required("APP_TZ")?,
            display_token: env::var("DISPLAY_TOKEN").ok(),
        })
    }
}

fn required(key: &str) -> Result<String, MissingEnvVar> {
    env::var(key).map_err(|_| MissingEnvVar(key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // env::set_var mutates process-global state shared with every other test in this
    // binary (e.g. web::health's tests read DATABASE_URL too), so each test here must
    // save and restore the vars it touches - not just serialize access to them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = env::var(key).ok();
            env::set_var(key, value);
            EnvVarGuard { key, original }
        }

        fn unset(key: &'static str) -> Self {
            let original = env::var(key).ok();
            env::remove_var(key);
            EnvVarGuard { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => env::set_var(self.key, value),
                None => env::remove_var(self.key),
            }
        }
    }

    fn set_all_required() -> Vec<EnvVarGuard> {
        vec![
            EnvVarGuard::set("DATABASE_URL", "postgres://u:p@localhost/db"),
            EnvVarGuard::set("HA_URL", "http://homeassistant.local:8123"),
            EnvVarGuard::set("HA_TOKEN", "test-token"),
            EnvVarGuard::set("HA_CALENDAR_ENTITY_ID", "calendar.foodinator"),
            EnvVarGuard::set("ADMIN_USERNAME", "admin"),
            EnvVarGuard::set("ADMIN_PASSWORD", "hunter2"),
            EnvVarGuard::set("APP_TZ", "Australia/Sydney"),
        ]
    }

    #[test]
    fn from_env_reads_required_vars_and_defaults_bind_addr() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _bind_addr = EnvVarGuard::unset("BIND_ADDR");
        let _guards = set_all_required();

        let config = Config::from_env().expect("all required vars are set");

        assert_eq!(config.database_url, "postgres://u:p@localhost/db");
        assert_eq!(
            config.ha_calendar_entity_id.as_deref(),
            Some("calendar.foodinator")
        );
        assert_eq!(config.bind_addr, "0.0.0.0:8080");
        assert_eq!(config.app_tz, "Australia/Sydney");
    }

    #[test]
    fn from_env_errors_on_missing_required_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guards = set_all_required();
        let _admin_username = EnvVarGuard::unset("ADMIN_USERNAME");

        let err = Config::from_env().expect_err("ADMIN_USERNAME is missing");

        assert_eq!(err.0, "ADMIN_USERNAME");
    }

    #[test]
    fn from_env_leaves_ha_fields_unset_when_env_vars_are_absent() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guards = set_all_required();
        let _ha_url = EnvVarGuard::unset("HA_URL");
        let _ha_token = EnvVarGuard::unset("HA_TOKEN");
        let _ha_entity = EnvVarGuard::unset("HA_CALENDAR_ENTITY_ID");

        let config = Config::from_env().expect("HA vars are optional");

        assert_eq!(config.ha_url, None);
        assert_eq!(config.ha_token, None);
        assert_eq!(config.ha_calendar_entity_id, None);
    }

    #[test]
    fn from_env_leaves_display_token_unset_when_env_var_is_absent() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guards = set_all_required();
        let _display_token = EnvVarGuard::unset("DISPLAY_TOKEN");

        let config = Config::from_env().expect("DISPLAY_TOKEN is optional");

        assert_eq!(config.display_token, None);
    }

    #[test]
    fn from_env_reads_display_token_when_present() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guards = set_all_required();
        let _display_token = EnvVarGuard::set("DISPLAY_TOKEN", "kiosk-secret");

        let config = Config::from_env().expect("all required vars are set");

        assert_eq!(config.display_token.as_deref(), Some("kiosk-secret"));
    }

    #[test]
    fn from_env_errors_on_missing_app_tz() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guards = set_all_required();
        let _app_tz = EnvVarGuard::unset("APP_TZ");

        let err = Config::from_env().expect_err("APP_TZ is missing");

        assert_eq!(err.0, "APP_TZ");
    }

    #[test]
    fn from_env_leaves_gcal_fields_unset_when_env_vars_are_absent() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guards = set_all_required();
        let _gcal_id = EnvVarGuard::unset("GCAL_CLIENT_ID");
        let _gcal_secret = EnvVarGuard::unset("GCAL_CLIENT_SECRET");

        let config = Config::from_env().expect("GCal vars are optional");

        assert_eq!(config.gcal_client_id, None);
        assert_eq!(config.gcal_client_secret, None);
    }

    #[test]
    fn from_env_reads_gcal_fields_when_present() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guards = set_all_required();
        let _gcal_id = EnvVarGuard::set("GCAL_CLIENT_ID", "my-client-id");
        let _gcal_secret = EnvVarGuard::set("GCAL_CLIENT_SECRET", "my-secret");

        let config = Config::from_env().expect("all vars present");

        assert_eq!(config.gcal_client_id.as_deref(), Some("my-client-id"));
        assert_eq!(config.gcal_client_secret.as_deref(), Some("my-secret"));
    }
}
