# Agentic Workflow Guide

Complete guide to using NexiBot in agentic mode for autonomous task execution, multi-step planning, and orchestrated agent teams.

## Agentic vs Conversational Modes

### Conversational Mode (Default)

Interactive back-and-forth discussion:

```
User: "What's the weather in San Francisco?"
→ NexiBot: "Let me check... (uses weather tool) ... It's 72°F and sunny"
→ User can ask follow-up questions
```

**Characteristics:**
- One-turn interaction
- Immediate responses
- Human in the loop
- Good for questions and discussions

### Agentic Mode

Autonomous task completion with planning:

```
User: "Create a deployment report for last month"
→ NexiBot:
  1. Plans: Query database, aggregate metrics, format report
  2. Executes: Queries data, analyzes trends
  3. Creates: Generates report document
  4. Reports: "Report created: deployment_report_2025_02.pdf"
→ Done! No follow-up questions needed
```

**Characteristics:**
- Multi-step execution
- Autonomous decision making
- Goal-oriented
- Good for projects and workflows

## Planning and Multi-Step Tasks

### Explicit Planning

Enable NexiBot to plan before executing:

**Settings > Autonomous > Planning Mode**
- **Off**: No planning (default)
- **Simple**: Basic 3-step planning
- **Detailed**: Full DAG with dependencies
- **Expert**: Adaptive planning with feedback loops

### Planning Example

```
Task: "Analyze customer sentiment from emails and create summary"

Plan Generated:
  Step 1: Query emails from last 30 days (dependency: none)
  Step 2: Analyze sentiment using NLP (dependency: step 1)
  Step 3: Summarize findings (dependency: step 2)
  Step 4: Create presentation (dependency: step 3)

Execution Order:
  → Step 1 runs
  → Step 2 waits for step 1 → runs
  → Step 3 waits for step 2 → runs
  → Step 4 waits for step 3 → runs
  → Complete
```

### Complex Multi-Step Workflows

```
Task: "Deploy new version and verify it's working"

Plan:
  Parallel Phase 1:
    ├─ Pull latest code
    ├─ Build Docker image
    └─ Run security scan

  Sequential Phase 2 (depends on phase 1):
    ├─ Push image to registry
    └─ Deploy to production

  Validation Phase (depends on phase 2):
    ├─ Health checks
    ├─ Smoke tests
    └─ Performance monitoring

  Reporting Phase:
    └─ Create deployment report
```

## Tool Execution and Orchestration

### Tool Loop Architecture

When executing a task, NexiBot uses a tool loop:

```
1. Generate tool calls based on plan
2. Execute tools (e.g., read files, query DB, run scripts)
3. Receive tool results
4. Analyze results
5. Decide next action
6. Repeat until task complete
```

### Example Tool Sequence

```
Task: "Find all Python files with security issues"

Iteration 1:
  Tool Call: search_files(pattern="*.py", path="/project")
  Result: Found 45 Python files

Iteration 2:
  Tool Call: read_file(files=[...])
  Result: File contents loaded

Iteration 3:
  Tool Call: analyze_code(content=..., checks=["injection", "auth"])
  Result: Found 3 issues in 2 files

Iteration 4:
  Tool Call: create_report(issues=[...])
  Result: Report saved to report.txt

Output: "Found 3 security issues - see report.txt"
```

### Tool Call Options

Configure how tools are called:

```yaml
# config.yaml
agentic:
  tool_execution:
    # Max iterations of tool loop
    max_iterations: 10

    # Timeout per tool call (seconds)
    tool_timeout: 30

    # Max parallel tool calls
    max_parallel: 3

    # Retry failed tools
    retry_failed: true
    max_retries: 2
```

## Orchestration System (Subagents)

### What Are Subagents?

Specialized agents that can be spawned for specific subtasks:

```
Main NexiBot Agent
  ├─ Subagent 1: Database Specialist
  │   └─ Responsible for: Queries, schema operations
  ├─ Subagent 2: File System Specialist
  │   └─ Responsible for: File operations, search
  └─ Subagent 3: Code Analyzer
      └─ Responsible for: Code review, security
```

### Enabling Orchestration

**Settings > Autonomous > Orchestration**
- **Disabled**: Single agent only (default)
- **Auto**: Model decides when to spawn subagents
- **Enabled**: Always available for complex tasks

### Agent Orchestration Example

