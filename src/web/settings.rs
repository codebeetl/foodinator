use askama::Template;
use axum::extract::{Form, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use chrono::NaiveTime;
use serde::Deserialize;

use crate::db::settings::{self, AppSettings};
use crate::state::{AppState, PageContext};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn router() -> Router<AppState> {
    Router::new().route("/settings", get(show).post(update))
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    settings: AppSettings,
    ctx: PageContext,
    app_version: &'static str,
}

impl IntoResponse for SettingsTemplate {
    fn into_response(self) -> Response {
        super::render_askama_template(self)
    }
}

async fn show(State(state): State<AppState>) -> SettingsTemplate {
    let settings = settings::get(&state.pool)
        .await
        .expect("failed to fetch settings");
    let ctx = PageContext::from_state(&state, &settings);
    SettingsTemplate {
        settings,
        ctx,
        app_version: APP_VERSION,
    }
}

#[derive(Deserialize)]
struct UpdateSettingsForm {
    default_start_time: NaiveTime,
    default_duration_minutes: i32,
    week_start_weekday: i16,
    theme: String,
}

async fn update(State(state): State<AppState>, Form(form): Form<UpdateSettingsForm>) -> Redirect {
    settings::update(
        &state.pool,
        form.default_start_time,
        form.default_duration_minutes,
        form.week_start_weekday,
        &form.theme,
    )
    .await
    .expect("failed to update settings");
    Redirect::to("/settings")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    #[sqlx::test(migrations = "./migrations")]
    async fn show_page_displays_the_app_version(pool: PgPool) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/settings").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(&format!("v{APP_VERSION}")),
            "settings page should display the app version: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn updating_settings_through_the_form_persists_and_redirects(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post("/settings")
                    .header(
                        axum::http::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from(
                        "default_start_time=19%3A00&default_duration_minutes=45&\
                         week_start_weekday=5&theme=dark",
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
        assert_eq!(updated.theme, "dark");

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn settings_page_reflects_the_saved_theme_in_the_root_data_attribute(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        settings::update(
            &pool,
            NaiveTime::from_hms_opt(18, 30, 0).unwrap(),
            30,
            5,
            "dark",
        )
        .await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/settings").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(r#"data-theme="dark""#),
            "the root element should carry the saved theme: {html}"
        );

        Ok(())
    }
}
