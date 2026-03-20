# NexiGate API Documentation (Part 2: Advanced Integration)

NexiGate is the secure shell integration layer in NexiBot that provides advanced detection, filtering, and plugin capabilities for secure command execution. This document covers the programmatic APIs available to developers.

## Overview

NexiGate consists of three core systems:

1. **DiscoveryEngine** - Detects environment variables, secrets, and configuration changes
2. **FilterLayer** - Intercepts and validates shell commands before execution
3. **PluginHost** - Loads and executes cryptographically signed security plugins

## DiscoveryEngine API

The DiscoveryEngine scans shell environments and detects exposed secrets and configuration changes.

### `scan_output(output: String) -> Vec<Discovery>`

Scans command output for exposed secrets, API keys, file paths, and other sensitive information.

**Parameters:**
- `output` - Command output string to scan

**Returns:** Vector of discovered items with type and severity

**Example:**
```rust
use nexibot::nexigate::DiscoveryEngine;

let engine = DiscoveryEngine::new();
let discoveries = engine.scan_output("AWS_KEY=AKIA2JFQK...".to_string());

for discovery in discoveries {
    println!("Found: {:?} (severity: {})", discovery.item_type, discovery.severity);
}
```

### `diff_env(before: HashMap<String, String>, after: HashMap<String, String>) -> Vec<EnvChange>`

Compares environment snapshots to detect changes (new vars, deleted vars, modified values).

**Parameters:**
- `before` - Environment before command execution
- `after` - Environment after command execution

**Returns:** Vector of environment changes with change type

**Example:**
```rust
let mut before = std::env::vars().collect::<HashMap<_, _>>();

// Execute command...

let after = std::env::vars().collect::<HashMap<_, _>>();
let changes = engine.diff_env(before, after);

for change in changes {
    println!("Env change: {} -> {}", change.var_name, change.change_type);
}
```

### `make_proxy_token(duration: Duration, capabilities: Vec<String>) -> String`

Creates a time-limited security token for delegated execution.

**Parameters:**
- `duration` - Token validity period
- `capabilities` - List of allowed capabilities (e.g., "execute_read", "execute_write")

**Returns:** JWT token string

**Example:**
```rust
use std::time::Duration;

let token = engine.make_proxy_token(
    Duration::from_secs(300),  // 5 minutes
    vec!["execute_read".to_string()]
);

// Pass token to subprocess for restricted execution
std::env::set_var("NEXIGATE_TOKEN", token);
```

## FilterLayer API

The FilterLayer intercepts and validates commands at the shell execution boundary.

### `register_secret(secret_name: String, pattern: Regex, action: FilterAction)`

Registers a secret pattern to be blocked or logged by the filter.

**Parameters:**
- `secret_name` - Human-readable name (e.g., "API_KEY", "PASSWORD")
- `pattern` - Regex pattern to match
- `action` - FilterAction enum (Block, Log, Transform)

**Returns:** Result indicating success or error

**Example:**
```rust
use nexibot::nexigate::FilterAction;
use regex::Regex;

filter.register_secret(
    "AWS_KEY".to_string(),
    Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
    FilterAction::Block
)?;

filter.register_secret(
    "JWT_TOKEN".to_string(),
    Regex::new(r"eyJ[A-Za-z0-9_-]{50,}").unwrap(),
    FilterAction::Log
)?;
```

### `filter_inbound(command: String) -> Result<FilteredCommand, FilterError>`

Filters incoming command before execution (BEFORE the command runs).

**Parameters:**
- `command` - Shell command string to validate

**Returns:** FilteredCommand with allowed status and any transformations

**Example:**
```rust
match filter.filter_inbound("rm -rf /".to_string()) {
    Ok(filtered) => {
        if filtered.allowed {
            println!("Command approved: {}", filtered.command);
        } else {
            println!("Command blocked: {}", filtered.reason.unwrap());
        }
    }
    Err(e) => eprintln!("Filter error: {}", e),
}
```

### `filter_outbound(output: String) -> FilteredOutput`

Filters command output before returning to user (AFTER the command runs).

**Parameters:**
- `output` - Command output string

**Returns:** FilteredOutput with redacted content and list of redactions

**Example:**
```rust
let raw_output = "Database password is db_pass_123456".to_string();
let filtered = filter.filter_outbound(raw_output);

println!("Redactions: {:?}", filtered.redactions);
println!("Output: {}", filtered.output); // Shows: "Database password is [REDACTED]"
```

### `known_real_values() -> HashMap<String, Vec<String>>`

Returns mapping of known real (legitimate) values for context-aware filtering.

**Parameters:** None

**Returns:** HashMap of category -> example values

