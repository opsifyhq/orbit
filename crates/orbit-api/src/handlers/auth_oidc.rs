//! Browser OIDC helpers: discovery plus Authorization Code + PKCE token exchange.
//!
//! The browser never sees the client secret. Dex and Keycloak both expose standard
//! discovery and token endpoints, so the flow is provider-neutral.

use std::time::Duration;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcWebConfig {
    pub issuer: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub authorization_endpoint: String,
    pub end_session_endpoint: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcTokenRequest {
    pub code: String,
    pub code_verifier: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcTokenResponse {
    pub access_token: Option<String>,
    pub id_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: Option<String>,
}

fn oidc(state: &AppState) -> Result<&orbit_security::OidcConfig, ApiError> {
    state
        .auth
        .oidc
        .as_ref()
        .ok_or_else(|| ApiError::not_found("OIDC authentication"))
}

/// How long a discovery document is served from memory.
const DISCOVERY_TTL: Duration = Duration::from_secs(600);
/// Minimum time between two fetches from the identity provider, including after a failure.
/// These endpoints are unauthenticated, so this bounds the traffic they can cause.
const DISCOVERY_MIN_REFRESH: Duration = Duration::from_secs(15);

#[derive(Default)]
struct DiscoveryCache {
    document: Option<Value>,
    fetched: Option<std::time::Instant>,
    last_attempt: Option<std::time::Instant>,
}

static DISCOVERY_CACHE: tokio::sync::Mutex<DiscoveryCache> =
    tokio::sync::Mutex::const_new(DiscoveryCache {
        document: None,
        fetched: None,
        last_attempt: None,
    });

/// Cached discovery document. The lock is held across a refresh so concurrent callers
/// share one fetch; a failed refresh keeps serving the previous document.
async fn cached_discovery<F, Fut>(
    cache: &tokio::sync::Mutex<DiscoveryCache>,
    ttl: Duration,
    min_refresh: Duration,
    fetch: F,
) -> Result<Value, ApiError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Value, ApiError>>,
{
    let mut guard = cache.lock().await;
    if let (Some(document), Some(fetched)) = (&guard.document, guard.fetched) {
        if fetched.elapsed() < ttl {
            return Ok(document.clone());
        }
    }
    let may_refresh = guard
        .last_attempt
        .is_none_or(|at| at.elapsed() >= min_refresh);
    if may_refresh {
        guard.last_attempt = Some(std::time::Instant::now());
        match fetch().await {
            Ok(document) => {
                guard.document = Some(document.clone());
                guard.fetched = Some(std::time::Instant::now());
                return Ok(document);
            }
            Err(error) => {
                if let Some(stale) = &guard.document {
                    tracing::warn!(error = %error.message, "OIDC discovery refresh failed; serving cached document");
                    return Ok(stale.clone());
                }
                return Err(error);
            }
        }
    }
    match &guard.document {
        Some(stale) => Ok(stale.clone()),
        None => Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "oidc_discovery_failed",
            "OIDC discovery is unavailable; retry shortly",
        )),
    }
}

async fn discovery(issuer: &str) -> Result<Value, ApiError> {
    cached_discovery(
        &DISCOVERY_CACHE,
        DISCOVERY_TTL,
        DISCOVERY_MIN_REFRESH,
        || fetch_discovery(issuer),
    )
    .await
}

async fn fetch_discovery(issuer: &str) -> Result<Value, ApiError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let response = orbit_security::client_with_custom_ca(Duration::from_secs(10))
        .get(&url)
        .send()
        .await
        .map_err(|error| {
            ApiError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "oidc_discovery_failed",
                format!("could not reach OIDC discovery: {error}"),
            )
        })?;
    if !response.status().is_success() {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "oidc_discovery_failed",
            format!("OIDC discovery returned {}", response.status()),
        ));
    }
    response
        .json()
        .await
        .map_err(|error| ApiError::internal(format!("invalid OIDC discovery: {error}")))
}