```
Task: "Code review and fix security issues in the codebase"

Main Agent decides:
  "This is complex. I'll spawn subagents."

Spawns:
  1. SecurityAnalyzer (analyze code for vulnerabilities)
  2. CodeFormatter (fix style issues)
  3. Tester (run tests on fixes)

Subagent 1 (SecurityAnalyzer):
  - Scans codebase
  - Identifies issues
  - Reports findings to main agent

Subagent 2 (CodeFormatter):
  - Receives issue locations
  - Applies fixes
  - Verifies syntax

Subagent 3 (Tester):
  - Runs test suite
  - Reports results

Main Agent:
  - Synthesizes results
  - Creates final report
  - "Code review complete. Fixed 5 issues."
```

### Controlling Subagent Behavior

```yaml
agentic:
  orchestration:
    # Allow subagent spawning
    enabled: true

    # Max subagents at once
    max_depth: 3

    # Max agents running in parallel
    max_concurrency: 2

    # Timeout per subagent (seconds)
    timeout: 120

    # How often main agent checks on subagents
    checkpoint_interval: 5
```

## Memory Usage in Agentic Work

### Task-Scoped Memory

Memories specific to current task:

```
Task: "Generate monthly report"

Task Memory:
  - Customer count: 1,234
  - Revenue: $45,678
  - New signups: 156
  - Churn rate: 2.1%

(Memory cleared after task completes)
```

### Persistent Memory

Information preserved across tasks:

```
Persistent:
  - Company name: Acme Corp
  - Industry: SaaS
  - Founding year: 2020
  - Employee count: 45

(Remains for future tasks)
```

### Configure Memory Behavior

```yaml
memory:
  # Preserve context between tasks
  cross_task_context: true

  # Max memory entries during task
  task_max_entries: 1000

  # Auto-save important findings
  auto_persist: true

  # Clean up temporary data after task
  cleanup_on_completion: true
```

## Defense and Safety in Autonomous Mode

### Guardrails

Automated safety checks prevent harmful actions:

```yaml
agentic:
  guardrails:
    # Security level (Standard/Enhanced/Maximum)
    level: "Enhanced"

    # Block dangerous operations
    blocked_operations:
      - "delete_database"
      - "access_credentials"
      - "modify_system_config"

    # Require approval for risky operations
    approval_required:
      - "deploy_to_production"
      - "delete_files"
      - "modify_users"
```

### Confirmation Gates

Critical operations require approval:

```
Agent: "I need to delete logs older than 90 days"
  ↓
NexiBot: "⚠️ Confirmation Required
          You're about to delete files.

          Path: /var/log/app/*
          Count: 1,234 files
          Size: 45 GB

          [Approve] [Cancel]"
  ↓
User clicks: [Approve]
  ↓
Operation proceeds
```

### Approval Settings

```yaml
agentic:
  approval_mode:
    # Always: Require approval
    # Critical: Only for risky operations
    # Auto: Never (very dangerous!)
    mode: "Critical"

    # Timeout for approval (seconds)
    timeout: 300

    # Notify user about approvals
    notifications: true
```

### Recovery from Errors

If an operation fails:

```
Agent: "I'm updating the database..."
  ↓
Tool Error: "Permission denied"
  ↓
Agent: "I don't have permission. Let me try a different approach..."
  ↓
Agent: "I'll use the read-only API instead"
  ↓
Success!
```

## Monitoring Agent Execution

### Real-Time Status

**Settings > Autonomous > Task Monitor**

Shows:
- Current task name
- Progress (step 1/5)
- Current operation (querying database...)
- Tool calls made
- Elapsed time
- Estimated time remaining

### Task History

**Settings > Autonomous > Task History**

View completed tasks:
- Task name
- Start/end time
- Result (success/failure)
- Operations performed
- Files created/modified

### Logs and Debugging

```bash
# macOS
tail -f ~/.config/nexibot/logs/agentic.log

# Linux
tail -f ~/.local/share/nexibot/logs/agentic.log
```

View detailed logs showing:
- Tool calls and results
- Decision points
- Errors and recovery
- Performance metrics

### Performance Metrics

Monitor efficiency:

```
Task: "Generate report"
  Duration: 2m 34s
  Tool calls: 7
  Errors: 0
  Files created: 1
  API calls: 23
  Cache hits: 5
  Total cost: $0.12
```

## Example Agentic Workflows

### Workflow 1: Data Pipeline