**Example:**
```rust
let known = filter.known_real_values();

// Check if a value is known to be legitimate
if known.get("env_vars").unwrap().contains(&"RUST_LOG".to_string()) {
    println!("RUST_LOG is a known legitimate environment variable");
}
```

## ShellSecurityPlugin Trait

Custom security plugins implement this trait to extend filtering capabilities.

```rust
pub trait ShellSecurityPlugin: Send + Sync {
    /// Initialize the plugin
    fn init(&mut self) -> Result<()>;

    /// Validate a command before execution
    fn validate_command(&self, command: &str) -> Result<()>;

    /// Process command output
    fn process_output(&self, output: &str) -> String;

    /// Get plugin metadata
    fn metadata(&self) -> PluginMetadata;
}

pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub author: String,
    pub required_permissions: Vec<String>,
}
```

**Example Plugin Implementation:**

```rust
use nexibot::nexigate::{ShellSecurityPlugin, PluginMetadata, Result};

struct CustomSecurityPlugin;

impl ShellSecurityPlugin for CustomSecurityPlugin {
    fn init(&mut self) -> Result<()> {
        println!("CustomSecurityPlugin initialized");
        Ok(())
    }

    fn validate_command(&self, command: &str) -> Result<()> {
        if command.contains("curl") && !command.contains("--capath") {
            return Err("curl without explicit CA path rejected".into());
        }
        Ok(())
    }

    fn process_output(&self, output: &str) -> String {
        // Custom redaction logic
        output.replace(regex::Regex::new(r"key_\w+").unwrap().as_str(), "[REDACTED]")
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "Custom Security".to_string(),
            version: "1.0.0".to_string(),
            author: "Your Name".to_string(),
            required_permissions: vec!["validate_commands".to_string()],
        }
    }
}
```

## PluginHost API

The PluginHost manages plugin lifecycle, loading, and execution.

### `load_signed_plugin(path: PathBuf, signature: String) -> Result<PluginHandle>`

Loads a cryptographically signed plugin from disk.

**Parameters:**
- `path` - Path to plugin binary or WASM module
- `signature` - Ed25519 signature string

**Returns:** PluginHandle for subsequent operations

**Example:**
```rust
use std::path::PathBuf;

let handle = host.load_signed_plugin(
    PathBuf::from("/path/to/plugin.wasm"),
    "sig_1234567890abcdef...".to_string()
)?;

println!("Plugin loaded: {}", handle.id());
```

### `dispatch(handle: PluginHandle, event: PluginEvent) -> Result<PluginResponse>`

Dispatches an event to a loaded plugin.

**Parameters:**
- `handle` - Plugin handle from load_signed_plugin
- `event` - PluginEvent to process

**Returns:** PluginResponse with result data

**Example:**
```rust
use nexibot::nexigate::{PluginEvent, PluginEventType};

let event = PluginEvent {
    event_type: PluginEventType::CommandValidation,
    data: serde_json::json!({
        "command": "rm -rf /tmp/*"
    }),
};

let response = host.dispatch(handle, event)?;

if response.allowed {
    println!("Plugin approved: {}", response.message);
} else {
    println!("Plugin denied: {}", response.message);
}
```

### `add_trusted_key(key_id: String, public_key: String) -> Result<()>`

Registers a trusted key for plugin signature verification.

**Parameters:**
- `key_id` - Identifier for this key
- `public_key` - Ed25519 public key (base64 encoded)

**Returns:** Result indicating success or error

**Example:**
```rust
host.add_trusted_key(
    "security-team-key-1".to_string(),
    "MCowBQYDK2VwAyEA...".to_string()  // base64 public key
)?;

println!("Trusted key registered");
```

## Configuration

### DiscoveryConfig

```rust
pub struct DiscoveryConfig {
    /// Enable secret detection
    pub enable_secrets: bool,

    /// Enable environment diff detection
    pub enable_env_diff: bool,

    /// Custom patterns for detection
    pub custom_patterns: HashMap<String, String>,

    /// Severity threshold (0-100)
    pub severity_threshold: u32,
}
```

### PluginConfig

```rust
pub struct PluginConfig {
    /// Directory containing plugins
    pub plugin_dir: PathBuf,

    /// Enable plugin execution
    pub enabled: bool,

    /// Trusted key IDs
    pub trusted_keys: Vec<String>,

    /// Plugin execution timeout (seconds)
    pub timeout: u64,

    /// Maximum plugins loaded simultaneously
    pub max_plugins: usize,
}
```

## Tauri Commands

### `generate_plugin_signing_key()`

Generates a new Ed25519 keypair for plugin signing.

**Parameters:** None

