//! Plugin host: loads, verifies, and dispatches events to security plugins.
//!
//! ## Built-in plugins
//! Registered via `register_builtin()` — implicitly trusted, no signing required.
//!
//! ## Rhai plugins
//! Loaded from `{dir}/{name}.rhai` + `{dir}/{name}.manifest.json`.
//! Signature is verified before the script is compiled:
//!   - SHA-256(script_bytes) must equal `manifest.content_sha256`
//!   - Ed25519 signature of the raw 32 SHA-256 bytes must verify against a key in `trusted_keys`
//!
//! ## Sandbox
//! The Rhai engine is configured with no module resolver (no imports), no `eval`,
//! and operation/string/depth limits to prevent runaway scripts.
//!
//! ## Dispatch
//! Each plugin receives the event via `on_event`. Built-in plugins use the async
//! trait; Rhai plugins are called synchronously in a blocking task with a 5-second timeout.
//! Plugins that error or time out fail open (their decisions are ignored).

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use rhai::{Dynamic, Engine as RhaiEngine};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use crate::config::PluginConfig;

use super::plugin::{
    PluginDecision, PluginManifest, PluginType, ShellPluginEvent, ShellSecurityPlugin,
};

// ---------------------------------------------------------------------------
// Rhai plugin wrapper
// ---------------------------------------------------------------------------

/// A loaded and verified Rhai script plugin.
struct RhaiPlugin {
    manifest: PluginManifest,
    script: String, // compiled + validated at load time
}

impl RhaiPlugin {
    /// Execute the plugin script for a single event.
    ///
    /// Runs in a fresh `Scope` each invocation; no state persists between calls.
    /// Returns parsed `PluginDecision` values from the Rhai array.
    fn call(&self, event_json: &str) -> Result<Vec<PluginDecision>> {
        let engine = build_rhai_engine();

        // Expose parse_json helper via a prelude evaluation.
        // All characters that Rhai treats as statement terminators or that
        // have special meaning inside a string literal must be escaped so
        // that a crafted event_json cannot break out of the string and inject
        // arbitrary Rhai statements.
        let prelude = format!(
            r#"let event = parse_json("{}");"#,
            event_json
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        );

        // Full script: prelude + user script + call
        let full_script = format!(
            "{prelude}\n{script}\nlet result = on_event(event);\nresult",
            prelude = prelude,
            script = self.script,
        );

        let result: Dynamic = engine
            .eval::<Dynamic>(&full_script)
            .map_err(|e| anyhow!("Rhai eval error in plugin '{}': {}", self.manifest.name, e))?;

        parse_rhai_decisions(result)
    }
}

/// Parse an array of Rhai maps into `Vec<PluginDecision>`.
fn parse_rhai_decisions(result: Dynamic) -> Result<Vec<PluginDecision>> {
    let mut decisions = Vec::new();

    let arr = match result.try_cast::<rhai::Array>() {
        Some(a) => a,
        None => return Ok(decisions), // non-array → treat as empty
    };

    for item in arr {
        let map = match item.try_cast::<rhai::Map>() {
            Some(m) => m,
            None => continue,
        };

        let decision_type = map
            .get("type")
            .and_then(|v| v.clone().try_cast::<String>())
            .unwrap_or_default();

        let decision = match decision_type.as_str() {
            "Deny" => {
                let reason = map
                    .get("reason")
                    .and_then(|v| v.clone().try_cast::<String>())
                    .unwrap_or_else(|| "Plugin denied command".to_string());
                PluginDecision::Deny { reason }
            }
            "RegisterSecret" => {
                let real = map
                    .get("real")
                    .and_then(|v| v.clone().try_cast::<String>())
                    .unwrap_or_default();
                let proxy = map
                    .get("proxy")
                    .and_then(|v| v.clone().try_cast::<String>())
                    .unwrap_or_default();
                if real.is_empty() || proxy.is_empty() {
                    continue;
                }
                PluginDecision::RegisterSecret { real, proxy }
            }
            _ => PluginDecision::Allow,
        };

        decisions.push(decision);
    }

    Ok(decisions)
}

// ---------------------------------------------------------------------------
// Build sandbox Rhai engine
// ---------------------------------------------------------------------------

