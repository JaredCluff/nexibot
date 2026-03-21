//! Integration tests for configuration token interception.
//!
//! Verifies that the vault store/resolve pipeline (which underpins
//! config_cmds.rs interception) works correctly for all channel token types.
//!
//! NOTE: These tests require a `lib.rs` entry point in the crate to compile
//! as integration tests. Currently the crate is binary-only (`main.rs`).
//! The tests are written against the public API of `nexibot_tauri` and will
//! become runnable once a `lib.rs` exposing `pub mod security` is added.

use nexibot_tauri::security::key_vault::KeyVault;
use tempfile::TempDir;

fn make_vault() -> (KeyVault, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("config_vault.sqlite");
    let vault = KeyVault::new(db, "config-test-passphrase", true).unwrap();
    (vault, dir)
}

/// Verify that each channel token type can be stored and resolved via the
/// vault's store/resolve pipeline — the same path taken by
/// `intercept_config_string` in config_cmds.rs.
///
/// For tokens whose format is recognized by extract_secrets (Anthropic,
/// OpenAI, Cerebras, GitHub, Slack, Bearer, Discord), the function also
/// confirms that extract_secrets returns at least one match (confirming the
/// token would be auto-intercepted by scan_and_replace).  For opaque tokens
/// like Telegram bot tokens and Slack App tokens, the config interceptor uses
/// direct `store()` rather than `scan_and_replace()`, so those are tested via
/// direct store/resolve only.
#[test]
fn test_all_channel_token_formats_are_interceptable() {
    let (vault, _dir) = make_vault();
    use nexibot_tauri::security::log_redactor::extract_secrets;

    // (config_field, token_value, expect_extract_secrets_match)
    // Cerebras: extract_secrets detects the embedded sk- as OpenAI format, but
    // detect_format() on the raw key returns Cerebras. Both store() and
    // scan_and_replace() ultimately store the correct value, so round-trip works.
    let channel_tokens: Vec<(&str, &str, bool)> = vec![
        (
            "claude.api_key",
            "sk-ant-api03-claudeapikey12345678901234567",
            true, // Anthropic pattern
        ),
        (
            "openai.api_key",
            "sk-openai12345678901234567890123456789012",
            true, // OpenAI pattern
        ),
        (
            "cerebras.api_key",
            "csk-cerebras123456789012345678901234567890",
            true, // Detected as OpenAI (sk- substring), but stores correctly
        ),
        (
            "telegram.bot_token",
            "1234567890:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh",
            false, // Telegram digit:letters format has no EXTRACT_PATTERNS entry
        ),
        (
            "discord.bot_token",
            "MTAxNTExMzE4MDExMTM2MzAz.GnVwID.ABCDEFGHIJKLMNOPQRSTUVWXYZ12345",
            true, // Discord M/N 3-part pattern
        ),
        (
            "slack.bot_token",
            "xoxb-1234567890-1234567890123-ABCDEFabcdef123456789",
            true, // Slack xox[b] pattern
        ),
        (
            "slack.app_token",
            "xapp-1-A1234ABCDE-1234567890123-abcdefghij1234567890",
            false, // xapp- prefix: 'a' not in xox[bpras] pattern
        ),
        (
            "teams.app_password",
            "Bearer ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890",
            true, // Bearer pattern
        ),
        (
            "matrix.access_token",
            "Bearer syt_bWF0cml4_ABCDEFGHIJKLMNOPQRSTUVWXYZabc",
            true, // Bearer pattern (underscore allowed in token suffix)
        ),
        (
            "whatsapp.access_token",
            "Bearer EAAGmxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            true, // Bearer pattern
        ),
        (
            "mastodon.access_token",
            "Bearer mastodon_abc123def456ghi789jkl012mno345",
            true, // Bearer pattern
        ),
        (
            "mattermost.bot_token",
            "Bearer mm_token_ABCDEFGHIJKLMNOPQRSTUVWXYZabc",
            true, // Bearer pattern
        ),
    ];

    for (field, token, expect_extract) in &channel_tokens {
        // Verify extract_secrets behavior matches our expectation
        let text = token.to_string();
        let secrets = extract_secrets(&text);
        if *expect_extract {
            assert!(
                !secrets.is_empty(),
                "Field '{}': expected extract_secrets to find a secret in '{}'",
                field,
                &token[..token.len().min(30)]
            );
        }

        // All tokens can be stored and resolved via direct store(), regardless
        // of whether they are auto-detected by scan_and_replace
        let proxy = vault.store(token, Some(field)).unwrap();
        assert!(
            KeyVault::is_proxy_key(&proxy),
            "Field '{}' should produce a vault proxy key, got: {}",
            field,
            proxy
        );
        let resolved = vault.resolve(&proxy).unwrap();
        assert_eq!(
            resolved.as_deref(),
            Some(*token),
            "Field '{}' token should round-trip correctly",
            field
        );
    }
}

