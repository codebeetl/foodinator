use std::collections::HashMap;

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::response::Redirect;
use axum::routing::{get, post};
use axum::Router;
use chrono::{Datelike, Duration, NaiveDate, NaiveTime};
use serde::Deserialize;

use crate::clock;
use crate::db::consumers::{self, Consumer};
use crate::db::meal_plan;
use crate::db::settings;
use crate::state::AppState;

const ATTENDEE_FIELD_PREFIX: &str = "attendee_";
const GUEST_FIELD_PREFIX: &str = "guest_name_";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plan", get(show))
        .route("/plan/:date", post(update))
        .route("/plan/:date/delete", post(delete))
}

/// The nearest date whose weekday matches `week_start_weekday`
/// (0=Monday..6=Sunday, per app_settings.week_start_weekday), inclusive of
/// today.
fn next_week_start(today: NaiveDate, week_start_weekday: i16) -> NaiveDate {
    let days_ahead =
        (week_start_weekday as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
    today + Duration::days(days_ahead)
}

struct PlanDay {
    date: NaiveDate,
    date_label: String,
    has_entry: bool,
    selected_meal_id: Option<i64>,
    selected_meal_name: Option<String>,
    notes: String,
    // Always the value that's actually in effect - the override if one is
    // set, otherwise the current global default - so the field never renders
    // blank and always reads as "the meal time," not "an unset override."
    effective_start_time: NaiveTime,
    effective_duration_minutes: i32,
    guest_names: Vec<String>,
    attendee_ids: Vec<i64>,
    // Whether any non-default consumer is eligible to be added but isn't
    // already attending - gates whether the "+ Add known consumer" disclosure
    // has anything in it.
    has_hidden_consumers: bool,
}

impl PlanDay {
    fn attends(&self, consumer_id: &i64) -> bool {
        self.attendee_ids.contains(consumer_id)
    }
}

#[derive(Template)]
#[template(path = "plan.html")]
struct PlanTemplate {
    week_start: NaiveDate,
    prev_start: NaiveDate,
    next_start: NaiveDate,
    // Only set when week_start isn't already the current week, so the
    // template can hide the link rather than show a no-op jump to itself.
    current_week_start: Option<NaiveDate>,
    days: Vec<PlanDay>,
    consumers: Vec<Consumer>,
    ha_configured: bool,
}

#[derive(Deserialize)]
struct PlanQuery {
    start: Option<NaiveDate>,
}

async fn show(State(state): State<AppState>, Query(query): Query<PlanQuery>) -> PlanTemplate {
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to load settings");
    let this_week_start = next_week_start(
        clock::today(&state.household_tz),
        app_settings.week_start_weekday,
    );
    let week_start = query.start.unwrap_or(this_week_start);

    let consumers: Vec<Consumer> = consumers::list_all(&state.pool)
        .await
        .expect("failed to list consumers")
        .into_iter()
        .filter(|c| c.active)
        .collect();
    let default_attendee_ids: Vec<i64> = consumers
        .iter()
        .filter(|c| c.is_default)
        .map(|c| c.id)
        .collect();

    let mut days = Vec::with_capacity(7);
    for offset in 0..7 {
        let date = week_start + Duration::days(offset);
        let live_entry = meal_plan::get_by_date(&state.pool, date)
            .await
            .expect("failed to fetch plan entry")
            .filter(|entry| entry.deleted_at.is_none());

        // A day with no entry yet hasn't been planned at all, so it starts
        // from the household's default attendees rather than an empty set.
        let attendee_ids = match &live_entry {
            Some(entry) => meal_plan::get_attendance(&state.pool, entry.id)
                .await
                .expect("failed to fetch attendance"),
            None => default_attendee_ids.clone(),
        };
        let meals = meal_plan::suitability_for_attendees(&state.pool, &attendee_ids)
            .await
            .expect("failed to compute suitability");
        let selected_meal_id = live_entry.as_ref().map(|entry| entry.meal_id);
        let selected_meal_name = selected_meal_id
            .and_then(|id| meals.iter().find(|m| m.id == id))
            .map(|m| m.name.clone());
        let has_hidden_consumers = consumers
            .iter()
            .any(|c| !c.is_default && !attendee_ids.contains(&c.id));

        days.push(PlanDay {
            date,
            date_label: date.format("%A, %-d %B").to_string(),
            has_entry: live_entry.is_some(),
            selected_meal_id,
            selected_meal_name,
            notes: live_entry
                .as_ref()
                .and_then(|entry| entry.notes.clone())
                .unwrap_or_default(),
            effective_start_time: live_entry
                .as_ref()
                .and_then(|entry| entry.start_time_override)
                .unwrap_or(app_settings.default_start_time),
            effective_duration_minutes: live_entry
                .as_ref()
                .and_then(|entry| entry.duration_minutes_override)
                .unwrap_or(app_settings.default_duration_minutes),
            guest_names: live_entry
                .as_ref()
                .map(|entry| entry.guest_names.clone())
                .unwrap_or_default(),
            attendee_ids,
            has_hidden_consumers,
        });
    }

    PlanTemplate {
        week_start,
        prev_start: week_start - Duration::days(7),
        next_start: week_start + Duration::days(7),
        current_week_start: (week_start != this_week_start).then_some(this_week_start),
        days,
        consumers,
        ha_configured: state.ha_client().await.is_some(),
    }
}

#[derive(Deserialize)]
struct UpdatePlanForm {
    week_start: NaiveDate,
    meal_id: i64,
    notes: String,
    meal_time: String,
    duration_minutes: String,
    // Catches this day's dynamic `attendee_<consumer_id>` checkboxes and
    // `guest_name_<n>` text fields, since neither set is known at compile time.
    #[serde(flatten)]
    dynamic_fields: HashMap<String, String>,
}

fn non_empty(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn guest_names_from_form(dynamic_fields: &HashMap<String, String>) -> Vec<String> {
    let mut entries: Vec<(usize, String)> = dynamic_fields
        .iter()
        .filter_map(|(field, value)| {
            let index = field
                .strip_prefix(GUEST_FIELD_PREFIX)?
                .parse::<usize>()
                .ok()?;
            let name = non_empty(value)?.to_string();
            Some((index, name))
        })
        .collect();
    entries.sort_by_key(|(index, _)| *index);
    entries.into_iter().map(|(_, name)| name).collect()
}

async fn update(
    State(state): State<AppState>,
    Path(date): Path<NaiveDate>,
    Form(form): Form<UpdatePlanForm>,
) -> Redirect {
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to load settings");

    let notes = non_empty(&form.notes);
    let meal_time = form
        .meal_time
        .parse::<NaiveTime>()
        .expect("invalid meal time");
    let start_time_override = (meal_time != app_settings.default_start_time).then_some(meal_time);
    let duration_minutes = form
        .duration_minutes
        .parse::<i32>()
        .expect("invalid duration");
    let duration_minutes_override =
        (duration_minutes != app_settings.default_duration_minutes).then_some(duration_minutes);
    let guest_names = guest_names_from_form(&form.dynamic_fields);

    let entry = meal_plan::upsert_entry(
        &state.pool,
        date,
        form.meal_id,
        notes,
        start_time_override,
        duration_minutes_override,
        &guest_names,
    )
    .await
    .expect("failed to upsert plan entry");

    let attendee_ids: Vec<i64> = form
        .dynamic_fields
        .keys()
        .filter_map(|field| field.strip_prefix(ATTENDEE_FIELD_PREFIX))
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();
    meal_plan::set_attendance(&state.pool, entry.id, &attendee_ids)
        .await
        .expect("failed to set attendance");

    Redirect::to(&format!("/plan?start={}", form.week_start))
}

#[derive(Deserialize)]
struct DeletePlanForm {
    week_start: NaiveDate,
}

async fn delete(
    State(state): State<AppState>,
    Path(date): Path<NaiveDate>,
    Form(form): Form<DeletePlanForm>,
) -> Redirect {
    meal_plan::soft_delete(&state.pool, date)
        .await
        .expect("failed to clear plan entry");
    Redirect::to(&format!("/plan?start={}", form.week_start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    #[test]
    fn next_week_start_finds_the_nearest_matching_weekday_inclusive_of_today() {
        // All three fall in the same calendar week, in forward order.
        let monday = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let wednesday = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let saturday = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();

        // week_start_weekday=5 (Saturday) - today's existing default behavior.
        assert_eq!(
            next_week_start(saturday, 5),
            saturday,
            "today counts as a match"
        );
        assert_eq!(next_week_start(monday, 5), saturday);

        // week_start_weekday=2 (Wednesday) - a household with a different planning day.
        assert_eq!(next_week_start(wednesday, 2), wednesday);
        assert_eq!(next_week_start(monday, 2), wednesday);
        assert_eq!(
            next_week_start(saturday, 2),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
            "forward-only: the next Wednesday, not last week's"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn planning_a_day_through_the_form_persists_meal_and_attendance(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let body = format!(
            "week_start=2026-08-08&meal_id={}&notes=Family+dinner&meal_time=19%3A00&\
             duration_minutes=45&attendee_{}=on&guest_name_0=Aunt+Jane",
            tacos.id, alice.id
        );
        let response = app
            .oneshot(
                Request::post(format!("/plan/{date}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/plan?start=2026-08-08"
        );

        let entry = meal_plan::get_by_date(&pool, date)
            .await?
            .expect("entry should exist");
        assert_eq!(entry.meal_id, tacos.id);
        assert_eq!(entry.notes.as_deref(), Some("Family dinner"));
        assert_eq!(
            entry.start_time_override,
            Some(NaiveTime::from_hms_opt(19, 0, 0).unwrap())
        );
        assert_eq!(entry.duration_minutes_override, Some(45));
        assert_eq!(entry.guest_names, vec!["Aunt Jane".to_string()]);

        let attendance = meal_plan::get_attendance(&pool, entry.id).await?;
        assert_eq!(attendance, vec![alice.id]);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn blank_notes_and_default_time_and_duration_clear_overrides(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        meal_plan::upsert_entry(
            &pool,
            date,
            tacos.id,
            Some("old notes"),
            Some(NaiveTime::from_hms_opt(19, 0, 0).unwrap()),
            Some(45),
            &[],
        )
        .await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        // 18:30 / 30 are the migration defaults - submitting them back should
        // clear the override rather than storing them as an explicit one.
        let body = format!(
            "week_start=2026-08-08&meal_id={}&notes=&meal_time=18%3A30&duration_minutes=30",
            tacos.id
        );
        app.oneshot(
            Request::post(format!("/plan/{date}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

        let entry = meal_plan::get_by_date(&pool, date).await?.unwrap();
        assert_eq!(entry.notes, None, "blank text input should clear notes");
        assert_eq!(entry.start_time_override, None);
        assert_eq!(entry.duration_minutes_override, None);
        assert!(entry.guest_names.is_empty(), "no guest fields submitted");

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn clearing_a_day_soft_deletes_it(pool: PgPool) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        meal_plan::upsert_entry(&pool, date, tacos.id, None, None, None, &[]).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post(format!("/plan/{date}/delete"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("week_start=2026-08-08"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let entry = meal_plan::get_by_date(&pool, date).await?.unwrap();
        assert!(entry.deleted_at.is_some());

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_blank_day_defaults_to_default_consumers_and_the_global_meal_time(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        consumers::set_default(&pool, alice.id, true).await?;
        let bob = consumers::insert(&pool, "Bob").await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(
                Request::get("/plan?start=2026-08-08")
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
            html.contains(&format!(r#"name="attendee_{}" checked"#, alice.id)),
            "default consumer should be pre-checked on an unplanned day: {html}"
        );
        assert!(
            !html.contains(&format!(r#"name="attendee_{}" checked"#, bob.id)),
            "non-default consumer should not be pre-checked: {html}"
        );
        assert!(
            html.contains(r#"name="meal_time" value='18:30'"#),
            "unplanned day should show the global default meal time: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn show_page_displays_the_selected_meals_name_on_the_picker_button(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        // Disliked-by-attendee flagging now lives entirely in the /api/meals
        // search results (see web::meals::tests) - the plan page itself only
        // needs to show which meal is currently selected.
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        meal_plan::upsert_entry(&pool, date, tacos.id, None, None, None, &[]).await?;

        let app = router().with_state(crate::state::test_app_state(pool.clone()));
        let response = app
            .oneshot(
                Request::get("/plan?start=2026-08-08")
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
        let button = html
            .split(r#"class="meal-picker-trigger">"#)
            .nth(1)
            .and_then(|rest| rest.split("</button>").next())
            .expect("picker button should be present");
        assert!(
            button.contains("Tacos"),
            "picker button should show the currently selected meal's name: {button}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn nav_hides_the_sync_link_when_ha_is_not_configured(pool: PgPool) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.ha_env_url = None;
        state.ha_env_token = None;
        state.ha_env_calendar_entity_id = None;
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::get("/plan?start=2026-08-08")
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
            !html.contains(r#"href="/sync""#),
            "Sync nav link should be hidden when HA isn't configured: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn jump_to_current_week_link_is_hidden_when_already_on_the_current_week(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        // No `start` query param: show() falls back to today's week, so this
        // is always the current week regardless of when the test runs.
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/plan").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !html.contains("This week"),
            "jump-to-current-week link should be hidden when already viewing it: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn jump_to_current_week_link_is_shown_when_viewing_a_different_week(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(
                Request::get("/plan?start=2030-01-05")
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
            html.contains("This week"),
            "jump-to-current-week link should show when browsing a different week: {html}"
        );

        Ok(())
    }
}