/// Public sign-in configuration for the browser's Authorization Code + PKCE flow.
#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/config",
    tag = "auth",
    security(()),
    responses(
        (status = 200, body = OidcWebConfig),
        (status = 404, description = "OIDC is not enabled"),
        (status = 502, description = "The identity provider's discovery document could not be read")
    )
)]
pub async fn config(State(state): State<AppState>) -> Result<Json<OidcWebConfig>, ApiError> {
    let config = oidc(&state)?;
    let document = discovery(&config.issuer).await?;
    let authorization_endpoint = document
        .get("authorization_endpoint")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::internal("OIDC discovery has no authorization_endpoint"))?
        .to_string();
    Ok(Json(OidcWebConfig {
        issuer: config.issuer.clone(),
        client_id: config.client_id.clone(),
        redirect_uri: config.redirect_uri.clone(),
        scopes: config.scopes.clone(),
        authorization_endpoint,
        end_session_endpoint: document
            .get("end_session_endpoint")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

/// Exchange an authorization code for tokens. The client secret stays on the server.
#[utoipa::path(
    post,
    path = "/api/v1/auth/oidc/token",
    tag = "auth",
    security(()),
    request_body = OidcTokenRequest,
    responses(
        (status = 200, body = OidcTokenResponse),
        (status = 400, description = "Missing code or verifier"),
        (status = 502, description = "The identity provider rejected or failed the exchange")
    )
)]
pub async fn token(
    State(state): State<AppState>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<OidcTokenRequest>,
) -> Result<Json<OidcTokenResponse>, ApiError> {
    let config = oidc(&state)?;
    if body.code.trim().is_empty() || body.code_verifier.trim().is_empty() {
        return Err(ApiError::bad_request("code and codeVerifier are required"));
    }
    let document = discovery(&config.issuer).await?;
    let token_endpoint = document
        .get("token_endpoint")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::internal("OIDC discovery has no token_endpoint"))?;
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", body.code),
        ("redirect_uri", config.redirect_uri.clone()),
        ("client_id", config.client_id.clone()),
        ("code_verifier", body.code_verifier),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let response = orbit_security::client_with_custom_ca(Duration::from_secs(20))
        .post(token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|error| {
            ApiError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "oidc_token_failed",
                format!("token exchange failed: {error}"),
            )
        })?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    if !status.is_success() {
        let message = value
            .get("error_description")
            .or_else(|| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("OIDC token exchange failed");
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "oidc_token_failed",
            message,
        ));
    }
    Ok(Json(OidcTokenResponse {
        access_token: value
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        id_token: value
            .get("id_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        refresh_token: value
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_in: value.get("expires_in").and_then(Value::as_u64),
        token_type: value
            .get("token_type")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn failing() -> ApiError {
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "oidc_discovery_failed",
            "down",
        )
    }

    #[tokio::test]
    async fn discovery_is_cached_and_survives_a_failed_refresh() {
        let cache = tokio::sync::Mutex::new(DiscoveryCache::default());
        let calls = AtomicUsize::new(0);
        let doc = serde_json::json!({ "token_endpoint": "https://idp/token" });
        for _ in 0..50 {
            let result = cached_discovery(&cache, Duration::from_secs(60), Duration::ZERO, || {
                calls.fetch_add(1, Ordering::SeqCst);
                let doc = doc.clone();
                async move { Ok(doc) }
            })
            .await
            .unwrap();
            assert_eq!(result, doc);
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fresh cache must not refetch"
        );

        // Expired (ttl zero): refresh fails, the cached copy is still served.
        let served = cached_discovery(&cache, Duration::ZERO, Duration::ZERO, || async {
            Err(failing())
        })
        .await
        .unwrap();
        assert_eq!(served, doc);
    }

    #[tokio::test]
    async fn failed_discovery_is_not_retried_within_the_refresh_interval() {
        let cache = tokio::sync::Mutex::new(DiscoveryCache::default());
        let calls = AtomicUsize::new(0);
        for _ in 0..20 {
            let result = cached_discovery(
                &cache,
                Duration::from_secs(60),
                Duration::from_secs(60),
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Err(failing()) }
                },
            )
            .await;
            assert!(result.is_err());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
