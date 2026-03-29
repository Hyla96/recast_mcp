//! SSRF protection — two-phase outbound URL validation.
//!
//! # Design
//!
//! Outgoing URLs are validated in two phases:
//!
//! - **Phase 1** ([`validate_url`]) — synchronous, zero I/O. Rejects bad
//!   schemes, blocked ports, well-known cloud-metadata hostnames, localhost
//!   aliases, and bare IP addresses that fall in any blocked CIDR range.
//!
//! - **Phase 2** ([`validate_url_with_dns`]) — Phase 1 plus async DNS
//!   resolution. Every IP address returned by the resolver is checked against
//!   all blocked CIDR ranges to prevent DNS rebinding attacks.
//!
//! # Environment variables
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `MCP_SSRF_DNS_TIMEOUT_SECS` | `5` | DNS resolution timeout in seconds. |

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use ipnetwork::{Ipv4Network, Ipv6Network};
use url::Url;

use crate::AppError;

// ── Blocked constants ────────────────────────────────────────────────────────

/// Ports never permitted in outgoing request URLs.
const BLOCKED_PORTS: &[u16] = &[
    22,   // SSH
    23,   // Telnet
    25,   // SMTP
    3306, // MySQL
    5432, // PostgreSQL
    6379, // Redis
];

/// Hostnames blocked by exact name (case-insensitive).
///
/// Covers cloud IMDS endpoints that would not resolve to RFC 1918 addresses
/// and could be reached even with IP-based blocking.
const BLOCKED_HOSTNAMES: &[&str] = &[
    "169.254.169.254",          // AWS / Azure / GCP IMDS
    "metadata.google.internal", // GCP metadata DNS alias
];

/// Loopback and localhost aliases blocked by hostname string.
const LOOPBACK_HOSTNAMES: &[&str] = &["localhost", "ip6-localhost", "ip6-loopback"];

// ── CIDR range helpers ────────────────────────────────────────────────────────

/// Returns all blocked IPv4 CIDR ranges.
///
/// All prefix lengths are within 0–32 and all addresses are the canonical
/// network addresses, so [`Ipv4Network::new`] is infallible for every entry.
/// `filter_map`/`.ok()` is used to satisfy the workspace no-`unwrap` lint
/// without a silent panic path.
fn blocked_ipv4_networks() -> Vec<Ipv4Network> {
    let specs: &[(Ipv4Addr, u8)] = &[
        (Ipv4Addr::new(10, 0, 0, 0), 8),      // RFC 1918 Class A
        (Ipv4Addr::new(172, 16, 0, 0), 12),    // RFC 1918 Class B
        (Ipv4Addr::new(192, 168, 0, 0), 16),   // RFC 1918 Class C
        (Ipv4Addr::new(127, 0, 0, 0), 8),      // Loopback (127/8)
        (Ipv4Addr::new(169, 254, 0, 0), 16),   // Link-local
        (Ipv4Addr::new(100, 64, 0, 0), 10),    // CGNAT — RFC 6598
        (Ipv4Addr::new(224, 0, 0, 0), 4),      // Multicast (Class D)
        (Ipv4Addr::new(240, 0, 0, 0), 4),      // Reserved (Class E)
        (Ipv4Addr::new(0, 0, 0, 0), 8),        // "This" network — RFC 1122
        (Ipv4Addr::new(192, 0, 2, 0), 24),     // TEST-NET-1 — RFC 5737
        (Ipv4Addr::new(198, 51, 100, 0), 24),  // TEST-NET-2 — RFC 5737
        (Ipv4Addr::new(203, 0, 113, 0), 24),   // TEST-NET-3 — RFC 5737
    ];
    specs
        .iter()
        .filter_map(|(addr, prefix)| Ipv4Network::new(*addr, *prefix).ok())
        .collect()
}

/// Returns all blocked IPv6 CIDR ranges.
fn blocked_ipv6_networks() -> Vec<Ipv6Network> {
    let specs: &[(Ipv6Addr, u8)] = &[
        // Loopback — ::1/128
        (Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), 128),
        // Link-local — fe80::/10
        (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
        // Unique Local Addresses — fc00::/7 (includes fc00::/8 and fd00::/8)
        (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
        // IPv4-mapped — ::ffff:0:0/96
        (Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0, 0), 96),
    ];
    specs
        .iter()
        .filter_map(|(addr, prefix)| Ipv6Network::new(*addr, *prefix).ok())
        .collect()
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns `true` if `ip` falls within any blocked network range.
///
/// See the module-level documentation for the full list of blocked ranges.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_ipv4_networks().iter().any(|net| net.contains(v4)),
        IpAddr::V6(v6) => blocked_ipv6_networks().iter().any(|net| net.contains(v6)),
    }
}

