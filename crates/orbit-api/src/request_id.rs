use axum::extract::{MatchedPath, Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

use crate::state::AppState;

pub const HEADER: &str = "x-request-id";

tokio::task_local! {
    static REQUEST_ID: String;
}

pub fn current() -> String {
    REQUEST_ID.try_with(|id| id.clone()).unwrap_or_default()
}

/// Generates/propagates a request id, opens a tracing span and records metrics.
pub async fn instrument(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let incoming = request
        .headers()
        .get(HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .filter(|value| is_acceptable_request_id(value));
    let request_id = incoming.unwrap_or_else(|| Uuid::now_v7().to_string());

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let span = tracing::info_span!(
        "http.request",
        http.method = %method,
        http.path = %path,
        request.id = %request_id
    );

    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    // The route template keeps the label set bounded; a request that matched no route
    // (the 404 fallback) is grouped instead of labelled with its caller-chosen path.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let metric_method = method.as_str().to_string();
    let started = std::time::Instant::now();
    let in_flight = InFlight::start(state.metrics.clone());

    let response = REQUEST_ID
        .scope(request_id.clone(), async move {
            let _guard = span.enter();
            next.run(request).await
        })
        .await;

    in_flight.finish(
        &metric_method,
        &route,
        response.status().as_u16(),
        started.elapsed(),
    );

    let mut response = response;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(HEADER, value);
    }
    response
}

/// A caller-supplied request id is echoed in the response header and in error bodies, so
/// only plain ids are kept: 1 to 128 characters of letters, digits, `.`, `_` and `-`.
/// Anything else (quotes, angle brackets, spaces) is replaced by a generated id (#443).
fn is_acceptable_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::is_acceptable_request_id;

    #[test]
    fn only_plain_request_ids_are_kept() {
        for ok in ["0199-abc", "trace.id_42", "a"] {
            assert!(is_acceptable_request_id(ok), "{ok}");
        }
        let long = "x".repeat(129);
        for bad in ["", "inj\"ect", "<script>", "a b", "a/b", long.as_str()] {
            assert!(!is_acceptable_request_id(bad), "{bad}");
        }
    }
}

/// Keeps `orbit_http_in_flight_requests` right when a request is dropped before it is
/// answered (client disconnect, shutdown): the guard then releases the slot unrecorded.
struct InFlight {
    metrics: Option<std::sync::Arc<crate::metrics::Metrics>>,
}

impl InFlight {
    fn start(metrics: std::sync::Arc<crate::metrics::Metrics>) -> Self {
        metrics.http_started();
        Self {
            metrics: Some(metrics),
        }
    }

    fn finish(mut self, method: &str, route: &str, status: u16, elapsed: std::time::Duration) {
        if let Some(metrics) = self.metrics.take() {
            metrics.http_finished(method, route, status, elapsed);
        }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(metrics) = self.metrics.take() {
            metrics.http_abandoned();
        }
    }
}

/// Extracted request id.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl<S> axum::extract::FromRequestParts<S> for RequestId
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<RequestId>()
            .cloned()
            .unwrap_or_else(|| RequestId(current())))
    }
}
