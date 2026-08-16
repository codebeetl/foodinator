use askama::Template;
use axum::extract::{Form, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{Duration, NaiveDate};
use serde::Deserialize;

use crate::clock;
use crate::db::{settings, sync as sync_db};
use crate::ha::sync as ha_sync;
use crate::state::{AppState, PageContext};

/// Only entries within this many days are eligible to sync - see
/// ha::sync::is_within_sync_horizon for why (HA has no update service, so
/// most edits must happen before an entry is ever pushed).
const SYNC_HORIZON_DAYS: i64 = 14;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync", get(show).post(run_sync))
        .route("/sync/ha-test", get(show).post(test_ha_connection))
        .route("/sync/ha-save", get(show).post(save_ha_config))
}

#[derive(Clone)]
enum HaTestOutcome {
    Success,
    Failure(String),
    NotConfigured,
}

struct SyncOutcome {
    entry_date: NaiveDate,
    meal_name: String,
    error: Option<String>,
}

struct SyncResults {
    synced: Vec<SyncOutcome>,
    failed: Vec<SyncOutcome>,
}

#[derive(Template)]
#[template(path = "sync.html")]
struct SyncTemplate {
    ctx: PageContext,
    results: Option<SyncResults>,
    ha_url_input: String,
    ha_calendar_entity_id_input: String,
    ha_test_result: Option<HaTestOutcome>,
}

impl IntoResponse for SyncTemplate {
    fn into_response(self) -> Response {
        super::render_askama_template(self)
    }
}

async fn build_sync_template(state: &AppState) -> SyncTemplate {
    let settings = settings::get(&state.pool)
        .await
        .expect("failed to fetch settings");
    let ctx = PageContext::from_state(state, &settings);
    SyncTemplate {
        ctx,
        results: None,
        ha_url_input: settings.ha_url.unwrap_or_default(),
        ha_calendar_entity_id_input: settings.ha_calendar_entity_id.unwrap_or_default(),
        ha_test_result: None,
    }
}

async fn show(State(state): State<AppState>) -> SyncTemplate {
    build_sync_template(&state).await
}

async fn run_sync(State(state): State<AppState>) -> SyncTemplate {
    let Some(ha_client) = state.ha_client().await else {
        return build_sync_template(&state).await;
    };

    let today = clock::today(&state.household_tz);
    let candidates = sync_db::list_syncable(&state.pool, today, SYNC_HORIZON_DAYS)
        .await
        .expect("failed to list syncable entries");
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to fetch settings");

    let mut synced = Vec::new();
    let mut failed = Vec::new();

    for candidate in candidates {
        let start_time = candidate.effective_start_time(app_settings.default_start_time);
        let duration_minutes =
            candidate.effective_duration_minutes(app_settings.default_duration_minutes);

        let start_utc =
            clock::household_datetime_utc(&state.household_tz, candidate.entry_date, start_time);
        let end_utc = start_utc + Duration::minutes(duration_minutes as i64);

        let description = ha_sync::build_description(
            &candidate.attendee_names,
            candidate.notes.as_deref(),
            candidate.meal_plan_entry_id,
        );
        let hash = ha_sync::content_hash(
            &candidate.meal_name,
            &description,
            &start_utc.to_rfc3339(),
            &end_utc.to_rfc3339(),
        );

        let outcome = ha_client
            .create_event(&candidate.meal_name, &description, start_utc, end_utc)
            .await;

        match outcome {
            Ok(()) => {
                sync_db::record_synced(&state.pool, candidate.meal_plan_entry_id, &hash)
                    .await
                    .expect("failed to record sync");
                synced.push(SyncOutcome {
                    entry_date: candidate.entry_date,
                    meal_name: candidate.meal_name,
                    error: None,
                });
            }
            Err(err) => {
                let message = err.to_string();
                sync_db::record_failed(&state.pool, candidate.meal_plan_entry_id, &hash, &message)
                    .await
                    .expect("failed to record sync failure");
                failed.push(SyncOutcome {
                    entry_date: candidate.entry_date,
                    meal_name: candidate.meal_name,
                    error: Some(message),
                });
            }
        }
    }

    SyncTemplate {
        ctx: PageContext::from_state(&state, &app_settings),
        results: Some(SyncResults { synced, failed }),
        ha_url_input: app_settings.ha_url.unwrap_or_default(),
        ha_calendar_entity_id_input: app_settings.ha_calendar_entity_id.unwrap_or_default(),
        ha_test_result: None,
    }
}

#[derive(Deserialize)]
struct HaConfigForm {
    ha_url: String,
    ha_token: String,
    ha_calendar_entity_id: String,
}