fn build_rhai_engine() -> RhaiEngine {
    let mut engine = RhaiEngine::new();

    // No module resolver — deny all imports
    engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());

    // Disable eval (avoids script-within-script escapes)
    engine.disable_symbol("eval");

    // Strict resource limits
    engine.set_max_operations(100_000);
    engine.set_max_string_size(1_000_000);
    engine.set_max_call_levels(32);
    engine.set_max_array_size(10_000);
    engine.set_max_map_size(1_000);

    // Register parse_json helper
    engine.register_fn("parse_json", |s: String| -> Dynamic {
        serde_json::from_str::<serde_json::Value>(&s)
            .ok()
            .and_then(|v| rhai::serde::to_dynamic(v).ok())
            .unwrap_or(Dynamic::UNIT)
    });

    engine
}

// ---------------------------------------------------------------------------
// PluginHost
// ---------------------------------------------------------------------------

/// Hosts, verifies, and dispatches events to all registered security plugins.
pub struct PluginHost {
    builtin_plugins: RwLock<Vec<Arc<dyn ShellSecurityPlugin>>>,
    rhai_plugins: RwLock<Vec<RhaiPlugin>>,
    trusted_keys: RwLock<Vec<[u8; 32]>>,
    config: RwLock<PluginConfig>,
}

impl PluginHost {
    /// Create a new plugin host with the given configuration.
    pub fn new(config: PluginConfig) -> Self {
        let mut trusted_keys: Vec<[u8; 32]> = Vec::new();
        for hex_key in &config.trusted_keys {
            match decode_trusted_key(hex_key) {
                Ok(k) => trusted_keys.push(k),
                Err(e) => warn!(
                    "[NEXIGATE/PLUGINS] Invalid trusted_key '{}...': {}",
                    &hex_key[..hex_key.len().min(16)],
                    e
                ),
            }
        }

        Self {
            builtin_plugins: RwLock::new(Vec::new()),
            rhai_plugins: RwLock::new(Vec::new()),
            trusted_keys: RwLock::new(trusted_keys),
            config: RwLock::new(config),
        }
    }

    /// Register a built-in (compiled Rust) plugin. Always trusted.
    #[allow(dead_code)]
    pub async fn register_builtin(&self, plugin: Arc<dyn ShellSecurityPlugin>) {
        let name = plugin.name().to_string();
        let version = plugin.version().to_string();
        self.builtin_plugins.write().await.push(plugin);
        debug!(
            "[NEXIGATE/PLUGINS] Registered built-in plugin '{}' v{}",
            name, version
        );
    }

    /// Load a signed Rhai plugin.
    ///
    /// Expects: `{plugin_dir}/{name}.rhai` and `{plugin_dir}/{name}.manifest.json`
    #[allow(dead_code)]
    pub async fn load_signed_plugin(&self, plugin_dir: &Path, name: &str) -> Result<()> {
        let script_path = plugin_dir.join(format!("{}.rhai", name));
        let manifest_path = plugin_dir.join(format!("{}.manifest.json", name));

        let script_bytes = tokio::fs::read(&script_path).await.map_err(|e| {
            anyhow!(
                "Cannot read plugin script '{}': {}",
                script_path.display(),
                e
            )
        })?;

        let manifest_bytes = tokio::fs::read(&manifest_path).await.map_err(|e| {
            anyhow!(
                "Cannot read plugin manifest '{}': {}",
                manifest_path.display(),
                e
            )
        })?;

        let manifest: PluginManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| anyhow!("Invalid manifest JSON for plugin '{}': {}", name, e))?;

        if manifest.plugin_type != PluginType::Rhai {
            bail!("Plugin '{}' manifest type is not 'rhai'", name);
        }

        // Verify signature against trusted keys
        let trusted = self.trusted_keys.read().await;
        Self::verify_manifest(&manifest, &script_bytes, &trusted)?;
        drop(trusted);

        // Test-compile with sandbox to catch syntax errors early
        let script = String::from_utf8(script_bytes)
            .map_err(|_| anyhow!("Plugin '{}' script is not valid UTF-8", name))?;

        {
            let engine = build_rhai_engine();
            engine
                .compile(&script)
                .map_err(|e| anyhow!("Plugin '{}' syntax error: {}", name, e))?;
        }

        let plugin_name = manifest.name.clone();
        let plugin_version = manifest.version.clone();
        let plugin_author = manifest.author.clone();

