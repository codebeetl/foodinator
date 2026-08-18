pub mod client;
pub mod sync;

use serde::{Deserialize, Serialize};
use std::fmt;

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Google Calendar OAuth2 scopes needed for event management.
const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/calendar.readonly";

#[derive(Debug)]
pub enum GcalError {
    TokenExchange(String),
    TokenRefresh(String),
    /// Google rejected the refresh with `invalid_grant` - the refresh token
    /// itself has been revoked or expired, not a transient failure.
    TokenRevoked(String),
    Http(String),
    Api {
        status: u16,
        message: String,
    },
}

impl fmt::Display for GcalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GcalError::TokenExchange(msg) => write!(f, "token exchange failed: {msg}"),
            GcalError::TokenRefresh(msg) => write!(f, "token refresh failed: {msg}"),
            GcalError::TokenRevoked(msg) => {
                write!(f, "Google Calendar authorization was revoked: {msg}")
            }
            GcalError::Http(msg) => write!(f, "HTTP error: {msg}"),
            GcalError::Api { status, message } => {
                write!(f, "Google Calendar API error ({status}): {message}")
            }
        }
    }
}

impl std::error::Error for GcalError {}

/// A calendar entry returned from the Google Calendar API calendarList endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcalCalendarEntry {
    pub id: String,
    pub summary: String,
    #[serde(default)]
    pub is_primary: bool,
}

/// Tokens returned from the OAuth2 flow.
#[derive(Debug, Clone)]
pub struct GcalTokens {
    pub access_token: String,
    /// Only present on the initial consent; absent on subsequent refreshes.
    pub refresh_token: Option<String>,
    /// Seconds until the access token expires (typically 3600).
    #[allow(dead_code)]
    pub expires_in: u64,
}

/// Builds the Google OAuth2 authorization URL. The user should be redirected
/// to this URL to consent. When `device_id` and `device_name` are provided
/// they are appended (required by Google for private/internal redirect hosts).
pub fn build_auth_url(
    client_id: &str,
    redirect_uri: &str,
    device_id: Option<&str>,
    device_name: Option<&str>,
) -> String {
    let mut url = format!(
        "{GOOGLE_AUTH_URL}?\
         client_id={client_id}&\
         redirect_uri={redirect_uri}&\
         response_type=code&\
         scope={SCOPE}&\
         access_type=offline&\
         prompt=consent"
    );
    if let Some(id) = device_id {
        url.push_str(&format!("&device_id={id}"));
    }
    if let Some(name) = device_name {
        url.push_str(&format!("&device_name={name}"));
    }
    url
}

/// Exchange an authorization code for access + refresh tokens.
pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<GcalTokens, GcalError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| GcalError::TokenExchange(e.to_string()))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| GcalError::TokenExchange(e.to_string()))?;

    if !status.is_success() {
        let error = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(GcalError::TokenExchange(error.to_string()));
    }

    Ok(GcalTokens {
        access_token: body["access_token"]
            .as_str()
            .ok_or_else(|| GcalError::TokenExchange("missing access_token".into()))?
            .to_string(),
        refresh_token: body["refresh_token"].as_str().map(str::to_string),
        expires_in: body["expires_in"].as_u64().unwrap_or(3600),
    })
}

/// Refresh an expired access token using the stored refresh token.
/// `token_url_override` lets tests point this at a mock server instead of
/// Google's real token endpoint - production callers pass `None`.
pub async fn refresh_access_token(
    token_url_override: Option<&str>,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<String, GcalError> {
    let token_url = token_url_override.unwrap_or(GOOGLE_TOKEN_URL);
    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|e| GcalError::TokenRefresh(e.to_string()))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| GcalError::TokenRefresh(e.to_string()))?;

    if !status.is_success() {
        let error_code = body.get("error").and_then(|v| v.as_str());
        let message = body
            .get("error_description")
            .and_then(|v| v.as_str())
            .or(error_code)
            .unwrap_or("unknown error")
            .to_string();
        if error_code == Some("invalid_grant") {
            return Err(GcalError::TokenRevoked(message));
        }
        return Err(GcalError::TokenRefresh(message));
    }

    body["access_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| GcalError::TokenRefresh("missing access_token".into()))
}

/// Generate a random state token for CSRF protection on the OAuth2 callback.
pub fn generate_state_token() -> String {
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    let result = hasher.finalize();
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn refresh_access_token_reports_revoked_on_invalid_grant() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "Token has been expired or revoked.",
            })))
            .mount(&server)
            .await;

        let result = refresh_access_token(
            Some(&server.uri()),
            "client-id",
            "client-secret",
            "old-token",
        )
        .await;

        assert!(
            matches!(result, Err(GcalError::TokenRevoked(_))),
            "expected TokenRevoked, got {result:?}"
        );
    }

    #[tokio::test]
    async fn refresh_access_token_reports_plain_refresh_failure_on_other_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": "server_error",
            })))
            .mount(&server)
            .await;

        let result = refresh_access_token(
            Some(&server.uri()),
            "client-id",
            "client-secret",
            "old-token",
        )
        .await;

        assert!(
            matches!(result, Err(GcalError::TokenRefresh(_))),
            "a non-invalid_grant error should not be treated as revocation, got {result:?}"
        );
    }

    #[test]
    fn generate_state_token_produces_a_url_safe_string() {
        let token = generate_state_token();
        assert!(!token.is_empty());
        assert!(token.len() > 16, "should be a reasonable length");
        assert!(
            !token.contains('+') && !token.contains('/') && !token.contains('='),
            "URL-safe base64 should not contain +, /, or ="
        );
    }

    #[test]
    fn build_auth_url_contains_required_parameters() {
        let url = build_auth_url("my-client-id", "https://example.com/callback", None, None);
        assert!(url.contains("client_id=my-client-id"));
        assert!(url.contains("redirect_uri=https://example.com/callback"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
    }

    #[test]
    fn build_auth_url_includes_device_params_for_private_ip() {
        let url = build_auth_url(
            "my-client-id",
            "http://192.168.1.100:8000/callback",
            Some("192.168.1.100:8000"),
            Some("foodinator"),
        );
        assert!(url.contains("device_id=192.168.1.100:8000"));
        assert!(url.contains("device_name=foodinator"));
    }
}
