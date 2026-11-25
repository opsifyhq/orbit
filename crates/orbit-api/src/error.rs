use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use orbit_domain::{DomainError, ValidationIssue};
use orbit_security::SecurityError;
use orbit_store::StoreError;

use crate::request_id;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Vec<ValidationIssue>,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub details: Vec<ValidationIssue>,
    /// Caused by a storage failure a retry can clear (#436). Not serialized; lets callers
    /// keep work queued instead of failing it for good.
    transient: bool,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: Vec::new(),
            transient: false,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn validation(issues: Vec<ValidationIssue>) -> Self {
        let message = issues
            .first()
            .map(|issue| format!("{}: {}", issue.path, issue.message))
            .unwrap_or_else(|| "validation failed".to_string());
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "validation_error",
            message,
            details: issues,
            transient: false,
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_IMPLEMENTED, "unsupported", message)
    }

    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Whether a retry may succeed: the error came from a storage failure such as a pool
    /// timeout or a dropped connection, not from the request or the stored data.
    pub fn is_transient(&self) -> bool {
        self.transient
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::error!(code = self.code, message = %self.message, "request failed");
        } else {
            tracing::debug!(code = self.code, message = %self.message, "request rejected");
        }
        let body = ErrorEnvelope {
            error: ErrorBody {
                code: self.code.to_string(),
                message: self.message,
                details: self.details,
                request_id: request_id::current(),
            },
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::NotFound(what) => ApiError::not_found(what),
            StoreError::Conflict(message) => ApiError::conflict(message),
            StoreError::Invalid(message) => ApiError::bad_request(message),
            StoreError::Database(err) => {
                if let Some(db) = err.as_database_error() {
                    if db.is_foreign_key_violation() {
                        let constraint = db.constraint().unwrap_or("foreign key");
                        return ApiError::conflict(format!(
                            "this item is still referenced by {constraint}; remove the reference before deleting it"
                        ));
                    }
                }
                let error = StoreError::Database(err);
                tracing::error!(error = %error, "store error");
                ApiError {
                    transient: error.is_transient(),
                    ..ApiError::internal("a storage error occurred")
                }
            }
            other => {
                tracing::error!(error = %other, "store error");
                ApiError::internal("a storage error occurred")
            }
        }
    }
}

impl From<DomainError> for ApiError {
    fn from(err: DomainError) -> Self {
        match err {
            DomainError::Validation(report) => ApiError::validation(report.issues),
            DomainError::NotFound(what) => ApiError::not_found(what),
            DomainError::Conflict(message) => ApiError::conflict(message),
            DomainError::Forbidden(message) => ApiError::forbidden(message),
            DomainError::InvalidTransition { from, to } => {
                ApiError::conflict(format!("invalid transition {from} -> {to}"))
            }
            DomainError::Invalid(message) => ApiError::bad_request(message),
            DomainError::Internal(message) => {
                tracing::error!(error = %message, "domain error");
                ApiError::internal("an internal error occurred")
            }
        }
    }
}

impl From<SecurityError> for ApiError {
    fn from(err: SecurityError) -> Self {
        match err {
            SecurityError::Unauthorized => ApiError::unauthorized("authentication required"),
            SecurityError::Forbidden(message) => ApiError::forbidden(message),
            SecurityError::Unavailable(message) => {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
            }
            other => {
                tracing::error!(error = %other, "security error");
                ApiError::internal("a security error occurred")
            }
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        ApiError::bad_request(format!("invalid JSON body: {err}"))
    }
}

impl From<axum::extract::rejection::JsonRejection> for ApiError {
    fn from(err: axum::extract::rejection::JsonRejection) -> Self {
        // Malformed JSON is 400; structurally valid JSON that fails typed
        // deserialization is a validation error (422).
        let code = if err.status() == StatusCode::UNPROCESSABLE_ENTITY {
            "validation_error"
        } else {
            "bad_request"
        };
        ApiError::new(err.status(), code, err.body_text())
    }
}

/// Maximum number of bytes of a foreign plain-text error body copied into the envelope.
const FOREIGN_BODY_LIMIT: usize = 1024;

pub(crate) fn code_for_status(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        405 => "method_not_allowed",
        406 => "not_acceptable",
        408 => "request_timeout",
        409 => "conflict",
        413 => "payload_too_large",
        414 => "uri_too_long",
        415 => "unsupported_media_type",
        422 => "validation_error",
        429 => "rate_limited",
        501 => "unsupported",
        502 => "bad_gateway",
        503 => "unavailable",
        504 => "gateway_timeout",
        s if (400..500).contains(&s) => "bad_request",
        _ => "internal",
    }
}

/// Central safety net for the error envelope (AGENTS.md rule 9).
///
/// Extractor rejections (`Path`, `Query`, body limits), unmatched routes and
/// method mismatches are produced by axum/tower as plain text or empty bodies.
/// This middleware rewrites any 4xx/5xx response that is not already JSON into
/// the standard envelope. It leaves successful responses, redirects, 304,
/// streams, `HEAD` responses, already-JSON errors and the Swagger UI untouched.
/// Headers such as `Allow` are preserved.
pub async fn envelope_rejections(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;

    let status = response.status();
    if !(status.is_client_error() || status.is_server_error())
        || method == axum::http::Method::HEAD
        || path == "/docs"
        || path.starts_with("/docs/")
    {
        return response;
    }
    let is_json = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("json"));
    if is_json {
        return response;
    }

    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, FOREIGN_BODY_LIMIT)
        .await
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).trim().to_string();
    let message = if text.is_empty() {
        status
            .canonical_reason()
            .unwrap_or("request failed")
            .to_string()
    } else {
        text
    };

    let mut rewritten = ApiError::new(status, code_for_status(status), message).into_response();
    for (name, value) in parts.headers.iter() {
        if name == axum::http::header::CONTENT_TYPE || name == axum::http::header::CONTENT_LENGTH {
            continue;
        }
        rewritten.headers_mut().append(name.clone(), value.clone());
    }
    rewritten
}

/// Router fallback for unmatched routes.
pub async fn not_found_fallback() -> ApiError {
    ApiError::not_found("no such route")
}

#[cfg(test)]
mod transient_tests {
    use super::*;

    #[test]
    fn storage_outages_are_transient_and_keep_the_generic_message() {
        let error = ApiError::from(StoreError::Database(sqlx::Error::PoolTimedOut));
        assert!(error.is_transient());
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.code, "internal");
        assert_eq!(error.message, "a storage error occurred");
        assert!(error.clone().is_transient());
    }

    #[test]
    fn missing_rows_and_request_errors_are_not_transient() {
        for error in [
            ApiError::from(StoreError::NotFound("agent".into())),
            ApiError::from(StoreError::Conflict("x".into())),
            ApiError::from(StoreError::Internal("cipher".into())),
            ApiError::from(StoreError::Database(sqlx::Error::RowNotFound)),
            ApiError::internal("stored agent spec is invalid"),
            ApiError::conflict("harness 'mock' is disabled"),
        ] {
            assert!(!error.is_transient(), "{error}");
        }
    }
}
