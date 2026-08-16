use askama::Template;
use axum::extract::{Form, Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::db::consumers::{self, Consumer};
use crate::state::{AppState, PageContext};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/consumers", get(list).post(create))
        .route("/consumers/{id}", axum::routing::post(update))
}

#[derive(Template)]
#[template(path = "consumers/list.html")]
struct ConsumersListTemplate {
    consumers: Vec<Consumer>,
    ctx: PageContext,
}

impl IntoResponse for ConsumersListTemplate {
    fn into_response(self) -> Response {
        super::render_askama_template(self)
    }
}

async fn list(State(state): State<AppState>) -> ConsumersListTemplate {
    let consumers = consumers::list_all(&state.pool)
        .await
        .expect("failed to list consumers");
    ConsumersListTemplate {
        consumers,
        ctx: state.page_context().await,
    }
}

#[derive(Deserialize)]
struct NewConsumerForm {
    name: String,
}

async fn create(State(state): State<AppState>, Form(form): Form<NewConsumerForm>) -> Redirect {
    consumers::insert(&state.pool, &form.name)
        .await
        .expect("failed to insert consumer");
    Redirect::to("/consumers")
}

#[derive(Deserialize)]
struct UpdateConsumerForm {
    // Checkboxes are absent from form data when unchecked, so this must be optional.
    is_default: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<UpdateConsumerForm>,
) -> Redirect {
    consumers::set_default(&state.pool, id, form.is_default.is_some())
        .await
        .expect("failed to update consumer");
    Redirect::to("/consumers")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    #[sqlx::test(migrations = "./migrations")]
    async fn adding_a_consumer_through_the_form_makes_it_appear_in_the_list(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let app = router().with_state(crate::state::test_app_state(pool));

        let create_response = app
            .clone()
            .oneshot(
                Request::post("/consumers")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("name=Alice"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::SEE_OTHER);

        let list_response = app
            .oneshot(Request::get("/consumers").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Alice"),
            "list page should show the new consumer: {html}"
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn marking_a_consumer_default_through_the_form_persists_it(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        let response = app
            .oneshot(
                Request::post(format!("/consumers/{}", alice.id))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("is_default=on"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let all = consumers::list_all(&pool).await?;
        assert!(all[0].is_default);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn omitting_the_checkbox_clears_default(pool: PgPool) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        consumers::set_default(&pool, alice.id, true).await?;
        let app = router().with_state(crate::state::test_app_state(pool.clone()));

        app.oneshot(
            Request::post(format!("/consumers/{}", alice.id))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();

        let all = consumers::list_all(&pool).await?;
        assert!(!all[0].is_default);

        Ok(())
    }
}
