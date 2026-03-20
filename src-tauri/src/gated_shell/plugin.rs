//! Shell security plugin system for NexiGate.
//!
//! Defines the `ShellSecurityPlugin` async trait, the `ShellPluginEvent` enum
//! dispatched to plugins, the `PluginDecision` return type, and the `PluginManifest`
//! used for signed Rhai plugin verification.
//!
//! # Safety contract
//!
//! - `raw_output` is **NEVER** included in events dispatched to Rhai plugins.
//!   Only `filtered_output` (proxy tokens) is visible to external scripts.
//! - Built-in trait plugins receive the full `OutputObserved` event and are
//!   responsible for handling `raw_output` without logging or exfiltrating it.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ShellPluginEvent
// ---------------------------------------------------------------------------

/// Events dispatched to all registered plugins.
///
/// Lifetime `'a` avoids cloning strings per dispatch — events are borrowed views.
#[derive(Debug)]
pub enum ShellPluginEvent<'a> {
    /// A new shell session was created.
    SessionCreated {
        session_id: &'a str,
        agent_id: &'a str,
        timestamp_ms: u64,
    },
    /// A shell session was closed.
    SessionClosed {
        session_id: &'a str,
        timestamp_ms: u64,
    },
    /// A command is about to be executed (post-inbound-filter, pre-PTY).
    ///
    /// Plugins may veto execution by returning `PluginDecision::Deny`.
    CommandObserved {
        session_id: &'a str,
        agent_id: &'a str,
        /// What the LLM sent (proxy tokens).
        raw_command: &'a str,
        /// What the PTY will receive (real values after inbound filter).
        filtered_command: &'a str,
        timestamp_ms: u64,
    },
    /// A command has completed and output is ready.
    ///
    /// IMPORTANT: `raw_output` is the real PTY output containing real secret values.
    /// It is provided to built-in trait plugins only. Rhai plugins receive this event
    /// with `raw_output` set to an empty string — only `filtered_output` is passed.
    OutputObserved {
        session_id: &'a str,
        agent_id: &'a str,
        /// Real PTY output — DO NOT log or exfiltrate. Empty for Rhai plugins.
        #[allow(dead_code)]
        raw_output: &'a str,
        /// Proxy-token output (what the LLM receives).
        filtered_output: &'a str,
        exit_code: i32,
        duration_ms: u64,
    },
    /// A new secret was dynamically discovered and registered.
    SecretDiscovered {
        session_id: &'a str,
        proxy_token: &'a str,
        format: &'a str,
        source: &'a str,
    },
    /// A command was denied by the policy engine.
    PolicyDenied {
        session_id: &'a str,
        agent_id: &'a str,
        command_preview: &'a str,
        reason: &'a str,
    },
}

