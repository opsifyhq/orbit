use std::time::Duration;

use async_nats::jetstream;
use futures::StreamExt;

use orbit_contracts::{OrbitEvent, SubjectFilter};

use crate::{EventBus, EventBusError, EventStream};

/// NATS JetStream transport.
///
/// Published events are written to a JetStream stream (`ORBIT` by default). `publish`
/// returns an error when the message cannot be handed to the client within
/// [`DEFAULT_PUBLISH_ACK_TIMEOUT`]; the server acknowledgement is then confirmed in the
/// background (same timeout) and a missing ack or error is logged, so a slow JetStream
/// never adds latency to request paths.
/// Live consumers use a plain core NATS subscription for low latency. There are currently
/// **no durable pull consumers and no replay**: the stream is a retained log only.
/// Execution work never depends on the bus: the worker polls the durable PostgreSQL queue
/// as the source of truth, and callers treat publish failures as non-fatal.
pub struct NatsEventBus {
    client: async_nats::Client,
    js: jetstream::Context,
    stream_name: String,
    ack_timeout: Duration,
}

/// Timeout for handing a message to the client, and for the background ack confirmation.
pub const DEFAULT_PUBLISH_ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// Size cap of the retained log. Every execution event (output deltas, tool payloads)
/// lands in the stream and nothing consumes it durably, so without a cap it grows for
/// the whole `max_age` window. When full, JetStream drops the oldest messages.
pub const DEFAULT_STREAM_MAX_BYTES: i64 = 1024 * 1024 * 1024;
const STREAM_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// A republished event (same idempotency key) within this window is stored once.
const STREAM_DUPLICATE_WINDOW: Duration = Duration::from_secs(120);

fn stream_config(name: &str, max_bytes: i64) -> jetstream::stream::Config {
    jetstream::stream::Config {
        name: name.to_string(),
        subjects: vec!["orbit.>".to_string()],
        max_age: STREAM_MAX_AGE,
        max_bytes,
        duplicate_window: STREAM_DUPLICATE_WINDOW,
        ..Default::default()
    }
}

/// `ORBIT_NATS_STREAM_MAX_BYTES`, or the default. `-1` removes the cap.
fn configured_stream_max_bytes() -> i64 {
    std::env::var("ORBIT_NATS_STREAM_MAX_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|bytes| *bytes == -1 || *bytes > 0)
        .unwrap_or(DEFAULT_STREAM_MAX_BYTES)
}

/// Drive a JetStream publish (send + ack) to completion within `timeout`, mapping failures
/// and timeouts to [`EventBusError::Unavailable`]. Generic so the mapping is unit-testable
/// without a NATS server.
pub(crate) async fn confirm_publish<F>(fut: F, timeout: Duration) -> Result<(), EventBusError>
where
    F: std::future::Future<Output = Result<(), String>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => Err(EventBusError::Unavailable(format!(
            "NATS publish failed: {message}"
        ))),
        Err(_) => Err(EventBusError::Unavailable(format!(
            "NATS publish not acknowledged within {}ms",
            timeout.as_millis()
        ))),
    }
}

impl NatsEventBus {
    pub async fn connect(url: &str, stream_name: &str) -> Result<Self, EventBusError> {
        Self::connect_with_limit(url, stream_name, configured_stream_max_bytes()).await
    }

    /// Connect and make sure the stream exists with the given size cap.
    pub async fn connect_with_limit(
        url: &str,
        stream_name: &str,
        max_bytes: i64,
    ) -> Result<Self, EventBusError> {
        let client = async_nats::connect(url)
            .await
            .map_err(|e| EventBusError::Unavailable(format!("NATS connect failed: {e}")))?;
        let js = jetstream::new(client.clone());
        let desired = stream_config(stream_name, max_bytes);
        let mut stream = js
            .get_or_create_stream(desired.clone())
            .await
            .map_err(|e| {
                EventBusError::Unavailable(format!(
                    "could not create JetStream stream '{stream_name}': {e}"
                ))
            })?;
        // A stream created by an older version has no cap and no duplicate window:
        // `get_or_create` returns it as it is, so bring it in line here. Not fatal: the
        // bus is an accelerator and works with the old limits too.
        match stream.info().await {
            Ok(info)
                if info.config.max_bytes != desired.max_bytes
                    || info.config.duplicate_window != desired.duplicate_window =>
            {
                if let Err(error) = js.update_stream(desired).await {
                    tracing::warn!(stream = %stream_name, %error, "could not apply JetStream stream limits");
                } else {
                    tracing::info!(stream = %stream_name, max_bytes, "applied JetStream stream limits");
                }
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(stream = %stream_name, %error, "could not read JetStream stream info")
            }
        }
        Ok(Self {
            client,
            js,
            stream_name: stream_name.to_string(),
            ack_timeout: DEFAULT_PUBLISH_ACK_TIMEOUT,
        })
    }