/// Phase 1 validation — synchronous, no I/O.
///
/// Checks, in order:
/// 1. Scheme must be `http` or `https`.
/// 2. Port (if explicit) must not be in the blocked port list.
/// 3. Host must be present.
/// 4. For domain hosts: must not be a blocked cloud-metadata or loopback name.
/// 5. For IP hosts (IPv4 or IPv6): must not fall in a blocked CIDR range.
///
/// Uses [`url::Url::host`] (structured form) for IP extraction so IPv6
/// addresses in bracket notation (`[::1]`) are handled correctly.
///
/// Returns `Ok(())` on success or `Err(AppError::SsrfBlocked { .. })` with a
/// human-readable `reason` on failure.
pub fn validate_url(url: &Url) -> Result<(), AppError> {
    // 1. Scheme check.
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(AppError::SsrfBlocked {
                url: url.to_string(),
                reason: format!(
                    "scheme '{scheme}' is not allowed; only http and https are permitted"
                ),
            });
        }
    }

    // 2. Port check.
    if let Some(port) = url.port() {
        if BLOCKED_PORTS.contains(&port) {
            return Err(AppError::SsrfBlocked {
                url: url.to_string(),
                reason: format!("port {port} is not permitted"),
            });
        }
    }

    // 3–5. Host check — use the structured Host enum to handle IPv6 brackets.
    match url.host() {
        None => {
            return Err(AppError::SsrfBlocked {
                url: url.to_string(),
                reason: "URL has no host component".to_string(),
            });
        }
        Some(url::Host::Ipv4(ip)) => {
            // 5a. IPv4 address range check.
            if is_blocked_ip(IpAddr::V4(ip)) {
                return Err(AppError::SsrfBlocked {
                    url: url.to_string(),
                    reason: format!("IP address {ip} is in a blocked network range"),
                });
            }
        }
        Some(url::Host::Ipv6(ip)) => {
            // 5b. IPv6 address range check.
            if is_blocked_ip(IpAddr::V6(ip)) {
                return Err(AppError::SsrfBlocked {
                    url: url.to_string(),
                    reason: format!("IP address {ip} is in a blocked network range"),
                });
            }
        }
        Some(url::Host::Domain(name)) => {
            // 4. Blocked hostname check (exact, case-insensitive).
            let name_lower = name.to_ascii_lowercase();
            if BLOCKED_HOSTNAMES.iter().any(|b| name_lower == *b) {
                return Err(AppError::SsrfBlocked {
                    url: url.to_string(),
                    reason: format!("host '{name}' is a blocked cloud-metadata endpoint"),
                });
            }
            if LOOPBACK_HOSTNAMES.iter().any(|lo| name_lower == *lo) {
                return Err(AppError::SsrfBlocked {
                    url: url.to_string(),
                    reason: format!("host '{name}' is a loopback alias"),
                });
            }
        }
    }

    Ok(())
}

