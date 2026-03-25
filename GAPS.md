# NexiBot Feature Gaps

Living tracker of competitor features and their closure status.

| # | Feature | Competitor | Status | Closed In |
|---|---------|-----------|--------|-----------|
| 1 | Self-learning skills (autonomous creation) | Hermes Agent | ✅ Closed | v0.8.1 |
| 2 | Explicit skill capture (`/save-as-skill`) | Hermes Agent | ✅ Closed | v0.8.1 |
| 3 | Skill improvement loop (usage-based rewrites) | Hermes Agent | ✅ Closed | v0.8.1 |
| 4 | Agent-initiated skill management LLM tools | Hermes Agent | ✅ Closed | v0.8.1 |
| 5 | Parallel tool execution (multi-tool concurrency) | OpenClaw | ✅ Closed | v0.8.1 |
| 6 | PII redaction before LLM send | Hermes Agent | ✅ Closed | v0.8.1 |
| 7 | User behavioral modeling (Honcho-style) | Hermes Agent | 🔴 Open | — |
| 8 | IDE / editor integration (ACP protocol) | Hermes Agent | 🔴 Open | — |
| 9 | Persistent per-user cross-session memory | Hermes Agent | 🟡 Partial (SQLite FTS5) | — |
| 10 | Marketplace with 10k+ community skills | OpenClaw (13,729+) | 🟡 Partial (ClawHub) | — |
| 11 | Multi-agent workflow visual editor | OpenClaw | 🔴 Open | — |
| 12 | Native iOS companion app | OpenClaw | 🟡 Partial (mobile/ hooks) | — |
| 13 | Structured workflow recording/playback | OpenClaw | 🔴 Open | — |

## Legend

- ✅ Closed — Feature shipped; gap eliminated.
- 🟡 Partial — Comparable capability exists but is less mature or feature-complete.
- 🔴 Open — Gap exists; not yet scheduled.

## Contributing

Add rows with `gap/competitor-feature` GitHub Issues. Increment `#` sequentially.
When closing a gap, change status to ✅ and record the release version in "Closed In".
