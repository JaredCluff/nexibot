//! SSRF (Server-Side Request Forgery) protection.
//!
//! Validates URLs before fetching by checking hostnames and resolved IPs
//! against private/internal network ranges, blocking DNS rebinding attacks.
#![allow(dead_code)]

use idna::domain_to_ascii;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use tracing::debug;

/// Policy controlling SSRF validation behavior.
#[derive(Debug, Clone, Default)]
pub struct SsrfPolicy {
    /// When true, allows requests to private/internal networks.
    pub allow_private_network: bool,
}

/// Error type for SSRF-blocked requests.
#[derive(Debug, Clone)]
pub struct SsrfBlockedError {
    pub message: String,
}

impl std::fmt::Display for SsrfBlockedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SSRF blocked: {}", self.message)
    }
}

impl std::error::Error for SsrfBlockedError {}

/// Blocked hostnames that should never be fetched.
const BLOCKED_HOSTNAMES: &[&str] = &[
    "localhost",
    "metadata.google.internal",
    "metadata.internal",
    "instance-data",
];

/// Check if an IPv4 address is in a private/internal range.
fn is_private_ipv4(addr: &Ipv4Addr) -> bool {
    let octets = addr.octets();
    let (o1, o2) = (octets[0], octets[1]);

    // 0.0.0.0/8 — current network
    if o1 == 0 {
        return true;
    }
    // 10.0.0.0/8 — private
    if o1 == 10 {
        return true;
    }
    // 127.0.0.0/8 — loopback
    if o1 == 127 {
        return true;
    }
    // 169.254.0.0/16 — link-local
    if o1 == 169 && o2 == 254 {
        return true;
    }
    // 172.16.0.0/12 — private
    if o1 == 172 && (16..=31).contains(&o2) {
        return true;
    }
    // 192.168.0.0/16 — private
    if o1 == 192 && o2 == 168 {
        return true;
    }
    // 100.64.0.0/10 — CGNAT
    if o1 == 100 && (64..=127).contains(&o2) {
        return true;
    }

    false
}

/// Check if an IPv6 address is private/internal, including embedded IPv4.
fn is_private_ipv6(addr: &Ipv6Addr) -> bool {
    let segments = addr.segments();

    // :: (unspecified)
    if addr.is_unspecified() {
        return true;
    }
    // ::1 (loopback)
    if addr.is_loopback() {
        return true;
    }

    // Check for embedded IPv4 (IPv4-mapped, IPv4-compatible, NAT64, 6to4, Teredo)
    if let Some(ipv4) = extract_embedded_ipv4(segments) {
        return is_private_ipv4(&ipv4);
    }

    let first = segments[0];

    // fe80::/10 — link-local
    if (first & 0xffc0) == 0xfe80 {
        return true;
    }
    // fec0::/10 — site-local (deprecated but still internal)
    if (first & 0xffc0) == 0xfec0 {
        return true;
    }
    // fc00::/7 — unique local
    if (first & 0xfe00) == 0xfc00 {
        return true;
    }

    false
}

/// Extract an embedded IPv4 address from an IPv6 address.
///
/// Handles IPv4-mapped (::ffff:x.x.x.x), IPv4-compatible (::x.x.x.x),
/// NAT64 (64:ff9b::/96), 6to4 (2002::/16), and Teredo (2001:0000::/32).
fn extract_embedded_ipv4(segments: [u16; 8]) -> Option<Ipv4Addr> {
    // IPv4-mapped: ::ffff:x.x.x.x
    if segments[0] == 0
        && segments[1] == 0
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && segments[5] == 0xffff
    {
        return Some(decode_ipv4_from_segments(segments[6], segments[7]));
    }

    // IPv4-compatible: ::x.x.x.x (deprecated but must block)
    if segments[0] == 0
        && segments[1] == 0
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && segments[5] == 0
        && (segments[6] != 0 || segments[7] > 1)
    {
        return Some(decode_ipv4_from_segments(segments[6], segments[7]));
    }

    // NAT64: 64:ff9b::/96
    if segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && segments[5] == 0
    {
        return Some(decode_ipv4_from_segments(segments[6], segments[7]));
    }

    // 6to4: 2002::/16 — IPv4 in segments[1..2]
    if segments[0] == 0x2002 {
        return Some(decode_ipv4_from_segments(segments[1], segments[2]));
    }

    // Teredo: 2001:0000::/32 — client IPv4 XOR 0xffff in segments[6..7]
    if segments[0] == 0x2001 && segments[1] == 0x0000 {
        return Some(decode_ipv4_from_segments(
            segments[6] ^ 0xffff,
            segments[7] ^ 0xffff,
        ));
    }

    // ISATAP: prefix::0:5efe:a.b.c.d or prefix::0200:5efe:a.b.c.d
    // The IPv4 address is in the last two segments when segments[4..6] match
    // the ISATAP interface identifier pattern.
    if (segments[4] == 0x0000 || segments[4] == 0x0200) && segments[5] == 0x5efe {
        return Some(decode_ipv4_from_segments(segments[6], segments[7]));
    }

    // NAT64 local-use: 64:ff9b:1::/48
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 0x0001 {
        return Some(decode_ipv4_from_segments(segments[6], segments[7]));
    }

    None
}