    /// Override how long `publish` waits for the JetStream ack.
    pub fn with_ack_timeout(mut self, timeout: Duration) -> Self {
        self.ack_timeout = timeout;
        self
    }

    pub fn stream_name(&self) -> &str {
        &self.stream_name
    }
}

#[async_trait::async_trait]
impl EventBus for NatsEventBus {
    fn driver(&self) -> &'static str {
        "nats"
    }

    async fn publish(&self, event: OrbitEvent) -> Result<(), EventBusError> {
        let payload = event
            .encode()
            .map_err(|e| EventBusError::Protocol(format!("could not encode event: {e}")))?;
        let js = self.js.clone();
        let subject = event.subject.clone();
        // JetStream stores a message id once per duplicate window: a republished event
        // (a retry after a lost ack) does not become a second copy.
        let mut headers = async_nats::HeaderMap::new();
        if let Some(key) = event
            .idempotency_key
            .as_deref()
            .filter(|key| !key.is_empty())
        {
            headers.insert(async_nats::header::NATS_MESSAGE_ID, key);
        }
        // Hand the message to the client within the timeout and report failures to the
        // caller. The JetStream ack is confirmed in the background: publishes happen on
        // request paths (e.g. worker execution events), and waiting for the ack inline
        // would add up to `ack_timeout` of latency per event whenever JetStream is slow.
        // The bus is an accelerator (AGENTS.md rule 5), so a missing ack is logged, not
        // propagated.
        let mut pending_ack = None;
        confirm_publish(
            async {
                let ack = js
                    .publish_with_headers(subject.clone(), headers, payload.into())
                    .await
                    .map_err(|e| e.to_string())?;
                pending_ack = Some(ack);
                Ok(())
            },
            self.ack_timeout,
        )
        .await?;
        if let Some(ack) = pending_ack {
            let timeout = self.ack_timeout;
            tokio::spawn(async move {
                if let Err(error) = confirm_publish(
                    async { ack.await.map(|_| ()).map_err(|e| e.to_string()) },
                    timeout,
                )
                .await
                {
                    tracing::warn!(%subject, %error, "JetStream did not confirm event");
                }
            });
        }
        Ok(())
    }

    async fn subscribe(&self, filter: SubjectFilter) -> Result<EventStream, EventBusError> {
        let mut subscriber = self
            .client
            .subscribe(filter.as_str().to_string())
            .await
            .map_err(|e| EventBusError::Unavailable(format!("NATS subscribe failed: {e}")))?;
        let pattern = filter.as_str().to_string();
        let stream = async_stream::stream! {
            while let Some(message) = subscriber.next().await {
                match OrbitEvent::decode(&message.payload) {
                    Ok(event) => yield Ok(event),
                    Err(err) => yield Err(EventBusError::Protocol(format!(
                        "invalid event payload on {}: {err}",
                        message.subject
                    ))),
                }
            }
            tracing::debug!(pattern = %pattern, "NATS subscription ended");
        };
        Ok(Box::pin(stream))
    }

    async fn health(&self) -> Result<(), EventBusError> {
        let mut account = self
            .js
            .get_stream(&self.stream_name)
            .await
            .map_err(|e| EventBusError::Unavailable(format!("NATS stream missing: {e}")))?;
        let _info = account
            .info()
            .await
            .map_err(|e| EventBusError::Unavailable(format!("NATS stream info failed: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ack_success_is_ok() {
        assert!(confirm_publish(async { Ok(()) }, Duration::from_secs(1))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn ack_error_maps_to_unavailable() {
        let err = confirm_publish(
            async { Err("no responders".into()) },
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(matches!(&err, EventBusError::Unavailable(m) if m.contains("no responders")));
    }

    #[tokio::test]
    async fn missing_ack_times_out_as_unavailable() {
        let err = confirm_publish(std::future::pending(), Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(matches!(&err, EventBusError::Unavailable(m) if m.contains("not acknowledged")));
    }
}
