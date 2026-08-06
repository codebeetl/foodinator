use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use crate::state::AppState;

pub async fn require_basic_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if credentials_match(&state, request.headers().get(header::AUTHORIZATION)) {
        return next.run(request).await;
    }

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, r#"Basic realm="ha-foodinator""#)
        .body(axum::body::Body::empty())
        .expect("static response is well-formed")
}

fn credentials_match(state: &AppState, header: Option<&axum::http::HeaderValue>) -> bool {
    let Some(header) = header.and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let Some(encoded) = header.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = BASE64.decode(encoded) else {
        return false;
    };
    let Ok(decoded) = String::from_utf8(decoded) else {
        return false;
    };
    let Some((user, pass)) = decoded.split_once(':') else {
        return false;
    };
    user == state.admin_username && pass == state.admin_password
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use axum::Router;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/db")
            .expect("valid connection string");
        crate::state::test_app_state(pool)
    }

    fn protected_app() -> Router {
        let state = test_state();
        Router::new()
            .route("/protected", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                require_basic_auth,
            ))
            .with_state(state)
    }

    #[tokio::test]
    async fn rejects_requests_without_credentials() {
        let response = protected_app()
            .oneshot(HttpRequest::get("/protected").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_wrong_credentials() {
        let encoded = BASE64.encode("admin:wrong-password");
        let response = protected_app()
            .oneshot(
                HttpRequest::get("/protected")
                    .header(header::AUTHORIZATION, format!("Basic {encoded}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_correct_credentials() {
        let encoded = BASE64.encode("admin:hunter2");
        let response = protected_app()
            .oneshot(
                HttpRequest::get("/protected")
                    .header(header::AUTHORIZATION, format!("Basic {encoded}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
