use std::{net::SocketAddr, sync::Arc};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use base64::Engine;
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use orbit_api::{
    build_router, config::DEFAULT_ANON_RATE_LIMIT_PER_MINUTE, rate_limit::RateLimiter, AppState,
    DevIdentity,
};
use orbit_domain::UserRole;
use orbit_events::{EventBusError, MemoryEventBus};
use orbit_harness::HarnessRegistry;
use orbit_security::{
    AuthConfig, AuthMode, DevAuthConfig, LicenseHandle, LicenseStatus, SecretCipher,
};
use orbit_store::Store;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

/// Event bus whose health can be flipped to simulate a NATS outage.
struct SwitchableBus {
    inner: MemoryEventBus,
    healthy: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl orbit_events::EventBus for SwitchableBus {
    fn driver(&self) -> &'static str {
        "memory"
    }
    async fn publish(&self, event: orbit_contracts::OrbitEvent) -> Result<(), EventBusError> {
        self.inner.publish(event).await
    }
    async fn subscribe(
        &self,
        filter: orbit_contracts::SubjectFilter,
    ) -> Result<orbit_events::EventStream, EventBusError> {
        self.inner.subscribe(filter).await
    }
    async fn health(&self) -> Result<(), EventBusError> {
        if self.healthy.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(EventBusError::Unavailable("simulated outage".into()))
        }
    }
}

struct TestApp {
    bus_healthy: Arc<std::sync::atomic::AtomicBool>,
    shutdown: tokio_util::sync::CancellationToken,
    router: axum::Router,
    state: AppState,
    store: Store,
    org_id: Uuid,
    agent_id: Uuid,
}

async fn test_app(allow_anonymous: bool) -> Option<TestApp> {
    test_app_with_license(allow_anonymous, None).await
}

async fn test_app_with_license(
    allow_anonymous: bool,
    license: Option<LicenseStatus>,
) -> Option<TestApp> {
    let url = std::env::var("ORBIT_TEST_DATABASE_URL").ok()?;
    let key = base64::engine::general_purpose::STANDARD.encode([9u8; 32]);
    let cipher = Arc::new(SecretCipher::from_base64_key(&key).unwrap());
    let store = Store::connect(&url, 5)
        .await
        .expect("connect")
        .with_cipher(cipher);
    store.migrate().await.expect("migrate");

    let slug = format!("api-test-{}", Uuid::now_v7());
    let org = store
        .create_organization(&slug, "API Test", orbit_domain::OrganizationMode::Personal)
        .await
        .unwrap();
    let user = store
        .ensure_user(org.id, "api@test.local", "API Test", UserRole::Owner)
        .await
        .unwrap();

    // Seed one connector so connection endpoints work.
    for manifest in orbit_api::connectors::builtin_manifests() {
        store.upsert_connector_manifest(&manifest).await.unwrap();
    }
    store
        .upsert_harness_descriptor(
            &HarnessRegistry::builtin()
                .descriptors()
                .into_iter()
                .find(|d| d.kind == orbit_domain::HarnessKind::Mock)
                .unwrap(),
        )
        .await
        .unwrap();

    let auth_config = AuthConfig {
        mode: AuthMode::Dev,
        dev: DevAuthConfig {
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            org_slug: org.slug.clone(),
            org_name: org.name.clone(),
            role: UserRole::Owner,
            allow_anonymous: true,
            dev_token: "test-token".into(),
        },
        oidc: None,
        allow_api_tokens: true,
    };

    let bus_healthy = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let bus = Arc::new(SwitchableBus {
        inner: MemoryEventBus::new(128),
        healthy: bus_healthy.clone(),
    });
    let state = AppState {
        store: store.clone(),
        bus,
        registry: Arc::new(HarnessRegistry::builtin()),
        auth: Arc::new(auth_config),
        jwt: None,
        default_org_id: org.id,
        dev_identity: Some(Arc::new(DevIdentity {
            user_id: user.id,
            org_id: org.id,
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            role: UserRole::Owner,
        })),
        environment: "test".into(),
        events_driver: "memory".into(),
        public_base_url: "http://localhost".into(),
        metrics: Arc::new(orbit_api::metrics::Metrics::new()),
        max_body_bytes: 2 * 1024 * 1024,
        rate_limiter: Arc::new(RateLimiter::new(DEFAULT_ANON_RATE_LIMIT_PER_MINUTE)),
        cors_origins: vec!["*".into()],
        launcher_url: None,
        launcher_token: None,
        worker_api_url: "http://localhost:8080".into(),
        ca_secret_name: None,
        ca_secret_key: "ca-bundle.crt".into(),
        archive_after_days: None,
        shutdown: shutdown.clone(),
        license: license.map(LicenseHandle::from_status).unwrap_or_else(|| {
            orbit_security::LicenseHandle::load(orbit_security::LicenseSource {
                token: None,
                file: None,
            })
        }),
    };
    // In this test harness anonymous is controlled by auth_config.dev.allow_anonymous;
    // rebuild the config accordingly for the negative auth test.
    let mut state = state;
    let mut auth = (*state.auth).clone();
    auth.dev.allow_anonymous = allow_anonymous;
    state.auth = Arc::new(auth);

    let created = create_demo_agent(&store, org.id, user.id).await;
    Some(TestApp {
        bus_healthy,
        shutdown,
        router: build_router(state.clone()),
        state,
        store,
        org_id: org.id,
        agent_id: created,
    })
}

fn definition() -> Value {
    json!({
        "apiVersion": "orbit.io/v1",
        "kind": "Agent",
        "metadata": { "name": "API Test Agent", "description": "created by tests" },
        "spec": {
            "harness": { "type": "mock", "executionMode": "on_demand", "config": {} },
            "model": { "management": "harness" },
            "instructions": { "system": "You are a test agent." },
            "connectors": [],
            "triggers": [{ "type": "schedule", "cron": "0 * * * *" }],
            "autonomy": { "level": 2 },
            "resources": { "timeoutSeconds": 30, "maxConcurrentExecutions": 1 }
        }
    })
}

fn licensed(features: &[&str], limits: Value) -> LicenseStatus {
    let now = Utc::now();
    let features: Vec<String> = features.iter().map(|value| value.to_string()).collect();
    let available_features = features
        .iter()
        .filter(|feature| {
            !orbit_security::RESERVED_FEATURES
                .iter()
                .any(|reserved| reserved == feature)
        })
        .cloned()
        .collect();
    LicenseStatus {
        tier: "enterprise".into(),
        customer: Some("Test customer".into()),
        plan: Some("enterprise".into()),
        license_id: Some("lic_test".into()),
        org_slug: None,
        features,
        available_features,
        limits: serde_json::from_value(limits).unwrap(),
        issued_at: Some(now - Duration::days(1)),
        expires_at: Some(now + Duration::days(30)),
        days_remaining: Some(30),
        grace: false,
        grace_ends_at: Some(now + Duration::days(60)),
        valid: true,
        trial: false,
        error: None,
    }
}

fn expired_license() -> LicenseStatus {
    let mut status = licensed(&["custom_roles", "teams"], json!({}));
    status.valid = false;
    status.tier = "community".into();
    status.features.clear();
    status.available_features.clear();
    status.expires_at = Some(Utc::now() - Duration::days(40));
    status.days_remaining = Some(-40);
    status.grace = false;
    status.error = Some("license expired".into());
    status
}

async fn create_demo_agent(store: &Store, org_id: Uuid, user_id: Uuid) -> Uuid {
    let definition: orbit_domain::AgentDefinition = serde_json::from_value(definition()).unwrap();
    let (agent, _) = store
        .create_agent(org_id, Some(user_id), &definition)
        .await
        .unwrap();
    store
        .set_agent_status(org_id, agent.id, orbit_domain::AgentStatus::Active)
        .await
        .unwrap();
    agent.id
}

