use std::collections::HashMap;

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::StatusCode;
use axum::response::Redirect;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::db::meal_plan::MealSuitability;
use crate::db::meals::{self, Meal};
use crate::db::preferences::{self, ConsumerPreference};
use crate::state::AppState;

const PREFERENCE_FIELD_PREFIX: &str = "preference_";

/// Results are capped for the search picker - no session tracks usage
/// frequency, so this is purely a "don't render a huge list" limit.
const SEARCH_RESULT_LIMIT: i64 = 10;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/meals", get(list).post(create))
        .route("/meals/:id", get(edit_form).post(update))
        .route("/api/meals", get(search).post(create_api))
}

#[derive(Template)]
#[template(path = "meals/list.html")]
struct MealsListTemplate {
    meals: Vec<Meal>,
    ha_configured: bool,
}

async fn list(State(state): State<AppState>) -> MealsListTemplate {
    let meals = meals::list_all(&state.pool)
        .await
        .expect("failed to list meals");
    MealsListTemplate {
        meals,
        ha_configured: state.ha_client().await.is_some(),
    }
}

#[derive(Deserialize)]
struct NewMealForm {
    name: String,
}

async fn create(State(state): State<AppState>, Form(form): Form<NewMealForm>) -> Redirect {
    meals::insert(&state.pool, &form.name)
        .await
        .expect("failed to insert meal");
    Redirect::to("/meals")
}

#[derive(Template)]
#[template(path = "meals/edit.html")]
struct MealEditTemplate {
    meal: Meal,
    preferences: Vec<ConsumerPreference>,
    ha_configured: bool,
}

async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
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
        ha_configured: state.ha_client().await.is_some(),
    })
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

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<UpdateMealForm>,
) -> Redirect {
    meals::update(&state.pool, id, &form.name, form.active.is_some())
        .await
        .expect("failed to update meal");

    for (field, value) in &form.preferences {
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
        preferences::set(&state.pool, consumer_id, id, preference)
            .await
            .expect("failed to set preference");
    }

    Redirect::to("/meals")
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

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Json<Vec<MealSuitability>> {
    let attendee_ids = parse_attendee_ids(query.attendee_ids.as_deref());
    let q = query.q.as_deref().unwrap_or("").trim();

    let results = if q.is_empty() {
        meals::list_top(&state.pool, &attendee_ids, SEARCH_RESULT_LIMIT).await
    } else {
        meals::search(&state.pool, q, &attendee_ids, SEARCH_RESULT_LIMIT).await
    }
    .expect("failed to search meals");

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
        let results: Vec<MealSuitability> = serde_json::from_slice(&body).unwrap();
        assert_eq!(results[0].name, "Tacos", "exact match ranks first");

        let empty_query_response = app
            .oneshot(Request::get("/api/meals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(empty_query_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(empty_query_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let results: Vec<MealSuitability> = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            results.iter().map(|m| &m.name).collect::<Vec<_>>(),
            vec!["Taco Salad", "Tacos"],
            "no query should list everything alphabetically"
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
