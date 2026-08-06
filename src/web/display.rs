use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use axum::Router;
use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::clock;
use crate::db::{consumers, meal_plan, meals, settings, sync as sync_db};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/display", get(show))
        .route("/display/data", get(data))
        .route("/display/preview", get(preview))
}

#[derive(Deserialize)]
struct DisplayQuery {
    token: Option<String>,
}

fn authorize(state: &AppState, token: Option<&str>) -> Result<(), StatusCode> {
    let Some(expected) = &state.display_token else {
        return Err(StatusCode::NOT_FOUND);
    };
    if token != Some(expected.as_str()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

/// The week_start_weekday occurrence that begins the week containing
/// `today`. Deliberately the mirror of plan::next_week_start (which only
/// looks forward, since /plan is for planning the *upcoming* week): /display
/// is a status view, so it must always be able to mark exactly one card as
/// "today," which means walking backward to the most recent match when
/// today isn't itself the start weekday.
fn week_containing(today: NaiveDate, week_start_weekday: i16) -> NaiveDate {
    let days_since =
        (today.weekday().num_days_from_monday() as i64 - week_start_weekday as i64).rem_euclid(7);
    today - Duration::days(days_since)
}

/// Every field here is already formatted exactly as it should appear on
/// screen, so the initial server render and a later JSON poll produce
/// identical text for the same underlying data - the poll only ever swaps
/// this text into fixed DOM nodes, never reformats it client-side.
#[derive(Clone, Serialize)]
struct DisplayDay {
    #[serde(skip)]
    date_label: String,
    is_today: bool,
    meal_name: Option<String>,
    meal_time: Option<String>,
    attendees: Option<String>,
    notes: Option<String>,
    sync_status: Option<&'static str>,
}

async fn build_days(state: &AppState) -> (NaiveDate, Vec<DisplayDay>) {
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to load settings");
    let today = clock::today(&state.household_tz);
    let week_start = week_containing(today, app_settings.week_start_weekday);
    let ha_configured = state.ha_client().await.is_some();

    let all_consumers = consumers::list_all(&state.pool)
        .await
        .expect("failed to list consumers");

    let mut days = Vec::with_capacity(7);
    for offset in 0..7 {
        let date = week_start + Duration::days(offset);
        let is_today = date == today;
        let live_entry = meal_plan::get_by_date(&state.pool, date)
            .await
            .expect("failed to fetch plan entry")
            .filter(|entry| entry.deleted_at.is_none());

        let day = match live_entry {
            Some(entry) => {
                let meal = meals::get(&state.pool, entry.meal_id)
                    .await
                    .expect("failed to fetch meal")
                    .expect("meal_plan_entries.meal_id references an existing meal");
                let attendee_ids = meal_plan::get_attendance(&state.pool, entry.id)
                    .await
                    .expect("failed to fetch attendance");
                let mut attendee_names: Vec<String> = all_consumers
                    .iter()
                    .filter(|c| attendee_ids.contains(&c.id))
                    .map(|c| c.name.clone())
                    .collect();
                attendee_names.extend(entry.guest_names.iter().cloned());
                let sync_status = if ha_configured {
                    Some(
                        sync_db::status_for_entry(&state.pool, entry.id)
                            .await
                            .expect("failed to fetch sync status")
                            .as_str(),
                    )
                } else {
                    None
                };
                let start_time = entry
                    .start_time_override
                    .unwrap_or(app_settings.default_start_time);
                let notes = entry.notes.filter(|_| is_today).filter(|n| !n.is_empty());

                DisplayDay {
                    date_label: date.format("%A, %-d %B").to_string(),
                    is_today,
                    meal_name: Some(meal.name),
                    meal_time: Some(start_time.format("%-I:%M %p").to_string()),
                    attendees: (!attendee_names.is_empty()).then(|| attendee_names.join(", ")),
                    notes,
                    sync_status,
                }
            }
            None => DisplayDay {
                date_label: date.format("%A, %-d %B").to_string(),
                is_today,
                meal_name: None,
                meal_time: None,
                attendees: None,
                notes: None,
                sync_status: None,
            },
        };
        days.push(day);
    }

    (week_start, days)
}

#[derive(Template)]
#[template(path = "display.html")]
struct DisplayTemplate {
    week_start: NaiveDate,
    days: Vec<DisplayDay>,
}

async fn show(State(state): State<AppState>, Query(query): Query<DisplayQuery>) -> Response {
    if let Err(status) = authorize(&state, query.token.as_deref()) {
        return status.into_response();
    }

    let (week_start, days) = build_days(&state).await;
    DisplayTemplate { week_start, days }.into_response()
}

#[derive(Serialize)]
struct DisplayData {
    week_start: NaiveDate,
    days: Vec<DisplayDay>,
}

async fn data(State(state): State<AppState>, Query(query): Query<DisplayQuery>) -> Response {
    if let Err(status) = authorize(&state, query.token.as_deref()) {
        return status.into_response();
    }

    let (week_start, days) = build_days(&state).await;
    Json(DisplayData { week_start, days }).into_response()
}

/// Lives inside the normal Basic-Auth-protected admin app (unlike
/// /display itself, which is token-gated so a tablet can load it without a
/// password) - lets the household preview the kiosk view and grab its URL to
/// paste into a tablet's browser, without needing to know the token by heart.
#[derive(Template)]
#[template(path = "display_preview.html")]
struct DisplayPreviewTemplate {
    display_path: String,
    ha_configured: bool,
    display_configured: bool,
    theme: String,
}

async fn preview(State(state): State<AppState>) -> Response {
    let Some(token) = &state.display_token else {
        return StatusCode::NOT_FOUND.into_response();
    };

    DisplayPreviewTemplate {
        display_path: format!("/display?token={token}"),
        ha_configured: state.ha_client().await.is_some(),
        display_configured: true,
        theme: state.theme().await,
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;
    use tower::ServiceExt;

    #[test]
    fn week_containing_finds_the_nearest_matching_weekday_on_or_before_today() {
        let monday = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let wednesday = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let saturday = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();

        // week_start_weekday=5 (Saturday) - the default.
        assert_eq!(
            week_containing(saturday, 5),
            saturday,
            "today counts as a match"
        );
        assert_eq!(
            week_containing(monday, 5),
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
            "backward-only: last Saturday, not next week's"
        );

        // week_start_weekday=2 (Wednesday) - a household with a different planning day.
        assert_eq!(week_containing(wednesday, 2), wednesday);
        assert_eq!(
            week_containing(saturday, 2),
            wednesday,
            "today (Saturday) should fall within the week that started this Wednesday"
        );
    }

    async fn pin_week_start_to_today(pool: &PgPool, today: NaiveDate) {
        let current = settings::get(pool).await.expect("failed to load settings");
        settings::update(
            pool,
            current.default_start_time,
            current.default_duration_minutes,
            today.weekday().num_days_from_monday() as i16,
            &current.theme,
        )
        .await
        .expect("failed to pin week_start_weekday");
    }

    fn state_with(display_token: Option<&str>) -> AppState {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/db")
            .expect("valid connection string");
        let mut state = crate::state::test_app_state(pool);
        state.display_token = display_token.map(str::to_string);
        state
    }

    #[tokio::test]
    async fn returns_404_when_display_token_is_not_configured() {
        let app = router().with_state(state_with(None));

        let response = app
            .oneshot(
                Request::get("/display?token=anything")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn preview_returns_404_when_display_token_is_not_configured() {
        let app = router().with_state(state_with(None));

        let response = app
            .oneshot(
                Request::get("/display/preview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn preview_shows_the_reference_url_with_the_token_embedded(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.display_token = Some("kiosk-secret".to_string());
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/display/preview")
                    .body(Body::empty())
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
            html.contains("/display?token=kiosk-secret"),
            "the reference URL should include the real token: {html}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn returns_401_when_token_query_param_is_missing() {
        let app = router().with_state(state_with(Some("kiosk-secret")));

        let response = app
            .oneshot(Request::get("/display").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn returns_401_when_token_is_wrong() {
        let app = router().with_state(state_with(Some("kiosk-secret")));

        let response = app
            .oneshot(
                Request::get("/display?token=wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn data_endpoint_is_also_token_gated() {
        let app = router().with_state(state_with(Some("kiosk-secret")));

        let response = app
            .oneshot(
                Request::get("/display/data?token=wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn shows_todays_meal_and_attendees_when_token_matches(pool: PgPool) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = clock::today(&chrono_tz::UTC);
        pin_week_start_to_today(&pool, today).await;
        let entry = meal_plan::upsert_entry(
            &pool,
            today,
            tacos.id,
            Some("bring hot sauce"),
            None,
            None,
            &["Aunt Jane".to_string()],
        )
        .await?;
        meal_plan::set_attendance(&pool, entry.id, &[alice.id]).await?;

        let mut state = crate::state::test_app_state(pool);
        state.display_token = Some("kiosk-secret".to_string());
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/display?token=kiosk-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Tacos"), "meal name should render: {html}");
        assert!(html.contains("Alice"), "attendee should render: {html}");
        assert!(html.contains("Aunt Jane"), "guest should render: {html}");
        assert!(
            html.contains("bring hot sauce"),
            "today's notes should render: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn shows_a_placeholder_for_an_unplanned_day(pool: PgPool) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.display_token = Some("kiosk-secret".to_string());
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/display?token=kiosk-secret")
                    .body(Body::empty())
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
            html.contains("Nothing planned"),
            "an unplanned day should show a placeholder, not default attendees: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn omits_notes_on_non_today_cards(pool: PgPool) -> sqlx::Result<()> {
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = clock::today(&chrono_tz::UTC);
        pin_week_start_to_today(&pool, today).await;
        meal_plan::upsert_entry(
            &pool,
            today + chrono::Duration::days(1),
            tacos.id,
            Some("should not appear"),
            None,
            None,
            &[],
        )
        .await?;

        let mut state = crate::state::test_app_state(pool);
        state.display_token = Some("kiosk-secret".to_string());
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/display?token=kiosk-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !html.contains("should not appear"),
            "notes should only render on today's card: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hides_sync_status_when_ha_is_not_configured(pool: PgPool) -> sqlx::Result<()> {
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = clock::today(&chrono_tz::UTC);
        meal_plan::upsert_entry(&pool, today, tacos.id, None, None, None, &[]).await?;

        let mut state = crate::state::test_app_state(pool);
        state.display_token = Some("kiosk-secret".to_string());
        state.ha_env_url = None;
        state.ha_env_token = None;
        state.ha_env_calendar_entity_id = None;
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/display?token=kiosk-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !html.contains("kiosk-sync-status--"),
            "sync status should have no value/modifier class when HA isn't configured: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn data_endpoint_returns_the_same_week_and_fields_as_the_page(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = meals::insert(&pool, "Tacos").await?;
        let today = clock::today(&chrono_tz::UTC);
        pin_week_start_to_today(&pool, today).await;
        let entry = meal_plan::upsert_entry(&pool, today, tacos.id, None, None, None, &[]).await?;
        meal_plan::set_attendance(&pool, entry.id, &[alice.id]).await?;

        let mut state = crate::state::test_app_state(pool);
        state.display_token = Some("kiosk-secret".to_string());
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/display/data?token=kiosk-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["week_start"], today.to_string());
        let days = json["days"].as_array().unwrap();
        assert_eq!(days.len(), 7);
        let todays = days.iter().find(|d| d["is_today"] == true).unwrap();
        assert_eq!(todays["meal_name"], "Tacos");
        assert_eq!(todays["attendees"], "Alice");

        Ok(())
    }
}
