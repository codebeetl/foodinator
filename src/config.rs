use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
}

#[derive(Debug, thiserror::Error)]
#[error("missing required environment variable: {0}")]
pub struct MissingEnvVar(pub String);

impl Config {
    pub fn from_env() -> Result<Self, MissingEnvVar> {
        Ok(Config {
            database_url: required("DATABASE_URL")?,
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
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

    #[test]
    fn from_env_reads_required_vars_and_defaults_bind_addr() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _bind_addr = EnvVarGuard::unset("BIND_ADDR");
        let _database_url = EnvVarGuard::set("DATABASE_URL", "postgres://u:p@localhost/db");

        let config = Config::from_env().expect("all required vars are set");

        assert_eq!(config.database_url, "postgres://u:p@localhost/db");
        assert_eq!(config.bind_addr, "0.0.0.0:8080");
    }

    #[test]
    fn from_env_errors_on_missing_required_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _database_url = EnvVarGuard::unset("DATABASE_URL");

        let err = Config::from_env().expect_err("DATABASE_URL is missing");

        assert_eq!(err.0, "DATABASE_URL");
    }
}
