pub mod client;
pub mod sync;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

pub use client::HaRestClient;

#[derive(Debug, Clone, PartialEq)]
pub struct HaEvent {
    pub uid: Option<String>,
    pub summary: String,
    pub description: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum HaError {
    #[error("request to Home Assistant failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Home Assistant returned status {0}")]
    Status(reqwest::StatusCode),
}

#[async_trait]
pub trait CalendarSync: Send + Sync {
    async fn get_api_status(&self) -> Result<(), HaError>;

    async fn create_event(
        &self,
        summary: &str,
        description: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(), HaError>;

    async fn list_events(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<HaEvent>, HaError>;
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    /// A CalendarSync that always succeeds, for tests that need an AppState but don't
    /// exercise HA connectivity themselves.
    pub struct NoopCalendarSync;

    #[async_trait]
    impl CalendarSync for NoopCalendarSync {
        async fn get_api_status(&self) -> Result<(), HaError> {
            Ok(())
        }

        async fn create_event(
            &self,
            _summary: &str,
            _description: &str,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> Result<(), HaError> {
            Ok(())
        }

        async fn list_events(
            &self,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> Result<Vec<HaEvent>, HaError> {
            Ok(vec![])
        }
    }

    /// A CalendarSync whose create_event always fails, for testing the sync
    /// job's failure path.
    pub struct FailingCalendarSync;

    #[async_trait]
    impl CalendarSync for FailingCalendarSync {
        async fn get_api_status(&self) -> Result<(), HaError> {
            Err(HaError::Status(reqwest::StatusCode::SERVICE_UNAVAILABLE))
        }

        async fn create_event(
            &self,
            _summary: &str,
            _description: &str,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> Result<(), HaError> {
            Err(HaError::Status(reqwest::StatusCode::SERVICE_UNAVAILABLE))
        }

        async fn list_events(
            &self,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> Result<Vec<HaEvent>, HaError> {
            Ok(vec![])
        }
    }
}