/// Decode two 16-bit segments into an IPv4 address.
fn decode_ipv4_from_segments(high: u16, low: u16) -> Ipv4Addr {
    Ipv4Addr::new(
        (high >> 8) as u8,
        (high & 0xff) as u8,
        (low >> 8) as u8,
        (low & 0xff) as u8,
    )
}

/// Check if an IP address is private/internal.
pub fn is_private_ip(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

/// Normalize a hostname for comparison.
///
/// Lowercases the hostname and normalizes IDN (Internationalized Domain Names)
/// to their ASCII/Punycode representation via `idna::domain_to_ascii`.  This
/// prevents Unicode homograph bypass attacks where a domain like
/// `metadata.google.ιnternal` (Greek iota) would otherwise pass the blocklist
/// check despite resolving to the same address as `metadata.google.internal`.
fn normalize_hostname(hostname: &str) -> String {
    let mut h = hostname.trim().to_lowercase();
    // Strip bracket notation [::1]
    if h.starts_with('[') && h.ends_with(']') {
        h = h[1..h.len() - 1].to_string();
    }
    // Strip trailing dot (FQDN)
    if h.ends_with('.') {
        h.pop();
    }
    // Normalize IDN/Unicode hostnames to their ASCII/Punycode form so that
    // Unicode homograph domains (e.g. metadata.google.ιnternal with Greek iota)
    // cannot bypass the blocklist.  On failure we fall back to the already
    // lowercased ASCII representation.
    match domain_to_ascii(&h) {
        Ok(ascii) => ascii,
        Err(_) => h,
    }
}

/// Check if a hostname is in the blocklist.
pub fn is_blocked_hostname(hostname: &str) -> bool {
    let normalized = normalize_hostname(hostname);
    if normalized.is_empty() {
        return false;
    }

    for blocked in BLOCKED_HOSTNAMES {
        if normalized == *blocked {
            return true;
        }
    }

    normalized.ends_with(".localhost")
        || normalized.ends_with(".local")
        || normalized.ends_with(".internal")
}

/// Reject non-canonical IPv4 literal representations that could bypass SSRF checks.
///
/// Blocks: octal (0177.0.0.1), hex (0x7f000001), short (127.1), packed decimal (2130706433).
/// Only strict dotted-decimal with 4 octets (0-255) is accepted.
fn is_non_canonical_ipv4_literal(host: &str) -> bool {
    // Must have exactly 4 dot-separated parts
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        // Could be packed decimal (single integer) or short form
        // If it parses as a valid IPv4 packed decimal (≤ u32::MAX), block it.
        // Values > u32::MAX cannot be valid IPv4 literals, but block them too
        // as they're clearly not legitimate hostnames.
        if host.chars().all(|c| c.is_ascii_digit()) {
            return true; // packed decimal like "2130706433", or oversized integer
        }
        // Hex integer like "0x7f000001"
        if let Some(hex_str) = host.strip_prefix("0x").or_else(|| host.strip_prefix("0X")) {
            if !hex_str.is_empty() && hex_str.chars().all(|c| c.is_ascii_hexdigit()) {
                return true;
            }
        }
        // Short form like "127.1" (2 or 3 parts that resolve to IPv4)
        if parts.len() >= 2
            && parts.len() <= 3
            && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
        {
            return true;
        }
        return false;
    }

    for part in &parts {
        if part.is_empty() {
            return true;
        }
        // Octal: starts with 0 and has more than 1 digit
        if part.len() > 1
            && part.starts_with('0')
            && !part.starts_with("0x")
            && !part.starts_with("0X")
        {
            return true; // e.g., "0177" is octal for 127
        }
        // Hex: starts with 0x
        if part.starts_with("0x") || part.starts_with("0X") {
            return true;
        }
        // Must be valid decimal 0-255
        match part.parse::<u16>() {
            Ok(n) if n <= 255 => {}
            _ => return true, // out of range or not a number
        }
    }
    false
}

