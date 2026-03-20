# NexiBot Threat Model v1.0

## MITRE ATLAS Framework

**Version:** 1.0-draft
**Last Updated:** 2026-02-19
**Methodology:** MITRE ATLAS + Data Flow Diagrams
**Framework:** [MITRE ATLAS](https://atlas.mitre.org/) (Adversarial Threat Landscape for AI Systems)

### Framework Attribution

This threat model is built on [MITRE ATLAS](https://atlas.mitre.org/), the industry-standard framework for documenting adversarial threats to AI/ML systems. ATLAS is maintained by [MITRE](https://www.mitre.org/) in collaboration with the AI security community. NexiBot is a Tauri-based desktop AI assistant with a Rust backend and React frontend that exposes a broad attack surface through messaging integrations, voice pipelines, tool execution, and federated knowledge search.

**Key ATLAS Resources:**

- [ATLAS Techniques](https://atlas.mitre.org/techniques/)
- [ATLAS Tactics](https://atlas.mitre.org/tactics/)
- [ATLAS Case Studies](https://atlas.mitre.org/studies/)
- [ATLAS GitHub](https://github.com/mitre-atlas/atlas-data)
- [Contributing to ATLAS](https://atlas.mitre.org/resources/contribute)

### Purpose

This document identifies, categorizes, and prioritizes adversarial threats against NexiBot across all architectural layers -- from channel ingress to tool execution to supply chain. It serves as a living reference for security engineering decisions and audit readiness.

---

## 1. Introduction

### 1.1 Scope

| Component                     | Included | Notes                                                            |
| ----------------------------- | -------- | ---------------------------------------------------------------- |
| Tauri Desktop Runtime         | Yes      | Rust backend, React/WebView frontend, IPC bridge                 |
| Gateway WebSocket Server      | Yes      | Multi-user mode, authentication, session routing                 |
| Channel Integrations (x8)     | Yes      | Telegram, Discord, WhatsApp, Slack, Signal, Teams, Matrix, Email |
| Voice Pipeline                | Yes      | STT, TTS, Wake Word detection, VAD                               |
| Tool Execution Layer          | Yes      | MCP, Computer Use, Browser CDP, file system, shell commands      |
| K2K Federated Knowledge       | Yes      | Cross-instance knowledge search                                  |
| Agent Teams / Subagents       | Yes      | Orchestration, delegation, inter-agent messaging                 |
| ClawHub Skill Marketplace     | Yes      | Skill discovery, installation, updates                           |
| Docker Sandbox                | Yes      | Command isolation for shell execution                            |
| ML Defense Pipeline           | Yes      | DeBERTa prompt injection detector, Llama Guard content safety    |
| Credential Storage            | Yes      | OS keyring integration                                           |
| Session Encryption            | Yes      | AES-256-GCM transcript encryption                                |

### 1.2 Out of Scope

| Component             | Reason                                       |
| --------------------- | -------------------------------------------- |
| LLM Provider Internals | Model weights and provider-side security are outside NexiBot's control |
| OS-level Hardening    | Host operating system configuration is the user's responsibility       |

---

## 2. System Architecture

### 2.1 Trust Boundaries

```
                        UNTRUSTED ZONE
  ┌────────────────────────────────────────────────────────────────────────┐
  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐   │
  │  │ Telegram │ │ Discord  │ │ WhatsApp │ │  Slack   │ │  Signal  │   │
  │  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘   │
  │       │            │            │            │            │           │
  │  ┌────┴─────┐ ┌────┴─────┐ ┌────┴─────┐                             │
  │  │  Teams   │ │  Matrix  │ │  Email   │   Voice Mic / Wake Word     │
  │  └────┬─────┘ └────┬─────┘ └────┬─────┘          │                  │
  └───────┼────────────┼────────────┼─────────────────┼──────────────────┘
          │            │            │                 │
          ▼            ▼            ▼                 ▼
┌─────────────────────────────────────────────────────────────────────────┐
│              TRUST BOUNDARY 1 (TB1): Channel Access                     │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                    GATEWAY / TAURI IPC                             │  │
│  │  * DM pairing with time-limited codes                             │  │
│  │  * AllowFrom sender allowlists per channel                        │  │
│  │  * Token / Password / Tailscale authentication                    │  │
│  │  * WebSocket session binding (multi-user mode)                    │  │
│  │  * Voice wake-word gating before STT activation                   │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────┐
│              TRUST BOUNDARY 2 (TB2): Session Isolation                   │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                      AGENT SESSIONS                               │  │
│  │  * Session key = agent:channel:peer (unique per conversation)     │  │
│  │  * Per-agent tool policies (allow/deny/ask)                       │  │
│  │  * AES-256-GCM encrypted transcript storage                      │  │
│  │  * Agent team orchestration boundaries                            │  │
│  │  * Subagent delegation scoping                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────┐
│              TRUST BOUNDARY 3 (TB3): Tool Execution                      │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                   EXECUTION SANDBOX                                │  │
│  │  * Docker sandbox for shell command isolation                     │  │
│  │  * DCG (Dangerous Command Guard) guardrails                       │  │
│  │  * Exec approval prompts (allow/deny/ask per tool)                │  │
│  │  * SSRF protection (DNS pinning + private IP blocking)            │  │
│  │  * MCP server process isolation                                   │  │
│  │  * Computer Use / Browser CDP confined sessions                   │  │
│  │  * File system access scoped to allowed paths                     │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────┐
│              TRUST BOUNDARY 4 (TB4): External Content                    │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │              FETCHED URLs / EMAILS / K2K RESULTS                  │  │
│  │  * Boundary markers (XML/delimiter tags) on external content      │  │
│  │  * DeBERTa prompt injection classifier                            │  │
│  │  * Llama Guard content safety filter                              │  │
│  │  * Homoglyph / Unicode normalization detection                    │  │
│  │  * K2K result provenance tagging                                  │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────┐
│              TRUST BOUNDARY 5 (TB5): Supply Chain                        │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                        CLAWHUB                                    │  │
│  │  * Skill publishing moderation (pattern + behavioral)             │  │
│  │  * Static skill scanning (regex FLAG_RULES)                       │  │
│  │  * Package integrity verification (hash / signature)              │  │
│  │  * GitHub account age verification                                │  │
│  │  * Semver version control + SKILL.md required                     │  │
│  │  * Community reporting pipeline                                   │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Data Flows

| Flow | Source          | Destination     | Data                        | Protection                                  |
| ---- | -------------- | --------------- | --------------------------- | ------------------------------------------- |
| F1   | Channel        | Gateway         | User messages               | TLS, AllowFrom, DM pairing                  |
| F2   | Voice Mic      | STT Engine      | Audio stream                | Wake-word gate, VAD filtering               |
| F3   | Gateway        | Agent Session   | Routed messages             | Session isolation, AES-256-GCM transcripts  |
| F4   | Agent          | Tools           | Tool invocations            | Tool policy, DCG, exec approvals            |
| F5   | Agent          | Docker Sandbox  | Shell commands              | Container isolation, resource limits         |
| F6   | Agent          | External URLs   | web_fetch / Browser CDP     | SSRF blocking, boundary markers             |
| F7   | K2K Peer       | Agent           | Knowledge search results    | Provenance tagging, content scanning        |
| F8   | ClawHub        | Agent           | Skill packages              | Moderation, scanning, integrity checks      |
| F9   | Agent          | Channel         | Responses                   | Llama Guard, DeBERTa, output filtering      |
| F10  | Primary Agent  | Subagent        | Delegated tasks             | Scoped tool policies, session boundaries    |
| F11  | Agent          | OS Keyring      | Credential read/write       | OS-level keyring encryption                 |

---

## 3. Threat Catalog

### 3.1 Reconnaissance (AML.TA0002)

#### T-RECON-001: Gateway Endpoint Discovery

| Attribute               | Value                                                                        |
| ----------------------- | ---------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Reconnaissance (AML.TA0002)                                                  |
| **ATLAS Technique**     | AML.T0006 - Active Scanning                                                 |
| **Description**         | Attacker scans for exposed NexiBot Gateway WebSocket endpoints on the network |
| **Attack Vector**       | Port scanning, Shodan/Censys queries, DNS enumeration of WebSocket services  |
| **Affected Components** | Gateway WebSocket server, Tauri IPC surface                                  |
| **Trust Boundary**      | TB1 - Channel Access                                                         |
| **Likelihood**          | Medium                                                                       |
| **Impact**              | Low                                                                          |
| **Priority**            | P2                                                                           |
| **Current Mitigations** | Tailscale auth option; Gateway binds to loopback by default in single-user mode |
| **Status**              | Partial                                                                      |
| **Recommendations**     | Add rate limiting on WebSocket handshake; document secure multi-user deployment patterns |

#### T-RECON-002: Voice Pipeline Fingerprinting

| Attribute               | Value                                                                                    |
| ----------------------- | ---------------------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Reconnaissance (AML.TA0002)                                                              |
| **ATLAS Technique**     | AML.T0006 - Active Scanning                                                             |
| **Description**         | Attacker identifies NexiBot presence by probing wake-word responses or TTS output patterns |
| **Attack Vector**       | Playing wake word through speakers in shared spaces; analyzing TTS audio characteristics |
| **Affected Components** | Voice pipeline (Wake Word, STT, TTS)                                                     |
| **Trust Boundary**      | TB1 - Channel Access                                                                     |
| **Likelihood**          | Low                                                                                      |
| **Impact**              | Low                                                                                      |
| **Priority**            | P2                                                                                       |
| **Current Mitigations** | Wake word is user-configurable                                                           |
| **Status**              | Partial                                                                                  |
| **Recommendations**     | Allow disabling voice pipeline entirely; add configurable activation confirmation        |

---

### 3.2 Initial Access (AML.TA0004)

#### T-ACCESS-001: DM Pairing Code Interception

| Attribute               | Value                                                                            |
| ----------------------- | -------------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Initial Access (AML.TA0004)                                                      |
| **ATLAS Technique**     | AML.T0040 - AI Model Inference API Access                                        |
| **Description**         | Attacker intercepts or brute-forces the time-limited DM pairing code to bind an unauthorized device to a channel |
| **Attack Vector**       | Shoulder surfing, network sniffing during code exchange, social engineering       |
| **Affected Components** | DM pairing system across all 8 channels                                          |
| **Trust Boundary**      | TB1 - Channel Access                                                             |
| **Likelihood**          | Low                                                                              |
| **Impact**              | High                                                                             |
| **Priority**            | P2                                                                               |
| **Current Mitigations** | Short expiry window; codes transmitted via the paired channel itself              |
| **Status**              | Mitigated                                                                        |
| **Recommendations**     | Add mutual confirmation step; log and alert on pairing attempts from new senders |

#### T-ACCESS-002: AllowFrom Identity Spoofing

| Attribute               | Value                                                                           |
| ----------------------- | ------------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Initial Access (AML.TA0004)                                                     |
| **ATLAS Technique**     | AML.T0040 - AI Model Inference API Access                                       |
| **Description**         | Attacker spoofs the sender identity that AllowFrom validation trusts, gaining unauthorized agent access |
| **Attack Vector**       | Phone number spoofing (WhatsApp/Signal), username impersonation (Discord/Slack/Matrix), email header forgery |
| **Affected Components** | AllowFrom validation across all 8 channel integrations                          |
| **Trust Boundary**      | TB1 - Channel Access                                                            |
| **Likelihood**          | Medium                                                                          |
| **Impact**              | High                                                                            |
| **Priority**            | P1                                                                              |
| **Current Mitigations** | Channel-native identity verification (varies by platform)                       |
| **Status**              | Partial                                                                         |
| **Recommendations**     | Document per-channel spoofing risks; implement cryptographic sender verification where channel APIs allow; add anomaly detection for sender behavior |

#### T-ACCESS-003: Credential Theft from OS Keyring

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Initial Access (AML.TA0004)                                                 |
| **ATLAS Technique**     | AML.T0040 - AI Model Inference API Access                                   |
| **Description**         | Attacker extracts API keys, tokens, or channel credentials from the OS keyring via malware, physical access, or a malicious skill with host-level privileges |
| **Attack Vector**       | Local privilege escalation; keyring access from malicious process; memory scraping |
| **Affected Components** | OS keyring credential storage, all stored API keys and tokens               |
| **Trust Boundary**      | TB1 - Channel Access                                                        |
| **Likelihood**          | Medium                                                                      |
| **Impact**              | Critical                                                                    |
| **Priority**            | P1                                                                          |
| **Current Mitigations** | OS keyring uses platform encryption (macOS Keychain, Windows DPAPI, Linux Secret Service); credentials never stored in plaintext config files |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Add credential rotation support; implement per-session ephemeral tokens where possible; restrict keyring access to the Tauri process only |

---

### 3.3 Execution (AML.TA0005)

#### T-EXEC-001: Direct Prompt Injection via Channel Message

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Execution (AML.TA0005)                                                      |
| **ATLAS Technique**     | AML.T0051.000 - LLM Prompt Injection: Direct                                |
| **Description**         | Attacker sends a crafted message through any of the 8 channels to override the agent's system prompt, alter behavior, bypass safety filters, or trigger unauthorized tool calls |
| **Attack Vector**       | Adversarial instructions embedded in channel messages; multi-turn manipulation; instruction-following exploits |
| **Affected Components** | Agent LLM, all 8 channel input surfaces, voice STT transcription            |
| **Trust Boundary**      | TB2 - Session Isolation                                                     |
| **Likelihood**          | High                                                                        |
| **Impact**              | Critical                                                                    |
| **Priority**            | P0                                                                          |
| **Current Mitigations** | DeBERTa prompt injection classifier on inbound messages; Llama Guard content safety filtering; boundary markers on external content; tool policies with ask/deny modes |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Add layered defense: output-side validation for sensitive actions; require explicit user confirmation before tool execution triggered by suspicious inputs; tune DeBERTa detection threshold on production traffic |

#### T-EXEC-002: Indirect Prompt Injection via External Content

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Execution (AML.TA0005)                                                      |
| **ATLAS Technique**     | AML.T0051.001 - LLM Prompt Injection: Indirect                              |
| **Description**         | Attacker embeds adversarial instructions in web pages, emails, K2K results, or documents that the agent fetches and processes, causing it to execute unintended actions |
| **Attack Vector**       | Poisoned URLs fetched via web_fetch or Browser CDP; malicious email bodies; compromised K2K peer responses; adversarial PDF/document content |
| **Affected Components** | web_fetch, Browser CDP, email ingestion, K2K federated search               |
| **Trust Boundary**      | TB4 - External Content                                                      |
| **Likelihood**          | High                                                                        |
| **Impact**              | High                                                                        |
| **Priority**            | P0                                                                          |
| **Current Mitigations** | Boundary markers (XML/delimiter tags) wrapping external content; DeBERTa classifier applied to fetched content; Llama Guard content safety check; homoglyph detection normalizing Unicode tricks |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Implement separate execution contexts for external content processing; add content sanitization layer before LLM ingestion; apply DeBERTa with lower confidence threshold on external content vs. direct user messages |

#### T-EXEC-003: Tool Argument Injection

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Execution (AML.TA0005)                                                      |
| **ATLAS Technique**     | AML.T0051.000 - LLM Prompt Injection: Direct                                |
| **Description**         | Attacker manipulates the arguments the LLM passes to tools (shell commands, file paths, MCP parameters, Browser CDP targets) through prompt injection, causing unintended operations |
| **Attack Vector**       | Crafted prompts that influence tool parameter values; path traversal strings in file tool arguments; command chaining in shell arguments |
| **Affected Components** | All tool invocations: MCP, Computer Use, Browser CDP, file system, shell    |
| **Trust Boundary**      | TB3 - Tool Execution                                                        |
| **Likelihood**          | Medium                                                                      |
| **Impact**              | Critical                                                                    |
| **Priority**            | P0                                                                          |
| **Current Mitigations** | Exec approval prompts for dangerous commands; DCG (Dangerous Command Guard) guardrails; Docker sandbox for shell isolation |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Implement argument validation and sanitization layer between LLM output and tool dispatch; parameterize tool calls to prevent injection; add path canonicalization for file system tools |

#### T-EXEC-004: Docker Sandbox Escape

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Execution (AML.TA0005)                                                      |
| **ATLAS Technique**     | AML.T0043 - Craft Adversarial Data                                          |
| **Description**         | Attacker crafts commands that escape the Docker sandbox and execute on the host system, bypassing the isolation boundary |
| **Attack Vector**       | Container escape exploits; volume mount abuse; Docker socket exposure; kernel exploits from within container |
| **Affected Components** | Docker sandbox, host operating system                                       |
| **Trust Boundary**      | TB3 - Tool Execution                                                        |
| **Likelihood**          | Low                                                                         |
| **Impact**              | Critical                                                                    |
| **Priority**            | P1                                                                          |
| **Current Mitigations** | Docker container isolation; limited volume mounts; non-root container user  |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Use rootless Docker or gVisor runtime; drop all capabilities except required minimum; disable Docker socket mount; add seccomp profiles; regularly update container base images |

---

### 3.4 Persistence (AML.TA0006)

#### T-PERSIST-001: Malicious Skill Installation via ClawHub

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Persistence (AML.TA0006)                                                    |
| **ATLAS Technique**     | AML.T0010.001 - Supply Chain Compromise: AI Software                        |
| **Description**         | Attacker publishes a skill to ClawHub containing hidden malicious code (credential theft, backdoor, data exfiltration) that persists across agent restarts once installed |
| **Attack Vector**       | Create GitHub account, pass age check, publish skill with obfuscated payload that evades regex-based moderation |
| **Affected Components** | ClawHub marketplace, skill loading, agent execution environment             |
| **Trust Boundary**      | TB5 - Supply Chain                                                          |
| **Likelihood**          | High                                                                        |
| **Impact**              | Critical                                                                    |
| **Priority**            | P0                                                                          |
| **Current Mitigations** | GitHub account age verification; pattern-based FLAG_RULES moderation; SKILL.md requirement; community reporting pipeline |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Complete VirusTotal/behavioral analysis integration; implement skill sandboxing with restricted permissions; add mandatory code review for skills accessing sensitive APIs; implement skill signing |

#### T-PERSIST-002: Subagent Configuration Hijacking

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Persistence (AML.TA0006)                                                    |
| **ATLAS Technique**     | AML.T0010.002 - Supply Chain Compromise: Data                               |
| **Description**         | Attacker modifies agent team or subagent configuration to inject a persistent malicious agent into the orchestration hierarchy, receiving delegated tasks and accessing shared context |
| **Attack Vector**       | Config file modification via local access or prompt injection; social engineering user into adding attacker-controlled MCP server as a subagent tool provider |
| **Affected Components** | Agent team configuration, subagent orchestration, MCP server registry       |
| **Trust Boundary**      | TB2 - Session Isolation                                                     |
| **Likelihood**          | Low                                                                         |
| **Impact**              | High                                                                        |
| **Priority**            | P2                                                                          |
| **Current Mitigations** | Configuration stored locally with file permissions                          |
| **Status**              | Open                                                                        |
| **Recommendations**     | Add configuration integrity verification (hash-based); audit logging for all config changes; require re-authentication for agent team modifications |

---

### 3.5 Defense Evasion (AML.TA0007)

#### T-EVADE-001: ML Defense Pipeline Bypass

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Defense Evasion (AML.TA0007)                                                |
| **ATLAS Technique**     | AML.T0043 - Craft Adversarial Data                                          |
| **Description**         | Attacker crafts inputs that evade both the DeBERTa prompt injection classifier and Llama Guard content safety filter simultaneously, using adversarial examples tuned against known model architectures |
| **Attack Vector**       | Adversarial token sequences; semantic paraphrasing of malicious intent; Unicode homoglyphs (despite detection); multi-turn attacks that are individually benign but collectively malicious; language mixing to exploit monolingual training gaps |
| **Affected Components** | DeBERTa classifier, Llama Guard, homoglyph detection                        |
| **Trust Boundary**      | TB4 - External Content                                                      |
| **Likelihood**          | High                                                                        |
| **Impact**              | High                                                                        |
| **Priority**            | P1                                                                          |
| **Current Mitigations** | Two-layer defense (DeBERTa + Llama Guard); homoglyph/Unicode normalization; boundary markers on external content |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Add third detection layer (rule-based heuristics for known bypass patterns); implement adversarial training with red-team datasets; add multi-turn conversation analysis for cumulative intent detection; establish model update cadence for classifiers |

#### T-EVADE-002: Boundary Marker Escape

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Defense Evasion (AML.TA0007)                                                |
| **ATLAS Technique**     | AML.T0043 - Craft Adversarial Data                                          |
| **Description**         | Attacker crafts external content that escapes or neutralizes the boundary markers (XML/delimiter tags) used to delineate untrusted content, causing the LLM to treat injected instructions as trusted system context |
| **Attack Vector**       | Tag injection/closing; context window overflow pushing markers out of attention; instruction override ("ignore previous markers"); nested tag confusion |
| **Affected Components** | External content wrapping system, boundary marker implementation            |
| **Trust Boundary**      | TB4 - External Content                                                      |
| **Likelihood**          | Medium                                                                      |
| **Impact**              | High                                                                        |
| **Priority**            | P1                                                                          |
| **Current Mitigations** | XML/delimiter boundary markers; security notice injection in wrapped content |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Use randomized/session-unique boundary tokens instead of static XML tags; implement output-side validation to detect marker escape; add content length limits for external content to prevent attention overflow |

---

### 3.6 Collection & Exfiltration (AML.TA0009, AML.TA0010)

#### T-EXFIL-001: Data Exfiltration via Tool Channels

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Exfiltration (AML.TA0010)                                                   |
| **ATLAS Technique**     | AML.T0009 - Collection via AI-Accessible Data                               |
| **Description**         | Attacker uses prompt injection to instruct the agent to exfiltrate sensitive data (credentials, transcript content, file contents) through web_fetch POST requests, Browser CDP navigation, or outbound channel messages to attacker-controlled destinations |
| **Attack Vector**       | Indirect prompt injection causing agent to POST session data to attacker URL; agent instructed to send file contents via channel message; Browser CDP used to submit form data to external site |
| **Affected Components** | web_fetch, Browser CDP, outbound channel messaging, file system tools       |
| **Trust Boundary**      | TB3 - Tool Execution                                                        |
| **Likelihood**          | Medium                                                                      |
| **Impact**              | Critical                                                                    |
| **Priority**            | P0                                                                          |
| **Current Mitigations** | SSRF blocking for internal/private networks; exec approvals for dangerous operations; DeBERTa/Llama Guard on inbound content |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Implement URL allowlisting for outbound requests; add data classification awareness to prevent credential/PII leakage; require user confirmation for all outbound data transfers above a size threshold; log all outbound data flows for audit |

#### T-EXFIL-002: Session Transcript Extraction

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Collection (AML.TA0009)                                                     |
| **ATLAS Technique**     | AML.T0009 - Collection via AI-Accessible Data                               |
| **Description**         | Attacker extracts sensitive conversation history from encrypted session transcripts by compromising the encryption key, exploiting a key management weakness, or using prompt injection to cause the agent to reveal session content |
| **Attack Vector**       | Encryption key theft from memory or config; prompt injection asking agent to repeat/summarize prior conversations; cross-session context leakage in agent teams |
| **Affected Components** | AES-256-GCM session encryption, transcript storage, agent context window    |
| **Trust Boundary**      | TB2 - Session Isolation                                                     |
| **Likelihood**          | Medium                                                                      |
| **Impact**              | High                                                                        |
| **Priority**            | P1                                                                          |
| **Current Mitigations** | AES-256-GCM transcript encryption; session isolation per agent:channel:peer key |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Implement key rotation; store encryption keys in OS keyring (not alongside transcripts); add sensitive data redaction before transcript storage; limit cross-session context sharing in agent teams |

---

### 3.7 Impact (AML.TA0011)

#### T-IMPACT-001: Arbitrary Command Execution on Host

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Impact (AML.TA0011)                                                         |
| **ATLAS Technique**     | AML.T0031 - Erode AI Model Integrity                                        |
| **Description**         | Attacker achieves arbitrary command execution on the user's host system by chaining prompt injection with exec approval bypass or by exploiting host-mode execution (when Docker sandbox is not enabled) |
| **Attack Vector**       | Prompt injection to shell tool with obfuscated commands; DCG guardrail bypass via encoding/aliasing; direct host execution when Docker sandbox is disabled |
| **Affected Components** | Shell command tool, DCG, exec approvals, Docker sandbox                     |
| **Trust Boundary**      | TB3 - Tool Execution                                                        |
| **Likelihood**          | Medium                                                                      |
| **Impact**              | Critical                                                                    |
| **Priority**            | P0                                                                          |
| **Current Mitigations** | Docker sandbox (optional); DCG dangerous command guardrails; exec approval prompts; tool policies with ask/deny |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Default to Docker sandbox for all shell execution; implement command normalization before DCG evaluation; expand blocklist patterns; add post-execution output scanning for signs of exploitation |

#### T-IMPACT-002: Resource Exhaustion / Cost Abuse

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Impact (AML.TA0011)                                                         |
| **ATLAS Technique**     | AML.T0031 - Erode AI Model Integrity                                        |
| **Description**         | Attacker floods the agent with messages across channels or triggers expensive tool calls (LLM API calls, Browser CDP sessions, K2K searches) to exhaust API credits, compute resources, or storage |
| **Attack Vector**       | Automated message flooding via multiple channels; recursive agent team calls; large-scale K2K searches; repeated Computer Use sessions |
| **Affected Components** | Gateway, agent sessions, LLM API provider, K2K, Computer Use               |
| **Trust Boundary**      | TB1 - Channel Access                                                        |
| **Likelihood**          | High                                                                        |
| **Impact**              | Medium                                                                      |
| **Priority**            | P1                                                                          |
| **Current Mitigations** | AllowFrom sender restrictions; DM pairing requirements                      |
| **Status**              | Open                                                                        |
| **Recommendations**     | Implement per-sender rate limiting; add cost budgets per session/day; set maximum recursion depth for agent teams; add circuit breakers for runaway tool execution loops |

#### T-IMPACT-003: K2K Federated Knowledge Poisoning

| Attribute               | Value                                                                       |
| ----------------------- | --------------------------------------------------------------------------- |
| **ATLAS Tactic**        | Impact (AML.TA0011)                                                         |
| **ATLAS Technique**     | AML.T0010.002 - Supply Chain Compromise: Data                               |
| **Description**         | Attacker operates a malicious K2K peer that returns poisoned knowledge results containing prompt injection payloads, disinformation, or adversarial content designed to manipulate the requesting agent's behavior |
| **Attack Vector**       | Registering a malicious K2K peer; responding to searches with adversarial content; exploiting trust in federated results |
| **Affected Components** | K2K federated knowledge search, agent context window                        |
| **Trust Boundary**      | TB4 - External Content                                                      |
| **Likelihood**          | Medium                                                                      |
| **Impact**              | High                                                                        |
| **Priority**            | P1                                                                          |
| **Current Mitigations** | K2K result provenance tagging; boundary markers on external content; DeBERTa/Llama Guard filtering |
| **Status**              | Partial                                                                     |
| **Recommendations**     | Implement K2K peer reputation scoring; add result cross-validation against multiple peers; apply heightened DeBERTa thresholds to K2K results; allow users to pin trusted K2K peers |

---

## 4. Risk Matrix

### 4.1 Likelihood vs Impact Summary

| Threat ID      | Threat Name                          | Likelihood | Impact   | Risk Level   | Priority | Status    |
| -------------- | ------------------------------------ | ---------- | -------- | ------------ | -------- | --------- |
| T-EXEC-001     | Direct Prompt Injection              | High       | Critical | **Critical** | P0       | Partial   |
| T-EXEC-002     | Indirect Prompt Injection            | High       | High     | **Critical** | P0       | Partial   |
| T-EXEC-003     | Tool Argument Injection              | Medium     | Critical | **Critical** | P0       | Partial   |
| T-PERSIST-001  | Malicious Skill Installation         | High       | Critical | **Critical** | P0       | Partial   |
| T-EXFIL-001    | Data Exfiltration via Tools          | Medium     | Critical | **Critical** | P0       | Partial   |
| T-IMPACT-001   | Arbitrary Command Execution          | Medium     | Critical | **Critical** | P0       | Partial   |
| T-ACCESS-002   | AllowFrom Identity Spoofing          | Medium     | High     | **High**     | P1       | Partial   |
| T-ACCESS-003   | Credential Theft from Keyring        | Medium     | Critical | **High**     | P1       | Partial   |
| T-EXEC-004     | Docker Sandbox Escape                | Low        | Critical | **High**     | P1       | Partial   |
| T-EVADE-001    | ML Defense Pipeline Bypass           | High       | High     | **High**     | P1       | Partial   |
| T-EVADE-002    | Boundary Marker Escape               | Medium     | High     | **High**     | P1       | Partial   |
| T-EXFIL-002    | Session Transcript Extraction        | Medium     | High     | **High**     | P1       | Partial   |
| T-IMPACT-002   | Resource Exhaustion / Cost Abuse     | High       | Medium   | **High**     | P1       | Open      |
| T-IMPACT-003   | K2K Knowledge Poisoning              | Medium     | High     | **High**     | P1       | Partial   |
| T-RECON-001    | Gateway Endpoint Discovery           | Medium     | Low      | **Medium**   | P2       | Partial   |
| T-RECON-002    | Voice Pipeline Fingerprinting        | Low        | Low      | **Low**      | P2       | Partial   |
| T-ACCESS-001   | DM Pairing Code Interception         | Low        | High     | **Medium**   | P2       | Mitigated |
| T-PERSIST-002  | Subagent Config Hijacking            | Low        | High     | **Medium**   | P2       | Open      |

### 4.2 Risk Distribution

| Risk Level   | Count | Percentage |
| ------------ | ----- | ---------- |
| **Critical** | 6     | 33%        |
| **High**     | 8     | 44%        |
| **Medium**   | 3     | 17%        |
| **Low**      | 1     | 6%         |

### 4.3 Critical Path Attack Chains

**Attack Chain 1: Indirect Injection to Host Compromise**

```
T-EXEC-002 --> T-EVADE-001 --> T-EXEC-003 --> T-IMPACT-001
(Poisoned URL)  (Bypass DeBERTa)  (Inject tool args)  (Execute on host)
```

Attacker hosts a poisoned web page. Agent fetches it via web_fetch. The adversarial content bypasses the DeBERTa classifier and boundary markers, manipulates shell command arguments, and achieves arbitrary command execution on the host (especially if Docker sandbox is disabled).

**Attack Chain 2: Supply Chain to Data Theft**

```
T-PERSIST-001 --> T-EVADE-001 --> T-EXFIL-001
(Malicious skill)  (Evade moderation)  (Exfiltrate credentials)
```

Attacker publishes a skill to ClawHub with obfuscated credential-harvesting code that evades regex-based moderation. Once installed, the skill accesses the OS keyring and exfiltrates API keys via outbound HTTP requests.

**Attack Chain 3: K2K Poisoning to Prompt Injection**

```
T-IMPACT-003 --> T-EVADE-002 --> T-EXEC-001 --> T-EXFIL-001
(Poison K2K results)  (Escape markers)  (Inject prompt)  (Exfiltrate data)
```

A malicious K2K peer returns knowledge results containing adversarial content that escapes boundary markers, injects instructions into the agent context, and causes the agent to exfiltrate session data to an attacker-controlled endpoint.

**Attack Chain 4: Voice-Initiated Command Execution**

```
T-RECON-002 --> T-EXEC-001 --> T-IMPACT-001
(Trigger wake word)  (Voice prompt injection)  (Execute commands)
```

Attacker in a shared physical space triggers the wake word, delivers a voice-based prompt injection via the STT pipeline, and achieves command execution through the shell tool.

---

## 5. Recommendations

### 5.1 Immediate (P0)

| ID    | Recommendation                                                | Addresses                           |
| ----- | ------------------------------------------------------------- | ----------------------------------- |
| R-001 | Add output-side validation for sensitive tool calls -- require explicit user confirmation before executing shell commands, file writes, or outbound data transfers triggered by content flagged as potentially injected | T-EXEC-001, T-EXEC-003, T-IMPACT-001 |
| R-002 | Default to Docker sandbox for all shell execution; make host-mode opt-in with explicit risk acknowledgment | T-IMPACT-001, T-EXEC-004            |
| R-003 | Implement argument validation and sanitization layer between LLM output and tool dispatch, including path canonicalization, command normalization, and parameterized tool calls | T-EXEC-003                          |
| R-004 | Complete ClawHub VirusTotal/behavioral analysis integration and implement skill sandboxing with restricted API access | T-PERSIST-001, T-EXFIL-001          |
| R-005 | Apply heightened DeBERTa confidence thresholds and separate execution contexts for external content (web_fetch, K2K, email) vs. direct user messages | T-EXEC-002, T-IMPACT-003            |
| R-006 | Implement URL allowlisting for outbound requests and data transfer size thresholds requiring user confirmation | T-EXFIL-001                          |

### 5.2 Short-term (P1)

| ID    | Recommendation                                                | Addresses                            |
| ----- | ------------------------------------------------------------- | ------------------------------------ |
| R-007 | Implement per-sender rate limiting and per-session cost budgets across all channels and tool types | T-IMPACT-002                         |
| R-008 | Add adversarial training datasets and red-team evaluation cadence for DeBERTa and Llama Guard models; add multi-turn cumulative intent analysis | T-EVADE-001                          |
| R-009 | Replace static XML boundary markers with randomized per-session tokens; add output-side marker integrity verification | T-EVADE-002                          |
| R-010 | Implement credential rotation support and per-session ephemeral tokens; restrict keyring access scope to the Tauri process | T-ACCESS-003                         |
| R-011 | Add K2K peer reputation scoring, result cross-validation, and user-configurable trusted peer lists | T-IMPACT-003                         |
| R-012 | Implement encryption key rotation for session transcripts; store keys in OS keyring separate from encrypted data; add sensitive data redaction before storage | T-EXFIL-002                          |
| R-013 | Document per-channel identity spoofing risks; implement anomaly detection for sender behavior patterns across channels | T-ACCESS-002                         |
| R-014 | Harden Docker sandbox: use rootless mode or gVisor, drop unnecessary capabilities, apply seccomp profiles, disable Docker socket mount | T-EXEC-004                           |

### 5.3 Medium-term (P2)

| ID    | Recommendation                                                | Addresses                            |
| ----- | ------------------------------------------------------------- | ------------------------------------ |
| R-015 | Add WebSocket handshake rate limiting and authenticated service discovery for Gateway multi-user mode | T-RECON-001                          |
| R-016 | Implement configuration integrity verification (hash-based) with audit logging for all agent team and subagent config changes | T-PERSIST-002                        |
| R-017 | Add mutual confirmation step for DM pairing; log and alert on pairing attempts from unknown senders | T-ACCESS-001                         |
| R-018 | Allow full voice pipeline disablement; add configurable activation confirmation beyond wake word | T-RECON-002                          |
| R-019 | Implement skill signing with verifiable publisher identity; add version pinning and rollback capability for installed skills | T-PERSIST-001                        |

---

## 6. Appendices

### 6.1 ATLAS Technique Mapping

| ATLAS ID      | Technique Name                        | NexiBot Threats                                 |
| ------------- | ------------------------------------- | ----------------------------------------------- |
| AML.T0006     | Active Scanning                       | T-RECON-001, T-RECON-002                        |
| AML.T0009     | Collection via AI-Accessible Data     | T-EXFIL-001, T-EXFIL-002                        |
| AML.T0010.001 | Supply Chain Compromise: AI Software  | T-PERSIST-001                                   |
| AML.T0010.002 | Supply Chain Compromise: Data         | T-PERSIST-002, T-IMPACT-003                     |
| AML.T0031     | Erode AI Model Integrity              | T-IMPACT-001, T-IMPACT-002                      |
| AML.T0040     | AI Model Inference API Access         | T-ACCESS-001, T-ACCESS-002, T-ACCESS-003        |
| AML.T0043     | Craft Adversarial Data                | T-EXEC-004, T-EVADE-001, T-EVADE-002            |
| AML.T0051.000 | LLM Prompt Injection: Direct          | T-EXEC-001, T-EXEC-003                          |
| AML.T0051.001 | LLM Prompt Injection: Indirect        | T-EXEC-002                                      |

### 6.2 Trust Boundary to Threat Mapping

| Trust Boundary | Threats                                                                    |
| -------------- | -------------------------------------------------------------------------- |
| TB1: Channel Access      | T-RECON-001, T-RECON-002, T-ACCESS-001, T-ACCESS-002, T-ACCESS-003, T-IMPACT-002 |
| TB2: Session Isolation   | T-EXEC-001, T-EXFIL-002, T-PERSIST-002                                    |
| TB3: Tool Execution      | T-EXEC-003, T-EXEC-004, T-EXFIL-001, T-IMPACT-001                        |
| TB4: External Content    | T-EXEC-002, T-EVADE-001, T-EVADE-002, T-IMPACT-003                       |
| TB5: Supply Chain        | T-PERSIST-001                                                              |

### 6.3 Component Attack Surface Summary

| Component               | Input Surfaces | Tools Accessible | Trust Boundary | Risk Exposure |
| ----------------------- | -------------- | ---------------- | -------------- | ------------- |
| Telegram Integration    | DM messages    | Per agent policy | TB1            | Medium        |
| Discord Integration     | DM messages    | Per agent policy | TB1            | Medium        |
| WhatsApp Integration    | DM messages    | Per agent policy | TB1            | Medium        |
| Slack Integration       | DM messages    | Per agent policy | TB1            | Medium        |
| Signal Integration      | DM messages    | Per agent policy | TB1            | Medium        |
| Teams Integration       | DM messages    | Per agent policy | TB1            | Medium        |
| Matrix Integration      | DM messages    | Per agent policy | TB1            | Medium        |
| Email Integration       | Email body     | Per agent policy | TB1, TB4       | High          |
| Voice Pipeline          | Audio stream   | Per agent policy | TB1            | Medium        |
| Gateway WebSocket       | WS frames      | Routing only     | TB1            | High          |
| MCP Servers             | Tool results   | N/A (provider)   | TB3            | High          |
| Browser CDP             | Page content   | Web interaction  | TB3, TB4       | High          |
| Computer Use            | Screen content | UI interaction   | TB3            | High          |
| K2K Federated Search    | Search results | N/A (data)       | TB4            | High          |
| ClawHub Skills          | Skill code     | Per skill scope  | TB5            | Critical      |
| Docker Sandbox          | Command output | Shell            | TB3            | Medium        |

### 6.4 Glossary

| Term                       | Definition                                                                      |
| -------------------------- | ------------------------------------------------------------------------------- |
| **ATLAS**                  | MITRE's Adversarial Threat Landscape for AI Systems                             |
| **AES-256-GCM**            | Advanced Encryption Standard with 256-bit key in Galois/Counter Mode            |
| **Browser CDP**            | Chrome DevTools Protocol for browser automation                                 |
| **ClawHub**                | Skill marketplace for downloading and publishing agent extensions               |
| **Computer Use**           | Tool allowing the agent to interact with the desktop GUI                        |
| **DCG**                    | Dangerous Command Guard -- guardrails for shell command execution               |
| **DeBERTa**                | Decoding-enhanced BERT with disentangled attention; used for prompt injection detection |
| **DM Pairing**             | Process of binding a messaging channel identity to an authorized NexiBot user   |
| **Gateway**                | WebSocket server providing multi-user routing, authentication, and session management |
| **K2K**                    | Knowledge-to-Knowledge federated search across NexiBot instances                |
| **Llama Guard**            | Meta's content safety classifier for LLM input/output filtering                |
| **MCP**                    | Model Context Protocol -- standard interface for LLM tool providers             |
| **Prompt Injection**       | Attack where adversarial instructions are embedded in LLM input                 |
| **SSRF**                   | Server-Side Request Forgery -- attack causing server to make unintended requests |
| **STT/TTS**                | Speech-to-Text / Text-to-Speech                                                |
| **Tauri**                  | Framework for building desktop applications with a Rust backend and web frontend |
| **VAD**                    | Voice Activity Detection                                                        |

---

_This threat model is a living document. It should be reviewed and updated when new features are added, new attack techniques are discovered, or mitigations are implemented. Security issues can be reported through the project's responsible disclosure process._
