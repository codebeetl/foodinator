use std::collections::HashMap;

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::response::Redirect;
use axum::routing::{get, post};
use axum::Router;
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Weekday};
use serde::Deserialize;

use crate::clock;
use crate::db::consumers::{self, Consumer};
use crate::db::meal_plan::{self, MealSuitability};
use crate::state::AppState;

const ATTENDEE_FIELD_PREFIX: &str = "attendee_";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plan", get(show))
        .route("/plan/:date", post(update))
        .route("/plan/:date/delete", post(delete))
}

/// The nearest upcoming Saturday, inclusive of today.
fn next_saturday(today: NaiveDate) -> NaiveDate {
    let days_ahead = (Weekday::Sat.num_days_from_monday() as i64
        - today.weekday().num_days_from_monday() as i64)
        .rem_euclid(7);
    today + Duration::days(days_ahead)
}

struct PlanDay {
    date: NaiveDate,
    date_label: String,
    has_entry: bool,
    selected_meal_id: Option<i64>,
    notes: String,
    start_time_override: Option<NaiveTime>,
    duration_minutes_override: Option<i32>,
    attendee_ids: Vec<i64>,
    meals: Vec<MealSuitability>,
}

impl PlanDay {
    fn attends(&self, consumer_id: &i64) -> bool {
        self.attendee_ids.contains(consumer_id)
    }

    fn meal_is_selected(&self, meal_id: &i64) -> bool {
        self.selected_meal_id == Some(*meal_id)
    }
}

#[derive(Template)]
#[template(path = "plan.html")]
struct PlanTemplate {
    week_start: NaiveDate,
    prev_start: NaiveDate,
    next_start: NaiveDate,
    days: Vec<PlanDay>,
    consumers: Vec<Consumer>,
}

#[derive(Deserialize)]
struct PlanQuery {
    start: Option<NaiveDate>,
}

async fn show(State(state): State<AppState>, Query(query): Query<PlanQuery>) -> PlanTemplate {
    let week_start = query
        .start
        .unwrap_or_else(|| next_saturday(clock::today(&state.household_tz)));

    let consumers: Vec<Consumer> = consumers::list_all(&state.pool)
        .await
        .expect("failed to list consumers")
        .into_iter()
        .filter(|c| c.active)
        .collect();

    let mut days = Vec::with_capacity(7);
    for offset in 0..7 {
        let date = week_start + Duration::days(offset);
        let live_entry = meal_plan::get_by_date(&state.pool, date)
            .await
            .expect("failed to fetch plan entry")
            .filter(|entry| entry.deleted_at.is_none());

        let attendee_ids = match &live_entry {
            Some(entry) => meal_plan::get_attendance(&state.pool, entry.id)
                .await
                .expect("failed to fetch attendance"),
            None => Vec::new(),
        };
        let meals = meal_plan::suitability_for_attendees(&state.pool, &attendee_ids)
            .await
            .expect("failed to compute suitability");

        days.push(PlanDay {
            date,
            date_label: date.format("%A, %-d %B").to_string(),
            has_entry: live_entry.is_some(),
            selected_meal_id: live_entry.as_ref().map(|entry| entry.meal_id),
            notes: live_entry
                .as_ref()
                .and_then(|entry| entry.notes.clone())
                .unwrap_or_default(),
            start_time_override: live_entry
                .as_ref()
                .and_then(|entry| entry.start_time_override),
            duration_minutes_override: live_entry
                .as_ref()
                .and_then(|entry| entry.duration_minutes_override),
            attendee_ids,
            meals,
        });
    }

    PlanTemplate {
        week_start,
        prev_start: week_start - Duration::days(7),
        next_start: week_start + Duration::days(7),
        days,
        consumers,
    }
}

#[derive(Deserialize)]
struct UpdatePlanForm {
    week_start: NaiveDate,
    meal_id: i64,
    notes: String,
    start_time_override: String,
    duration_minutes_override: String,
    // Catches this day's dynamic per-consumer `attendee_<consumer_id>` checkboxes,
    // since the set of consumers isn't known at compile time.
    #[serde(flatten)]
    attendees: HashMap<String, String>,
}

fn non_empty(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

async fn update(
    State(state): State<AppState>,
    Path(date): Path<NaiveDate>,
    Form(form): Form<UpdatePlanForm>,
) -> Redirect {
    let notes = non_empty(&form.notes);
    let start_time_override = non_empty(&form.start_time_override)
        .map(|s| s.parse::<NaiveTime>().expect("invalid start time override"));
    let duration_minutes_override = non_empty(&form.duration_minutes_override)
        .map(|s| s.parse::<i32>().expect("invalid duration override"));

    let entry = meal_plan::upsert_entry(
        &state.pool,
        date,
        form.meal_id,
        notes,
        start_time_override,
        duration_minutes_override,
    )
    .await
    .expect("failed to upsert plan entry");

    let attendee_ids: Vec<i64> = form
        .attendees
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

    #[sqlx::test(migrations = "./migrations")]
    async fn planning_a_day_through_the_form_persists_meal_and_attendance(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let body = format!(
            "week_start=2026-08-08&meal_id={}&notes=Family+dinner&start_time_override=19%3A00&\
             duration_minutes_override=45&attendee_{}=on",
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

        let attendance = meal_plan::get_attendance(&pool, entry.id).await?;
        assert_eq!(attendance, vec![alice.id]);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn blank_optional_fields_clear_notes_and_overrides(pool: PgPool) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        meal_plan::upsert_entry(
            &pool,
            date,
            tacos.id,
            Some("old notes"),
            Some(NaiveTime::from_hms_opt(19, 0, 0).unwrap()),
            Some(45),
        )
        .await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let body = format!(
            "week_start=2026-08-08&meal_id={}&notes=&start_time_override=&\
             duration_minutes_override=",
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

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn clearing_a_day_soft_deletes_it(pool: PgPool) -> sqlx::Result<()> {
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        meal_plan::upsert_entry(&pool, date, tacos.id, None, None, None).await?;
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
    async fn show_page_flags_disliked_meals_for_the_days_persisted_attendees(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let tacos = crate::db::meals::insert(&pool, "Tacos").await?;
        crate::db::preferences::set(&pool, alice.id, tacos.id, Some("dislike")).await?;
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let entry = meal_plan::upsert_entry(&pool, date, tacos.id, None, None, None).await?;
        meal_plan::set_attendance(&pool, entry.id, &[alice.id]).await?;

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
        assert!(
            html.contains("disliked by an attendee"),
            "page should flag the disliked meal for the persisted attendee: {html}"
        );

        Ok(())
    }
}
