//! Mock-server coverage for the Telegram output delivery adapter (#268).
//! These tests exercise the real HTTP path without a database or a live bot token.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::Json;
use orbit_api::handlers::outputs::{send_telegram, TelegramEndpoint};
use orbit_contracts::{TelegramTarget, TELEGRAM_TEXT_LIMIT};
use orbit_security::OutboundPolicy;
use serde_json::{json, Value};
use uuid::Uuid;

const TOKEN: &str = "123456:TEST-BOT-TOKEN";

#[derive(Clone)]
struct MockTelegram {
    requests: Arc<Mutex<Vec<(String, Value, HeaderMap)>>>,
    status: Arc<AtomicU16>,
    description: Arc<Mutex<Option<String>>>,
}

impl MockTelegram {
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn bodies(&self) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(_, body, _)| body.clone())
            .collect()
    }
}

async fn start_mock(status: u16, description: Option<&str>) -> (String, MockTelegram) {
    let mock = MockTelegram {
        requests: Arc::new(Mutex::new(Vec::new())),
        status: Arc::new(AtomicU16::new(status)),
        description: Arc::new(Mutex::new(description.map(str::to_string))),
    };
    async fn handle(
        State(mock): State<MockTelegram>,
        uri: Uri,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        mock.requests
            .lock()
            .unwrap()
            .push((uri.to_string(), body, headers));
        let status = StatusCode::from_u16(mock.status.load(Ordering::SeqCst)).unwrap();
        let payload = if status.is_success() {
            json!({ "ok": true })
        } else {
            json!({
                "ok": false,
                "error_code": status.as_u16(),
                "description": mock.description.lock().unwrap().clone().unwrap_or_default(),
            })
        };
        (status, Json(payload))
    }
    let router = axum::Router::new()
        .fallback(handle)
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (base_url, mock)
}

fn endpoint(base_url: String) -> TelegramEndpoint {
    TelegramEndpoint::new(base_url, OutboundPolicy::new(false, "127.0.0.1"))
}

#[tokio::test]
async fn telegram_success_posts_official_payload_with_topic_and_delivery_footer() {
    let (base_url, mock) = start_mock(200, None).await;
    let target = TelegramTarget::parse("-1001234567890", Some("42")).unwrap();
    let delivery_id = Uuid::now_v7();

    send_telegram(
        &endpoint(base_url),
        TOKEN,
        &target,
        "run finished",
        Some(delivery_id),
    )
    .await
    .expect("telegram send succeeds");

    assert_eq!(mock.count(), 1);
    let (path, body, headers) = mock.requests.lock().unwrap().first().unwrap().clone();
    assert_eq!(path, format!("/bot{TOKEN}/sendMessage"));
    assert_eq!(body["chat_id"], "-1001234567890");
    assert_eq!(body["message_thread_id"], "42");
    assert_eq!(body["disable_web_page_preview"], true);
    assert!(body["text"]
        .as_str()
        .unwrap()
        .starts_with("run finished\n\n[ORBIT delivery "));
    assert!(body["text"]
        .as_str()
        .unwrap()
        .contains(&delivery_id.to_string()));
    assert_eq!(headers["idempotency-key"], format!("{delivery_id}-0"));
    assert_eq!(headers["x-orbit-delivery-id"], delivery_id.to_string());
}

#[tokio::test]
async fn telegram_auth_failure_is_actionable_and_token_free() {
    let (base_url, mock) = start_mock(401, Some("Unauthorized")).await;
    let target = TelegramTarget::parse("12345", None).unwrap();

    let error = send_telegram(&endpoint(base_url), TOKEN, &target, "hello", None)
        .await
        .expect_err("401 fails");

    assert_eq!(error.status, Some(401));
    assert!(!error.retryable);
    assert!(error.message.contains("bot token or chat access invalid"));
    assert!(error.message.contains("Unauthorized"));
    assert!(!error.message.contains(TOKEN));
    assert_eq!(mock.count(), 1);
}

#[tokio::test]
async fn telegram_chat_not_found_includes_setup_hint() {
    let (base_url, _mock) = start_mock(400, Some("Chat not found")).await;
    let target = TelegramTarget::parse("-1009999999999", None).unwrap();

    let error = send_telegram(&endpoint(base_url), TOKEN, &target, "hello", None)
        .await
        .expect_err("400 fails");

    assert_eq!(error.status, Some(400));
    assert!(!error.retryable);
    assert!(error.message.contains("Chat not found"));
    assert!(error
        .message
        .contains("Add the bot to the target group/channel"));
    assert!(!error.message.contains(TOKEN));
}

#[tokio::test]
async fn telegram_override_still_passes_the_outbound_guard() {
    let (base_url, mock) = start_mock(200, None).await;
    let target = TelegramTarget::parse("-1001234567890", None).unwrap();
    let blocked = TelegramEndpoint::new(base_url, OutboundPolicy::default());

    let error = send_telegram(&blocked, TOKEN, &target, "hello", None)
        .await
        .expect_err("loopback is blocked by default");

    assert!(error.message.contains("outbound policy"));
    assert!(!error.message.contains(TOKEN));
    assert_eq!(mock.count(), 0, "no request leaves after a guard refusal");
}

#[tokio::test]
async fn telegram_long_text_is_split_below_the_api_limit() {
    let (base_url, mock) = start_mock(200, None).await;
    let target = TelegramTarget::parse("-1001234567890", None).unwrap();
    let text = "a line of build output that must be delivered safely\n".repeat(400);
    let delivery_id = Uuid::now_v7();

    send_telegram(
        &endpoint(base_url),
        TOKEN,
        &target,
        &text,
        Some(delivery_id),
    )
    .await
    .expect("long text sends in parts");

    let bodies = mock.bodies();
    assert!(bodies.len() > 1, "long text must be split");
    let mut keys = std::collections::HashSet::new();
    for (index, body) in bodies.iter().enumerate() {
        let part = body["text"].as_str().unwrap();
        assert!(part.chars().count() <= TELEGRAM_TEXT_LIMIT);
        assert!(part.contains(&delivery_id.to_string()));
        assert!(part.contains(&format!("part {}/{}", index + 1, bodies.len())));
        keys.insert(
            mock.requests.lock().unwrap()[index]
                .2
                .get("idempotency-key")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(keys.len(), bodies.len(), "each part has a unique key");
}
