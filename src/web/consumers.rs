use askama::Template;
use axum::extract::{Form, State};
use axum::response::Redirect;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::db::consumers::{self, Consumer};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/consumers", get(list).post(create))
}

#[derive(Template)]
#[template(path = "consumers/list.html")]
struct ConsumersListTemplate {
    consumers: Vec<Consumer>,
}

async fn list(State(state): State<AppState>) -> ConsumersListTemplate {
    let consumers = consumers::list_all(&state.pool)
        .await
        .expect("failed to list consumers");
    ConsumersListTemplate { consumers }
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
}
