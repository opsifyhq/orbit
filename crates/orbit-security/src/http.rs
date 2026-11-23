//! HTTP client helpers that honour custom CA certificates.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// Build a `reqwest::Client` that trusts the built-in roots **and** any CA bundle
/// pointed to by `SSL_CERT_FILE` / `REQUESTS_CA_BUNDLE` / `NODE_EXTRA_CA_CERTS`.
///
/// `reqwest` with `rustls-tls` uses webpki roots and does not read the system trust
/// store, so a private CA mounted by the Helm chart would otherwise be ignored. Bundles
/// may contain multiple certificates (root + intermediates); every certificate in the
/// file is added, not just the first one.
pub fn client_with_custom_ca(timeout: Duration) -> reqwest::Client {
    build_or_default(client_builder_with_custom_ca(timeout))
}

fn build_or_default(builder: reqwest::ClientBuilder) -> reqwest::Client {
    builder.build().unwrap_or_else(|error| {
        tracing::error!(%error, "failed to build HTTP client with custom CAs; using defaults");
        reqwest::Client::new()
    })
}

/// Same as [`client_with_custom_ca`] but returns the builder so callers can add policy.
pub fn client_builder_with_custom_ca(timeout: Duration) -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder().timeout(timeout);
    let mut seen = BTreeSet::new();
    let mut added = 0usize;

    for var in ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "NODE_EXTRA_CA_CERTS"] {
        let Ok(path) = std::env::var(var) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() || !seen.insert(path.to_string()) {
            continue;
        }
        let pem = match std::fs::read(path) {
            Ok(pem) => pem,
            Err(error) => {
                tracing::warn!(%error, path, variable = var, "failed to read CA bundle");
                continue;
            }
        };

        match reqwest::Certificate::from_pem_bundle(&pem) {
            Ok(certificates) => {
                for certificate in certificates {
                    builder = builder.add_root_certificate(certificate);
                    added += 1;
                }
            }
            Err(bundle_error) => {
                // Some bundles mix certificate blocks with comments or other PEM
                // material. Fall back to adding each certificate block individually so
                // one bad block does not discard the whole bundle.
                let mut any = false;
                for block in certificate_blocks(&pem) {
                    if let Ok(certificate) = reqwest::Certificate::from_pem(&block) {
                        builder = builder.add_root_certificate(certificate);
                        added += 1;
                        any = true;
                    }
                }
                if !any {
                    tracing::warn!(
                        error = %bundle_error,
                        path,
                        variable = var,
                        "CA bundle could not be parsed; no certificates added"
                    );
                }
            }
        }
    }

    if added > 0 {
        tracing::debug!(certificates = added, "loaded custom CA certificates");
    }

    builder
}

/// Extract every `-----BEGIN CERTIFICATE-----` … `-----END CERTIFICATE-----` block.
fn certificate_blocks(pem: &[u8]) -> Vec<Vec<u8>> {
    const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const END: &[u8] = b"-----END CERTIFICATE-----";
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some(start) = find(&pem[cursor..], BEGIN) {
        let start = cursor + start;
        let Some(end) = find(&pem[start..], END) else {
            break;
        };
        let end = start + end + END.len();
        blocks.push(pem[start..end].to_vec());
        cursor = end;
    }
    blocks
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---------------------------------------------------------------- outbound guard

/// Why an outbound request target was refused. The `Display` text is deliberately
/// generic (category only) so it can be returned to API callers without leaking
/// internal topology or resolver error strings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboundError {
    #[error("outbound URL is invalid or not http(s)")]
    Invalid,
    #[error("outbound host could not be resolved (dns)")]
    Dns,
    #[error(
        "outbound address is not permitted (loopback, link-local, metadata or reserved range); \
         see ORBIT_OUTBOUND_ALLOWED_HOSTS"
    )]
    Blocked,
}

impl OutboundError {
    /// Short machine category: `invalid`, `dns` or `blocked`.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Dns => "dns",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AllowEntry {
    Host(String),
    Cidr(IpAddr, u8),
}

