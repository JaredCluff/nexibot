//! Declarative outbound network policy engine.
//!
//! Enforces YAML-based allowlists for all outbound HTTP requests,
//! supporting hot-reload and managed policy integration.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};
use url::Url;

/// Network policy configuration version.
const CURRENT_POLICY_VERSION: u32 = 1;

/// A single endpoint rule in the network policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointRule {
    /// Allowed hostnames (exact match or wildcard prefix like "*.example.com").
    pub hosts: Vec<String>,
    /// Allowed ports (empty = allow all ports).
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Allowed HTTP methods (["*"] = all methods).
    #[serde(default = "default_allowed_methods")]
    pub allowed_methods: Vec<String>,
}

fn default_allowed_methods() -> Vec<String> {
    vec!["*".to_string()]
}

/// Default action when no endpoint rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DefaultAction {
    /// Deny unmatched requests (fail-closed).
    #[default]
    Deny,
    /// Allow unmatched requests (fail-open).
    Allow,
}

/// Network policy definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicy {
    /// Policy format version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Action for requests that don't match any endpoint rule.
    #[serde(default)]
    pub default_action: DefaultAction,
    /// Named endpoint rules.
    #[serde(default)]
    pub endpoints: HashMap<String, EndpointRule>,
}

fn default_version() -> u32 {
    CURRENT_POLICY_VERSION
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            version: CURRENT_POLICY_VERSION,
            default_action: DefaultAction::Allow, // Default to allow for backward compat
            endpoints: default_endpoints(),
        }
    }
}

/// Default endpoints that are always needed.
fn default_endpoints() -> HashMap<String, EndpointRule> {
    let mut endpoints = HashMap::new();

    endpoints.insert(
        "bridge_local".to_string(),
        EndpointRule {
            hosts: vec!["127.0.0.1".to_string(), "localhost".to_string()],
            ports: vec![18790],
            allowed_methods: vec!["*".to_string()],
        },
    );

    endpoints.insert(
        "anthropic".to_string(),
        EndpointRule {
            hosts: vec![
                "api.anthropic.com".to_string(),
                "statsig.anthropic.com".to_string(),
            ],
            ports: vec![443],
            allowed_methods: vec!["*".to_string()],
        },
    );

    endpoints
}

/// Reason a request was denied by network policy.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkPolicyDenied {
    pub url: String,
    pub method: String,
    pub reason: String,
}

impl std::fmt::Display for NetworkPolicyDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Network policy denied {} {} — {}",
            self.method, self.url, self.reason
        )
    }
}

impl std::error::Error for NetworkPolicyDenied {}

/// Network policy engine with thread-safe hot-reload support.
pub struct NetworkPolicyEngine {
    policy: Arc<RwLock<NetworkPolicy>>,
}

impl NetworkPolicyEngine {
    /// Create a new policy engine with the given policy.
    pub fn new(policy: NetworkPolicy) -> Self {
        info!(
            "[NET_POLICY] Initialized with {} endpoint rules, default_action={:?}",
            policy.endpoints.len(),
            policy.default_action,
        );
        Self {
            policy: Arc::new(RwLock::new(policy)),
        }
    }

    /// Create a new policy engine with default (allow-all) policy.
    pub fn permissive() -> Self {
        Self::new(NetworkPolicy::default())
    }

