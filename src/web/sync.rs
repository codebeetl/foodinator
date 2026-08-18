use askama::Template;
use axum::extract::{Form, Query, State};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{Duration, NaiveDate};
use serde::Deserialize;

use crate::clock;
use crate::db::{settings, sync as sync_db};
use crate::gcal::{self, client::GcalClient, sync as gcal_sync};
use crate::ha::sync as ha_sync;
use crate::state::{AppState, PageContext};

struct RequestOrigin {
    scheme: String,
    host: String,
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for RequestOrigin {
    type Rejection = Redirect;

    #[allow(refining_impl_trait)]
    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self, Self::Rejection>> + Send>>
    {
        let host = parts
            .headers
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let scheme = parts
            .uri
            .scheme_str()
            .map(str::to_string)
            .or_else(|| {
                parts
                    .headers
                    .get("x-forwarded-proto")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "http".to_string());
        Box::pin(async move {
            match host {
                Some(h) => Ok(RequestOrigin { scheme, host: h }),
                None => Err(Redirect::temporary("/sync?gcal_error=no_host")),
            }
        })
    }
}

/// Only entries within this many days are eligible to sync - see
/// ha::sync::is_within_sync_horizon for why (HA has no update service, so
/// most edits must happen before an entry is ever pushed).
const SYNC_HORIZON_DAYS: i64 = 14;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync", get(show).post(run_sync))
        .route("/sync/ha-test", get(show).post(test_ha_connection))
        .route("/sync/ha-save", get(show).post(save_ha_config))
        .route("/sync/gcal/auth", get(gcal_auth))
        .route("/sync/gcal/callback", get(gcal_callback))
        .route("/sync/gcal/calendars", get(gcal_calendars))
        .route("/sync/gcal/calendar", post(gcal_save_calendar))
        .route("/sync/gcal", post(run_gcal_sync))
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
    gcal_connected: bool,
    gcal_fields_filled: bool,
    gcal_sync_result: Option<SyncResults>,
    gcal_error: Option<String>,
    gcal_just_connected: bool,
    gcal_calendars: Vec<gcal::GcalCalendarEntry>,
    gcal_show_picker: bool,
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
    let gcal_connected = settings::resolve_gcal_config(
        &settings,
        state.gcal_env_client_id.as_deref(),
        state.gcal_env_client_secret.as_deref(),
    )
    .is_some();
    // "fields filled" means we have a usable client_id + client_secret
    // from the env vars — enough to start OAuth.
    let gcal_fields_filled = state
        .gcal_env_client_id
        .as_deref()
        .is_some_and(|v| !v.is_empty())
        && state
            .gcal_env_client_secret
            .as_deref()
            .is_some_and(|v| !v.is_empty());
    SyncTemplate {
        ctx,
        results: None,
        ha_url_input: settings.ha_url.unwrap_or_default(),
        ha_calendar_entity_id_input: settings.ha_calendar_entity_id.unwrap_or_default(),
        ha_test_result: None,
        gcal_connected,
        gcal_fields_filled,
        gcal_sync_result: None,
        gcal_error: None,
        gcal_just_connected: false,
        gcal_calendars: Vec::new(),
        gcal_show_picker: false,
    }
}

#[derive(Deserialize)]
struct SyncShowParams {
    gcal_error: Option<String>,
    gcal_connected: Option<String>,
}

async fn show(State(state): State<AppState>, Query(params): Query<SyncShowParams>) -> SyncTemplate {
    let mut template = build_sync_template(&state).await;
    template.gcal_error = params.gcal_error;
    template.gcal_just_connected = params.gcal_connected.as_deref() == Some("true");
    template
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

    let gcal_connected = settings::resolve_gcal_config(
        &app_settings,
        state.gcal_env_client_id.as_deref(),
        state.gcal_env_client_secret.as_deref(),
    )
    .is_some();
    let gcal_fields_filled = state
        .gcal_env_client_id
        .as_deref()
        .is_some_and(|v| !v.is_empty())
        && state
            .gcal_env_client_secret
            .as_deref()
            .is_some_and(|v| !v.is_empty());
    SyncTemplate {
        ctx: PageContext::from_state(&state, &app_settings),
        results: Some(SyncResults { synced, failed }),
        ha_url_input: app_settings.ha_url.unwrap_or_default(),
        ha_calendar_entity_id_input: app_settings.ha_calendar_entity_id.unwrap_or_default(),
        ha_test_result: None,
        gcal_connected,
        gcal_fields_filled,
        gcal_sync_result: None,
        gcal_error: None,
        gcal_just_connected: false,
        gcal_calendars: Vec::new(),
        gcal_show_picker: false,
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

fn build_redirect_uri(scheme: &str, host: &str) -> String {
    format!("{scheme}://{host}/sync/gcal/callback")
}

/// Build the OAuth redirect URI from the incoming request, or from the
/// `GCAL_REDIRECT_URI` env-var override when set. The override is needed
/// because Google only allows `localhost` as a redirect host, which doesn't
/// work when the browser is on a different machine than the server.
fn resolve_redirect_uri(state: &AppState, origin: &RequestOrigin) -> String {
    if let Some(ref uri) = state.gcal_redirect_uri {
        return uri.clone();
    }
    build_redirect_uri(&origin.scheme, &origin.host)
}

async fn gcal_auth(State(state): State<AppState>, origin: RequestOrigin) -> Redirect {
    let Some(client_id) = state.gcal_env_client_id.as_deref() else {
        return Redirect::temporary("/sync?gcal_error=not_configured");
    };
    if client_id.is_empty() {
        return Redirect::temporary("/sync?gcal_error=not_configured");
    }

    // Use the env-var override when set, otherwise derive from the request.
    // Google only allows "localhost" for private-network redirect URIs, so
    // when accessing from another machine, set GCAL_REDIRECT_URI to a tunnel
    // URL (e.g. ngrok, cloudflare tunnel) or use an SSH port forward.
    let redirect_uri = resolve_redirect_uri(&state, &origin);

    let auth_url = gcal::build_auth_url(client_id, &redirect_uri, None, None);

    Redirect::temporary(&auth_url)
}

#[derive(Deserialize)]
pub struct GcalCallbackParams {
    code: Option<String>,
    error: Option<String>,
}

async fn gcal_callback(
    State(state): State<AppState>,
    origin: RequestOrigin,
    Query(params): Query<GcalCallbackParams>,
) -> Redirect {
    if let Some(error) = &params.error {
        return Redirect::temporary(&format!("/sync?gcal_error={error}"));
    }

    let Some(code) = &params.code else {
        return Redirect::temporary("/sync?gcal_error=no_code");
    };

    let settings = settings::get(&state.pool)
        .await
        .expect("failed to fetch settings");

    // Only client_id and client_secret are needed to exchange the auth code.
    // resolve_gcal_config requires refresh_token which doesn't exist yet on
    // first connection — that's the whole point of this OAuth flow.
    let client_id = settings
        .gcal_client_id
        .clone()
        .or_else(|| state.gcal_env_client_id.clone());
    let client_secret = settings
        .gcal_client_secret
        .clone()
        .or_else(|| state.gcal_env_client_secret.clone());
    let (Some(client_id), Some(client_secret)) = (client_id.as_deref(), client_secret.as_deref())
    else {
        return Redirect::temporary("/sync?gcal_error=not_configured");
    };

    let redirect_uri = resolve_redirect_uri(&state, &origin);

    match gcal::exchange_code(client_id, client_secret, code, &redirect_uri).await {
        Ok(tokens) => {
            if let Some(refresh_token) = &tokens.refresh_token {
                settings::set_gcal_refresh_token(&state.pool, refresh_token)
                    .await
                    .expect("failed to store refresh token");
            }
            // Redirect to the calendar picker with the access token so the
            // user can select which calendar to sync to.
            let picker_url = format!("/sync/gcal/calendars?token={}", tokens.access_token);
            Redirect::temporary(&picker_url)
        }
        Err(err) => Redirect::temporary(&format!("/sync?gcal_error={err}")),
    }
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

async fn run_gcal_sync(State(state): State<AppState>) -> SyncTemplate {
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to fetch settings");

    let gcal_config = match settings::resolve_gcal_config(
        &app_settings,
        state.gcal_env_client_id.as_deref(),
        state.gcal_env_client_secret.as_deref(),
    ) {
        Some(c) => c,
        None => {
            let mut template = build_sync_template(&state).await;
            template.gcal_error = Some("Google Calendar not configured".into());
            return template;
        }
    };

    let refresh_token = &gcal_config.refresh_token;
    let access_token = match gcal::refresh_access_token(
        state.gcal_token_url.as_deref(),
        &gcal_config.client_id,
        &gcal_config.client_secret,
        refresh_token,
    )
    .await
    {
        Ok(token) => token,
        Err(gcal::GcalError::TokenRevoked(_)) => {
            settings::clear_gcal_refresh_token(&state.pool)
                .await
                .expect("failed to clear revoked refresh token");
            let mut template = build_sync_template(&state).await;
            template.gcal_error =
                Some("Google Calendar authorization was revoked - please reconnect.".into());
            return template;
        }
        Err(err) => {
            let mut template = build_sync_template(&state).await;
            template.gcal_error = Some(format!("Token refresh failed: {err}"));
            return template;
        }
    };

    let client = GcalClient::new(access_token, gcal_config.calendar_id.clone());

    let today = clock::today(&state.household_tz);
    let candidates = sync_db::gcal_list_syncable(&state.pool, today, SYNC_HORIZON_DAYS)
        .await
        .expect("failed to list GCal syncable entries");

    let mut synced = Vec::new();
    let mut failed = Vec::new();

    for candidate in candidates {
        let start_time = candidate.effective_start_time(app_settings.default_start_time);
        let duration_minutes =
            candidate.effective_duration_minutes(app_settings.default_duration_minutes);

        let start_utc =
            clock::household_datetime_utc(&state.household_tz, candidate.entry_date, start_time);
        let end_utc = start_utc + Duration::minutes(duration_minutes as i64);

        let description = gcal_sync::build_description(
            &candidate.attendee_names,
            candidate.notes.as_deref(),
            candidate.meal_plan_entry_id,
        );
        let hash = gcal_sync::content_hash(
            &candidate.meal_name,
            &description,
            &start_utc.to_rfc3339(),
            &end_utc.to_rfc3339(),
        );

        let outcome = client
            .create_event(&candidate.meal_name, &description, start_utc, end_utc)
            .await;

        match outcome {
            Ok(event) => {
                sync_db::gcal_record_synced(
                    &state.pool,
                    candidate.meal_plan_entry_id,
                    &event.id,
                    &hash,
                )
                .await
                .expect("failed to record GCal sync");
                synced.push(SyncOutcome {
                    entry_date: candidate.entry_date,
                    meal_name: candidate.meal_name,
                    error: None,
                });
            }
            Err(err) => {
                let message = err.to_string();
                sync_db::gcal_record_failed(
                    &state.pool,
                    candidate.meal_plan_entry_id,
                    &hash,
                    &message,
                )
                .await
                .expect("failed to record GCal sync failure");
                failed.push(SyncOutcome {
                    entry_date: candidate.entry_date,
                    meal_name: candidate.meal_name,
                    error: Some(message),
                });
            }
        }
    }

    let mut template = build_sync_template(&state).await;
    template.gcal_sync_result = Some(SyncResults { synced, failed });
    template
}

#[derive(Deserialize)]
struct CalendarsQuery {
    token: Option<String>,
}

async fn gcal_calendars(
    State(state): State<AppState>,
    query: Query<CalendarsQuery>,
) -> Result<SyncTemplate, Redirect> {
    let Some(access_token) = &query.token else {
        return Err(Redirect::temporary("/sync?gcal_error=missing_token"));
    };

    let settings = settings::get(&state.pool)
        .await
        .expect("failed to fetch settings");

    let gcal_config = match settings::resolve_gcal_config(
        &settings,
        state.gcal_env_client_id.as_deref(),
        state.gcal_env_client_secret.as_deref(),
    ) {
        Some(c) => c,
        None => {
            return Err(Redirect::temporary("/sync?gcal_error=not_configured"));
        }
    };

    // If the stored calendar_id is empty, default to "primary" so the UI
    // can pre-select something sensible when the user has already chosen
    // a calendar.
    let gcal_fields_filled = !gcal_config.client_id.is_empty()
        && !gcal_config.calendar_id.is_empty()
        && gcal_config.refresh_token.is_empty();

    let client =
        crate::gcal::client::GcalClient::new(access_token.clone(), gcal_config.calendar_id.clone());

    let calendars = match client.list_calendars().await {
        Ok(cals) => cals,
        Err(err) => {
            let mut tmpl = build_sync_template(&state).await;
            tmpl.gcal_error = Some(format!("Failed to list calendars: {err}"));
            return Ok(tmpl);
        }
    };

    let mut tmpl = build_sync_template(&state).await;
    tmpl.gcal_calendars = calendars;
    tmpl.gcal_show_picker = true;
    tmpl.gcal_fields_filled = gcal_fields_filled;
    Ok(tmpl)
}

#[derive(Deserialize)]
struct CalendarSelectForm {
    calendar_id: String,
}

async fn gcal_save_calendar(
    State(state): State<AppState>,
    Form(form): Form<CalendarSelectForm>,
) -> Redirect {
    settings::update_gcal(&state.pool, None, None, Some(&form.calendar_id))
        .await
        .expect("failed to save calendar selection");
    Redirect::temporary("/sync?gcal_connected=true")
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
        // HA is configured in test_app_state — sync button should be enabled
        assert!(
            !html.contains("disabled") || !html.contains("Sync to Home Assistant\" disabled"),
            "sync button should be enabled when HA is configured: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn ha_sync_button_is_disabled_when_not_configured(pool: PgPool) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.ha_env_url = None;
        state.ha_env_token = None;
        state.ha_env_calendar_entity_id = None;
        let app = router().with_state(state);

        let response = app
            .oneshot(Request::get("/sync").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("disabled>Sync to Home Assistant"),
            "sync button should be disabled when HA is not configured: {html}"
        );
        assert!(
            html.contains("Fill in and save all fields above"),
            "should show hint when HA is not configured: {html}"
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

    #[sqlx::test(migrations = "./migrations")]
    async fn show_renders_gcal_section(pool: PgPool) -> sqlx::Result<()> {
        // No GCal env vars set → button should be disabled.
        let mut state = crate::state::test_app_state(pool.clone());
        state.gcal_env_client_id = None;
        state.gcal_env_client_secret = None;
        let app = router().with_state(state);

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
            html.contains("Google Calendar"),
            "should show GCal section: {html}"
        );
        // No GCal env vars set — connect button should be disabled
        assert!(
            html.contains("aria-disabled=\"true\""),
            "connect button should be disabled when fields are blank: {html}"
        );
        assert!(
            html.contains("Set GCAL_CLIENT_ID and GCAL_CLIENT_SECRET"),
            "should show hint when fields are blank: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn gcal_auth_redirects_to_google_when_configured(pool: PgPool) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.gcal_env_client_id = Some("my-client-id".to_string());
        state.gcal_env_client_secret = Some("my-secret".to_string());
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/sync/gcal/auth")
                    .header(axum::http::header::HOST, "foodinator.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);

        let location = response
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            location.contains("accounts.google.com"),
            "should redirect to Google: {location}"
        );
        assert!(
            location.contains("client_id=my-client-id"),
            "should include client_id: {location}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn run_gcal_sync_clears_stale_refresh_token_when_google_reports_invalid_grant(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "Token has been expired or revoked.",
            })))
            .mount(&server)
            .await;

        settings::update_gcal(
            &pool,
            Some("client-id"),
            Some("client-secret"),
            Some("primary"),
        )
        .await?;
        settings::set_gcal_refresh_token(&pool, "revoked-token").await?;

        let mut state = crate::state::test_app_state(pool.clone());
        state.gcal_token_url = Some(server.uri());
        let app = router().with_state(state);

        let response = app
            .oneshot(Request::post("/sync/gcal").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("revoked") && html.contains("reconnect"),
            "should show a reconnect message: {html}"
        );

        let settings_after = settings::get(&pool).await?;
        assert_eq!(
            settings_after.gcal_refresh_token, None,
            "the revoked refresh token should be cleared"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn gcal_auth_redirects_with_error_when_not_configured(pool: PgPool) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.gcal_env_client_id = None;
        state.gcal_env_client_secret = None;
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/sync/gcal/auth")
                    .header(axum::http::header::HOST, "foodinator.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);

        let location = response
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            location.contains("gcal_error=not_configured"),
            "should redirect with error: {location}"
        );

        Ok(())
    }
}