/// Phase 1 + Phase 2 validation — Phase 1 checks plus async DNS resolution.
///
/// After Phase 1 succeeds the hostname is resolved via
/// [`tokio::net::lookup_host`]. Every IP returned by the resolver is checked
/// against all blocked CIDR ranges. This prevents DNS rebinding attacks where
/// a hostname initially resolves to a public IP but later resolves to a
/// private one.
///
/// DNS resolution timeout is controlled by `MCP_SSRF_DNS_TIMEOUT_SECS`
/// (default: 5 seconds).
///
/// # Errors
///
/// - [`AppError::SsrfBlocked`] — URL fails Phase 1 or a resolved IP is
///   blocked.
/// - [`AppError::UpstreamUnreachable`] — DNS resolution fails or times out.
pub async fn validate_url_with_dns(url: &Url) -> Result<(), AppError> {
    // Phase 1 — fail fast on obvious violations.
    validate_url(url)?;

    // Extract domain name. IP hosts (IPv4 and IPv6) were validated in Phase 1
    // and do not require DNS resolution.
    let host = match url.host() {
        None => return Ok(()),
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => return Ok(()),
        Some(url::Host::Domain(name)) => name.to_owned(),
    };

    let timeout_secs: u64 = std::env::var("MCP_SSRF_DNS_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5);

    // lookup_host requires "host:port" format.
    let port = url.port_or_known_default().unwrap_or(80);
    let lookup_target = format!("{host}:{port}");

    let resolved = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::net::lookup_host(lookup_target),
    )
    .await
    .map_err(|_| AppError::UpstreamUnreachable {
        reason: format!("DNS resolution for '{host}' timed out after {timeout_secs}s"),
    })?
    .map_err(|e| AppError::UpstreamUnreachable {
        reason: format!("DNS resolution failed for '{host}': {e}"),
    })?;

    for socket_addr in resolved {
        let ip = socket_addr.ip();
        if is_blocked_ip(ip) {
            return Err(AppError::SsrfBlocked {
                url: url.to_string(),
                reason: format!(
                    "resolved IP {ip} for host '{host}' is in a blocked network range"
                ),
            });
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_blocked_ip: IPv4 ranges ────────────────────────────────────────────

    #[test]
    fn test_rfc1918_class_a_blocked() {
        // 10.0.0.0/8
        assert!(is_blocked_ip("10.0.0.1".parse().expect("parse")));
        assert!(is_blocked_ip("10.255.255.255".parse().expect("parse")));
        assert!(is_blocked_ip("10.128.42.1".parse().expect("parse")));
    }

    #[test]
    fn test_rfc1918_class_b_blocked() {
        // 172.16.0.0/12 — 172.16.0.0 to 172.31.255.255
        assert!(is_blocked_ip("172.16.0.1".parse().expect("parse")));
        assert!(is_blocked_ip("172.31.255.255".parse().expect("parse")));
        assert!(is_blocked_ip("172.20.100.1".parse().expect("parse")));
    }

    #[test]
    fn test_rfc1918_class_b_boundary_not_blocked() {
        // 172.32.0.0 is outside 172.16.0.0/12
        assert!(!is_blocked_ip("172.32.0.1".parse().expect("parse")));
    }

    #[test]
    fn test_rfc1918_class_c_blocked() {
        // 192.168.0.0/16
        assert!(is_blocked_ip("192.168.0.1".parse().expect("parse")));
        assert!(is_blocked_ip("192.168.255.255".parse().expect("parse")));
    }

    #[test]
    fn test_ipv4_loopback_blocked() {
        // 127.0.0.0/8
        assert!(is_blocked_ip("127.0.0.1".parse().expect("parse")));
        assert!(is_blocked_ip("127.255.255.255".parse().expect("parse")));
        assert!(is_blocked_ip("127.0.0.0".parse().expect("parse")));
    }

    #[test]
    fn test_link_local_ipv4_blocked() {
        // 169.254.0.0/16
        assert!(is_blocked_ip("169.254.0.1".parse().expect("parse")));
        assert!(is_blocked_ip("169.254.169.254".parse().expect("parse")));
        assert!(is_blocked_ip("169.254.255.255".parse().expect("parse")));
    }

    #[test]
    fn test_cgnat_blocked() {
        // 100.64.0.0/10 — 100.64.0.0 to 100.127.255.255
        assert!(is_blocked_ip("100.64.0.1".parse().expect("parse")));
        assert!(is_blocked_ip("100.127.255.255".parse().expect("parse")));
        assert!(is_blocked_ip("100.100.50.1".parse().expect("parse")));
    }

    #[test]
    fn test_cgnat_boundary_not_blocked() {
        // 100.128.0.0 is outside 100.64.0.0/10
        assert!(!is_blocked_ip("100.128.0.1".parse().expect("parse")));
    }

    #[test]
    fn test_multicast_ipv4_blocked() {
        // 224.0.0.0/4
        assert!(is_blocked_ip("224.0.0.1".parse().expect("parse")));
        assert!(is_blocked_ip("239.255.255.255".parse().expect("parse")));
        assert!(is_blocked_ip("230.5.5.5".parse().expect("parse")));
    }

    #[test]
    fn test_reserved_class_e_blocked() {
        // 240.0.0.0/4
        assert!(is_blocked_ip("240.0.0.1".parse().expect("parse")));
        assert!(is_blocked_ip("255.255.255.254".parse().expect("parse")));
    }

    #[test]
    fn test_this_network_blocked() {
        // 0.0.0.0/8
        assert!(is_blocked_ip("0.0.0.0".parse().expect("parse")));
        assert!(is_blocked_ip("0.255.255.255".parse().expect("parse")));
    }

    #[test]
    fn test_public_ipv4_not_blocked() {
        // Well-known public addresses should not be blocked.
        assert!(!is_blocked_ip("8.8.8.8".parse().expect("parse")));
        assert!(!is_blocked_ip("1.1.1.1".parse().expect("parse")));
        assert!(!is_blocked_ip("93.184.216.34".parse().expect("parse")));
    }

    // ── is_blocked_ip: IPv6 ranges ────────────────────────────────────────────

    #[test]
    fn test_ipv6_loopback_blocked() {
        // ::1/128
        assert!(is_blocked_ip("::1".parse().expect("parse")));
    }

    #[test]
    fn test_ipv6_link_local_blocked() {
        // fe80::/10
        assert!(is_blocked_ip("fe80::1".parse().expect("parse")));
        assert!(is_blocked_ip("fe80::cafe:babe".parse().expect("parse")));
        assert!(is_blocked_ip("febf::1".parse().expect("parse")));
    }

    #[test]
    fn test_ipv6_ula_fc_blocked() {
        // fc00::/7 (fc range)
        assert!(is_blocked_ip("fc00::1".parse().expect("parse")));
        assert!(is_blocked_ip("fcff::1".parse().expect("parse")));
    }

    #[test]
    fn test_ipv6_ula_fd_blocked() {
        // fc00::/7 (fd range — same /7 prefix)
        assert!(is_blocked_ip("fd00::1".parse().expect("parse")));
        assert!(is_blocked_ip("fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff".parse().expect("parse")));
    }

    #[test]
    fn test_ipv6_mapped_ipv4_blocked() {
        // ::ffff:0:0/96 — IPv4-mapped
        assert!(is_blocked_ip("::ffff:192.168.1.1".parse().expect("parse")));
        assert!(is_blocked_ip("::ffff:127.0.0.1".parse().expect("parse")));
    }

    #[test]
    fn test_public_ipv6_not_blocked() {
        // 2001:db8::/32 is documentation range, but outside all blocked ranges.
        // Real public IPv6:
        assert!(!is_blocked_ip("2001:4860:4860::8888".parse().expect("parse"))); // Google DNS
        assert!(!is_blocked_ip("2606:4700:4700::1111".parse().expect("parse"))); // Cloudflare
    }

    // ── validate_url: Phase 1 checks ─────────────────────────────────────────

    #[test]
    fn test_valid_public_https_url_passes() {
        let url = Url::parse("https://api.stripe.com/v1/customers").expect("parse");
        assert!(validate_url(&url).is_ok());
    }

    #[test]
    fn test_valid_public_http_url_passes() {
        let url = Url::parse("http://api.example.com/endpoint").expect("parse");
        assert!(validate_url(&url).is_ok());
    }

    #[test]
    fn test_file_scheme_blocked() {
        let url = Url::parse("file:///etc/passwd").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_ftp_scheme_blocked() {
        let url = Url::parse("ftp://files.example.com/data").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_private_ip_blocked() {
        let url = Url::parse("http://192.168.1.1/api").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_loopback_ip_blocked() {
        let url = Url::parse("http://127.0.0.1/internal").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_ipv6_loopback_url_blocked() {
        let url = Url::parse("http://[::1]/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_cloud_metadata_169_blocked() {
        let url = Url::parse("http://169.254.169.254/latest/meta-data/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_metadata_google_internal_blocked() {
        let url = Url::parse("http://metadata.google.internal/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_localhost_hostname_blocked() {
        let url = Url::parse("http://localhost/admin").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_blocked_port_ssh_blocked() {
        let url = Url::parse("https://api.example.com:22/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_blocked_port_postgres_blocked() {
        let url = Url::parse("https://api.example.com:5432/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_blocked_port_redis_blocked() {
        let url = Url::parse("https://api.example.com:6379/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_blocked_port_mysql_blocked() {
        let url = Url::parse("https://api.example.com:3306/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_blocked_port_smtp_blocked() {
        let url = Url::parse("https://api.example.com:25/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_blocked_port_telnet_blocked() {
        let url = Url::parse("https://api.example.com:23/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_standard_https_port_allowed() {
        let url = Url::parse("https://api.example.com:443/endpoint").expect("parse");
        assert!(validate_url(&url).is_ok());
    }

    #[test]
    fn test_standard_http_port_allowed() {
        let url = Url::parse("http://api.example.com:80/endpoint").expect("parse");
        assert!(validate_url(&url).is_ok());
    }

    #[test]
    fn test_rfc1918_class_a_blocked_url() {
        let url = Url::parse("https://10.0.0.1/api").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_cgnat_blocked_url() {
        let url = Url::parse("https://100.64.0.1/api").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[test]
    fn test_multicast_blocked_url() {
        let url = Url::parse("https://224.0.0.1/").expect("parse");
        let err = validate_url(&url).expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    // ── validate_url_with_dns: Phase 2 with IP host (no DNS) ─────────────────

    #[tokio::test]
    async fn test_validate_with_dns_bare_ip_blocked() {
        // Bare IP host — Phase 1 catches it; Phase 2 skips DNS.
        let url = Url::parse("http://10.0.0.1/api").expect("parse");
        let err = validate_url_with_dns(&url).await.expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }

    #[tokio::test]
    async fn test_validate_with_dns_file_scheme_blocked() {
        let url = Url::parse("file:///etc/passwd").expect("parse");
        let err = validate_url_with_dns(&url).await.expect_err("should be blocked");
        assert!(matches!(err, AppError::SsrfBlocked { .. }));
    }
}
