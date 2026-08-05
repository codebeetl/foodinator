use askama::Template;
use axum::extract::{Form, State};
use axum::response::Redirect;
use axum::routing::get;
use axum::Router;
use chrono::NaiveTime;
use serde::Deserialize;

use crate::db::settings::{self, AppSettings};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/settings", get(show).post(update))
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    settings: AppSettings,
}

async fn show(State(state): State<AppState>) -> SettingsTemplate {
    let settings = settings::get(&state.pool)
        .await
        .expect("failed to fetch settings");
    SettingsTemplate { settings }
}

#[derive(Deserialize)]
struct UpdateSettingsForm {
    default_start_time: NaiveTime,
    default_duration_minutes: i32,
}

async fn update(State(state): State<AppState>, Form(form): Form<UpdateSettingsForm>) -> Redirect {
    settings::update(
        &state.pool,
        form.default_start_time,
        form.default_duration_minutes,
    )
    .await
    .expect("failed to update settings");
    Redirect::to("/settings")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    #[sqlx::test(migrations = "./migrations")]
    async fn updating_settings_through_the_form_persists_and_redirects(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post("/settings")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "default_start_time=19%3A00&default_duration_minutes=45",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let updated = settings::get(&pool).await?;
        assert_eq!(
            updated.default_start_time,
            NaiveTime::from_hms_opt(19, 0, 0).unwrap()
        );
        assert_eq!(updated.default_duration_minutes, 45);

        Ok(())
    }
}
