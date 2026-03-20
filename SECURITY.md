# Security Policy

If you believe you've found a security issue in NexiBot, please report it privately.

## Reporting

Email **security@nexibot.ai** with the details below.

### Required in Reports

1. **Title** - Brief description of the vulnerability
2. **Severity Assessment** - Critical / High / Medium / Low
3. **Impact** - What an attacker could achieve
4. **Affected Component** - Module, file, or feature
5. **Technical Reproduction** - Step-by-step reproduction
6. **Demonstrated Impact** - Proof of concept or evidence
7. **Environment** - OS, NexiBot version, config
8. **Remediation Advice** - Suggested fix

Reports without reproduction steps and demonstrated impact will be deprioritized.

## Response Timeline

- **Acknowledgment**: within 48 hours
- **Initial assessment**: within 1 week
- **Fix timeline**: depends on severity, but we aim for Critical within 7 days, High within 30 days

## Supported Versions

| Version | Supported |
|---|---|
| 0.8.x | Yes |
| < 0.8.0 | No |

## Security Architecture

NexiBot implements defense-in-depth with 18 security modules:

- **ML Defense Pipeline**: DeBERTa v3 prompt injection detection + Llama Guard 3 content safety
- **Guardrails**: AST-based Destructive Command Guard + sensitive data detection
- **SSRF Protection**: IP classification, DNS pinning, IPv6 transition blocking, scheme blocking
- **Sandbox**: Docker container isolation with env sanitization
- **Credential Storage**: OS keyring (macOS Keychain, Windows Credential Manager, Linux Secret Service)
- **Session Encryption**: AES-256-GCM with Argon2id key derivation
- **Operator Scopes**: 5-scope method authorization with default-deny
- **Audit System**: 21+ security checks with auto-fix capability

See [THREAT-MODEL.md](docs/security/THREAT-MODEL.md) for the formal threat model.

## Out of Scope

- Public Internet Exposure (NexiBot is a desktop application, not a public web service)
- Using NexiBot in ways that the docs recommend not to
- Prompt injection attacks that only affect the LLM's output quality (not security boundaries)

## Plugin Trust Boundary

Skills and MCP servers are loaded as trusted code. Only install skills you trust, and prefer configuring tool permissions in the Security settings tab.

## Runtime Requirements

NexiBot requires:
- **macOS 12+**, **Windows 10+**, or **Linux** (with WebKitGTK)
- **Node.js 22.12.0+** for the Anthropic Bridge service