/// Validate a URL against SSRF protections.
///
/// Resolves DNS and checks all resolved IPs against private ranges.
/// Returns `Ok(())` if the URL is safe to fetch.
pub fn validate_url(url: &url::Url, policy: &SsrfPolicy) -> Result<(), SsrfBlockedError> {
    if policy.allow_private_network {
        tracing::warn!(
            "[SSRF] allow_private_network is ENABLED — all SSRF protections bypassed for {}. \
             Only use this in trusted local-network deployments.",
            url
        );
        return Ok(());
    }

    // Block dangerous URI schemes
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(SsrfBlockedError {
            message: format!("Blocked URI scheme '{}' — only http/https allowed", scheme),
        });
    }

    let host = url.host_str().ok_or_else(|| SsrfBlockedError {
        message: "URL has no host".to_string(),
    })?;

    let normalized = normalize_hostname(host);

    // Reject non-canonical IPv4 literals (octal, hex, short, packed)
    if is_non_canonical_ipv4_literal(&normalized) {
        return Err(SsrfBlockedError {
            message: format!("Blocked non-canonical IPv4 literal: {}", host),
        });
    }

    // Check hostname blocklist
    if is_blocked_hostname(&normalized) {
        return Err(SsrfBlockedError {
            message: format!("Blocked hostname: {}", host),
        });
    }

    // Check if the host is already a literal IP
    if let Ok(ip) = normalized.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(SsrfBlockedError {
                message: "Blocked: private/internal IP address".to_string(),
            });
        }
        return Ok(());
    }

    // Resolve DNS and check all addresses
    let port = url.port_or_known_default().unwrap_or(80);
    let socket_addr = format!("{}:{}", normalized, port);

    match socket_addr.to_socket_addrs() {
        Ok(addrs) => {
            let resolved: Vec<_> = addrs.collect();
            if resolved.is_empty() {
                return Err(SsrfBlockedError {
                    message: format!("Unable to resolve hostname: {}", host),
                });
            }
            for addr in &resolved {
                if is_private_ip(&addr.ip()) {
                    return Err(SsrfBlockedError {
                        message: "Blocked: resolves to private/internal IP address".to_string(),
                    });
                }
            }
            debug!(
                "[SSRF] Validated URL {} ({} resolved addresses)",
                url,
                resolved.len()
            );
            Ok(())
        }
        Err(_) => {
            // DNS resolution failure — fail CLOSED to prevent bypass via flaky DNS
            debug!(
                "[SSRF] DNS resolution failed for {}, blocking (fail closed)",
                host
            );
            Err(SsrfBlockedError {
                message: format!(
                    "DNS resolution failed for '{}' — request blocked (fail closed)",
                    host
                ),
            })
        }
    }
}

/// Result of DNS resolution with pinned addresses.
///
/// After calling `resolve_and_pin`, the caller should connect directly to one
/// of the `pinned_addrs` rather than re-resolving via hostname. This prevents
/// DNS rebinding attacks where the first resolution returns a public IP but a
/// subsequent resolution returns a private IP.
#[derive(Debug, Clone)]
pub struct PinnedResolution {
    /// The original hostname that was resolved.
    pub hostname: String,
    /// All resolved IP addresses (already validated against private ranges).
    pub pinned_addrs: Vec<IpAddr>,
}