**Returns:**
```json
{
  "private_key": "MCowBQYDK2VwAyEA...",
  "public_key": "MCowBQYDK2VwAyEA...",
  "key_id": "key_1234567890"
}
```

**TypeScript:**
```typescript
const keys = await invoke('generate_plugin_signing_key');
console.log('Public key:', keys.public_key);
// Save private key securely!
```

### `sign_plugin_file(path: String, private_key: String) -> String`

Signs a plugin file with a private key.

**Parameters:**
- `path` - Path to plugin file
- `private_key` - Ed25519 private key (base64)

**Returns:** Ed25519 signature string

**TypeScript:**
```typescript
const signature = await invoke('sign_plugin_file', {
  path: '/path/to/plugin.wasm',
  privateKey: 'MCowBQYDK2VwAyEA...'
});
console.log('Signature:', signature);
```

## Event Types

NexiGate emits events for security-relevant operations:

### `shell:secret-discovered`

Fired when a secret is detected in command output.

```json
{
  "secret_type": "api_key",
  "severity": "high",
  "pattern_name": "AWS_KEY",
  "redacted_value": "[REDACTED]"
}
```

### `shell:plugin-decision`

Fired after a plugin makes an allow/deny decision.

```json
{
  "plugin_id": "custom-security-1",
  "command": "rm -rf /tmp/*",
  "allowed": false,
  "reason": "Dangerous command pattern detected"
}
```

### `shell:filter-applied`

Fired when the filter applies transformations.

```json
{
  "command_original": "echo $API_KEY",
  "command_transformed": "echo [REDACTED]",
  "transformations": ["secret_redaction"]
}
```

### `shell:env-change-detected`

Fired when environment changes are detected.

```json
{
  "variable_name": "DATABASE_URL",
  "change_type": "added",
  "value_length": 120,
  "risk_level": "medium"
}
```

## Complete Example: Building a Secure Shell Executor

```rust
use nexibot::nexigate::{
    DiscoveryEngine, FilterLayer, PluginHost, PluginConfig,
    ShellSecurityPlugin, PluginMetadata, Result
};
use std::path::PathBuf;
use std::collections::HashMap;

pub struct SecureShellExecutor {
    engine: DiscoveryEngine,
    filter: FilterLayer,
    host: PluginHost,
}

impl SecureShellExecutor {
    pub fn new(config: PluginConfig) -> Result<Self> {
        Ok(Self {
            engine: DiscoveryEngine::new(),
            filter: FilterLayer::new(),
            host: PluginHost::new(config)?,
        })
    }

    pub fn execute(&self, command: &str) -> Result<String> {
        // Step 1: Filter inbound command
        let filtered = self.filter.filter_inbound(command.to_string())?;
        if !filtered.allowed {
            return Err(format!("Command blocked: {}", filtered.reason.unwrap()).into());
        }

        // Step 2: Execute command
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(&filtered.command)
            .output()?;
        let output_str = String::from_utf8_lossy(&output.stdout).to_string();

        // Step 3: Scan output for secrets
        let discoveries = self.engine.scan_output(output_str.clone());
        for discovery in discoveries {
            eprintln!("WARNING: Found {} in output (severity: {})",
                      discovery.item_type, discovery.severity);
        }

        // Step 4: Filter outbound output
        let filtered_output = self.filter.filter_outbound(output_str);

        Ok(filtered_output.output)
    }
}

// Usage
fn main() -> Result<()> {
    let config = PluginConfig {
        plugin_dir: PathBuf::from("/path/to/plugins"),
        enabled: true,
        trusted_keys: vec!["security-team-key-1".to_string()],
        timeout: 30,
        max_plugins: 10,
    };

    let executor = SecureShellExecutor::new(config)?;

    let output = executor.execute("ls -la /home")?;
    println!("Output: {}", output);

    Ok(())
}
```

## Best Practices

1. **Always Filter Both Directions**: Apply `filter_inbound` before execution and `filter_outbound` after
2. **Register Custom Secrets Early**: Use `register_secret` for domain-specific patterns
3. **Enable Discovery Scanning**: Always scan output with `scan_output` for secrets
4. **Use Proxy Tokens**: For delegated execution, create time-limited tokens with `make_proxy_token`
5. **Validate Plugins**: Only load plugins signed by trusted keys
6. **Monitor Events**: Subscribe to security events for audit logging
7. **Set Reasonable Timeouts**: Plugins have execution timeouts; adjust for your use case

## Security Considerations

- Signatures are cryptographically verified before plugin loading
- Plugins run in WASM sandbox for isolation
- Secrets are redacted in logs automatically
- Discovery patterns have customizable severity thresholds
- All operations are audit-logged with timestamps
- Filter rules can be updated without restart
