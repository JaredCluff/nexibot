# NexiBot Documentation Index

Complete index of comprehensive technical documentation for NexiBot and Knowledge Nexus ecosystem.

## New Documentation (6,831 lines across 10 files)

### Setup & Installation

**[SETUP_NEXIBOT.md](./SETUP_NEXIBOT.md)** (475 lines)
Complete guide to installing and configuring the NexiBot desktop application.

- System requirements for macOS, Windows, Linux
- Installation from release binaries or source
- First-run configuration and permissions
- Settings overview (chat, voice, channels, memory, security)
- Troubleshooting common issues
- Updating and uninstalling
- Data locations and backup procedures

**[SETUP_KN_AGENT_CLI.md](./SETUP_KN_AGENT_CLI.md)**
Redirect to the [knowledge-nexus-local](https://github.com/jaredcluff/knowledge-nexus-local) repository.

### Configuration & Integration

**[CHANNELS_SETUP.md](./CHANNELS_SETUP.md)** (756 lines)
Complete setup guide for all 8 messaging channels.

- Overview of supported channels (Telegram, Discord, WhatsApp, Slack, Signal, Teams, Matrix, Email)
- Step-by-step setup for each channel with credentials
- Features and limitations per channel
- Webhook configuration for production
- Multi-channel management and unified identity
- Security best practices per channel
- Per-channel settings and customization

**[VOICE_CONFIGURATION.md](./VOICE_CONFIGURATION.md)** (695 lines)
Complete voice pipeline configuration guide.

- Voice system architecture and data flow
- System requirements for audio hardware
- Initial setup: microphone selection, STT/TTS backends, wake word
- Wake word detection tuning (sensitivity, false positives)
- Voice Activity Detection (VAD) configuration
- STT backends: macOS Speech, SenseVoice, Deepgram, OpenAI
- TTS backends: macOS say, Piper, ElevenLabs, Cerebras
- Audio preprocessing, noise gates, normalization
- Recording duration limits and stop phrases
- Troubleshooting microphone, STT, TTS, and latency issues
- Performance optimization for different scenarios
- Privacy considerations (local-only vs cloud)

**[MCP_INTEGRATION.md](./MCP_INTEGRATION.md)** (712 lines)
Model Context Protocol integration and server management.

- What is MCP and how it works
- Available official and third-party servers
- Configuration file format and server types
- Setting up specific servers (Filesystem, SQLite, GitHub, Bash)
- Via UI registration and CLI commands
- Tool discovery and automatic integration
- Debugging MCP issues and common problems
- Custom MCP server development (Python, Node.js)
- Tool execution model and example workflows
- Advanced configuration: env vars, timeouts, security controls
- Best practices and troubleshooting checklist

### Skills & Capabilities

**[SKILLS_MANAGEMENT.md](./SKILLS_MANAGEMENT.md)** (702 lines)
Complete skill creation and management guide.

- What are skills and structure (SKILL.md format)
- YAML frontmatter fields and markdown content
- Creating custom skills step-by-step
- Script execution (bash, Python)
- Assets and templates
- Managing skills: enable/disable, update, delete
- Skill security scanning and dangerous patterns
- Security levels and safe skill creation
- Registering skills: local, ClawHub, third-party
- Skill execution model: user vs model invocation
- Example skills (calculator, note-taking, code formatter)
- Sharing and versioning skills
- Debugging and troubleshooting skills
- Python skill development and async/concurrent skills

**[NEXIGATE_API.md](./NEXIGATE_API.md)** (544 lines)
NexiGate Part 2 secure shell integration API documentation.

- DiscoveryEngine API for secret/environment detection
- FilterLayer API for command validation and output filtering
- ShellSecurityPlugin trait for custom plugins
- PluginHost API for plugin management
- Configuration structures (DiscoveryConfig, PluginConfig)
- Tauri commands for key generation and plugin signing
- Event types (secret-discovered, plugin-decision, filter-applied)
- Complete example: secure shell executor
- Best practices and security considerations

### Advanced Features

**[AGENTIC_WORKFLOW.md](./AGENTIC_WORKFLOW.md)** (621 lines)
Agentic mode for autonomous task execution and multi-step planning.

- Conversational vs agentic modes
- Planning and multi-step workflow execution
- Tool execution loop architecture
- Orchestration system (subagents and agent teams)
- Controlling subagent behavior and concurrency
- Memory usage in agentic work
- Defense and safety in autonomous mode
- Monitoring agent execution in real-time
- Task history and performance metrics
- Example agentic workflows (data pipeline, security audit, deployment, support)
- Adaptive planning and feedback loops
- Error recovery and fault tolerance
- Best practices and troubleshooting

**[MEMORY_AND_CONTEXT.md](./MEMORY_AND_CONTEXT.md)** (707 lines)
Sophisticated memory system with hybrid search and context management.

- Memory system overview and architecture
- Memory types: Conversation, Preference, Fact, Context
- SQLite FTS5 database backend with vector embeddings
- Capacity constraints and LRU eviction
- Hybrid search pipeline: keyword + semantic + MMR re-ranking
- Fact extraction from conversations
- Session management and compaction
- Memory context formatting into system prompt
- Searching memory via UI and API
- Privacy and data retention policies
- Importance scoring and relationship linking
- Per-user memory in multi-user environments
- Performance tuning and optimization
- Configuration reference
- Troubleshooting memory issues

**[SECURITY_GUIDE.md](./SECURITY_GUIDE.md)** (939 lines)
Comprehensive security configuration and hardening guide.

- Defense-in-depth architecture (6 security layers)
- Security levels: Standard, Enhanced, Maximum
- Prompt injection detection (DeBERTa v3)
- Content safety classification (Llama Guard 3)
- Guardrails system for command validation
- SSRF protection with fail-closed DNS validation
- Session encryption (AES-256-GCM with Argon2id)
- Tool and command execution control
- Tool allowlist/blocklist per agent
- Execution approval modes and confirmation gates
- Computer Use confirmation gates
- Audit logging (17-point security audit)
- Multi-user RBAC (Admin, Parent, User, Guest)
- Credential management and secure storage
- API key rotation with fallback
- Network security: TLS validation, rate limiting
- Log redaction and secrets filtering
- Workspace confinement
- Docker sandbox for command execution
- Security best practices and compliance
- Incident response procedures
- Hardening checklist

## Quick Navigation

### For New Users
1. Start with [SETUP_NEXIBOT.md](./SETUP_NEXIBOT.md) - Installation and first-run
2. Review [VOICE_CONFIGURATION.md](./VOICE_CONFIGURATION.md) - If using voice
3. Check [CHANNELS_SETUP.md](./CHANNELS_SETUP.md) - For messaging integration

### For Developers
1. [NEXIGATE_API.md](./NEXIGATE_API.md) - Shell security integration
2. [MCP_INTEGRATION.md](./MCP_INTEGRATION.md) - Extending capabilities
3. [SKILLS_MANAGEMENT.md](./SKILLS_MANAGEMENT.md) - Custom skills
4. [SETUP_KN_AGENT_CLI.md](./SETUP_KN_AGENT_CLI.md) - K2K agent development

### For Advanced Users
1. [AGENTIC_WORKFLOW.md](./AGENTIC_WORKFLOW.md) - Autonomous task execution
2. [MEMORY_AND_CONTEXT.md](./MEMORY_AND_CONTEXT.md) - Memory system deep dive
3. [SECURITY_GUIDE.md](./SECURITY_GUIDE.md) - Hardening and compliance

### For System Administrators
1. [SETUP_KN_AGENT_CLI.md](./SETUP_KN_AGENT_CLI.md) - Enterprise deployment
2. [SECURITY_GUIDE.md](./SECURITY_GUIDE.md) - Security hardening
3. [CHANNELS_SETUP.md](./CHANNELS_SETUP.md) - Channel management

## Documentation Statistics

- **Total Lines**: 6,831 lines
- **Total Files**: 10 markdown files
- **Total Size**: ~350 KB
- **Coverage**: 10 major topic areas
- **Code Examples**: 50+ complete examples
- **Troubleshooting Sections**: One per guide
- **Best Practices**: Comprehensive coverage
- **Configuration Options**: 200+ documented settings

## File Organization

```
nexibot/docs/
├── INDEX.md                    # This file
├── SETUP_NEXIBOT.md           # Desktop app setup
├── SETUP_KN_AGENT_CLI.md      # CLI agent setup
├── CHANNELS_SETUP.md          # Messaging channels (8 platforms)
├── VOICE_CONFIGURATION.md     # Voice pipeline setup
├── SKILLS_MANAGEMENT.md       # Custom skills creation
├── MCP_INTEGRATION.md         # Model Context Protocol
├── NEXIGATE_API.md            # Shell security APIs
├── AGENTIC_WORKFLOW.md        # Autonomous task execution
├── MEMORY_AND_CONTEXT.md      # Memory system deep dive
└── SECURITY_GUIDE.md          # Security hardening
```

## Key Features Documented

### Installation & Setup
- ✅ System requirements for all platforms
- ✅ Installation from binaries and source
- ✅ First-run configuration
- ✅ Credential setup (API keys, OAuth)

### Integration
- ✅ 8 messaging channels (Telegram, Discord, WhatsApp, Slack, Signal, Teams, Matrix, Email)
- ✅ Voice I/O (STT, TTS, wake word, VAD)
- ✅ MCP servers (30+ available)
- ✅ Custom skills framework

### Configuration
- ✅ 200+ documented settings
- ✅ Environment variables
- ✅ YAML configuration format
- ✅ Per-user customization

### Advanced Features
- ✅ Agentic orchestration
- ✅ Subagent spawning
- ✅ Multi-step planning
- ✅ Sophisticated memory (50k entries, hybrid search)
- ✅ K2K federation

### Security
- ✅ 6-layer defense architecture
- ✅ Prompt injection detection
- ✅ Content safety filtering
- ✅ SSRF protection
- ✅ Session encryption
- ✅ RBAC (4 roles)
- ✅ Audit logging
- ✅ Compliance support

## Cross-References

Documents are heavily cross-linked:

- SETUP_NEXIBOT → VOICE_CONFIGURATION (for voice setup)
- SETUP_NEXIBOT → CHANNELS_SETUP (for messaging)
- AGENTIC_WORKFLOW → MEMORY_AND_CONTEXT (memory in agents)
- AGENTIC_WORKFLOW → SECURITY_GUIDE (safe execution)
- SKILLS_MANAGEMENT → SECURITY_GUIDE (skill security)
- MCP_INTEGRATION → SECURITY_GUIDE (tool security)

## Target Audiences

### Beginners
Clear step-by-step instructions with examples for:
- Installing NexiBot
- Setting up voice
- Connecting to messaging channels
- Creating first skill

### Intermediate Users
Comprehensive guides for:
- Advanced voice configuration
- Multi-channel management
- Memory system usage
- Skill development
- MCP server integration

### Advanced Users
Deep dives into:
- Agentic workflows
- Memory search algorithms
- Security architecture
- Custom plugin development
- K2K federation
- Enterprise deployment

## Best Practices Emphasized

1. **Security First**: Security considerations in every guide
2. **Gradual Complexity**: Start simple, advance to complex
3. **Practical Examples**: 50+ complete, runnable examples
4. **Troubleshooting**: Comprehensive troubleshooting sections
5. **Configuration Options**: Every documented with defaults
6. **Cross-linking**: Easy navigation between related topics
7. **Code Snippets**: Copy-paste ready examples in YAML, Python, Bash, JavaScript

## How to Use This Documentation

### Finding Information
1. Check this INDEX for topic overview
2. Click relevant guide for detailed content
3. Use Ctrl+F to search within documents
4. Follow cross-references to related topics

### Learning Path
1. **Day 1**: SETUP_NEXIBOT → basic configuration
2. **Day 2**: VOICE_CONFIGURATION → voice setup
3. **Day 3**: CHANNELS_SETUP → add messaging
4. **Day 4**: SKILLS_MANAGEMENT → create custom skill
5. **Day 5**: SECURITY_GUIDE → harden setup
6. **Week 2**: MCP_INTEGRATION → extend capabilities
7. **Week 3**: AGENTIC_WORKFLOW → autonomous tasks
8. **Week 4**: MEMORY_AND_CONTEXT → advanced memory

## Contributing

To contribute or report issues with documentation:
1. Review the guide
2. Note any gaps or outdated information
3. Submit improvements via pull request
4. Follow same format and style

## Related Documentation

**In Main Repo:**
- `/CLAUDE.md` - Developer guidelines
- `/SECURITY.md` - Security architecture overview
- `/README.md` - Project overview

**External:**
- Knowledge Nexus (coming soon)
- K2K Architecture documentation (coming soon)
- Device Protocol Architecture documentation (coming soon)

## Version History

**Created**: 2025-02-28
**Status**: Complete and reviewed
**Coverage**: All major NexiBot systems

---

**Happy documenting! For questions or improvements, please contribute back to the project.**