```
Task: "Process daily sales data and email report"

Steps:
1. Query sales database for today's transactions
2. Clean and validate data
3. Calculate metrics (total, average, trends)
4. Create visualization
5. Generate summary report
6. Compose email with report
7. Send to stakeholders
8. Log completion

Result: Email sent to sales@example.com ✓
```

**Configuration:**
```yaml
agentic:
  max_iterations: 15
  tool_timeout: 60
  approval_mode: "Auto"  # No approval needed
```

### Workflow 2: Security Audit

```
Task: "Audit permissions and security settings"

Steps:
1. List all users and roles
2. Check for inactive accounts
3. Verify MFA is enabled
4. Scan for exposed credentials
5. Review access logs for anomalies
6. Generate security report
7. Flag issues for review

⚠️ Confirmation needed: "Disable inactive accounts?"
```

**Configuration:**
```yaml
agentic:
  approval_mode: "Critical"  # Require approval for changes
  max_depth: 2  # Limit subagents
  blocked_operations:
    - "modify_database"
    - "delete_accounts"
```

### Workflow 3: Code Deployment

```
Task: "Deploy new version with verification"

Steps:
1. Clone latest repository
2. Run tests
3. Build deployment package
4. ⚠️ Push to staging (requires approval)
5. Run smoke tests on staging
6. ⚠️ Deploy to production (requires approval)
7. Monitor logs for errors
8. Run post-deployment tests
9. Notify team of completion

Result: Version 1.2.3 deployed ✓
```

**Configuration:**
```yaml
agentic:
  orchestration:
    enabled: true  # Use subagents
    max_depth: 3
    max_concurrency: 2

  approval_mode: "Critical"
  approval_required:
    - "deploy_*"
    - "delete_*"
```

### Workflow 4: Customer Support

```
Task: "Research customer issue and create ticket"

Steps:
1. Retrieve customer account details
2. Query support tickets for history
3. Analyze error logs
4. Check knowledge base for solutions
5. Create support ticket with findings
6. Assign to appropriate team
7. Send acknowledgment email

Result: Support ticket #12345 created ✓
```

## Advanced Features

### Adaptive Planning

Agent adjusts plan based on results:

```
Initial Plan:
  1. Query database
  2. Analyze results

Execution:
  Step 1: Database unavailable → Retry logic kicks in
  Step 1: Retry succeeds → Continue
  Step 2: Results unexpected → Plan adjusts
  Step 2: Use fallback analysis method

Result: Task completed with adapted approach
```

### Feedback Loops

Agent can ask user for clarification:

```
Agent: "I found two possible approaches:
  A) Quick solution, 80% accurate
  B) Thorough solution, 99% accurate (takes 30min)

Which would you prefer?"

User: "Go with B, I have time"

Agent: Proceeds with thorough analysis
```

### Error Recovery

Agent handles failures gracefully:

```
Agent: "Attempting operation 1..."
  ↓ FAILS
Agent: "Operation 1 failed. Trying alternative..."
  ↓ SUCCEEDS
Agent: "Alternative succeeded. Continuing..."
  ↓ COMPLETE
```

## Best Practices

1. **Start Simple**: Begin with basic tasks before complex ones
2. **Test First**: Manually test tool calls before automating
3. **Set Timeouts**: Prevent hanging with reasonable timeouts
4. **Monitor Progress**: Watch first run of new workflow
5. **Use Approval Gates**: Require approval for critical operations
6. **Keep History**: Review task history for improvements
7. **Provide Context**: Give agent relevant background information
8. **Break Down Tasks**: Split complex tasks into manageable steps
9. **Handle Edge Cases**: Anticipate failures and recovery
10. **Iterate**: Refine workflows based on results

## Troubleshooting

### Agent Gets Stuck

```yaml
agentic:
  max_iterations: 10  # Limit iterations
  tool_timeout: 30    # Timeout tools
```

### Tool Calls Not Working

1. Test tool manually first
2. Check logs for errors
3. Verify tool has required permissions
4. Add timeout handling

### Memory Issues

1. Reduce task memory limits
2. Clear old task history
3. Reduce persistent memory

### Performance Too Slow

1. Reduce max_concurrency
2. Use faster models
3. Optimize tool calls
4. Cache results when possible

## See Also

- [NexiGate API](./NEXIGATE_API.md)
- [Security Guide](./SECURITY_GUIDE.md)
- [Memory and Context](./MEMORY_AND_CONTEXT.md)
