use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::auth::oidc_public_config;
use crate::dto::{MetaFeatures, MetaResponse};
use crate::metrics::{GaugeSample, LICENSE_PHASES};
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

/// Prometheus scrape endpoint. Open unless `ORBIT_METRICS_TOKEN` is set, in which case
/// the scraper must send it as a bearer token.
#[utoipa::path(get, path = "/metrics", tag = "system", responses((status = 200, description = "Prometheus metrics in the text exposition format (0.0.4)", content_type = "text/plain"), (status = 401, description = "A metrics token is configured and the request did not present it")))]
pub async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(expected) = &state.metrics_access.token {
        if !bearer_matches(&headers, expected) {
            let mut response =
                crate::error::ApiError::unauthorized("a metrics token is required").into_response();
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
            return response;
        }
    }
    state
        .metrics
        .set_rate_limited(state.rate_limiter.rejected());
    let gauges = collect_gauges(&state).await;
    let body = state.metrics.render(&gauges);
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}

/// Compare the presented bearer token with the configured one. Both are hashed first,
/// so the comparison takes the same time whatever the presented value is.
fn bearer_matches(headers: &HeaderMap, expected: &str) -> bool {
    use sha2::{Digest, Sha256};
    let Some(presented) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
        })
    else {
        return false;
    };
    Sha256::digest(presented.trim().as_bytes()) == Sha256::digest(expected.as_bytes())
}

/// Current-state gauges: build info, license, connection pool, harness registry and the
/// database-backed counts. None of them carries an organization, user or resource name.
async fn collect_gauges(state: &AppState) -> Vec<GaugeSample> {
    let mut gauges = vec![GaugeSample::new(
        "orbit_info",
        &[("version", VERSION), ("environment", &state.environment)],
        1.0,
    )];

    let license = state.license_status();
    let phase = crate::background::license_phase(&license);
    for known in LICENSE_PHASES {
        gauges.push(GaugeSample::new(
            "orbit_license_phase",
            &[("phase", known)],
            if known == phase { 1.0 } else { 0.0 },
        ));
    }
    for resource in ["seats", "agents", "workers", "executions_per_day"] {
        if let Some(limit) = license.limit(resource) {
            gauges.push(GaugeSample::new(
                "orbit_license_limit",
                &[("resource", resource)],
                limit as f64,
            ));
        }
    }

    let (open, idle) = state.store.pool_stats();
    gauges.push(GaugeSample::new(
        "orbit_db_pool_connections",
        &[("state", "idle")],
        idle as f64,
    ));
    gauges.push(GaugeSample::new(
        "orbit_db_pool_connections",
        &[("state", "active")],
        open.saturating_sub(idle) as f64,
    ));

    let mut harnesses = state.registry.descriptors();
    harnesses.sort_by_key(|descriptor| descriptor.kind.as_str());
    for descriptor in harnesses {
        let available =
            descriptor.implementation_status == orbit_domain::ImplementationStatus::Available;
        gauges.push(GaugeSample::new(
            "orbit_harness_available",
            &[("kind", descriptor.kind.as_str())],
            if available { 1.0 } else { 0.0 },
        ));
    }

    if let Some(snapshot) = state
        .metrics
        .snapshot(&state.store, crate::background::HEARTBEAT_STALE_SECONDS)
        .await
    {
        for (status, count) in &snapshot.executions_in_flight {
            gauges.push(GaugeSample::new(
                "orbit_executions_in_flight",
                &[("status", status)],
                *count as f64,
            ));
        }
        gauges.push(GaugeSample::new(
            "orbit_workers_online",
            &[],
            snapshot.workers_online as f64,
        ));
        for (status, count) in &snapshot.worker_launches {
            gauges.push(GaugeSample::new(
                "orbit_worker_launches",
                &[("status", status)],
                *count as f64,
            ));
        }
        for (resource, count) in &snapshot.license_usage {
            gauges.push(GaugeSample::new(
                "orbit_license_usage",
                &[("resource", resource)],
                *count as f64,
            ));
        }
    }
    gauges
}

#[utoipa::path(get, path = "/api/v1/meta", tag = "system", responses((status = 200, body = MetaResponse)))]
pub async fn meta(State(state): State<AppState>) -> Json<MetaResponse> {
    Json(MetaResponse {
        version: VERSION.to_string(),
        api_version: API_VERSION.to_string(),
        environment: state.environment.clone(),
        auth: oidc_public_config(&state),
        events_driver: state.events_driver.clone(),
        license: (&state.license_status()).into(),
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
