//! Integration tests for the Smart Key Vault pipeline.
//!
//! Tests the full end-to-end vault pipeline without using real credentials.
//! Verifies that real keys are intercepted, stored, and restored correctly.
//!
//! NOTE: These tests require a `lib.rs` entry point in the crate to compile
//! as integration tests. Currently the crate is binary-only (`main.rs`).
//! The tests are written against the public API of `nexibot_tauri` and will
//! become runnable once a `lib.rs` exposing `pub mod security` is added.

use tempfile::TempDir;

// Helper: create a fresh vault with a temp DB
fn make_test_vault(enabled: bool) -> (nexibot_tauri::security::key_vault::KeyVault, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("vault.sqlite");
    let vault =
        nexibot_tauri::security::key_vault::KeyVault::new(db, "test-passphrase", enabled).unwrap();
    (vault, dir)
}

/// Verify that pasting a real Anthropic key into chat causes it to be replaced
/// with a proxy key before it reaches the model context.
#[test]
fn test_chat_ingress_real_key_becomes_proxy() {
    let (vault, _dir) = make_test_vault(true);
    let text = "My API key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz123456 please use it";
    let (sanitized, intercepted) = vault.scan_and_replace(text);
    assert_eq!(intercepted.len(), 1, "Should intercept exactly one key");
    assert!(
        sanitized.contains("sk-ant-PROXY-"),
        "Sanitized text should contain proxy key"
    );
    assert!(
        !sanitized.contains("sk-ant-api03-"),
        "Real key should not appear in output"
    );
}

/// Verify that a proxy key stored in a nested JSON tool input is recursively
/// restored to the real key before tool execution.
#[test]
fn test_tool_input_proxy_restored_recursively() {
    let (vault, _dir) = make_test_vault(true);
    // Use an OpenAI-format key so extract_secrets pattern matches correctly
    let real_key = "sk-abcdefghijklmnopqrstuvwxyz12345";
    let proxy = vault.store(real_key, None).unwrap();

    // Nested JSON: proxy appears 3 levels deep
    let mut val = serde_json::json!({
        "config": {
            "auth": {
                "api_key": proxy.clone()
            }
        },
        "other": "data"
    });
    vault.restore_in_json(&mut val);

    assert_eq!(val["config"]["auth"]["api_key"].as_str().unwrap(), real_key);
    assert_eq!(val["other"].as_str().unwrap(), "data");
}

/// Verify that a tool result echoing a real key back is sanitized so the
/// real key never re-enters the model context.
#[test]
fn test_tool_result_sanitization() {
    let (vault, _dir) = make_test_vault(true);
    // Simulate a tool result that echoes back a real key
    let real_key = "sk-ant-api03-toolresultkey1234567890abcde";
    let tool_result = format!("The API key in your config is: {}", real_key);
    let (sanitized, intercepted) = vault.scan_and_replace(&tool_result);
    assert!(
        !intercepted.is_empty(),
        "Tool result should trigger interception"
    );
    assert!(
        !sanitized.contains("sk-ant-api03-"),
        "Real key should not pass through"
    );
    assert!(
        sanitized.contains("sk-ant-PROXY-"),
        "Proxy key should be in sanitized output"
    );
}

/// Verify that vault.store() + vault.resolve() round-trips correctly for
/// all common config token field types (the pipeline underpinning
/// config_cmds.rs key interception).
#[test]
fn test_config_token_fields_produce_proxy_keys() {
    let (vault, _dir) = make_test_vault(true);
    // Simulate intercepting various config field values via direct store().
    // Note: Cerebras keys (csk-) are stored directly here; scan_and_replace would
    // detect the embedded sk- substring as OpenAI format, but detect_format() on
    // the raw key correctly returns Cerebras, so the proxy prefix is csk-PROXY.
    let fields = vec![
        ("claude_api_key", "sk-ant-api03-claudekey123456789012345678"),
        ("openai_api_key", "sk-openaikey1234567890123456789012345"),
        (
            "cerebras_api_key",
            "csk-cerebraskey123456789012345678901234",
        ),
        (
            "telegram_bot_token",
            "1234567890:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi",
        ),
        (
            "discord_bot_token",
            "MTAxNTExMzE4MDExMTM2MzAz.GnVwID.ABCDEFGHIJKLMNOPQRSTUVWXYZabc12",
        ),
        (
            "slack_bot_token",
            "xoxb-1234567890-1234567890123-ABCDEFabcdef12345678",
        ),
        (
            "github_token",
            "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcde12345678",
        ),
    ];

    for (field_name, value) in &fields {
        let proxy = vault.store(value, Some(field_name)).unwrap();
        assert!(
            nexibot_tauri::security::key_vault::KeyVault::is_proxy_key(&proxy),
            "Field '{}' should produce a proxy key, got: {}",
            field_name,
            proxy
        );
        let resolved = vault.resolve(&proxy).unwrap();
        assert_eq!(
            resolved,
            Some(value.to_string()),
            "Field '{}' should resolve back",
            field_name
        );
    }
}

