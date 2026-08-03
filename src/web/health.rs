use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn readyz(State(state): State<AppState>) -> StatusCode {
    match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    fn state_with_lazy_pool() -> AppState {
        // connect_lazy never touches the network, so this is safe without a real database -
        // fine for routes like /healthz that never read `state.pool`.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/db")
            .expect("valid connection string");
        AppState { pool }
    }

    #[tokio::test]
    async fn healthz_returns_200_without_touching_the_database() {
        let app = router().with_state(state_with_lazy_pool());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn readyz_returns_503_when_database_is_unreachable() {
        // Loopback port 1 is never listening, so this exercises the real failure path
        // without depending on any external service being up or down. A short
        // acquire_timeout keeps this bounded even if the sandbox drops rather than
        // rejects connections to unused ports.
        let pool = PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_lazy("postgres://user:pass@127.0.0.1:1/db")
            .expect("valid connection string");
        let app = router().with_state(AppState { pool });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn readyz_returns_200_when_database_is_reachable() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping: DATABASE_URL not set (no live Postgres for this test run)");
            return;
        };
        let pool = PgPoolOptions::new()
            .connect(&database_url)
            .await
            .expect("DATABASE_URL should be reachable when this test is not skipped");
        let app = router().with_state(AppState { pool });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
