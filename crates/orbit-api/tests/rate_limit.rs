//! HTTP-level checks for the anonymous rate limit. These use a minimal router so they
//! run without PostgreSQL; the full-router wiring is covered in `api_integration.rs`.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    middleware,
    routing::get,
    Router,
};
use http_body_util::BodyExt;
use orbit_api::rate_limit::{enforce, RateLimiter};
use serde_json::Value;
use tower::ServiceExt;

fn test_router(limit: u32) -> Router {
    Router::new()
        .route("/public", get(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(
            Arc::new(RateLimiter::new(limit)),
            enforce,
        ))
}

fn request(octet: u8) -> Request<Body> {
    Request::builder()
        .uri("/public")
        .extension(ConnectInfo(SocketAddr::from(([203, 0, 113, octet], 5000))))
        .body(Body::empty())
        .unwrap()
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value, Option<String>) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body, retry_after)
}

#[tokio::test]
async fn requests_above_the_configured_rate_return_an_enveloped_429() {
    let router = test_router(2);

    for _ in 0..2 {
        let (status, _, _) = send(&router, request(10)).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body, retry_after) = send(&router, request(10)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "rate_limited", "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .is_some_and(|m| !m.is_empty()));
    assert!(body["error"]["details"].is_array());
    assert!(body["error"]["requestId"].is_string());
    assert!(
        retry_after.is_some_and(|value| value.parse::<u64>().is_ok_and(|seconds| seconds > 0)),
        "429 should carry a positive Retry-After"
    );
}

#[tokio::test]
async fn distinct_source_ips_have_independent_buckets() {
    let router = test_router(1);

    let (status, _, _) = send(&router, request(10)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&router, request(10)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    let (status, _, _) = send(&router, request(11)).await;
    assert_eq!(status, StatusCode::OK);
}

fn proxied_router(limit: u32) -> Router {
    Router::new()
        .route("/public", get(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(
            Arc::new(RateLimiter::new(limit).with_trusted_proxies(
                orbit_api::rate_limit::parse_trusted_proxies("10.0.0.0/8").0,
            )),
            enforce,
        ))
}

/// A request as the web proxy (10.1.2.3) forwards it for `client`.
fn via_web_proxy(client: &str) -> Request<Body> {
    Request::builder()
        .uri("/public")
        .header("x-forwarded-for", client)
        .extension(ConnectInfo(SocketAddr::from(([10, 1, 2, 3], 5000))))
        .body(Body::empty())
        .unwrap()
}

/// #302: behind the web proxy every request has the same TCP peer; clients must still
/// get separate budgets, so one client cannot block sign-in for everyone.
#[tokio::test]
async fn clients_behind_a_trusted_proxy_have_separate_budgets() {
    let router = proxied_router(1);
    let (status, _, _) = send(&router, via_web_proxy("198.51.100.1")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&router, via_web_proxy("198.51.100.1")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, _, _) = send(&router, via_web_proxy("198.51.100.2")).await;
    assert_eq!(status, StatusCode::OK, "another client is not affected");
}

#[tokio::test]
async fn forwarded_for_from_an_untrusted_peer_is_ignored() {
    let router = proxied_router(1);
    let spoofed = |claimed: &str| {
        Request::builder()
            .uri("/public")
            .header("x-forwarded-for", claimed)
            .extension(ConnectInfo(SocketAddr::from(([203, 0, 113, 50], 5000))))
            .body(Body::empty())
            .unwrap()
    };
    let (status, _, _) = send(&router, spoofed("198.51.100.1")).await;
    assert_eq!(status, StatusCode::OK);
    // A new claimed address does not buy a new budget.
    let (status, _, _) = send(&router, spoofed("198.51.100.2")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}
