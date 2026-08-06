mod admin;
mod auth;
mod consumers;
mod display;
mod health;
mod meals;
mod plan;
mod settings;
mod sync;

use axum::middleware;
use axum::response::Redirect;
use axum::routing::get;
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

use crate::state::AppState;

async fn root() -> Redirect {
    Redirect::to("/plan")
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/", get(root))
        .merge(consumers::router())
        .merge(meals::router())
        .merge(plan::router())
        .merge(settings::router())
        .merge(sync::router())
        .merge(admin::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_basic_auth,
        ));

    Router::new()
        .merge(health::router())
        .merge(display::router())
        .merge(protected)
        .with_state(state)
        .nest_service("/static", ServeDir::new("static"))
        .route_service("/favicon.ico", ServeFile::new("static/favicon.ico"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    #[tokio::test]
    async fn favicon_is_served_without_basic_auth() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/db")
            .expect("valid connection string");
        let app = router(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/favicon.ico").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
