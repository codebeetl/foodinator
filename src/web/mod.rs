mod admin;
mod auth;
mod consumers;
mod display;
mod health;
mod meals;
mod plan;
mod settings;
mod sync;

use axum::http::StatusCode;
use axum::middleware;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::services::{ServeDir, ServeFile};

use crate::state::AppState;

async fn root() -> Redirect {
    Redirect::to("/plan")
}

/// askama_axum is deprecated and gone as of askama 0.13 - this is the
/// officially recommended replacement (render, then build the response by
/// hand), shared so every Template struct's `impl IntoResponse` is a
/// one-line call instead of repeating the render/error-handling match.
pub(crate) fn render_askama_template<T: askama::Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

/// A plain HTML form POST never sends this header - only script-driven
/// fetch() calls do - so this distinguishes an AJAX request wanting a
/// re-rendered fragment back from a real navigation wanting a redirect.
pub(crate) fn is_ajax_request(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("X-Requested-With")
        .is_some_and(|v| v == "XMLHttpRequest")
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
        // Without this, a handler panic (e.g. one of the many .expect()s on
        // "should never happen" DB/infra failures) unwinds the connection's
        // Tokio task and the client sees a dropped connection rather than a
        // clean 500 - the server itself is fine either way since axum/hyper
        // isolate each connection to its own task, but this makes a real bug
        // fail visibly instead of looking like a network hiccup.
        .layer(CatchPanicLayer::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
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

    // Regression test for the CatchPanicLayer: a handler that panics (here,
    // meals::list's `.expect("failed to list meals")` against an unreachable
    // pool) should surface as a clean 500, not unwind the connection. Without
    // the layer, this test would fail its own process rather than return a
    // response at all.
    #[tokio::test]
    async fn a_handler_panic_is_converted_into_a_500_instead_of_crashing_the_connection() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/db")
            .expect("valid connection string");
        let app = router(crate::state::test_app_state(pool));
        let encoded = BASE64.encode("admin:hunter2");

        let response = app
            .oneshot(
                Request::get("/meals")
                    .header(header::AUTHORIZATION, format!("Basic {encoded}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
