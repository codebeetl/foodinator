use std::collections::HashMap;

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{Duration, NaiveDate, NaiveTime};
use serde::Deserialize;
use sqlx::PgPool;

use crate::clock;
use crate::db::consumers::{self, Consumer};
use crate::db::meal_plan;
use crate::db::settings;
use crate::state::{AppState, PageContext};

const ATTENDEE_FIELD_PREFIX: &str = "attendee_";
const GUEST_FIELD_PREFIX: &str = "guest_name_";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plan", get(show))
        .route("/plan/{date}", post(update))
        .route("/plan/{date}/delete", post(delete))
        .route("/plan/{date}/clear-meal", post(clear_meal))
        .route("/plan/{date}/suggest", post(suggest_day))
        .route("/plan/week/suggest", post(suggest_week))
}

struct PlanDay {
    date: NaiveDate,
    date_label: String,
    /// A day that exists at all - a non-deleted row. True even once the meal
    /// has been cleared, which is what keeps "Clear this day" reachable so the
    /// notes/time/attendees you left behind can still be wiped.
    has_entry: bool,
    /// A day that currently names a meal. Gates the clear-the-meal button and
    /// the Reroll/Suggest label - neither means anything with no meal.
    has_meal: bool,
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
    // Only set when week_start isn't already the upcoming week, so the
    // template can hide the link rather than show a no-op jump to itself.
    upcoming_week_start: Option<NaiveDate>,
    days: Vec<PlanDay>,
    consumers: Vec<Consumer>,
    ctx: PageContext,
}

impl IntoResponse for PlanTemplate {
    fn into_response(self) -> Response {
        super::render_askama_template(self)
    }
}

#[derive(Template)]
#[template(path = "plan_day_fragment.html")]
struct PlanDayFragmentTemplate {
    day: PlanDay,
    week_start: NaiveDate,
    consumers: Vec<Consumer>,
}

impl IntoResponse for PlanDayFragmentTemplate {
    fn into_response(self) -> Response {
        super::render_askama_template(self)
    }
}

#[derive(Deserialize)]
struct PlanQuery {
    start: Option<NaiveDate>,
}

async fn active_consumers(pool: &PgPool) -> Vec<Consumer> {
    consumers::list_all(pool)
        .await
        .expect("failed to list consumers")
        .into_iter()
        .filter(|c| c.active)
        .collect()
}

async fn build_plan_day(
    pool: &PgPool,
    date: NaiveDate,
    consumers: &[Consumer],
    default_start_time: NaiveTime,
    default_duration_minutes: i32,
) -> PlanDay {
    // A day with no entry yet hasn't been planned at all, so it starts from
    // the household's default attendees rather than an empty set.
    let default_attendee_ids: Vec<i64> = consumers
        .iter()
        .filter(|c| c.is_default)
        .map(|c| c.id)
        .collect();

    let live_entry = meal_plan::get_by_date(pool, date)
        .await
        .expect("failed to fetch plan entry")
        .filter(|entry| entry.deleted_at.is_none());

    let attendee_ids = match &live_entry {
        Some(entry) => meal_plan::get_attendance(pool, entry.id)
            .await
            .expect("failed to fetch attendance"),
        None => default_attendee_ids,
    };
    let meals = meal_plan::suitability_for_attendees(pool, &attendee_ids)
        .await
        .expect("failed to compute suitability");
    let selected_meal_id = live_entry.as_ref().and_then(|entry| entry.meal_id);
    let selected_meal_name = selected_meal_id
        .and_then(|id| meals.iter().find(|m| m.id == id))
        .map(|m| m.name.clone());
    let has_hidden_consumers = consumers
        .iter()
        .any(|c| !c.is_default && !attendee_ids.contains(&c.id));

    PlanDay {
        date,
        date_label: date.format("%A, %-d %B").to_string(),
        has_entry: live_entry.is_some(),
        has_meal: live_entry
            .as_ref()
            .is_some_and(|entry| entry.meal_id.is_some()),
        selected_meal_id,
        selected_meal_name,
        notes: live_entry
            .as_ref()
            .and_then(|entry| entry.notes.clone())
            .unwrap_or_default(),
        effective_start_time: live_entry
            .as_ref()
            .map(|entry| entry.effective_start_time(default_start_time))
            .unwrap_or(default_start_time),
        effective_duration_minutes: live_entry
            .as_ref()
            .map(|entry| entry.effective_duration_minutes(default_duration_minutes))
            .unwrap_or(default_duration_minutes),
        guest_names: live_entry
            .as_ref()
            .map(|entry| entry.guest_names.clone())
            .unwrap_or_default(),
        attendee_ids,
        has_hidden_consumers,
    }
}