/// Resolve a hostname and pin the result, validating against SSRF rules.
///
/// Returns the validated IP addresses. The caller should use these directly
/// for the connection rather than re-resolving the hostname.
pub fn resolve_and_pin(
    hostname: &str,
    port: u16,
    policy: &SsrfPolicy,
) -> Result<PinnedResolution, SsrfBlockedError> {
    if policy.allow_private_network {
        // Skip validation but still resolve
        let socket_addr = format!("{}:{}", hostname, port);
        match socket_addr.to_socket_addrs() {
            Ok(addrs) => Ok(PinnedResolution {
                hostname: hostname.to_string(),
                pinned_addrs: addrs.map(|a| a.ip()).collect(),
            }),
            Err(_) => Err(SsrfBlockedError {
                message: format!("DNS resolution failed for '{}'", hostname),
            }),
        }
    } else {
        let normalized = normalize_hostname(hostname);
        if is_blocked_hostname(&normalized) {
            return Err(SsrfBlockedError {
                message: format!("Blocked hostname: {}", hostname),
            });
        }

        // Reject non-canonical IPv4 literals (octal, hex, short, packed)
        if is_non_canonical_ipv4_literal(&normalized) {
            return Err(SsrfBlockedError {
                message: format!("Blocked non-canonical IPv4 literal: {}", hostname),
            });
        }

        // Check literal IP
        if let Ok(ip) = normalized.parse::<IpAddr>() {
            if is_private_ip(&ip) {
                return Err(SsrfBlockedError {
                    message: "Blocked: private/internal IP address".to_string(),
                });
            }
            return Ok(PinnedResolution {
                hostname: hostname.to_string(),
                pinned_addrs: vec![ip],
            });
        }

        // Resolve and validate all addresses
        let socket_addr = format!("{}:{}", normalized, port);
        match socket_addr.to_socket_addrs() {
            Ok(addrs) => {
                let resolved: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
                if resolved.is_empty() {
                    return Err(SsrfBlockedError {
                        message: format!("Unable to resolve hostname: {}", hostname),
                    });
                }
                for addr in &resolved {
                    if is_private_ip(addr) {
                        return Err(SsrfBlockedError {
                            message: "Blocked: resolves to private/internal IP address".to_string(),
                        });
                    }
                }
                debug!("[SSRF] Pinned {} to {} addresses", hostname, resolved.len());
                Ok(PinnedResolution {
                    hostname: hostname.to_string(),
                    pinned_addrs: resolved,
                })
            }
            Err(_) => {
                debug!(
                    "[SSRF] DNS resolution failed for {}, blocking (fail closed)",
                    hostname
                );
                Err(SsrfBlockedError {
                    message: format!(
                        "DNS resolution failed for '{}' — request blocked (fail closed)",
                        hostname
                    ),
                })
            }
        }
    }
}

/// Validate a redirect URL against SSRF protections.
///
/// Should be called for every HTTP redirect in a chain to prevent DNS
/// rebinding attacks where the initial URL is safe but a redirect targets
/// a private/internal address.
pub fn validate_redirect(
    redirect_url: &url::Url,
    policy: &SsrfPolicy,
) -> Result<(), SsrfBlockedError> {
    debug!("[SSRF] Validating redirect to {}", redirect_url);
    validate_url(redirect_url, policy)
}

/// Headers that should be stripped when a redirect crosses origin boundaries.
const CROSS_ORIGIN_SENSITIVE_HEADERS: &[&str] =
    &["authorization", "proxy-authorization", "cookie", "cookie2"];

/// Check if a redirect crosses origin boundaries and return headers to strip.
///
/// Returns the list of header names that should be removed from the redirect
/// request to prevent credential forwarding across origins.
pub fn headers_to_strip_on_redirect(
    original_url: &url::Url,
    redirect_url: &url::Url,
) -> Vec<&'static str> {
    let orig_origin = (
        original_url.scheme(),
        original_url.host_str().unwrap_or(""),
        original_url.port_or_known_default(),
    );
    let redir_origin = (
        redirect_url.scheme(),
        redirect_url.host_str().unwrap_or(""),
        redirect_url.port_or_known_default(),
    );

    if orig_origin != redir_origin {
        CROSS_ORIGIN_SENSITIVE_HEADERS.to_vec()
    } else {
        vec![]
    }
}

