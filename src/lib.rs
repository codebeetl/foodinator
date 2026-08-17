pub mod clock;
pub mod config;
pub mod db;
pub mod gcal;
pub mod ha;
pub mod state;
pub mod web;

use std::sync::Arc;

use config::Config;
use ha::HaRestClient;
use sqlx::postgres::PgPoolOptions;
use state::AppState;

pub async fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env().expect("invalid configuration");

    let pool = PgPoolOptions::new()
        .connect(&config.database_url)
        .await
        .expect("failed to connect to database");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run database migrations");

    let household_tz: chrono_tz::Tz = config
        .app_tz
        .parse()
        .expect("APP_TZ is not a valid IANA timezone name");

    let state = AppState {
        pool,
        ha_client_factory: Arc::new(|config| {
            Arc::new(HaRestClient::new(
                config.url,
                config.token,
                config.calendar_entity_id,
            ))
        }),
        ha_env_url: config.ha_url.clone(),
        ha_env_token: config.ha_token.clone(),
        ha_env_calendar_entity_id: config.ha_calendar_entity_id.clone(),
        gcal_env_client_id: config.gcal_client_id.clone(),
        gcal_env_client_secret: config.gcal_client_secret.clone(),
        gcal_redirect_uri: config.gcal_redirect_uri.clone(),
        admin_username: config.admin_username.clone(),
        admin_password: config.admin_password.clone(),
        household_tz,
        display_token: config.display_token.clone(),
    };
    let app = web::router(state);

    tracing::info!(bind_addr = %config.bind_addr, "foodinator v{} starting", env!("CARGO_PKG_VERSION"));
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .expect("failed to bind address");
    axum::serve(listener, app).await.expect("server error");
}