/// Verify that storing the same real key twice returns the identical proxy key
/// and does not create duplicate vault entries.
#[test]
fn test_vault_deduplication() {
    let (vault, _dir) = make_test_vault(true);
    let real = "sk-ant-api03-deduplication-key12345678901";
    let p1 = vault.store(real, None).unwrap();
    let p2 = vault.store(real, None).unwrap();
    assert_eq!(p1, p2, "Same real key should always get the same proxy key");
    let entries = vault.list();
    let matching: Vec<_> = entries.iter().filter(|e| e.proxy_key == p1).collect();
    assert_eq!(
        matching.len(),
        1,
        "Should only have one vault entry for the real key"
    );
}

/// Verify that revoking a proxy key causes subsequent resolve() calls to
/// return None, making the key permanently inaccessible.
#[test]
fn test_vault_revocation() {
    let (vault, _dir) = make_test_vault(true);
    let real = "sk-ant-api03-revocationtest12345678901234";
    let proxy = vault.store(real, None).unwrap();

    // Verify it resolves before revocation
    assert_eq!(vault.resolve(&proxy).unwrap(), Some(real.to_string()));

    // Revoke
    vault.revoke(&proxy).unwrap();

    // Verify it no longer resolves
    assert_eq!(
        vault.resolve(&proxy).unwrap(),
        None,
        "Revoked key should return None"
    );
}

/// Verify that all major key formats are stored and resolved correctly,
/// confirming that format-specific proxy prefixes are issued and that
/// round-trip fidelity is maintained for every format.
#[test]
fn test_multi_format_store_and_resolve() {
    let (vault, _dir) = make_test_vault(true);

    let test_keys = vec![
        "sk-ant-api03-anthropickey12345678901234567",
        "sk-openaikey12345678901234567890123456789",
        "csk-cerebraskey12345678901234567890123456",
        "ghp_githubpersonalaccesstoken12345678901234",
        "AKIAIOSFODNN7EXAMPLE1234",
        "AIzaSyCfake123456789012345678901234567890",
        "xoxb-1234567890-abcdefghij-ABCDEFGHIJKLM",
        "sk_live_stripekey12345678901234567890123",
    ];

    for key in &test_keys {
        let preview = &key[..20.min(key.len())];
        let proxy = vault.store(key, None).unwrap();
        assert!(
            nexibot_tauri::security::key_vault::KeyVault::is_proxy_key(&proxy),
            "Key '{}...' should produce a proxy key",
            preview
        );
        let resolved = vault.resolve(&proxy).unwrap();
        assert_eq!(
            resolved.as_deref(),
            Some(*key),
            "Should resolve back for key '{}...'",
            preview
        );
    }
}

/// Verify that when the vault is disabled, scan_and_replace is a no-op:
/// text passes through unmodified and no intercepted keys are reported.
#[test]
fn test_disabled_vault_passes_through_unchanged() {
    let (vault, _dir) = make_test_vault(false); // disabled
    let key = "sk-ant-api03-disabledvaulttest1234567890";
    let text = format!("My key is {}", key);
    let (sanitized, intercepted) = vault.scan_and_replace(&text);
    assert_eq!(sanitized, text, "Disabled vault should not modify text");
    assert_eq!(
        intercepted.len(),
        0,
        "Disabled vault should intercept nothing"
    );
}

/// Verify that restore_in_json traverses three levels of JSON nesting and
/// restores the proxy key to the real key at the deepest level.
#[test]
fn test_nested_json_proxy_restore_three_levels() {
    let (vault, _dir) = make_test_vault(true);
    let real = "sk-ant-api03-nestedjsontest12345678901234";
    let proxy = vault.store(real, None).unwrap();

    // 3-level deep JSON
    let mut val = serde_json::json!({
        "level1": {
            "level2": {
                "level3": proxy.clone()
            }
        }
    });
    vault.restore_in_json(&mut val);
    assert_eq!(val["level1"]["level2"]["level3"].as_str().unwrap(), real);
}

/// Verify that scan_and_replace on text that already contains a vault-issued
/// proxy key does not attempt to re-intercept it, preserving idempotency.
#[test]
fn test_proxy_key_not_re_intercepted() {
    let (vault, _dir) = make_test_vault(true);
    let real = "sk-ant-api03-idempotenttest12345678901234";
    let proxy = vault.store(real, None).unwrap();

    // scan_and_replace on text containing proxy key should be idempotent
    let text = format!("Use proxy key {}", proxy);
    let (sanitized, intercepted) = vault.scan_and_replace(&text);
    assert_eq!(
        intercepted.len(),
        0,
        "Proxy key should not be re-intercepted"
    );
    assert_eq!(
        sanitized, text,
        "Text with proxy key should not be modified"
    );
}

/// Verify that 100 unique keys can all be stored and subsequently resolved
/// without collision or data loss, confirming vault scalability.
#[test]
fn test_vault_handles_large_number_of_entries() {
    let (vault, _dir) = make_test_vault(true);

    let mut proxies: Vec<(String, String)> = Vec::new();
    for i in 0..100 {
        // Pad to ensure each key is unique and at least 20 chars after 'sk-ant-'
        let key = format!("sk-ant-api03-masstest{:040}", i);
        let proxy = vault.store(&key, None).unwrap();
        proxies.push((proxy, key));
    }

    for (proxy, real) in &proxies {
        let resolved = vault.resolve(proxy).unwrap();
        assert_eq!(
            resolved.as_deref(),
            Some(real.as_str()),
            "All {} stored keys should resolve correctly",
            proxies.len()
        );
    }
}
