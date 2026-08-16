use askama::Template;
use axum::extract::{Form, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use chrono::NaiveTime;
use serde::Deserialize;

use crate::db::settings::{self, AppSettings, HaConfig};
use crate::state::{AppState, PageContext};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(show).post(update))
        .route("/settings/ha-test", axum::routing::post(test_ha_connection))
}

enum TestResult {
    Success,
    Failure(String),
    NotConfigured,
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    settings: AppSettings,
    ctx: PageContext,
    // Only set right after a Test Connection submission - the typed (not
    // necessarily saved) values are echoed back so the URL/entity-ID fields
    // don't appear to reset, without ever echoing the token itself.
    ha_url_input: String,
    ha_calendar_entity_id_input: String,
    test_result: Option<TestResult>,
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
        ha_url_input: settings.ha_url.clone().unwrap_or_default(),
        ha_calendar_entity_id_input: settings.ha_calendar_entity_id.clone().unwrap_or_default(),
        settings,
        ctx,
        test_result: None,
        app_version: APP_VERSION,
    }
}

#[derive(Deserialize)]
struct UpdateSettingsForm {
    default_start_time: NaiveTime,
    default_duration_minutes: i32,
    week_start_weekday: i16,
    theme: String,
    ha_url: String,
    ha_token: String,
    ha_calendar_entity_id: String,
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
    settings::update_ha(
        &state.pool,
        super::non_empty(&form.ha_url),
        super::non_empty(&form.ha_token),
        super::non_empty(&form.ha_calendar_entity_id),
    )
    .await
    .expect("failed to update HA settings");
    Redirect::to("/settings")
}

#[derive(Deserialize)]
struct HaTestForm {
    ha_url: String,
    ha_token: String,
    ha_calendar_entity_id: String,
}

/// Resolves what a connection test should actually try: the typed field if
/// non-blank, otherwise whatever's currently in effect (DB override or env
/// fallback) - so testing after only changing the URL still uses the real
/// stored token rather than failing for lack of one.
async fn effective_test_config(state: &AppState, form: &HaTestForm) -> Option<HaConfig> {
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to load settings");
    let current = settings::resolve_ha_config(
        &app_settings,
        state.ha_env_url.as_deref(),
        state.ha_env_token.as_deref(),
        state.ha_env_calendar_entity_id.as_deref(),
    );

    let url = super::non_empty(&form.ha_url)
        .map(str::to_string)
        .or_else(|| current.as_ref().map(|c| c.url.clone()))?;
    let token = super::non_empty(&form.ha_token)
        .map(str::to_string)
        .or_else(|| current.as_ref().map(|c| c.token.clone()))?;
    let calendar_entity_id = super::non_empty(&form.ha_calendar_entity_id)
        .map(str::to_string)
        .or_else(|| current.as_ref().map(|c| c.calendar_entity_id.clone()))?;

    Some(HaConfig {
        url,
        token,
        calendar_entity_id,
    })
}

async fn test_ha_connection(
    State(state): State<AppState>,
    Form(form): Form<HaTestForm>,
) -> SettingsTemplate {
    let config = effective_test_config(&state, &form).await;

    let test_result = Some(match &config {
        None => TestResult::NotConfigured,
        Some(config) => {
            let client = (state.ha_client_factory)(config.clone());
            match client.get_api_status().await {
                Ok(()) => TestResult::Success,
                Err(err) => TestResult::Failure(err.to_string()),
            }
        }
    });

    let settings = settings::get(&state.pool)
        .await
        .expect("failed to fetch settings");
    let ctx = PageContext::from_state(&state, &settings);
    SettingsTemplate {
        settings,
        ctx,
        ha_url_input: form.ha_url,
        ha_calendar_entity_id_input: form.ha_calendar_entity_id,
        test_result,
        app_version: APP_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
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
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "default_start_time=19%3A00&default_duration_minutes=45&\
                         week_start_weekday=5&theme=dark&\
                         ha_url=http%3A%2F%2Fha.local&ha_token=secret&\
                         ha_calendar_entity_id=calendar.foodinator",
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
        assert_eq!(updated.ha_url.as_deref(), Some("http://ha.local"));
        assert_eq!(updated.ha_token.as_deref(), Some("secret"));
        assert_eq!(
            updated.ha_calendar_entity_id.as_deref(),
            Some("calendar.foodinator")
        );

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

    #[sqlx::test(migrations = "./migrations")]
    async fn blank_ha_token_on_save_keeps_the_existing_token(pool: PgPool) -> sqlx::Result<()> {
        settings::update_ha(&pool, Some("http://ha.local"), Some("secret"), None).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        app.oneshot(
            Request::post("/settings")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "default_start_time=18%3A30&default_duration_minutes=30&\
                     week_start_weekday=5&theme=auto&\
                     ha_url=http%3A%2F%2Fha.local&ha_token=&ha_calendar_entity_id=",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

        let updated = settings::get(&pool).await?;
        assert_eq!(
            updated.ha_token.as_deref(),
            Some("secret"),
            "blank token submission must not clear the stored token"
        );
        assert_eq!(
            updated.ha_calendar_entity_id, None,
            "blank url/entity-id fields do clear their override, which already disables HA"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_connection_reports_not_configured_with_no_fields_set(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.ha_env_url = None;
        state.ha_env_token = None;
        state.ha_env_calendar_entity_id = None;
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::post("/settings/ha-test")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("ha_url=&ha_token=&ha_calendar_entity_id="))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("not configured") || html.contains("Not configured"),
            "should report the not-configured state: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_connection_uses_the_stored_token_when_the_field_is_left_blank(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        settings::update_ha(
            &pool,
            Some("http://ha.local"),
            Some("stored-token"),
            Some("calendar.foodinator"),
        )
        .await?;
        let mut state = crate::state::test_app_state(pool);
        state.ha_env_url = None;
        state.ha_env_token = None;
        state.ha_env_calendar_entity_id = None;
        let app = router().with_state(state);

        // Only change the URL - the token field is left blank.
        let response = app
            .oneshot(
                Request::post("/settings/ha-test")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "ha_url=http%3A%2F%2Fha-new.local&ha_token=&\
                         ha_calendar_entity_id=calendar.foodinator",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // NoopCalendarSync (the default test factory) always succeeds, so a
        // non-500 response here confirms effective_test_config resolved
        // Some(..) rather than treating the blank token as "not configured."
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !html.to_lowercase().contains("not configured"),
            "the stored token should have been reused: {html}"
        );

        Ok(())
    }
}
