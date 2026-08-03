mod config;
mod state;
mod web;

use config::Config;
use sqlx::postgres::PgPoolOptions;
use state::AppState;

#[tokio::main]
async fn main() {
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

    let state = AppState { pool };
    let app = web::router(state);

    tracing::info!(bind_addr = %config.bind_addr, "ha-foodinator starting");
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .expect("failed to bind address");
    axum::serve(listener, app).await.expect("server error");
}