async fn show(State(state): State<AppState>, Query(query): Query<PlanQuery>) -> PlanTemplate {
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to load settings");
    let upcoming_week_start = clock::nearest_week_start(
        clock::today(&state.household_tz),
        app_settings.week_start_weekday,
        clock::WeekStartDirection::Forward,
    );
    let week_start = query.start.unwrap_or(upcoming_week_start);

    let consumers = active_consumers(&state.pool).await;

    let mut days = Vec::with_capacity(7);
    for offset in 0..7 {
        let date = week_start + Duration::days(offset);
        days.push(
            build_plan_day(
                &state.pool,
                date,
                &consumers,
                app_settings.default_start_time,
                app_settings.default_duration_minutes,
            )
            .await,
        );
    }

    PlanTemplate {
        week_start,
        prev_start: week_start - Duration::days(7),
        next_start: week_start + Duration::days(7),
        upcoming_week_start: (week_start != upcoming_week_start).then_some(upcoming_week_start),
        days,
        consumers,
        ctx: PageContext::from_state(&state, &app_settings),
    }
}

#[derive(Deserialize)]
struct UpdatePlanForm {
    week_start: NaiveDate,
    // Kept as a String rather than Option<i64> because the form always posts a
    // value, and it's empty once the meal has been cleared from the day - so
    // the rest of the form (notes, time, attendees) can still autosave with no
    // meal named.
    meal_id: String,
    notes: String,
    meal_time: String,
    duration_minutes: String,
    // Catches this day's dynamic `attendee_<consumer_id>` checkboxes and
    // `guest_name_<n>` text fields, since neither set is known at compile time.
    #[serde(flatten)]
    dynamic_fields: HashMap<String, String>,
}

fn guest_names_from_form(dynamic_fields: &HashMap<String, String>) -> Vec<String> {
    let mut entries: Vec<(usize, String)> = dynamic_fields
        .iter()
        .filter_map(|(field, value)| {
            let index = field
                .strip_prefix(GUEST_FIELD_PREFIX)?
                .parse::<usize>()
                .ok()?;
            let name = super::non_empty(value)?.to_string();
            Some((index, name))
        })
        .collect();
    entries.sort_by_key(|(index, _)| *index);
    entries.into_iter().map(|(_, name)| name).collect()
}

async fn update(
    State(state): State<AppState>,
    Path(date): Path<NaiveDate>,
    headers: HeaderMap,
    Form(form): Form<UpdatePlanForm>,
) -> Response {
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to load settings");

    let notes = super::non_empty(&form.notes);
    // An empty meal_id is a legitimate state, not a bad request: the day exists
    // but has no meal, and this is how its other fields keep saving.
    let meal_id = form.meal_id.trim().parse::<i64>().ok();
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
        meal_id,
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

    if super::is_ajax_request(&headers) {
        let consumers = active_consumers(&state.pool).await;
        let day = build_plan_day(
            &state.pool,
            date,
            &consumers,
            app_settings.default_start_time,
            app_settings.default_duration_minutes,
        )
        .await;
        return PlanDayFragmentTemplate {
            day,
            week_start: form.week_start,
            consumers,
        }
        .into_response();
    }

    Redirect::to(&format!("/plan?start={}", form.week_start)).into_response()
}

#[derive(Deserialize)]
struct DeletePlanForm {
    week_start: NaiveDate,
}

