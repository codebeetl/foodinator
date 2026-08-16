use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::GcalError;

/// A created or updated Google Calendar event.
#[derive(Debug, Clone, Deserialize)]
pub struct GcalEvent {
    pub id: String,
    #[allow(dead_code)]
    pub summary: Option<String>,
}

#[derive(Serialize)]
struct EventPayload<'a> {
    summary: &'a str,
    description: &'a str,
    start: EventTime,
    end: EventTime,
}

#[derive(Serialize)]
struct EventTime {
    #[serde(rename = "dateTime")]
    date_time: String,
}

/// Minimal Google Calendar API v3 client. Holds a long-lived access token
/// (refreshed on demand by the sync layer) and the target calendar ID.
pub struct GcalClient {
    access_token: String,
    calendar_id: String,
    http: reqwest::Client,
}

impl GcalClient {
    pub fn new(access_token: String, calendar_id: String) -> Self {
        Self {
            access_token,
            calendar_id,
            http: reqwest::Client::new(),
        }
    }

    /// Replaces the current access token (used after a token refresh).
    pub fn set_access_token(&mut self, token: String) {
        self.access_token = token;
    }

    /// Create a new event on the calendar. Returns Google's event ID.
    pub async fn create_event(
        &self,
        summary: &str,
        description: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<GcalEvent, GcalError> {
        let payload = EventPayload {
            summary,
            description,
            start: EventTime {
                date_time: start.to_rfc3339(),
            },
            end: EventTime {
                date_time: end.to_rfc3339(),
            },
        };

        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events",
            urlencoding::encode(&self.calendar_id)
        );

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| GcalError::Api(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let message = body["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(GcalError::Api(format!("{status}: {message}")));
        }

        let event: GcalEvent = resp
            .json()
            .await
            .map_err(|e| GcalError::Api(e.to_string()))?;
        Ok(event)
    }

    /// Update an existing event by its Google event ID.
    pub async fn update_event(
        &self,
        event_id: &str,
        summary: &str,
        description: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<GcalEvent, GcalError> {
        let payload = EventPayload {
            summary,
            description,
            start: EventTime {
                date_time: start.to_rfc3339(),
            },
            end: EventTime {
                date_time: end.to_rfc3339(),
            },
        };

        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
            urlencoding::encode(&self.calendar_id),
            urlencoding::encode(event_id)
        );

        let resp = self
            .http
            .put(&url)
            .bearer_auth(&self.access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| GcalError::Api(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let message = body["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(GcalError::Api(format!("{status}: {message}")));
        }

        let event: GcalEvent = resp
            .json()
            .await
            .map_err(|e| GcalError::Api(e.to_string()))?;
        Ok(event)
    }

    /// Delete an event from the calendar by its Google event ID.
    pub async fn delete_event(&self, event_id: &str) -> Result<(), GcalError> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
            urlencoding::encode(&self.calendar_id),
            urlencoding::encode(event_id)
        );

        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(|e| GcalError::Api(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() && status.as_u16() != 404 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let message = body["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(GcalError::Api(format!("{status}: {message}")));
        }

        Ok(())
    }
}

/// A trait for the Google Calendar sync client, allowing tests to swap in
/// no-op or failing implementations without hitting the real API.
#[async_trait::async_trait]
pub trait GcalCalendarSync: Send + Sync {
    async fn create_event(
        &self,
        summary: &str,
        description: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<String, GcalError>;

    async fn update_event(
        &self,
        event_id: &str,
        summary: &str,
        description: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(), GcalError>;

    async fn delete_event(&self, event_id: &str) -> Result<(), GcalError>;
}

#[async_trait::async_trait]
impl GcalCalendarSync for GcalClient {
    async fn create_event(
        &self,
        summary: &str,
        description: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<String, GcalError> {
        let event = GcalClient::create_event(self, summary, description, start, end).await?;
        Ok(event.id)
    }

    async fn update_event(
        &self,
        event_id: &str,
        summary: &str,
        description: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(), GcalError> {
        GcalClient::update_event(self, event_id, summary, description, start, end).await?;
        Ok(())
    }

    async fn delete_event(&self, event_id: &str) -> Result<(), GcalError> {
        GcalClient::delete_event(self, event_id).await
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A test double that records calls but never hits the network.
    pub struct NoopGcalSync;

    #[async_trait::async_trait]
    impl GcalCalendarSync for NoopGcalSync {
        async fn create_event(
            &self,
            _summary: &str,
            _description: &str,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> Result<String, GcalError> {
            Ok("noop-event-id".to_string())
        }

        async fn update_event(
            &self,
            _event_id: &str,
            _summary: &str,
            _description: &str,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> Result<(), GcalError> {
            Ok(())
        }

        async fn delete_event(&self, _event_id: &str) -> Result<(), GcalError> {
            Ok(())
        }
    }

    /// A test double that always fails with a simulated error.
    pub struct FailingGcalSync {
        pub called: AtomicBool,
    }

    #[async_trait::async_trait]
    impl GcalCalendarSync for FailingGcalSync {
        async fn create_event(
            &self,
            _summary: &str,
            _description: &str,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> Result<String, GcalError> {
            self.called.store(true, Ordering::SeqCst);
            Err(GcalError::Api("simulated failure".into()))
        }

        async fn update_event(
            &self,
            _event_id: &str,
            _summary: &str,
            _description: &str,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> Result<(), GcalError> {
            self.called.store(true, Ordering::SeqCst);
            Err(GcalError::Api("simulated failure".into()))
        }

        async fn delete_event(&self, _event_id: &str) -> Result<(), GcalError> {
            self.called.store(true, Ordering::SeqCst);
            Err(GcalError::Api("simulated failure".into()))
        }
    }
}

// NOTE: urlencoding is not currently in Cargo.toml. We need to add it.
// For now, we'll use a simple manual URL encoding for the path segments.
mod urlencoding {
    use std::fmt::Write;

    pub fn encode(input: &str) -> String {
        let mut output = String::with_capacity(input.len() * 3);
        for byte in input.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    write!(output, "{}", byte as char).unwrap();
                }
                _ => {
                    write!(output, "%{byte:02X}").unwrap();
                }
            }
        }
        output
    }
}
