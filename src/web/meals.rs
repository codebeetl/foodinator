use std::collections::HashMap;

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::Redirect;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::db::meals::{self, Meal};
use crate::db::preferences::{self, ConsumerPreference};
use crate::state::AppState;

const PREFERENCE_FIELD_PREFIX: &str = "preference_";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/meals", get(list).post(create))
        .route("/meals/:id", get(edit_form).post(update))
}

#[derive(Template)]
#[template(path = "meals/list.html")]
struct MealsListTemplate {
    meals: Vec<Meal>,
}

async fn list(State(state): State<AppState>) -> MealsListTemplate {
    let meals = meals::list_all(&state.pool)
        .await
        .expect("failed to list meals");
    MealsListTemplate { meals }
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
    Ok(MealEditTemplate { meal, preferences })
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
}