async fn delete(
    State(state): State<AppState>,
    Path(date): Path<NaiveDate>,
    headers: HeaderMap,
    Form(form): Form<DeletePlanForm>,
) -> Response {
    meal_plan::soft_delete(&state.pool, date)
        .await
        .expect("failed to clear plan entry");

    if super::is_ajax_request(&headers) {
        let app_settings = settings::get(&state.pool)
            .await
            .expect("failed to load settings");
        let consumers = active_consumers(&state.pool).await;
        let day = build_plan_day(
            &state.pool,
            date,
            &consumers,
            app_settings.default_start_time,
            app_settings.default_duration_minutes,
        )
        .await;
        return PlanDayFragmentTemplate {
            day,
            week_start: form.week_start,
            consumers,
        }
        .into_response();
    }

    Redirect::to(&format!("/plan?start={}", form.week_start)).into_response()
}

#[derive(Deserialize)]
struct ClearMealForm {
    week_start: NaiveDate,
}

/// Removes just the meal from a day, keeping its notes, guests, attendees, and
/// time/duration overrides. The clear button next to the picker posts here; the
/// day's "Clear this day" button still soft-deletes the whole entry.
async fn clear_meal(
    State(state): State<AppState>,
    Path(date): Path<NaiveDate>,
    headers: HeaderMap,
    Form(form): Form<ClearMealForm>,
) -> Response {
    meal_plan::clear_meal(&state.pool, date)
        .await
        .expect("failed to clear meal from plan entry");

    if super::is_ajax_request(&headers) {
        let app_settings = settings::get(&state.pool)
            .await
            .expect("failed to load settings");
        let consumers = active_consumers(&state.pool).await;
        let day = build_plan_day(
            &state.pool,
            date,
            &consumers,
            app_settings.default_start_time,
            app_settings.default_duration_minutes,
        )
        .await;
        return PlanDayFragmentTemplate {
            day,
            week_start: form.week_start,
            consumers,
        }
        .into_response();
    }

    Redirect::to(&format!("/plan?start={}", form.week_start)).into_response()
}

#[derive(Deserialize)]
struct SuggestDayForm {
    week_start: NaiveDate,
}

fn jitter(_meal_id: i64) -> f64 {
    rand::random::<f64>() * 3.0
}

/// Fills an empty day, or picks a different meal for one that's already
/// planned ("reroll") - the current meal is excluded so a reroll can't just
/// reconfirm the same pick.
async fn suggest_day(
    State(state): State<AppState>,
    Path(date): Path<NaiveDate>,
    headers: HeaderMap,
    Form(form): Form<SuggestDayForm>,
) -> Response {
    let app_settings = settings::get(&state.pool)
        .await
        .expect("failed to load settings");
    let consumers = active_consumers(&state.pool).await;
    let default_attendee_ids: Vec<i64> = consumers
        .iter()
        .filter(|c| c.is_default)
        .map(|c| c.id)
        .collect();

    let live_entry = meal_plan::get_by_date(&state.pool, date)
        .await
        .expect("failed to fetch plan entry")
        .filter(|entry| entry.deleted_at.is_none());
    let attendee_ids = match &live_entry {
        Some(entry) => meal_plan::get_attendance(&state.pool, entry.id)
            .await
            .expect("failed to fetch attendance"),
        None => default_attendee_ids.clone(),
    };

    let window_start = date - Duration::days(crate::suggest::STALENESS_CAP_DAYS);
    let window_end = date + Duration::days(crate::suggest::STALENESS_CAP_DAYS);
    let candidates =
        crate::db::meals::candidates_for_suggestion(&state.pool, window_start, window_end)
            .await
            .expect("failed to fetch suggestion candidates");

    let picked_meal_id = crate::suggest::score_candidates(
        &candidates,
        date,
        &attendee_ids,
        live_entry.as_ref().and_then(|entry| entry.meal_id),
        jitter,
    );

    if let Some(meal_id) = picked_meal_id {
        match &live_entry {
            Some(entry) => {
                meal_plan::upsert_entry(
                    &state.pool,
                    date,
                    meal_id,
                    entry.notes.as_deref(),
                    entry.start_time_override,
                    entry.duration_minutes_override,
                    &entry.guest_names,
                )
                .await
                .expect("failed to upsert plan entry");
            }
            None => {
                let entry =
                    meal_plan::upsert_entry(&state.pool, date, meal_id, None, None, None, &[])
                        .await
                        .expect("failed to upsert plan entry");
                meal_plan::set_attendance(&state.pool, entry.id, &default_attendee_ids)
                    .await
                    .expect("failed to set attendance");
            }
        }
    }

    if super::is_ajax_request(&headers) {
        let consumers = active_consumers(&state.pool).await;
        let day = build_plan_day(
            &state.pool,
            date,
            &consumers,
            app_settings.default_start_time,
            app_settings.default_duration_minutes,
        )
        .await;
        return PlanDayFragmentTemplate {
            day,
            week_start: form.week_start,
            consumers,
        }
        .into_response();
    }

    Redirect::to(&format!("/plan?start={}", form.week_start)).into_response()
}