        self.rhai_plugins
            .write()
            .await
            .push(RhaiPlugin { manifest, script });

        debug!(
            "[NEXIGATE/PLUGINS] Loaded Rhai plugin '{}' v{} by {}",
            plugin_name, plugin_version, plugin_author
        );
        Ok(())
    }

    /// Load all `*.manifest.json` plugins from the plugin directory.
    ///
    /// Returns a list of errors for any that failed; successful loads proceed.
    #[allow(dead_code)]
    pub async fn load_all_from_dir(&self, plugin_dir: &Path) -> Vec<anyhow::Error> {
        let mut errors = Vec::new();

        let mut entries = match tokio::fs::read_dir(plugin_dir).await {
            Ok(e) => e,
            Err(err) => {
                errors.push(anyhow!(
                    "Cannot read plugin dir '{}': {}",
                    plugin_dir.display(),
                    err
                ));
                return errors;
            }
        };

        let mut names_to_load = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if stem.ends_with(".manifest") {
                        let plugin_name = stem.trim_end_matches(".manifest").to_string();
                        names_to_load.push(plugin_name);
                    }
                }
            }
        }

        for name in names_to_load {
            if let Err(e) = self.load_signed_plugin(plugin_dir, &name).await {
                warn!("[NEXIGATE/PLUGINS] Failed to load plugin '{}': {}", name, e);
                errors.push(e);
            }
        }

        errors
    }

    /// Dispatch an event to all plugins (5s timeout per plugin).
    ///
    /// Fails open: plugins that error or time out are silently skipped.
    pub async fn dispatch<'a>(&self, event: &ShellPluginEvent<'a>) -> Vec<PluginDecision> {
        let cfg = self.config.read().await;
        if !cfg.enabled {
            return Vec::new();
        }
        drop(cfg);

        let mut decisions = Vec::new();

        // Dispatch to built-in plugins
        let builtins = self.builtin_plugins.read().await;
        for plugin in builtins.iter() {
            match timeout(Duration::from_secs(5), plugin.on_event(event)).await {
                Ok(d) => decisions.extend(d),
                Err(_) => {
                    warn!(
                        "[NEXIGATE/PLUGINS] Built-in plugin '{}' timed out for event '{}'",
                        plugin.name(),
                        event.type_name()
                    );
                }
            }
        }
        drop(builtins);

        // Dispatch to Rhai plugins
        let event_json = event.to_rhai_json().to_string();
        let rhai_plugins = self.rhai_plugins.read().await;
        for plugin in rhai_plugins.iter() {
            let script = plugin.script.clone();
            let manifest_name = plugin.manifest.name.clone();
            let event_json_clone = event_json.clone();

            // Run Rhai in a blocking task with timeout
            let result = timeout(
                Duration::from_secs(5),
                tokio::task::spawn_blocking(move || {
                    let p = RhaiPlugin {
                        manifest: PluginManifest {
                            name: manifest_name.clone(),
                            version: String::new(),
                            author: String::new(),
                            description: String::new(),
                            plugin_type: PluginType::Rhai,
                            content_sha256: String::new(),
                            signature: String::new(),
                            public_key: String::new(),
                        },
                        script,
                    };
                    p.call(&event_json_clone)
                }),
            )
            .await;

            match result {
                Ok(Ok(Ok(d))) => decisions.extend(d),
                Ok(Ok(Err(e))) => {
                    warn!(
                        "[NEXIGATE/PLUGINS] Rhai plugin '{}' runtime error: {}",
                        plugin.manifest.name, e
                    );
                }
                Ok(Err(e)) => {
                    warn!(
                        "[NEXIGATE/PLUGINS] Rhai plugin '{}' task panicked: {}",
                        plugin.manifest.name, e
                    );
                }
                Err(_) => {
                    warn!(
                        "[NEXIGATE/PLUGINS] Rhai plugin '{}' timed out",
                        plugin.manifest.name
                    );
                }
            }
        }

        decisions
    }

    /// Add a trusted Ed25519 public key (hex-encoded 32 raw bytes).
    #[allow(dead_code)]
    pub async fn add_trusted_key(&self, hex_key: &str) -> Result<()> {
        let key = decode_trusted_key(hex_key)?;
        self.trusted_keys.write().await.push(key);
        Ok(())
    }

    /// Total number of loaded plugins (built-in + Rhai).
    #[allow(dead_code)]
    pub async fn plugin_count(&self) -> usize {
        let b = self.builtin_plugins.read().await.len();
        let r = self.rhai_plugins.read().await.len();
        b + r
    }

    /// Hot-reload configuration (trusted_keys are updated; plugins already loaded stay loaded).
    pub async fn update_config(&self, new_config: PluginConfig) {
        let mut keys = self.trusted_keys.write().await;
        keys.clear();
        for hex_key in &new_config.trusted_keys {
            match decode_trusted_key(hex_key) {
                Ok(k) => keys.push(k),
                Err(e) => warn!("[NEXIGATE/PLUGINS] Invalid trusted_key on reload: {}", e),
            }
        }
        *self.config.write().await = new_config;
    }

    /// Verify a plugin manifest against script bytes and the trusted key set.
    #[allow(dead_code)]
    fn verify_manifest(
        manifest: &PluginManifest,
        script_bytes: &[u8],
        trusted_keys: &[[u8; 32]],
    ) -> Result<()> {
        // 1. Verify the script hash matches the manifest
        let actual_hash: [u8; 32] = Sha256::digest(script_bytes).into();
        let declared_hash = hex::decode(&manifest.content_sha256)
            .map_err(|_| anyhow!("Invalid content_sha256 hex"))?;

        if actual_hash.as_slice() != declared_hash.as_slice() {
            bail!("Plugin content hash mismatch — file may have been tampered with");
        }

        // 2. Verify the Ed25519 signature
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&manifest.signature)
            .map_err(|_| anyhow!("Invalid base64 signature"))?;

        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&manifest.public_key)
            .map_err(|_| anyhow!("Invalid base64 public_key"))?;

        let key_arr: [u8; 32] = key_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Public key must be exactly 32 bytes"))?;

        let sig_arr: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Signature must be exactly 64 bytes"))?;

        let vk = ed25519_dalek::VerifyingKey::from_bytes(&key_arr)
            .map_err(|e| anyhow!("Invalid verifying key: {}", e))?;

        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);

        vk.verify_strict(&actual_hash, &sig)
            .map_err(|_| anyhow!("Ed25519 signature verification failed"))?;

        // 3. Verify the key is in the trusted set
        if !trusted_keys.iter().any(|k| k == &key_arr) {
            bail!("Plugin public key is not in the trusted_keys list");
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn decode_trusted_key(hex_key: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hex_key).map_err(|_| anyhow!("trusted_key is not valid hex"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("trusted_key must be exactly 32 bytes (64 hex chars)"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use sha2::Sha256;

    // A minimal built-in plugin for testing
    struct EchoPlugin;

    #[async_trait::async_trait]
    impl ShellSecurityPlugin for EchoPlugin {
        fn name(&self) -> &str {
            "echo-plugin"
        }
        fn version(&self) -> &str {
            "1.0"
        }
        fn description(&self) -> &str {
            "Returns Allow for every event"
        }
        async fn on_event(&self, _event: &ShellPluginEvent<'_>) -> Vec<PluginDecision> {
            vec![PluginDecision::Allow]
        }
    }

    struct DenyPlugin;

    #[async_trait::async_trait]
    impl ShellSecurityPlugin for DenyPlugin {
        fn name(&self) -> &str {
            "deny-plugin"
        }
        fn version(&self) -> &str {
            "1.0"
        }
        fn description(&self) -> &str {
            "Always denies"
        }
        async fn on_event(&self, _event: &ShellPluginEvent<'_>) -> Vec<PluginDecision> {
            vec![PluginDecision::Deny {
                reason: "test deny".to_string(),
            }]
        }
    }

    struct RegisterPlugin;

    #[async_trait::async_trait]
    impl ShellSecurityPlugin for RegisterPlugin {
        fn name(&self) -> &str {
            "register-plugin"
        }
        fn version(&self) -> &str {
            "1.0"
        }
        fn description(&self) -> &str {
            "Registers a test secret"
        }
        async fn on_event(&self, _event: &ShellPluginEvent<'_>) -> Vec<PluginDecision> {
            vec![PluginDecision::RegisterSecret {
                real: "REAL_FROM_PLUGIN".to_string(),
                proxy: "PROXY_FROM_PLUGIN".to_string(),
            }]
        }
    }

    fn disabled_config() -> PluginConfig {
        PluginConfig {
            enabled: false,
            plugin_dir: None,
            trusted_keys: vec![],
        }
    }

    fn enabled_config() -> PluginConfig {
        PluginConfig {
            enabled: true,
            plugin_dir: None,
            trusted_keys: vec![],
        }
    }

    fn make_event<'a>() -> ShellPluginEvent<'a> {
        ShellPluginEvent::SessionCreated {
            session_id: "test-session",
            agent_id: "test-agent",
            timestamp_ms: 0,
        }
    }

    fn make_command_event<'a>() -> ShellPluginEvent<'a> {
        ShellPluginEvent::CommandObserved {
            session_id: "test-session",
            agent_id: "test-agent",
            raw_command: "echo hello",
            filtered_command: "echo hello",
            timestamp_ms: 0,
        }
    }

    fn sign_script(script: &[u8]) -> (PluginManifest, [u8; 32]) {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        let hash: [u8; 32] = Sha256::digest(script).into();
        let sig = signing_key.sign(&hash);

        let manifest = PluginManifest {
            name: "test_rhai".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            description: "test rhai plugin".to_string(),
            plugin_type: PluginType::Rhai,
            content_sha256: hex::encode(hash),
            signature: base64::engine::general_purpose::STANDARD.encode(sig.to_bytes()),
            public_key: base64::engine::general_purpose::STANDARD.encode(verifying_key.as_bytes()),
        };
        (manifest, *verifying_key.as_bytes())
    }

    #[tokio::test]
    async fn test_disabled_plugin_system_returns_empty() {
        let host = PluginHost::new(disabled_config());
        host.register_builtin(Arc::new(DenyPlugin)).await;
        let decisions = host.dispatch(&make_event()).await;
        // disabled → no dispatch → empty
        assert!(decisions.is_empty());
    }

    #[tokio::test]
    async fn test_builtin_plugin_receives_event() {
        let host = PluginHost::new(enabled_config());
        host.register_builtin(Arc::new(EchoPlugin)).await;
        let decisions = host.dispatch(&make_event()).await;
        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0], PluginDecision::Allow));
    }

    #[tokio::test]
    async fn test_deny_decision_returned() {
        let host = PluginHost::new(enabled_config());
        host.register_builtin(Arc::new(DenyPlugin)).await;
        let decisions = host.dispatch(&make_command_event()).await;
        assert!(
            decisions
                .iter()
                .any(|d| matches!(d, PluginDecision::Deny { .. })),
            "expected Deny decision"
        );
    }

    #[tokio::test]
    async fn test_register_secret_decision_returned() {
        let host = PluginHost::new(enabled_config());
        host.register_builtin(Arc::new(RegisterPlugin)).await;
        let decisions = host.dispatch(&make_event()).await;
        assert!(
            decisions
                .iter()
                .any(|d| matches!(d, PluginDecision::RegisterSecret { .. })),
            "expected RegisterSecret decision"
        );
    }

    #[tokio::test]
    async fn test_plugin_count_correct() {
        let host = PluginHost::new(enabled_config());
        assert_eq!(host.plugin_count().await, 0);
        host.register_builtin(Arc::new(EchoPlugin)).await;
        assert_eq!(host.plugin_count().await, 1);
        host.register_builtin(Arc::new(DenyPlugin)).await;
        assert_eq!(host.plugin_count().await, 2);
    }

    #[tokio::test]
    async fn test_unsigned_rhai_load_fails() {
        let host = PluginHost::new(enabled_config());
        let dir = tempfile::tempdir().unwrap();

        // Write a script without a manifest → load should fail
        let script_path = dir.path().join("unsigned.rhai");
        tokio::fs::write(&script_path, b"fn on_event(e) { [] }")
            .await
            .unwrap();

        let result = host.load_signed_plugin(dir.path(), "unsigned").await;
        assert!(result.is_err(), "unsigned plugin should fail to load");
    }

    #[tokio::test]
    async fn test_valid_signed_rhai_loads() {
        let script = b"fn on_event(e) { [] }";
        let (manifest, key_bytes) = sign_script(script);

        let mut cfg = enabled_config();
        cfg.trusted_keys = vec![hex::encode(key_bytes)];

        let host = PluginHost::new(cfg);

        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("myplugin.rhai"), script)
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("myplugin.manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();

        host.load_signed_plugin(dir.path(), "myplugin")
            .await
            .unwrap();
        assert_eq!(host.plugin_count().await, 1);
    }

    #[tokio::test]
    async fn test_tampered_rhai_rejected() {
        let script = b"fn on_event(e) { [] }";
        let (manifest, key_bytes) = sign_script(script);

        let mut cfg = enabled_config();
        cfg.trusted_keys = vec![hex::encode(key_bytes)];

        let host = PluginHost::new(cfg);

        let dir = tempfile::tempdir().unwrap();
        // Write tampered script (different content than what was signed)
        tokio::fs::write(
            dir.path().join("evil.rhai"),
            b"fn on_event(e) { [#{ type: \"Deny\", reason: \"hacked\" }] }",
        )
        .await
        .unwrap();
        tokio::fs::write(
            dir.path().join("evil.manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let result = host.load_signed_plugin(dir.path(), "evil").await;
        assert!(result.is_err(), "tampered script should be rejected");
    }

    #[tokio::test]
    async fn test_untrusted_key_rejected() {
        let script = b"fn on_event(e) { [] }";
        let (manifest, _key_bytes) = sign_script(script);

        // Don't add the key to trusted_keys
        let cfg = enabled_config(); // empty trusted_keys

        let host = PluginHost::new(cfg);

        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("untrusted.rhai"), script)
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("untrusted.manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let result = host.load_signed_plugin(dir.path(), "untrusted").await;
        assert!(result.is_err(), "untrusted key should be rejected");
    }

    #[tokio::test]
    async fn test_rhai_deny_decision_dispatched() {
        let script = b"fn on_event(e) { [#{ type: \"Deny\", reason: \"sudo blocked\" }] }";
        let (manifest, key_bytes) = sign_script(script);

        let mut cfg = enabled_config();
        cfg.trusted_keys = vec![hex::encode(key_bytes)];

        let host = PluginHost::new(cfg);

        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("deny.rhai"), script)
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("deny.manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();

        host.load_signed_plugin(dir.path(), "deny").await.unwrap();

        let decisions = host.dispatch(&make_command_event()).await;
        assert!(
            decisions
                .iter()
                .any(|d| matches!(d, PluginDecision::Deny { .. })),
            "expected Deny from Rhai plugin, got: {:?}",
            decisions
        );
    }

    #[tokio::test]
    async fn test_load_all_from_dir_partial_success() {
        // 1 valid plugin + 1 invalid (no manifest)
        let script = b"fn on_event(e) { [] }";
        let (manifest, key_bytes) = sign_script(script);

        let mut cfg = enabled_config();
        cfg.trusted_keys = vec![hex::encode(key_bytes)];

        let host = PluginHost::new(cfg);
        let dir = tempfile::tempdir().unwrap();

        // Valid plugin
        tokio::fs::write(dir.path().join("valid.rhai"), script)
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("valid.manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();

        // Invalid: manifest present but no .rhai
        let bad_manifest = PluginManifest {
            name: "missing_script".to_string(),
            version: "1.0".to_string(),
            author: "test".to_string(),
            description: "".to_string(),
            plugin_type: PluginType::Rhai,
            content_sha256: "00".repeat(32),
            signature: "aGk=".to_string(), // "hi" in base64
            public_key: "aGk=".to_string(),
        };
        tokio::fs::write(
            dir.path().join("missing_script.manifest.json"),
            serde_json::to_vec(&bad_manifest).unwrap(),
        )
        .await
        .unwrap();

        let errors = host.load_all_from_dir(dir.path()).await;
        // Exactly 1 error (the invalid plugin)
        assert_eq!(errors.len(), 1, "expected 1 error, got {}", errors.len());
        // The valid plugin was loaded
        assert_eq!(host.plugin_count().await, 1);
    }

    #[tokio::test]
    async fn test_broadcast_to_all_plugins() {
        let host = PluginHost::new(enabled_config());
        host.register_builtin(Arc::new(EchoPlugin)).await;
        host.register_builtin(Arc::new(DenyPlugin)).await;
        let decisions = host.dispatch(&make_command_event()).await;
        // Two plugins → at least 2 decisions
        assert!(decisions.len() >= 2, "expected decisions from all plugins");
    }
}
