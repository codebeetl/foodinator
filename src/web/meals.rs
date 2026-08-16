use std::collections::HashMap;

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::db::meals::{self, Meal};
use crate::db::preferences::{self, ConsumerPreference};
use crate::state::{AppState, PageContext};

const PREFERENCE_FIELD_PREFIX: &str = "preference_";

/// Results are capped for the search picker - no session tracks usage
/// frequency, so this is purely a "don't render a huge list" limit.
const SEARCH_RESULT_LIMIT: i64 = 10;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/meals", get(list).post(create))
        .route("/meals/{id}", get(edit_form).post(update))
        .route("/meals/{id}/delete", axum::routing::post(delete))
        .route(
            "/meals/{id}/preferences",
            axum::routing::post(update_preferences),
        )
        .route("/api/meals", get(search).post(create_api))
}

/// One meal plus its per-consumer preferences, pre-grouped for the list
/// page's collapsible column - `likes`/`dislikes` are counted here rather
/// than in the template to keep the template free of aggregation logic.
struct MealRow {
    meal: Meal,
    preferences: Vec<ConsumerPreference>,
    likes: i64,
    dislikes: i64,
    // Whether this row's preferences <details> should render open - computed
    // here (rather than compared against `open` in the template) since
    // Askama's expression syntax doesn't support the `*deref` this comparison
    // would otherwise need.
    is_open: bool,
    // Raw date kept alongside the formatted string below so the list can be
    // sorted on it (None sorts before every date, which is exactly "never
    // planned" ranking as most overdue) without re-parsing the display text.
    last_planned_date: Option<NaiveDate>,
    // Pre-formatted (ISO date, or "Never") rather than a raw NaiveDate, again
    // to keep formatting logic out of the template.
    last_planned: String,
}

#[derive(Template)]
#[template(path = "meals/list.html")]
struct MealsListTemplate {
    rows: Vec<MealRow>,
    duplicate: bool,
    ctx: PageContext,
    sort: String,
    dir: String,
}

impl IntoResponse for MealsListTemplate {
    fn into_response(self) -> Response {
        super::render_askama_template(self)
    }
}

#[derive(Deserialize)]
struct ListQuery {
    // Which row's preferences <details> should render open, e.g. right after
    // saving it - otherwise the list page always starts fully collapsed.
    open: Option<i64>,
    // Set after a rejected duplicate-name submission - the client-side
    // autocomplete on the add-meal form already blocks the common case, this
    // is the server-side backstop (meals.name is UNIQUE) for anyone who gets
    // past it (JS disabled, a name added in another tab moments earlier).
    #[serde(default)]
    duplicate: bool,
    // "name" | "status" | "preferences" | "last_planned" - anything else
    // (including absent) falls back to the default name sort.
    sort: Option<String>,
    // "asc" | "desc" - anything other than exactly "desc" is treated as
    // ascending, matching each column's documented ascending order.
    dir: Option<String>,
}

async fn list(State(state): State<AppState>, Query(query): Query<ListQuery>) -> MealsListTemplate {
    let meals = meals::list_all(&state.pool)
        .await
        .expect("failed to list meals");
    let pref_rows = preferences::list_for_all_meals(&state.pool)
        .await
        .expect("failed to list preferences");
    let mut last_planned_dates = meals::last_planned_dates(&state.pool)
        .await
        .expect("failed to list last-planned dates");

    let mut prefs_by_meal: HashMap<i64, Vec<ConsumerPreference>> = HashMap::new();
    for row in pref_rows {
        prefs_by_meal
            .entry(row.meal_id)
            .or_default()
            .push(ConsumerPreference {
                consumer_id: row.consumer_id,
                consumer_name: row.consumer_name,
                preference: row.preference,
            });
    }

    let mut rows: Vec<MealRow> = meals
        .into_iter()
        .map(|meal| {
            let preferences = prefs_by_meal.remove(&meal.id).unwrap_or_default();
            let likes = preferences
                .iter()
                .filter(|p| p.preference.as_deref() == Some("like"))
                .count() as i64;
            let dislikes = preferences
                .iter()
                .filter(|p| p.preference.as_deref() == Some("dislike"))
                .count() as i64;
            let is_open = query.open == Some(meal.id);
            let last_planned_date = last_planned_dates.remove(&meal.id);
            let last_planned = last_planned_date
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "Never".to_string());
            MealRow {
                meal,
                preferences,
                likes,
                dislikes,
                is_open,
                last_planned_date,
                last_planned,
            }
        })
        .collect();

    let sort = query.sort.unwrap_or_else(|| "name".to_string());
    match sort.as_str() {
        "status" => rows.sort_by(|a, b| {
            std::cmp::Reverse(a.meal.active)
                .cmp(&std::cmp::Reverse(b.meal.active))
                .then_with(|| a.meal.name.cmp(&b.meal.name))
        }),
        "preferences" => rows.sort_by(|a, b| {
            b.likes
                .cmp(&a.likes)
                .then_with(|| a.meal.name.cmp(&b.meal.name))
        }),
        "last_planned" => rows.sort_by(|a, b| {
            a.last_planned_date
                .cmp(&b.last_planned_date)
                .then_with(|| a.meal.name.cmp(&b.meal.name))
        }),
        _ => rows.sort_by(|a, b| a.meal.name.cmp(&b.meal.name)),
    }
    let ascending = query.dir.as_deref() != Some("desc");
    if !ascending {
        rows.reverse();
    }

    MealsListTemplate {
        rows,
        sort,
        dir: if ascending {
            "asc".to_string()
        } else {
            "desc".to_string()
        },
        duplicate: query.duplicate,
        ctx: state.page_context().await,
    }
}