async fn send(router: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn public_endpoints_and_meta() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(&app.router, get("/healthz")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    let (status, body) = send(&app.router, get("/api/v1/meta")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["apiVersion"], "v1");
    assert_eq!(body["auth"]["mode"], "dev");

    let (status, body) = send(&app.router, get("/api/v1/openapi.json")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["paths"]["/api/v1/agents"].is_object());
}

#[tokio::test]
async fn public_routes_return_429_above_configured_rate_per_ip() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let mut state = app.state.clone();
    state.rate_limiter = Arc::new(RateLimiter::new(2));
    let router = build_router(state);

    let request_from = |octet: u8, path: &str| {
        Request::builder()
            .uri(path)
            .extension(ConnectInfo(SocketAddr::from(([203, 0, 113, octet], 5000))))
            .body(Body::empty())
            .unwrap()
    };

    for _ in 0..2 {
        let (status, body) = send(&router, request_from(10, "/api/v1/auth/oidc/config")).await;
        assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    }
    let (status, body) = send(&router, request_from(10, "/api/v1/auth/oidc/config")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "rate_limited", "{body}");
    assert!(
        !body["error"]["requestId"].as_str().unwrap_or("").is_empty(),
        "{body}"
    );

    // The bucket is shared by the whole public surface, not one handler.
    let (status, _) = send(&router, request_from(10, "/healthz")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // A different source IP has an independent bucket.
    let (status, body) = send(&router, request_from(11, "/api/v1/auth/oidc/config")).await;
    assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");

    // Protected routes are untouched, including dev-auth anonymous mode.
    for _ in 0..3 {
        let (status, body) = send(&router, get("/api/v1/agents")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

#[tokio::test]
async fn readyz_depends_on_database_only() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(&app.router, get("/readyz")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
    assert_eq!(body["checks"]["eventBus"], "ok");

    // Bus outage: still ready, but reported as unavailable/degraded.
    app.bus_healthy
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let (status, body) = send(&app.router, get("/readyz")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ready");
    assert_eq!(body["checks"]["database"], "ok");
    assert_eq!(body["checks"]["eventBus"], "unavailable");
    // Dependency error text is logged, never returned to unauthenticated callers.
    assert!(body["checks"].get("eventBusError").is_none(), "{body}");
    assert!(body["checks"].get("databaseError").is_none(), "{body}");
    assert!(!body.to_string().contains("simulated outage"), "{body}");
    assert_eq!(body["degraded"], json!(["eventBus"]));
}

#[tokio::test]
async fn execution_sse_stream_ends_on_shutdown() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agents/{}/executions", app.agent_id),
            json!({ "task": "long running" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let execution_id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    app.store
        .claim_execution(execution_id, "test-worker")
        .await
        .unwrap();
    app.store
        .mark_execution_running(execution_id, Some("session-sse"))
        .await
        .unwrap();

    let uri = format!("/api/v1/executions/{execution_id}/events/stream");
    let response = app
        .router
        .clone()
        .oneshot(get(&uri))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body();

    // The execution never finishes, so the stream only ends because of the token.
    let shutdown = app.shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        shutdown.cancel();
    });
    let started = std::time::Instant::now();
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(5), body.collect())
        .await
        .expect("SSE stream must end shortly after shutdown")
        .unwrap()
        .to_bytes();
    assert!(started.elapsed() >= std::time::Duration::from_millis(1400));
    assert!(String::from_utf8_lossy(&bytes).contains("event: shutdown"));
}

#[tokio::test]
async fn unauthenticated_requests_are_rejected_when_anonymous_disabled() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let (status, body) = send(&app.router, get("/api/v1/agents")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
    assert!(!body["error"]["requestId"].as_str().unwrap_or("").is_empty());

    let authed = Request::builder()
        .uri("/api/v1/agents")
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app.router, authed).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn dev_auth_without_anonymous_requires_exact_dev_token() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let with_token = |token: &str| {
        Request::builder()
            .uri("/api/v1/agents")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let (status, _) = send(&app.router, get("/api/v1/agents")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for wrong in [
        "wrong-token",
        "test-toke",
        "test-token-x",
        "orbit_sk_unknown",
    ] {
        let (status, body) = send(&app.router, with_token(wrong)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "token {wrong}");
        assert_eq!(body["error"]["code"], "unauthorized");
    }
    let (status, _) = send(&app.router, with_token("test-token")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn dev_auth_with_anonymous_accepts_missing_and_non_dev_tokens() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, _) = send(&app.router, get("/api/v1/agents")).await;
    assert_eq!(status, StatusCode::OK);
    let req = Request::builder()
        .uri("/api/v1/agents")
        .header("authorization", "Bearer something-else")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app.router, req).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn agent_crud_validation_and_export() {
    let Some(app) = test_app(true).await else {
        return;
    };

    let (status, body) = send(&app.router, get("/api/v1/agents?limit=10")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["harness"], "mock");

    let (status, body) = send(&app.router, post("/api/v1/agents", definition())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["telegram_test"], Value::Null);
    assert_eq!(body["spec"]["autonomy"]["level"], 2);

    // Unknown harness → validation error envelope.
    let mut invalid = definition();
    invalid["spec"]["harness"]["type"] = json!("not-a-harness");
    let (status, body) = send(&app.router, post("/api/v1/agents", invalid)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation_error");

    // Semantic validation (bad cron) → 422 with field details.
    let mut invalid = definition();
    invalid["spec"]["triggers"] = json!([{ "type": "schedule", "cron": "nope" }]);
    invalid["metadata"]["name"] = json!("Invalid Agent");
    let (status, body) = send(&app.router, post("/api/v1/agents", invalid)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation_error");
    assert!(body["error"]["details"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["path"].as_str().unwrap_or("").contains("triggers")));

    // Export
    let (status, _) = send(&app.router, get(&format!("/api/v1/agents/{id}/export"))).await;
    assert_eq!(status, StatusCode::OK);

    // 404 envelope
    let (status, body) = send(
        &app.router,
        get(&format!("/api/v1/agents/{}", Uuid::now_v7())),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn run_task_is_stored_as_the_single_run_instruction() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let task = "AURORA-259-API-UNIQUE-RUN-INSTRUCTION";
    let configured = "AURORA-259-API-CONFIGURED-SYSTEM-INSTRUCTIONS";
    let path = format!("/api/v1/agents/{}/executions", app.agent_id);
    let (status, body) = send(
        &app.router,
        Request::builder()
            .method("POST")
            .uri(&path)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "task": task }).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let execution_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let execution = app
        .store
        .get_execution(app.org_id, execution_id)
        .await
        .unwrap();
    let input = execution.input;
    assert_eq!(input["task"], task);
    assert_eq!(input["task"].as_str().unwrap().matches(task).count(), 1);
    // Configured system instructions live in the agent spec, never in run input.
    assert!(input.get("instructions").is_none());
    assert!(!input.to_string().contains(configured));
}

#[tokio::test]
async fn execution_creation_is_idempotent_and_streams_events() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let path = format!("/api/v1/agents/{}/executions", app.agent_id);
    let request = |key: &str| {
        Request::builder()
            .method("POST")
            .uri(&path)
            .header("content-type", "application/json")
            .header("idempotency-key", key)
            .body(Body::from(json!({ "task": "test task" }).to_string()))
            .unwrap()
    };
    let (status, body) = send(&app.router, request("api-test-idem")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let execution_id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["status"], "queued");

    let (status, body2) = send(&app.router, request("api-test-idem")).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body2["id"], body["id"]);

    // Simulate the worker's durable writes so we can verify read endpoints.
    let execution_id = Uuid::parse_str(&execution_id).unwrap();
    app.store
        .claim_execution(execution_id, "test-worker")
        .await
        .unwrap();
    app.store
        .mark_execution_running(execution_id, Some("session-test"))
        .await
        .unwrap();
    app.store
        .append_execution_event(
            app.org_id,
            execution_id,
            "log",
            orbit_domain::LogLevel::Info,
            "hello from test",
            &json!({}),
        )
        .await
        .unwrap();
    app.store
        .finish_execution(
            execution_id,
            orbit_domain::ExecutionStatus::Succeeded,
            Some(&json!({ "ok": true })),
            None,
            None,
            Some("session-test"),
        )
        .await
        .unwrap();

    let (status, body) = send(
        &app.router,
        get(&format!("/api/v1/executions/{execution_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "succeeded");
    assert_eq!(body["output"]["ok"], true);

    let (status, body) = send(
        &app.router,
        get(&format!("/api/v1/executions/{execution_id}/events")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["message"] == "hello from test"));

    // SSE replay includes snapshot, the stored event and a done marker.
    let response = app
        .router
        .clone()
        .oneshot(get(&format!(
            "/api/v1/executions/{execution_id}/events/stream"
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // The envelope middleware must leave streams alone.
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: snapshot"), "{text}");
    assert!(text.contains("event: execution.event"), "{text}");
    assert!(text.contains("event: done"), "{text}");
}

#[tokio::test]
async fn approvals_can_be_listed_and_decided() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let _serial = APPROVAL_SETTINGS_LOCK.lock().await;
    reset_approval_policy(&app).await;
    let (status, execution) = send(
        &app.router,
        post(
            &format!("/api/v1/agents/{}/executions", app.agent_id),
            json!({ "task": "approval test" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{execution}");
    let execution_id = Uuid::parse_str(execution["id"].as_str().unwrap()).unwrap();
    let approval = app
        .store
        .create_approval(
            app.org_id,
            execution_id,
            json!({ "capability": "issues.comment", "effect": "write" }),
            None,
        )
        .await
        .unwrap();

    let (status, body) = send(&app.router, get("/api/v1/approvals?status=pending")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"] == approval.id.to_string()));

    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/approvals/{}/approve", approval.id),
            json!({ "comment": "looks good" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "approved");
    assert_eq!(body["comment"], "looks good");

    // Deciding twice is a conflict, not a silent overwrite.
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/approvals/{}/approve", approval.id),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "conflict");
}

/// Every provider kind the contract defines can be stored. The database check on
/// `providers.kind` once lagged behind the enum, which turned a valid request into a 500.
#[tokio::test]
async fn every_provider_kind_can_be_saved() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let cases = [
        (
            "openai_compatible",
            json!({ "baseUrl": "http://localhost:9999/v1", "model": "qwen" }),
        ),
        (
            "anthropic",
            json!({ "baseUrl": "http://localhost:9998/v1", "model": "claude-sonnet-5-5" }),
        ),
        ("harness_managed", json!({ "harness": "opencode" })),
    ];
    for (kind, config) in cases {
        let (status, body) = send(
            &app.router,
            post(
                "/api/v1/providers",
                json!({ "name": format!("Kind {kind}"), "kind": kind, "config": config }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{kind}: {body}");
        assert_eq!(body["kind"], kind, "{body}");
    }

    let (status, body) = send(&app.router, get("/api/v1/providers")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let kinds: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|provider| provider["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"anthropic"), "{kinds:?}");
}

#[tokio::test]
async fn provider_can_be_created_disabled() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/providers",
            json!({
                "name": format!("Off {}", Uuid::now_v7()),
                "kind": "harness_managed",
                "config": { "harness": "opencode" },
                "enabled": false
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["enabled"], false, "{body}");
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/providers",
            json!({ "name": format!("On {}", Uuid::now_v7()), "kind": "harness_managed", "config": { "harness": "opencode" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["enabled"], true, "enabled is the default");
}

#[tokio::test]
async fn tested_endpoint_provider_can_still_be_edited() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/providers",
            json!({
                "name": format!("Tested {}", Uuid::now_v7()),
                "kind": "openai_compatible",
                "config": { "baseUrl": "http://10.1.2.3:9999/v1", "model": "qwen" }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    // What a successful Test stores: the discovered catalog under `settings.models`.
    let tested = json!({
        "baseUrl": "http://10.1.2.3:9999/v1",
        "model": "qwen",
        "settings": { "models": ["qwen", "llama"] }
    });
    app.store
        .update_provider(app.org_id, id, None, Some(&tested), None, None)
        .await
        .unwrap();

    // The form sends the stored config back with one field changed.
    let mut edited = tested.clone();
    edited["model"] = json!("llama");
    let (status, body) = send(
        &app.router,
        patch_json(
            &format!("/api/v1/providers/{id}"),
            json!({ "config": edited }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"]["model"], "llama");
    assert_eq!(
        body["config"]["settings"]["models"],
        json!(["qwen", "llama"])
    );
}

#[tokio::test]
async fn provider_credentials_are_write_only() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let body = json!({
        "name": "Test Gateway",
        "kind": "openai_compatible",
        "config": { "baseUrl": "http://localhost:9999/v1", "model": "qwen" },
        "credential": "sk-test-should-never-leak"
    });
    let (status, body) = send(&app.router, post("/api/v1/providers", body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["hasCredential"], true);
    assert!(!body.to_string().contains("sk-test-should-never-leak"));
    let provider_id = body["id"].as_str().unwrap();

    let (status, body) = send(&app.router, get("/api/v1/providers")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.to_string().contains("sk-test-should-never-leak"));
    assert_eq!(body[0]["hasCredential"], true);

    // Secrets are stored encrypted.
    let ciphertext: String =
        sqlx::query_scalar("SELECT ciphertext FROM secrets WHERE reference LIKE 'provider/%'")
            .fetch_one(app.store.pool())
            .await
            .unwrap();
    assert!(!ciphertext.contains("sk-test-should-never-leak"));

    // Invalid provider config is rejected before persistence.
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/providers",
            json!({ "name": "Bad", "kind": "openai_compatible", "config": { "baseUrl": "ftp://x", "model": "m" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation_error");

    let (status, _) = send(
        &app.router,
        get(&format!("/api/v1/providers/{provider_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn connections_are_org_scoped_and_credentials_hidden() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let body = json!({
        "connectorSlug": "telegram",
        "name": "Telegram Bot",
        "credential": "123456:bot-secret",
        "scopes": ["messages.list"]
    });
    let (status, body) = send(&app.router, post("/api/v1/connections", body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["hasCredential"], true);
    assert_eq!(body["connectorSlug"], "telegram");
    assert!(!body.to_string().contains("bot-secret"));
    let connection_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = send(&app.router, get("/api/v1/connections")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);

    // Unknown connector → 404
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/connections",
            json!({ "connectorSlug": "nope", "name": "x" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/connections/{connection_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

fn auth_get(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn auth_delete(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

async fn token_for(store: &Store, org_id: Uuid, user_id: Uuid, name: &str) -> String {
    let issued = orbit_security::issue_api_token();
    store
        .create_api_token(
            org_id,
            user_id,
            name,
            &issued.prefix,
            &issued.hash,
            &[],
            None,
        )
        .await
        .expect("api token");
    issued.token
}

#[tokio::test]
async fn private_agents_are_hidden_from_other_users() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(app.org_id, "alice@test.local", "Alice", UserRole::Operator)
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(app.org_id, "bob@test.local", "Bob", UserRole::Operator)
        .await
        .unwrap();
    let alice_agent = create_demo_agent(&app.store, app.org_id, alice.id).await;
    let bob_agent = create_demo_agent(&app.store, app.org_id, bob.id).await;
    make_private(&app.store, app.org_id, "agents", alice_agent, alice.id).await;
    make_private(&app.store, app.org_id, "agents", bob_agent, bob.id).await;
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;

    let (status, body) = send(&app.router, auth_get("/api/v1/agents", &alice_token)).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert!(items.iter().all(|item| item["id"] != bob_agent.to_string()));
    assert!(items
        .iter()
        .any(|item| item["id"] == alice_agent.to_string()));

    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/agents/{bob_agent}"), &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/agents/{alice_agent}"), &bob_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/agents/{alice_agent}"), &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        &app.router,
        auth_delete(&format!("/api/v1/agents/{alice_agent}"), &bob_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/agents/{alice_agent}"),
            &bob_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/agents/{alice_agent}/export"), &bob_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{alice_agent}/executions"),
            &bob_token,
            json!({ "task": "cross-team execution" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Sharing to org visibility makes the same resource visible again.
    app.store
        .set_resource_visibility(app.org_id, "agents", bob_agent, "org", None)
        .await
        .unwrap();
    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/agents/{bob_agent}"), &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Back to private: hidden again.
    app.store
        .set_resource_visibility(app.org_id, "agents", bob_agent, "private", None)
        .await
        .unwrap();
    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/agents/{bob_agent}"), &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn auth_patch(path: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn auth_post(path: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn private_skills_are_hidden_from_other_users() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "skills-alice@test.local",
            "Alice",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(
            app.org_id,
            "skills-bob@test.local",
            "Bob",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;

    let (status, alice_skill) = send(
        &app.router,
        auth_post(
            "/api/v1/skills",
            &alice_token,
            json!({
                "name": "Alice skill",
                "slug": "alice-skill",
                "content": "alice",
                "files": {}
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, bob_skill) = send(
        &app.router,
        auth_post(
            "/api/v1/skills",
            &bob_token,
            json!({
                "name": "Bob skill",
                "slug": "bob-skill",
                "content": "bob",
                "files": {}
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let alice_id = alice_skill["id"].as_str().unwrap();
    let bob_id = bob_skill["id"].as_str().unwrap();
    for id in [alice_id, bob_id] {
        app.store
            .set_resource_visibility(
                app.org_id,
                "skills",
                Uuid::parse_str(id).unwrap(),
                "private",
                None,
            )
            .await
            .unwrap();
    }

    let (status, body) = send(&app.router, auth_get("/api/v1/skills", &alice_token)).await;
    assert_eq!(status, StatusCode::OK);
    let items = body.as_array().unwrap();
    assert!(items.iter().any(|item| item["id"] == alice_id));
    assert!(items.iter().all(|item| item["id"] != bob_id));

    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/skills/{bob_id}"), &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app.router,
        auth_delete(&format!("/api/v1/skills/{bob_id}"), &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

async fn make_private(store: &Store, org_id: Uuid, table: &str, id: Uuid, user_id: Uuid) {
    store
        .assign_resource_owner(org_id, table, id, user_id)
        .await
        .unwrap();
    store
        .set_resource_visibility(org_id, table, id, "private", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn private_resources_are_hidden_from_other_users() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "res-alice@test.local",
            "Alice",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(app.org_id, "res-bob@test.local", "Bob", UserRole::Operator)
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;
    let alice_agent = create_demo_agent(&app.store, app.org_id, alice.id).await;
    let bob_agent = create_demo_agent(&app.store, app.org_id, bob.id).await;
    let connector = app
        .store
        .get_connector_by_slug(app.org_id, "webhook")
        .await
        .unwrap()
        .expect("webhook connector");

    let alice_provider = app
        .store
        .create_provider(
            app.org_id,
            "Alice provider",
            "openai_compatible",
            &json!({ "baseUrl": "http://127.0.0.1:9/v1" }),
            None,
            true,
        )
        .await
        .unwrap();
    let bob_provider = app
        .store
        .create_provider(
            app.org_id,
            "Bob provider",
            "openai_compatible",
            &json!({ "baseUrl": "http://127.0.0.1:9/v1" }),
            None,
            true,
        )
        .await
        .unwrap();
    let alice_connection = app
        .store
        .create_connection(
            app.org_id,
            connector.id,
            "Alice webhook",
            &json!({ "url": "http://127.0.0.1:9/alice" }),
            &[],
            None,
            Some(alice.id),
        )
        .await
        .unwrap();
    let bob_connection = app
        .store
        .create_connection(
            app.org_id,
            connector.id,
            "Bob webhook",
            &json!({ "url": "http://127.0.0.1:9/bob" }),
            &[],
            None,
            Some(bob.id),
        )
        .await
        .unwrap();
    let alice_worker = app
        .store
        .create_worker_profile(
            app.org_id,
            "Alice worker",
            "mock",
            "local",
            "process",
            "orbit-worker",
            "test",
            "all",
            &[],
            &[],
            None,
            None,
            None,
            &json!({}),
            &json!({}),
            &json!({}),
            false,
            false,
            false,
            false,
            &json!({}),
        )
        .await
        .unwrap();
    let bob_worker = app
        .store
        .create_worker_profile(
            app.org_id,
            "Bob worker",
            "mock",
            "local",
            "process",
            "orbit-worker",
            "test",
            "all",
            &[],
            &[],
            None,
            None,
            None,
            &json!({}),
            &json!({}),
            &json!({}),
            false,
            false,
            false,
            false,
            &json!({}),
        )
        .await
        .unwrap();
    let alice_trigger = app
        .store
        .create_trigger(app.org_id, alice_agent, "manual", None, json!({}), true)
        .await
        .unwrap();
    let bob_trigger = app
        .store
        .create_trigger(app.org_id, bob_agent, "manual", None, json!({}), true)
        .await
        .unwrap();
    let alice_policy = app
        .store
        .create_tool_policy(
            app.org_id,
            None,
            "Alice policy",
            100,
            true,
            "allow",
            &json!({}),
            None,
            None,
        )
        .await
        .unwrap();
    let bob_policy = app
        .store
        .create_tool_policy(
            app.org_id,
            None,
            "Bob policy",
            100,
            true,
            "allow",
            &json!({}),
            None,
            None,
        )
        .await
        .unwrap();

    for (table, alice_id, bob_id) in [
        ("providers", alice_provider.id, bob_provider.id),
        ("connections", alice_connection.id, bob_connection.id),
        ("worker_profiles", alice_worker.id, bob_worker.id),
        ("triggers", alice_trigger.id, bob_trigger.id),
        ("tool_policies", alice_policy.id, bob_policy.id),
    ] {
        make_private(&app.store, app.org_id, table, alice_id, alice.id).await;
        make_private(&app.store, app.org_id, table, bob_id, bob.id).await;
    }

    for (path, bob_id, has_get) in [
        ("/api/v1/providers", bob_provider.id, false),
        ("/api/v1/connections", bob_connection.id, false),
        ("/api/v1/worker-profiles", bob_worker.id, true),
        ("/api/v1/triggers", bob_trigger.id, false),
        ("/api/v1/tool-policies", bob_policy.id, false),
    ] {
        let (status, body) = send(&app.router, auth_get(path, &alice_token)).await;
        assert_eq!(status, StatusCode::OK, "{path} list");
        let items = body
            .as_array()
            .unwrap_or_else(|| panic!("{path} list is not an array"));
        assert!(
            items.iter().all(|item| item["id"] != bob_id.to_string()),
            "{path} leaked another user's resource"
        );

        if has_get {
            let (status, _) = send(
                &app.router,
                auth_get(&format!("{path}/{bob_id}"), &alice_token),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}/{{id}} get");
        }

        let (status, _) = send(
            &app.router,
            auth_delete(&format!("{path}/{bob_id}"), &alice_token),
        )
        .await;
        // Operators lack write permission for some resources; both 403 (RBAC) and
        // 404 (hidden by visibility) are acceptable and must never be 2xx.
        assert!(
            matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "{path}/{{id}} delete returned {status}"
        );
    }

    // The owner still sees their own resource.
    let (status, body) = send(&app.router, auth_get("/api/v1/providers", &bob_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == bob_provider.id.to_string()));
}

#[tokio::test]
async fn private_agent_teams_and_runs_are_hidden_from_other_users() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "team-alice@test.local",
            "Alice",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(app.org_id, "team-bob@test.local", "Bob", UserRole::Operator)
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;
    let alice_team = app
        .store
        .create_agent_team(
            app.org_id,
            "Alice team",
            "",
            &json!({ "steps": [] }),
            Some(alice.id),
        )
        .await
        .unwrap();
    let bob_team = app
        .store
        .create_agent_team(
            app.org_id,
            "Bob team",
            "",
            &json!({ "steps": [] }),
            Some(bob.id),
        )
        .await
        .unwrap();
    make_private(
        &app.store,
        app.org_id,
        "agent_teams",
        alice_team.id,
        alice.id,
    )
    .await;
    make_private(&app.store, app.org_id, "agent_teams", bob_team.id, bob.id).await;
    let bob_run = app
        .store
        .create_team_run(app.org_id, bob_team.id, &json!({}), Some(bob.id))
        .await
        .unwrap();

    let (status, body) = send(&app.router, auth_get("/api/v1/agent-teams", &alice_token)).await;
    assert_eq!(status, StatusCode::OK);
    let items = body.as_array().unwrap();
    assert!(items
        .iter()
        .any(|item| item["id"] == alice_team.id.to_string()));
    assert!(items
        .iter()
        .all(|item| item["id"] != bob_team.id.to_string()));

    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/agent-teams/{}", bob_team.id),
            &alice_token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(&app.router, auth_get("/api/v1/team-runs", &alice_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["id"] != bob_run.id.to_string()));

    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/team-runs/{}", bob_run.id), &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agent-teams/{}/runs", bob_team.id),
            &alice_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Bob still sees his own run.
    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/team-runs/{}", bob_run.id), &bob_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn private_execution_events_and_approvals_are_hidden() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "exec-alice@test.local",
            "Alice",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(app.org_id, "exec-bob@test.local", "Bob", UserRole::Operator)
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;
    let bob_agent = create_demo_agent(&app.store, app.org_id, bob.id).await;
    make_private(&app.store, app.org_id, "agents", bob_agent, bob.id).await;

    let (status, execution) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{bob_agent}/executions"),
            &bob_token,
            json!({ "task": "private execution" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let execution_id = execution["id"].as_str().unwrap();

    for path in [
        format!("/api/v1/executions/{execution_id}"),
        format!("/api/v1/executions/{execution_id}/events"),
        format!("/api/v1/executions/{execution_id}/events/stream"),
    ] {
        let (status, _) = send(&app.router, auth_get(&path, &alice_token)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }

    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/executions/{execution_id}/cancel"),
            &alice_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let approval = app
        .store
        .create_approval(
            app.org_id,
            Uuid::parse_str(execution_id).unwrap(),
            json!({ "action": "deploy" }),
            None,
        )
        .await
        .unwrap();

    let (status, body) = send(&app.router, auth_get("/api/v1/approvals", &alice_token)).await;
    assert!(
        status == StatusCode::FORBIDDEN
            || (status == StatusCode::OK
                && body
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|item| item["id"] != approval.id.to_string()))
    );
}

#[tokio::test]
async fn community_license_blocks_team_and_sharing_writes() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let token = "test-token";
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/teams",
            token,
            json!({ "name": "Ops", "slug": "ops" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(body["error"]["code"], "license_required");

    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/sharing/agents/{}", app.agent_id),
            token,
            json!({ "visibility": "private" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(body["error"]["code"], "license_required");
}

#[tokio::test]
async fn same_agent_queues_on_different_harnesses() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let mut value = definition();
    value["spec"].as_object_mut().unwrap().remove("harness");
    value["spec"]["resources"]["maxConcurrentExecutions"] = json!(2);
    let definition: orbit_domain::AgentDefinition = serde_json::from_value(value).unwrap();
    let owner = app
        .store
        .list_users(app.org_id)
        .await
        .unwrap()
        .into_iter()
        .find(|user| user.role == "owner")
        .unwrap();
    let (agent, _) = app
        .store
        .create_agent(app.org_id, Some(owner.id), &definition)
        .await
        .unwrap();
    app.store
        .set_agent_status(app.org_id, agent.id, orbit_domain::AgentStatus::Active)
        .await
        .unwrap();
    app.store
        .set_platform_setting(
            "harness_default",
            &json!({ "default": "mock" }),
            Some(owner.id),
        )
        .await
        .unwrap();

    let (status, first) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{}/executions", agent.id),
            "test-token",
            json!({ "task": "default harness" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["harnessKind"], "mock");

    let (status, second) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{}/executions", agent.id),
            "test-token",
            json!({ "task": "pi override", "harness": "pi" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(second["harnessKind"], "pi");
}

/// `runtime_tick` advances every runtime in the shared test database, so tests that tick
/// (or interleave with a tick) run one at a time.
static RUNTIME_TICK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Approval policy is a platform-wide setting; tests that change it must not overlap.
static APPROVAL_SETTINGS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn reset_approval_policy(app: &TestApp) {
    app.store
        .set_platform_setting(
            "approvals",
            &json!({ "approvalsRequireDifferentApprover": false }),
            None,
        )
        .await
        .unwrap();
}

async fn raw_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&std::env::var("ORBIT_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn poisoned_runtimes_do_not_block_healthy_ones() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let _serial = RUNTIME_TICK_LOCK.lock().await;
    let user = app
        .store
        .ensure_user(app.org_id, "api@test.local", "API Test", UserRole::Owner)
        .await
        .unwrap();
    let pool = raw_pool().await;

    // Oldest runtime: unknown harness (permanent error).
    let bad_harness = create_demo_agent(&app.store, app.org_id, user.id).await;
    let rt_harness = app
        .store
        .create_agent_runtime(app.org_id, bad_harness, "t", "bogus", 60, None)
        .await
        .unwrap();
    // Second: current spec that no longer deserialises (transient, backs off).
    let bad_spec = create_demo_agent(&app.store, app.org_id, user.id).await;
    sqlx::query("UPDATE agent_versions SET spec = '{\"bogus\": true}'::jsonb WHERE agent_id = $1")
        .bind(bad_spec)
        .execute(&pool)
        .await
        .unwrap();
    app.store
        .create_agent_runtime(app.org_id, bad_spec, "t", "mock", 10, None)
        .await
        .unwrap();
    // Healthy runtime created last.
    let healthy = create_demo_agent(&app.store, app.org_id, user.id).await;
    app.store
        .create_agent_runtime(app.org_id, healthy, "t", "mock", 60, None)
        .await
        .unwrap();

    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();

    let ok = app
        .store
        .get_agent_runtime(app.org_id, healthy)
        .await
        .unwrap()
        .unwrap();
    assert!(
        ok.last_execution_id.is_some(),
        "healthy runtime must advance"
    );
    assert!(ok.last_error.is_none());

    let spec = app
        .store
        .get_agent_runtime(app.org_id, bad_spec)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(spec.status, "running");
    assert!(spec.last_error.as_deref().unwrap().contains("invalid"));
    assert!(spec.next_run_at > chrono::Utc::now() + chrono::Duration::seconds(30));
    assert!(spec.last_execution_id.is_none());

    let (status, error): (String, Option<String>) =
        sqlx::query_as("SELECT status, last_error FROM agent_runtimes WHERE id = $1")
            .bind(rt_harness.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "stopped");
    assert!(error.unwrap().contains("unknown harness"));
}

#[tokio::test]
async fn runtime_tick_retry_reuses_execution() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let _serial = RUNTIME_TICK_LOCK.lock().await;
    let user = app
        .store
        .ensure_user(app.org_id, "api@test.local", "API Test", UserRole::Owner)
        .await
        .unwrap();
    let agent_id = create_demo_agent(&app.store, app.org_id, user.id).await;
    let runtime = app
        .store
        .create_agent_runtime(app.org_id, agent_id, "t", "mock", 60, None)
        .await
        .unwrap();
    let pool = raw_pool().await;

    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    let first = app
        .store
        .get_agent_runtime(app.org_id, agent_id)
        .await
        .unwrap()
        .unwrap()
        .last_execution_id
        .unwrap();

    // Simulate a crash between create_execution and update_agent_runtime: the execution
    // exists but the runtime row still points at nothing and is due again.
    sqlx::query("UPDATE agent_runtimes SET last_execution_id = NULL, next_run_at = now() - interval '1 second' WHERE id = $1")
        .bind(runtime.id)
        .execute(&pool)
        .await
        .unwrap();
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();

    let retried = app
        .store
        .get_agent_runtime(app.org_id, agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retried.last_execution_id,
        Some(first),
        "retry reuses the execution"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM executions WHERE org_id = $1 AND agent_id = $2")
            .bind(app.org_id)
            .bind(agent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "no duplicate execution");
}

async fn runtime_row(
    app: &TestApp,
    agent_id: uuid::Uuid,
) -> orbit_store::agent_runtimes::AgentRuntime {
    sqlx::query_as("SELECT * FROM agent_runtimes WHERE org_id = $1 AND agent_id = $2")
        .bind(app.org_id)
        .bind(agent_id)
        .fetch_one(&raw_pool().await)
        .await
        .unwrap()
}

async fn fail_execution(pool: &sqlx::PgPool, execution_id: uuid::Uuid) {
    end_execution(pool, execution_id, "failed").await;
}

async fn end_execution(pool: &sqlx::PgPool, execution_id: uuid::Uuid, status: &str) {
    sqlx::query("UPDATE executions SET status = $2, error = '\"boom\"'::jsonb, finished_at = now() WHERE id = $1")
        .bind(execution_id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
}

async fn make_runtime_due(pool: &sqlx::PgPool, runtime_id: uuid::Uuid) {
    sqlx::query(
        "UPDATE agent_runtimes SET next_run_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(runtime_id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn runtime_stops_after_consecutive_failures() {
    runtime_stops_after_consecutive("failed").await;
}

#[tokio::test]
async fn runtime_stops_after_consecutive_timeouts() {
    runtime_stops_after_consecutive("timed_out").await;
}

async fn runtime_stops_after_consecutive(ended_as: &str) {
    let Some(app) = test_app(false).await else {
        return;
    };
    let _serial = RUNTIME_TICK_LOCK.lock().await;
    let user = app
        .store
        .ensure_user(app.org_id, "api@test.local", "API Test", UserRole::Owner)
        .await
        .unwrap();
    let agent_id = create_demo_agent(&app.store, app.org_id, user.id).await;
    let runtime = app
        .store
        .create_agent_runtime(app.org_id, agent_id, "t", "mock", 60, None)
        .await
        .unwrap();
    let pool = raw_pool().await;
    let limit = orbit_api::background::RUNTIME_MAX_CONSECUTIVE_FAILURES;

    for round in 1..=limit {
        make_runtime_due(&pool, runtime.id).await;
        orbit_api::background::runtime_tick(&app.state)
            .await
            .unwrap();
        let id = runtime_row(&app, agent_id).await.last_execution_id.unwrap();
        end_execution(&pool, id, ended_as).await;
        make_runtime_due(&pool, runtime.id).await;
        orbit_api::background::runtime_tick(&app.state)
            .await
            .unwrap();
        let row = runtime_row(&app, agent_id).await;
        assert_eq!(row.consecutive_failures, round);
        if round < limit {
            assert_eq!(row.status, "running");
        }
    }
    let row = runtime_row(&app, agent_id).await;
    assert_eq!(row.status, "stopped");
    assert!(row.last_error.as_deref().unwrap().contains(&format!(
        "stopped after {limit} consecutive failed executions"
    )));
}

async fn new_execution(app: &TestApp, agent_id: Uuid) -> Uuid {
    let (status, execution) = send(
        &app.router,
        post(
            &format!("/api/v1/agents/{agent_id}/executions"),
            json!({ "task": "approval policy test" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{execution}");
    Uuid::parse_str(execution["id"].as_str().unwrap()).unwrap()
}

async fn new_approval(app: &TestApp, execution_id: Uuid) -> Uuid {
    app.store
        .create_approval(
            app.org_id,
            execution_id,
            json!({ "action": "deploy" }),
            None,
        )
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn approval_decisions_require_use_and_optional_four_eyes() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let _serial = APPROVAL_SETTINGS_LOCK.lock().await;
    reset_approval_policy(&app).await;
    app.store
        .create_custom_role(
            app.org_id,
            "Approver",
            &[
                "agents:read".into(),
                "executions:read".into(),
                "approvals:decide".into(),
            ],
        )
        .await
        .unwrap();
    let carol = app
        .store
        .ensure_user(
            app.org_id,
            "appr-carol@test.local",
            "Carol",
            UserRole::Viewer,
        )
        .await
        .unwrap();
    app.store
        .set_user_role_name(app.org_id, carol.id, "Approver")
        .await
        .unwrap();
    let carol_token = token_for(&app.store, app.org_id, carol.id, "carol").await;
    let team = app
        .store
        .create_access_team(
            app.org_id,
            "Approvers",
            &format!("appr-{}", Uuid::now_v7()),
            None,
        )
        .await
        .unwrap();
    // Someone other than Carol owns the agent, so only her team role grants access.
    let dave = app
        .store
        .ensure_user(
            app.org_id,
            "appr-dave@test.local",
            "Dave",
            UserRole::Operator,
        )
        .await
        .unwrap();
    app.store
        .assign_resource_owner(app.org_id, "agents", app.agent_id, dave.id)
        .await
        .unwrap();
    app.store
        .set_resource_visibility(app.org_id, "agents", app.agent_id, "team", Some(team.id))
        .await
        .unwrap();
    app.store
        .upsert_team_member(team.id, carol.id, "viewer")
        .await
        .unwrap();

    let execution_id = new_execution(&app, app.agent_id).await;
    let approval_id = new_approval(&app, execution_id).await;

    // Team viewer: can see the agent but cannot use it -> 403.
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/approvals/{approval_id}/approve"),
            &carol_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Not in the team at all -> the agent is invisible -> 404.
    app.store
        .remove_team_member(team.id, carol.id)
        .await
        .unwrap();
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/approvals/{approval_id}/approve"),
            &carol_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Team member: allowed (four-eyes off by default; requester is the dev owner).
    app.store
        .upsert_team_member(team.id, carol.id, "member")
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/approvals/{approval_id}/approve"),
            &carol_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Default setting is off.
    let (status, body) = send(&app.router, get("/api/v1/settings/approvals")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["approvalsRequireDifferentApprover"], false);

    // Four-eyes off: the requester (dev owner) may approve their own execution.
    let own = new_approval(&app, execution_id).await;
    let (status, body) = send(
        &app.router,
        post(&format!("/api/v1/approvals/{own}/approve"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Four-eyes on: the requester is refused, another user is not.
    let request = Request::builder()
        .method("PUT")
        .uri("/api/v1/settings/approvals")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "approvalsRequireDifferentApprover": true }).to_string(),
        ))
        .unwrap();
    let (status, body) = send(&app.router, request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["approvalsRequireDifferentApprover"], true);

    let gated = new_approval(&app, execution_id).await;
    for decision in ["approve", "reject"] {
        let (status, body) = send(
            &app.router,
            post(&format!("/api/v1/approvals/{gated}/{decision}"), json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("different approver"));
    }
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/approvals/{gated}/approve"),
            &carol_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "approved");
}

#[tokio::test]
async fn expired_approval_cannot_be_decided_via_api() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let _serial = APPROVAL_SETTINGS_LOCK.lock().await;
    reset_approval_policy(&app).await;
    let execution_id = new_execution(&app, app.agent_id).await;
    let approval = app
        .store
        .create_approval(
            app.org_id,
            execution_id,
            json!({ "action": "late" }),
            Some(chrono::Utc::now() - chrono::Duration::seconds(30)),
        )
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/approvals/{}/approve", approval.id),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("approval expired"));
    assert_eq!(
        app.store
            .get_approval(app.org_id, approval.id)
            .await
            .unwrap()
            .status,
        "expired"
    );
}

fn worker_request(method: &str, path: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

#[tokio::test]
async fn worker_approvals_are_bound_to_the_execution_and_clamped() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let issued = orbit_security::issue_worker_token();
    let token_record = app
        .store
        .create_worker_token(
            app.org_id,
            None,
            "approvals-test",
            &issued.prefix,
            &issued.hash,
            &[],
        )
        .await
        .unwrap();
    let execution_a = new_execution(&app, app.agent_id).await;
    let zed = app
        .store
        .ensure_user(
            app.org_id,
            "worker-zed@test.local",
            "Zed",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let other_agent = create_demo_agent(&app.store, app.org_id, zed.id).await;
    let execution_b = new_execution(&app, other_agent).await;
    for execution_id in [execution_a, execution_b] {
        app.store
            .claim_execution(execution_id, "approvals-test-worker")
            .await
            .unwrap()
            .expect("execution is queued");
        app.store
            .bind_execution_worker_token(execution_id, token_record.id)
            .await
            .unwrap();
    }

    // u64::MAX must neither panic nor produce an absurd deadline.
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/approvals",
            &issued.token,
            Some(json!({
                "executionId": execution_a,
                "action": { "capability": "x" },
                "waitSeconds": u64::MAX
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let expires: chrono::DateTime<chrono::Utc> =
        body["expiresAt"].as_str().unwrap().parse().unwrap();
    assert!(
        expires <= chrono::Utc::now() + chrono::Duration::hours(24) + chrono::Duration::minutes(1)
    );
    let approval_id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();

    // Decide it so the wait returns immediately for the matching execution.
    app.store
        .decide_approval(app.org_id, approval_id, "approved", zed.id, None)
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        worker_request(
            "GET",
            &format!(
                "/api/v1/worker/approvals/{approval_id}/wait?executionId={execution_a}&seconds=1"
            ),
            &issued.token,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Another execution in the same org cannot read this approval's status.
    let (status, _) = send(
        &app.router,
        worker_request(
            "GET",
            &format!(
                "/api/v1/worker/approvals/{approval_id}/wait?executionId={execution_b}&seconds=1"
            ),
            &issued.token,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Per-execution cap on pending approvals.
    for _ in 0..49 {
        app.store
            .create_approval(app.org_id, execution_b, json!({ "n": 1 }), None)
            .await
            .unwrap();
    }
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/approvals",
            &issued.token,
            Some(json!({ "executionId": execution_b, "action": {} })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/approvals",
            &issued.token,
            Some(json!({ "executionId": execution_b, "action": {} })),
        ),
    )
    .await;
    // 49 + 1 created above = 50 pending -> the next request is refused.
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
}

#[tokio::test]
async fn env_mapping_secret_refs_must_be_usable_in_the_callers_org() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let mapping = |reference: &str| {
        json!({
            "name": format!("map-{}", Uuid::now_v7()),
            "mappings": [{ "envVar": "TOKEN", "secretRef": reference }]
        })
    };

    // A real secret that belongs to another organization.
    let other = app
        .store
        .create_organization(
            &format!("other-{}", Uuid::now_v7()),
            "Other",
            orbit_domain::OrganizationMode::Personal,
        )
        .await
        .unwrap();
    let foreign = format!("connection/{}", Uuid::now_v7());
    app.store
        .put_secret(
            Some(other.id),
            &foreign,
            &orbit_security::SecretValue::new("foreign-super-secret"),
            "t",
        )
        .await
        .unwrap();
    let (status, body) = send(&app.router, post("/api/v1/env-mappings", mapping(&foreign))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(!body.to_string().contains("foreign-super-secret"));

    // Unknown reference.
    let (status, _) = send(
        &app.router,
        post("/api/v1/env-mappings", mapping("connection/does-not-exist")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Own-org secret that no connection/provider owns.
    let orphan = format!("connection/{}", Uuid::now_v7());
    app.store
        .put_secret(
            Some(app.org_id),
            &orphan,
            &orbit_security::SecretValue::new("orphan"),
            "t",
        )
        .await
        .unwrap();
    let (status, _) = send(&app.router, post("/api/v1/env-mappings", mapping(&orphan))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Unknown connection id.
    let (status, _) = send(
        &app.router,
        post(
            "/api/v1/env-mappings",
            json!({
                "name": "bad-conn",
                "mappings": [{ "envVar": "TOKEN", "connectionId": Uuid::now_v7() }]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // A usable connection's credential in the caller's org is accepted (create + update).
    let (status, created) = send(
        &app.router,
        post(
            "/api/v1/connections",
            json!({
                "connectorSlug": "telegram",
                "name": "Mapped bot",
                "credential": "123456:bot-secret",
                "scopes": ["messages.list"]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let connection = app
        .store
        .get_connection(
            app.org_id,
            Uuid::parse_str(created["id"].as_str().unwrap()).unwrap(),
        )
        .await
        .unwrap();
    let reference = connection.credential_ref.unwrap();
    let (status, body) = send(
        &app.router,
        post("/api/v1/env-mappings", mapping(&reference)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap();
    let patch = Request::builder()
        .method("PATCH")
        .uri(format!("/api/v1/env-mappings/{id}"))
        .header("content-type", "application/json")
        .body(Body::from(mapping(&foreign).to_string()))
        .unwrap();
    let (status, _) = send(&app.router, patch).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn env_mapping_secret_refs_respect_resource_access() {
    let Some(app) = test_app(true).await else {
        return;
    };
    app.store
        .create_custom_role(
            app.org_id,
            "Mapper",
            &["settings:read".into(), "settings:write".into()],
        )
        .await
        .unwrap();
    let erin = app
        .store
        .ensure_user(app.org_id, "map-erin@test.local", "Erin", UserRole::Viewer)
        .await
        .unwrap();
    app.store
        .set_user_role_name(app.org_id, erin.id, "Mapper")
        .await
        .unwrap();
    let erin_token = token_for(&app.store, app.org_id, erin.id, "erin").await;
    let frank = app
        .store
        .ensure_user(
            app.org_id,
            "map-frank@test.local",
            "Frank",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let connector = app
        .store
        .get_connector_by_slug(app.org_id, "webhook")
        .await
        .unwrap()
        .expect("webhook connector");
    let reference = format!("connection/{}", Uuid::now_v7());
    app.store
        .put_secret(
            Some(app.org_id),
            &reference,
            &orbit_security::SecretValue::new("franks-token"),
            "t",
        )
        .await
        .unwrap();
    let connection = app
        .store
        .create_connection(
            app.org_id,
            connector.id,
            "Frank private",
            &json!({ "url": "http://127.0.0.1:9/frank" }),
            &[],
            Some(&reference),
            Some(frank.id),
        )
        .await
        .unwrap();
    make_private(
        &app.store,
        app.org_id,
        "connections",
        connection.id,
        frank.id,
    )
    .await;

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/env-mappings",
            &erin_token,
            json!({
                "name": "erin-map",
                "mappings": [{ "envVar": "TOKEN", "secretRef": reference }]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(!body.to_string().contains("franks-token"));

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/env-mappings",
            &erin_token,
            json!({
                "name": "erin-map-conn",
                "mappings": [{ "envVar": "TOKEN", "connectionId": connection.id }]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

async fn runtime_with_first_execution(
    app: &TestApp,
) -> (
    orbit_store::agent_runtimes::AgentRuntime,
    uuid::Uuid,
    uuid::Uuid,
) {
    let user = app
        .store
        .ensure_user(app.org_id, "api@test.local", "API Test", UserRole::Owner)
        .await
        .unwrap();
    let agent_id = create_demo_agent(&app.store, app.org_id, user.id).await;
    let runtime = app
        .store
        .create_agent_runtime(app.org_id, agent_id, "t", "mock", 60, None)
        .await
        .unwrap();
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    let first = runtime_row(app, agent_id).await.last_execution_id.unwrap();
    (runtime, agent_id, first)
}

async fn execution_count(pool: &sqlx::PgPool, agent_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM executions WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn stop_during_inflight_tick_is_not_overwritten_by_launch() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let _serial = RUNTIME_TICK_LOCK.lock().await;
    let pool = raw_pool().await;
    let (runtime, agent_id, exec) = runtime_with_first_execution(&app).await;

    // Rewind to "tick claimed, execution created, runtime row not yet updated".
    sqlx::query("UPDATE agent_runtimes SET last_execution_id = NULL, next_run_at = now() - interval '1 second' WHERE id = $1")
        .bind(runtime.id)
        .execute(&pool)
        .await
        .unwrap();
    let claimed = app
        .store
        .claim_agent_runtime_tick(
            runtime.id,
            chrono::Utc::now() + chrono::Duration::seconds(5),
        )
        .await
        .unwrap()
        .expect("claimed");

    // Operator presses Stop while the tick is in flight.
    let stopping = app
        .store
        .request_agent_runtime_stop(runtime.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopping.status, "stopping");

    // The tick's final write must not revive the runtime, but must record the execution.
    let recorded = app
        .store
        .record_agent_runtime_launch(
            runtime.id,
            exec,
            true,
            chrono::Utc::now() + chrono::Duration::seconds(60),
        )
        .await
        .unwrap()
        .expect("launch is recorded for a stopping runtime");
    assert_eq!(recorded.status, "stopping");
    assert_eq!(recorded.last_execution_id, Some(exec));
    assert_eq!(
        recorded.next_run_at, claimed.next_run_at,
        "schedule untouched"
    );

    // The stopping branch cancels the raced execution, then finishes the stop.
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    assert_eq!(runtime_row(&app, agent_id).await.status, "stopping");
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    let row = runtime_row(&app, agent_id).await;
    assert_eq!(row.status, "stopped");
    assert!(row.stopped_at.is_some());
    let status: String = sqlx::query_scalar("SELECT status FROM executions WHERE id = $1")
        .bind(exec)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");

    // No further executions, ever.
    make_runtime_due(&pool, runtime.id).await;
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    assert_eq!(runtime_row(&app, agent_id).await.status, "stopped");
    assert_eq!(execution_count(&pool, agent_id).await, 1);
}

#[tokio::test]
async fn stop_during_inflight_tick_is_not_overwritten_by_backoff() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let _serial = RUNTIME_TICK_LOCK.lock().await;
    let pool = raw_pool().await;
    let (runtime, agent_id, first) = runtime_with_first_execution(&app).await;
    fail_execution(&pool, first).await;
    make_runtime_due(&pool, runtime.id).await;
    let claimed = app
        .store
        .claim_agent_runtime_tick(
            runtime.id,
            chrono::Utc::now() + chrono::Duration::seconds(5),
        )
        .await
        .unwrap()
        .expect("claimed");

    app.store
        .request_agent_runtime_stop(runtime.id)
        .await
        .unwrap()
        .unwrap();

    // Failure accounting, plain backoff and the stop-at-limit write are all refused.
    let later = chrono::Utc::now() + chrono::Duration::seconds(300);
    assert!(app
        .store
        .record_agent_runtime_failure(runtime.id, false, first, 1, "boom", later)
        .await
        .unwrap()
        .is_none());
    assert!(app
        .store
        .record_agent_runtime_failure(runtime.id, true, first, 5, "boom", later)
        .await
        .unwrap()
        .is_none());
    assert!(app
        .store
        .advance_agent_runtime(runtime.id, Some("transient"), later)
        .await
        .unwrap()
        .is_none());
    assert!(app
        .store
        .halt_running_agent_runtime(runtime.id, "permanent")
        .await
        .unwrap()
        .is_none());

    let row = runtime_row(&app, agent_id).await;
    assert_eq!(row.status, "stopping");
    assert_eq!(row.consecutive_failures, 0);
    assert_eq!(row.next_run_at, claimed.next_run_at);

    // The last execution is terminal, so the next tick completes the stop.
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    assert_eq!(runtime_row(&app, agent_id).await.status, "stopped");
    assert_eq!(execution_count(&pool, agent_id).await, 1);
}

#[tokio::test]
async fn runtime_status_transitions_are_guarded() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let _serial = RUNTIME_TICK_LOCK.lock().await;
    let (runtime, agent_id, exec) = runtime_with_first_execution(&app).await;
    let soon = chrono::Utc::now() + chrono::Duration::seconds(60);

    // stopping -> stopped only from stopping.
    assert!(app
        .store
        .finish_agent_runtime_stop(runtime.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(runtime_row(&app, agent_id).await.status, "running");

    app.store
        .request_agent_runtime_stop(runtime.id)
        .await
        .unwrap()
        .unwrap();
    // Stop is idempotent while stopping.
    assert_eq!(
        app.store
            .request_agent_runtime_stop(runtime.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "stopping"
    );
    app.store
        .finish_agent_runtime_stop(runtime.id)
        .await
        .unwrap()
        .unwrap();

    // Once stopped nothing moves it: no restart, no schedule, no launch, no second stop.
    assert!(app
        .store
        .record_agent_runtime_launch(runtime.id, exec, true, soon)
        .await
        .unwrap()
        .is_none());
    assert!(app
        .store
        .advance_agent_runtime(runtime.id, None, soon)
        .await
        .unwrap()
        .is_none());
    assert!(app
        .store
        .halt_agent_runtime(runtime.id, "x")
        .await
        .unwrap()
        .is_none());
    assert!(app
        .store
        .request_agent_runtime_stop(runtime.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(runtime_row(&app, agent_id).await.status, "stopped");
}

#[tokio::test]
async fn disabled_users_tokens_are_rejected() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let user = app
        .store
        .ensure_user(app.org_id, "off@test.local", "Off", UserRole::Owner)
        .await
        .unwrap();
    let token = token_for(&app.store, app.org_id, user.id, "off").await;
    let (status, _) = send(&app.router, auth_get("/api/v1/me", &token)).await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(user.id)
        .execute(app.store.pool())
        .await
        .unwrap();
    let (status, _) = send(&app.router, auth_get("/api/v1/me", &token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

fn put_json(path: &str, token: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn token_permissions_follow_the_owners_current_role() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let user = app
        .store
        .ensure_user(app.org_id, "demoted@test.local", "Demoted", UserRole::Owner)
        .await
        .unwrap();
    let issued = orbit_security::issue_api_token();
    let scopes: Vec<String> = orbit_security::Permission::all_for(UserRole::Owner)
        .into_iter()
        .map(str::to_string)
        .collect();
    app.store
        .create_api_token(
            app.org_id,
            user.id,
            "explicit",
            &issued.prefix,
            &issued.hash,
            &scopes,
            None,
        )
        .await
        .unwrap();
    let (status, current) = send(
        &app.router,
        auth_get("/api/v1/settings/access", &issued.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &app.router,
        put_json("/api/v1/settings/access", &issued.token, &current),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner write allowed: {body}");

    app.store
        .set_user_role(app.org_id, user.id, UserRole::Viewer)
        .await
        .unwrap();

    let (status, _) = send(
        &app.router,
        auth_get("/api/v1/settings/access", &issued.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "viewer can still read");
    let (status, body) = send(
        &app.router,
        put_json("/api/v1/settings/access", &issued.token, &current),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = send(
        &app.router,
        auth_post(
            "/api/v1/api-tokens",
            &issued.token,
            json!({ "name": "escalate" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn tokens_cannot_be_minted_with_scopes_the_caller_lacks() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let user = app
        .store
        .ensure_user(app.org_id, "minter@test.local", "Minter", UserRole::Viewer)
        .await
        .unwrap();
    let role = format!("Token minter {}", Uuid::now_v7());
    app.store
        .create_custom_role(
            app.org_id,
            &role,
            &[
                "agents:read".to_string(),
                "settings:read".to_string(),
                "settings:write".to_string(),
            ],
        )
        .await
        .unwrap();
    app.store
        .set_user_role_name(app.org_id, user.id, &role)
        .await
        .unwrap();
    let token = token_for(&app.store, app.org_id, user.id, "minter").await;

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/api-tokens",
            &token,
            json!({ "name": "too much", "scopes": ["agents:read", "providers:write", "*"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let details = body["error"]["details"].to_string();
    assert!(details.contains("providers:write"), "{details}");
    assert!(!details.contains("'*'"), "{details}");
    assert!(!details.contains("'agents:read'"), "{details}");

    // `*` is allowed: it resolves to the caller's current permissions at request time.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/api-tokens",
            &token,
            json!({ "name": "wildcard", "scopes": ["*"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/api-tokens",
            &token,
            json!({ "name": "ok", "scopes": ["agents:read"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = send(
        &app.router,
        auth_post("/api/v1/api-tokens", &token, json!({ "name": "default" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let mut got: Vec<String> = serde_json::from_value(body["scopes"].clone()).unwrap();
    got.sort();
    assert_eq!(got, ["agents:read", "settings:read", "settings:write"]);
}

#[tokio::test]
async fn dashboard_summary_hides_other_teams_agents_and_executions() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let owner = owner_user(&app).await;
    // The demo agent created by the harness belongs to the owner only.
    make_private(&app.store, app.org_id, "agents", app.agent_id, owner.id).await;

    let team_x = app
        .store
        .create_access_team(app.org_id, "Team X", "team-x", Some(owner.id))
        .await
        .unwrap();
    let team_y = app
        .store
        .create_access_team(app.org_id, "Team Y", "team-y", Some(owner.id))
        .await
        .unwrap();
    let viewer = app
        .store
        .ensure_user(
            app.org_id,
            "dash-viewer@test.local",
            "Viewer",
            UserRole::Viewer,
        )
        .await
        .unwrap();
    app.store
        .upsert_team_member(team_x.id, viewer.id, "viewer")
        .await
        .unwrap();
    app.store
        .set_platform_setting(
            "team_isolation",
            &json!({ "enabled": true }),
            Some(owner.id),
        )
        .await
        .unwrap();

    let (_, exec_x) =
        seed_team_agent_with_execution(&app, owner.id, team_x.id, "visible-to-x").await;
    let (_, exec_y) =
        seed_team_agent_with_execution(&app, owner.id, team_y.id, "SECRET-of-team-y").await;
    let viewer_token = token_for(&app.store, app.org_id, viewer.id, "dash-viewer").await;

    let (status, body) = send(
        &app.router,
        auth_get("/api/v1/dashboard/summary", &viewer_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recent_ids(&body), vec![exec_x.to_string()]);
    assert!(!body.to_string().contains("SECRET-of-team-y"));
    let summary = &body["summary"];
    assert_eq!(summary["totalAgents"], 1);
    assert_eq!(summary["activeAgents"], 1);
    assert_eq!(summary["executions24h"], 1);
    assert_eq!(summary["runningExecutions"], 1);
    assert_eq!(summary["pendingApprovals"], 1);
    assert_eq!(usage_total(&body), 1);

    // The owner still sees everything: both teams' executions and the private demo agent.
    let (status, body) = send(
        &app.router,
        auth_get("/api/v1/dashboard/summary", "test-token"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let recent = recent_ids(&body);
    assert!(recent.contains(&exec_x.to_string()));
    assert!(recent.contains(&exec_y.to_string()));
    let summary = &body["summary"];
    assert_eq!(summary["totalAgents"], 3);
    assert_eq!(summary["executions24h"], 2);
    assert_eq!(summary["pendingApprovals"], 2);
    assert_eq!(usage_total(&body), 2);
}

#[tokio::test]
async fn dashboard_summary_is_empty_for_user_outside_every_team() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let owner = owner_user(&app).await;
    make_private(&app.store, app.org_id, "agents", app.agent_id, owner.id).await;
    let team_y = app
        .store
        .create_access_team(app.org_id, "Team Y2", "team-y2", Some(owner.id))
        .await
        .unwrap();
    let (_, exec_y) = seed_team_agent_with_execution(&app, owner.id, team_y.id, "hidden").await;
    let outsider = app
        .store
        .ensure_user(
            app.org_id,
            "dash-outsider@test.local",
            "Outsider",
            UserRole::Viewer,
        )
        .await
        .unwrap();
    let token = token_for(&app.store, app.org_id, outsider.id, "dash-outsider").await;
    let (status, body) = send(&app.router, auth_get("/api/v1/dashboard/summary", &token)).await;
    assert_eq!(status, StatusCode::OK);
    let summary = &body["summary"];
    assert_eq!(summary["totalAgents"], 0);
    assert_eq!(summary["executions24h"], 0);
    assert_eq!(summary["pendingApprovals"], 0);
    assert!(recent_ids(&body).is_empty());
    assert_eq!(usage_total(&body), 0);
    assert!(!body.to_string().contains(&exec_y.to_string()));
}

async fn owner_user(app: &TestApp) -> orbit_store::models::User {
    app.store
        .list_users(app.org_id)
        .await
        .unwrap()
        .into_iter()
        .find(|user| user.role == "owner")
        .unwrap()
}

fn recent_ids(body: &Value) -> Vec<String> {
    body["summary"]["recentExecutions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect()
}

async fn seed_team_agent_with_execution(
    app: &TestApp,
    owner_id: Uuid,
    team_id: Uuid,
    task: &str,
) -> (Uuid, Uuid) {
    let definition: orbit_domain::AgentDefinition = serde_json::from_value(definition()).unwrap();
    let (agent, version) = app
        .store
        .create_agent(app.org_id, Some(owner_id), &definition)
        .await
        .unwrap();
    app.store
        .set_agent_status(app.org_id, agent.id, orbit_domain::AgentStatus::Active)
        .await
        .unwrap();
    app.store
        .set_resource_visibility(app.org_id, "agents", agent.id, "team", Some(team_id))
        .await
        .unwrap();
    let execution = app
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: app.org_id,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: orbit_domain::HarnessKind::Mock,
            trigger_source: orbit_domain::TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: json!({ "task": task }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(owner_id),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    app.store
        .create_approval(app.org_id, execution.id, json!({}), None)
        .await
        .unwrap();
    (agent.id, execution.id)
}

fn usage_total(body: &Value) -> i64 {
    body["usage"]["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["executions"].as_i64().unwrap())
        .sum()
}

#[tokio::test]
async fn trigger_creation_requires_use_on_the_target_agent() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "trigger-alice@test.local",
            "Alice",
            UserRole::Owner,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(
            app.org_id,
            "trigger-bob@test.local",
            "Bob",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let agent = create_demo_agent(&app.store, app.org_id, alice.id).await;
    make_private(&app.store, app.org_id, "agents", agent, alice.id).await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "trigger-bob").await;
    let alice_token = token_for(&app.store, app.org_id, alice.id, "trigger-alice").await;

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/triggers",
            &bob_token,
            json!({ "agentId": agent, "kind": "manual" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/triggers",
            &alice_token,
            json!({ "agentId": agent, "kind": "manual" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["agentId"], agent.to_string());
}

#[tokio::test]
async fn team_queue_requires_team_view_and_use() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "queue-alice@test.local",
            "Alice",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(
            app.org_id,
            "queue-bob@test.local",
            "Bob",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let charlie = app
        .store
        .ensure_user(
            app.org_id,
            "queue-charlie@test.local",
            "Charlie",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;
    let charlie_token = token_for(&app.store, app.org_id, charlie.id, "charlie").await;

    let team = app
        .store
        .create_agent_team(
            app.org_id,
            "Queue team",
            "",
            &json!({ "steps": [] }),
            Some(alice.id),
        )
        .await
        .unwrap();
    app.store
        .assign_resource_owner(app.org_id, "agent_teams", team.id, alice.id)
        .await
        .unwrap();
    let access_team = app
        .store
        .create_access_team(
            app.org_id,
            "Queue viewers",
            &format!("queue-{}", Uuid::now_v7()),
            Some(alice.id),
        )
        .await
        .unwrap();
    app.store
        .upsert_team_member(access_team.id, bob.id, "viewer")
        .await
        .unwrap();
    app.store
        .set_resource_visibility(
            app.org_id,
            "agent_teams",
            team.id,
            "team",
            Some(access_team.id),
        )
        .await
        .unwrap();

    // A non-member cannot see the queue at all.
    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/agent-teams/{}/queue", team.id),
            &charlie_token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agent-teams/{}/queue", team.id),
            &charlie_token,
            json!({ "items": [{ "externalKey": "x" }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A team viewer can read the queue but cannot enqueue or run.
    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/agent-teams/{}/queue", team.id),
            &bob_token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agent-teams/{}/queue", team.id),
            &bob_token,
            json!({ "items": [{ "externalKey": "x" }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agent-teams/{}/queue", team.id),
            &alice_token,
            json!({ "items": [{ "externalKey": "item-1" }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let item_id = body[0]["id"].as_str().unwrap();
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agent-teams/{}/queue/{item_id}/run", team.id),
            &bob_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---- issue #193: provider use is checked in both management modes ----

fn provider_definition(management: &str, provider: &str) -> Value {
    let mut definition = definition();
    definition["spec"]["model"] = json!({ "management": management, "provider": provider });
    if management == "platform" {
        definition["spec"]["model"]["model"] = json!("test-model");
    }
    definition
}

struct ProviderFixture {
    app: TestApp,
    owner: orbit_store::User,
    operator: orbit_store::User,
    operator_token: String,
    provider_name: String,
}

/// A private provider with a credential owned by `owner`, plus an Operator who cannot
/// view it.
async fn provider_fixture() -> Option<ProviderFixture> {
    let app = test_app(true).await?;
    let owner = app
        .store
        .ensure_user(
            app.org_id,
            "victim@test.local",
            "Victim",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let operator = app
        .store
        .ensure_user(
            app.org_id,
            "attacker@test.local",
            "Attacker",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let provider_name = format!("provider-b-{}", Uuid::now_v7());
    let reference = format!("provider/{}", Uuid::now_v7());
    app.store
        .put_secret(
            Some(app.org_id),
            &reference,
            &orbit_security::SecretValue::new("sk-victim-secret"),
            "test",
        )
        .await
        .unwrap();
    let provider = app
        .store
        .create_provider(
            app.org_id,
            &provider_name,
            "openai_compatible",
            &json!({ "baseUrl": "http://127.0.0.1:9/v1", "model": "m" }),
            Some(&reference),
            true,
        )
        .await
        .unwrap();
    make_private(&app.store, app.org_id, "providers", provider.id, owner.id).await;
    let operator_token = token_for(&app.store, app.org_id, operator.id, "attacker").await;
    Some(ProviderFixture {
        app,
        owner,
        operator,
        operator_token,
        provider_name,
    })
}

#[tokio::test]
async fn operator_cannot_bind_unviewable_provider() {
    let Some(f) = provider_fixture().await else {
        return;
    };
    for management in ["harness", "platform"] {
        // Create.
        let (status, body) = send(
            &f.app.router,
            auth_post(
                "/api/v1/agents",
                &f.operator_token,
                provider_definition(management, &f.provider_name),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{management}: {body}");
        assert_eq!(body["error"]["details"][0]["path"], "spec.model.provider");

        // Update of an agent the operator owns.
        let own = create_demo_agent(&f.app.store, f.app.org_id, f.operator.id).await;
        let (status, body) = send(
            &f.app.router,
            auth_patch(
                &format!("/api/v1/agents/{own}"),
                &f.operator_token,
                json!({ "definition": provider_definition(management, &f.provider_name) }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{management}: {body}");

        // Execute preflight: an agent that already names the provider (stored before the
        // fix, or written directly) cannot be run by the operator.
        let definition: orbit_domain::AgentDefinition =
            serde_json::from_value(provider_definition(management, &f.provider_name)).unwrap();
        let (agent, _) = f
            .app
            .store
            .create_agent(f.app.org_id, Some(f.operator.id), &definition)
            .await
            .unwrap();
        f.app
            .store
            .set_agent_status(f.app.org_id, agent.id, orbit_domain::AgentStatus::Active)
            .await
            .unwrap();
        let (status, body) = send(
            &f.app.router,
            auth_post(
                &format!("/api/v1/agents/{}/executions", agent.id),
                &f.operator_token,
                json!({ "task": "steal" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{management}: {body}");
    }

    // The owner can bind it in harness mode.
    let owner_token = token_for(&f.app.store, f.app.org_id, f.owner.id, "victim").await;
    let (status, body) = send(
        &f.app.router,
        auth_post(
            "/api/v1/agents",
            &owner_token,
            provider_definition("harness", &f.provider_name),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn queued_execution(
    f: &ProviderFixture,
    agent_id: Uuid,
    requested_by: Option<Uuid>,
    input: Value,
    token_id: Uuid,
) -> Uuid {
    let agent = f.app.store.get_agent(f.app.org_id, agent_id).await.unwrap();
    let version = f
        .app
        .store
        .get_current_version(f.app.org_id, &agent)
        .await
        .unwrap();
    let execution = f
        .app
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: f.app.org_id,
            agent_id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: orbit_domain::HarnessKind::Mock,
            trigger_source: orbit_domain::TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input,
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by,
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    f.app
        .store
        .claim_execution(execution.id, "provider-test-worker")
        .await
        .unwrap()
        .expect("queued");
    f.app
        .store
        .bind_execution_worker_token(execution.id, token_id)
        .await
        .unwrap();
    execution.id
}

#[tokio::test]
async fn worker_context_withholds_credential_from_unauthorized_principal() {
    let Some(f) = provider_fixture().await else {
        return;
    };
    let issued = orbit_security::issue_worker_token();
    let token = f
        .app
        .store
        .create_worker_token(
            f.app.org_id,
            None,
            "provider-test",
            &issued.prefix,
            &issued.hash,
            &[],
        )
        .await
        .unwrap();
    let context_path = |id: Uuid| format!("/api/v1/worker/executions/{id}/context");

    for management in ["harness", "platform"] {
        let definition: orbit_domain::AgentDefinition =
            serde_json::from_value(provider_definition(management, &f.provider_name)).unwrap();
        // Agent owned by the victim, so only the requester differs between the cases.
        let (agent, _) = f
            .app
            .store
            .create_agent(f.app.org_id, Some(f.owner.id), &definition)
            .await
            .unwrap();

        // Allowed: requested by the provider owner. Credential present.
        let allowed = queued_execution(&f, agent.id, Some(f.owner.id), json!({}), token.id).await;
        let (status, body) = send(
            &f.app.router,
            worker_request("GET", &context_path(allowed), &issued.token, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{management}: {body}");
        assert_eq!(
            body["model"]["credential"]["value"], "sk-victim-secret",
            "{management}"
        );

        // Denied: requested by an operator who cannot view the provider.
        let denied = queued_execution(&f, agent.id, Some(f.operator.id), json!({}), token.id).await;
        let (status, body) = send(
            &f.app.router,
            worker_request("GET", &context_path(denied), &issued.token, None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{management}: {body}");
        assert!(!body.to_string().contains("sk-victim-secret"));
    }

    // Subagent model: the lead agent is harmless, the team subagent names the provider.
    let sub_definition: orbit_domain::AgentDefinition =
        serde_json::from_value(provider_definition("harness", &f.provider_name)).unwrap();
    let (sub_agent, _) = f
        .app
        .store
        .create_agent(f.app.org_id, Some(f.owner.id), &sub_definition)
        .await
        .unwrap();
    let input = json!({ "team": { "subagents": [{ "agentId": sub_agent.id, "name": "sub" }] } });
    let denied = queued_execution(
        &f,
        f.app.agent_id,
        Some(f.operator.id),
        input.clone(),
        token.id,
    )
    .await;
    let (status, body) = send(
        &f.app.router,
        worker_request("GET", &context_path(denied), &issued.token, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(!body.to_string().contains("sk-victim-secret"));
    let allowed = queued_execution(&f, f.app.agent_id, Some(f.owner.id), input, token.id).await;
    let (status, body) = send(
        &f.app.router,
        worker_request("GET", &context_path(allowed), &issued.token, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["subagentModels"][0]["model"]["credential"]["value"],
        "sk-victim-secret"
    );
}

#[tokio::test]
async fn team_run_auto_approve_requires_approvals_decide() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let operator = app
        .store
        .ensure_user(app.org_id, "op194@test.local", "Op", UserRole::Operator)
        .await
        .unwrap();
    let token = token_for(&app.store, app.org_id, operator.id, "op194").await;
    // The permission check runs before any team lookup, so the team id need not exist.
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agent-teams/{}/runs", Uuid::now_v7()),
            &token,
            json!({ "task": "x", "autoApprove": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("approvals:decide"));
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agent-teams/{}/runs", Uuid::now_v7()),
            &token,
            json!({ "task": "x" }),
        ),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rerun_strips_approval_override_without_approvals_decide() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let owner = owner_user(&app).await;
    let operator = app
        .store
        .ensure_user(app.org_id, "rr194@test.local", "Op", UserRole::Operator)
        .await
        .unwrap();
    let definition: orbit_domain::AgentDefinition = serde_json::from_value(definition()).unwrap();
    let (agent, version) = app
        .store
        .create_agent(app.org_id, Some(operator.id), &definition)
        .await
        .unwrap();
    app.store
        .set_agent_status(app.org_id, agent.id, orbit_domain::AgentStatus::Active)
        .await
        .unwrap();
    let agent_id = agent.id;
    app.store
        .set_resource_visibility(app.org_id, "agents", agent_id, "org", None)
        .await
        .unwrap();
    let original = app
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: app.org_id,
            agent_id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: orbit_domain::HarnessKind::Mock,
            trigger_source: orbit_domain::TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: json!({ "task": "t", "approvalOverride": "auto", "autonomyOverride": 4 }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(operator.id),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    app.store
        .claim_execution(original.id, "test-worker")
        .await
        .unwrap();
    app.store
        .mark_execution_running(original.id, Some("s"))
        .await
        .unwrap();
    app.store
        .finish_execution(
            original.id,
            orbit_domain::ExecutionStatus::Succeeded,
            Some(&json!({})),
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let op_token = token_for(&app.store, app.org_id, operator.id, "rr194").await;
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/executions/{}/rerun", original.id),
            &op_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(body["input"].get("approvalOverride").is_none());
    assert!(body["input"].get("autonomyOverride").is_none());
    assert_eq!(body["input"]["rerunOf"], original.id.to_string());

    // Free the agent's single concurrency slot before the second rerun.
    let first = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    app.store
        .claim_execution(first, "test-worker")
        .await
        .unwrap();
    app.store.mark_execution_running(first, None).await.unwrap();
    app.store
        .finish_execution(
            first,
            orbit_domain::ExecutionStatus::Succeeded,
            Some(&json!({})),
            None,
            None,
            None,
        )
        .await
        .unwrap();

    // The owner holds approvals:decide: the override is kept, the legacy numeric one is not.
    let owner_token = token_for(&app.store, app.org_id, owner.id, "rr194-owner").await;
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/executions/{}/rerun", original.id),
            &owner_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["input"]["approvalOverride"], "auto");
    assert!(body["input"].get("autonomyOverride").is_none());
}

fn anon_put(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn oidc_settings_update_preserves_absent_fields() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let path = "/api/v1/settings/oidc";
    let (status, body) = send(
        &app.router,
        anon_put(
            path,
            json!({ "settings": {
                "issuer": "https://idp.example.com",
                "clientId": "orbit",
                "groupRoles": { "admins": "admin" },
                "teamGroups": { "eng": "platform:lead" },
                "syncRoles": true,
                "roleClaim": "roles",
                "autoCreateTeams": true,
                "requireMappedGroup": true,
            } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A partial body (what the form used to send) must not reset the mappings.
    let (status, body) = send(
        &app.router,
        anon_put(
            path,
            json!({ "settings": { "issuer": "https://idp2.example.com", "clientId": "orbit2" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(&app.router, get(path)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["issuer"], "https://idp2.example.com");
    assert_eq!(body["clientId"], "orbit2");
    assert_eq!(body["groupRoles"], json!({ "admins": "admin" }));
    assert_eq!(body["teamGroups"], json!({ "eng": "platform:lead" }));
    assert_eq!(body["syncRoles"], true);
    assert_eq!(body["roleClaim"], "roles");
    assert_eq!(body["autoCreateTeams"], true);
    assert_eq!(body["requireMappedGroup"], true);

    // Explicit values still clear a mapping.
    let (status, _) = send(
        &app.router,
        anon_put(path, json!({ "settings": { "groupRoles": {} } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(&app.router, get(path)).await;
    assert_eq!(body["groupRoles"], json!({}));
    assert_eq!(body["syncRoles"], true);

    // A validation error returns 422 and changes nothing.
    for bad in [
        json!({ "settings": { "issuer": "ftp://nope" } }),
        json!({ "settings": { "groupRoles": { "x": "superuser" } } }),
        json!({ "settings": { "syncRoles": "yes" } }),
        json!({ "settings": "not an object" }),
    ] {
        let (_, before) = send(&app.router, get(path)).await;
        let (status, body) = send(&app.router, anon_put(path, bad.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body}");
        assert_eq!(body["error"]["code"], "validation_error");
        let (_, after) = send(&app.router, get(path)).await;
        assert_eq!(before, after, "{bad} changed stored settings");
    }
}

#[tokio::test]
async fn env_mapping_edit_requires_manage_access() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(app.org_id, "env-alice@test.local", "Alice", UserRole::Owner)
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(app.org_id, "env-bob@test.local", "Bob", UserRole::Operator)
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "env-alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "env-bob").await;
    let mapping = app
        .store
        .create_env_mapping(
            app.org_id,
            "private env",
            "",
            &json!([{ "envVar": "TOKEN", "secretKey": "missing" }]),
        )
        .await
        .unwrap();
    make_private(&app.store, app.org_id, "env_mappings", mapping.id, alice.id).await;

    let (status, body) = send(&app.router, auth_get("/api/v1/env-mappings", &bob_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["id"] != mapping.id.to_string()));

    let (status, _) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/env-mappings/{}", mapping.id),
            &bob_token,
            json!({ "name": "hijacked", "mappings": [] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app.router,
        auth_delete(&format!("/api/v1/env-mappings/{}", mapping.id), &bob_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/env-mappings/{}", mapping.id),
            &alice_token,
            json!({ "name": "private env renamed", "mappings": [] }),
        ),
    )
    .await;
    // The owner passes the access check; validation still rejects the empty mapping.
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

// ---------------------------------------------------------------------------
// #113: team run / queue state transitions (restored, #191)
// ---------------------------------------------------------------------------

async fn make_team(app: &TestApp, steps: Vec<Uuid>, extra: Value) -> Uuid {
    let mut definition = json!({
        "mode": "pipeline",
        "instructions": "do the work",
        "steps": steps.iter().map(|id| json!({ "agentId": id })).collect::<Vec<_>>(),
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        definition[key] = value;
    }
    app.store
        .create_agent_team(app.org_id, "T", "", &definition, None)
        .await
        .unwrap()
        .id
}

async fn start_run(app: &TestApp, team_id: Uuid) -> Uuid {
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team_id}/runs"),
            json!({ "task": "go" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["run"]["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn team_run_task_is_not_mixed_with_configured_goal() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let task = "AURORA-259-TEAM-API-UNIQUE-RUN-INSTRUCTION";
    let goal = "AURORA-259-TEAM-API-CONFIGURED-GOAL";
    let team_id = make_team(&app, vec![app.agent_id], json!({ "instructions": goal })).await;
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team_id}/runs"),
            json!({ "task": task }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let run_id: Uuid = body["run"]["id"].as_str().unwrap().parse().unwrap();
    let step = app.store.current_team_step(run_id).await.unwrap().unwrap();
    let execution = app
        .store
        .get_execution(app.org_id, step.execution_id.unwrap())
        .await
        .unwrap();
    let input = execution.input;
    assert_eq!(input["task"], task);
    assert_eq!(input["task"].as_str().unwrap().matches(task).count(), 1);
    assert!(!input["task"].as_str().unwrap().contains(goal));
    // The configured goal is carried as system context, not as another task.
    assert_eq!(input["team"]["goal"], goal);
}

async fn succeed_current_step(app: &TestApp, run_id: Uuid) {
    let step = app.store.current_team_step(run_id).await.unwrap().unwrap();
    // Since #139 the store refuses queued -> succeeded, so walk the real lifecycle first.
    let execution_id = step.execution_id.unwrap();
    app.store
        .claim_execution(execution_id, "test-worker")
        .await
        .unwrap();
    app.store
        .mark_execution_running(execution_id, None)
        .await
        .unwrap();
    app.store
        .finish_execution(
            step.execution_id.unwrap(),
            orbit_domain::ExecutionStatus::Succeeded,
            Some(&json!({ "summary": "ok" })),
            None,
            None,
            None,
        )
        .await
        .unwrap();
}

async fn advance_concurrently(
    state: &AppState,
    run: &orbit_store::TeamRun,
    n: usize,
) -> Vec<anyhow::Result<()>> {
    let mut tasks = Vec::new();
    for _ in 0..n {
        let state = state.clone();
        let run = run.clone();
        tasks.push(tokio::spawn(async move {
            orbit_api::handlers::teams::advance(&state, run).await
        }));
    }
    let mut out = Vec::new();
    for task in tasks {
        out.push(task.await.unwrap());
    }
    out
}

#[tokio::test]
async fn concurrent_advance_creates_one_execution_per_step() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let team = make_team(&app, vec![app.agent_id, app.agent_id], json!({})).await;
    let run_id = start_run(&app, team).await;
    succeed_current_step(&app, run_id).await;
    let run = app.store.get_team_run(app.org_id, run_id).await.unwrap();
    let results = advance_concurrently(&app.state, &run, 4).await;
    assert!(results.iter().all(|result| result.is_ok()), "{results:?}");
    let steps = app.store.list_team_steps(run_id).await.unwrap();
    assert_eq!(steps.len(), 2);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM executions WHERE team_run_id = $1 AND team_step_index = 1",
    )
    .bind(run_id)
    .fetch_one(&raw_pool().await)
    .await
    .unwrap();
    assert_eq!(count, 1, "exactly one execution for step 1");

    // Finishing the last step succeeds the run exactly once, even when raced.
    succeed_current_step(&app, run_id).await;
    let run = app.store.get_team_run(app.org_id, run_id).await.unwrap();
    advance_concurrently(&app.state, &run, 4).await;
    let run = app.store.get_team_run(app.org_id, run_id).await.unwrap();
    assert_eq!(run.status, "succeeded");
}

#[tokio::test]
async fn advance_resumes_after_step_finished_without_next_step() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let team = make_team(&app, vec![app.agent_id, app.agent_id], json!({})).await;
    let run_id = start_run(&app, team).await;
    // Simulate a tick that died right after finishing step 0.
    let step = app.store.current_team_step(run_id).await.unwrap().unwrap();
    app.store
        .finish_team_step(step.id, "succeeded", Some(&json!({ "x": 1 })), None)
        .await
        .unwrap()
        .unwrap();
    // A second finish is a no-op instead of overwriting.
    assert!(app
        .store
        .finish_team_step(step.id, "failed", None, Some("late"))
        .await
        .unwrap()
        .is_none());
    let run = app.store.get_team_run(app.org_id, run_id).await.unwrap();
    orbit_api::handlers::teams::advance(&app.state, run)
        .await
        .unwrap();
    assert_eq!(app.store.list_team_steps(run_id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn failed_next_step_start_is_retried_then_fails_the_run() {
    let Some(app) = test_app(true).await else {
        return;
    };
    // The second step cannot be started (a non-client, i.e. retryable, error). Team-run
    // creation now rejects unknown step agents up front, and a deleted agent is a 404 that
    // fails the run immediately (see #191 notes), so corrupt the second agent's stored
    // spec: start_step then fails with an internal error.
    let owner = app.store.list_users(app.org_id).await.unwrap()[0].id;
    let broken = create_demo_agent(&app.store, app.org_id, owner).await;
    let team = make_team(&app, vec![app.agent_id, broken], json!({})).await;
    let run_id = start_run(&app, team).await;
    sqlx::query("UPDATE agent_versions SET spec = '{}'::jsonb WHERE agent_id = $1")
        .bind(broken)
        .execute(&raw_pool().await)
        .await
        .unwrap();
    succeed_current_step(&app, run_id).await;
    let run = app.store.get_team_run(app.org_id, run_id).await.unwrap();

    // Within the retry window: error, run stays running, step stays succeeded (retryable).
    assert!(orbit_api::handlers::teams::advance(&app.state, run.clone())
        .await
        .is_err());
    let run_now = app.store.get_team_run(app.org_id, run_id).await.unwrap();
    assert_eq!(run_now.status, "running");
    let step = app.store.current_team_step(run_id).await.unwrap().unwrap();
    assert_eq!(step.status, "succeeded");
    // The lease was released, so the next tick retries.
    assert!(orbit_api::handlers::teams::advance(&app.state, run.clone())
        .await
        .is_err());

    // Past the window: the run fails with a clear error instead of retrying forever.
    sqlx::query("UPDATE team_steps SET finished_at = now() - interval '1 hour' WHERE id = $1")
        .bind(step.id)
        .execute(&raw_pool().await)
        .await
        .unwrap();
    orbit_api::handlers::teams::advance(&app.state, run)
        .await
        .unwrap();
    let run_now = app.store.get_team_run(app.org_id, run_id).await.unwrap();
    assert_eq!(run_now.status, "failed");
    assert!(run_now.error.unwrap().contains("could not start step 2"));
}

async fn queue_item(app: &TestApp, team: Uuid, key: &str, max_attempts: i32) -> Uuid {
    app.store
        .try_enqueue_team_item(
            app.org_id,
            team,
            key,
            None,
            &json!({ "task": key }),
            max_attempts,
        )
        .await
        .unwrap()
        .unwrap()
        .id
}

#[tokio::test]
async fn concurrent_queue_start_creates_exactly_one_run() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let team = make_team(&app, vec![app.agent_id], json!({})).await;
    let item = queue_item(&app, team, "gh#1", 1).await;
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let router = app.router.clone();
        tasks.push(tokio::spawn(async move {
            send(
                &router,
                post(
                    &format!("/api/v1/agent-teams/{team}/queue/{item}/run"),
                    json!({}),
                ),
            )
            .await
            .0
        }));
    }
    let mut created = 0;
    for task in tasks {
        let status = task.await.unwrap();
        if status == StatusCode::CREATED {
            created += 1;
        } else {
            assert_eq!(status, StatusCode::CONFLICT);
        }
    }
    assert_eq!(created, 1);
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM team_runs WHERE team_id = $1")
        .bind(team)
        .fetch_one(&raw_pool().await)
        .await
        .unwrap();
    assert_eq!(runs, 1);
    let stored = app
        .store
        .get_team_queue_item(app.org_id, item)
        .await
        .unwrap();
    assert_eq!(stored.status, "running");
    assert!(stored.team_run_id.is_some());
}

/// The step execution of a team run, by run id.
async fn run_execution(run: Uuid) -> (Uuid, String, bool) {
    sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT id, status, cancel_requested FROM executions WHERE team_run_id = $1",
    )
    .bind(run)
    .fetch_one(&raw_pool().await)
    .await
    .unwrap()
}

async fn start_queue_run(app: &TestApp, team: Uuid, key: &str) -> (Uuid, Uuid) {
    let item = queue_item(app, team, key, 1).await;
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team}/queue/{item}/run"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let run = app
        .store
        .get_team_queue_item(app.org_id, item)
        .await
        .unwrap()
        .team_run_id
        .expect("run attached");
    (item, run)
}

/// #299: cancelling a queue item must stop its step execution, not only the run row,
/// and the item cannot start a second run while that execution is still stopping.
#[tokio::test]
async fn cancelling_a_queue_item_stops_its_execution() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let team = make_team(&app, vec![app.agent_id], json!({})).await;

    // Claimed by a worker: the execution is asked to stop.
    let (item, run) = start_queue_run(&app, team, "gh#299a").await;
    let (execution, _, _) = run_execution(run).await;
    app.store
        .claim_execution(execution, "worker-299")
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team}/queue/{item}/cancel"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, exec_status, cancel_requested) = run_execution(run).await;
    assert_eq!(exec_status, "dispatched");
    assert!(cancel_requested, "the worker must be told to stop");
    assert_eq!(
        app.store
            .get_team_run(app.org_id, run)
            .await
            .unwrap()
            .status,
        "cancelled"
    );

    // While that execution is still stopping, the item cannot be requeued.
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team}/queue/{item}/requeue"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team}/queue/{item}/retry"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    sqlx::query("UPDATE executions SET status = 'cancelled', finished_at = now() WHERE id = $1")
        .bind(execution)
        .execute(&raw_pool().await)
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team}/queue/{item}/requeue"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");

    // Not yet claimed: the execution is cancelled outright.
    let (item, run) = start_queue_run(&app, team, "gh#299b").await;
    let (status, _) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team}/queue/{item}/cancel"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, exec_status, _) = run_execution(run).await;
    assert_eq!(exec_status, "cancelled");
}

#[tokio::test]
async fn team_runs_can_be_cancelled_directly() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let team = make_team(&app, vec![app.agent_id], json!({})).await;
    let (item, run) = start_queue_run(&app, team, "gh#299c").await;
    let (execution, _, _) = run_execution(run).await;
    app.store
        .claim_execution(execution, "worker-299")
        .await
        .unwrap();

    let (status, body) = send(
        &app.router,
        post(&format!("/api/v1/team-runs/{run}/cancel"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["run"]["status"], "cancelled");
    let (_, _, cancel_requested) = run_execution(run).await;
    assert!(cancel_requested);
    assert_eq!(
        app.store
            .get_team_queue_item(app.org_id, item)
            .await
            .unwrap()
            .status,
        "cancelled",
        "the queue item of a cancelled run is cancelled too"
    );

    let (status, _) = send(
        &app.router,
        post(&format!("/api/v1/team-runs/{run}/cancel"), json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a finished run cannot be cancelled again"
    );
}

#[tokio::test]
async fn deleting_a_team_stops_its_executions() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let team = make_team(&app, vec![app.agent_id], json!({})).await;
    let (_, run) = start_queue_run(&app, team, "gh#299d").await;
    let (execution, _, _) = run_execution(run).await;
    app.store
        .claim_execution(execution, "worker-299")
        .await
        .unwrap();

    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/agent-teams/{team}"))
        .body(Body::empty())
        .unwrap();
    let (status, body) = send(&app.router, request).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let cancel_requested: bool =
        sqlx::query_scalar("SELECT cancel_requested FROM executions WHERE id = $1")
            .bind(execution)
            .fetch_one(&raw_pool().await)
            .await
            .unwrap();
    assert!(
        cancel_requested,
        "a detached execution must still be stopped"
    );
}

#[tokio::test]
async fn failed_launch_leaves_no_running_orphan() {
    let Some(app) = test_app(true).await else {
        return;
    };
    // auto_launch with a worker profile that does not exist: launch fails after the run
    // and its step/execution were created.
    let team = make_team(
        &app,
        vec![app.agent_id],
        json!({ "autoLaunch": true, "workerProfileId": Uuid::now_v7() }),
    )
    .await;
    let item = queue_item(&app, team, "gh#2", 3).await;
    let (status, _) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team}/queue/{item}/run"),
            json!({}),
        ),
    )
    .await;
    assert!(!status.is_success(), "{status}");
    let stored = app
        .store
        .get_team_queue_item(app.org_id, item)
        .await
        .unwrap();
    assert_eq!(stored.status, "failed");
    assert!(stored.team_run_id.is_none());
    assert!(stored.error.is_some());
    let running: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM team_runs WHERE team_id = $1 AND status = 'running'",
    )
    .bind(team)
    .fetch_one(&raw_pool().await)
    .await
    .unwrap();
    assert_eq!(running, 0);
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM executions e JOIN team_runs r ON r.id = e.team_run_id
         WHERE r.team_id = $1
           AND e.status NOT IN ('cancelled','failed','succeeded','timed_out','rejected')",
    )
    .bind(team)
    .fetch_one(&raw_pool().await)
    .await
    .unwrap();
    assert_eq!(live, 0, "no executable orphan execution");
    // The failed item is retryable.
    assert!(app
        .store
        .claim_team_queue_item(app.org_id, item)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn retry_and_requeue_cannot_revive_succeeded_and_retry_respects_max_attempts() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let team = make_team(&app, vec![app.agent_id], json!({})).await;
    let done = queue_item(&app, team, "gh#3", 3).await;
    app.store
        .set_team_queue_status(app.org_id, done, "succeeded", None)
        .await
        .unwrap();
    for action in ["retry", "requeue", "cancel"] {
        let (status, _) = send(
            &app.router,
            post(
                &format!("/api/v1/agent-teams/{team}/queue/{done}/{action}"),
                json!({}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{action}");
    }
    assert_eq!(
        app.store
            .get_team_queue_item(app.org_id, done)
            .await
            .unwrap()
            .status,
        "succeeded"
    );

    // max_attempts = 1: the first manual retry is allowed, the next is refused.
    let failed = queue_item(&app, team, "gh#4", 1).await;
    app.store
        .set_team_queue_status(app.org_id, failed, "failed", Some("boom"))
        .await
        .unwrap();
    let url = format!("/api/v1/agent-teams/{team}/queue/{failed}/retry");
    let (status, body) = send(&app.router, post(&url, json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["attempts"], 1);
    app.store
        .set_team_queue_status(app.org_id, failed, "failed", Some("boom"))
        .await
        .unwrap();
    let (status, _) = send(&app.router, post(&url, json!({}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // requeue is the explicit reset.
    let (status, _) = send(
        &app.router,
        post(
            &format!("/api/v1/agent-teams/{team}/queue/{failed}/requeue"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn terminal_run_with_unclosed_queue_item_is_reconciled() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let team = make_team(&app, vec![app.agent_id], json!({})).await;
    let item = queue_item(&app, team, "gh#5", 1).await;
    app.store
        .claim_team_queue_item(app.org_id, item)
        .await
        .unwrap()
        .unwrap();
    let run = app
        .store
        .create_team_run(app.org_id, team, &json!({}), None)
        .await
        .unwrap();
    assert!(app
        .store
        .attach_team_run_to_queue_item(app.org_id, item, run.id)
        .await
        .unwrap());
    // Run reached a terminal state but close_queue_loop never completed.
    app.store
        .finish_team_run(run.id, "failed", None, Some("boom"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        app.store
            .get_team_queue_item(app.org_id, item)
            .await
            .unwrap()
            .status,
        "running"
    );
    orbit_api::handlers::teams::reconcile_team_outcomes(&app.state)
        .await
        .unwrap();
    let stored = app
        .store
        .get_team_queue_item(app.org_id, item)
        .await
        .unwrap();
    assert_eq!(stored.status, "failed");
    assert_eq!(stored.error.as_deref(), Some("boom"));
}

#[tokio::test]
async fn enqueue_happens_before_claim_and_claim_failure_withdraws_the_item() {
    let Some(app) = test_app(true).await else {
        return;
    };
    use orbit_api::dispatcher::{enqueue_then_claim, EnqueueOutcome};
    let team_id = make_team(&app, vec![app.agent_id], json!({})).await;
    let team = app.store.get_agent_team(app.org_id, team_id).await.unwrap();
    let payload = json!({ "task": "t" });

    // Claim only runs after the item is durable.
    let store = app.store.clone();
    let outcome = enqueue_then_claim(
        &app.store,
        &team,
        "gh#6",
        None,
        &payload,
        1,
        || async move {
            let keys = store.list_team_queue_external_keys(team_id, 10).await?;
            anyhow::ensure!(
                keys.contains(&"gh#6".to_string()),
                "item not enqueued before claim"
            );
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(outcome, EnqueueOutcome::Enqueued);

    // Already queued: no claim attempted.
    let outcome = enqueue_then_claim(&app.store, &team, "gh#6", None, &payload, 1, || async {
        anyhow::bail!("must not claim")
    })
    .await
    .unwrap();
    assert_eq!(outcome, EnqueueOutcome::AlreadyQueued);

    // Claim failure withdraws the item so the next poll retries.
    let outcome = enqueue_then_claim(&app.store, &team, "gh#7", None, &payload, 1, || async {
        anyhow::bail!("label update failed")
    })
    .await
    .unwrap();
    assert!(matches!(outcome, EnqueueOutcome::ClaimFailed(_)));
    let keys = app
        .store
        .list_team_queue_external_keys(team_id, 10)
        .await
        .unwrap();
    assert!(!keys.contains(&"gh#7".to_string()));

    // Enqueue (database) failure: the claim is never attempted, so the issue keeps its labels.
    let mut broken = team.clone();
    broken.id = Uuid::now_v7();
    let claimed = std::sync::atomic::AtomicBool::new(false);
    let result = enqueue_then_claim(&app.store, &broken, "gh#8", None, &payload, 1, || async {
        claimed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    })
    .await;
    assert!(result.is_err());
    assert!(!claimed.load(std::sync::atomic::Ordering::SeqCst));
}

// ---------------------------------------------------------------------------
// Output delivery (#112, restored by #191)
// ---------------------------------------------------------------------------

mod output_delivery {
    use super::*;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post as route_post;
    use orbit_api::handlers::outputs::deliver_pending;
    use orbit_security::SecretValue;
    use orbit_store::Output;
    use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;

    /// deliver_pending is global across orgs; serialize these tests so counts are exact.
    fn guard() -> &'static tokio::sync::Mutex<()> {
        static GUARD: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        // The test hooks listen on 127.0.0.1; the outbound guard (#142) blocks loopback
        // unless explicitly allowlisted.
        static ALLOW: std::sync::Once = std::sync::Once::new();
        ALLOW.call_once(|| std::env::set_var("ORBIT_OUTBOUND_ALLOWED_HOSTS", "127.0.0.1"));
        GUARD.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    #[derive(Clone)]
    struct Hook {
        url: String,
        hits: Arc<StdMutex<Vec<HeaderMap>>>,
        status: Arc<AtomicU16>,
        delay_ms: Arc<AtomicU64>,
    }

    impl Hook {
        fn count(&self) -> usize {
            self.hits.lock().unwrap().len()
        }
    }

    async fn start_hook() -> Hook {
        let hook = Hook {
            url: String::new(),
            hits: Arc::new(StdMutex::new(Vec::new())),
            status: Arc::new(AtomicU16::new(200)),
            delay_ms: Arc::new(AtomicU64::new(0)),
        };
        async fn handle(State(hook): State<Hook>, headers: HeaderMap) -> StatusCode {
            hook.hits.lock().unwrap().push(headers);
            let delay = hook.delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            StatusCode::from_u16(hook.status.load(Ordering::SeqCst)).unwrap()
        }
        let router = axum::Router::new()
            .route("/hook", route_post(handle))
            .with_state(hook.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Hook { url, ..hook }
    }

    async fn make_output(app: &TestApp, hook: &Hook, kind: &str) -> Output {
        let secret_ref = format!("secret://output/{}", Uuid::now_v7());
        app.store
            .put_secret(
                Some(app.org_id),
                &secret_ref,
                &SecretValue::new(hook.url.clone()),
                "test hook",
            )
            .await
            .unwrap();
        app.store
            .create_output(
                app.org_id,
                &format!("hook-{}", Uuid::now_v7()),
                kind,
                None,
                None,
                &["succeeded".to_string(), "failed".to_string()],
                &secret_ref,
                None,
                None,
                None,
                true,
            )
            .await
            .unwrap()
    }

    async fn finished_execution(app: &TestApp) -> Uuid {
        let path = format!("/api/v1/agents/{}/executions", app.agent_id);
        let request = Request::builder()
            .method("POST")
            .uri(&path)
            .header("content-type", "application/json")
            .header("idempotency-key", Uuid::now_v7().to_string())
            .body(Body::from(json!({ "task": "notify" }).to_string()))
            .unwrap();
        let (status, body) = send(&app.router, request).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
        app.store.claim_execution(id, "test-worker").await.unwrap();
        app.store.mark_execution_running(id, None).await.unwrap();
        app.store
            .finish_execution(
                id,
                orbit_domain::ExecutionStatus::Succeeded,
                Some(&json!({ "text": "done" })),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        id
    }

    /// Pretend the run finished `hours` ago (e.g. while the API was down).
    async fn backdate_execution(app: &TestApp, id: Uuid, hours: i32) {
        sqlx::query(
            "UPDATE executions SET finished_at = now() - make_interval(hours => $2), \
             updated_at = now() - make_interval(hours => $2) WHERE id = $1",
        )
        .bind(id)
        .bind(hours)
        .execute(app.store.pool())
        .await
        .unwrap();
    }

    async fn backdate_output(app: &TestApp, id: Uuid, hours: i32) {
        sqlx::query(
            "UPDATE outputs SET enqueue_from = now() - make_interval(hours => $2), \
             enqueue_cursor = now() - make_interval(hours => $2) WHERE id = $1",
        )
        .bind(id)
        .bind(hours)
        .execute(app.store.pool())
        .await
        .unwrap();
    }

    async fn rows(app: &TestApp, output_id: Uuid) -> Vec<(String, i32, Option<i32>)> {
        sqlx::query_as(
            "SELECT status, attempts, response_status FROM output_deliveries \
             WHERE output_id = $1 ORDER BY created_at",
        )
        .bind(output_id)
        .fetch_all(app.store.pool())
        .await
        .unwrap()
    }

    async fn make_due(app: &TestApp, output_id: Uuid) {
        sqlx::query("UPDATE output_deliveries SET next_attempt_at = now() WHERE output_id = $1")
            .bind(output_id)
            .execute(app.store.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn concurrent_deliver_pending_sends_each_delivery_once() {
        let Some(app) = test_app(true).await else {
            return;
        };
        let _guard = guard().lock().await;
        let hook = start_hook().await;
        hook.delay_ms.store(300, Ordering::SeqCst);
        let output = make_output(&app, &hook, "webhook").await;
        for _ in 0..3 {
            finished_execution(&app).await;
        }
        tokio::join!(
            deliver_pending(&app.state),
            deliver_pending(&app.state),
            deliver_pending(&app.state)
        );
        assert_eq!(hook.count(), 3, "each delivery sent exactly once");
        let rows = rows(&app, output.id).await;
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter().all(|r| r.0 == "succeeded" && r.1 == 1),
            "{rows:?}"
        );
        // Idempotency headers carry the delivery id and are unique per delivery.
        let ids: std::collections::HashSet<String> = hook
            .hits
            .lock()
            .unwrap()
            .iter()
            .map(|h| h["idempotency-key"].to_str().unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 3);
        assert!(hook
            .hits
            .lock()
            .unwrap()
            .iter()
            .all(|h| h.contains_key("x-orbit-delivery-id")));
        // A later tick does not resend.
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 3);
    }

    #[tokio::test]
    async fn interrupted_send_is_retried_after_lease_expiry() {
        let Some(app) = test_app(true).await else {
            return;
        };
        let _guard = guard().lock().await;
        let hook = start_hook().await;
        let output = make_output(&app, &hook, "slack").await;
        finished_execution(&app).await;
        app.store.enqueue_output_deliveries().await.unwrap();
        // A replica claims the row and "crashes" before sending.
        let claimed = app
            .store
            .claim_output_deliveries(10, chrono::Duration::seconds(120))
            .await
            .unwrap();
        assert!(claimed.iter().any(|j| j.output_id == output.id));
        // While the lease is live, nobody else sends it.
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 0);
        assert_eq!(rows(&app, output.id).await[0].0, "sending");
        // Lease expires: the row is reclaimed and delivered; the stale claim cannot finish.
        sqlx::query("UPDATE output_deliveries SET lease_until = now() - interval '1 second' WHERE output_id = $1")
            .bind(output.id)
            .execute(app.store.pool())
            .await
            .unwrap();
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 1);
        let stale = claimed.iter().find(|j| j.output_id == output.id).unwrap();
        let finished = app
            .store
            .finish_output_delivery(
                stale.id,
                stale.claim_token,
                orbit_store::outputs::DeliveryOutcome::Failed,
                None,
                Some("stale"),
                stale.attempts,
            )
            .await
            .unwrap();
        assert!(!finished, "stale claim must not overwrite the result");
        let rows = rows(&app, output.id).await;
        assert_eq!(rows[0].0, "succeeded");
        assert_eq!(rows[0].1, 2, "interrupted attempt counts");
    }

    #[tokio::test]
    async fn runs_finished_during_long_outage_are_delivered_once() {
        let Some(app) = test_app(true).await else {
            return;
        };
        let _guard = guard().lock().await;
        let hook = start_hook().await;
        let output = make_output(&app, &hook, "webhook").await;
        // Output existed 5h ago; the API has been down since; the run finished 2h ago.
        backdate_output(&app, output.id, 5).await;
        let id = finished_execution(&app).await;
        backdate_execution(&app, id, 2).await;
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 1);
        deliver_pending(&app.state).await;
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 1, "exactly once");
        assert_eq!(rows(&app, output.id).await.len(), 1);
    }

    #[tokio::test]
    async fn new_output_does_not_backfill_old_runs() {
        let Some(app) = test_app(true).await else {
            return;
        };
        let _guard = guard().lock().await;
        let hook = start_hook().await;
        finished_execution(&app).await;
        let old = finished_execution(&app).await;
        backdate_execution(&app, old, 2).await;
        let output = make_output(&app, &hook, "webhook").await;
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 0);
        assert!(rows(&app, output.id).await.is_empty());
        // A run that finishes after creation is delivered.
        finished_execution(&app).await;
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 1);
    }

    #[tokio::test]
    async fn client_errors_are_permanent_and_server_errors_retry() {
        let Some(app) = test_app(true).await else {
            return;
        };
        let _guard = guard().lock().await;

        // 404: permanent, never retried.
        let hook = start_hook().await;
        hook.status.store(404, Ordering::SeqCst);
        let output = make_output(&app, &hook, "slack").await;
        finished_execution(&app).await;
        deliver_pending(&app.state).await;
        assert_eq!(
            rows(&app, output.id).await,
            vec![("failed".to_string(), 1, Some(404))]
        );
        make_due(&app, output.id).await;
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 1, "4xx is not retried");

        // 503: retried with backoff, dead after the final attempt.
        let hook = start_hook().await;
        hook.status.store(503, Ordering::SeqCst);
        let output = make_output(&app, &hook, "mattermost").await;
        finished_execution(&app).await;
        deliver_pending(&app.state).await;
        assert_eq!(
            rows(&app, output.id).await,
            vec![("pending".to_string(), 1, Some(503))]
        );
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 1, "backoff holds the retry");
        make_due(&app, output.id).await;
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 2);
        sqlx::query("UPDATE output_deliveries SET attempts = 4, next_attempt_at = now() WHERE output_id = $1")
            .bind(output.id)
            .execute(app.store.pool())
            .await
            .unwrap();
        deliver_pending(&app.state).await;
        assert_eq!(
            rows(&app, output.id).await,
            vec![("dead".to_string(), 5, Some(503))]
        );

        // 429 is retryable.
        let hook = start_hook().await;
        hook.status.store(429, Ordering::SeqCst);
        let output = make_output(&app, &hook, "slack").await;
        finished_execution(&app).await;
        deliver_pending(&app.state).await;
        assert_eq!(rows(&app, output.id).await[0].0, "pending");
    }

    #[tokio::test]
    async fn deleted_or_disabled_output_leaves_no_pending_rows() {
        let Some(app) = test_app(true).await else {
            return;
        };
        let _guard = guard().lock().await;
        let hook = start_hook().await;

        // Disabled after enqueue: skipped, nothing sent.
        let disabled = make_output(&app, &hook, "webhook").await;
        finished_execution(&app).await;
        app.store.enqueue_output_deliveries().await.unwrap();
        sqlx::query("UPDATE outputs SET enabled = false WHERE id = $1")
            .bind(disabled.id)
            .execute(app.store.pool())
            .await
            .unwrap();
        deliver_pending(&app.state).await;
        assert_eq!(hook.count(), 0);
        let rows_disabled = rows(&app, disabled.id).await;
        assert_eq!(rows_disabled.len(), 1);
        assert_eq!(rows_disabled[0].0, "skipped");

        // Deleted: its rows are removed with it (cascade), nothing pending remains.
        let deleted = make_output(&app, &hook, "webhook").await;
        finished_execution(&app).await;
        app.store.enqueue_output_deliveries().await.unwrap();
        app.store
            .delete_output(app.org_id, deleted.id)
            .await
            .unwrap();
        deliver_pending(&app.state).await;
        assert!(rows(&app, deleted.id).await.is_empty());
        assert_eq!(hook.count(), 0);
    }

    #[tokio::test]
    async fn delivery_history_shows_results_and_hides_runs_from_members() {
        let Some(app) = test_app(true).await else {
            return;
        };
        let _guard = guard().lock().await;
        let hook = start_hook().await;
        hook.status.store(404, Ordering::SeqCst);
        // Organisation-wide: it reports on every agent's runs.
        let output = make_output(&app, &hook, "slack").await;
        let execution = finished_execution(&app).await;
        deliver_pending(&app.state).await;

        // An admin sees the result and the run it reported on.
        let path = format!("/api/v1/outputs/{}/deliveries", output.id);
        let (status, body) = send(&app.router, get(&path)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let rows = body.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "failed");
        assert_eq!(rows[0]["attempts"], 1);
        assert_eq!(rows[0]["responseStatus"], 404);
        assert_eq!(rows[0]["error"], "webhook returned HTTP 404 Not Found");
        assert_eq!(rows[0]["executionId"], execution.to_string());
        assert_eq!(rows[0]["task"], "notify");
        assert!(
            rows[0]["nextAttemptAt"].is_null(),
            "only pending rows have one"
        );
        assert!(
            !body.to_string().contains(&hook.url),
            "the webhook URL never appears"
        );

        // The list carries the newest delivery, without run details.
        let (status, body) = send(&app.router, get("/api/v1/outputs")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let listed = body
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == output.id.to_string())
            .unwrap();
        assert_eq!(listed["lastDelivery"]["status"], "failed");
        assert!(listed["lastDelivery"]["executionId"].is_null());
        assert!(listed["lastDelivery"]["task"].is_null());

        // A member sees that it failed, but not which agent's run it was about.
        let fx = d2_fixture(&app).await;
        let (status, body) = send(&app.router, auth_get(&path, &fx.mallory_token)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let rows = body.as_array().unwrap();
        assert_eq!(rows[0]["status"], "failed");
        assert!(rows[0]["executionId"].is_null());
        assert!(rows[0]["task"].is_null());
        assert!(rows[0]["agentName"].is_null());

        let (status, _) = send(
            &app.router,
            get(&format!("/api/v1/outputs/{}/deliveries", Uuid::now_v7())),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

// ---------------------------------------------------------------------------
// #139: worker identity binding, memory scoping, launch visibility (restored, #191)
// ---------------------------------------------------------------------------

async fn worker_token(store: &Store, org_id: Uuid, name: &str) -> String {
    let issued = orbit_security::issue_worker_token();
    store
        .create_worker_token(
            org_id,
            None,
            name,
            &issued.prefix,
            &issued.hash,
            &["mock".to_string()],
        )
        .await
        .expect("worker token");
    issued.token
}

async fn queue_execution(app: &TestApp, key: &str) -> Uuid {
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{}/executions", app.agent_id),
            "test-token",
            json!({ "task": key }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
}

async fn claim_as(app: &TestApp, token: &str, worker_id: &str) -> Option<Uuid> {
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/claim",
            token,
            Some(json!({ "workerId": worker_id, "harnesses": ["mock"], "target": "local" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["execution"]["execution"]["id"]
        .as_str()
        .map(|id| Uuid::parse_str(id).unwrap())
}

/// #306: unknown event and memory kinds are validation errors, not database errors.
#[tokio::test]
async fn worker_event_and_memory_kinds_are_validated() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let token = worker_token(&app.store, app.org_id, "worker-kinds").await;
    queue_execution(&app, "kinds").await;
    let claimed = claim_as(&app, &token, "worker-kinds")
        .await
        .expect("claimed");
    let events = format!("/api/v1/worker/executions/{claimed}/events");
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            &events,
            &token,
            Some(json!({ "kind": "worker_oom", "level": "error", "message": "x" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            &events,
            &token,
            Some(json!({ "kind": "error", "level": "error", "message": "x", "data": { "code": "worker_oom" } })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/memory/store",
            &token,
            Some(json!({ "executionId": claimed, "kind": "long_term", "content": "x" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

/// #306: generating bootstrap output must not revoke the tokens of running workers, is a
/// POST (a GET must not change state), and a live launch cannot be deleted.
#[tokio::test]
async fn bootstrap_keeps_existing_worker_tokens_and_live_launches_are_kept() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let profile = d2_profile(&app, &format!("bootstrap-{}", Uuid::now_v7())).await;
    let issued = orbit_security::issue_worker_token();
    let launched = app
        .store
        .create_worker_token(
            app.org_id,
            Some(profile),
            "launch:running",
            &issued.prefix,
            &issued.hash,
            &["mock".to_string()],
        )
        .await
        .unwrap();
    let path = format!("/api/v1/worker-profiles/{profile}/bootstrap");
    for _ in 0..2 {
        let (status, body) = send(&app.router, post(&path, json!({}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let tokens = app.store.list_worker_tokens(app.org_id).await.unwrap();
    let profile_tokens: Vec<_> = tokens
        .iter()
        .filter(|token| token.profile_id == Some(profile))
        .collect();
    assert_eq!(
        profile_tokens.len(),
        3,
        "one launch token plus one per generation"
    );
    assert!(
        profile_tokens
            .iter()
            .all(|token| token.revoked_at.is_none()),
        "bootstrap output must not revoke other tokens"
    );
    assert!(profile_tokens.iter().any(|token| token.id == launched.id));
    let (status, _) = send(&app.router, get(&path)).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    let launch = app
        .store
        .create_worker_launch(
            app.org_id,
            profile,
            "orbit-worker-live",
            "docker",
            "orbit-worker:test",
            Some(launched.id),
            None,
            false,
        )
        .await
        .unwrap();
    let delete = |id: Uuid| {
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/worker-launches/{id}"))
            .body(Body::empty())
            .unwrap()
    };
    let (status, body) = send(&app.router, delete(launch.id)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    sqlx::query("UPDATE worker_launches SET status = 'stopped' WHERE id = $1")
        .bind(launch.id)
        .execute(&raw_pool().await)
        .await
        .unwrap();
    let (status, body) = send(&app.router, delete(launch.id)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

#[tokio::test]
async fn worker_cannot_act_on_execution_claimed_by_another_worker() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let token_a = worker_token(&app.store, app.org_id, "worker-a").await;
    let token_b = worker_token(&app.store, app.org_id, "worker-b").await;
    let queued = queue_execution(&app, "bound execution").await;

    // Unclaimed executions are not reachable through the worker surface.
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            &format!("/api/v1/worker/executions/{queued}/events"),
            &token_a,
            Some(json!({ "kind": "log", "level": "info", "message": "early" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let claimed = claim_as(&app, &token_a, "worker-a").await.expect("claimed");
    assert_eq!(claimed, queued);

    let base = format!("/api/v1/worker/executions/{claimed}");
    // Worker B is rejected on every execution-scoped endpoint, with the error envelope.
    let attempts = [
        ("GET", format!("{base}/context"), None),
        ("GET", format!("{base}/cancel"), None),
        (
            "POST",
            format!("{base}/running"),
            Some(json!({ "sessionRef": "s" })),
        ),
        (
            "POST",
            format!("{base}/events"),
            Some(json!({ "kind": "log", "level": "info", "message": "forged" })),
        ),
        (
            "POST",
            format!("{base}/complete"),
            Some(json!({ "status": "succeeded", "output": { "forged": true } })),
        ),
        (
            "POST",
            "/api/v1/worker/approvals".to_string(),
            Some(json!({ "executionId": claimed, "action": { "a": 1 } })),
        ),
        (
            "POST",
            "/api/v1/worker/audit".to_string(),
            Some(json!({ "executionId": claimed, "action": "x" })),
        ),
    ];
    for (method, path, body) in attempts {
        let (status, body) = send(&app.router, worker_request(method, &path, &token_b, body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        assert!(body["error"]["code"].is_string(), "{path}: {body}");
    }
    let untouched = app.store.get_execution(app.org_id, claimed).await.unwrap();
    assert_eq!(untouched.status, "dispatched");
    assert!(untouched.output.is_none());

    // The claiming worker still drives its execution end to end.
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            &format!("{base}/running"),
            &token_a,
            Some(json!({ "sessionRef": "s" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            &format!("{base}/events"),
            &token_a,
            Some(json!({ "kind": "log", "level": "info", "message": "ok" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            &format!("{base}/complete"),
            &token_a,
            Some(json!({ "status": "succeeded", "output": { "ok": true } })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["updated"], true);
}

#[tokio::test]
async fn illegal_status_transitions_are_rejected() {
    let Some(app) = test_app(true).await else {
        return;
    };
    // queued -> succeeded is refused by the store.
    let queued = queue_execution(&app, "illegal transition").await;
    let result = app
        .store
        .finish_execution(
            queued,
            orbit_domain::ExecutionStatus::Succeeded,
            Some(&json!({ "forged": true })),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(result.is_none());
    let current = app.store.get_execution(app.org_id, queued).await.unwrap();
    assert_eq!(current.status, "queued");
    assert!(current.output.is_none());

    // Cancellation from queued is still allowed (and frees the agent's concurrency slot).
    let cancelled = app
        .store
        .request_execution_cancel(app.org_id, queued)
        .await
        .unwrap();
    assert_eq!(cancelled.status, "cancelled");

    // dispatched -> succeeded (skipping `running`) is refused through the API too.
    queue_execution(&app, "dispatched transition").await;
    let token = worker_token(&app.store, app.org_id, "worker-t").await;
    let claimed = claim_as(&app, &token, "worker-t").await.unwrap();
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            &format!("/api/v1/worker/executions/{claimed}/complete"),
            &token,
            Some(json!({ "status": "succeeded", "output": { "x": 1 } })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["updated"], false);
    let current = app.store.get_execution(app.org_id, claimed).await.unwrap();
    assert_eq!(current.status, "dispatched");
}

#[tokio::test]
async fn worker_memory_requires_a_bound_execution_and_uses_its_agent() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let token_a = worker_token(&app.store, app.org_id, "mem-a").await;
    let token_b = worker_token(&app.store, app.org_id, "mem-b").await;
    let execution = queue_execution(&app, "memory").await;
    claim_as(&app, &token_a, "mem-a").await.expect("claimed");
    let owner = app.store.list_users(app.org_id).await.unwrap()[0].id;
    let other_agent = create_demo_agent(&app.store, app.org_id, owner).await;

    // execution_id is mandatory.
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/memory/store",
            &token_a,
            Some(json!({ "agentId": other_agent, "kind": "short_term", "content": "x" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Another worker cannot use the execution to reach the agent's memory.
    for path in ["list", "store"] {
        let (status, _) = send(
            &app.router,
            worker_request(
                "POST",
                &format!("/api/v1/worker/memory/{path}"),
                &token_b,
                Some(json!({ "executionId": execution, "kind": "k", "content": "x" })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }

    // A caller-supplied agent id that differs from the execution's agent is refused.
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/memory/store",
            &token_a,
            Some(json!({
                "executionId": execution,
                "agentId": other_agent,
                "kind": "short_term",
                "content": "poison"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(app
        .store
        .list_memory(app.org_id, other_agent, None, 20)
        .await
        .unwrap()
        .is_empty());

    // Without agentId the entry lands on the execution's own agent.
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/memory/store",
            &token_a,
            Some(json!({ "executionId": execution, "kind": "short_term", "content": "mine" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, listed) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/memory/list",
            &token_a,
            Some(json!({ "executionId": execution })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed[0]["content"], "mine");

    // Oversize content is rejected with a 413 envelope.
    let (status, body) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/memory/store",
            &token_a,
            Some(json!({
                "executionId": execution,
                "kind": "short_term",
                "content": "x".repeat(64 * 1024 + 1)
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"]["code"], "payload_too_large");
}

#[tokio::test]
async fn worker_offline_and_heartbeat_are_owner_scoped() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let token_a = worker_token(&app.store, app.org_id, "hb-a").await;
    let token_b = worker_token(&app.store, app.org_id, "hb-b").await;
    let worker_id = format!("hb-worker-{}", Uuid::now_v7());
    let beat = json!({ "workerId": worker_id, "harnesses": ["mock"], "target": "local" });
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/heartbeat",
            &token_a,
            Some(beat.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Another token can neither take over nor delete the record.
    let (status, _) = send(
        &app.router,
        worker_request("POST", "/api/v1/worker/heartbeat", &token_b, Some(beat)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        &app.router,
        worker_request(
            "DELETE",
            &format!("/api/v1/worker/heartbeat/{worker_id}"),
            &token_b,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let listed = app.store.list_worker_heartbeats(app.org_id).await.unwrap();
    assert!(listed.iter().any(|w| w.worker_id == worker_id));
    assert!(app
        .store
        .list_worker_heartbeats(Uuid::now_v7())
        .await
        .unwrap()
        .is_empty());

    let (status, _) = send(
        &app.router,
        worker_request(
            "DELETE",
            &format!("/api/v1/worker/heartbeat/{worker_id}"),
            &token_a,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn worker_launches_follow_profile_visibility() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "launch-alice@test.local",
            "Alice",
            UserRole::Viewer,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(
            app.org_id,
            "launch-bob@test.local",
            "Bob",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;
    let profile = app
        .store
        .create_worker_profile(
            app.org_id,
            "Bob private worker",
            "mock",
            "local",
            "process",
            "orbit-worker",
            "test",
            "all",
            &[],
            &[],
            None,
            None,
            None,
            &json!({}),
            &json!({}),
            &json!({}),
            false,
            false,
            false,
            false,
            &json!({}),
        )
        .await
        .unwrap();
    make_private(
        &app.store,
        app.org_id,
        "worker_profiles",
        profile.id,
        bob.id,
    )
    .await;
    let launch = app
        .store
        .create_worker_launch(
            app.org_id,
            profile.id,
            &profile.name,
            "local",
            "orbit-worker:test",
            None,
            Some(bob.id),
            false,
        )
        .await
        .unwrap();

    let (status, listed) = send(
        &app.router,
        auth_get("/api/v1/worker-launches?includeInactive=true", &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["id"] != launch.id.to_string()));
    for suffix in ["", "/logs"] {
        let (status, _) = send(
            &app.router,
            auth_get(
                &format!("/api/v1/worker-launches/{}{suffix}", launch.id),
                &alice_token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "alice {suffix}");
    }

    // The profile owner sees it (logs then fail only because no launcher is configured).
    let (status, listed) = send(
        &app.router,
        auth_get("/api/v1/worker-launches?includeInactive=true", &bob_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == launch.id.to_string()));
    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/worker-launches/{}", launch.id),
            &bob_token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/worker-launches/{}/logs", launch.id),
            &bob_token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// #44: embedded docs (restored, #191)
// ---------------------------------------------------------------------------

async fn send_raw(
    router: &axum::Router,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, bytes.to_vec())
}

#[tokio::test]
async fn docs_index_lists_categories_and_excludes_planning() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(&app.router, get("/api/v1/docs")).await;
    assert_eq!(status, StatusCode::OK);
    let categories = body["categories"].as_array().expect("categories");
    assert!(categories.len() >= 4);
    let ids: Vec<&str> = categories.iter().filter_map(|c| c["id"].as_str()).collect();
    // Section ids come from docs/navigation.yaml when present, else from folder names;
    // "architecture" exists under both schemes.
    assert!(ids.contains(&"architecture"), "{ids:?}");
    let slugs: Vec<&str> = categories
        .iter()
        .flat_map(|c| c["pages"].as_array().unwrap())
        .filter_map(|p| p["slug"].as_str())
        .collect();
    assert!(slugs.contains(&"architecture/overview"));
    assert!(!slugs.iter().any(|s| s.starts_with("planning")));
    let first = &categories[0]["pages"][0];
    assert!(first["title"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(first["url"].as_str().unwrap().starts_with("/api/v1/docs/"));

    let (status, headers, bytes) = send_raw(&app.router, get("/api/v1/docs?format=html")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert!(String::from_utf8_lossy(&bytes).contains("architecture/overview"));
}

#[tokio::test]
async fn docs_page_rewrites_links_and_supports_html() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(&app.router, get("/api/v1/docs/index")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], "index");
    let markdown = body["markdown"].as_str().unwrap();
    assert!(
        markdown.contains("](/api/v1/docs/architecture/overview)"),
        "{markdown}"
    );
    assert!(!markdown.contains("overview.md)"));

    let (status, headers, bytes) = send_raw(
        &app.router,
        get("/api/v1/docs/architecture/overview?format=markdown"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/markdown"));
    assert!(String::from_utf8_lossy(&bytes).starts_with("# "));

    let request = Request::builder()
        .uri("/api/v1/docs/index")
        .header("accept", "text/html")
        .body(Body::empty())
        .unwrap();
    let (status, headers, bytes) = send_raw(&app.router, request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert!(headers.contains_key("content-security-policy"));
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("<h1 id="));
    assert!(html.contains("href=\"/api/v1/docs/"));
}

#[tokio::test]
async fn docs_assets_are_served_with_content_type() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, headers, bytes) = send_raw(
        &app.router,
        get("/api/v1/docs/assets/architecture/orbit-overview.png"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/png");
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));

    let (status, headers, _) = send_raw(
        &app.router,
        get("/api/v1/docs/assets/architecture/orbit-overview.svg"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/svg+xml");

    // A page whose slug starts with `assets/` is still reachable.
    let (status, body) = send(&app.router, get("/api/v1/docs/assets/brand")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], "assets/brand");
}

#[tokio::test]
async fn docs_reject_traversal_unknown_and_planning() {
    let Some(app) = test_app(true).await else {
        return;
    };
    for path in [
        "/api/v1/docs/../Cargo.toml",
        "/api/v1/docs/..%2F..%2FCargo.toml",
        "/api/v1/docs/%2e%2e/%2e%2e/Cargo.toml",
        "/api/v1/docs/assets/../../Cargo.toml",
        "/api/v1/docs/assets/..%2F..%2FCargo.toml",
        "/api/v1/docs/does/not/exist",
        "/api/v1/docs/planning/provider-ux-and-harness-decoupling",
        "/api/v1/docs/assets/planning/provider-ux-and-harness-decoupling.md",
        "/api/v1/docs/assets/nope.png",
    ] {
        let (status, body) = send(&app.router, get(path)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(body["error"]["code"], "not_found", "{path}");
        assert!(!body["error"]["requestId"].as_str().unwrap_or("").is_empty());
    }
}

#[tokio::test]
async fn docs_require_authentication_but_swagger_stays_public() {
    let Some(app) = test_app(false).await else {
        return;
    };
    for path in [
        "/api/v1/docs",
        "/api/v1/docs/index",
        "/api/v1/docs/assets/architecture/orbit-overview.png",
    ] {
        let (status, body) = send(&app.router, get(path)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(body["error"]["code"], "unauthorized");
    }
    let (status, _) = send(&app.router, auth_get("/api/v1/docs", "test-token")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app.router, get("/api/v1/openapi.json")).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&app.router, get("/api/v1/meta")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["docsUrl"], "/api/v1/docs");
}

// ---------------------------------------------------------------------------
// #140: shared execution preflight and permission checks (restored, #191)
// ---------------------------------------------------------------------------

async fn token_with_scopes(
    store: &Store,
    org_id: Uuid,
    user_id: Uuid,
    name: &str,
    scopes: &[&str],
) -> String {
    let issued = orbit_security::issue_api_token();
    let scopes: Vec<String> = scopes.iter().map(|scope| (*scope).to_string()).collect();
    store
        .create_api_token(
            org_id,
            user_id,
            name,
            &issued.prefix,
            &issued.hash,
            &scopes,
            None,
        )
        .await
        .expect("api token");
    issued.token
}

fn auth_json(method: &str, path: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn webhook_connection(app: &TestApp, name: &str, owner: Uuid) -> Uuid {
    let connector = app
        .store
        .get_connector_by_slug(app.org_id, "webhook")
        .await
        .unwrap()
        .expect("webhook connector");
    let connection = app
        .store
        .create_connection(
            app.org_id,
            connector.id,
            name,
            &json!({ "url": "http://127.0.0.1:9/hook" }),
            &[],
            None,
            Some(owner),
        )
        .await
        .unwrap();
    make_private(&app.store, app.org_id, "connections", connection.id, owner).await;
    connection.id
}

/// Agent owned by `owner` with optional connection binding and hourly quota.
async fn bound_agent(
    app: &TestApp,
    owner: Uuid,
    connection: Option<Uuid>,
    max_per_hour: Option<u32>,
) -> Uuid {
    let mut value = definition();
    if let Some(connection) = connection {
        value["spec"]["connectors"] =
            json!([{ "connector": "webhook", "connectionId": connection }]);
    }
    if let Some(max) = max_per_hour {
        value["spec"]["autonomy"]["maxExecutionsPerHour"] = json!(max);
    }
    let definition: orbit_domain::AgentDefinition = serde_json::from_value(value).unwrap();
    let (agent, _) = app
        .store
        .create_agent(app.org_id, Some(owner), &definition)
        .await
        .unwrap();
    app.store
        .set_agent_status(app.org_id, agent.id, orbit_domain::AgentStatus::Active)
        .await
        .unwrap();
    app.store
        .assign_resource_owner(app.org_id, "agents", agent.id, owner)
        .await
        .unwrap();
    agent.id
}

async fn share_org(app: &TestApp, table: &str, id: Uuid) {
    app.store
        .set_resource_visibility(app.org_id, table, id, "org", None)
        .await
        .unwrap();
}

async fn operator(app: &TestApp, email: &str) -> orbit_store::User {
    app.store
        .ensure_user(app.org_id, email, email, UserRole::Operator)
        .await
        .unwrap()
}

async fn access_team(app: &TestApp, slug: &str) -> Uuid {
    app.store
        .create_access_team(app.org_id, slug, slug, None)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn agent_specs_require_use_of_referenced_connections_and_providers() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = operator(&app, "pf-spec-alice@test.local").await;
    let bob = operator(&app, "pf-spec-bob@test.local").await;
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;
    let alice_connection = webhook_connection(&app, "Alice hook", alice.id).await;

    let mut with_connection = definition();
    with_connection["spec"]["connectors"] =
        json!([{ "connector": "webhook", "connectionId": alice_connection }]);

    // Invisible connection: 404 with the field path in the envelope details.
    let (status, body) = send(
        &app.router,
        auth_post("/api/v1/agents", &bob_token, with_connection.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        body["error"]["details"][0]["path"],
        "spec.connectors[0].connectionId"
    );

    // The owner can bind it.
    let (status, body) = send(
        &app.router,
        auth_post("/api/v1/agents", &alice_token, with_connection.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Updating an existing agent re-checks the references.
    let bob_agent = bound_agent(&app, bob.id, None, None).await;
    let (status, _) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/agents/{bob_agent}"),
            &bob_token,
            json!({ "definition": with_connection.clone() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Visible but not usable (team viewer): 403.
    let team = access_team(&app, "pf-spec-team").await;
    app.store
        .upsert_team_member(team, bob.id, "viewer")
        .await
        .unwrap();
    app.store
        .set_resource_visibility(
            app.org_id,
            "connections",
            alice_connection,
            "team",
            Some(team),
        )
        .await
        .unwrap();
    let (status, _) = send(
        &app.router,
        auth_post("/api/v1/agents", &bob_token, with_connection),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Providers: a private provider cannot be bound by another user.
    let provider = app
        .store
        .create_provider(
            app.org_id,
            "pf-alice-provider",
            "openai_compatible",
            &json!({ "baseUrl": "http://127.0.0.1:9/v1" }),
            None,
            true,
        )
        .await
        .unwrap();
    make_private(&app.store, app.org_id, "providers", provider.id, alice.id).await;
    let mut with_provider = definition();
    with_provider["spec"]["model"] =
        json!({ "management": "platform", "provider": "pf-alice-provider", "model": "m" });
    let (status, body) = send(
        &app.router,
        auth_post("/api/v1/agents", &bob_token, with_provider.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["details"][0]["path"], "spec.model.provider");
    let (status, body) = send(
        &app.router,
        auth_post("/api/v1/agents", &alice_token, with_provider),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn agent_writes_reject_connector_bindings_without_connections() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let user = operator(&app, "connector-required@test.local").await;
    let token = token_for(&app.store, app.org_id, user.id, "connector-required").await;
    webhook_connection(&app, "Connector required hook", user.id).await;

    let mut invalid = definition();
    invalid["spec"]["connectors"] =
        json!([{ "connector": "webhook", "capabilities": ["messages.send"] }]);

    let assert_envelope = |body: &Value| {
        assert_eq!(body["error"]["code"], "validation_error");
        assert!(body["error"]["requestId"].is_string());
        let details = body["error"]["details"].as_array().unwrap();
        assert!(
            details.iter().any(|issue| {
                issue["path"] == "spec.connectors[0].connectionId"
                    && issue["message"].as_str().unwrap().contains("connectionId")
            }),
            "{body}"
        );
    };

    let (status, body) = send(
        &app.router,
        auth_post("/api/v1/agents", &token, invalid.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_envelope(&body);

    let agent = bound_agent(&app, user.id, None, None).await;
    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/agents/{agent}"),
            &token,
            json!({ "definition": invalid }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_envelope(&body);

    // A disabled connector (absent from the spec) and an empty connector list stay valid.
    let (status, body) = send(
        &app.router,
        auth_post("/api/v1/agents", &token, definition()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn rerun_applies_the_execute_preflight() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = operator(&app, "pf-rerun-alice@test.local").await;
    let bob = operator(&app, "pf-rerun-bob@test.local").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;
    // Alice owns the connection and shares it org-wide; bob's agent binds it.
    let connection = webhook_connection(&app, "Shared hook", alice.id).await;
    share_org(&app, "connections", connection).await;
    let agent = bound_agent(&app, bob.id, Some(connection), None).await;

    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{agent}/executions"),
            &bob_token,
            json!({ "task": "first" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let execution_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    app.store
        .request_execution_cancel(app.org_id, execution_id)
        .await
        .unwrap();

    // Baseline: rerun works while bob can use the connection.
    let rerun = format!("/api/v1/executions/{execution_id}/rerun");
    let (status, body) = send(&app.router, auth_post(&rerun, &bob_token, json!({}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let second: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    app.store
        .request_execution_cancel(app.org_id, second)
        .await
        .unwrap();

    // Connection access revoked (alice makes it private again): the rerun is refused.
    make_private(&app.store, app.org_id, "connections", connection, alice.id).await;
    let (status, body) = send(&app.router, auth_post(&rerun, &bob_token, json!({}))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        body["error"]["details"][0]["path"],
        "spec.connectors[0].connectionId"
    );

    // Quota: maxExecutionsPerHour is enforced with 429.
    let limited = bound_agent(&app, bob.id, None, Some(1)).await;
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{limited}/executions"),
            &bob_token,
            json!({ "task": "once" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let first: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    app.store
        .request_execution_cancel(app.org_id, first)
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/executions/{first}/rerun"),
            &bob_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");

    // A harness that cannot run executions blocks the rerun.
    let planned = HarnessRegistry::builtin()
        .descriptors()
        .into_iter()
        .find(|descriptor| descriptor.kind == orbit_domain::HarnessKind::ClaudeCode)
        .expect("claude code descriptor");
    assert_ne!(planned.implementation_status.as_str(), "available");
    app.store.upsert_harness_descriptor(&planned).await.unwrap();
    let plain = bound_agent(&app, bob.id, None, None).await;
    let agent_row = app.store.get_agent(app.org_id, plain).await.unwrap();
    let version = app
        .store
        .get_current_version(app.org_id, &agent_row)
        .await
        .unwrap();
    let blocked = app
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: app.org_id,
            agent_id: plain,
            agent_version_id: version.id,
            agent_name: &agent_row.name,
            harness_kind: orbit_domain::HarnessKind::ClaudeCode,
            trigger_source: orbit_domain::TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: json!({ "task": "x" }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(bob.id),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    app.store
        .request_execution_cancel(app.org_id, blocked.id)
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/executions/{}/rerun", blocked.id),
            &bob_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["path"], "harness");
}

#[tokio::test]
async fn cancel_and_team_runs_require_use_not_just_view() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = operator(&app, "pf-use-alice@test.local").await;
    let viola = operator(&app, "pf-use-viola@test.local").await;
    let mark = operator(&app, "pf-use-mark@test.local").await;
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let viola_token = token_for(&app.store, app.org_id, viola.id, "viola").await;
    let mark_token = token_for(&app.store, app.org_id, mark.id, "mark").await;
    let team = access_team(&app, "pf-use-team").await;
    app.store
        .upsert_team_member(team, viola.id, "viewer")
        .await
        .unwrap();
    app.store
        .upsert_team_member(team, mark.id, "member")
        .await
        .unwrap();

    let agent = bound_agent(&app, alice.id, None, None).await;
    app.store
        .set_resource_visibility(app.org_id, "agents", agent, "team", Some(team))
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{agent}/executions"),
            &alice_token,
            json!({ "task": "run" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let execution = body["id"].as_str().unwrap().to_string();

    // A team viewer can see the execution but not cancel it.
    let (status, _) = send(
        &app.router,
        auth_get(&format!("/api/v1/executions/{execution}"), &viola_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/executions/{execution}/cancel"),
            &viola_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/executions/{execution}/cancel"),
            &mark_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Team runs: viewer refused, member allowed.
    let team_definition = json!({ "steps": [{ "agentId": agent }] });
    let team_row = app
        .store
        .create_agent_team(app.org_id, "Use team", "", &team_definition, Some(alice.id))
        .await
        .unwrap();
    app.store
        .assign_resource_owner(app.org_id, "agent_teams", team_row.id, alice.id)
        .await
        .unwrap();
    app.store
        .set_resource_visibility(app.org_id, "agent_teams", team_row.id, "team", Some(team))
        .await
        .unwrap();
    let runs = format!("/api/v1/agent-teams/{}/runs", team_row.id);
    let (status, _) = send(
        &app.router,
        auth_post(&runs, &viola_token, json!({ "task": "go" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A caller-supplied repository outside the team's configured set is refused.
    let (status, body) = send(
        &app.router,
        auth_post(
            &runs,
            &mark_token,
            json!({ "task": "go", "payload": { "repository": "evil/repo" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["path"], "payload.repository");

    let (status, body) = send(
        &app.router,
        auth_post(&runs, &mark_token, json!({ "task": "go" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // A step agent bound to a connection the member cannot use blocks the run.
    let secret = webhook_connection(&app, "Alice private hook", alice.id).await;
    let bound = bound_agent(&app, alice.id, Some(secret), None).await;
    app.store
        .set_resource_visibility(app.org_id, "agents", bound, "team", Some(team))
        .await
        .unwrap();
    let team2 = app
        .store
        .create_agent_team(
            app.org_id,
            "Bound team",
            "",
            &json!({ "steps": [{ "agentId": bound }] }),
            Some(alice.id),
        )
        .await
        .unwrap();
    app.store
        .set_resource_visibility(app.org_id, "agent_teams", team2.id, "team", Some(team))
        .await
        .unwrap();
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agent-teams/{}/runs", team2.id),
            &mark_token,
            json!({ "task": "go" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn tool_policies_require_manage_on_the_target_agent() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = operator(&app, "pf-policy-alice@test.local").await;
    let carol = operator(&app, "pf-policy-carol@test.local").await;
    // Since #136 a token never exceeds its owner's current role and only owners hold
    // settings:write among the built-in roles (owners manage everything). A custom role
    // gives carol settings:write without privileged resource access, which is the case
    // this test is about.
    app.store
        .create_custom_role(
            app.org_id,
            "Policy author",
            &[
                "settings:write".into(),
                "settings:read".into(),
                "agents:read".into(),
            ],
        )
        .await
        .unwrap();
    app.store
        .set_user_role_name(app.org_id, carol.id, "Policy author")
        .await
        .unwrap();
    let carol_token = token_with_scopes(
        &app.store,
        app.org_id,
        carol.id,
        "carol",
        &["settings:write", "settings:read", "agents:read"],
    )
    .await;
    let alice_agent = bound_agent(&app, alice.id, None, None).await;
    make_private(&app.store, app.org_id, "agents", alice_agent, alice.id).await;
    let carol_agent = bound_agent(&app, carol.id, None, None).await;
    let policy = |agent: Option<Uuid>| {
        json!({
            "name": "allow bash",
            "agentId": agent,
            "action": "allow",
            "matcher": { "tool": "bash" }
        })
    };

    let (status, _) = send(
        &app.router,
        auth_post(
            "/api/v1/tool-policies",
            &carol_token,
            policy(Some(alice_agent)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/tool-policies",
            &carol_token,
            policy(Some(carol_agent)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let policy_id = body["id"].as_str().unwrap().to_string();

    // Moving the policy onto an agent she cannot manage is refused too.
    let (status, _) = send(
        &app.router,
        auth_json(
            "PATCH",
            &format!("/api/v1/tool-policies/{policy_id}"),
            &carol_token,
            policy(Some(alice_agent)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Org-wide policies keep requiring only settings:write.
    let (status, body) = send(
        &app.router,
        auth_post("/api/v1/tool-policies", &carol_token, policy(None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn export_checks_the_read_permission_of_the_resource_kind() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let dave = operator(&app, "pf-export-dave@test.local").await;
    let agents_only = token_with_scopes(
        &app.store,
        app.org_id,
        dave.id,
        "agents-only",
        &["agents:read"],
    )
    .await;
    let dave_agent = bound_agent(&app, dave.id, None, None).await;
    let provider = app
        .store
        .create_provider(
            app.org_id,
            "pf-export-provider",
            "openai_compatible",
            &json!({ "baseUrl": "http://127.0.0.1:9/v1" }),
            None,
            true,
        )
        .await
        .unwrap();
    share_org(&app, "providers", provider.id).await;

    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/export/providers/{}", provider.id),
            &agents_only,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(
        &app.router,
        auth_get(&format!("/api/v1/export/agents/{dave_agent}"), &agents_only),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let providers_reader = token_with_scopes(
        &app.store,
        app.org_id,
        dave.id,
        "providers",
        &["agents:read", "providers:read"],
    )
    .await;
    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/export/providers/{}", provider.id),
            &providers_reader,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn scheduled_trigger_fails_when_its_owner_lost_connection_access() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = operator(&app, "pf-trigger-alice@test.local").await;
    let bob = operator(&app, "pf-trigger-bob@test.local").await;
    // Bob owns the connection and shares it org-wide; alice's agent and trigger use it.
    let connection = webhook_connection(&app, "Bob trigger hook", bob.id).await;
    share_org(&app, "connections", connection).await;
    let agent = bound_agent(&app, alice.id, Some(connection), None).await;
    let trigger = app
        .store
        .create_trigger(
            app.org_id,
            agent,
            "schedule",
            None,
            json!({ "cron": "* * * * *" }),
            true,
        )
        .await
        .unwrap();
    app.store
        .assign_resource_owner(app.org_id, "triggers", trigger.id, alice.id)
        .await
        .unwrap();

    // While the owner can use the connection the trigger runs.
    let current = app.store.get_trigger(app.org_id, trigger.id).await.unwrap();
    orbit_api::background::fire_trigger(&app.state, current)
        .await
        .expect("first fire succeeds");
    let (executions, _, _) = app
        .store
        .list_executions(app.org_id, 50, None, None, Some(agent), None, None)
        .await
        .unwrap();
    assert_eq!(executions.len(), 1);
    app.store
        .request_execution_cancel(app.org_id, executions[0].id)
        .await
        .unwrap();

    // Bob withdraws the connection: the trigger owner has lost access at fire time.
    make_private(&app.store, app.org_id, "connections", connection, bob.id).await;
    let current = app.store.get_trigger(app.org_id, trigger.id).await.unwrap();
    let error = orbit_api::background::fire_trigger(&app.state, current)
        .await
        .expect_err("fire is refused");
    assert!(error.to_string().contains("refused"), "{error}");
    let (executions, _, _) = app
        .store
        .list_executions(app.org_id, 50, None, None, Some(agent), None, None)
        .await
        .unwrap();
    assert_eq!(executions.len(), 1, "no execution may be created");
    let (logs, _) = app
        .store
        .list_audit_logs(app.org_id, 10, None, Some("trigger.fire_denied"))
        .await
        .unwrap();
    assert_eq!(logs.len(), 1);
}

// ---------------------------------------------------------------------------
// #123: error envelope and validated settings (restored, #191)
// ---------------------------------------------------------------------------

fn put(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn assert_envelope(status: StatusCode, body: &Value, expected: StatusCode, code: &str) {
    assert_eq!(status, expected, "{body}");
    assert_eq!(body["error"]["code"], code, "{body}");
    assert!(body["error"]["message"].is_string(), "{body}");
    assert!(body["error"]["details"].is_array(), "{body}");
    assert!(
        !body["error"]["requestId"].as_str().unwrap_or("").is_empty(),
        "{body}"
    );
}

#[tokio::test]
async fn path_query_route_and_method_rejections_use_the_error_envelope() {
    let Some(app) = test_app(true).await else {
        return;
    };

    let (status, body) = send(&app.router, get("/api/v1/agents/not-a-uuid")).await;
    assert_envelope(status, &body, StatusCode::BAD_REQUEST, "bad_request");

    let (status, body) = send(&app.router, get("/api/v1/agents?limit=abc")).await;
    assert_envelope(status, &body, StatusCode::BAD_REQUEST, "bad_request");

    let (status, body) = send(&app.router, get("/api/v1/no/such/route")).await;
    assert_envelope(status, &body, StatusCode::NOT_FOUND, "not_found");

    let request = Request::builder()
        .method("DELETE")
        .uri("/api/v1/meta")
        .body(Body::empty())
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(response.headers().contains_key("allow"));
    assert!(response.headers().contains_key("x-request-id"));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_envelope(
        StatusCode::METHOD_NOT_ALLOWED,
        &body,
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
    );

    // Existing envelope errors and successful responses are unchanged.
    let missing = format!("/api/v1/agents/{}", Uuid::now_v7());
    let (status, body) = send(&app.router, get(&missing)).await;
    assert_envelope(status, &body, StatusCode::NOT_FOUND, "not_found");
    let (status, body) = send(&app.router, post("/api/v1/agents", json!({"bad": true}))).await;
    assert!(status.is_client_error(), "{body}");
    assert!(body["error"]["requestId"].is_string(), "{body}");

    let response = app
        .router
        .clone()
        .oneshot(get("/api/v1/agents"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn settings_updates_are_validated_enveloped_and_audited() {
    let Some(app) = test_app(true).await else {
        return;
    };

    // Malformed bodies use the envelope for all three handlers.
    for path in [
        "/api/v1/settings/oidc",
        "/api/v1/settings/access",
        "/api/v1/settings/harness",
    ] {
        let request = Request::builder()
            .method("PUT")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from("{not json"))
            .unwrap();
        let (status, body) = send(&app.router, request).await;
        assert_envelope(status, &body, StatusCode::BAD_REQUEST, "bad_request");
    }

    // Invalid OIDC settings are rejected with field-level details.
    let (status, body) = send(
        &app.router,
        put(
            "/api/v1/settings/oidc",
            json!({"settings": {
                "issuer": "not a url",
                "defaultRole": "viewer",
                "groupRoles": {"admins": "no-such-role"},
            }}),
        ),
    )
    .await;
    assert_envelope(
        status,
        &body,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_error",
    );
    let paths: Vec<&str> = body["error"]["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"settings.issuer"), "{paths:?}");
    assert!(paths.contains(&"settings.groupRoles.admins"), "{paths:?}");

    // Valid updates succeed and write audit rows.
    let (status, _) = send(
        &app.router,
        put(
            "/api/v1/settings/oidc",
            json!({"settings": {
                "issuer": "https://idp.example.com",
                "clientId": "orbit",
                "defaultRole": "viewer",
                "groupRoles": {"admins": "admin"},
            }}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app.router,
        put("/api/v1/settings/access", json!({"teamIsolation": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app.router,
        put(
            "/api/v1/settings/harness",
            json!({"defaultHarness": "mock"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    for (action, key, expected) in [
        (
            "settings.oidc.update",
            "after",
            json!("https://idp.example.com"),
        ),
        ("settings.access.update", "after", json!(false)),
        ("settings.harness.update", "after", json!("mock")),
    ] {
        let (rows, _) = app
            .store
            .list_audit_logs(app.org_id, 10, None, Some(action))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "{action}");
        let after = &rows[0].metadata[key];
        let found = match action {
            "settings.oidc.update" => &after["issuer"],
            "settings.access.update" => &after["teamIsolation"],
            _ => &after["defaultHarness"],
        };
        assert_eq!(found, &expected, "{action}");
        assert!(rows[0].request_id.is_some());
    }
}

// ---------------------------------------------------------------------------
// #122: list visibility applied in SQL (restored, #191)
// ---------------------------------------------------------------------------

async fn seed_execution(store: &Store, org_id: Uuid, agent_id: Uuid) -> Uuid {
    let agent = store.get_agent(org_id, agent_id).await.unwrap();
    let version = store.get_current_version(org_id, &agent).await.unwrap();
    store
        .create_execution(orbit_store::executions::NewExecution {
            org_id,
            agent_id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: orbit_domain::HarnessKind::Mock,
            trigger_source: orbit_domain::TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: json!({ "task": "seeded" }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: None,
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn execution_list_applies_visibility_before_paging() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "page-alice@test.local",
            "Alice",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(app.org_id, "page-bob@test.local", "Bob", UserRole::Operator)
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let agent_a = create_demo_agent(&app.store, app.org_id, alice.id).await;
    let agent_b = create_demo_agent(&app.store, app.org_id, bob.id).await;
    make_private(&app.store, app.org_id, "agents", agent_a, alice.id).await;
    make_private(&app.store, app.org_id, "agents", agent_b, bob.id).await;

    // 3 visible executions first, then 7 newer hidden ones.
    let mut visible_ids = Vec::new();
    for _ in 0..3 {
        visible_ids.push(seed_execution(&app.store, app.org_id, agent_a).await);
    }
    for _ in 0..7 {
        seed_execution(&app.store, app.org_id, agent_b).await;
    }

    let (status, page1) = send(
        &app.router,
        auth_get("/api/v1/executions?limit=2", &alice_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    assert!(page1["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["agentId"] == agent_a.to_string()));
    assert_eq!(page1["total"], 3);
    let cursor = page1["nextCursor"]
        .as_str()
        .expect("next cursor")
        .to_string();

    let (status, page2) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/executions?limit=2&cursor={cursor}"),
            &alice_token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert_eq!(page2["items"][0]["agentId"], agent_a.to_string());
    assert!(page2["nextCursor"].is_null(), "{page2}");
    assert_eq!(page2["total"], 3);

    let mut seen: Vec<String> = [&page1, &page2]
        .iter()
        .flat_map(|p| p["items"].as_array().unwrap().iter())
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    seen.sort();
    let mut expected: Vec<String> = visible_ids.iter().map(|id| id.to_string()).collect();
    expected.sort();
    assert_eq!(seen, expected);

    // The org owner (dev identity) sees everything: 10 executions, one full page.
    let (status, owner) = send(&app.router, get("/api/v1/executions?limit=50")).await;
    assert_eq!(status, StatusCode::OK, "{owner}");
    assert_eq!(owner["items"].as_array().unwrap().len(), 10);
    assert_eq!(owner["total"], 10);
}

#[tokio::test]
async fn approval_list_applies_visibility_in_sql() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "appr-alice@test.local",
            "Alice",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(app.org_id, "appr-bob@test.local", "Bob", UserRole::Operator)
        .await
        .unwrap();
    let agent_a = create_demo_agent(&app.store, app.org_id, alice.id).await;
    let agent_b = create_demo_agent(&app.store, app.org_id, bob.id).await;
    make_private(&app.store, app.org_id, "agents", agent_a, alice.id).await;
    make_private(&app.store, app.org_id, "agents", agent_b, bob.id).await;

    let mut visible = Vec::new();
    for _ in 0..2 {
        let execution = seed_execution(&app.store, app.org_id, agent_a).await;
        let approval = app
            .store
            .create_approval(app.org_id, execution, json!({ "action": "a" }), None)
            .await
            .unwrap();
        visible.push(approval.id);
    }
    for _ in 0..5 {
        let execution = seed_execution(&app.store, app.org_id, agent_b).await;
        app.store
            .create_approval(app.org_id, execution, json!({ "action": "b" }), None)
            .await
            .unwrap();
    }

    // Operators cannot decide approvals over HTTP, so exercise the store predicate the
    // handler passes for non-privileged callers: newer hidden rows must not consume the
    // page limit.
    let alice_access =
        orbit_store::access::ResourceAccess::new(alice.id, false, vec![]).with_team_isolation(true);
    let rows = app
        .store
        .list_approvals(app.org_id, Some("pending"), None, 2, Some(&alice_access))
        .await
        .unwrap();
    let mut got: Vec<Uuid> = rows.iter().map(|r| r.approval.id).collect();
    got.sort();
    visible.sort();
    assert_eq!(got, visible);
    assert!(rows.iter().all(|r| r.agent_id == agent_a));

    // Internal access and the privileged owner see all 7 rows.
    let all = app
        .store
        .list_approvals(app.org_id, Some("pending"), None, 50, None)
        .await
        .unwrap();
    assert_eq!(all.len(), 7);
    let (status, body) = send(&app.router, get("/api/v1/approvals?status=pending")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 7);
}

// ---------------------------------------------------------------------------
// #150: runtime continues after a failed execution (restored, #191)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_backs_off_then_continues_after_failed_execution() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let _serial = RUNTIME_TICK_LOCK.lock().await;
    let user = app
        .store
        .ensure_user(app.org_id, "api@test.local", "API Test", UserRole::Owner)
        .await
        .unwrap();
    let agent_id = create_demo_agent(&app.store, app.org_id, user.id).await;
    let runtime = app
        .store
        .create_agent_runtime(app.org_id, agent_id, "t", "mock", 60, None)
        .await
        .unwrap();
    let pool = raw_pool().await;

    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    let first = runtime_row(&app, agent_id).await.last_execution_id.unwrap();
    fail_execution(&pool, first).await;
    make_runtime_due(&pool, runtime.id).await;

    // Failure is accounted for and delays the next run; no new execution yet.
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    let row = runtime_row(&app, agent_id).await;
    assert_eq!(row.status, "running");
    assert_eq!(row.consecutive_failures, 1);
    assert_eq!(row.failed_execution_id, Some(first));
    assert_eq!(row.last_execution_id, Some(first));
    assert!(row.last_error.as_deref().unwrap().contains("boom"));
    assert!(row.next_run_at > chrono::Utc::now() + chrono::Duration::seconds(30));

    // Not yet due: still nothing new, and the failure is not double counted.
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    let row = runtime_row(&app, agent_id).await;
    assert_eq!(row.consecutive_failures, 1);
    assert_eq!(row.last_execution_id, Some(first));

    // Backoff elapsed: the next tick creates a new execution.
    make_runtime_due(&pool, runtime.id).await;
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    let row = runtime_row(&app, agent_id).await;
    let second = row.last_execution_id.unwrap();
    assert_ne!(second, first, "runtime must continue after a failure");
    assert_eq!(row.consecutive_failures, 1, "reset only after a success");
    assert!(row.last_error.is_none());

    // A successful execution resets the counter on the following launch.
    sqlx::query("UPDATE executions SET status = 'succeeded', finished_at = now() WHERE id = $1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    make_runtime_due(&pool, runtime.id).await;
    orbit_api::background::runtime_tick(&app.state)
        .await
        .unwrap();
    let row = runtime_row(&app, agent_id).await;
    assert_ne!(row.last_execution_id, Some(second));
    assert_eq!(row.consecutive_failures, 0);
    assert!(row.failed_execution_id.is_none());
}

// ---------------------------------------------------------------------------
// #142: provider credential exposure (restored, #191)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn changing_provider_base_url_requires_reentering_the_credential() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/providers",
            json!({
                "name": "Rebind Gateway",
                "kind": "openai_compatible",
                "config": { "baseUrl": "http://10.1.2.3:9999/v1", "model": "qwen" },
                "credential": "sk-rebind-secret"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/providers/{id}");
    let patch = |body: Value| {
        Request::builder()
            .method("PATCH")
            .uri(&path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };

    // New host without a new key: refused, nothing changes.
    let (status, body) = send(
        &app.router,
        patch(json!({ "config": { "baseUrl": "http://evil.example/v1", "model": "qwen" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("re-enter the API key"));
    let stored: Value = sqlx::query_scalar("SELECT config FROM providers WHERE id = $1::uuid")
        .bind(&id)
        .fetch_one(app.store.pool())
        .await
        .unwrap();
    assert_eq!(stored["baseUrl"], "http://10.1.2.3:9999/v1");

    // Same URL (trailing slash only) keeps the credential.
    let (status, body) = send(
        &app.router,
        patch(json!({ "config": { "baseUrl": "http://10.1.2.3:9999/v1", "model": "other" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // New host with a freshly supplied key is accepted.
    let (status, body) = send(
        &app.router,
        patch(json!({
            "config": { "baseUrl": "http://10.9.9.9:9999/v1", "model": "qwen" },
            "credential": "sk-new-key"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn output_can_be_created_off() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/outputs",
            json!({
                "name": format!("off-{}", Uuid::now_v7()),
                "kind": "webhook",
                "webhookUrl": "http://10.0.0.5/hook",
                "enabled": false
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["enabled"], false,
        "creating an output off keeps it off"
    );
    let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    assert!(!app.store.get_output(app.org_id, id).await.unwrap().enabled);
}

#[tokio::test]
async fn output_webhook_urls_are_validated_on_create_and_update() {
    let Some(app) = test_app(true).await else {
        return;
    };

    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/outputs",
            json!({
                "name": "Bad hook",
                "kind": "webhook",
                "webhookUrl": "http://169.254.169.254/latest/meta-data"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["details"]
        .as_array()
        .is_some_and(|details| details.iter().any(|detail| detail["path"] == "webhookUrl")));

    let secret_ref = format!("secret://output/{}", Uuid::now_v7());
    app.store
        .put_secret(
            Some(app.org_id),
            &secret_ref,
            &orbit_security::SecretValue::new("http://93.184.216.34/hook"),
            "test hook",
        )
        .await
        .unwrap();
    let output = app
        .store
        .create_output(
            app.org_id,
            "Patchable hook",
            "webhook",
            None,
            None,
            &["succeeded".to_string()],
            &secret_ref,
            None,
            None,
            None,
            true,
        )
        .await
        .unwrap();

    let (status, body) = send(
        &app.router,
        Request::builder()
            .method("PATCH")
            .uri(format!("/api/v1/outputs/{}", output.id))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Patchable hook",
                    "kind": "webhook",
                    "events": ["succeeded"],
                    "enabled": true,
                    "webhookUrl": "http://169.254.169.254/latest/meta-data"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["details"]
        .as_array()
        .is_some_and(|details| details.iter().any(|detail| detail["path"] == "webhookUrl")));
}

#[tokio::test]
async fn connection_base_urls_are_validated_on_create_and_update() {
    let Some(app) = test_app(true).await else {
        return;
    };

    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/connections",
            json!({
                "connectorSlug": "github",
                "name": "Bad base URL",
                "config": { "baseUrl": "http://169.254.169.254" }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["details"]
        .as_array()
        .is_some_and(|details| details.iter().any(|detail| detail["path"] == "baseUrl")));

    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/connections",
            json!({
                "connectorSlug": "github",
                "name": "Good base URL",
                "config": { "baseUrl": "https://93.184.216.34" }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap();

    let (status, body) = send(
        &app.router,
        Request::builder()
            .method("PATCH")
            .uri(format!("/api/v1/connections/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "config": { "baseUrl": "http://169.254.169.254" } }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["details"]
        .as_array()
        .is_some_and(|details| details.iter().any(|detail| detail["path"] == "baseUrl")));
}

/// #321: a stored credential must not follow an edited endpoint, as for providers.
#[tokio::test]
async fn connection_credential_does_not_follow_an_edited_endpoint() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/connections",
            json!({
                "connectorSlug": "github",
                "name": "Endpoint edit",
                "config": { "baseUrl": "https://93.184.216.34" },
                "credential": "dummy-token"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let path = format!("/api/v1/connections/{}", body["id"].as_str().unwrap());

    // A new host without a new credential is refused.
    let (status, body) = send(
        &app.router,
        patch_json(
            &path,
            json!({ "config": { "baseUrl": "https://93.184.216.35" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["details"]
        .as_array()
        .is_some_and(|details| details.iter().any(|detail| detail["path"] == "credential")));

    // The same endpoint (modulo a trailing slash) and other fields may change freely.
    let (status, body) = send(
        &app.router,
        patch_json(
            &path,
            json!({ "name": "Renamed", "config": { "baseUrl": "https://93.184.216.34/", "defaultRepository": "acme/x" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A new host together with its credential is accepted.
    let (status, body) = send(
        &app.router,
        patch_json(
            &path,
            json!({ "config": { "baseUrl": "https://93.184.216.35" }, "credential": "new-token" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// #321: the Kubernetes endpoint is checked like every other connection URL, and a test
/// sends the credential, so it needs write permission and use of the connection.
#[tokio::test]
async fn kubernetes_api_server_is_guarded_and_testing_needs_write() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/connections",
            json!({
                "connectorSlug": "kubernetes",
                "name": "Metadata cluster",
                "config": { "authMode": "service_account", "apiServer": "https://169.254.169.254" },
                "credential": "dummy-token"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["details"]
        .as_array()
        .is_some_and(|details| details.iter().any(|detail| detail["path"] == "apiServer")));

    // A loopback API server stored before this check is refused when the test connects,
    // and the response does not echo what the address returned.
    let kubernetes = app
        .store
        .list_connectors(app.org_id)
        .await
        .unwrap()
        .into_iter()
        .find(|connector| connector.slug == "kubernetes")
        .unwrap();
    let user = owner_user(&app).await;
    let reference = format!("connection/{}", Uuid::now_v7());
    app.store
        .put_secret(
            Some(app.org_id),
            &reference,
            &orbit_security::SecretValue::new("dummy-token"),
            "test",
        )
        .await
        .unwrap();
    let connection = app
        .store
        .create_connection(
            app.org_id,
            kubernetes.id,
            "Loopback cluster",
            &json!({ "authMode": "service_account", "apiServer": "https://127.0.0.1:6443" }),
            &[],
            Some(&reference),
            Some(user.id),
        )
        .await
        .unwrap();
    let test_path = format!("/api/v1/connections/{}/test", connection.id);
    let (status, body) = send(&app.router, post(&test_path, json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("request refused"),
        "{body}"
    );

    // A viewer can see the connection but cannot make ORBIT send its credential.
    let viewer = app
        .store
        .ensure_user(
            app.org_id,
            "conn-viewer@test.local",
            "Viewer",
            UserRole::Viewer,
        )
        .await
        .unwrap();
    let token = token_for(&app.store, app.org_id, viewer.id, "conn-viewer").await;
    let (status, _) = send(&app.router, auth_post(&test_path, &token, json!({}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn provider_probe_requires_manage_and_refuses_metadata_addresses() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let provider = app
        .store
        .create_provider(
            app.org_id,
            "Metadata probe",
            "openai_compatible",
            &json!({ "baseUrl": "http://169.254.169.254/v1", "model": "m" }),
            None,
            true,
        )
        .await
        .unwrap();
    // An operator can see org-visible providers but cannot probe (send credentials).
    let operator = app
        .store
        .ensure_user(
            app.org_id,
            "probe-op@test.local",
            "Operator",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let operator_token = token_for(&app.store, app.org_id, operator.id, "probe-op").await;
    let path = format!("/api/v1/providers/{}/test", provider.id);
    let (status, _) = send(&app.router, auth_post(&path, &operator_token, json!({}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The manager's probe is refused by the outbound guard with a generic category.
    let (status, body) = send(&app.router, post(&path, json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "endpoint refused (blocked)");
}

#[tokio::test]
async fn worker_context_injects_gh_token_from_agent_connection() {
    let Some(app) = test_app(false).await else {
        return;
    };
    let user = owner_user(&app).await;
    let github = app
        .store
        .get_connector_by_slug(app.org_id, "github")
        .await
        .unwrap()
        .expect("github connector");
    let reference = orbit_security::new_secret_ref("connection");
    app.store
        .put_secret(
            Some(app.org_id),
            &reference,
            &orbit_security::SecretValue::new("gh-test-token"),
            "test github connection",
        )
        .await
        .unwrap();
    let connection = app
        .store
        .create_connection(
            app.org_id,
            github.id,
            "Agent GitHub",
            &json!({ "defaultRepository": "opsifyhq/orbit" }),
            &["repo.checkout".to_string()],
            Some(&reference),
            Some(user.id),
        )
        .await
        .unwrap();

    let gitlab = app
        .store
        .get_connector_by_slug(app.org_id, "gitlab")
        .await
        .unwrap()
        .expect("gitlab connector");
    let gitlab_reference = orbit_security::new_secret_ref("connection");
    app.store
        .put_secret(
            Some(app.org_id),
            &gitlab_reference,
            &orbit_security::SecretValue::new("glab-test-token"),
            "test gitlab connection",
        )
        .await
        .unwrap();
    let gitlab_connection = app
        .store
        .create_connection(
            app.org_id,
            gitlab.id,
            "Agent GitLab",
            &json!({}),
            &["repo.checkout".to_string()],
            Some(&gitlab_reference),
            Some(user.id),
        )
        .await
        .unwrap();

    let mut definition: orbit_domain::AgentDefinition =
        serde_json::from_value(definition()).unwrap();
    definition.spec.connectors = vec![
        orbit_domain::ConnectorBinding {
            connector: "github".into(),
            connection_id: Some(connection.id),
            capabilities: vec!["repo.checkout".into()],
        },
        orbit_domain::ConnectorBinding {
            connector: "gitlab".into(),
            connection_id: Some(gitlab_connection.id),
            capabilities: vec!["repo.checkout".into()],
        },
    ];
    let (agent, version) = app
        .store
        .create_agent(app.org_id, Some(user.id), &definition)
        .await
        .unwrap();
    let profile = app
        .store
        .create_worker_profile(
            app.org_id,
            "gh worker",
            "mock",
            "local",
            "process",
            "orbit-worker",
            "0.1.0",
            "selected",
            &[agent.id],
            &[],
            None,
            None,
            None,
            &json!({}),
            &json!([]),
            &json!({}),
            false,
            false,
            true,
            true,
            &json!({}),
        )
        .await
        .unwrap();
    let issued = orbit_security::issue_worker_token();
    let token = app
        .store
        .create_worker_token(
            app.org_id,
            Some(profile.id),
            "gh-worker",
            &issued.prefix,
            &issued.hash,
            &["mock".to_string()],
        )
        .await
        .unwrap();
    let execution = app
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: app.org_id,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: orbit_domain::HarnessKind::Mock,
            trigger_source: orbit_domain::TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: json!({ "task": "pr" }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user.id),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    app.store
        .claim_execution(execution.id, "gh-worker")
        .await
        .unwrap()
        .expect("queued");
    app.store
        .bind_execution_worker_token(execution.id, token.id)
        .await
        .unwrap();

    let (status, body) = send(
        &app.router,
        worker_request(
            "GET",
            &format!("/api/v1/worker/executions/{}/context", execution.id),
            &issued.token,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let env = body["env"].as_array().unwrap();
    assert!(env
        .iter()
        .any(|item| item["envVar"] == "GH_TOKEN" && item["value"] == "gh-test-token"));
    assert!(env
        .iter()
        .any(|item| item["envVar"] == "GITLAB_TOKEN" && item["value"] == "glab-test-token"));
}

#[tokio::test]
async fn agent_summaries_list_their_skills() {
    let Some(app) = test_app(true).await else {
        return;
    };
    for slug in ["release-notes", "code-review"] {
        let (status, body) = send(
            &app.router,
            post(
                "/api/v1/skills",
                json!({ "name": slug, "slug": slug, "description": "", "content": "# Skill", "files": {} }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let mut with_skills = definition();
    with_skills["metadata"]["name"] = json!("Skilled Agent");
    with_skills["spec"]["skills"] = json!(["release-notes", "code-review"]);
    let (status, body) = send(&app.router, post("/api/v1/agents", with_skills)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();

    let (status, body) = send(&app.router, get("/api/v1/agents?limit=50")).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    let skilled = items
        .iter()
        .find(|item| item["id"] == id)
        .expect("new agent listed");
    assert_eq!(skilled["skills"], json!(["release-notes", "code-review"]));
    // Agents without skills report an empty list rather than omitting the field.
    assert!(items
        .iter()
        .filter(|item| item["id"] != id)
        .all(|item| item["skills"] == json!([])));
}

#[tokio::test]
async fn agent_update_can_return_to_an_earlier_configuration() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let mut original = definition();
    original["metadata"]["name"] = json!("Revert API Agent");
    let (status, body) = send(&app.router, post("/api/v1/agents", original.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let patch = |definition: Value| {
        Request::builder()
            .method("PATCH")
            .uri(format!("/api/v1/agents/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "definition": definition }).to_string()))
            .unwrap()
    };

    let mut changed = original.clone();
    changed["spec"]["autonomy"]["level"] = json!(3);
    let (status, body) = send(&app.router, patch(changed)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["currentVersion"], 2);

    // Undoing the change is saved as version 3 instead of being refused.
    let (status, body) = send(&app.router, patch(original.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["currentVersion"], 3);
    assert_eq!(body["spec"]["autonomy"]["level"], 2);

    // Saving the current configuration again stays a metadata-only update.
    let (status, body) = send(&app.router, patch(original)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["currentVersion"], 3);
}

#[tokio::test]
async fn execution_event_list_accepts_kind_tool_and_q_filters() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, execution) = send(
        &app.router,
        post(
            &format!("/api/v1/agents/{}/executions", app.agent_id),
            json!({ "task": "event filters" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{execution}");
    let execution_id = execution["id"].as_str().unwrap();
    for (kind, message, data) in [
        (
            "tool_call",
            "github.issues_list executed",
            json!({ "tool": "github_issues_list", "connector": "github" }),
        ),
        ("output", "the answer", json!({})),
        (
            "error",
            "issues.comment failed",
            json!({ "capability": "issues.comment" }),
        ),
    ] {
        app.store
            .append_execution_event(
                app.org_id,
                Uuid::parse_str(execution_id).unwrap(),
                kind,
                orbit_domain::LogLevel::Info,
                message,
                &data,
            )
            .await
            .unwrap();
    }

    assert_eq!(execution_events(&app, execution_id, "").await.len(), 4); // includes the queued status event
    assert_eq!(
        execution_events(&app, execution_id, "&kind=tool_call,error")
            .await
            .len(),
        2
    );
    assert_eq!(
        execution_events(&app, execution_id, "&tool=GITHUB")
            .await
            .len(),
        1
    );
    assert_eq!(
        execution_events(&app, execution_id, "&q=answer")
            .await
            .len(),
        1
    );
    assert_eq!(
        execution_events(&app, execution_id, "&kind=error&tool=issues.comment")
            .await
            .len(),
        1
    );
    assert_eq!(
        execution_events(&app, execution_id, "&afterSeq=3")
            .await
            .len(),
        1
    );

    let (status, body) = send(
        &app.router,
        get(&format!(
            "/api/v1/executions/{execution_id}/events?kind=bogus"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
}

async fn execution_events(app: &TestApp, execution_id: &str, query: &str) -> Vec<Value> {
    let (status, body) = send(
        &app.router,
        get(&format!(
            "/api/v1/executions/{execution_id}/events?limit=100{query}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

// ---------------------------------------------------------------------------
// #249: bulk archive/delete for agents and bulk delete for skills
// ---------------------------------------------------------------------------

async fn bulk_team_agent(app: &TestApp, team_id: Uuid, owner_id: Uuid) -> Uuid {
    let agent = create_demo_agent(&app.store, app.org_id, owner_id).await;
    app.store
        .set_resource_visibility(app.org_id, "agents", agent, "team", Some(team_id))
        .await
        .unwrap();
    agent
}

async fn bulk_team_skill(app: &TestApp, slug: &str, team_id: Uuid) -> Uuid {
    let skill = app
        .store
        .create_skill(app.org_id, slug, slug, "", "content", &json!({}))
        .await
        .unwrap();
    app.store
        .set_resource_visibility(app.org_id, "skills", skill.id, "team", Some(team_id))
        .await
        .unwrap();
    skill.id
}

fn bulk_failed_ids(body: &Value) -> Vec<Uuid> {
    body["failed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

#[tokio::test]
async fn bulk_agents_archive_delete_manage_access_and_partial_failures() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let owner = app
        .store
        .ensure_user(
            app.org_id,
            "bulk-owner@test.local",
            "Owner",
            UserRole::Owner,
        )
        .await
        .unwrap();
    let admin = app
        .store
        .ensure_user(
            app.org_id,
            "bulk-admin@test.local",
            "Admin",
            UserRole::Admin,
        )
        .await
        .unwrap();
    let lead = app
        .store
        .ensure_user(
            app.org_id,
            "bulk-lead@test.local",
            "Lead",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let member = app
        .store
        .ensure_user(
            app.org_id,
            "bulk-member@test.local",
            "Member",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let outsider = app
        .store
        .ensure_user(
            app.org_id,
            "bulk-outsider@test.local",
            "Outsider",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let team = app
        .store
        .create_access_team(app.org_id, "Bulk ops", "bulk-ops", None)
        .await
        .unwrap();
    app.store
        .upsert_team_member(team.id, lead.id, "lead")
        .await
        .unwrap();
    app.store
        .upsert_team_member(team.id, member.id, "member")
        .await
        .unwrap();

    let tokens = (
        token_for(&app.store, app.org_id, owner.id, "bulk-owner").await,
        token_for(&app.store, app.org_id, admin.id, "bulk-admin").await,
        token_for(&app.store, app.org_id, lead.id, "bulk-lead").await,
        token_for(&app.store, app.org_id, member.id, "bulk-member").await,
        token_for(&app.store, app.org_id, outsider.id, "bulk-outsider").await,
    );
    let (owner_token, admin_token, lead_token, member_token, outsider_token) = tokens;

    let a1 = bulk_team_agent(&app, team.id, owner.id).await;
    let a2 = bulk_team_agent(&app, team.id, owner.id).await;
    let blocked = bulk_team_agent(&app, team.id, owner.id).await;
    let archived = bulk_team_agent(&app, team.id, owner.id).await;
    app.store
        .set_agent_status(app.org_id, archived, orbit_domain::AgentStatus::Archived)
        .await
        .unwrap();
    let private = create_demo_agent(&app.store, app.org_id, outsider.id).await;
    make_private(&app.store, app.org_id, "agents", private, outsider.id).await;

    // An agent used by a team step is exactly what single DELETE refuses (FK RESTRICT).
    let workflow = app
        .store
        .create_agent_team(
            app.org_id,
            "Bulk workflow",
            "",
            &json!({ "steps": [] }),
            Some(owner.id),
        )
        .await
        .unwrap();
    let run = app
        .store
        .create_team_run(app.org_id, workflow.id, &json!({}), Some(owner.id))
        .await
        .unwrap();
    let agent_row = app.store.get_agent(app.org_id, blocked).await.unwrap();
    let version = app
        .store
        .get_current_version(app.org_id, &agent_row)
        .await
        .unwrap();
    app.store
        .create_team_step(
            run.id,
            app.org_id,
            0,
            "blocked",
            blocked,
            version.id,
            &[],
            &json!({}),
            None,
        )
        .await
        .unwrap();

    // Preview reports mixed states and changes nothing.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/agents/bulk/archive",
            &lead_token,
            json!({ "ids": [a1, a2, archived, private, a1], "dryRun": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["updated"].as_array().unwrap().len(), 2);
    assert_eq!(body["unchanged"].as_array().unwrap().len(), 1);
    assert_eq!(bulk_failed_ids(&body), vec![private]);
    assert_eq!(body["failed"][0]["code"], "not_found");
    assert_eq!(
        app.store.get_agent(app.org_id, a1).await.unwrap().status,
        "active"
    );

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/agents/bulk/delete",
            &lead_token,
            json!({ "ids": [blocked, a1, archived], "dryRun": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"].as_array().unwrap().len(), 2);
    assert_eq!(bulk_failed_ids(&body), vec![blocked]);
    assert_eq!(body["failed"][0]["code"], "conflict");
    assert!(app.store.get_agent(app.org_id, a1).await.is_ok());

    // Actual archive: lead can manage, outsider's private agent cannot be seen.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/agents/bulk/archive",
            &lead_token,
            json!({ "ids": [a1, a2, archived, private] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["updated"].as_array().unwrap().len(), 2);
    assert_eq!(body["unchanged"].as_array().unwrap().len(), 1);
    assert_eq!(bulk_failed_ids(&body), vec![private]);
    for id in [a1, a2, archived] {
        let agent = app.store.get_agent(app.org_id, id).await.unwrap();
        assert_eq!(agent.status, "archived");
        assert!(agent.archived_at.is_some());
    }
    let (rows, _) = app
        .store
        .list_audit_logs(app.org_id, 10, None, Some("agent.bulk_archived"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].metadata["updated"], 2);
    assert_eq!(rows[0].metadata["unchanged"], 1);
    assert_eq!(rows[0].metadata["failed"], 1);

    // Archived agents disappear from the normal list but keep versions and can return.
    let (status, body) = send(
        &app.router,
        auth_get("/api/v1/agents?status=active", &lead_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let visible = body["items"].as_array().unwrap();
    assert!(visible
        .iter()
        .all(|item| item["id"].as_str().unwrap().parse::<Uuid>().unwrap() != a1));
    assert!(!app
        .store
        .list_agent_versions(app.org_id, a1)
        .await
        .unwrap()
        .is_empty());
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{a1}/activate"),
            &lead_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "active");
    let restored = app.store.get_agent(app.org_id, a1).await.unwrap();
    assert_eq!(restored.status, "active");
    assert!(restored.archived_at.is_none());

    // Member cannot manage, outsider cannot view, admin can.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/agents/bulk/archive",
            &member_token,
            json!({ "ids": [a1, a2] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(bulk_failed_ids(&body), vec![a1, a2]);
    assert!(body["failed"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["code"] == "forbidden"));

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/agents/bulk/archive",
            &outsider_token,
            json!({ "ids": [a1, a2] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(bulk_failed_ids(&body), vec![a1, a2]);
    assert!(body["failed"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["code"] == "not_found"));

    // Bulk delete uses the same FK rule as single delete.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/agents/bulk/delete",
            &admin_token,
            json!({ "ids": [a1, blocked, archived] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"].as_array().unwrap().len(), 2);
    assert_eq!(bulk_failed_ids(&body), vec![blocked]);
    assert_eq!(body["failed"][0]["code"], "conflict");
    assert!(app.store.get_agent(app.org_id, a1).await.is_err());
    assert!(app.store.get_agent(app.org_id, archived).await.is_err());
    assert!(app.store.get_agent(app.org_id, blocked).await.is_ok());
    let (status, single) = send(
        &app.router,
        auth_delete(&format!("/api/v1/agents/{blocked}?hard=true"), &owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{single}");
    let (rows, _) = app
        .store
        .list_audit_logs(app.org_id, 10, None, Some("agent.bulk_deleted"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].metadata["deleted"], 2);
    assert_eq!(rows[0].metadata["failed"], 1);
}

#[tokio::test]
async fn bulk_skills_are_delete_only_with_manage_access_and_id_bounds() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let owner = app
        .store
        .ensure_user(
            app.org_id,
            "skill-owner@test.local",
            "Owner",
            UserRole::Owner,
        )
        .await
        .unwrap();
    let admin = app
        .store
        .ensure_user(
            app.org_id,
            "skill-admin@test.local",
            "Admin",
            UserRole::Admin,
        )
        .await
        .unwrap();
    let lead = app
        .store
        .ensure_user(
            app.org_id,
            "skill-lead@test.local",
            "Lead",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let member = app
        .store
        .ensure_user(
            app.org_id,
            "skill-member@test.local",
            "Member",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let outsider = app
        .store
        .ensure_user(
            app.org_id,
            "skill-outsider@test.local",
            "Outsider",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let team = app
        .store
        .create_access_team(app.org_id, "Skill ops", "skill-ops", None)
        .await
        .unwrap();
    app.store
        .upsert_team_member(team.id, lead.id, "lead")
        .await
        .unwrap();
    app.store
        .upsert_team_member(team.id, member.id, "member")
        .await
        .unwrap();

    let owner_token = token_for(&app.store, app.org_id, owner.id, "skill-owner").await;
    let admin_token = token_for(&app.store, app.org_id, admin.id, "skill-admin").await;
    let lead_token = token_for(&app.store, app.org_id, lead.id, "skill-lead").await;
    let member_token = token_for(&app.store, app.org_id, member.id, "skill-member").await;
    let outsider_token = token_for(&app.store, app.org_id, outsider.id, "skill-outsider").await;

    let s1 = bulk_team_skill(&app, "bulk-s1", team.id).await;
    let s2 = bulk_team_skill(&app, "bulk-s2", team.id).await;
    let s3 = bulk_team_skill(&app, "bulk-s3", team.id).await;
    let s4 = bulk_team_skill(&app, "bulk-s4", team.id).await;
    let s5 = bulk_team_skill(&app, "bulk-s5", team.id).await;

    // Preview is read-only; skills have no archive state at all.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/skills/bulk/delete",
            &lead_token,
            json!({ "ids": [s1, s2], "dryRun": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"].as_array().unwrap().len(), 2);
    assert!(app.store.get_skill(app.org_id, s1).await.is_ok());
    let (status, _) = send(
        &app.router,
        auth_post(
            "/api/v1/skills/bulk/archive",
            &lead_token,
            json!({ "ids": [s1] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Duplicate ids are removed; the result contains each id once.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/skills/bulk/delete",
            &lead_token,
            json!({ "ids": [s1, s1, s2] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"].as_array().unwrap().len(), 2);
    assert!(app.store.get_skill(app.org_id, s1).await.is_err());
    let (rows, _) = app
        .store
        .list_audit_logs(app.org_id, 10, None, Some("skill.bulk_deleted"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].metadata["deleted"], 2);
    assert_eq!(rows[0].metadata["failed"], 0);

    // Member is forbidden, non-member gets not_found, admin may delete.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/skills/bulk/delete",
            &member_token,
            json!({ "ids": [s3] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["failed"][0]["code"], "forbidden");
    assert!(app.store.get_skill(app.org_id, s3).await.is_ok());

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/skills/bulk/delete",
            &outsider_token,
            json!({ "ids": [s3] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["failed"][0]["code"], "not_found");

    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/skills/bulk/delete",
            &admin_token,
            json!({ "ids": [s3] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"].as_array().unwrap().len(), 1);
    assert!(app.store.get_skill(app.org_id, s3).await.is_err());

    // Bounds apply after dedupe: 500 unique ids are allowed, 501 are rejected.
    let ids: Vec<Uuid> = (0..501).map(|_| Uuid::now_v7()).collect();
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/skills/bulk/delete",
            &owner_token,
            json!({ "ids": ids }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/skills/bulk/delete",
            &owner_token,
            json!({ "ids": [s4, s4, s5] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"].as_array().unwrap().len(), 2);
}

/// #207: the Connections "Test" button called a handler that was never routed.
#[tokio::test]
async fn connection_test_endpoint_is_routed_and_honest_about_unsupported_connectors() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let create = |slug: &'static str, extra: Value| {
        let router = app.router.clone();
        async move {
            let mut body = json!({ "connectorSlug": slug, "name": format!("{slug} test") });
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let (status, body) = send(&router, post("/api/v1/connections", body)).await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            body
        }
    };
    let status_of = |id: String| {
        let router = app.router.clone();
        async move {
            let (_, list) = send(&router, get("/api/v1/connections")).await;
            list.as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id.as_str())
                .map(|row| row["status"].clone())
                .unwrap()
        }
    };

    // A connector with a probe: the result is recorded on the connection.
    let mock = create("mock", json!({})).await;
    let mock_id = mock["id"].as_str().unwrap().to_string();
    let (status, body) = send(
        &app.router,
        post(&format!("/api/v1/connections/{mock_id}/test"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["supported"], true);
    assert_eq!(body["ok"], true);
    // `active` is the only status workers accept; the check time proves it ran.
    assert_eq!(status_of(mock_id.clone()).await, "active");
    let (_, list) = send(&app.router, get("/api/v1/connections")).await;
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == mock_id.as_str())
        .unwrap();
    assert!(row["lastCheckedAt"].is_string(), "{row}");

    // A failed check is recoverable: testing again restores `active`.
    sqlx::query("UPDATE connections SET status = 'error' WHERE id = $1")
        .bind(Uuid::parse_str(&mock_id).unwrap())
        .execute(&raw_pool().await)
        .await
        .unwrap();
    let (_, body) = send(
        &app.router,
        post(&format!("/api/v1/connections/{mock_id}/test"), json!({})),
    )
    .await;
    assert_eq!(body["ok"], true);
    assert_eq!(status_of(mock_id.clone()).await, "active");

    // A probe never revives a revoked connection.
    sqlx::query("UPDATE connections SET status = 'revoked' WHERE id = $1")
        .bind(Uuid::parse_str(&mock_id).unwrap())
        .execute(&raw_pool().await)
        .await
        .unwrap();
    send(
        &app.router,
        post(&format!("/api/v1/connections/{mock_id}/test"), json!({})),
    )
    .await;
    assert_eq!(status_of(mock_id).await, "revoked");

    // A connector without one: say so, contact nothing, and do not mark it failed.
    let telegram = create("telegram", json!({ "credential": "123456:bot-secret" })).await;
    let telegram_id = telegram["id"].as_str().unwrap().to_string();
    let before = status_of(telegram_id.clone()).await;
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/connections/{telegram_id}/test"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["supported"], false);
    assert_eq!(body["ok"], false);
    assert!(body["message"].as_str().unwrap().contains("Telegram"));
    assert!(!body.to_string().contains("bot-secret"));
    assert_eq!(status_of(telegram_id).await, before);

    // Unknown connection → enveloped 404.
    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/connections/{}/test", Uuid::now_v7()),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn connection_test_route_returns_connection_test_response() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let user = owner_user(&app).await;
    let github = app
        .store
        .get_connector_by_slug(app.org_id, "github")
        .await
        .unwrap()
        .expect("github connector");
    let connection = app
        .store
        .create_connection(
            app.org_id,
            github.id,
            "Missing base URL",
            &json!({ "baseUrl": "" }),
            &[],
            None,
            Some(user.id),
        )
        .await
        .unwrap();

    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/connections/{}/test", connection.id),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["supported"], true);
    assert_eq!(body["ok"], false);
    assert!(body["status"].is_null(), "{body}");
    assert!(body["latencyMs"].as_u64().is_some(), "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("missing baseUrl"),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// #216 memory retention and #215 launch archive guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn worker_memory_honours_the_agents_retention_days() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let owner = app.store.list_users(app.org_id).await.unwrap()[0].id;
    let mut definition = definition();
    definition["metadata"]["name"] = json!("Retention Agent");
    definition["spec"]["memory"] =
        json!({ "enabled": true, "strategy": "short_term", "retentionDays": 2 });
    let definition: orbit_domain::AgentDefinition = serde_json::from_value(definition).unwrap();
    let (agent, _) = app
        .store
        .create_agent(app.org_id, Some(owner), &definition)
        .await
        .unwrap();
    app.store
        .set_agent_status(app.org_id, agent.id, orbit_domain::AgentStatus::Active)
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{}/executions", agent.id),
            "test-token",
            json!({ "task": "retention" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let token = worker_token(&app.store, app.org_id, "retention-worker").await;
    // Other queued executions in this org (the demo agent's) may be claimed first.
    let execution = loop {
        let claimed = claim_as(&app, &token, "retention-worker")
            .await
            .expect("execution to claim");
        if app
            .store
            .get_execution(app.org_id, claimed)
            .await
            .unwrap()
            .agent_id
            == agent.id
        {
            break claimed;
        }
    };

    // A stored entry expires after the agent's retention window, whatever the worker sends.
    let (status, _) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/memory/store",
            &token,
            Some(json!({
                "executionId": execution,
                "kind": "short_term",
                "content": "fresh",
                "retentionDays": 3650
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let entries = app
        .store
        .list_memory(app.org_id, agent.id, None, 20)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    let expires = entries[0].expires_at.expect("retention sets expires_at");
    let days = (expires - chrono::Utc::now()).num_hours() as f64 / 24.0;
    assert!((1.9..=2.1).contains(&days), "expires in {days} days");

    // Entries older than the window (legacy rows without expires_at) are not injected.
    let stale = app
        .store
        .store_memory(
            app.org_id,
            agent.id,
            "short_term",
            "stale",
            &json!({}),
            None,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE agent_memory SET created_at = now() - interval '5 days' WHERE id = $1")
        .bind(stale.id)
        .execute(app.store.pool())
        .await
        .unwrap();
    let (status, listed) = send(
        &app.router,
        worker_request(
            "POST",
            "/api/v1/worker/memory/list",
            &token,
            Some(json!({ "executionId": execution })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let contents: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["content"].as_str().unwrap())
        .collect();
    assert_eq!(contents, vec!["fresh"]);
}

#[tokio::test]
async fn archiving_a_live_worker_launch_is_a_conflict() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let profile = app
        .store
        .create_worker_profile(
            app.org_id,
            "Archive API",
            "pi",
            "docker",
            "process",
            "orbit-worker-pi",
            "0.3.0",
            "all",
            &[],
            &[],
            None,
            None,
            None,
            &json!({}),
            &json!([]),
            &json!({}),
            false,
            false,
            false,
            false,
            &json!({}),
        )
        .await
        .unwrap();
    let launch = app
        .store
        .create_worker_launch(
            app.org_id,
            profile.id,
            &profile.name,
            "docker",
            "img:1",
            None,
            None,
            false,
        )
        .await
        .unwrap();
    let path = format!("/api/v1/worker-launches/{}/archive", launch.id);
    let (status, body) = send(&app.router, auth_post(&path, "test-token", json!({}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "conflict");

    app.store
        .update_worker_launch(
            app.org_id, launch.id, "stopped", None, None, None, false, true,
        )
        .await
        .unwrap();
    let (status, body) = send(&app.router, auth_post(&path, "test-token", json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["archivedAt"].is_string());
}

/// Lists are filtered where the data lives: the web app used to fetch a capped page and
/// filter it in the browser, so anything beyond the cap silently disappeared.
#[tokio::test]
async fn execution_team_run_and_approval_lists_filter_server_side() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let pool = raw_pool().await;

    // Executions: one status, or several separated by commas.
    // The demo agent runs one execution at a time: finish each before starting the next.
    let mut finished = Vec::new();
    for status in ["failed", "timed_out"] {
        let id = new_execution(&app, app.agent_id).await;
        sqlx::query("UPDATE executions SET status = $2, finished_at = now() WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
        finished.push(id);
    }
    let (failed, timed_out) = (finished[0], finished[1]);
    let queued = new_execution(&app, app.agent_id).await;
    let ids = |body: &Value| -> Vec<Uuid> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().parse().unwrap())
            .collect()
    };
    let (status, body) = send(
        &app.router,
        get("/api/v1/executions?status=failed,timed_out"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = ids(&body);
    assert!(listed.contains(&failed) && listed.contains(&timed_out));
    assert!(!listed.contains(&queued));
    assert_eq!(body["total"], 2);

    let (_, body) = send(&app.router, get("/api/v1/executions?status=queued")).await;
    assert_eq!(ids(&body), vec![queued]);

    let (status, body) = send(&app.router, get("/api/v1/executions?status=failed,nope")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("nope"));

    // Approvals: only those of one execution.
    for execution in [failed, queued] {
        app.store
            .create_approval(
                app.org_id,
                execution,
                json!({ "capability": "issues.create" }),
                None,
            )
            .await
            .unwrap();
    }
    let (status, body) = send(
        &app.router,
        get(&format!(
            "/api/v1/approvals?status=pending&executionId={queued}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["executionId"], queued.to_string());

    // Team runs: only those of one team, newest first, with a limit.
    let user = owner_user(&app).await;
    let mut teams = Vec::new();
    for name in ["Filter Team A", "Filter Team B"] {
        let team = app
            .store
            .create_agent_team(app.org_id, name, "", &json!({}), Some(user.id))
            .await
            .unwrap();
        for _ in 0..2 {
            sqlx::query(
                "INSERT INTO team_runs (id, org_id, team_id, status, input) VALUES ($1, $2, $3, 'succeeded', '{}'::jsonb)",
            )
            .bind(Uuid::now_v7())
            .bind(app.org_id)
            .bind(team.id)
            .execute(&pool)
            .await
            .unwrap();
        }
        teams.push(team.id);
    }
    let (status, body) = send(
        &app.router,
        get(&format!("/api/v1/team-runs?teamId={}", teams[0])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let runs = body.as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|run| run["teamId"] == teams[0].to_string()));

    let (_, body) = send(&app.router, get("/api/v1/team-runs?limit=3")).await;
    assert_eq!(body.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn agent_save_rejects_unknown_capability_ids() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let connector = app
        .store
        .get_connector_by_slug(app.org_id, "github")
        .await
        .unwrap()
        .expect("connector is seeded");
    let connection = app
        .store
        .create_connection(
            app.org_id,
            connector.id,
            "cap-check",
            &json!({}),
            &[],
            None,
            None,
        )
        .await
        .unwrap();
    let mut body = definition();
    body["metadata"]["name"] = json!("Capability check agent");
    body["spec"]["connectors"] = json!([{
        "connector": "github",
        "connectionId": connection.id,
        "capabilities": ["issues.list", "issues.explode"]
    }]);
    let (status, response) = send(&app.router, post("/api/v1/agents", body.clone())).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    let fields: Vec<&str> = response["error"]["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    assert_eq!(fields, vec!["spec.connectors[0].capabilities[1]"]);

    body["spec"]["connectors"][0]["capabilities"] = json!(["issues.list"]);
    let (status, response) = send(&app.router, post("/api/v1/agents", body)).await;
    assert_eq!(status, StatusCode::CREATED, "{response}");
}

#[tokio::test]
async fn private_skills_cannot_be_attached_by_other_users() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let alice = app
        .store
        .ensure_user(
            app.org_id,
            "skill-use-alice@test.local",
            "Alice",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let bob = app
        .store
        .ensure_user(
            app.org_id,
            "skill-use-bob@test.local",
            "Bob",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let alice_token = token_for(&app.store, app.org_id, alice.id, "alice").await;
    let bob_token = token_for(&app.store, app.org_id, bob.id, "bob").await;
    let (status, skill) = send(
        &app.router,
        auth_post(
            "/api/v1/skills",
            &alice_token,
            json!({ "name": "Secret", "slug": "alice-secret-skill", "content": "hush", "files": {} }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{skill}");
    app.store
        .set_resource_visibility(
            app.org_id,
            "skills",
            Uuid::parse_str(skill["id"].as_str().unwrap()).unwrap(),
            "private",
            None,
        )
        .await
        .unwrap();
    let bob_agent = create_demo_agent(&app.store, app.org_id, bob.id).await;
    app.store
        .assign_resource_owner(app.org_id, "agents", bob_agent, bob.id)
        .await
        .unwrap();

    // Same error for a private slug and a slug that does not exist.
    let mut messages = Vec::new();
    for slug in ["alice-secret-skill", "no-such-skill"] {
        let (status, body) = send(
            &app.router,
            auth_post(
                &format!("/api/v1/agents/{bob_agent}/executions"),
                &bob_token,
                json!({ "task": "t", "skills": [slug] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        messages.push(
            body["error"]["details"][0]["message"]
                .as_str()
                .unwrap()
                .replace(slug, "<slug>"),
        );

        let mut agent_definition = definition();
        agent_definition["metadata"]["name"] = json!(format!("skill agent {slug}"));
        agent_definition["spec"]["skills"] = json!([slug]);
        let (status, body) = send(
            &app.router,
            auth_post("/api/v1/agents", &bob_token, agent_definition),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    assert_eq!(messages[0], messages[1]);

    // The owner may use it.
    let alice_agent = create_demo_agent(&app.store, app.org_id, alice.id).await;
    app.store
        .assign_resource_owner(app.org_id, "agents", alice_agent, alice.id)
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/agents/{alice_agent}/executions"),
            &alice_token,
            json!({ "task": "t", "skills": ["alice-secret-skill"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn team_step_executions_cannot_be_rerun_alone() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let pool = raw_pool().await;
    let agent = app.store.get_agent(app.org_id, app.agent_id).await.unwrap();
    let version = app
        .store
        .get_current_version(app.org_id, &agent)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for (input, key) in [
        (
            json!({ "task": "t", "team": { "connectionIds": [Uuid::now_v7()], "envMappingIds": [] } }),
            "team-step",
        ),
        (json!({ "task": "t" }), "plain"),
    ] {
        let key = format!("{key}-{}", Uuid::now_v7());
        let execution = app
            .store
            .create_execution(orbit_store::executions::NewExecution {
                org_id: app.org_id,
                agent_id: agent.id,
                agent_version_id: version.id,
                agent_name: &agent.name,
                harness_kind: orbit_domain::HarnessKind::Mock,
                trigger_source: orbit_domain::TriggerSource::Manual,
                placement: "auto",
                trigger_id: None,
                input,
                session_ref: None,
                skills: &[],
                timeout_seconds: 30,
                requested_by: None,
                idempotency_key: Some(&key),
                team_run_id: None,
                team_step_index: None,
            })
            .await
            .unwrap();
        fail_execution(&pool, execution.id).await;
        ids.push(execution.id);
    }

    let (status, body) = send(
        &app.router,
        post(&format!("/api/v1/executions/{}/rerun", ids[0]), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("re-run the team"));

    let (status, body) = send(
        &app.router,
        post(&format!("/api/v1/executions/{}/rerun", ids[1]), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn connection_url_fields_report_config_paths_and_cover_clone_base_url() {
    let Some(app) = test_app(true).await else {
        return;
    };
    for (field, url) in [
        ("baseUrl", "http://169.254.169.254/latest"),
        ("cloneBaseUrl", "http://127.0.0.1:8080"),
        ("url", "ftp://example.com"),
    ] {
        let (status, body) = send(
            &app.router,
            post(
                "/api/v1/connections",
                json!({ "connectorSlug": "gitlab", "name": "ssrf", "config": { field: url } }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{url}: {body}");
        assert_eq!(body["error"]["details"][0]["path"], field);
    }
    // Private ranges stay allowed by default (self-hosted GitLab / GitHub Enterprise).
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/connections",
            json!({ "connectorSlug": "gitlab", "name": "onprem-c1", "config": {
                "baseUrl": "http://10.1.2.3/api/v4", "cloneBaseUrl": "http://10.1.2.3" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// Start work asks where a run should execute; the answer used to be dropped.
#[tokio::test]
async fn execution_placement_can_be_chosen_per_run() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let run = |body: Value| {
        let router = app.router.clone();
        let agent = app.agent_id;
        async move {
            send(
                &router,
                post(&format!("/api/v1/agents/{agent}/executions"), body),
            )
            .await
        }
    };

    let (status, body) = run(json!({ "task": "on kubernetes", "placement": "kubernetes" })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["placement"], "kubernetes");
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();

    // A local worker must not pick it up; a kubernetes worker does.
    let claimed = app
        .store
        .claim_next_queued(
            app.org_id,
            "w-local",
            &["mock".into()],
            "local",
            &[],
            None,
            None,
        )
        .await
        .unwrap();
    assert!(claimed.is_none(), "a local worker claimed a kubernetes run");
    let claimed = app
        .store
        .claim_next_queued(
            app.org_id,
            "w-k8s",
            &["mock".into()],
            "kubernetes",
            &[],
            None,
            None,
        )
        .await
        .unwrap()
        .expect("a kubernetes worker claims it");
    assert_eq!(claimed.id, id);
    sqlx::query("UPDATE executions SET status = 'succeeded', finished_at = now() WHERE id = $1")
        .bind(id)
        .execute(&raw_pool().await)
        .await
        .unwrap();

    // Without a choice the agent's own placement applies (auto for the demo agent).
    let (status, body) = run(json!({ "task": "anywhere" })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["placement"], "auto");

    let (status, body) = run(json!({ "task": "nowhere", "placement": "the moon" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["path"], "placement");
}

/// Two people edit the same agent: the second save must not silently overwrite the first.
#[tokio::test]
async fn agent_update_refuses_an_edit_based_on_an_older_version() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let (status, created) = send(&app.router, post("/api/v1/agents", definition())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["currentVersion"], 1);
    let path = format!("/api/v1/agents/{id}");
    let edit = |system: &str, expected: Option<i64>| {
        let mut body = definition();
        body["spec"]["instructions"]["system"] = json!(system);
        let mut request = json!({ "definition": body });
        if let Some(version) = expected {
            request["expectedVersion"] = json!(version);
        }
        auth_patch(&path, "test-token", request)
    };

    // Alice saves from version 1: version 2.
    let (status, body) = send(&app.router, edit("Alice's instructions", Some(1))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["currentVersion"], 2);

    // Bob also started from version 1: refused, and nothing is written.
    let (status, body) = send(&app.router, edit("Bob's instructions", Some(1))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "conflict");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("version 1") && message.contains("version 2"),
        "{message}"
    );
    let (_, current) = send(&app.router, get(&path)).await;
    assert_eq!(current["currentVersion"], 2);
    assert_eq!(
        current["spec"]["instructions"]["system"],
        "Alice's instructions"
    );

    // After reloading, Bob's edit from version 2 goes through.
    let (status, body) = send(&app.router, edit("Bob's instructions", Some(2))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["currentVersion"], 3);

    // Clients that do not send a version keep the old last-write-wins behaviour.
    let (status, body) = send(&app.router, edit("No version sent", None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["currentVersion"], 4);
}

// ---------------------------------------------------------------------------
// Worker profiles: env values are write-only; auth Secret names are allowlisted (#203)
// ---------------------------------------------------------------------------

fn patch_json(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn profile_body(name: &str, env: Value) -> Value {
    json!({ "name": name, "harnessKind": "mock", "env": env })
}

#[tokio::test]
async fn worker_profile_env_values_are_never_returned_and_updates_keep_them() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let name = format!("env-redact-{}", Uuid::now_v7());
    let (status, created) = send(
        &app.router,
        post(
            "/api/v1/worker-profiles",
            profile_body(
                &name,
                json!({ "API_KEY": "sk-canary-one", "OTHER": "sk-canary-two", "EMPTY": "" }),
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(
        created["env"],
        json!({
            "API_KEY": { "hasValue": true },
            "OTHER": { "hasValue": true },
            "EMPTY": { "hasValue": false },
        })
    );

    // Update: keep API_KEY (null), replace OTHER, add NEW, drop EMPTY by leaving it out.
    let (status, updated) = send(
        &app.router,
        patch_json(
            &format!("/api/v1/worker-profiles/{id}"),
            profile_body(
                &name,
                json!({ "API_KEY": null, "OTHER": "sk-canary-three", "NEW": "sk-canary-four" }),
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(
        updated["env"],
        json!({
            "API_KEY": { "hasValue": true },
            "NEW": { "hasValue": true },
            "OTHER": { "hasValue": true },
        })
    );

    // Nothing that can be read back contains a value: get, list, enable, disable.
    let mut bodies = vec![created.to_string(), updated.to_string()];
    let (_, got) = send(&app.router, get(&format!("/api/v1/worker-profiles/{id}"))).await;
    bodies.push(got.to_string());
    let (_, listed) = send(&app.router, get("/api/v1/worker-profiles")).await;
    bodies.push(listed.to_string());
    for action in ["disable", "enable"] {
        let (status, body) = send(
            &app.router,
            post(&format!("/api/v1/worker-profiles/{id}/{action}"), json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        bodies.push(body.to_string());
    }
    for body in &bodies {
        assert!(!body.contains("sk-canary"), "value leaked: {body}");
    }

    // Stored values are what the update said (kept, replaced, added, removed).
    let stored = app
        .store
        .get_worker_profile(app.org_id, id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(
        stored.env,
        json!({ "API_KEY": "sk-canary-one", "OTHER": "sk-canary-three", "NEW": "sk-canary-four" })
    );

    // Audit entries never carry values.
    let pool = raw_pool().await;
    let audit: Vec<String> = sqlx::query_scalar(
        "SELECT metadata::text FROM audit_logs WHERE org_id = $1 AND action LIKE 'worker_profile.%'",
    )
    .bind(app.org_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!audit.is_empty());
    assert!(audit.iter().all(|entry| !entry.contains("sk-canary")));
}

#[tokio::test]
async fn worker_profile_env_update_validates_keep_and_value_types() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let name = format!("env-validate-{}", Uuid::now_v7());
    // Keeping a variable that has no stored value is a field error on create...
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/worker-profiles",
            profile_body(&name, json!({ "MISSING": null })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["path"], "env.MISSING");

    let (status, created) = send(
        &app.router,
        post(
            "/api/v1/worker-profiles",
            profile_body(&name, json!({ "A": "sk-canary-a" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap();
    // ...and on update; non-string values (including the old response shape) are refused.
    for (env, path) in [
        (json!({ "A": null, "B": null }), "env.B"),
        (json!({ "A": { "hasValue": true } }), "env.A"),
        (json!({ "A": 5 }), "env.A"),
    ] {
        let (status, body) = send(
            &app.router,
            patch_json(
                &format!("/api/v1/worker-profiles/{id}"),
                profile_body(&name, env),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["details"][0]["path"], path);
        assert!(!body.to_string().contains("sk-canary"));
    }
    // A refused update changed nothing.
    let stored = app
        .store
        .get_worker_profile(app.org_id, id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(stored.env, json!({ "A": "sk-canary-a" }));
}

#[tokio::test]
async fn worker_facing_paths_still_receive_env_values_and_resolved_secrets() {
    let Some(app) = test_app(true).await else {
        return;
    };
    // Launcher payload: a mock launcher records the request the API sends it.
    let captured: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
    let sink = captured.clone();
    let mock = axum::Router::new().route(
        "/launches",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push(body);
                axum::Json(json!({ "status": "running", "runtimeId": "pod-1", "workerId": "w-1" }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let mut state = app.state.clone();
    state.launcher_url = Some(format!("http://{addr}"));
    state.launcher_token = Some("launcher-token".into());
    let router = build_router(state);

    let secret_ref = format!("test/env-{}", Uuid::now_v7());
    app.store
        .put_secret(
            Some(app.org_id),
            &secret_ref,
            &orbit_security::SecretValue::new("resolved-secret-value"),
            "t",
        )
        .await
        .unwrap();
    let (status, created) = send(
        &router,
        post(
            "/api/v1/worker-profiles",
            json!({
                "name": format!("worker-facing-{}", Uuid::now_v7()),
                "harnessKind": "mock",
                "target": "kubernetes",
                "authSecretName": "orbit-pi-auth",
                "env": { "PROFILE_VAR": "plain-profile-value" },
                "envMappings": [{ "envVar": "MAPPED", "secretKey": secret_ref }],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["envMappings"][0]["envVar"], "MAPPED");
    let id = created["id"].as_str().unwrap();
    let (status, body) = send(
        &router,
        post(&format!("/api/v1/worker-profiles/{id}/enable"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, token) = send(
        &router,
        post(&format!("/api/v1/worker-profiles/{id}/token"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{token}");

    // Worker env resolution resolves the referenced secret for the worker.
    let (status, resolved) = send(
        &router,
        worker_request(
            "GET",
            "/api/v1/worker/env",
            token["token"].as_str().unwrap(),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert!(resolved
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["envVar"] == "MAPPED" && item["value"] == "resolved-secret-value"));

    // The launcher payload still carries the stored profile env, un-redacted.
    let (status, launch) = send(
        &router,
        post(&format!("/api/v1/worker-profiles/{id}/launch"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{launch}");
    assert!(!launch.to_string().contains("plain-profile-value"));
    let sent = captured.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["env"]["PROFILE_VAR"], "plain-profile-value");
    assert_eq!(sent[0]["authSecretName"], "orbit-pi-auth");
}

#[tokio::test]
async fn env_mappings_hold_references_only() {
    let Some(app) = test_app(true).await else {
        return;
    };
    // Inline mappings on a profile: an extra key is refused, nothing is stored.
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/worker-profiles",
            json!({
                "name": format!("inline-{}", Uuid::now_v7()),
                "harnessKind": "mock",
                "envMappings": [{ "envVar": "X", "secretKey": "k", "value": "sk-canary-inline" }],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["path"], "envMappings");
    assert!(!body.to_string().contains("sk-canary"));

    // Reusable mappings: same rule.
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/env-mappings",
            json!({
                "name": format!("reusable-{}", Uuid::now_v7()),
                "mappings": [{ "envVar": "X", "secretKey": "k", "value": "sk-canary-reusable" }],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(!body.to_string().contains("sk-canary"));

    // Rows stored by an older version with a stray `value` key are not echoed back.
    let legacy = app
        .store
        .create_env_mapping(
            app.org_id,
            &format!("legacy-{}", Uuid::now_v7()),
            "",
            &json!([{ "envVar": "X", "secretKey": "k", "value": "sk-canary-legacy" }]),
        )
        .await
        .unwrap();
    let (_, listed) = send(&app.router, get("/api/v1/env-mappings")).await;
    assert!(listed.to_string().contains(&legacy.id.to_string()));
    assert!(!listed.to_string().contains("sk-canary"));
    assert_eq!(
        serde_json::to_value(&legacy).unwrap()["mappings"],
        json!([{ "envVar": "X", "secretKey": "k" }])
    );
}

#[tokio::test]
async fn worker_profile_auth_secret_name_must_be_allowed() {
    let Some(app) = test_app(true).await else {
        return;
    };
    for bad in [
        "orbit-orbit",
        "orbit-secrets",
        "postgres",
        "Bad_Name",
        "../x",
        "orbit-pi-auth-",
    ] {
        let (status, body) = send(
            &app.router,
            post(
                "/api/v1/worker-profiles",
                json!({
                    "name": format!("auth-bad-{}", Uuid::now_v7()),
                    "harnessKind": "mock",
                    "authSecretName": bad,
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body}");
        assert_eq!(
            body["error"]["details"][0]["path"], "authSecretName",
            "{bad}"
        );
    }
    let mut ids = Vec::new();
    for ok in [
        "orbit-opencode-auth",
        "orbit-pi-auth",
        "orbit-pi-auth-team-a",
    ] {
        let (status, body) = send(
            &app.router,
            post(
                "/api/v1/worker-profiles",
                json!({
                    "name": format!("auth-ok-{}", Uuid::now_v7()),
                    "harnessKind": "mock",
                    "authSecretName": ok,
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{ok}: {body}");
        ids.push(body["id"].as_str().unwrap().to_string());
    }
    // Update applies the same rule.
    let (status, body) = send(
        &app.router,
        patch_json(
            &format!("/api/v1/worker-profiles/{}", ids[0]),
            json!({ "name": "renamed-auth", "harnessKind": "mock", "authSecretName": "orbit-orbit" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

// ---- #202 / #218: object-level checks behind settings:write, scoped lists ----

struct D2Fixture {
    mallory: orbit_store::User,
    mallory_token: String,
    team_a: Uuid,
    profile_id: Uuid,
}

async fn d2_profile(app: &TestApp, name: &str) -> Uuid {
    app.store
        .create_worker_profile(
            app.org_id,
            name,
            "mock",
            "local",
            "process",
            "orbit-worker",
            "test",
            "all",
            &[],
            &[],
            None,
            None,
            None,
            &json!({}),
            &json!({}),
            &json!({}),
            false,
            false,
            false,
            false,
            &json!({}),
        )
        .await
        .unwrap()
        .id
}

/// A custom role with settings:write that is neither an Owner nor in team A, plus a worker
/// profile owned by team A.
async fn d2_fixture(app: &TestApp) -> D2Fixture {
    let role = format!("TeamAdmin-{}", Uuid::now_v7());
    app.store
        .create_custom_role(
            app.org_id,
            &role,
            &[
                "agents:read".into(),
                "agents:write".into(),
                "executions:read".into(),
                "harnesses:read".into(),
                "providers:read".into(),
                "connections:read".into(),
                "settings:read".into(),
                "settings:write".into(),
                "members:read".into(),
            ],
        )
        .await
        .unwrap();
    let mallory = app
        .store
        .ensure_user(
            app.org_id,
            &format!("mallory-{}@test.local", Uuid::now_v7()),
            "Mallory",
            UserRole::Viewer,
        )
        .await
        .unwrap();
    app.store
        .set_user_role_name(app.org_id, mallory.id, &role)
        .await
        .unwrap();
    let mallory_token = token_for(&app.store, app.org_id, mallory.id, "mallory").await;
    let team_a = app
        .store
        .create_access_team(
            app.org_id,
            "Team A",
            &format!("team-a-{}", Uuid::now_v7()),
            None,
        )
        .await
        .unwrap()
        .id;
    let profile_id = d2_profile(app, "Team A worker").await;
    app.store
        .set_resource_visibility(
            app.org_id,
            "worker_profiles",
            profile_id,
            "team",
            Some(team_a),
        )
        .await
        .unwrap();
    D2Fixture {
        mallory,
        mallory_token,
        team_a,
        profile_id,
    }
}

/// App whose router sees an enterprise license, so premium role endpoints reach their
/// permission checks instead of stopping at the feature gate. The key is registered only
/// for this test binary; no production key or secret is involved.
async fn licensed_test_app() -> Option<TestApp> {
    let mut app = test_app(true).await?;
    use ed25519_dalek::{Signer, SigningKey};

    let signing = SigningKey::from_bytes(&[202u8; 32]);
    let kid = "orbit-test-role-grants";
    let public_key =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes());
    std::env::set_var(
        "ORBIT_LICENSE_PUBLIC_KEYS",
        json!({ kid: public_key }).to_string(),
    );
    let encode = |value: Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&value).expect("license JSON"))
    };
    let header = encode(json!({ "alg": "EdDSA", "kid": kid }));
    let claims = encode(json!({
        "v": 1,
        "id": "orbit-test-role-grants",
        "customer": "Test",
        "plan": "enterprise",
        "features": ["custom_roles", "teams"],
        "issued_at": "2020-01-01T00:00:00Z",
        "expires_at": "2099-01-01T00:00:00Z",
        "grace_days": 0,
    }));
    let signed = format!("{header}.{claims}");
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(signing.sign(signed.as_bytes()).to_bytes());
    app.state.license = orbit_security::LicenseHandle::load(orbit_security::LicenseSource {
        token: Some(format!("{signed}.{signature}")),
        file: None,
    });
    // The router keeps its own clone of the state, so rebuild it with the licensed one.
    app.router = orbit_api::build_router(app.state.clone());
    Some(app)
}

#[tokio::test]
async fn settings_write_holder_cannot_touch_other_teams_worker_objects() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let fx = d2_fixture(&app).await;
    let m = fx.mallory_token.as_str();

    // Worker tokens: rotate, create (for the profile, and org-wide), revoke.
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/worker-profiles/{}/token", fx.profile_id),
            m,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app.router,
        auth_post(
            "/api/v1/worker-tokens",
            m,
            json!({ "name": "steal", "profileId": fx.profile_id }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app.router,
        auth_post(
            "/api/v1/worker-tokens",
            m,
            json!({ "name": "any", "harnesses": ["mock"] }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "org-wide tokens are privileged-only"
    );
    let profile_token = app
        .store
        .create_worker_token(
            app.org_id,
            Some(fx.profile_id),
            "a",
            &format!("pfx{}", Uuid::now_v7()),
            &format!("h{}", Uuid::now_v7()),
            &["mock".into()],
        )
        .await
        .unwrap();
    let org_token = app
        .store
        .create_worker_token(
            app.org_id,
            None,
            "org",
            &format!("pfx{}", Uuid::now_v7()),
            &format!("h{}", Uuid::now_v7()),
            &["mock".into()],
        )
        .await
        .unwrap();
    for id in [profile_token.id, org_token.id] {
        let (status, _) = send(
            &app.router,
            auth_delete(&format!("/api/v1/worker-tokens/{id}"), m),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, listed) = send(&app.router, auth_get("/api/v1/worker-tokens", m)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["id"] != json!(profile_token.id) && t["id"] != json!(org_token.id)));

    // Profile delete, launch stop/delete/archive.
    let (status, _) = send(
        &app.router,
        auth_delete(&format!("/api/v1/worker-profiles/{}", fx.profile_id), m),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let launch = app
        .store
        .create_worker_launch(
            app.org_id,
            fx.profile_id,
            "launch",
            "local",
            "orbit-worker:test",
            None,
            None,
            false,
        )
        .await
        .unwrap();
    for (method, suffix) in [("POST", "/stop"), ("POST", "/archive"), ("DELETE", "")] {
        let (status, _) = send(
            &app.router,
            auth_json(
                method,
                &format!("/api/v1/worker-launches/{}{suffix}", launch.id),
                m,
                json!({}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {suffix}");
    }
    // Nothing was changed: the owner can still see the launch.
    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/worker-launches/{}", launch.id),
            "test-token",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A privileged caller (Owner) still manages them, including org-wide tokens.
    let (status, _) = send(
        &app.router,
        auth_delete(
            &format!("/api/v1/worker-tokens/{}", profile_token.id),
            "test-token",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app.router,
        auth_post(
            "/api/v1/worker-tokens",
            "test-token",
            json!({ "name": "org", "harnesses": ["mock"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // A team lead can manage their own team's profile token.
    app.store
        .upsert_team_member(fx.team_a, fx.mallory.id, "lead")
        .await
        .unwrap();
    let (status, _) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/worker-profiles/{}/token", fx.profile_id),
            m,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn settings_write_holder_cannot_attach_other_teams_mappings_or_profiles() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let fx = d2_fixture(&app).await;
    let m = fx.mallory_token.as_str();
    let mapping = app
        .store
        .create_env_mapping(
            app.org_id,
            "team a env",
            "",
            &json!([{ "envVar": "TOKEN", "secretKey": "x" }]),
        )
        .await
        .unwrap();
    app.store
        .set_resource_visibility(
            app.org_id,
            "env_mappings",
            mapping.id,
            "team",
            Some(fx.team_a),
        )
        .await
        .unwrap();
    let body = json!({
        "name": "m", "mappings": [{ "envVar": "TOKEN", "secretKey": "x" }]
    });
    let (status, _) = send(
        &app.router,
        auth_json(
            "PATCH",
            &format!("/api/v1/env-mappings/{}", mapping.id),
            m,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app.router,
        auth_delete(&format!("/api/v1/env-mappings/{}", mapping.id), m),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, listed) = send(&app.router, auth_get("/api/v1/env-mappings", m)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["id"] != json!(mapping.id)));
    let (status, _) = send(
        &app.router,
        auth_get(
            &format!("/api/v1/worker-profiles/{}/env-mappings", fx.profile_id),
            m,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Agent teams: step agents, worker profile and env mappings must be visible.
    let hidden_agent = create_demo_agent(&app.store, app.org_id, fx.mallory.id).await;
    app.store
        .set_resource_visibility(app.org_id, "agents", hidden_agent, "team", Some(fx.team_a))
        .await
        .unwrap();
    for (field, value) in [
        ("steps", json!([{ "agentId": hidden_agent }])),
        ("workerProfileId", json!(fx.profile_id)),
        ("envMappingIds", json!([mapping.id])),
    ] {
        let mut request = json!({
            "name": "t", "mode": "pipeline",
            "steps": [{ "agentId": app.agent_id }],
        });
        request[field] = value;
        let (status, body) = send(
            &app.router,
            auth_post("/api/v1/agent-teams", m, request.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field}: {body}");
    }
}

#[tokio::test]
async fn outputs_require_manage_on_their_target() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let fx = d2_fixture(&app).await;
    let m = fx.mallory_token.as_str();
    // Agent owned by team A (mallory is not a member), and one mallory owns.
    let owner_id = app.store.list_users(app.org_id).await.unwrap()[0].id;
    let team_agent = create_demo_agent(&app.store, app.org_id, owner_id).await;
    app.store
        .set_resource_visibility(app.org_id, "agents", team_agent, "team", Some(fx.team_a))
        .await
        .unwrap();
    let own_agent = create_demo_agent(&app.store, app.org_id, fx.mallory.id).await;
    make_private(&app.store, app.org_id, "agents", own_agent, fx.mallory.id).await;
    let team = app
        .store
        .create_agent_team(
            app.org_id,
            "team a pipeline",
            "",
            &json!({ "mode": "pipeline", "steps": [{ "agentId": team_agent }] }),
            None,
        )
        .await
        .unwrap();
    app.store
        .set_resource_visibility(app.org_id, "agent_teams", team.id, "team", Some(fx.team_a))
        .await
        .unwrap();
    let hook = "http://10.0.0.5/hook";
    let create = |agent: Option<Uuid>, team: Option<Uuid>| json!({ "name": format!("o-{}", Uuid::now_v7()), "kind": "webhook", "agentId": agent, "teamId": team, "webhookUrl": hook });

    let (status, _) = send(
        &app.router,
        auth_post("/api/v1/outputs", m, create(Some(team_agent), None)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "other team's agent");
    let (status, _) = send(
        &app.router,
        auth_post("/api/v1/outputs", m, create(None, Some(team.id))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "other team's agent team");
    let (status, _) = send(
        &app.router,
        auth_post("/api/v1/outputs", m, create(None, None)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "org-wide output");
    let (status, own) = send(
        &app.router,
        auth_post("/api/v1/outputs", m, create(Some(own_agent), None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{own}");
    let own_id = own["id"].as_str().unwrap().to_string();

    // Retargeting my output to someone else's agent, or org-wide, is refused.
    for target in [create(Some(team_agent), None), create(None, None)] {
        let (status, _) = send(
            &app.router,
            auth_json("PATCH", &format!("/api/v1/outputs/{own_id}"), m, target),
        )
        .await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
            "{status}"
        );
    }

    // Existing outputs of other teams / org-wide: no update, delete or test, hidden in list.
    let (status, owner_made) = send(
        &app.router,
        auth_post(
            "/api/v1/outputs",
            "test-token",
            create(Some(team_agent), None),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{owner_made}");
    let (status, org_wide) = send(
        &app.router,
        auth_post("/api/v1/outputs", "test-token", create(None, None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org_wide}");
    for id in [
        owner_made["id"].as_str().unwrap(),
        org_wide["id"].as_str().unwrap(),
    ] {
        let path = format!("/api/v1/outputs/{id}");
        let (status, _) = send(
            &app.router,
            auth_json("PATCH", &path, m, create(Some(own_agent), None)),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "update {id}");
        let (status, _) = send(&app.router, auth_delete(&path, m)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "delete {id}");
        let (status, _) = send(
            &app.router,
            auth_post(&format!("{path}/test"), m, json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "test {id}");
    }
    let (status, listed) = send(&app.router, auth_get("/api/v1/outputs", m)).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&own_id.as_str()));
    assert!(
        !ids.contains(&owner_made["id"].as_str().unwrap()),
        "{ids:?}"
    );
    let (_, owner_list) = send(&app.router, auth_get("/api/v1/outputs", "test-token")).await;
    assert!(owner_list.as_array().unwrap().len() >= 3);
}

#[tokio::test]
async fn settings_write_holder_cannot_escalate_roles_or_revoke_foreign_tokens() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let fx = d2_fixture(&app).await;
    let m = fx.mallory_token.as_str();
    let owner_id = app.store.list_users(app.org_id).await.unwrap()[0].id;
    let victim = app
        .store
        .ensure_user(
            app.org_id,
            &format!("victim-{}@test.local", Uuid::now_v7()),
            "Victim",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let patch_role = |user: Uuid, role: &str| {
        auth_patch(&format!("/api/v1/users/{user}"), m, json!({ "role": role }))
    };
    // Cannot promote anyone to Owner/Admin (permissions above the caller's own).
    for role in ["owner", "admin", "operator"] {
        let (status, body) = send(&app.router, patch_role(victim.id, role)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{role}: {body}");
    }
    // Cannot demote an Owner.
    let (status, _) = send(&app.router, patch_role(owner_id, "viewer")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The victim (an Operator today) holds more than the caller, so even a demotion to a
    // role the caller holds is refused.
    let (status, _) = send(&app.router, patch_role(victim.id, "viewer")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A viewer can be moved to another role the caller fully holds.
    let viewer = app
        .store
        .ensure_user(
            app.org_id,
            &format!("viewer-{}@test.local", Uuid::now_v7()),
            "Viewer",
            UserRole::Viewer,
        )
        .await
        .unwrap();
    let (status, body) = send(&app.router, patch_role(viewer.id, "viewer")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The Owner may assign Owner.
    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/users/{}", victim.id),
            "test-token",
            json!({ "role": "owner" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // API tokens: own only, unless privileged.
    token_for(&app.store, app.org_id, victim.id, "victim").await;
    let (status, listed) = send(&app.router, auth_get("/api/v1/api-tokens", m)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    let (_, all) = send(&app.router, auth_get("/api/v1/api-tokens", "test-token")).await;
    let victim_token_id = all
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "victim")
        .map(|t| t["id"].as_str().unwrap().to_string())
        .expect("owner sees every token");
    let (status, _) = send(
        &app.router,
        auth_delete(&format!("/api/v1/api-tokens/{victim_token_id}"), m),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app.router,
        auth_delete(
            &format!("/api/v1/api-tokens/{victim_token_id}"),
            "test-token",
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "privileged callers may revoke any token"
    );
}

#[tokio::test]
async fn user_list_hides_other_users_oidc_identity_from_non_privileged_callers() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let fx = d2_fixture(&app).await;
    let issuer = format!("https://idp.test/{}", Uuid::now_v7());
    let linked = app
        .store
        .upsert_oidc_user(
            app.org_id,
            &issuer,
            "subject-123",
            Some(&format!("linked-{}@test.local", Uuid::now_v7())),
            Some("Linked"),
            UserRole::Viewer,
            true,
        )
        .await
        .unwrap();
    let find = |list: &Value| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|u| u["id"] == json!(linked.id))
            .cloned()
            .expect("user listed")
    };
    let (status, list) = send(&app.router, auth_get("/api/v1/users", &fx.mallory_token)).await;
    assert_eq!(status, StatusCode::OK);
    let seen = find(&list);
    assert!(
        seen["oidcSubject"].is_null() && seen["oidcIssuer"].is_null(),
        "{seen}"
    );
    let (_, list) = send(&app.router, auth_get("/api/v1/users", "test-token")).await;
    assert_eq!(find(&list)["oidcSubject"], "subject-123");
}

#[tokio::test]
async fn agent_instances_listing_is_scoped_like_agents() {
    let Some(app) = test_app(true).await else {
        return;
    };
    let fx = d2_fixture(&app).await;
    let (status, body) = send(
        &app.router,
        auth_get("/api/v1/agent-instances", &fx.mallory_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 0);
    // The scoped query itself (the table is never written by the product today).
    let access = orbit_store::access::ResourceAccess::new(fx.mallory.id, false, vec![]);
    assert!(app
        .store
        .list_agent_instances(app.org_id, Some(&access))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn settings_write_holder_cannot_grant_beyond_own_permissions_in_roles() {
    let Some(app) = licensed_test_app().await else {
        return;
    };
    let fx = d2_fixture(&app).await;
    let m = fx.mallory_token.as_str();

    // An Owner holds every permission, so the wildcard itself is legal for them. This
    // also proves the licensed router reaches the role handlers (not the feature gate).
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/roles",
            "test-token",
            json!({
                "name": format!("owner-grant-{}", Uuid::now_v7()),
                "permissions": ["*"],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Mallory has settings:write but lacks `*`, audit:read and providers:write.
    let owner_permissions: Vec<String> = orbit_security::Permission::all_for(UserRole::Owner)
        .into_iter()
        .map(str::to_string)
        .collect();
    for (label, permissions) in [
        ("wildcard", json!(["*"])),
        ("unheld audit read", json!(["agents:read", "audit:read"])),
        (
            "unheld providers write",
            json!(["agents:read", "providers:write"]),
        ),
        ("owner's full set", json!(owner_permissions.clone())),
    ] {
        let name = format!("grant-{}-{}", label.replace(' ', "-"), Uuid::now_v7());
        let (status, body) = send(
            &app.router,
            auth_post(
                "/api/v1/roles",
                m,
                json!({ "name": name, "permissions": permissions }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}: {body}");
        assert_eq!(body["error"]["code"], "forbidden", "{body}");
        assert!(
            app.store
                .custom_role_permissions(app.org_id, &name)
                .await
                .unwrap()
                .is_none(),
            "{label}: role was created"
        );
    }

    // A role made only of permissions she actually holds is still allowed.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/roles",
            m,
            json!({
                "name": format!("delegated-{}", Uuid::now_v7()),
                "permissions": ["agents:read", "settings:write"],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Updating someone else's role is bounded the same way.
    let target = app
        .store
        .create_custom_role(
            app.org_id,
            &format!("target-{}", Uuid::now_v7()),
            &["agents:read".into()],
        )
        .await
        .unwrap();
    for (label, permissions) in [
        ("wildcard", json!(["*"])),
        ("unheld", json!(["agents:read", "audit:read"])),
        ("owner's full set", json!(owner_permissions)),
    ] {
        let (status, body) = send(
            &app.router,
            auth_patch(
                &format!("/api/v1/roles/{}", target.id),
                m,
                json!({ "name": target.name, "permissions": permissions }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}: {body}");
    }
    let unchanged = app
        .store
        .get_custom_role(app.org_id, target.id)
        .await
        .unwrap();
    assert_eq!(unchanged.permissions, vec!["agents:read".to_string()]);

    // Editing the role she holds herself is refused even without new permissions.
    // `fx.mallory` predates the role assignment, so read the current role name back.
    let held_role = app
        .store
        .get_user(app.org_id, fx.mallory.id)
        .await
        .unwrap()
        .role;
    let own = app
        .store
        .list_custom_roles(app.org_id)
        .await
        .unwrap()
        .into_iter()
        .find(|role| role.name == held_role)
        .expect("mallory's custom role");
    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/roles/{}", own.id),
            m,
            json!({ "name": own.name, "permissions": own.permissions.clone() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        body["error"]["message"], "you cannot edit your own role",
        "{body}"
    );

    // The Owner may still grant the wildcard through an update.
    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/roles/{}", target.id),
            "test-token",
            json!({ "name": target.name, "permissions": ["*"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn settings_write_holder_cannot_become_or_assign_owner() {
    let Some(app) = licensed_test_app().await else {
        return;
    };
    let fx = d2_fixture(&app).await;
    let m = fx.mallory_token.as_str();
    let victim = app
        .store
        .ensure_user(
            app.org_id,
            &format!("victim-{}@test.local", Uuid::now_v7()),
            "Victim",
            UserRole::Viewer,
        )
        .await
        .unwrap();

    // Built-in role names stay reserved in both create and update, so a custom role
    // cannot be used as an alias for Owner.
    let (status, body) = send(
        &app.router,
        auth_post(
            "/api/v1/roles",
            m,
            json!({ "name": "owner", "permissions": ["agents:read"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let target = app
        .store
        .create_custom_role(
            app.org_id,
            &format!("target-{}", Uuid::now_v7()),
            &["agents:read".into()],
        )
        .await
        .unwrap();
    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/roles/{}", target.id),
            m,
            json!({ "name": "owner", "permissions": ["agents:read"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let unchanged = app
        .store
        .get_custom_role(app.org_id, target.id)
        .await
        .unwrap();
    assert_eq!(unchanged.name, target.name);

    // Neither self-promotion nor promotion of another user to Owner is allowed.
    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/users/{}", fx.mallory.id),
            m,
            json!({ "role": "owner" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/users/{}", victim.id),
            m,
            json!({ "role": "owner" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        app.store
            .get_user(app.org_id, victim.id)
            .await
            .unwrap()
            .role,
        "viewer"
    );

    // An Owner can still assign Owner.
    let (status, body) = send(
        &app.router,
        auth_patch(
            &format!("/api/v1/users/{}", victim.id),
            "test-token",
            json!({ "role": "owner" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        app.store
            .get_user(app.org_id, victim.id)
            .await
            .unwrap()
            .role,
        "owner"
    );
}

async fn seed_overview_agent(
    app: &TestApp,
    owner_id: Uuid,
    name: &str,
    team_id: Option<Uuid>,
    visibility: &str,
    updated_seconds_ahead: i32,
) -> Uuid {
    let mut def: orbit_domain::AgentDefinition =
        serde_json::from_value(definition()).expect("definition");
    def.metadata.name = name.into();
    let (agent, _) = app
        .store
        .create_agent(app.org_id, Some(owner_id), &def)
        .await
        .unwrap();
    app.store
        .set_resource_visibility(app.org_id, "agents", agent.id, visibility, team_id)
        .await
        .unwrap();
    sqlx::query("UPDATE agents SET updated_at = now() + ($2 * interval '1 second') WHERE id = $1")
        .bind(agent.id)
        .bind(updated_seconds_ahead)
        .execute(app.store.pool())
        .await
        .unwrap();
    agent.id
}

#[tokio::test]
async fn team_resource_overview_enforces_scope_and_isolation() {
    let Some(app) = licensed_test_app().await else {
        return;
    };
    let owner = owner_user(&app).await;
    let lead = app
        .store
        .ensure_user(
            app.org_id,
            "overview-lead@test.local",
            "Overview Lead",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let member = app
        .store
        .ensure_user(
            app.org_id,
            "overview-member@test.local",
            "Overview Member",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let viewer = app
        .store
        .ensure_user(
            app.org_id,
            "overview-viewer@test.local",
            "Overview Viewer",
            UserRole::Viewer,
        )
        .await
        .unwrap();
    let outsider = app
        .store
        .ensure_user(
            app.org_id,
            "overview-outsider@test.local",
            "Overview Outsider",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let admin = app
        .store
        .ensure_user(
            app.org_id,
            "overview-admin@test.local",
            "Overview Admin",
            UserRole::Admin,
        )
        .await
        .unwrap();
    let extra = app
        .store
        .ensure_user(
            app.org_id,
            "overview-extra@test.local",
            "Overview Extra",
            UserRole::Operator,
        )
        .await
        .unwrap();

    app.store
        .set_platform_setting(
            "team_isolation",
            &json!({ "enabled": true }),
            Some(owner.id),
        )
        .await
        .unwrap();

    let team_a = app
        .store
        .create_access_team(
            app.org_id,
            "Overview A",
            &format!("overview-a-{}", Uuid::now_v7()),
            None,
        )
        .await
        .unwrap();
    let team_b = app
        .store
        .create_access_team(
            app.org_id,
            "Overview B",
            &format!("overview-b-{}", Uuid::now_v7()),
            None,
        )
        .await
        .unwrap();
    for (user, role) in [(&lead, "lead"), (&member, "member"), (&viewer, "viewer")] {
        app.store
            .upsert_team_member(team_a.id, user.id, role)
            .await
            .unwrap();
    }

    seed_overview_agent(&app, owner.id, "Team agent one", Some(team_a.id), "team", 3).await;
    seed_overview_agent(&app, owner.id, "Team agent two", Some(team_a.id), "team", 2).await;
    let skill = app
        .store
        .create_skill(
            app.org_id,
            &format!("overview-skill-{}", Uuid::now_v7()),
            "Team skill org",
            "",
            "# skill",
            &json!({}),
        )
        .await
        .unwrap();
    app.store
        .set_resource_visibility(app.org_id, "skills", skill.id, "org", Some(team_a.id))
        .await
        .unwrap();
    sqlx::query("UPDATE skills SET updated_at = now() + interval '1 second' WHERE id = $1")
        .bind(skill.id)
        .execute(app.store.pool())
        .await
        .unwrap();
    seed_overview_agent(&app, owner.id, "Org shared agent", None, "org", 0).await;
    // Invisible to this overview: another team's resource and a personal private resource.
    seed_overview_agent(
        &app,
        owner.id,
        "SECRET other team",
        Some(team_b.id),
        "org",
        5,
    )
    .await;
    seed_overview_agent(&app, lead.id, "SECRET personal", None, "private", 4).await;

    let lead_token = token_for(&app.store, app.org_id, lead.id, "overview-lead").await;
    let member_token = token_for(&app.store, app.org_id, member.id, "overview-member").await;
    let viewer_token = token_for(&app.store, app.org_id, viewer.id, "overview-viewer").await;
    let outsider_token = token_for(&app.store, app.org_id, outsider.id, "overview-outsider").await;
    let admin_token = token_for(&app.store, app.org_id, admin.id, "overview-admin").await;
    let url = format!("/api/v1/teams/{}/resources", team_a.id);

    // Team lead: full page, summary and management hint.
    let (status, body) = send(&app.router, auth_get(&url, &lead_token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 4, "{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 4, "{body}");
    assert_eq!(body["summary"]["roles"]["lead"], 1, "{body}");
    assert_eq!(body["summary"]["roles"]["member"], 1, "{body}");
    assert_eq!(body["summary"]["roles"]["viewer"], 1, "{body}");
    assert_eq!(body["summary"]["resources"]["agents"], 3, "{body}");
    assert_eq!(body["summary"]["resources"]["skills"], 1, "{body}");
    assert_eq!(body["summary"]["orgVisibleCount"], 1, "{body}");
    assert_eq!(body["summary"]["canManageTeam"], true, "{body}");
    assert!(body["items"].to_string().contains("Team skill org"));
    assert!(!body.to_string().contains("SECRET"), "{body}");
    let names: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "Team agent one",
            "Team agent two",
            "Team skill org",
            "Org shared agent"
        ],
        "{body}"
    );
    let team_owned = &body["items"][0];
    assert_eq!(team_owned["ownerTeamId"], team_a.id.to_string());
    assert_eq!(team_owned["isOwnerTeam"], true);
    assert_eq!(team_owned["visibility"], "team");
    assert_eq!(
        team_owned["effectiveAccess"],
        json!(["view", "use", "manage"]),
        "{body}"
    );

    // Team viewer: can read, but cannot use or manage team-owned resources, and cannot
    // change membership.
    let (status, body) = send(&app.router, auth_get(&url, &viewer_token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["summary"]["canManageTeam"], false, "{body}");
    assert_eq!(body["total"], 4, "{body}");
    assert_eq!(
        body["items"][0]["effectiveAccess"],
        json!(["view"]),
        "{body}"
    );
    let (status, denied) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/teams/{}/members", team_a.id),
            &viewer_token,
            json!({ "userId": extra.id, "role": "viewer" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(denied["error"]["code"], "forbidden");

    // Team member: use but not manage.
    let (status, body) = send(&app.router, auth_get(&url, &member_token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["items"][0]["effectiveAccess"],
        json!(["view", "use"]),
        "{body}"
    );
    assert_eq!(body["summary"]["canManageTeam"], false, "{body}");
    let (status, denied) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/teams/{}/members", team_a.id),
            &member_token,
            json!({ "userId": extra.id, "role": "viewer" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");

    // Non-member: no existence leak, even with a valid team id.
    let (status, body) = send(&app.router, auth_get(&url, &outsider_token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found", "{body}");

    // Org admin: same team-scoped view, never another team's resources.
    let (status, body) = send(&app.router, auth_get(&url, &admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 4, "{body}");
    assert_eq!(body["summary"]["canManageTeam"], true, "{body}");
    assert!(!body.to_string().contains("SECRET"), "{body}");
    assert!(body["items"][0]["effectiveAccess"]
        .as_array()
        .unwrap()
        .contains(&json!("manage")));

    // Type filter and keyset pagination keep the same visibility rules.
    let (status, body) = send(
        &app.router,
        auth_get(&format!("{url}?type=skills"), &member_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 1, "{body}");
    assert_eq!(body["items"][0]["resourceType"], "skills", "{body}");

    let (status, body) = send(
        &app.router,
        auth_get(&format!("{url}?limit=2"), &member_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 2, "{body}");
    let cursor = body["nextCursor"].as_str().unwrap().to_string();
    assert!(!cursor.is_empty());
    let (status, body) = send(
        &app.router,
        auth_get(&format!("{url}?limit=2&cursor={cursor}"), &member_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 2, "{body}");
    assert!(body["nextCursor"].is_null(), "{body}");

    let (status, body) = send(
        &app.router,
        auth_get(&format!("{url}?type=outputs"), &member_token),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // Team lead and org admin may change membership; API enforces it independently of UI.
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/teams/{}/members", team_a.id),
            &lead_token,
            json!({ "userId": extra.id, "role": "viewer" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        &app.router,
        auth_post(
            &format!("/api/v1/teams/{}/members", team_a.id),
            &admin_token,
            json!({ "userId": outsider.id, "role": "member" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn license_usage_counts_every_organization() {
    let generous = json!({
        "seats": 1_000_000,
        "agents": 1_000_000,
        "workers": 1_000_000,
        "executionsPerDay": 1_000_000
    });
    let Some(app) = test_app_with_license(true, Some(licensed(&["teams"], generous))).await else {
        return;
    };
    let (status, before) = send(&app.router, get("/api/v1/license")).await;
    assert_eq!(status, StatusCode::OK, "{before}");

    let slug = format!("license-org-{}", Uuid::now_v7());
    let org = app
        .store
        .create_organization(
            &slug,
            "Second org",
            orbit_domain::OrganizationMode::Personal,
        )
        .await
        .unwrap();
    app.store
        .ensure_user(org.id, "second@test.local", "Second", UserRole::Owner)
        .await
        .unwrap();
    let definition: orbit_domain::AgentDefinition = serde_json::from_value(definition()).unwrap();
    let (agent, _) = app
        .store
        .create_agent(org.id, None, &definition)
        .await
        .unwrap();
    let _ = agent;

    let (status, after) = send(&app.router, get("/api/v1/license")).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert!(
        after["usage"]["seats"].as_i64().unwrap() > before["usage"]["seats"].as_i64().unwrap(),
        "seats must aggregate all orgs: {before} -> {after}"
    );
    assert!(
        after["usage"]["agents"].as_i64().unwrap() > before["usage"]["agents"].as_i64().unwrap(),
        "agents must aggregate all orgs: {before} -> {after}"
    );
}

#[tokio::test]
async fn zero_limits_reject_creates_with_402() {
    let Some(app) = test_app_with_license(
        true,
        Some(licensed(&[], json!({ "agents": 0, "executionsPerDay": 0 }))),
    )
    .await
    else {
        return;
    };

    let (status, body) = send(&app.router, post("/api/v1/agents", definition())).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["error"]["code"], "license_limit");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("agents"));

    let (status, body) = send(
        &app.router,
        post(
            &format!("/api/v1/agents/{}/executions", app.agent_id),
            json!({ "task": "blocked by license" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["error"]["code"], "license_limit");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("executions_per_day"));
}

#[tokio::test]
async fn concurrent_agent_creates_cannot_race_past_the_limit() {
    let Some(url) = std::env::var("ORBIT_TEST_DATABASE_URL").ok() else {
        return;
    };
    // Derive the limit from the current installation-wide count: one slot is available.
    // Five connections for six contenders is intentional: failed try-lock attempts must
    // release their transaction before sleeping, or the lock holder cannot finish.
    let store = Store::connect(&url, 5).await.unwrap();
    let current = store.count_agents_installation().await.unwrap();
    let Some(app) =
        test_app_with_license(true, Some(licensed(&[], json!({ "agents": current + 1 })))).await
    else {
        return;
    };

    let mut tasks = Vec::new();
    for index in 0..6 {
        let router = app.router.clone();
        let mut body = definition();
        body["metadata"]["name"] = json!(format!("Concurrent agent {index}"));
        tasks.push(tokio::spawn(async move {
            send(&router, post("/api/v1/agents", body)).await
        }));
    }
    let mut created = 0;
    let mut limited = 0;
    for task in tasks {
        let (status, body) = task.await.unwrap();
        if status == StatusCode::CREATED {
            created += 1;
        } else {
            assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
            assert_eq!(body["error"]["code"], "license_limit");
            limited += 1;
        }
    }
    assert!(
        created <= 1,
        "at most one create may pass the lock: {created}"
    );
    assert!(
        limited >= 1,
        "the remaining creates must be refused: {limited}"
    );
}

#[tokio::test]
async fn reserved_license_flags_are_not_advertised() {
    let flags = [
        "custom_roles",
        "teams",
        "resource_permissions",
        "shared_connections",
        "quotas",
        "marketplace",
    ];
    let Some(app) = test_app_with_license(true, Some(licensed(&flags, json!({})))).await else {
        return;
    };

    let (status, body) = send(&app.router, get("/api/v1/license")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["license"]["features"].as_array().unwrap().len(), 6);
    assert_eq!(
        body["license"]["availableFeatures"],
        json!(["custom_roles", "teams"])
    );
    assert!(body["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("not available")));

    // Airgapped startup: no activation call or vendor network client is involved; /meta
    // exposes the same checked status to the web app.
    let (status, meta) = send(&app.router, get("/api/v1/meta")).await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(
        meta["license"]["availableFeatures"],
        json!(["custom_roles", "teams"])
    );
}

#[tokio::test]
async fn premium_feature_returns_402_without_the_flag() {
    let Some(app) = test_app_with_license(true, Some(licensed(&["teams"], json!({})))).await else {
        return;
    };
    let (status, body) = send(
        &app.router,
        post(
            "/api/v1/roles",
            json!({ "name": format!("lic-role-{}", Uuid::now_v7()), "permissions": ["agents:read"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["error"]["code"], "license_required");
}

#[tokio::test]
async fn grace_allows_premium_writes_and_expiry_rejects_them() {
    let mut grace = licensed(&["custom_roles"], json!({}));
    grace.grace = true;
    grace.expires_at = Some(Utc::now() - Duration::days(1));
    grace.days_remaining = Some(-1);
    grace.grace_ends_at = Some(Utc::now() + Duration::days(5));
    let Some(grace_app) = test_app_with_license(true, Some(grace)).await else {
        return;
    };
    let (status, body) = send(&grace_app.router, get("/api/v1/license")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["license"]["grace"], true);
    assert!(body["license"]["graceEndsAt"].is_string());
    assert!(body["license"]["daysRemaining"].as_i64().unwrap() <= 0);
    let (status, body) = send(
        &grace_app.router,
        post(
            "/api/v1/roles",
            json!({ "name": format!("grace-role-{}", Uuid::now_v7()), "permissions": ["agents:read"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let Some(expired_app) = test_app_with_license(true, Some(expired_license())).await else {
        return;
    };
    let (status, body) = send(&expired_app.router, get("/api/v1/license")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["license"]["valid"], false);
    assert_eq!(body["license"]["tier"], "community");
    assert_eq!(body["license"]["grace"], false);
    let (status, body) = send(
        &expired_app.router,
        post(
            "/api/v1/roles",
            json!({ "name": format!("expired-role-{}", Uuid::now_v7()), "permissions": ["agents:read"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["error"]["code"], "license_required");
}

#[tokio::test]
async fn license_phase_transitions_are_audited() {
    let Some(app) = test_app(true).await else {
        return;
    };
    for (from, to) in [("active", "grace"), ("grace", "expired")] {
        orbit_api::background::audit_license_transition(&app.state, from, to).await;
    }
    let (entries, _) = app
        .store
        .list_audit_logs(app.org_id, 10, None, Some("license.phase_changed"))
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].metadata["from"], "grace");
    assert_eq!(entries[0].metadata["to"], "expired");
    assert_eq!(entries[1].metadata["from"], "active");
    assert_eq!(entries[1].metadata["to"], "grace");
}