impl<'a> ShellPluginEvent<'a> {
    /// Event type name for logging and Tauri event payloads.
    pub fn type_name(&self) -> &'static str {
        match self {
            ShellPluginEvent::SessionCreated { .. } => "SessionCreated",
            ShellPluginEvent::SessionClosed { .. } => "SessionClosed",
            ShellPluginEvent::CommandObserved { .. } => "CommandObserved",
            ShellPluginEvent::OutputObserved { .. } => "OutputObserved",
            ShellPluginEvent::SecretDiscovered { .. } => "SecretDiscovered",
            ShellPluginEvent::PolicyDenied { .. } => "PolicyDenied",
        }
    }

    /// Serialize to JSON for Rhai plugin dispatch.
    ///
    /// IMPORTANT: `raw_output` is always replaced with `""` in the serialized form.
    pub fn to_rhai_json(&self) -> serde_json::Value {
        match self {
            ShellPluginEvent::SessionCreated { session_id, agent_id, timestamp_ms } => {
                serde_json::json!({
                    "type": "SessionCreated",
                    "session_id": session_id,
                    "agent_id": agent_id,
                    "timestamp_ms": timestamp_ms,
                })
            }
            ShellPluginEvent::SessionClosed { session_id, timestamp_ms } => {
                serde_json::json!({
                    "type": "SessionClosed",
                    "session_id": session_id,
                    "timestamp_ms": timestamp_ms,
                })
            }
            ShellPluginEvent::CommandObserved {
                session_id, agent_id, raw_command, filtered_command, timestamp_ms
            } => {
                serde_json::json!({
                    "type": "CommandObserved",
                    "session_id": session_id,
                    "agent_id": agent_id,
                    "raw_command": raw_command,
                    "filtered_command": filtered_command,
                    "timestamp_ms": timestamp_ms,
                })
            }
            ShellPluginEvent::OutputObserved {
                session_id, agent_id, filtered_output, exit_code, duration_ms, ..
                // raw_output intentionally omitted
            } => {
                serde_json::json!({
                    "type": "OutputObserved",
                    "session_id": session_id,
                    "agent_id": agent_id,
                    "raw_output": "",        // NEVER expose to Rhai
                    "filtered_output": filtered_output,
                    "exit_code": exit_code,
                    "duration_ms": duration_ms,
                })
            }
            ShellPluginEvent::SecretDiscovered { session_id, proxy_token, format, source } => {
                serde_json::json!({
                    "type": "SecretDiscovered",
                    "session_id": session_id,
                    "proxy_token": proxy_token,
                    "format": format,
                    "source": source,
                })
            }
            ShellPluginEvent::PolicyDenied {
                session_id, agent_id, command_preview, reason
            } => {
                serde_json::json!({
                    "type": "PolicyDenied",
                    "session_id": session_id,
                    "agent_id": agent_id,
                    "command_preview": command_preview,
                    "reason": reason,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PluginDecision
// ---------------------------------------------------------------------------

/// Decisions a plugin can return from `on_event`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginDecision {
    /// Observer only — take no action. Default.
    Allow,
    /// Veto the command. Only valid in response to `CommandObserved`.
    Deny { reason: String },
    /// Register an additional secret with the filter layer.
    RegisterSecret { real: String, proxy: String },
}

// ---------------------------------------------------------------------------
// ShellSecurityPlugin trait
// ---------------------------------------------------------------------------

/// A compiled (built-in) shell security plugin.
///
/// Implement this trait to observe shell events, veto commands, or register
/// additional secrets. Built-in plugins have full access to `raw_output`.
#[async_trait]
pub trait ShellSecurityPlugin: Send + Sync {
    /// Unique plugin name (used in logs and Tauri events).
    fn name(&self) -> &str;

    /// Plugin version string.
    #[allow(dead_code)]
    fn version(&self) -> &str;

    /// Human-readable description.
    #[allow(dead_code)]
    fn description(&self) -> &str;

    /// Handle a shell event and return zero or more decisions.
    async fn on_event(&self, event: &ShellPluginEvent<'_>) -> Vec<PluginDecision>;
}

// ---------------------------------------------------------------------------
// PluginManifest
// ---------------------------------------------------------------------------

/// Plugin type discriminant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    Rhai,
    Builtin,
}

/// Manifest for a signed Rhai plugin.
///
/// Stored as `{plugin_name}.manifest.json` alongside `{plugin_name}.rhai`.
///
/// # Signing convention
///
/// 1. `content_sha256` = hex-encoded SHA-256 of the `.rhai` script file bytes
/// 2. `signature` = base64-encoded Ed25519 signature of the raw 32 SHA-256 bytes
/// 3. Verification: `VerifyingKey::verify_strict(&sha256_bytes, &Signature)`
/// 4. The verifying key must match an entry in `config.gated_shell.plugins.trusted_keys`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub plugin_type: PluginType,
    /// Hex-encoded SHA-256 of the .rhai script content.
    pub content_sha256: String,
    /// Base64-encoded Ed25519 signature of the SHA-256 bytes.
    pub signature: String,
    /// Base64-encoded Ed25519 verifying key (32 raw bytes).
    pub public_key: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use sha2::{Digest, Sha256};

    fn make_manifest_for(script: &[u8]) -> (PluginManifest, SigningKey) {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();

        let hash: [u8; 32] = Sha256::digest(script).into();
        let content_sha256 = hex::encode(hash);
        let signature_bytes = signing_key.sign(&hash);

        let manifest = PluginManifest {
            name: "test_plugin".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            description: "A test plugin".to_string(),
            plugin_type: PluginType::Rhai,
            content_sha256,
            signature: base64::engine::general_purpose::STANDARD.encode(signature_bytes.to_bytes()),
            public_key: base64::engine::general_purpose::STANDARD.encode(verifying_key.as_bytes()),
        };
        (manifest, signing_key)
    }

    #[test]
    fn test_manifest_round_trips_json() {
        let script = b"fn on_event(e) { [] }";
        let (manifest, _) = make_manifest_for(script);
        let json = serde_json::to_string(&manifest).expect("serialize");
        let back: PluginManifest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.name, manifest.name);
        assert_eq!(back.signature, manifest.signature);
        assert_eq!(back.content_sha256, manifest.content_sha256);
    }

    #[test]
    fn test_plugin_type_serializes() {
        let t = PluginType::Rhai;
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(s, r#""rhai""#);
        let t2: PluginType = serde_json::from_str(&s).unwrap();
        assert_eq!(t2, PluginType::Rhai);
    }

    #[test]
    fn test_valid_signature_verifies() {
        use ed25519_dalek::Verifier;

        let script = b"fn on_event(e) { [] }";
        let (manifest, _) = make_manifest_for(script);

        let hash = hex::decode(&manifest.content_sha256).expect("decode sha256");
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&manifest.signature)
            .expect("decode sig");
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&manifest.public_key)
            .expect("decode key");

        let vk = ed25519_dalek::VerifyingKey::from_bytes(
            key_bytes.as_slice().try_into().expect("32 bytes"),
        )
        .expect("verifying key");
        let sig = ed25519_dalek::Signature::from_bytes(
            sig_bytes.as_slice().try_into().expect("64 bytes"),
        );

        assert!(
            vk.verify(&hash, &sig).is_ok(),
            "valid signature should verify"
        );
    }

    #[test]
    fn test_wrong_signature_rejected() {
        use ed25519_dalek::Verifier;

        let script = b"fn on_event(e) { [] }";
        let (manifest, _) = make_manifest_for(script);

        // Tamper with the script hash — compute hash of different content
        let other_hash = sha2::Sha256::digest(b"different content");

        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&manifest.public_key)
            .expect("decode key");
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&manifest.signature)
            .expect("decode sig");

        let vk = ed25519_dalek::VerifyingKey::from_bytes(
            key_bytes.as_slice().try_into().expect("32 bytes"),
        )
        .expect("verifying key");
        let sig = ed25519_dalek::Signature::from_bytes(
            sig_bytes.as_slice().try_into().expect("64 bytes"),
        );

        // Should fail because hash doesn't match what was signed
        assert!(
            vk.verify(other_hash.as_slice(), &sig).is_err(),
            "tampered hash should fail verification"
        );
    }

    #[test]
    fn test_plugin_decision_allow_serializes() {
        let d = PluginDecision::Allow;
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("Allow"));
    }

    #[test]
    fn test_plugin_decision_deny_serializes() {
        let d = PluginDecision::Deny {
            reason: "not allowed".to_string(),
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("Deny"));
        assert!(s.contains("not allowed"));
    }

    #[test]
    fn test_plugin_decision_register_secret_serializes() {
        let d = PluginDecision::RegisterSecret {
            real: "real_val".to_string(),
            proxy: "proxy_val".to_string(),
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("RegisterSecret"));
        assert!(s.contains("real_val"));
    }

    #[test]
    fn test_rhai_json_omits_raw_output() {
        let event = ShellPluginEvent::OutputObserved {
            session_id: "sess1",
            agent_id: "agent1",
            raw_output: "sk-ant-REALSECRET", // should NOT appear in JSON
            filtered_output: "sk-ant-NEXIGATE-proxy",
            exit_code: 0,
            duration_ms: 10,
        };
        let json = event.to_rhai_json().to_string();
        assert!(
            !json.contains("sk-ant-REALSECRET"),
            "raw_output must not appear in Rhai JSON: {}",
            json
        );
        assert!(
            json.contains("sk-ant-NEXIGATE-proxy"),
            "filtered_output must appear in Rhai JSON: {}",
            json
        );
    }

    #[test]
    fn test_event_type_names() {
        let e = ShellPluginEvent::SessionCreated {
            session_id: "s",
            agent_id: "a",
            timestamp_ms: 0,
        };
        assert_eq!(e.type_name(), "SessionCreated");
        let e2 = ShellPluginEvent::CommandObserved {
            session_id: "s",
            agent_id: "a",
            raw_command: "ls",
            filtered_command: "ls",
            timestamp_ms: 0,
        };
        assert_eq!(e2.type_name(), "CommandObserved");
    }
}