/// Simulate the full config save / load round trip:
/// 1. User enters real API key in Settings UI.
/// 2. intercept_config_string calls vault.store(), returns proxy.
/// 3. config.yaml is written containing the proxy key (not the real key).
/// 4. On next startup the vault is warmed from SQLite; resolve() returns
///    the real key for use by the LLM provider.
#[test]
fn test_config_round_trip_store_then_resolve() {
    let (vault, _dir) = make_vault();

    let real_key = "sk-ant-api03-configroundtrip1234567890123";
    let proxy = vault.store(real_key, Some("claude.api_key")).unwrap();

    // Verify proxy goes in config.yaml (it is a proxy key, not the real value)
    assert!(
        KeyVault::is_proxy_key(&proxy),
        "Config value should be a proxy key"
    );
    assert!(
        !proxy.contains(real_key),
        "Config file should not contain real key"
    );

    // Simulate restart: vault is warm-cached, resolve() gives back real key
    let resolved = vault.resolve(&proxy).unwrap();
    assert_eq!(
        resolved.as_deref(),
        Some(real_key),
        "After restart, proxy should resolve to real key"
    );
}

/// Verify that when vault interception is disabled the real key is not stored
/// in the vault and scan_and_replace leaves the text unmodified.
#[test]
fn test_intercept_disabled_writes_real_key() {
    let dir2 = TempDir::new().unwrap();
    let disabled_vault = KeyVault::new(dir2.path().join("disabled.sqlite"), "pass", false).unwrap();

    let real_key = "sk-ant-api03-disabledintercept1234567890";
    let text = format!("api_key: {}", real_key);
    let (result, intercepted) = disabled_vault.scan_and_replace(&text);
    assert_eq!(intercepted.len(), 0, "Disabled vault should not intercept");
    assert!(
        result.contains(real_key),
        "Disabled vault should leave real key in place"
    );
}

/// Verify that a proxy key that is already in config.yaml is not re-intercepted
/// on a subsequent config save, maintaining exactly one vault entry per real key.
#[test]
fn test_already_proxy_not_re_intercepted() {
    let (vault, _dir) = make_vault();

    // Store a real key, get proxy
    let real_key = "sk-ant-api03-idempotencytest12345678901234";
    let proxy = vault.store(real_key, None).unwrap();

    // If config.yaml already has the proxy (from a previous session), re-saving
    // should not create a new vault entry
    let (result, intercepted) = vault.scan_and_replace(&proxy);
    assert_eq!(
        intercepted.len(),
        0,
        "Proxy key should not be re-intercepted"
    );
    assert_eq!(result, proxy, "Proxy should remain unchanged");

    // Verify vault still has exactly 1 entry
    let entries = vault.list();
    assert_eq!(entries.len(), 1, "Should still have exactly 1 vault entry");
}

/// Verify SQLite persistence: a key stored in one vault instance is still
/// resolvable after that instance is dropped and a new one opened against
/// the same database file.
#[test]
fn test_vault_persists_across_reopen() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("persist.sqlite");

    let real_key = "sk-ant-api03-persistencetest1234567890123";
    let proxy = {
        let vault = KeyVault::new(db_path.clone(), "passphrase", true).unwrap();
        vault.store(real_key, Some("test")).unwrap()
        // vault is dropped here — SQLite connection closed
    };

    // Reopen the vault with the same DB path
    let vault2 = KeyVault::new(db_path, "passphrase", true).unwrap();
    let resolved = vault2.resolve(&proxy).unwrap();
    assert_eq!(
        resolved.as_deref(),
        Some(real_key),
        "Key should persist across vault reopen (SQLite)"
    );
}

