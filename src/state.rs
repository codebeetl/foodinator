use std::sync::Arc;

use sqlx::PgPool;

use crate::ha::CalendarSync;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub ha_client: Arc<dyn CalendarSync>,
    pub admin_username: String,
    pub admin_password: String,
}

#[cfg(test)]
pub fn test_app_state(pool: PgPool) -> AppState {
    use crate::ha::test_support::NoopCalendarSync;
    AppState {
        pool,
        ha_client: Arc::new(NoopCalendarSync),
        admin_username: "admin".to_string(),
        admin_password: "hunter2".to_string(),
    }
}
