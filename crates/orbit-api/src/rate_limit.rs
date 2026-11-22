//! Basic per-client rate limiting for unauthenticated HTTP routes.
//!
//! The limiter is deliberately small and local to the API process: it bounds how much
//! work an anonymous caller can trigger (in particular OIDC discovery/JWKS refreshes)
//! without adding another dependency or durable state. Buckets use a fixed 60-second
//! window and are pruned so a caller rotating source addresses cannot grow memory
//! without bound.
//!
//! Buckets are keyed by the client address. That is the TCP peer, unless the peer is one
//! of the configured trusted proxies (`ORBIT_TRUSTED_PROXIES`): then it is the right-most
//! `X-Forwarded-For` hop that is not itself a trusted proxy. Behind the web proxy every
//! request otherwise comes from the same peer, and one client could use up the budget
//! of all of them (#302). IPv6 clients are keyed by their /64, because a single host
//! usually controls a whole /64.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex, MutexGuard},
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
    trusted_proxies: Vec<TrustedProxy>,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

/// An address range whose `X-Forwarded-For` header is believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedProxy {
    network: IpAddr,
    prefix: u8,
}

impl TrustedProxy {
    /// Parse `10.0.0.0/8`, `fd00::/8` or a single address.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        let (address, prefix) = match raw.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (raw, None),
        };
        let network = normalize(address.parse::<IpAddr>().ok()?, false);
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.parse::<u8>().ok().filter(|bits| *bits <= max)?,
            None => max,
        };
        Some(Self { network, prefix })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                prefix_eq(&net.octets(), &ip.octets(), self.prefix)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                prefix_eq(&net.octets(), &ip.octets(), self.prefix)
            }
            _ => false,
        }
    }
}

/// Parse a comma-separated list of trusted proxies; invalid entries are returned
/// separately so the caller can report them.
pub fn parse_trusted_proxies(raw: &str) -> (Vec<TrustedProxy>, Vec<String>) {
    let mut proxies = Vec::new();
    let mut invalid = Vec::new();
    for entry in raw
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        match TrustedProxy::parse(entry) {
            Some(proxy) => proxies.push(proxy),
            None => invalid.push(entry.to_string()),
        }
    }
    (proxies, invalid)
}

fn prefix_eq(a: &[u8], b: &[u8], bits: u8) -> bool {
    let full = (bits / 8) as usize;
    if a[..full] != b[..full] {
        return false;
    }
    let rem = bits % 8;
    if rem == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - rem);
    (a[full] & mask) == (b[full] & mask)
}