    /// Check whether a request is allowed by the current policy.
    pub async fn check_request(&self, url: &str, method: &str) -> Result<(), NetworkPolicyDenied> {
        let parsed = Url::parse(url).map_err(|e| NetworkPolicyDenied {
            url: url.to_string(),
            method: method.to_string(),
            reason: format!("Invalid URL: {}", e),
        })?;

        let host = parsed.host_str().unwrap_or("");
        let port = parsed.port_or_known_default().unwrap_or(0);
        let method_upper = method.to_uppercase();

        let policy = self.policy.read().await;

        // Reject URLs with IP addresses in private ranges
        // (DNS rebinding protection is handled by the SSRF module;
        //  this catches direct IP-based bypass attempts)
        if is_private_ip(host) {
            // Allow localhost for bridge_local rule
            let localhost_allowed = policy.endpoints.values().any(|rule| {
                rule.hosts
                    .iter()
                    .any(|h| h == "127.0.0.1" || h == "localhost" || h == "::1")
            });
            if !localhost_allowed || (host != "127.0.0.1" && host != "localhost" && host != "::1") {
                // Still check against rules - if there's an explicit rule for this IP, allow it
                let has_explicit_rule = policy
                    .endpoints
                    .values()
                    .any(|rule| rule.hosts.iter().any(|h| h == host));
                if !has_explicit_rule {
                    return Err(NetworkPolicyDenied {
                        url: url.to_string(),
                        method: method.to_string(),
                        reason: format!("Private IP address '{}' not in any endpoint rule", host),
                    });
                }
            }
        }

        for (rule_name, rule) in &policy.endpoints {
            if self.matches_rule(host, port, &method_upper, rule) {
                debug!(
                    "[NET_POLICY] Request allowed by rule '{}': {} {}",
                    rule_name, method, url
                );
                return Ok(());
            }
        }

        // No rule matched — apply default action
        match policy.default_action {
            DefaultAction::Allow => {
                debug!(
                    "[NET_POLICY] Request allowed by default action: {} {}",
                    method, url
                );
                Ok(())
            }
            DefaultAction::Deny => {
                debug!(
                    "[NET_POLICY] Request denied (no matching rule): {} {}",
                    method, url
                );
                Err(NetworkPolicyDenied {
                    url: url.to_string(),
                    method: method.to_string(),
                    reason: "No matching endpoint rule and default action is deny".to_string(),
                })
            }
        }
    }

    /// Check if a request matches a specific endpoint rule.
    fn matches_rule(&self, host: &str, port: u16, method: &str, rule: &EndpointRule) -> bool {
        // Check host
        let host_match = rule.hosts.iter().any(|allowed_host| {
            if allowed_host.starts_with("*.") {
                // Wildcard match: *.example.com matches foo.example.com
                let suffix = &allowed_host[1..]; // .example.com
                host.ends_with(suffix) && host.len() > suffix.len()
            } else {
                host == allowed_host
            }
        });

        if !host_match {
            return false;
        }

        // Check port (empty = allow all)
        if !rule.ports.is_empty() && !rule.ports.contains(&port) {
            return false;
        }

        // Check method
        if !rule.allowed_methods.contains(&"*".to_string())
            && !rule
                .allowed_methods
                .iter()
                .any(|m| m.to_uppercase() == method)
        {
            return false;
        }

        true
    }

    /// Hot-reload the policy.
    pub async fn reload(&self, new_policy: NetworkPolicy) {
        info!(
            "[NET_POLICY] Reloading policy: {} endpoint rules, default_action={:?}",
            new_policy.endpoints.len(),
            new_policy.default_action,
        );
        let mut policy = self.policy.write().await;
        *policy = new_policy;
    }

    /// Merge managed policy endpoints (server-provided floors).
    /// Server endpoints are additive — they can't be removed locally.
    pub async fn merge_managed_policy(&self, managed_endpoints: &HashMap<String, EndpointRule>) {
        let mut policy = self.policy.write().await;
        for (name, rule) in managed_endpoints {
            let key = format!("managed_{}", name);
            info!("[NET_POLICY] Merging managed endpoint: {}", key);
            policy.endpoints.insert(key, rule.clone());
        }
    }

    /// Get a snapshot of the current policy.
    pub async fn current_policy(&self) -> NetworkPolicy {
        self.policy.read().await.clone()
    }

    /// Load policy from a YAML file. Returns the previous policy on parse error.
    pub async fn load_from_file(&self, path: &std::path::Path) -> Result<()> {
        let content = tokio::fs::read_to_string(path).await?;
        let new_policy: NetworkPolicy = serde_yml::from_str(&content)?;
        self.reload(new_policy).await;
        Ok(())
    }
}