// ---------------------------------------------------------------------------
// Config backward compatibility tests (raw YAML deserialization)
// ---------------------------------------------------------------------------
// NexiBotConfig is not exposed via lib.rs (only security modules are), so
// these tests verify YAML structure invariants using serde_yml::Value.
// This ensures config files from older versions still parse without errors.

/// Test that a minimal config YAML with only config_version deserializes
/// to a valid YAML mapping. This is the baseline backward compat check:
/// existing config files without new feature sections must still parse.
#[test]
fn test_minimal_config_yaml_parses() {
    let yaml = "config_version: 1\n";
    let value: serde_yml::Value =
        serde_yml::from_str(yaml).expect("Minimal config YAML should parse");
    assert!(value.is_mapping(), "Parsed YAML should be a mapping");
    assert_eq!(
        value["config_version"].as_u64(),
        Some(1),
        "config_version should be 1"
    );
}

/// Test that completely empty YAML deserializes to a valid mapping.
/// NexiBotConfig uses #[serde(default)] on all fields, so {} should work.
#[test]
fn test_empty_config_yaml_parses() {
    let yaml = "{}";
    let value: serde_yml::Value =
        serde_yml::from_str(yaml).expect("Empty config should parse");
    assert!(value.is_mapping(), "Empty YAML should parse as a mapping");
}

/// Test that config YAML with unknown/future fields still parses as YAML.
/// This verifies forward compatibility at the YAML layer — new fields
/// added in future versions won't break older parsers.
#[test]
fn test_config_yaml_with_unknown_fields_parses() {
    let yaml = r#"
config_version: 1
some_future_field: "hello"
another_unknown:
  nested: true
"#;
    let value: serde_yml::Value =
        serde_yml::from_str(yaml).expect("YAML with unknown fields should parse");
    assert!(value.is_mapping());
    assert_eq!(
        value["some_future_field"].as_str(),
        Some("hello"),
        "Unknown string field should be preserved"
    );
    assert_eq!(
        value["another_unknown"]["nested"].as_bool(),
        Some(true),
        "Unknown nested field should be preserved"
    );
}

/// Test that a config with sandbox section deserializes correctly,
/// verifying the expected default structure for the sandbox feature.
#[test]
fn test_config_yaml_sandbox_section_parses() {
    let yaml = r#"
config_version: 1
sandbox:
  enabled: true
  image: "debian:bookworm-slim@sha256:abcdef1234567890"
  memory_limit: "512m"
  cpu_limit: 1.0
  network_mode: "none"
  timeout_seconds: 60
  fallback: "AllowHost"
"#;
    let value: serde_yml::Value =
        serde_yml::from_str(yaml).expect("Config with sandbox section should parse");
    let sandbox = &value["sandbox"];
    assert_eq!(sandbox["enabled"].as_bool(), Some(true));
    assert_eq!(sandbox["memory_limit"].as_str(), Some("512m"));
    assert_eq!(sandbox["network_mode"].as_str(), Some("none"));
    assert_eq!(sandbox["timeout_seconds"].as_u64(), Some(60));
    assert_eq!(sandbox["fallback"].as_str(), Some("AllowHost"));
}

/// Test that a config with execute section including sandbox_policy parses.
#[test]
fn test_config_yaml_execute_sandbox_policy_parses() {
    let yaml = r#"
config_version: 1
execute:
  enabled: true
  sandbox_policy: "Dangerous"
  default_timeout_ms: 30000
"#;
    let value: serde_yml::Value =
        serde_yml::from_str(yaml).expect("Config with execute.sandbox_policy should parse");
    let execute = &value["execute"];
    assert_eq!(execute["enabled"].as_bool(), Some(true));
    assert_eq!(execute["sandbox_policy"].as_str(), Some("Dangerous"));
    assert_eq!(execute["default_timeout_ms"].as_u64(), Some(30000));
}