/// Validate an outbound HTTP request URL for SSRF safety.
///
/// This is the primary entry point for SSRF validation of outbound requests.
/// It performs comprehensive checks:
/// 1. Parses the URL string
/// 2. Validates the scheme is http or https only (blocks file://, ftp://, gopher://, etc.)
/// 3. Validates the hostname is not a private/reserved IP address
/// 4. Checks against blocked domains from the provided list
/// 5. Resolves DNS and validates all resolved IPs (fail-closed on DNS errors)
///
/// Returns `Ok(validated_url)` if safe, or `Err(SsrfBlockedError)` if blocked.
pub fn validate_outbound_request(
    url_str: &str,
    policy: &SsrfPolicy,
    blocked_domains: &[String],
) -> Result<url::Url, SsrfBlockedError> {
    // Parse the URL
    let parsed_url = url::Url::parse(url_str).map_err(|e| SsrfBlockedError {
        message: format!("Invalid URL '{}': {}", url_str, e),
    })?;

    // Validate scheme is http or https only
    match parsed_url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(SsrfBlockedError {
                message: format!("Blocked URI scheme '{}' — only http/https allowed", scheme),
            });
        }
    }

    // Run full SSRF validation (hostname blocklist, private IPs, DNS resolution)
    validate_url(&parsed_url, policy)?;

    // Check user-configured blocked domains
    if let Some(host) = parsed_url.host_str() {
        let host_lower = host.to_lowercase();
        for blocked in blocked_domains {
            let blocked_lower = blocked.to_lowercase();
            if host_lower == blocked_lower || host_lower.ends_with(&format!(".{}", blocked_lower)) {
                return Err(SsrfBlockedError {
                    message: format!(
                        "Domain '{}' is blocked by configuration (matched '{}')",
                        host, blocked
                    ),
                });
            }
        }
    }

    Ok(parsed_url)
}

