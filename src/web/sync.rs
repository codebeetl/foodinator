use askama::Template;
use axum::extract::State;
use axum::routing::get;
use axum::Router;
use chrono::{Duration, NaiveDate};

use crate::clock;
use crate::db::{settings, sync as sync_db};
use crate::ha::sync as ha_sync;
use crate::state::AppState;

/// Only entries within this many days are eligible to sync - see
/// ha::sync::is_within_sync_horizon for why (HA has no update service, so
/// most edits must happen before an entry is ever pushed).
const SYNC_HORIZON_DAYS: i64 = 14;

pub fn router() -> Router<AppState> {
    Router::new().route("/sync", get(show).post(run_sync))
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
    ha_configured: bool,
    display_configured: bool,
    theme: String,
    results: Option<SyncResults>,
}

async fn show(State(state): State<AppState>) -> SyncTemplate {
    SyncTemplate {
        ha_configured: state.ha_client().await.is_some(),
        display_configured: state.display_token.is_some(),
        theme: state.theme().await,
        results: None,
    }
}

async fn run_sync(State(state): State<AppState>) -> SyncTemplate {
    let Some(ha_client) = state.ha_client().await else {
        return SyncTemplate {
            ha_configured: false,
            display_configured: state.display_token.is_some(),
            theme: state.theme().await,
            results: None,
        };
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
        let start_time = candidate
            .start_time_override
            .unwrap_or(app_settings.default_start_time);
        let duration_minutes = candidate
            .duration_minutes_override
            .unwrap_or(app_settings.default_duration_minutes);

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

    SyncTemplate {
        ha_configured: true,
        display_configured: state.display_token.is_some(),
        theme: app_settings.theme,
        results: Some(SyncResults { synced, failed }),
    }
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
    async fn show_and_run_sync_render_a_not_configured_notice_when_ha_is_disabled(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let mut state = crate::state::test_app_state(pool);
        state.ha_env_url = None;
        state.ha_env_token = None;
        state.ha_env_calendar_entity_id = None;
        let app = router().with_state(state);

        let get_response = app
            .clone()
            .oneshot(Request::get("/sync").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !html.contains("Sync to Home Assistant</button>"),
            "the trigger button should be hidden when HA isn't configured: {html}"
        );

        let post_response = app
            .oneshot(Request::post("/sync").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            post_response.status(),
            StatusCode::OK,
            "posting while disabled should render the notice, not error"
        );

        Ok(())
    }
}
