use futures::StreamExt;
use orbit_contracts::{OrbitEvent, SubjectFilter};
use orbit_events::{EventBus, NatsEventBus};

/// Requires a running NATS server with JetStream enabled:
///   ORBIT_TEST_NATS_URL=nats://localhost:4222 cargo test -p orbit-events -- --ignored
#[tokio::test]
#[ignore]
async fn nats_publish_subscribe_roundtrip() {
    let url =
        std::env::var("ORBIT_TEST_NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let bus = NatsEventBus::connect(&url, "ORBIT").await.expect("connect");
    let mut stream = bus
        .subscribe(SubjectFilter::new("orbit.test.>"))
        .await
        .expect("subscribe");

    let event = OrbitEvent::new(
        "orbit.test.integration",
        "orbit-test",
        serde_json::json!({ "ok": true }),
    )
    .with_idempotency("test-key");
    bus.publish(event.clone()).await.expect("publish");

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .expect("timeout")
        .expect("stream item")
        .expect("event");
    assert_eq!(received.id, event.id);
    assert_eq!(received.subject, "orbit.test.integration");
    bus.health().await.expect("healthy");
}

/// The retained log is capped, an uncapped stream from an older version is brought in
/// line on connect, and a republished event is stored once.
///
/// Recreates the `ORBIT` stream, so use a throwaway server and `--test-threads=1`.
#[tokio::test]
#[ignore]
async fn stream_is_capped_and_republished_events_are_stored_once() {
    let url =
        std::env::var("ORBIT_TEST_NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let client = async_nats::connect(&url).await.expect("connect");
    let js = async_nats::jetstream::new(client);

    // What an older ORBIT created: no size cap, no duplicate window of ours.
    let _ = js.delete_stream("ORBIT").await;
    js.create_stream(async_nats::jetstream::stream::Config {
        name: "ORBIT".into(),
        subjects: vec!["orbit.>".to_string()],
        max_age: std::time::Duration::from_secs(7 * 24 * 3600),
        ..Default::default()
    })
    .await
    .expect("legacy stream");

    let bus = NatsEventBus::connect_with_limit(&url, "ORBIT", 4 * 1024 * 1024)
        .await
        .expect("connect with limit");
    let mut stream = js.get_stream("ORBIT").await.expect("stream");
    let info = stream.info().await.expect("info");
    assert_eq!(info.config.max_bytes, 4 * 1024 * 1024);
    assert_eq!(
        info.config.duplicate_window,
        std::time::Duration::from_secs(120)
    );

    let before = info.state.messages;
    let event = OrbitEvent::new(
        "orbit.test.dedupe",
        "orbit-test",
        serde_json::json!({ "n": 1 }),
    )
    .with_idempotency(format!("dedupe-{}", uuid::Uuid::now_v7()));
    bus.publish(event.clone()).await.expect("publish");
    bus.publish(event).await.expect("republish");
    // The ack is confirmed in the background; give the server a moment.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let after = stream.info().await.expect("info").state.messages;
    assert_eq!(after - before, 1, "the republished event was stored twice");

    // An event without an idempotency key is never deduplicated.
    for _ in 0..2 {
        bus.publish(OrbitEvent::new(
            "orbit.test.plain",
            "orbit-test",
            serde_json::json!({}),
        ))
        .await
        .expect("publish");
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let last = stream.info().await.expect("info").state.messages;
    assert_eq!(last - after, 2);
}