/// Validate that a URL targets a loopback address only.
///
/// Used for internal service URLs read from environment variables
/// (e.g. `ANTHROPIC_BRIDGE_URL`) where only loopback targets are expected.
/// Returns `Ok(())` for `127.x.x.x`, `::1`, and `localhost`; returns an
/// error for any other host.
pub fn validate_loopback_url(raw_url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(raw_url)
        .map_err(|e| format!("Invalid URL '{}': {}", raw_url, e))?;
    let is_loopback = match parsed.host() {
        Some(url::Host::Ipv4(addr)) => addr.is_loopback(),
        Some(url::Host::Ipv6(addr)) => addr.is_loopback(),
        Some(url::Host::Domain(h)) => h.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if is_loopback {
        Ok(())
    } else {
        Err(format!(
            "Internal service URL '{}' must target a loopback address \
             (127.0.0.1 / ::1 / localhost)",
            raw_url
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_private_ipv4() {
        assert!(is_private_ipv4(&Ipv4Addr::new(10, 0, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(192, 168, 1, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(172, 16, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(172, 31, 255, 255)));
        assert!(is_private_ipv4(&Ipv4Addr::new(169, 254, 1, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(0, 0, 0, 0)));
        assert!(is_private_ipv4(&Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(100, 127, 255, 255)));
    }

    #[test]
    fn test_public_ipv4() {
        assert!(!is_private_ipv4(&Ipv4Addr::new(8, 8, 8, 8)));
        assert!(!is_private_ipv4(&Ipv4Addr::new(1, 1, 1, 1)));
        assert!(!is_private_ipv4(&Ipv4Addr::new(93, 184, 216, 34)));
        assert!(!is_private_ipv4(&Ipv4Addr::new(172, 32, 0, 1)));
        assert!(!is_private_ipv4(&Ipv4Addr::new(100, 128, 0, 1)));
    }

    #[test]
    fn test_private_ipv6() {
        assert!(is_private_ipv6(&Ipv6Addr::UNSPECIFIED));
        assert!(is_private_ipv6(&Ipv6Addr::LOCALHOST));
        // fe80::1 link-local
        assert!(is_private_ipv6(&Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)));
        // fc00::1 unique local
        assert!(is_private_ipv6(&Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)));
        // fec0::1 site-local
        assert!(is_private_ipv6(&Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 1)));
    }

    #[test]
    fn test_ipv4_mapped_ipv6() {
        // ::ffff:127.0.0.1
        let addr = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        assert!(is_private_ipv6(&addr));

        // ::ffff:8.8.8.8 (public)
        let addr = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x0808, 0x0808);
        assert!(!is_private_ipv6(&addr));
    }

    #[test]
    fn test_6to4_embedded_ipv4() {
        // 2002:7f00:0001:: (embeds 127.0.0.1)
        let addr = Ipv6Addr::new(0x2002, 0x7f00, 0x0001, 0, 0, 0, 0, 0);
        assert!(is_private_ipv6(&addr));

        // 2002:0808:0808:: (embeds 8.8.8.8 — public)
        let addr = Ipv6Addr::new(0x2002, 0x0808, 0x0808, 0, 0, 0, 0, 0);
        assert!(!is_private_ipv6(&addr));
    }

    #[test]
    fn test_isatap_embedded_ipv4() {
        // fe80::0:5efe:127.0.0.1 (ISATAP with loopback)
        let addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0x0000, 0x5efe, 0x7f00, 0x0001);
        assert!(is_private_ipv6(&addr));

        // ::0200:5efe:10.0.0.1 (ISATAP variant with private IPv4)
        let addr = Ipv6Addr::new(0, 0, 0, 0, 0x0200, 0x5efe, 0x0a00, 0x0001);
        assert!(is_private_ipv6(&addr));

        // ::0:5efe:8.8.8.8 (ISATAP with public IPv4)
        let addr = Ipv6Addr::new(0, 0, 0, 0, 0x0000, 0x5efe, 0x0808, 0x0808);
        assert!(!is_private_ipv6(&addr));
    }

    #[test]
    fn test_nat64_local_use_embedded_ipv4() {
        // 64:ff9b:1::127.0.0.1
        let addr = Ipv6Addr::new(0x0064, 0xff9b, 0x0001, 0, 0, 0, 0x7f00, 0x0001);
        assert!(is_private_ipv6(&addr));

        // 64:ff9b:1::8.8.8.8 (public)
        let addr = Ipv6Addr::new(0x0064, 0xff9b, 0x0001, 0, 0, 0, 0x0808, 0x0808);
        assert!(!is_private_ipv6(&addr));
    }

    #[test]
    fn test_teredo_embedded_ipv4() {
        // 2001:0000::xxxx:yyyy where client = segments[6..7] XOR 0xffff
        // Teredo for 127.0.0.1: XOR 0xffff => 0x80ff, 0xfffe
        let addr = Ipv6Addr::new(0x2001, 0x0000, 0, 0, 0, 0, 0x80ff, 0xfffe);
        assert!(is_private_ipv6(&addr));
    }

    #[test]
    fn test_blocked_hostnames() {
        assert!(is_blocked_hostname("localhost"));
        assert!(is_blocked_hostname("LOCALHOST"));
        assert!(is_blocked_hostname("metadata.google.internal"));
        assert!(is_blocked_hostname("something.local"));
        assert!(is_blocked_hostname("test.localhost"));
        assert!(is_blocked_hostname("foo.internal"));
    }

    #[test]
    fn test_allowed_hostnames() {
        assert!(!is_blocked_hostname("example.com"));
        assert!(!is_blocked_hostname("google.com"));
        assert!(!is_blocked_hostname("api.anthropic.com"));
    }

    #[test]
    fn test_validate_url_blocks_localhost() {
        let url = url::Url::parse("http://localhost:8080/path").unwrap();
        let policy = SsrfPolicy::default();
        assert!(validate_url(&url, &policy).is_err());
    }

    #[test]
    fn test_validate_url_blocks_private_ip() {
        let url = url::Url::parse("http://192.168.1.1/admin").unwrap();
        let policy = SsrfPolicy::default();
        assert!(validate_url(&url, &policy).is_err());
    }

    #[test]
    fn test_validate_url_allows_with_private_network_policy() {
        let url = url::Url::parse("http://localhost:8080/path").unwrap();
        let policy = SsrfPolicy {
            allow_private_network: true,
        };
        assert!(validate_url(&url, &policy).is_ok());
    }

    #[test]
    fn test_validate_url_blocks_metadata() {
        let url = url::Url::parse("http://metadata.google.internal/computeMetadata/v1/").unwrap();
        let policy = SsrfPolicy::default();
        assert!(validate_url(&url, &policy).is_err());
    }

    #[test]
    fn test_validate_url_blocks_169_254() {
        let url = url::Url::parse("http://169.254.169.254/latest/meta-data/").unwrap();
        let policy = SsrfPolicy::default();
        assert!(validate_url(&url, &policy).is_err());
    }

    #[test]
    fn test_validate_url_blocks_on_dns_failure() {
        // A domain that almost certainly won't resolve — fail closed
        let url = url::Url::parse("http://this-domain-does-not-exist-12345.invalid/path").unwrap();
        let policy = SsrfPolicy::default();
        let result = validate_url(&url, &policy);
        assert!(
            result.is_err(),
            "DNS resolution failure should block the request (fail closed)"
        );
        let err = result.unwrap_err();
        assert!(
            err.message.contains("DNS resolution failed"),
            "Error should mention DNS failure: {}",
            err.message
        );
    }

    #[test]
    fn test_non_canonical_ipv4_octal() {
        assert!(is_non_canonical_ipv4_literal("0177.0.0.1")); // octal for 127
        assert!(is_non_canonical_ipv4_literal("010.0.0.1")); // octal for 8
    }

    #[test]
    fn test_non_canonical_ipv4_hex() {
        assert!(is_non_canonical_ipv4_literal("0x7f.0.0.1"));
        assert!(is_non_canonical_ipv4_literal("0x7f000001"));
    }

    #[test]
    fn test_non_canonical_ipv4_short() {
        assert!(is_non_canonical_ipv4_literal("127.1")); // short for 127.0.0.1
    }

    #[test]
    fn test_non_canonical_ipv4_packed() {
        assert!(is_non_canonical_ipv4_literal("2130706433")); // packed decimal for 127.0.0.1
    }

    #[test]
    fn test_canonical_ipv4_accepted() {
        assert!(!is_non_canonical_ipv4_literal("127.0.0.1"));
        assert!(!is_non_canonical_ipv4_literal("8.8.8.8"));
        assert!(!is_non_canonical_ipv4_literal("192.168.1.1"));
    }

    #[test]
    fn test_scheme_blocking() {
        let policy = SsrfPolicy::default();
        assert!(validate_url(&url::Url::parse("file:///etc/passwd").unwrap(), &policy).is_err());
        assert!(validate_url(&url::Url::parse("ftp://evil.com/file").unwrap(), &policy).is_err());
        assert!(validate_url(&url::Url::parse("gopher://evil.com/").unwrap(), &policy).is_err());
    }

    #[test]
    fn test_cross_origin_redirect_stripping() {
        let orig = url::Url::parse("https://example.com/api").unwrap();
        let same = url::Url::parse("https://example.com/other").unwrap();
        let cross = url::Url::parse("https://evil.com/steal").unwrap();

        assert!(headers_to_strip_on_redirect(&orig, &same).is_empty());
        assert!(!headers_to_strip_on_redirect(&orig, &cross).is_empty());
        assert!(headers_to_strip_on_redirect(&orig, &cross).contains(&"authorization"));
    }

    // ── validate_outbound_request tests ─────────────────────────────

    #[test]
    fn test_outbound_request_blocks_file_scheme() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("file:///etc/passwd", &policy, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Blocked URI scheme"));
    }

    #[test]
    fn test_outbound_request_blocks_ftp_scheme() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("ftp://evil.com/file", &policy, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Blocked URI scheme"));
    }

    #[test]
    fn test_outbound_request_blocks_gopher_scheme() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("gopher://evil.com/", &policy, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Blocked URI scheme"));
    }

    #[test]
    fn test_outbound_request_blocks_localhost() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("http://localhost:8080/api", &policy, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_blocks_private_ip_127() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("http://127.0.0.1/admin", &policy, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_blocks_private_ip_10() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("http://10.0.0.1/internal", &policy, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_blocks_private_ip_172_16() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("http://172.16.0.1/secret", &policy, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_blocks_private_ip_192_168() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("http://192.168.1.1/router", &policy, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_blocks_link_local_169_254() {
        let policy = SsrfPolicy::default();
        let result =
            validate_outbound_request("http://169.254.169.254/latest/meta-data/", &policy, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_blocks_ipv6_loopback() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("http://[::1]/path", &policy, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_blocks_ipv6_unique_local() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("http://[fc00::1]/path", &policy, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_blocks_configured_domain() {
        let policy = SsrfPolicy::default();
        let blocked = vec!["evil.com".to_string(), "malware.org".to_string()];
        let result = validate_outbound_request("https://evil.com/phish", &policy, &blocked);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message
            .contains("blocked by configuration"));
    }

    #[test]
    fn test_outbound_request_blocks_subdomain_of_configured_domain() {
        let policy = SsrfPolicy::default();
        let blocked = vec!["evil.com".to_string()];
        let result = validate_outbound_request("https://sub.evil.com/phish", &policy, &blocked);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message
            .contains("blocked by configuration"));
    }

    #[test]
    fn test_outbound_request_invalid_url() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request("not a url at all", &policy, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Invalid URL"));
    }

    #[test]
    fn test_outbound_request_allows_with_private_network_policy() {
        let policy = SsrfPolicy {
            allow_private_network: true,
        };
        let result = validate_outbound_request("http://127.0.0.1:8080/api", &policy, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_outbound_request_blocks_metadata_endpoint() {
        let policy = SsrfPolicy::default();
        let result = validate_outbound_request(
            "http://metadata.google.internal/computeMetadata/v1/",
            &policy,
            &[],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_request_returns_parsed_url_on_success() {
        let policy = SsrfPolicy::default();
        // Use a domain that will fail DNS resolution (fail-closed), so test
        // with private network policy to skip DNS check
        let policy_allow = SsrfPolicy {
            allow_private_network: true,
        };
        let result = validate_outbound_request("https://example.com/api/data", &policy_allow, &[]);
        assert!(result.is_ok());
        let url = result.unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("example.com"));
        assert_eq!(url.path(), "/api/data");
    }

    #[test]
    fn test_outbound_request_case_insensitive_domain_blocking() {
        let policy = SsrfPolicy {
            allow_private_network: true,
        };
        let blocked = vec!["Evil.COM".to_string()];
        let result = validate_outbound_request("https://evil.com/phish", &policy, &blocked);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message
            .contains("blocked by configuration"));
    }

    // -- Property-based fuzz tests --

    mod proptest_fuzz {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Any arbitrary IPv4 address should never panic is_private_ipv4.
            #[test]
            fn fuzz_is_private_ipv4_never_panics(a in 0u8..=255, b in 0u8..=255, c in 0u8..=255, d in 0u8..=255) {
                let _ = is_private_ipv4(&Ipv4Addr::new(a, b, c, d));
            }

            /// Any arbitrary IPv6 segments should never panic is_private_ipv6.
            #[test]
            fn fuzz_is_private_ipv6_never_panics(
                s0 in 0u16..=65535, s1 in 0u16..=65535, s2 in 0u16..=65535, s3 in 0u16..=65535,
                s4 in 0u16..=65535, s5 in 0u16..=65535, s6 in 0u16..=65535, s7 in 0u16..=65535,
            ) {
                let addr = Ipv6Addr::new(s0, s1, s2, s3, s4, s5, s6, s7);
                let _ = is_private_ipv6(&addr);
            }

            /// Arbitrary strings should never panic is_blocked_hostname.
            #[test]
            fn fuzz_is_blocked_hostname_never_panics(hostname in ".*") {
                let _ = is_blocked_hostname(&hostname);
            }

            /// Any valid URL should not panic validate_url (may return Ok or Err).
            #[test]
            fn fuzz_validate_url_never_panics(
                scheme in "(http|https)",
                host in "[a-z0-9]{1,20}\\.[a-z]{2,6}",
                port in 1u16..=65535,
                path in "/[a-z0-9/]{0,30}",
            ) {
                let url_str = format!("{}://{}:{}{}", scheme, host, port, path);
                if let Ok(url) = url::Url::parse(&url_str) {
                    let policy = SsrfPolicy::default();
                    let _ = validate_url(&url, &policy);
                }
            }

            /// Private IPs embedded in IPv6 via 6to4/Teredo must always be caught.
            #[test]
            fn fuzz_6to4_always_catches_private(a in 0u8..=255, b in 0u8..=255) {
                // 2002:AABB:CCDD:: where AABB encodes private IPv4
                let priv_octets = [10u8, a]; // 10.x.x.x is always private
                let seg1 = ((priv_octets[0] as u16) << 8) | (priv_octets[1] as u16);
                let seg2 = ((b as u16) << 8) | 1u16;
                let addr = Ipv6Addr::new(0x2002, seg1, seg2, 0, 0, 0, 0, 0);
                prop_assert!(is_private_ipv6(&addr), "6to4 with 10.x.x.x must be private: {}", addr);
            }
        }
    }
}