#[derive(Deserialize)]
struct NewMealForm {
    name: String,
}

async fn create(State(state): State<AppState>, Form(form): Form<NewMealForm>) -> Redirect {
    match meals::insert(&state.pool, &form.name).await {
        Ok(_) => Redirect::to("/meals"),
        Err(err)
            if err
                .as_database_error()
                .is_some_and(|e| e.is_unique_violation()) =>
        {
            Redirect::to("/meals?duplicate=true")
        }
        Err(err) => panic!("failed to insert meal: {err}"),
    }
}

#[derive(Template)]
#[template(path = "meals/edit.html")]
struct MealEditTemplate {
    meal: Meal,
    preferences: Vec<ConsumerPreference>,
    // Only set right after a failed delete attempt - meal_plan_entries.meal_id
    // is ON DELETE RESTRICT, so a meal that's ever been planned can't be
    // deleted outright.
    delete_blocked_by_plan_history: bool,
    // Only set right after a rename collided with another meal's name -
    // meals.name is UNIQUE, so this is the server-side backstop for anyone
    // who renames to a name that already exists.
    name_conflict: bool,
    ctx: PageContext,
}

impl IntoResponse for MealEditTemplate {
    fn into_response(self) -> Response {
        super::render_askama_template(self)
    }
}

#[derive(Deserialize)]
struct EditQuery {
    #[serde(default)]
    delete_blocked: bool,
    #[serde(default)]
    duplicate: bool,
}

async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EditQuery>,
) -> Result<MealEditTemplate, StatusCode> {
    let meal = meals::get(&state.pool, id)
        .await
        .expect("failed to fetch meal")
        .ok_or(StatusCode::NOT_FOUND)?;
    let preferences = preferences::list_for_meal(&state.pool, id)
        .await
        .expect("failed to list preferences");
    Ok(MealEditTemplate {
        meal,
        preferences,
        delete_blocked_by_plan_history: query.delete_blocked,
        name_conflict: query.duplicate,
        ctx: state.page_context().await,
    })
}

async fn delete(State(state): State<AppState>, Path(id): Path<i64>) -> Redirect {
    match meals::delete(&state.pool, id).await {
        Ok(()) => Redirect::to("/meals"),
        Err(err)
            if err
                .as_database_error()
                .is_some_and(|e| e.is_foreign_key_violation()) =>
        {
            Redirect::to(&format!("/meals/{id}?delete_blocked=true"))
        }
        Err(err) => panic!("failed to delete meal: {err}"),
    }
}

#[derive(Deserialize)]
struct UpdateMealForm {
    name: String,
    // Checkboxes are absent from form data when unchecked, so this must be optional.
    active: Option<String>,
    // Catches this meal's dynamic per-consumer `preference_<consumer_id>` fields,
    // since the set of consumers isn't known at compile time.
    #[serde(flatten)]
    preferences: HashMap<String, String>,
}