/// IPv4-mapped IPv6 becomes IPv4; with `per_network`, IPv6 is reduced to its /64.
fn normalize(ip: IpAddr, per_network: bool) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return IpAddr::V4(v4);
            }
            if !per_network {
                return ip;
            }
            let mut octets = v6.octets();
            octets[8..].fill(0);
            IpAddr::V6(std::net::Ipv6Addr::from(octets))
        }
        IpAddr::V4(_) => ip,
    }
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
            trusted_proxies: Vec::new(),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Believe `X-Forwarded-For` from these peers (the web proxy, the ingress controller).
    pub fn with_trusted_proxies(mut self, proxies: Vec<TrustedProxy>) -> Self {
        self.trusted_proxies = proxies;
        self
    }

    fn is_trusted(&self, ip: IpAddr) -> bool {
        self.trusted_proxies.iter().any(|proxy| proxy.contains(ip))
    }

    /// The address a request is counted against (see the module documentation).
    pub fn client_key(&self, peer: IpAddr, forwarded_for: Option<&str>) -> IpAddr {
        let peer = normalize(peer, false);
        if !self.is_trusted(peer) {
            return normalize(peer, true);
        }
        let hops: Vec<&str> = forwarded_for
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|hop| !hop.is_empty())
            .collect();
        let mut nearest = peer;
        for hop in hops.iter().rev() {
            // An entry we cannot read ends the walk: everything to its left is unverified.
            let Ok(ip) = hop.parse::<IpAddr>() else {
                break;
            };
            let ip = normalize(ip, false);
            if !self.is_trusted(ip) {
                return normalize(ip, true);
            }
            nearest = ip;
        }
        // Every hop is a trusted proxy (for example a client inside the trusted ranges):
        // count the request against the hop closest to the client that we could read.
        normalize(nearest, true)
    }

    /// Returns the number of seconds to wait when the caller is over its budget.
    pub fn check(&self, ip: IpAddr) -> Option<u64> {
        self.check_at(ip, Instant::now())
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
        // Evict down to 90% of the cap, so a flood of live buckets triggers this sort once
        // per few thousand new addresses instead of on every request (#302).
        let mut oldest: Vec<(IpAddr, Instant)> = buckets
            .iter()
            .map(|(ip, bucket)| (*ip, bucket.started))
            .collect();
        oldest.sort_by_key(|(_, started)| *started);
        let evict = oldest.len() - MAX_TRACKED_IPS * 9 / 10;
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
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    // Several X-Forwarded-For headers are one list, in order.
    let forwarded_for = request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(",");
    let ip = limiter.client_key(
        peer,
        (!forwarded_for.is_empty()).then_some(forwarded_for.as_str()),
    );

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

    fn behind(proxies: &str) -> RateLimiter {
        RateLimiter::new(10).with_trusted_proxies(parse_trusted_proxies(proxies).0)
    }

    #[test]
    fn forwarded_for_is_used_only_from_trusted_proxies() {
        let web_pod: IpAddr = "10.1.2.3".parse().unwrap();
        let limiter = behind("10.0.0.0/8");
        // The right-most hop that is not a trusted proxy is the client.
        assert_eq!(
            limiter.client_key(web_pod, Some("198.51.100.7, 10.1.0.9")),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        // A client cannot pick its key by sending its own header through a proxy that
        // appends: the appended (real) address wins.
        assert_eq!(
            limiter.client_key(web_pod, Some("6.6.6.6, 198.51.100.7")),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        // From an untrusted peer the header is ignored.
        let direct: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(limiter.client_key(direct, Some("198.51.100.7")), direct);
        // No header, or an unreadable hop: fall back to the nearest known address.
        assert_eq!(limiter.client_key(web_pod, None), web_pod);
        assert_eq!(limiter.client_key(web_pod, Some("garbage")), web_pod);
        // Without trusted proxies nothing changes.
        assert_eq!(
            RateLimiter::new(10).client_key(web_pod, Some("198.51.100.7")),
            web_pod
        );
    }

    #[test]
    fn ipv6_clients_share_a_bucket_per_64() {
        let limiter = RateLimiter::new(1);
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let other: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(limiter.client_key(a, None), limiter.client_key(b, None));
        assert_ne!(limiter.client_key(a, None), limiter.client_key(other, None));
        let mapped: IpAddr = "::ffff:198.51.100.7".parse().unwrap();
        assert_eq!(
            limiter.client_key(mapped, None),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn trusted_proxy_entries_are_validated() {
        let (proxies, invalid) =
            parse_trusted_proxies("10.0.0.0/8, fd00::/8,127.0.0.1, nope, 10.0.0.0/40");
        assert_eq!(proxies.len(), 3);
        assert_eq!(invalid, vec!["nope".to_string(), "10.0.0.0/40".to_string()]);
    }

    #[test]
    fn a_full_table_is_pruned_in_batches() {
        let limiter = RateLimiter::new(1);
        let now = Instant::now();
        for n in 0..MAX_TRACKED_IPS as u32 + 1 {
            let ip = IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + n));
            limiter.check_at(ip, now);
        }
        let len = limiter.lock().len();
        assert!(len <= MAX_TRACKED_IPS * 9 / 10 + 1, "{len}");
    }

    #[test]
    fn a_zero_limit_cannot_disable_the_limiter() {
        let limiter = RateLimiter::new(0);
        let now = Instant::now();
        assert_eq!(limiter.check_at(ip(1), now), None);
        assert_eq!(limiter.check_at(ip(1), now), Some(60));
    }
}
