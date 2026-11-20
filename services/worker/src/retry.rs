//! Bounded exponential backoff for the calls that move an execution's lifecycle.
//!
//! `running` and `complete` are idempotent on the API side (terminal states are never
//! overwritten), so retrying them is always safe.

use std::future::Future;
use std::time::Duration;

/// A non-success HTTP answer from the control plane.
#[derive(Debug)]
pub struct ApiStatusError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "worker API returned {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiStatusError {}

/// HTTP status of an API error, when the failure was an HTTP answer.
pub fn status_of(error: &anyhow::Error) -> Option<u16> {
    error.downcast_ref::<ApiStatusError>().map(|e| e.status)
}

/// Transport errors, timeouts, 5xx, 408 and 429 are worth retrying; other 4xx are
/// definitive answers (for example 409: the execution already ended).
pub fn is_transient(error: &anyhow::Error) -> bool {
    match status_of(error) {
        Some(status) => status >= 500 || status == 408 || status == 429,
        None => true,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Total attempts, including the first.
    pub attempts: u32,
    pub base: Duration,
    pub max_delay: Duration,
}

impl RetryPolicy {
    /// Lifecycle reports (`running`, `complete`): outlast an API restart or rolling
    /// deploy, about 90 s in total before giving up and leaving it to the control-plane
    /// reaper. Delays: 0.5, 1, 2, 4, 8, 16, 30, 30 s (each +/-25% jitter).
    pub const fn lifecycle() -> Self {
        Self {
            attempts: 9,
            base: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }

    /// Event posts: a short bounded retry (at most 4 attempts, delays capped at 2 s) that
    /// rides out an API blip without letting one dead endpoint stall the event stream.
    pub fn for_events(&self) -> Self {
        Self {
            attempts: self.attempts.min(4),
            base: self.base,
            max_delay: self.max_delay.min(Duration::from_secs(2)),
        }
    }

    /// Delay before retry number `retry` (1-based): base * 2^(retry-1), capped, with
    /// +/-25% jitter so a fleet of workers does not retry in lockstep.
    fn delay(&self, retry: u32) -> Duration {
        let exp = self
            .base
            .saturating_mul(1u32 << (retry - 1).min(16))
            .min(self.max_delay);
        let jitter = (uuid::Uuid::new_v4().as_u128() % 501) as f64 / 1000.0; // 0.0..=0.5
        exp.mul_f64(0.75 + jitter)
    }

    pub async fn run<T, F, Fut>(&self, what: &str, mut op: F) -> anyhow::Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        let mut attempt = 1;
        loop {
            match op().await {
                Ok(value) => return Ok(value),
                Err(error) if attempt < self.attempts && is_transient(&error) => {
                    let delay = self.delay(attempt);
                    tracing::warn!(%error, attempt, ?delay, "{what} failed; retrying");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }
}