/// Applies every `preference_<consumer_id>` field found in a submitted form
/// to the given meal - shared by the full edit form and the meals list's
/// per-row preferences form, which both submit the same field naming.
async fn apply_preference_fields(
    pool: &sqlx::PgPool,
    meal_id: i64,
    fields: &HashMap<String, String>,
) {
    for (field, value) in fields {
        let Some(consumer_id) = field
            .strip_prefix(PREFERENCE_FIELD_PREFIX)
            .and_then(|s| s.parse::<i64>().ok())
        else {
            continue;
        };
        let preference = if value.is_empty() {
            None
        } else {
            Some(value.as_str())
        };
        preferences::set(pool, consumer_id, meal_id, preference)
            .await
            .expect("failed to set preference");
    }
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<UpdateMealForm>,
) -> Redirect {
    match meals::update(&state.pool, id, &form.name, form.active.is_some()).await {
        Ok(_) => {}
        Err(err)
            if err
                .as_database_error()
                .is_some_and(|e| e.is_unique_violation()) =>
        {
            return Redirect::to(&format!("/meals/{id}?duplicate=true"));
        }
        Err(err) => panic!("failed to update meal: {err}"),
    }
    apply_preference_fields(&state.pool, id, &form.preferences).await;
    Redirect::to("/meals")
}

#[derive(Deserialize)]
struct UpdatePreferencesForm {
    #[serde(flatten)]
    preferences: HashMap<String, String>,
}

async fn update_preferences(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<UpdatePreferencesForm>,
) -> Redirect {
    apply_preference_fields(&state.pool, id, &form.preferences).await;
    Redirect::to(&format!("/meals?open={id}#meal-{id}"))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    // Comma-separated consumer ids, e.g. "1,2" - simpler and less ambiguous
    // than relying on repeated-key array deserialization for a query string.
    attendee_ids: Option<String>,
}

fn parse_attendee_ids(raw: Option<&str>) -> Vec<i64> {
    raw.unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect()
}

/// A search result plus its last-planned date, formatted the same way as
/// the meals list page's "Last planned" column - kept as a separate JSON DTO
/// rather than a field on `MealSuitability` itself, since that struct is
/// shared with `suitability_for_attendees` (used only to resolve a selected
/// meal's name in `build_plan_day`), which has no use for it.
#[derive(Serialize, Deserialize)]
struct SearchResult {
    id: i64,
    name: String,
    liked_by_attendee: bool,
    disliked_by_attendee_names: Vec<String>,
    last_planned: String,
}

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Json<Vec<SearchResult>> {
    let attendee_ids = parse_attendee_ids(query.attendee_ids.as_deref());
    let q = query.q.as_deref().unwrap_or("").trim();

    let results = if q.is_empty() {
        meals::list_top(&state.pool, &attendee_ids, SEARCH_RESULT_LIMIT).await
    } else {
        meals::search(&state.pool, q, &attendee_ids, SEARCH_RESULT_LIMIT).await
    }
    .expect("failed to search meals");

    let mut last_planned_dates = meals::last_planned_dates(&state.pool)
        .await
        .expect("failed to list last-planned dates");

    let results = results
        .into_iter()
        .map(|m| SearchResult {
            last_planned: last_planned_dates
                .remove(&m.id)
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "Never".to_string()),
            id: m.id,
            name: m.name,
            liked_by_attendee: m.liked_by_attendee,
            disliked_by_attendee_names: m.disliked_by_attendee_names,
        })
        .collect();

    Json(results)
}

#[derive(Deserialize)]
struct CreateMealApiForm {
    name: String,
}

#[derive(Serialize, Deserialize)]
struct CreatedMeal {
    id: i64,
    name: String,
}

