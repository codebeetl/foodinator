mod consumers;
mod health;

use axum::Router;
use tower_http::services::ServeDir;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::router())
        .merge(consumers::router())
        .with_state(state)
        .nest_service("/static", ServeDir::new("static"))
}