impl AllowEntry {
    fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().to_ascii_lowercase();
        if raw.is_empty() {
            return None;
        }
        if let Some((ip, bits)) = raw.split_once('/') {
            let ip: IpAddr = ip.parse().ok()?;
            let bits: u8 = bits.parse().ok()?;
            let max = if ip.is_ipv4() { 32 } else { 128 };
            return (bits <= max).then_some(Self::Cidr(ip, bits));
        }
        if let Ok(ip) = raw.parse::<IpAddr>() {
            let bits = if ip.is_ipv4() { 32 } else { 128 };
            return Some(Self::Cidr(ip, bits));
        }
        Some(Self::Host(raw))
    }

    fn matches_host(&self, host: &str) -> bool {
        matches!(self, Self::Host(h) if h == &host.to_ascii_lowercase())
    }

    fn matches_ip(&self, ip: IpAddr) -> bool {
        let Self::Cidr(net, bits) = self else {
            return false;
        };
        match (net, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => prefix_eq(&n.octets(), &a.octets(), *bits),
            (IpAddr::V6(n), IpAddr::V6(a)) => prefix_eq(&n.octets(), &a.octets(), *bits),
            _ => false,
        }
    }
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

/// Policy for outbound requests whose target is operator/tenant supplied (provider
/// base URLs, output webhooks, work-source base URLs).
///
/// Defaults: private ranges (RFC 1918, CGNAT, IPv6 ULA) are **allowed** because
/// self-hosted model servers and GitLab instances normally live there; loopback,
/// link-local, cloud-metadata, unspecified, multicast and reserved addresses are
/// **blocked**. Configure with:
///
/// * `ORBIT_OUTBOUND_ALLOW_PRIVATE` (default `true`): set `false` to also block private ranges.
/// * `ORBIT_OUTBOUND_ALLOWED_HOSTS`: comma-separated hostnames, IPs or CIDRs that are exempt
///   (for example `localhost,10.0.0.0/8,ollama.internal`). This is the only way to permit
///   loopback or link-local targets. Cloud metadata endpoints can never be allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundPolicy {
    allow_private: bool,
    allowed: Vec<AllowEntry>,
}

impl Default for OutboundPolicy {
    fn default() -> Self {
        Self {
            allow_private: true,
            allowed: Vec::new(),
        }
    }
}

impl OutboundPolicy {
    pub fn new(allow_private: bool, allowed: &str) -> Self {
        Self {
            allow_private,
            allowed: allowed.split(',').filter_map(AllowEntry::parse).collect(),
        }
    }

    pub fn from_env() -> Self {
        let allow_private = std::env::var("ORBIT_OUTBOUND_ALLOW_PRIVATE")
            .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0" | "no"))
            .unwrap_or(true);
        let allowed = std::env::var("ORBIT_OUTBOUND_ALLOWED_HOSTS").unwrap_or_default();
        Self::new(allow_private, &allowed)
    }

    fn host_allowed(&self, host: &str) -> bool {
        self.allowed.iter().any(|entry| entry.matches_host(host))
    }

    /// Check a resolved address. `host_exempt` is true when the hostname was allowlisted.
    pub fn check_ip(&self, ip: IpAddr, host_exempt: bool) -> Result<(), OutboundError> {
        let ip = ip.to_canonical();
        if is_hard_blocked(ip) {
            return Err(OutboundError::Blocked);
        }
        let explicitly_allowed = host_exempt || self.allowed.iter().any(|e| e.matches_ip(ip));
        if explicitly_allowed {
            return Ok(());
        }
        if is_loopback_or_link_local(ip) {
            return Err(OutboundError::Blocked);
        }
        if !self.allow_private && is_private(ip) {
            return Err(OutboundError::Blocked);
        }
        Ok(())
    }
}

/// Addresses that can never be allowlisted: cloud metadata, unspecified, multicast,
/// broadcast and reserved space.
fn is_hard_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || o[0] == 0
                || o[0] >= 240
                || o == [169, 254, 169, 254]
                || o == [169, 254, 170, 2]
                || o == [100, 100, 100, 200]
        }
        IpAddr::V6(v6) => {
            v6.is_unspecified()
                || v6.is_multicast()
                || v6 == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x254)
        }
    }
}

fn is_loopback_or_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unicast_link_local(),
    }
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private() || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => v6.is_unique_local(),
    }
}

/// Resolve and vet the target of `url`. Returns the generic [`OutboundError`] category.
pub async fn check_outbound_url(url: &str, policy: &OutboundPolicy) -> Result<(), OutboundError> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| OutboundError::Invalid)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(OutboundError::Invalid);
    }
    let host = parsed.host_str().ok_or(OutboundError::Invalid)?;
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return policy.check_ip(ip, false);
    }
    let exempt = policy.host_allowed(host);
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| OutboundError::Dns)?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err(OutboundError::Dns);
    }
    addrs
        .iter()
        .try_for_each(|addr| policy.check_ip(addr.ip(), exempt))
}