async fn save_ha_config(
    State(state): State<AppState>,
    Form(form): Form<HaConfigForm>,
) -> SyncTemplate {
    settings::update_ha(
        &state.pool,
        super::non_empty(&form.ha_url),
        super::non_empty(&form.ha_token),
        super::non_empty(&form.ha_calendar_entity_id),
    )
    .await
    .expect("failed to update HA settings");

    let mut template = build_sync_template(&state).await;
    template.ha_url_input = form.ha_url;
    template.ha_calendar_entity_id_input = form.ha_calendar_entity_id;
    template
}

/// Resolves what a connection test should actually try: the typed field if
/// non-blank, otherwise whatever's currently in effect (DB override or env
/// fallback).
async fn effective_ha_test_config(
    state: &AppState,
    form: &HaConfigForm,
) -> Option<settings::HaConfig> {
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

    Some(settings::HaConfig {
        url,
        token,
        calendar_entity_id,
    })
}

async fn test_ha_connection(
    State(state): State<AppState>,
    Form(form): Form<HaConfigForm>,
) -> SyncTemplate {
    let config = effective_ha_test_config(&state, &form).await;

    let ha_test_result = Some(match &config {
        None => HaTestOutcome::NotConfigured,
        Some(config) => {
            let client = (state.ha_client_factory)(config.clone());
            match client.get_api_status().await {
                Ok(()) => HaTestOutcome::Success,
                Err(err) => HaTestOutcome::Failure(err.to_string()),
            }
        }
    });

    let mut template = build_sync_template(&state).await;
    template.ha_url_input = form.ha_url;
    template.ha_calendar_entity_id_input = form.ha_calendar_entity_id;
    template.ha_test_result = ha_test_result;
    template
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{consumers, meal_plan, meals};
    use crate::ha::test_support::FailingCalendarSync;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use std::sync::Arc;
    use tower::ServiceExt;

    #[sqlx::test(migrations = "./migrations")]
    async fn syncing_pushes_eligible_entries_and_records_them_as_synced(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = clock::today(&chrono_tz::UTC);
        let entry = meal_plan::upsert_entry(
            &pool,
            today + chrono::Duration::days(1),
            tacos.id,
            Some("bring hot sauce"),
            None,
            None,
            &[],
        )
        .await?;
        meal_plan::set_attendance(&pool, entry.id, &[alice.id]).await?;

        let app = router().with_state(crate::state::test_app_state(pool.clone()));
        let response = app
            .oneshot(Request::post("/sync").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Tacos"),
            "synced entry should be listed: {html}"
        );

        let candidates = sync_db::list_syncable(&pool, today, SYNC_HORIZON_DAYS).await?;
        assert!(
            candidates.is_empty(),
            "a successfully synced entry should no longer be a candidate"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_failed_push_is_reported_and_stays_eligible_for_retry(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = clock::today(&chrono_tz::UTC);
        meal_plan::upsert_entry(
            &pool,
            today + chrono::Duration::days(1),
            tacos.id,
            None,
            None,
            None,
            &[],
        )
        .await?;

        let mut state = crate::state::test_app_state(pool.clone());
        state.ha_client_factory = Arc::new(|_config| Arc::new(FailingCalendarSync));
        let app = router().with_state(state);

        let response = app
            .oneshot(Request::post("/sync").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Tacos"),
            "failed entry should be listed: {html}"
        );

        let candidates = sync_db::list_syncable(&pool, today, SYNC_HORIZON_DAYS).await?;
        assert_eq!(
            candidates.len(),
            1,
            "a failed push should remain eligible to retry"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn show_renders_the_sync_page_with_ha_config_fields(pool: PgPool) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/sync").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Home Assistant"),
            "should show HA section: {html}"
        );
        assert!(
            html.contains("ha_url"),
            "should contain HA URL input: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_ha_connection_reports_not_configured_with_no_fields(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.ha_env_url = None;
        state.ha_env_token = None;
        state.ha_env_calendar_entity_id = None;
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::post("/sync/ha-test")
                    .header(
                        axum::http::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
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
            "should report not-configured state: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn saving_ha_config_persists_to_db(pool: PgPool) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post("/sync/ha-save")
                    .header(
                        axum::http::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from(
                        "ha_url=http%3A%2F%2Fha.local&ha_token=mytoken&ha_calendar_entity_id=calendar.foodinator",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let settings = settings::get(&pool).await?;
        assert_eq!(settings.ha_url.as_deref(), Some("http://ha.local"));
        assert_eq!(settings.ha_token.as_deref(), Some("mytoken"));
        assert_eq!(
            settings.ha_calendar_entity_id.as_deref(),
            Some("calendar.foodinator")
        );

        Ok(())
    }
}