#[derive(Deserialize)]
struct SuggestWeekForm {
    week_start: NaiveDate,
}

/// Fills every currently-empty day from today through the end of the
/// currently-viewed week - days before today, and days already planned, are
/// left untouched. Candidates are fetched once and reused across the whole
/// fill, with each pick fed back into its own candidate before the next day
/// is scored, so the same meal isn't repeated across the week when enough
/// distinct meals exist.
async fn suggest_week(
    State(state): State<AppState>,
    Form(form): Form<SuggestWeekForm>,
) -> Response {
    let today = clock::today(&state.household_tz);
    let week_start = form.week_start;
    let week_end = week_start + Duration::days(6);

    let consumers = active_consumers(&state.pool).await;
    let default_attendee_ids: Vec<i64> = consumers
        .iter()
        .filter(|c| c.is_default)
        .map(|c| c.id)
        .collect();

    let window_start = week_start - Duration::days(crate::suggest::STALENESS_CAP_DAYS);
    let window_end = week_end + Duration::days(crate::suggest::STALENESS_CAP_DAYS);
    let mut candidates =
        crate::db::meals::candidates_for_suggestion(&state.pool, window_start, window_end)
            .await
            .expect("failed to fetch suggestion candidates");

    for offset in 0..7 {
        let date = week_start + Duration::days(offset);
        if date < today {
            continue;
        }
        let live_entry = meal_plan::get_by_date(&state.pool, date)
            .await
            .expect("failed to fetch plan entry")
            .filter(|entry| entry.deleted_at.is_none());
        // A day whose meal was cleared still wants filling - it has no meal,
        // just notes and an attendee list that must survive the pick.
        if live_entry
            .as_ref()
            .is_some_and(|entry| entry.meal_id.is_some())
        {
            continue;
        }

        let attendee_ids = match &live_entry {
            Some(entry) => meal_plan::get_attendance(&state.pool, entry.id)
                .await
                .expect("failed to fetch attendance"),
            None => default_attendee_ids.clone(),
        };

        let picked_meal_id =
            crate::suggest::score_candidates(&candidates, date, &attendee_ids, None, jitter);
        let Some(meal_id) = picked_meal_id else {
            continue;
        };

        match &live_entry {
            Some(entry) => {
                meal_plan::upsert_entry(
                    &state.pool,
                    date,
                    meal_id,
                    entry.notes.as_deref(),
                    entry.start_time_override,
                    entry.duration_minutes_override,
                    &entry.guest_names,
                )
                .await
                .expect("failed to upsert plan entry");
            }
            None => {
                let entry =
                    meal_plan::upsert_entry(&state.pool, date, meal_id, None, None, None, &[])
                        .await
                        .expect("failed to upsert plan entry");
                meal_plan::set_attendance(&state.pool, entry.id, &default_attendee_ids)
                    .await
                    .expect("failed to set attendance");
            }
        }

        if let Some(candidate) = candidates.iter_mut().find(|c| c.id == meal_id) {
            candidate.planned_dates.push(date);
        }
    }

    Redirect::to(&format!("/plan?start={week_start}")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

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
        assert_eq!(entry.meal_id, Some(tacos.id));
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
    async fn updating_a_day_via_ajax_returns_the_rerendered_fragment(
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
                    .header("X-Requested-With", "XMLHttpRequest")
                    .body(Body::from(body))
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
            html.contains("Tacos"),
            "fragment should show the newly selected meal's name: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn clearing_a_day_via_ajax_returns_the_rerendered_fragment(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        meal_plan::upsert_entry(&pool, date, tacos.id, None, None, None, &[]).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post(format!("/plan/{date}/delete"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header("X-Requested-With", "XMLHttpRequest")
                    .body(Body::from("week_start=2026-08-08"))
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
            !html.contains("Clear this day"),
            "fragment for a cleared day should not show the clear button: {html}"
        );

        Ok(())
    }

    /// The x next to the picker is "remove the meal", not "clear the day" -
    /// everything the user configured around the meal has to survive it.
    #[sqlx::test(migrations = "./migrations")]
    async fn clearing_only_the_meal_keeps_the_rest_of_the_day(pool: PgPool) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let entry = meal_plan::upsert_entry(
            &pool,
            date,
            tacos.id,
            Some("Family dinner"),
            Some(NaiveTime::from_hms_opt(19, 0, 0).unwrap()),
            Some(45),
            &["Aunt Jane".to_string()],
        )
        .await?;
        meal_plan::set_attendance(&pool, entry.id, &[alice.id]).await?;

        meal_plan::clear_meal(&pool, date).await?;

        let after = meal_plan::get_by_date(&pool, date).await?.expect("entry");
        assert_eq!(after.meal_id, None, "the meal should be gone");
        assert_eq!(after.notes.as_deref(), Some("Family dinner"));
        assert_eq!(
            after.start_time_override,
            Some(NaiveTime::from_hms_opt(19, 0, 0).unwrap())
        );
        assert_eq!(after.duration_minutes_override, Some(45));
        assert_eq!(after.guest_names, vec!["Aunt Jane".to_string()]);
        assert_eq!(
            meal_plan::get_attendance(&pool, after.id).await?,
            vec![alice.id]
        );
        assert!(after.deleted_at.is_none(), "the day itself is still live");

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn clearing_only_the_meal_via_ajax_keeps_the_notes_time_and_attendees(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let entry = meal_plan::upsert_entry(
            &pool,
            date,
            tacos.id,
            Some("Family dinner"),
            Some(NaiveTime::from_hms_opt(19, 0, 0).unwrap()),
            Some(45),
            &[],
        )
        .await?;
        meal_plan::set_attendance(&pool, entry.id, &[alice.id]).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post(format!("/plan/{date}/clear-meal"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header("X-Requested-With", "XMLHttpRequest")
                    .body(Body::from("week_start=2026-08-08"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let after = meal_plan::get_by_date(&pool, date).await?.expect("entry");
        assert_eq!(after.meal_id, None);
        assert_eq!(after.notes.as_deref(), Some("Family dinner"));
        assert_eq!(
            meal_plan::get_attendance(&pool, after.id).await?,
            vec![alice.id]
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Choose a meal"),
            "the picker should fall back to its placeholder: {html}"
        );
        assert!(
            html.contains("Family dinner"),
            "notes should still be on the card: {html}"
        );
        assert!(
            html.contains("Clear this day"),
            "a meal-less day is still configured, so the whole-day clear must \
             stay reachable: {html}"
        );
        assert!(
            !html.contains("meal-picker-clear"),
            "there is no meal left to remove: {html}"
        );

        Ok(())
    }

    /// A day whose meal was cleared is still a day, so the other fields must
    /// keep autosaving - otherwise the time the user just set silently reverts
    /// until they pick a meal again.
    #[sqlx::test(migrations = "./migrations")]
    async fn updating_a_day_with_a_blank_meal_id_keeps_its_other_fields(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        meal_plan::upsert_entry(&pool, date, tacos.id, None, None, None, &[]).await?;
        meal_plan::clear_meal(&pool, date).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let body = "week_start=2026-08-08&meal_id=&notes=Still+here&meal_time=20%3A15&\
                    duration_minutes=20";
        let response = app
            .oneshot(
                Request::post(format!("/plan/{date}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header("X-Requested-With", "XMLHttpRequest")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let after = meal_plan::get_by_date(&pool, date).await?.expect("entry");
        assert_eq!(after.meal_id, None);
        assert_eq!(after.notes.as_deref(), Some("Still here"));
        assert_eq!(
            after.start_time_override,
            Some(NaiveTime::from_hms_opt(20, 15, 0).unwrap())
        );
        assert!(after.deleted_at.is_none());

        Ok(())
    }

    /// "Suggest week" fills days that have no meal - including one whose meal
    /// the user deliberately removed, without discarding what they set up.
    #[sqlx::test(migrations = "./migrations")]
    async fn suggesting_the_week_fills_a_day_whose_meal_was_cleared(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        crate::db::meals::insert(&pool, "Pasta").await?;
        let today = chrono::Utc::now().date_naive();
        let date = today + Duration::days(1);
        let entry = meal_plan::upsert_entry(
            &pool,
            date,
            tacos.id,
            Some("bring dessert"),
            Some(NaiveTime::from_hms_opt(19, 30, 0).unwrap()),
            None,
            &[],
        )
        .await?;
        meal_plan::clear_meal(&pool, date).await?;

        let app = router().with_state(crate::state::test_app_state(pool.clone()));
        app.oneshot(
            Request::post("/plan/week/suggest")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("week_start={today}")))
                .unwrap(),
        )
        .await
        .unwrap();

        let after = meal_plan::get_by_date(&pool, date).await?.expect("entry");
        assert!(
            after.meal_id.is_some(),
            "suggest week should have refilled the day whose meal was cleared"
        );
        assert_eq!(after.notes.as_deref(), Some("bring dessert"));
        assert_eq!(
            after.start_time_override,
            Some(NaiveTime::from_hms_opt(19, 30, 0).unwrap())
        );
        assert_eq!(
            after.id, entry.id,
            "it should be the same day, not a new row"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn ajax_fragment_does_not_include_page_chrome(pool: PgPool) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let body = format!(
            "week_start=2026-08-08&meal_id={}&notes=&meal_time=18%3A30&duration_minutes=30",
            tacos.id
        );
        let response = app
            .oneshot(
                Request::post(format!("/plan/{date}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header("X-Requested-With", "XMLHttpRequest")
                    .body(Body::from(body))
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
            !html.contains("<nav"),
            "fragment should not include page nav: {html}"
        );
        assert!(
            !html.contains("</html>"),
            "fragment should not extend base.html: {html}"
        );
        assert!(
            !html.contains("<!DOCTYPE"),
            "fragment should not extend base.html: {html}"
        );

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
    async fn nav_always_shows_sync_link_regardless_of_ha_config(pool: PgPool) -> sqlx::Result<()> {
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
            html.contains(r#"href="/sync""#),
            "Sync nav link should always be visible: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn jump_to_upcoming_week_link_is_hidden_when_already_on_the_upcoming_week(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        // No `start` query param: show() falls back to the upcoming week, so
        // this is always the upcoming week regardless of when the test runs.
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
            !html.contains("Next unplanned week"),
            "jump-to-upcoming-week link should be hidden when already viewing it: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn jump_to_upcoming_week_link_is_shown_when_viewing_a_different_week(
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
            html.contains("Next unplanned week"),
            "jump-to-upcoming-week link should show when browsing a different week: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn suggesting_an_empty_day_fills_it_with_the_only_active_meal(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post(format!("/plan/{date}/suggest"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("week_start=2026-08-08"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let entry = meal_plan::get_by_date(&pool, date)
            .await?
            .expect("entry should exist");
        assert_eq!(entry.meal_id, Some(tacos.id));

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn rerolling_a_planned_day_picks_a_different_meal(pool: PgPool) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let current = crate::db::meals::insert(&pool, "Tacos").await?;
        let alternative = crate::db::meals::insert(&pool, "Curry").await?;
        crate::db::preferences::set(&pool, alice.id, current.id, Some("dislike")).await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let entry = meal_plan::upsert_entry(&pool, date, current.id, None, None, None, &[]).await?;
        meal_plan::set_attendance(&pool, entry.id, &[alice.id]).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        app.oneshot(
            Request::post(format!("/plan/{date}/suggest"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("week_start=2026-08-08"))
                .unwrap(),
        )
        .await
        .unwrap();

        let entry = meal_plan::get_by_date(&pool, date).await?.unwrap();
        assert_eq!(
            entry.meal_id,
            Some(alternative.id),
            "Alice dislikes the current meal, so reroll should pick the other one"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn rerolling_preserves_notes_and_time_and_duration_overrides(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let current = crate::db::meals::insert(&pool, "Tacos").await?;
        crate::db::meals::insert(&pool, "Curry").await?;
        crate::db::preferences::set(&pool, alice.id, current.id, Some("dislike")).await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let entry = meal_plan::upsert_entry(
            &pool,
            date,
            current.id,
            Some("Family dinner"),
            Some(NaiveTime::from_hms_opt(19, 0, 0).unwrap()),
            Some(45),
            &["Aunt Jane".to_string()],
        )
        .await?;
        meal_plan::set_attendance(&pool, entry.id, &[alice.id]).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        app.oneshot(
            Request::post(format!("/plan/{date}/suggest"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("week_start=2026-08-08"))
                .unwrap(),
        )
        .await
        .unwrap();

        let entry = meal_plan::get_by_date(&pool, date).await?.unwrap();
        assert_eq!(entry.notes.as_deref(), Some("Family dinner"));
        assert_eq!(
            entry.start_time_override,
            Some(NaiveTime::from_hms_opt(19, 0, 0).unwrap())
        );
        assert_eq!(entry.duration_minutes_override, Some(45));
        assert_eq!(entry.guest_names, vec!["Aunt Jane".to_string()]);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn filling_a_cleared_day_resets_attendance_to_current_defaults(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let stale_attendee = consumers::insert(&pool, "Alice").await?;
        let default_attendee = consumers::insert(&pool, "Bob").await?;
        consumers::set_default(&pool, default_attendee.id, true).await?;
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let old_entry =
            meal_plan::upsert_entry(&pool, date, tacos.id, None, None, None, &[]).await?;
        meal_plan::set_attendance(&pool, old_entry.id, &[stale_attendee.id]).await?;
        meal_plan::soft_delete(&pool, date).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        app.oneshot(
            Request::post(format!("/plan/{date}/suggest"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("week_start=2026-08-08"))
                .unwrap(),
        )
        .await
        .unwrap();

        let entry = meal_plan::get_by_date(&pool, date).await?.unwrap();
        let attendance = meal_plan::get_attendance(&pool, entry.id).await?;
        assert_eq!(
            attendance,
            vec![default_attendee.id],
            "a filled-from-cleared day should attend the current defaults, not the stale attendance"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn suggesting_via_ajax_returns_the_rerendered_fragment(pool: PgPool) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post(format!("/plan/{date}/suggest"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header("X-Requested-With", "XMLHttpRequest")
                    .body(Body::from("week_start=2026-08-08"))
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
            html.contains(&tacos.name),
            "fragment should show the suggested meal's name: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn suggesting_the_week_fills_empty_days_with_distinct_meals_when_enough_exist(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        crate::db::meals::insert(&pool, "Tacos").await?;
        crate::db::meals::insert(&pool, "Curry").await?;
        let today = chrono::Utc::now().date_naive();
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        app.oneshot(
            Request::post("/plan/week/suggest")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("week_start={today}")))
                .unwrap(),
        )
        .await
        .unwrap();

        let day0 = meal_plan::get_by_date(&pool, today).await?.unwrap();
        let day1 = meal_plan::get_by_date(&pool, today + Duration::days(1))
            .await?
            .unwrap();
        assert_ne!(
            day0.meal_id, day1.meal_id,
            "with two candidate meals available, consecutive days shouldn't repeat one"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn suggesting_the_week_leaves_an_already_planned_day_untouched(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        crate::db::meals::insert(&pool, "Curry").await?;
        let today = chrono::Utc::now().date_naive();
        meal_plan::upsert_entry(
            &pool,
            today,
            tacos.id,
            Some("already planned"),
            None,
            None,
            &[],
        )
        .await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        app.oneshot(
            Request::post("/plan/week/suggest")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("week_start={today}")))
                .unwrap(),
        )
        .await
        .unwrap();

        let day0 = meal_plan::get_by_date(&pool, today).await?.unwrap();
        assert_eq!(day0.meal_id, Some(tacos.id));
        assert_eq!(day0.notes.as_deref(), Some("already planned"));

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn suggesting_the_week_skips_days_before_today(pool: PgPool) -> sqlx::Result<()> {
        crate::db::meals::insert(&pool, "Tacos").await?;
        let today = chrono::Utc::now().date_naive();
        let past_week_start = today - Duration::days(10);
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        app.oneshot(
            Request::post("/plan/week/suggest")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("week_start={past_week_start}")))
                .unwrap(),
        )
        .await
        .unwrap();

        for offset in 0..7 {
            let date = past_week_start + Duration::days(offset);
            assert!(
                meal_plan::get_by_date(&pool, date).await?.is_none(),
                "day {date} is before today and should have been skipped"
            );
        }

        Ok(())
    }

    // Every value of every class/id attribute in the page, split into tokens.
    fn dom_hooks(html: &str) -> Vec<String> {
        let mut hooks = Vec::new();
        for attr in ["class=\"", "id=\""] {
            let mut rest = html;
            while let Some(start) = rest.find(attr) {
                rest = &rest[start + attr.len()..];
                let Some(end) = rest.find('"') else { break };
                hooks.extend(rest[..end].split_whitespace().map(str::to_owned));
                rest = &rest[end + 1..];
            }
        }
        hooks
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn plan_page_exposes_the_hooks_plan_js_binds_to(pool: PgPool) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        meal_plan::upsert_entry(&pool, date, tacos.id, None, None, None, &[]).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::get(format!("/plan?start={date}"))
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
        let hooks = dom_hooks(&html);

        // static/plan.js reaches the page entirely through these selectors and
        // the test fixture in tests/js/plan.test.js mirrors the same markup.
        // Renaming one silently breaks every card interaction while all the
        // other server tests keep passing, because nothing server-side looks
        // them up.
        for hook in [
            "plan-day",
            "plan-day-form",
            "plan-day-suggest-form",
            "plan-day-clear-form",
            "plan-day-clear-meal-form",
            "meal-picker",
            "meal-picker-trigger",
            "meal-picker-clear",
            "add-guest-btn",
            "meal-search-dialog",
            "meal-search-input",
            "meal-search-results",
            "meal-search-close",
        ] {
            assert!(
                hooks.iter().any(|h| h == hook),
                "no element carries the {hook:?} hook that plan.js binds to"
            );
        }

        // The card's stable identity. submitFormAjax keys request sequencing by
        // it and re-queries the live card with it after each response, so a
        // day whose card races two writes repaints the right element.
        assert!(
            html.contains(&format!("data-date=\"{date}\"")),
            "the day card needs data-date to be addressable after a repaint: {html}"
        );

        // How plan.js tells a day that exists from one that doesn't. It gates
        // both autosave and the blank-meal branch of submit, so dropping it
        // silently stops a cleared meal-less day from saving its other fields.
        assert!(
            html.contains("data-planned=\"true\""),
            "a planned day needs data-planned for plan.js to autosave it: {html}"
        );

        Ok(())
    }
}