/// DNS resolver that refuses to hand out blocked addresses, closing the gap between
/// the pre-flight check and the connection (DNS rebinding) and covering redirects.
struct GuardedResolver {
    policy: OutboundPolicy,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let policy = self.policy.clone();
        Box::pin(async move {
            let host = name.as_str().to_string();
            let exempt = policy.host_allowed(&host);
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|_| Box::new(OutboundError::Dns) as BoxError)?
                .collect();
            for addr in &addrs {
                policy
                    .check_ip(addr.ip(), exempt)
                    .map_err(|e| Box::new(e) as BoxError)?;
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Build a client for tenant-supplied URLs: custom CA support, address policy enforced
/// at DNS resolution, and redirects followed only within the same host (never to a
/// different host, where credentials would otherwise travel).
pub fn guarded_client_with_policy(timeout: Duration, policy: OutboundPolicy) -> reqwest::Client {
    build_or_default(guard_client_builder(
        client_builder_with_custom_ca(timeout),
        policy,
    ))
}

/// Apply the outbound guard (refusing resolver, same-host redirects only) to a client
/// builder that needs its own TLS settings, such as a Kubernetes client with a cluster CA
/// or client certificate. IP-literal hosts never reach the resolver: pair this with
/// [`check_outbound_url`] before each request.
pub fn guard_client_builder(
    builder: reqwest::ClientBuilder,
    policy: OutboundPolicy,
) -> reqwest::ClientBuilder {
    let redirect = reqwest::redirect::Policy::custom(|attempt| {
        let same_host = attempt
            .previous()
            .last()
            .is_some_and(|prev| prev.host_str() == attempt.url().host_str());
        if attempt.previous().len() >= 3 || !same_host {
            attempt.stop()
        } else {
            attempt.follow()
        }
    });
    builder
        .redirect(redirect)
        .dns_resolver(std::sync::Arc::new(GuardedResolver { policy }))
}

/// [`guarded_client_with_policy`] using the policy from the environment.
pub fn guarded_client(timeout: Duration) -> reqwest::Client {
    guarded_client_with_policy(timeout, OutboundPolicy::from_env())
}

/// Classify a failed outbound request into a generic, non-leaky description.
pub fn describe_request_error(error: &reqwest::Error) -> String {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(err) = source {
        if let Some(outbound) = err.downcast_ref::<OutboundError>() {
            return format!("request refused ({})", outbound.category());
        }
        source = err.source();
    }
    if error.is_timeout() {
        "request failed (timeout)".into()
    } else if error.is_redirect() {
        "request failed (redirect to another host refused)".into()
    } else if error.is_connect() {
        let text = format!("{error:?}").to_ascii_lowercase();
        if text.contains("dns") || text.contains("resolve") || text.contains("lookup") {
            "request failed (dns)".into()
        } else if text.contains("certificate") || text.contains("tls") || text.contains("ssl") {
            "request failed (tls)".into()
        } else {
            "request failed (connect)".into()
        }
    } else {
        "request failed".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn default_policy_matrix() {
        let p = OutboundPolicy::default();
        for blocked in [
            "127.0.0.1",
            "127.8.9.1",
            "::1",
            "169.254.169.254",
            "169.254.10.10",
            "fe80::1",
            "0.0.0.0",
            "::",
            "224.0.0.1",
            "ff02::1",
            "255.255.255.255",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "fd00:ec2::254",
            "100.100.100.200",
            "240.0.0.1",
        ] {
            assert_eq!(
                p.check_ip(ip(blocked), false),
                Err(OutboundError::Blocked),
                "{blocked}"
            );
        }
        for allowed in [
            "10.1.2.3",
            "172.16.0.5",
            "192.168.1.201",
            "100.64.0.1",
            "fd12::1",
            "8.8.8.8",
            "2606:4700::1",
        ] {
            assert!(p.check_ip(ip(allowed), false).is_ok(), "{allowed}");
        }
    }

    #[test]
    fn strict_policy_blocks_private_ranges() {
        let p = OutboundPolicy::new(false, "");
        for blocked in [
            "10.0.0.1",
            "172.31.255.1",
            "192.168.0.1",
            "100.64.1.1",
            "fc00::1",
        ] {
            assert_eq!(
                p.check_ip(ip(blocked), false),
                Err(OutboundError::Blocked),
                "{blocked}"
            );
        }
        assert!(p.check_ip(ip("93.184.216.34"), false).is_ok());
    }

    #[test]
    fn allowlist_exempts_cidr_and_ip_but_never_metadata() {
        let p = OutboundPolicy::new(
            false,
            "10.0.0.0/8, 127.0.0.1 ,fe80::/10,169.254.0.0/16,ollama.internal",
        );
        assert!(p.check_ip(ip("10.9.9.9"), false).is_ok());
        assert!(p.check_ip(ip("127.0.0.1"), false).is_ok());
        assert!(p.check_ip(ip("fe80::5"), false).is_ok());
        assert!(p.check_ip(ip("169.254.1.1"), false).is_ok());
        assert_eq!(
            p.check_ip(ip("127.0.0.2"), false),
            Err(OutboundError::Blocked)
        );
        assert_eq!(
            p.check_ip(ip("192.168.0.1"), false),
            Err(OutboundError::Blocked)
        );
        // Metadata endpoints stay blocked even inside an allowlisted CIDR or host.
        assert_eq!(
            p.check_ip(ip("169.254.169.254"), false),
            Err(OutboundError::Blocked)
        );
        assert_eq!(
            p.check_ip(ip("169.254.169.254"), true),
            Err(OutboundError::Blocked)
        );
        assert!(p.host_allowed("Ollama.Internal"));
        assert!(p.check_ip(ip("127.0.0.5"), true).is_ok());
    }

    #[test]
    fn allow_entry_parsing() {
        assert_eq!(AllowEntry::parse("10.0.0.0/33"), None);
        assert_eq!(AllowEntry::parse(" "), None);
        let c24 = AllowEntry::parse("192.168.1.0/24").unwrap();
        assert!(c24.matches_ip(ip("192.168.1.77")));
        assert!(!c24.matches_ip(ip("192.168.2.1")));
        let c12 = AllowEntry::parse("10.0.0.0/12").unwrap();
        assert!(c12.matches_ip(ip("10.15.0.1")));
        assert!(!c12.matches_ip(ip("10.16.0.1")));
    }

    #[tokio::test]
    async fn url_check_rejects_literals_and_bad_schemes() {
        let p = OutboundPolicy::default();
        for url in [
            "http://169.254.169.254/latest/meta-data",
            "http://127.0.0.1:8080/x",
            "http://[::1]/x",
            "http://[fe80::1]/x",
            "http://0.0.0.0/",
        ] {
            assert_eq!(
                check_outbound_url(url, &p).await,
                Err(OutboundError::Blocked),
                "{url}"
            );
        }
        assert_eq!(
            check_outbound_url("ftp://example.com", &p).await,
            Err(OutboundError::Invalid)
        );
        assert_eq!(
            check_outbound_url("not a url", &p).await,
            Err(OutboundError::Invalid)
        );
        assert!(check_outbound_url("http://10.1.2.3:8080/v1", &p)
            .await
            .is_ok());
        let allow = OutboundPolicy::new(true, "127.0.0.1");
        assert!(check_outbound_url("http://127.0.0.1:9/", &allow)
            .await
            .is_ok());
        // `localhost` resolves to loopback and is blocked unless named in the allowlist.
        assert_eq!(
            check_outbound_url("http://localhost:9/", &p).await,
            Err(OutboundError::Blocked)
        );
        assert!(check_outbound_url(
            "http://localhost:9/",
            &OutboundPolicy::new(true, "localhost")
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn guarded_client_refuses_blocked_hostname_and_cross_host_redirect() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://other.invalid/x\r\nContent-Length: 0\r\n\r\n")
                    .await;
            }
        });
        // Allowlisted loopback: the 302 to another host is returned, not followed.
        let client = guarded_client_with_policy(
            Duration::from_secs(5),
            OutboundPolicy::new(true, "localhost"),
        );
        let response = client
            .get(format!("http://localhost:{port}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 302);
        // Not allowlisted: the resolver refuses loopback.
        let strict = guarded_client_with_policy(Duration::from_secs(5), OutboundPolicy::default());
        let error = strict
            .get(format!("http://localhost:{port}/"))
            .send()
            .await
            .unwrap_err();
        let text = describe_request_error(&error);
        assert!(text.contains("blocked"), "{text}");
    }

    #[test]
    fn extracts_multiple_certificate_blocks() {
        let pem =
            b"subject=/CN=one\n-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n\
-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----\n";
        let blocks = certificate_blocks(pem);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].ends_with(b"-----END CERTIFICATE-----"));
        assert!(blocks[1].starts_with(b"-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn ignores_unterminated_blocks() {
        let pem = b"-----BEGIN CERTIFICATE-----\nAAAA\n";
        assert!(certificate_blocks(pem).is_empty());
    }

    #[test]
    fn builds_a_client_without_a_configured_bundle() {
        // No env overrides in tests: the client must still build and be usable.
        let client = client_with_custom_ca(Duration::from_secs(1));
        assert!(client.get("http://127.0.0.1:1/").build().is_ok());
    }
}
