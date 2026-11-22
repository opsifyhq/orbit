//! Basic per-IP rate limiting for unauthenticated HTTP routes.
//!
//! The limiter is deliberately small and local to the API process: it bounds how much
//! work an anonymous caller can trigger (in particular OIDC discovery/JWKS refreshes)
//! without adding another dependency or durable state. Buckets use a fixed 60-second
//! window, are keyed by the TCP peer address, and are pruned so a caller rotating source
//! addresses cannot grow memory without bound.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header::RETRY_AFTER, HeaderValue},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::error::ApiError;

const WINDOW: Duration = Duration::from_secs(60);
/// Cap on tracked source addresses. Beyond this, expired buckets are dropped first and
/// then the oldest buckets are evicted; limits may reset for evicted IPs, which is an
/// acceptable trade-off against unbounded memory on a public endpoint.
const MAX_TRACKED_IPS: usize = 10_000;

#[derive(Debug)]
pub struct RateLimiter {
    limit: u32,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
    rejected: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    started: Instant,
    count: u32,
}

impl RateLimiter {
    pub fn new(limit_per_minute: u32) -> Self {
        Self {
            limit: limit_per_minute.max(1),
            buckets: Mutex::new(HashMap::new()),
            rejected: AtomicU64::new(0),
        }
    }

    /// Returns the number of seconds to wait when the caller is over its budget.
    pub fn check(&self, ip: IpAddr) -> Option<u64> {
        let wait = self.check_at(ip, Instant::now());
        if wait.is_some() {
            self.rejected.fetch_add(1, Ordering::Relaxed);
        }
        wait
    }

    /// Requests rejected since start (exported as `orbit_rate_limited_total`).
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    fn check_at(&self, ip: IpAddr, now: Instant) -> Option<u64> {
        let mut buckets = self.lock();
        if buckets.len() >= MAX_TRACKED_IPS {
            prune(&mut buckets, now, WINDOW);
        }

        let bucket = buckets.entry(ip).or_insert_with(|| Bucket {
            started: now,
            count: 0,
        });
        if elapsed_since(now, bucket.started) >= WINDOW {
            bucket.started = now;
            bucket.count = 0;
        }
        if bucket.count >= self.limit {
            let remaining = WINDOW
                .checked_sub(elapsed_since(now, bucket.started))
                .unwrap_or_default();
            return Some(remaining.as_secs().max(1));
        }
        bucket.count += 1;
        None
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<IpAddr, Bucket>> {
        self.buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn elapsed_since(now: Instant, started: Instant) -> Duration {
    now.checked_duration_since(started).unwrap_or_default()
}

fn prune(buckets: &mut HashMap<IpAddr, Bucket>, now: Instant, window: Duration) {
    buckets.retain(|_, bucket| elapsed_since(now, bucket.started) < window);
    if buckets.len() >= MAX_TRACKED_IPS {
        let mut oldest: Vec<(IpAddr, Instant)> = buckets
            .iter()
            .map(|(ip, bucket)| (*ip, bucket.started))
            .collect();
        oldest.sort_by_key(|(_, started)| *started);
        let evict = oldest.len() - MAX_TRACKED_IPS + 1;
        for (ip, _) in oldest.into_iter().take(evict) {
            buckets.remove(&ip);
        }
    }
}

/// Axum middleware applied to the unauthenticated/public router only. Authenticated
/// and worker-protocol routes keep their existing behaviour, including dev anonymous
/// mode. The request id layer already ran at this point, so a rejection carries it.
pub async fn enforce(
    State(limiter): State<Arc<RateLimiter>>,
    request: Request,
    next: Next,
) -> Response {
    let ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    let Some(retry_after) = limiter.check(ip) else {
        return next.run(request).await;
    };

    let mut response = ApiError::rate_limited(format!(
        "too many unauthenticated requests; retry in {retry_after} seconds"
    ))
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(octet: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, octet])
    }

    #[test]
    fn rejects_above_the_limit_until_the_window_expires() {
        let limiter = RateLimiter::new(2);
        let now = Instant::now();
        assert_eq!(limiter.check_at(ip(1), now), None);
        assert_eq!(limiter.check_at(ip(1), now), None);
        assert_eq!(limiter.check_at(ip(1), now), Some(60));
        assert_eq!(limiter.check_at(ip(1), now + WINDOW), None);
    }

    #[test]
    fn buckets_are_independent_per_ip() {
        let limiter = RateLimiter::new(1);
        let now = Instant::now();
        assert_eq!(limiter.check_at(ip(1), now), None);
        assert_eq!(limiter.check_at(ip(1), now), Some(60));
        assert_eq!(limiter.check_at(ip(2), now), None);
    }

    #[test]
    fn a_zero_limit_cannot_disable_the_limiter() {
        let limiter = RateLimiter::new(0);
        let now = Instant::now();
        assert_eq!(limiter.check_at(ip(1), now), None);
        assert_eq!(limiter.check_at(ip(1), now), Some(60));
    }
}
