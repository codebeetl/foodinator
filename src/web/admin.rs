use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/ha-check", get(ha_check))
        .route("/admin/test-event", post(create_test_event))
}

async fn ha_check(State(state): State<AppState>) -> StatusCode {
    match state.ha_client.get_api_status().await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Pushes a single, manually-specified event to HA. This is a connectivity smoke test
/// for the create_event call, not the meal-plan sync job (which doesn't exist yet -
/// see docs/ARCHITECTURE.md).
#[derive(Deserialize)]
struct TestEventForm {
    summary: String,
    description: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

async fn create_test_event(
    State(state): State<AppState>,
    Form(form): Form<TestEventForm>,
) -> StatusCode {
    match state
        .ha_client
        .create_event(&form.summary, &form.description, form.start, form.end)
        .await
    {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::BAD_GATEWAY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/db")
            .expect("valid connection string");
        crate::state::test_app_state(pool)
    }

    #[tokio::test]
    async fn ha_check_returns_200_when_ha_client_reports_ok() {
        let app = router().with_state(test_state());

        let response = app
            .oneshot(Request::get("/admin/ha-check").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_event_posts_form_and_returns_200_on_success() {
        let app = router().with_state(test_state());

        let response = app
            .oneshot(
                Request::post("/admin/test-event")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "summary=Alice%27s+dinner&description=foodinator%3Aentry%3D1&\
                         start=2026-08-10T18%3A00%3A00Z&end=2026-08-10T19%3A00%3A00Z",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