/// Check if a hostname looks like a private IP address.
///
/// Handles multiple IPv6 bypass vectors:
/// - IPv4-mapped addresses (`::ffff:127.0.0.1`)
/// - IPv4-compatible addresses (`::127.0.0.1`, deprecated but parseable)
/// - IPv6 link-local addresses (`fe80::1`)
/// - NAT64/DNS64 well-known prefix (`64:ff9b::` range)
fn is_private_ip(host: &str) -> bool {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        match ip {
            std::net::IpAddr::V4(v4) => is_private_v4(&v4),
            std::net::IpAddr::V6(v6) => {
                if v6.is_loopback() || v6.is_unspecified() {
                    return true;
                }
                // Check IPv4-mapped IPv6 addresses (::ffff:x.x.x.x)
                if let Some(mapped_v4) = v6.to_ipv4_mapped() {
                    return is_private_v4(&mapped_v4);
                }
                // Check deprecated IPv4-compatible addresses (::x.x.x.x)
                // These have the form 0:0:0:0:0:0:a.b.c.d
                let segments = v6.segments();
                if segments[0..5] == [0, 0, 0, 0, 0] && segments[5] == 0 {
                    let hi = (segments[6] >> 8) as u8;
                    let lo = (segments[6] & 0xff) as u8;
                    let hi2 = (segments[7] >> 8) as u8;
                    let lo2 = (segments[7] & 0xff) as u8;
                    let v4 = std::net::Ipv4Addr::new(hi, lo, hi2, lo2);
                    if is_private_v4(&v4) {
                        return true;
                    }
                }
                // Check IPv6 link-local (fe80::/10)
                if (segments[0] & 0xffc0) == 0xfe80 {
                    return true;
                }
                // Check NAT64/DNS64 well-known prefix (64:ff9b::/96)
                if segments[0] == 0x0064 && segments[1] == 0xff9b
                    && segments[2..6] == [0, 0, 0, 0]
                {
                    let hi = (segments[6] >> 8) as u8;
                    let lo = (segments[6] & 0xff) as u8;
                    let hi2 = (segments[7] >> 8) as u8;
                    let lo2 = (segments[7] & 0xff) as u8;
                    let v4 = std::net::Ipv4Addr::new(hi, lo, hi2, lo2);
                    if is_private_v4(&v4) {
                        return true;
                    }
                }
                false
            }
        }
    } else {
        false // Not an IP address, it's a hostname — DNS rebinding handled by SSRF module
    }
}

/// Check if an IPv4 address is in a private/reserved range.
fn is_private_v4(v4: &std::net::Ipv4Addr) -> bool {
    v4.is_loopback()      // 127.0.0.0/8
    || v4.is_private()    // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
    || v4.is_link_local() // 169.254.0.0/16
    || v4.is_unspecified() // 0.0.0.0
}

