use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::{CalendarSync, HaError, HaEvent};

pub struct HaRestClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
    calendar_entity_id: String,
}

impl HaRestClient {
    pub fn new(base_url: String, token: String, calendar_entity_id: String) -> Self {
        HaRestClient {
            http: reqwest::Client::new(),
            base_url,
            token,
            calendar_entity_id,
        }
    }
}

/// HA's `calendar.create_event` service treats a datetime string with no
/// offset as being in HA's own configured local timezone, not UTC - so a
/// bare "%Y-%m-%d %H:%M:%S" here (as this used to format) gets silently
/// reinterpreted as local time and re-shifted by HA's UTC offset on top of
/// the household-to-UTC conversion already applied upstream. RFC3339 keeps
/// the offset explicit so HA parses it as the UTC instant it actually is.
fn ha_datetime(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

#[derive(Debug, Deserialize)]
struct RawHaEvent {
    uid: Option<String>,
    summary: String,
    #[serde(default)]
    description: Option<String>,
}

impl From<RawHaEvent> for HaEvent {
    fn from(raw: RawHaEvent) -> Self {
        HaEvent {
            uid: raw.uid,
            summary: raw.summary,
            description: raw.description,
        }
    }
}

async fn ok_or_status(response: reqwest::Response) -> Result<reqwest::Response, HaError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(HaError::Status(response.status()))
    }
}

#[async_trait]
impl CalendarSync for HaRestClient {
    async fn get_api_status(&self) -> Result<(), HaError> {
        let response = self
            .http
            .get(format!("{}/api/", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await?;
        ok_or_status(response).await?;
        Ok(())
    }

    async fn create_event(
        &self,
        summary: &str,
        description: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(), HaError> {
        let body = serde_json::json!({
            "entity_id": self.calendar_entity_id,
            "summary": summary,
            "description": description,
            "start_date_time": ha_datetime(start),
            "end_date_time": ha_datetime(end),
        });
        let response = self
            .http
            .post(format!(
                "{}/api/services/calendar/create_event",
                self.base_url
            ))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await?;
        ok_or_status(response).await?;
        Ok(())
    }

    async fn list_events(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<HaEvent>, HaError> {
        let url = format!(
            "{}/api/calendars/{}?start={}&end={}",
            self.base_url,
            self.calendar_entity_id,
            start.to_rfc3339(),
            end.to_rfc3339()
        );
        let response = self.http.get(url).bearer_auth(&self.token).send().await?;
        let response = ok_or_status(response).await?;
        let raw: Vec<RawHaEvent> = response.json().await?;
        Ok(raw.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> HaRestClient {
        HaRestClient::new(
            server.uri(),
            "test-token".to_string(),
            "calendar.foodinator".to_string(),
        )
    }

    #[test]
    fn ha_datetime_includes_an_explicit_utc_offset() {
        let dt = DateTime::parse_from_rfc3339("2026-08-10T18:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            ha_datetime(dt),
            "2026-08-10T18:00:00+00:00",
            "a bare offset-less string would be reinterpreted by HA as its own local timezone"
        );
    }

    #[tokio::test]
    async fn get_api_status_sends_bearer_token_and_succeeds_on_200() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let result = client_for(&server).get_api_status().await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_api_status_errors_on_non_success_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let result = client_for(&server).get_api_status().await;

        assert!(matches!(result, Err(HaError::Status(status)) if status == 401));
    }

    #[tokio::test]
    async fn create_event_posts_expected_service_call_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/calendar/create_event"))
            .and(body_partial_json(serde_json::json!({
                "entity_id": "calendar.foodinator",
                "summary": "Alice's dinner",
                "start_date_time": "2026-08-10T18:00:00+00:00",
                "end_date_time": "2026-08-10T19:00:00+00:00",
            })))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let start = DateTime::parse_from_rfc3339("2026-08-10T18:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let end = DateTime::parse_from_rfc3339("2026-08-10T19:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let result = client_for(&server)
            .create_event("Alice's dinner", "foodinator:entry=1", start, end)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn list_events_parses_uid_and_description_from_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/calendars/calendar.foodinator"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"uid": "abc123", "summary": "Alice's dinner", "description": "foodinator:entry=1"}
            ])))
            .mount(&server)
            .await;

        let events = client_for(&server)
            .list_events(Utc::now(), Utc::now())
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].uid.as_deref(), Some("abc123"));
        assert_eq!(events[0].description.as_deref(), Some("foodinator:entry=1"));
    }
}