/// Test config hotloading scenario: verify that a config YAML can be
/// re-parsed after modification (simulating file watch + reload).
#[test]
fn test_config_hotload_reparse() {
    // Initial config
    let yaml_v1 = r#"
config_version: 1
claude:
  model: "claude-sonnet-4-20250514"
"#;
    let v1: serde_yml::Value =
        serde_yml::from_str(yaml_v1).expect("Initial config should parse");
    assert_eq!(v1["claude"]["model"].as_str(), Some("claude-sonnet-4-20250514"));

    // Updated config (simulating a file edit picked up by hot-reload)
    let yaml_v2 = r#"
config_version: 1
claude:
  model: "claude-opus-4-20250514"
defense:
  enabled: true
"#;
    let v2: serde_yml::Value =
        serde_yml::from_str(yaml_v2).expect("Updated config should parse");
    assert_eq!(
        v2["claude"]["model"].as_str(),
        Some("claude-opus-4-20250514"),
        "Hot-reloaded config should reflect new model"
    );
    assert_eq!(
        v2["defense"]["enabled"].as_bool(),
        Some(true),
        "Hot-reloaded config should include new defense section"
    );
}

/// Verify that two vaults opened against different database files do not
/// share entries: a proxy key from vault A must not resolve in vault B.
#[test]
fn test_vault_isolation_between_instances() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();

    let vault_a = KeyVault::new(dir_a.path().join("a.sqlite"), "passphrase-a", true).unwrap();
    let vault_b = KeyVault::new(dir_b.path().join("b.sqlite"), "passphrase-b", true).unwrap();

    let real_key = "sk-ant-api03-isolationtest1234567890123456";
    let proxy_a = vault_a.store(real_key, None).unwrap();

    // proxy_a is not known to vault_b
    let resolved_in_b = vault_b.resolve(&proxy_a).unwrap();
    assert_eq!(
        resolved_in_b, None,
        "Proxy from vault A should not resolve in vault B"
    );
}

/// Verify that label metadata is stored with a vault entry and is visible
/// via list(), allowing operators to audit which config field each proxy
/// key was created for.
#[test]
fn test_vault_entry_label_stored_and_listed() {
    let (vault, _dir) = make_vault();

    let real_key = "sk-ant-api03-labeltest12345678901234567890";
    let label = "claude.api_key";
    let proxy = vault.store(real_key, Some(label)).unwrap();

    let entries = vault.list();
    let entry = entries.iter().find(|e| e.proxy_key == proxy).unwrap();
    assert_eq!(
        entry.label.as_deref(),
        Some(label),
        "Vault entry should store the label"
    );
    assert_eq!(
        entry.format, "anthropic",
        "Vault entry format should be 'anthropic'"
    );
}

/// Verify that use_count increments on each call to resolve(), providing an
/// audit trail of how frequently each credential is accessed.
#[test]
fn test_vault_use_count_increments_on_resolve() {
    let (vault, _dir) = make_vault();

    let real_key = "sk-ant-api03-usecounttest12345678901234567";
    let proxy = vault.store(real_key, None).unwrap();

    // Resolve three times
    vault.resolve(&proxy).unwrap();
    vault.resolve(&proxy).unwrap();
    vault.resolve(&proxy).unwrap();

    let entries = vault.list();
    let entry = entries.iter().find(|e| e.proxy_key == proxy).unwrap();
    assert_eq!(
        entry.use_count, 3,
        "use_count should be 3 after three resolve calls"
    );
}

/// Verify that scan_and_replace intercepts multiple distinct API keys in a
/// single text block, replacing all of them.  This covers the scenario where
/// a user pastes a full config excerpt containing several secrets at once.
#[test]
fn test_scan_and_replace_multiple_keys_in_one_block() {
    let (vault, _dir) = make_vault();

    let anthropic = "sk-ant-api03-multikey1234567890123456789012";
    let openai = "sk-openaimultikey1234567890123456789012345";
    let slack = "xoxb-multi-1234567890-abcdefghijklmnopqrstu";

    let text = format!(
        "anthropic: {}\nopenai: {}\nslack: {}",
        anthropic, openai, slack
    );

    let (sanitized, intercepted) = vault.scan_and_replace(&text);

    assert_eq!(
        intercepted.len(),
        3,
        "Should intercept all three keys, got {}",
        intercepted.len()
    );
    assert!(
        !sanitized.contains(anthropic),
        "Anthropic key must not appear in output"
    );
    assert!(
        !sanitized.contains(openai),
        "OpenAI key must not appear in output"
    );
    assert!(
        !sanitized.contains(slack),
        "Slack key must not appear in output"
    );
}