impl Default for NetworkPolicyEngine {
    fn default() -> Self {
        Self::permissive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> NetworkPolicy {
        let mut endpoints = HashMap::new();
        endpoints.insert(
            "api".to_string(),
            EndpointRule {
                hosts: vec!["api.example.com".to_string()],
                ports: vec![443],
                allowed_methods: vec!["GET".to_string(), "POST".to_string()],
            },
        );
        endpoints.insert(
            "wildcard".to_string(),
            EndpointRule {
                hosts: vec!["*.cdn.example.com".to_string()],
                ports: vec![],
                allowed_methods: vec!["GET".to_string()],
            },
        );
        endpoints.insert(
            "local".to_string(),
            EndpointRule {
                hosts: vec!["127.0.0.1".to_string()],
                ports: vec![18790],
                allowed_methods: vec!["*".to_string()],
            },
        );

        NetworkPolicy {
            version: 1,
            default_action: DefaultAction::Deny,
            endpoints,
        }
    }

    #[tokio::test]
    async fn test_allowed_exact_host() {
        let engine = NetworkPolicyEngine::new(test_policy());
        assert!(engine
            .check_request("https://api.example.com/v1/chat", "POST")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_denied_wrong_host() {
        let engine = NetworkPolicyEngine::new(test_policy());
        assert!(engine
            .check_request("https://evil.com/hack", "GET")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_denied_wrong_port() {
        let engine = NetworkPolicyEngine::new(test_policy());
        assert!(engine
            .check_request("http://api.example.com:8080/v1/chat", "GET")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_denied_wrong_method() {
        let engine = NetworkPolicyEngine::new(test_policy());
        assert!(engine
            .check_request("https://api.example.com/v1/chat", "DELETE")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_wildcard_host() {
        let engine = NetworkPolicyEngine::new(test_policy());
        assert!(engine
            .check_request("https://images.cdn.example.com/file.png", "GET")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_wildcard_host_no_match_root() {
        let engine = NetworkPolicyEngine::new(test_policy());
        // cdn.example.com itself should NOT match *.cdn.example.com
        assert!(engine
            .check_request("https://cdn.example.com/file.png", "GET")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_local_any_method() {
        let engine = NetworkPolicyEngine::new(test_policy());
        assert!(engine
            .check_request("http://127.0.0.1:18790/api/messages", "POST")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_default_allow_policy() {
        let engine = NetworkPolicyEngine::permissive();
        assert!(engine
            .check_request("https://any.random.site.com/page", "GET")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_reload_policy() {
        let engine = NetworkPolicyEngine::new(test_policy());

        // Initially denied
        assert!(engine
            .check_request("https://new-api.com/v1", "GET")
            .await
            .is_err());

        // Reload with new policy
        let mut new_policy = test_policy();
        new_policy.endpoints.insert(
            "new_api".to_string(),
            EndpointRule {
                hosts: vec!["new-api.com".to_string()],
                ports: vec![443],
                allowed_methods: vec!["*".to_string()],
            },
        );
        engine.reload(new_policy).await;

        // Now allowed
        assert!(engine
            .check_request("https://new-api.com/v1", "GET")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_merge_managed_policy() {
        let engine = NetworkPolicyEngine::new(test_policy());

        // Initially denied
        assert!(engine
            .check_request("https://managed-api.com/v1", "GET")
            .await
            .is_err());

        // Merge managed endpoint
        let mut managed = HashMap::new();
        managed.insert(
            "managed_api".to_string(),
            EndpointRule {
                hosts: vec!["managed-api.com".to_string()],
                ports: vec![443],
                allowed_methods: vec!["*".to_string()],
            },
        );
        engine.merge_managed_policy(&managed).await;

        // Now allowed
        assert!(engine
            .check_request("https://managed-api.com/v1", "GET")
            .await
            .is_ok());
    }

    #[test]
    fn test_policy_serialization() {
        let policy = test_policy();
        let yaml = serde_yml::to_string(&policy).unwrap();
        let deserialized: NetworkPolicy = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(deserialized.version, 1);
        assert_eq!(deserialized.default_action, DefaultAction::Deny);
        assert_eq!(deserialized.endpoints.len(), policy.endpoints.len());
    }

    #[test]
    fn test_default_policy_is_permissive() {
        let policy = NetworkPolicy::default();
        assert_eq!(policy.default_action, DefaultAction::Allow);
        assert!(policy.endpoints.contains_key("bridge_local"));
        assert!(policy.endpoints.contains_key("anthropic"));
    }

    #[tokio::test]
    async fn test_private_ip_without_explicit_rule() {
        let mut policy = test_policy();
        // Remove the localhost rule to test private IP blocking
        policy.endpoints.remove("local");
        let engine = NetworkPolicyEngine::new(policy);

        // Private IPs without explicit rules should be denied
        assert!(engine
            .check_request("http://192.168.1.1:8080/api", "GET")
            .await
            .is_err());
        assert!(engine
            .check_request("http://10.0.0.1/internal", "GET")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_private_ip_with_explicit_rule() {
        let engine = NetworkPolicyEngine::new(test_policy());

        // 127.0.0.1:18790 has an explicit rule in test_policy
        assert!(engine
            .check_request("http://127.0.0.1:18790/api/messages", "POST")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_ipv4_mapped_ipv6_bypass_blocked() {
        let mut policy = test_policy();
        // Remove the localhost rule so ::ffff:127.0.0.1 has no explicit match
        policy.endpoints.remove("local");
        let engine = NetworkPolicyEngine::new(policy);

        // IPv4-mapped IPv6 addresses must be detected as private
        assert!(engine
            .check_request("http://[::ffff:127.0.0.1]:8080/api", "GET")
            .await
            .is_err());
        assert!(engine
            .check_request("http://[::ffff:10.0.0.1]/internal", "GET")
            .await
            .is_err());
        assert!(engine
            .check_request("http://[::ffff:192.168.1.1]:3000/api", "GET")
            .await
            .is_err());
    }

    #[test]
    fn test_is_private_ip_ipv4_mapped_v6() {
        assert!(is_private_ip("::ffff:127.0.0.1"));
        assert!(is_private_ip("::ffff:10.0.0.1"));
        assert!(is_private_ip("::ffff:192.168.1.1"));
        assert!(!is_private_ip("::ffff:8.8.8.8"));
    }

    #[test]
    fn test_is_private_ip_link_local_v6() {
        // fe80::/10 link-local addresses must be detected as private
        assert!(is_private_ip("fe80::1"));
        assert!(is_private_ip("fe80::aede:48ff:fe00:1122"));
        // Non-link-local should pass
        assert!(!is_private_ip("2001:db8::1"));
    }

    #[test]
    fn test_is_private_ip_ipv4_compatible_v6() {
        // Deprecated IPv4-compatible addresses (::x.x.x.x)
        assert!(is_private_ip("::127.0.0.1"));
        assert!(is_private_ip("::10.0.0.1"));
        // Public IPs in compatible format should pass
        assert!(!is_private_ip("::8.8.8.8"));
    }

    #[test]
    fn test_is_private_ip_nat64() {
        // NAT64 well-known prefix (64:ff9b::/96) embedding private IPs
        assert!(is_private_ip("64:ff9b::127.0.0.1"));
        assert!(is_private_ip("64:ff9b::10.0.0.1"));
        assert!(is_private_ip("64:ff9b::192.168.1.1"));
        // NAT64 with public IP should pass
        assert!(!is_private_ip("64:ff9b::8.8.8.8"));
    }

    #[tokio::test]
    async fn test_empty_policy_denies_all() {
        let policy = NetworkPolicy {
            version: 1,
            default_action: DefaultAction::Deny,
            endpoints: HashMap::new(),
        };
        let engine = NetworkPolicyEngine::new(policy);
        assert!(engine.check_request("https://example.com", "GET").await.is_err());
    }

    #[tokio::test]
    async fn test_empty_ports_allows_any_port() {
        let mut endpoints = HashMap::new();
        endpoints.insert("any_port".to_string(), EndpointRule {
            hosts: vec!["example.com".to_string()],
            ports: vec![], // empty = any port
            allowed_methods: vec!["*".to_string()],
        });
        let policy = NetworkPolicy {
            version: 1,
            default_action: DefaultAction::Deny,
            endpoints,
        };
        let engine = NetworkPolicyEngine::new(policy);
        assert!(engine.check_request("https://example.com:8443/api", "GET").await.is_ok());
        assert!(engine.check_request("http://example.com:3000/api", "GET").await.is_ok());
    }

    #[tokio::test]
    async fn test_method_case_insensitive() {
        let engine = NetworkPolicyEngine::new(test_policy());
        // "api" rule allows GET and POST
        assert!(engine.check_request("https://api.example.com/v1", "get").await.is_ok());
        assert!(engine.check_request("https://api.example.com/v1", "Get").await.is_ok());
        assert!(engine.check_request("https://api.example.com/v1", "POST").await.is_ok());
    }

    #[tokio::test]
    async fn test_invalid_url_denied() {
        let engine = NetworkPolicyEngine::new(test_policy());
        assert!(engine.check_request("not-a-url", "GET").await.is_err());
        assert!(engine.check_request("", "GET").await.is_err());
    }

    #[test]
    fn test_network_policy_denied_display() {
        let denied = NetworkPolicyDenied {
            url: "https://evil.com".to_string(),
            method: "GET".to_string(),
            reason: "No matching rule".to_string(),
        };
        let display = format!("{}", denied);
        assert!(display.contains("GET"));
        assert!(display.contains("evil.com"));
    }
}