async fn create_api(
    State(state): State<AppState>,
    Json(form): Json<CreateMealApiForm>,
) -> Json<CreatedMeal> {
    let meal = meals::insert(&state.pool, &form.name)
        .await
        .expect("failed to insert meal");
    Json(CreatedMeal {
        id: meal.id,
        name: meal.name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    #[sqlx::test(migrations = "./migrations")]
    async fn adding_a_meal_through_the_form_makes_it_appear_in_the_list(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool));

        let create_response = app
            .clone()
            .oneshot(
                Request::post("/meals")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spaghetti+Bolognese"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::SEE_OTHER);

        let list_response = app
            .oneshot(Request::get("/meals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Spaghetti Bolognese"),
            "list page should show the new meal: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_page_heading_shows_the_total_meal_count(pool: PgPool) -> sqlx::Result<()> {
        meals::insert(&pool, "Tacos").await?;
        meals::insert(&pool, "Pizza").await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/meals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Meals (2)"),
            "heading should show the total meal count: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn default_list_order_is_alphabetical_by_name(pool: PgPool) -> sqlx::Result<()> {
        meals::insert(&pool, "Zucchini Bake").await?;
        meals::insert(&pool, "Apple Pie").await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/meals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.find(">Apple Pie<").unwrap() < html.find(">Zucchini Bake<").unwrap(),
            "default order should be alphabetical by name: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sorting_by_status_puts_active_meals_before_inactive_when_ascending(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        meals::insert(&pool, "Active Meal").await?;
        let inactive = meals::insert(&pool, "Inactive Meal").await?;
        meals::update(&pool, inactive.id, &inactive.name, false).await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(
                Request::get("/meals?sort=status")
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
            html.find(">Active Meal<").unwrap() < html.find(">Inactive Meal<").unwrap(),
            "ascending status sort should list active meals first: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sorting_by_preferences_puts_most_liked_meals_first_when_ascending(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let liked = meals::insert(&pool, "Liked Meal").await?;
        meals::insert(&pool, "Unliked Meal").await?;
        let consumer = crate::db::consumers::insert(&pool, "Alex").await?;
        preferences::set(&pool, consumer.id, liked.id, Some("like")).await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(
                Request::get("/meals?sort=preferences")
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
            html.find(">Liked Meal<").unwrap() < html.find(">Unliked Meal<").unwrap(),
            "ascending preferences sort should list most-liked meals first: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sorting_by_last_planned_puts_never_planned_meals_first_when_ascending(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let planned = meals::insert(&pool, "Planned Meal").await?;
        meals::insert(&pool, "Never Planned Meal").await?;
        let today = chrono::Utc::now().date_naive();
        crate::db::meal_plan::upsert_entry(&pool, today, planned.id, None, None, None, &[]).await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(
                Request::get("/meals?sort=last_planned")
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
            html.find(">Never Planned Meal<").unwrap() < html.find(">Planned Meal<").unwrap(),
            "ascending last-planned sort should list never-planned meals first: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dir_desc_reverses_the_current_sort_order(pool: PgPool) -> sqlx::Result<()> {
        meals::insert(&pool, "Apple Pie").await?;
        meals::insert(&pool, "Zucchini Bake").await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(
                Request::get("/meals?sort=name&dir=desc")
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
            html.find(">Zucchini Bake<").unwrap() < html.find(">Apple Pie<").unwrap(),
            "dir=desc should reverse the ascending order: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn submitting_a_duplicate_meal_name_redirects_with_a_notice_instead_of_erroring(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        meals::insert(&pool, "Tacos").await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .clone()
            .oneshot(
                Request::post("/meals")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("name=Tacos"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/meals?duplicate=true"
        );

        let all = meals::list_all(&pool).await?;
        assert_eq!(all.len(), 1, "the duplicate must not be inserted");

        let list_response = app
            .oneshot(
                Request::get("/meals?duplicate=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("already exists"),
            "the duplicate-name notice should render: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn deactivating_a_meal_through_its_edit_page_persists_and_redirects(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let created = meals::insert(&pool, "Tacos").await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let update_response = app
            .oneshot(
                Request::post(format!("/meals/{}", created.id))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("name=Fish+Tacos"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update_response.status(), StatusCode::SEE_OTHER);

        let updated = meals::get(&pool, created.id).await?.expect("meal exists");
        assert_eq!(updated.name, "Fish Tacos");
        assert!(!updated.active, "omitting the checkbox should deactivate");

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn setting_a_preference_through_the_edit_form_persists_it(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = crate::db::consumers::insert(&pool, "Alice").await?;
        let meal = meals::insert(&pool, "Tacos").await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let body = format!("name=Tacos&active=true&preference_{}=dislike", alice.id);
        let update_response = app
            .oneshot(
                Request::post(format!("/meals/{}", meal.id))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update_response.status(), StatusCode::SEE_OTHER);

        let prefs = preferences::list_for_meal(&pool, meal.id).await?;
        assert_eq!(prefs[0].preference.as_deref(), Some("dislike"));

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn deleting_an_unplanned_meal_removes_it_and_redirects_to_the_list(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let meal = meals::insert(&pool, "Tacos").await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post(format!("/meals/{}/delete", meal.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/meals");

        assert_eq!(meals::get(&pool, meal.id).await?, None);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn deleting_a_meal_with_plan_history_is_blocked_and_shows_a_notice(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let meal = meals::insert(&pool, "Tacos").await?;
        crate::db::meal_plan::upsert_entry(
            &pool,
            chrono::NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
            meal.id,
            None,
            None,
            None,
            &[],
        )
        .await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/meals/{}/delete", meal.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            &format!("/meals/{}?delete_blocked=true", meal.id)
        );
        assert!(
            meals::get(&pool, meal.id).await?.is_some(),
            "blocked delete must not remove the meal"
        );

        let edit_response = app
            .oneshot(
                Request::get(format!("/meals/{}?delete_blocked=true", meal.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(edit_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Can&#x27;t delete") || html.contains("Can't delete"),
            "the blocked-delete notice should render: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn renaming_a_meal_to_an_existing_name_is_blocked_and_shows_a_notice(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        meals::insert(&pool, "Tacos").await?;
        let other = meals::insert(&pool, "Burritos").await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/meals/{}", other.id))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("name=Tacos&active=true"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            &format!("/meals/{}?duplicate=true", other.id)
        );
        assert_eq!(
            meals::get(&pool, other.id).await?.unwrap().name,
            "Burritos",
            "a blocked rename must not change the meal's name"
        );

        let edit_response = app
            .oneshot(
                Request::get(format!("/meals/{}?duplicate=true", other.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(edit_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Can&#x27;t save") || html.contains("Can't save"),
            "the name-conflict notice should render: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_page_shows_a_preferences_column_with_like_and_dislike_counts(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = crate::db::consumers::insert(&pool, "Alice").await?;
        let bob = crate::db::consumers::insert(&pool, "Bob").await?;
        let meal = meals::insert(&pool, "Tacos").await?;
        preferences::set(&pool, alice.id, meal.id, Some("like")).await?;
        preferences::set(&pool, bob.id, meal.id, Some("dislike")).await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/meals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Likes: 1") && html.contains("Dislikes: 1"),
            "list page should summarise preference counts: {html}"
        );
        assert!(
            html.contains(&format!("preference_{}", alice.id))
                && html.contains(&format!("preference_{}", bob.id)),
            "list page should include an editable field per consumer: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn saving_preferences_from_the_list_page_persists_and_redirects_back_open(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = crate::db::consumers::insert(&pool, "Alice").await?;
        let meal = meals::insert(&pool, "Tacos").await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let body = format!("preference_{}=like", alice.id);
        let response = app
            .oneshot(
                Request::post(format!("/meals/{}/preferences", meal.id))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(
            location,
            format!("/meals?open={}#meal-{}", meal.id, meal.id)
        );

        let prefs = preferences::list_for_meal(&pool, meal.id).await?;
        assert_eq!(prefs[0].preference.as_deref(), Some("like"));

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn search_endpoint_ranks_matches_and_falls_back_to_alphabetical_when_empty(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        meals::insert(&pool, "Tacos").await?;
        meals::insert(&pool, "Taco Salad").await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .clone()
            .oneshot(
                Request::get("/api/meals?q=Tacos")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let results: Vec<SearchResult> = serde_json::from_slice(&body).unwrap();
        assert_eq!(results[0].name, "Tacos", "exact match ranks first");

        let empty_query_response = app
            .oneshot(Request::get("/api/meals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(empty_query_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(empty_query_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let results: Vec<SearchResult> = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            results.iter().map(|m| &m.name).collect::<Vec<_>>(),
            vec!["Taco Salad", "Tacos"],
            "no query should list everything alphabetically"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn search_endpoint_includes_each_meals_last_planned_date(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let tacos = meals::insert(&pool, "Tacos").await?;
        meals::insert(&pool, "Pasta").await?;
        let today = chrono::Utc::now().date_naive();
        crate::db::meal_plan::upsert_entry(&pool, today, tacos.id, None, None, None, &[]).await?;
        let app = router().with_state(crate::state::test_app_state(pool));

        let response = app
            .oneshot(Request::get("/api/meals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let results: Vec<SearchResult> = serde_json::from_slice(&body).unwrap();

        let pasta = results.iter().find(|m| m.name == "Pasta").unwrap();
        assert_eq!(pasta.last_planned, "Never");
        let tacos_result = results.iter().find(|m| m.name == "Tacos").unwrap();
        assert_eq!(
            tacos_result.last_planned,
            today.format("%Y-%m-%d").to_string()
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_api_inserts_a_meal_and_returns_it_as_json(pool: PgPool) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post("/api/meals")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"Chili"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: CreatedMeal = serde_json::from_slice(&body).unwrap();
        assert_eq!(created.name, "Chili");

        let fetched = meals::get(&pool, created.id).await?.expect("meal exists");
        assert_eq!(fetched.name, "Chili");

        Ok(())
    }
}
