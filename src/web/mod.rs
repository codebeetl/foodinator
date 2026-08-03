mod admin;
mod auth;
mod consumers;
mod health;

use axum::middleware;
use axum::Router;
use tower_http::services::ServeDir;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .merge(consumers::router())
        .merge(admin::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_basic_auth,
        ));

    Router::new()
        .merge(health::router())
        .merge(protected)
        .with_state(state)
        .nest_service("/static", ServeDir::new("static"))
}
