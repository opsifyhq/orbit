use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::auth::oidc_public_config;
use crate::dto::{MetaFeatures, MetaResponse};
use crate::state::AppState;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const API_VERSION: &str = "v1";

#[utoipa::path(get, path = "/healthz", tag = "system", responses((status = 200, description = "Liveness probe")))]
pub async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

#[utoipa::path(get, path = "/readyz", tag = "system", responses((status = 200, description = "Dependencies are ready"), (status = 503, description = "A dependency is unavailable")))]
pub async fn readyz(State(state): State<AppState>) -> Response {
    let db = state.store.health().await;
    let bus = state.bus.health().await;
    let checks = json!({
        "database": if db.is_ok() { "ok" } else { "unavailable" },
        "eventBus": if bus.is_ok() { "ok" } else { "unavailable" },
        "eventBusDriver": state.events_driver,
    });
    // PostgreSQL is the source of truth; the bus is an accelerator (AGENTS.md rule 5) and
    // nothing in the API consumes it, so a bus outage must not remove pods from the
    // Service. It is reported as degraded but does not gate readiness.
    let ready = db.is_ok();
    // Dependency error text can name hosts, users or schema; it goes to the log only,
    // never to the unauthenticated caller.
    if let Err(err) = &db {
        tracing::error!(error = %err, "readiness check failed: database");
    }
    if let Err(err) = &bus {
        tracing::warn!(error = %err, "readiness check degraded: event bus");
    }
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let overall = if ready { "ready" } else { "degraded" };
    (
        status,
        Json(json!({
            "status": overall,
            "checks": checks,
            "degraded": if bus.is_ok() { json!([]) } else { json!(["eventBus"]) },
        })),
    )
        .into_response()
}

#[utoipa::path(get, path = "/metrics", tag = "system", responses((status = 200, description = "Prometheus metrics")))]
pub async fn metrics(State(state): State<AppState>) -> Response {
    let mut extra = std::collections::BTreeMap::new();
    extra.insert("version".to_string(), VERSION.to_string());
    extra.insert("environment".to_string(), state.environment.clone());
    let body = state.metrics.render(&extra);
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}

#[utoipa::path(get, path = "/api/v1/meta", tag = "system", responses((status = 200, body = MetaResponse)))]
pub async fn meta(State(state): State<AppState>) -> Json<MetaResponse> {
    Json(MetaResponse {
        version: VERSION.to_string(),
        api_version: API_VERSION.to_string(),
        environment: state.environment.clone(),
        auth: oidc_public_config(&state),
        events_driver: state.events_driver.clone(),
        license: state.license_status(),
        docs_url: crate::docs::DOCS_URL.to_string(),
        features: MetaFeatures {
            harness_execution: true,
            connectors_invocation: true,
            approvals: true,
            scheduled_triggers: true,
            webhook_triggers: false,
            oidc: state.auth.oidc.is_some(),
            api_tokens: state.auth.allow_api_tokens,
        },
    })
}
