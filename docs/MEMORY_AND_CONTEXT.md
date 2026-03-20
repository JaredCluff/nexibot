# Memory and Context System Guide

Complete guide to NexiBot's sophisticated memory system, including storage, retrieval, search, and context management.

## Memory System Overview

NexiBot maintains persistent memory across conversations using a hybrid search system combining full-text search and semantic vector similarity:

```
User: "What's my favorite coffee?"
  ↓
[Memory Search]
  ├─ Full-text search (BM25): Find "coffee"
  ├─ Semantic search (vectors): Find "beverage preference"
  └─ MMR re-ranking: Combine and diversify results
  ↓
Result: "Espresso with oat milk"
  ↓
Response: "You like espresso with oat milk"
```

## Memory Types

### Conversation Memory

Records of past conversations with users.

```yaml
Type: Conversation
Content: "User asked about weather in Paris, I provided forecast"
Created: 2025-02-28
Tags: ["weather", "paris", "forecast"]
```

### Preference Memory

Learned user preferences and habits.

```yaml
Type: Preference
Content: "User prefers responses in bullet points, not paragraphs"
Created: 2025-02-20
Tags: ["style", "formatting"]
```

### Fact Memory

Important facts about the user or context.

```yaml
Type: Fact
Content: "User works as a data engineer at Acme Corp"
Created: 2025-01-15
Tags: ["occupation", "company"]
```

### Context Memory

General context information.

```yaml
Type: Context
Content: "This is a customer service conversation"
Created: 2025-02-28
Tags: ["support", "customer"]
```

## Memory Storage

### Database Backend

NexiBot uses SQLite with FTS5 (Full-Text Search) for efficient memory storage:

```
Location:
- macOS: ~/.config/nexibot/memory/nexibot.db
- Linux: ~/.local/share/nexibot/memory/nexibot.db
- Windows: AppData\Local\nexibot\memory\nexibot.db
```

### Database Schema

```sql
-- Memory entries
CREATE TABLE memories (
  id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  created_at TIMESTAMP,
  last_accessed TIMESTAMP,
  access_count INTEGER,
  memory_type TEXT,
  tags TEXT[],
  embedding VECTOR(384),  -- 384-dim vectors
  importance REAL,        -- 0-100 score
  metadata JSONB
);

-- Full-text search index
CREATE VIRTUAL TABLE memories_fts USING fts5(
  content, memory_type, tags,
  content=memories
);

-- Conversation sessions
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  title TEXT,
  started_at TIMESTAMP,
  last_activity TIMESTAMP,
  messages JSONB[]  -- Array of message objects
);

-- Session messages
CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES sessions(id),
  role TEXT,
  content TEXT,
  timestamp TIMESTAMP
);
```

### Capacity Constraints

```
Maximum entries per session:    500 messages
Maximum total memories:          50,000 entries
Maximum sessions:                200 sessions
```

When limits are exceeded, oldest/least-used entries are evicted automatically (LRU).

## Hybrid Search Pipeline

### How Search Works

When NexiBot needs to recall information:

```
Query: "What did I say about Python?"

Step 1: Query Expansion
  Variations:
  - "Python programming"
  - "Python code"
  - "Python language"

Step 2: Keyword Search (BM25)
  FTS5 index matches: "python", "code", "programming"
  Results: [mem1, mem2, mem3, mem4]

Step 3: Semantic Search (Vector)
  Convert query to 384-dim embedding
  Cosine similarity to memory embeddings
  Results: [mem2, mem3, mem5, mem6]

Step 4: Score Merge (RRF)
  Reciprocal Rank Fusion combines keyword & vector scores
  Combined ranking: [mem2, mem3, mem1, mem4, mem5, mem6]

Step 5: Re-ranking (MMR)
  Maximal Marginal Relevance diversifies results
  Final ranking: [mem2, mem4, mem3, mem6]

Step 6: Return Top 5
  Return highest-scoring memories
  Format for system prompt
```

### Configuration

```yaml
memory:
  # Search settings
  search:
    # Enable different search types
    enable_keyword_search: true      # BM25 full-text search
    enable_semantic_search: true     # Vector similarity
    use_mmr_reranking: true          # Maximal Marginal Relevance

    # Semantic search threshold
    similarity_threshold: 0.5        # 0-1 (higher = stricter)

    # MMR diversity
    mmr_diversity_weight: 0.5        # 0-1 (higher = more diverse)

    # Max results per search
    max_results: 10

  # Storage settings
  storage:
    max_memories: 50000              # Total entries limit
    max_sessions: 200                # Conversation sessions limit
    max_session_messages: 500        # Messages per session

  # Embedding settings
  embeddings:
    model: "all-MiniLM-L6-v2"        # ONNX model
    dimension: 384
    cache_size: 2048                 # LRU cache entries

  # Importance scoring
  importance:
    enabled: true
    decay_factor: 0.99               # Score decay per day
    boost_on_access: true            # Increase on retrieval
```

## Fact Extraction

NexiBot automatically extracts facts from conversations:

### Detected Patterns

Automatically recognized:
- "I prefer..."
- "My name is..."
- "I work at..."
- "I'm interested in..."
- "I have..."
- "I live in..."

### Example Extraction

```
User: "I prefer responses in markdown format. My name is Alice."

Auto-extracted Facts:
  1. "User prefers responses in markdown format"
     Type: Preference
     Tags: [formatting, markdown]

  2. "User's name is Alice"
     Type: Fact
     Tags: [identity, name]
```

### Configuration

```yaml
memory:
  fact_extraction:
    # Enable auto-extraction
    enabled: true

    # Patterns to look for
    patterns:
      - "I prefer"
      - "I like"
      - "My name"
      - "I work at"
      - "I'm interested in"

    # Custom patterns (regex)
    custom_patterns:
      - "\\bcompany:\\s*([^,]+)"
      - "\\bemail:\\s*([\\w\\.@]+)"

    # Don't extract duplicates within X days
    dedup_days: 7

    # Confidence threshold (0-1)
    min_confidence: 0.8
```

## Session Management

### Conversation Sessions

Each conversation is grouped into sessions for context:

```
Session: "Project Planning"
  ├─ Message 1: User - "Help me plan the project"
  ├─ Message 2: Bot - "I'll help. What's the scope?"
  ├─ Message 3: User - "Need to handle authentication"
  ├─ Message 4: Bot - "Let's create an auth plan..."
  └─ ...
```

### Session Operations

```yaml
# Create session
session_id = create_session("Project Planning")

# Add message
add_message(session_id, "user", "What's the timeline?")

# Set title
set_session_title(session_id, "Project Timeline Discussion")

# End session
end_session(session_id)

# Get current session
current = get_current_session()

# List all sessions
all = list_sessions()

# Set active session
set_current_session_id(session_id)
```

### Session Compaction

As sessions grow, old messages are summarized:

```
Original Session (500+ messages):
  - Messages 1-100: Initial planning
  - Messages 101-200: Requirements gathering
  - ...
  - Messages 401-500: Latest updates

Compacted Session:
  [Compaction: Initial planning discussed scope, timeline, budget]
  - Messages 401-500: Latest updates
```

This preserves memory while keeping sessions manageable.

## Memory Context Formatting

### System Prompt Injection

Retrieved memories are formatted into the system prompt:

```
[Memory Context]

Conversation Memories:
  - User previously asked about Python deployment
  - We discussed Docker best practices
  - User interested in Kubernetes

Preferences:
  - Prefers responses in bullet points
  - Likes technical details
  - Works in Pacific timezone

Important Facts:
  - User is a DevOps engineer
  - Company uses AWS
  - Team uses Kubernetes

---
[Original System Prompt follows...]
```

### Relevance-Based Context

Different queries use different memory subsets:

```
Query: "How do I deploy my app?"
  ↓
Retrieved Context:
  - Previous deployment discussions
  - User's preferred tools
  - Infrastructure facts (AWS, Kubernetes)
  ✗ Not included: Unrelated conversations

Query: "What's my meeting tomorrow?"
  ↓
Retrieved Context:
  - Calendar/schedule preferences
  - Relevant past discussions
  ✗ Not included: Technical discussions
```

## Searching Memory

### Via UI

**Settings > Memory > Search**

```
Search box: [Enter query]
Results:
  [Relevance score] Memory content
  [Relevance score] Memory content
  ...
```

### Via API/Commands

```rust
// Search for relevant memories
let results = memory.search("python deployment", 5)?;

// Get specific memory
let memory = memory.get_by_id("mem-123")?;

// List all memories
let all = memory.list_all()?;

// Get memories by type
let prefs = memory.get_by_type(MemoryType::Preference)?;

// Get memories by tag
let aws = memory.get_by_tag("aws")?;
```

## Privacy and Retention

### Data Retention

```yaml
memory:
  retention:
    # Auto-delete old memories
    auto_delete_enabled: true

    # Keep for X days (0 = never delete)
    retention_days: 365

    # Keep less-accessed memories shorter
    retention_decay: true
    decay_rate: 0.99  # Per day

    # Never delete certain types
    never_delete_types:
      - "important_fact"
```

### Export and Backup

```bash
# Export all memories
nexibot memory export --format json > memories.json

# Backup memory database
cp ~/.config/nexibot/memory/nexibot.db \
   ~/.config/nexibot/memory/nexibot.db.backup

# Import memories
nexibot memory import memories.json
```

### Clearing Memory

```bash
# Clear all memories (warning: cannot undo!)
nexibot memory clear --confirm

# Clear specific type
nexibot memory clear --type conversation

# Clear older than X days
nexibot memory clear --older-than 90
```

## Advanced Features

### Importance Scoring

Memories are scored 0-100 based on:
- Access frequency
- Recency
- Relevance
- User interactions

```
Fresh memory:        80/100
Frequently accessed: 90/100
Old memory:          10/100
Marked important:    95/100
```

### Relationship Linking

Memories can be linked to show relationships:

```
Memory 1: "User likes Python"
  ↓ Related to ↓
Memory 2: "User works as engineer"
  ↓ Related to ↓
Memory 3: "User's company uses Django"
```

### Duplicate Detection

Similar memories are identified and merged:

```
Memory 1: "User prefers bullet points"
Memory 2: "Likes responses as bullet lists"
→ Detected as duplicate
→ Merged into one entry
```

## Configuration

### Complete Memory Config

```yaml
memory:
  # Enable/disable memory
  enabled: true

  # Search configuration
  search:
    enable_keyword_search: true
    enable_semantic_search: true
    use_mmr_reranking: true
    similarity_threshold: 0.5
    mmr_diversity_weight: 0.5
    max_results: 10

  # Storage settings
  storage:
    max_memories: 50000
    max_sessions: 200
    max_session_messages: 500
    database_path: "~/.config/nexibot/memory"

  # Embedding settings
  embeddings:
    model: "all-MiniLM-L6-v2"
    dimension: 384
    cache_size: 2048

  # Importance scoring
  importance:
    enabled: true
    decay_factor: 0.99
    boost_on_access: true

  # Fact extraction
  fact_extraction:
    enabled: true
    dedup_days: 7
    min_confidence: 0.8

  # Retention policy
  retention:
    auto_delete_enabled: true
    retention_days: 365
    retention_decay: true
    decay_rate: 0.99

  # Automatic context in responses
  auto_context_enabled: true
  context_max_size: 2000  # Characters in prompt
```

## Performance Tuning

### Faster Searches

```yaml
memory:
  search:
    enable_semantic_search: false  # Only use keyword search
    use_mmr_reranking: false       # Skip reranking
    max_results: 5                 # Return fewer results
```

### Reduce Memory Usage

```yaml
memory:
  storage:
    max_memories: 10000            # Smaller limit
    max_sessions: 50               # Fewer sessions
    max_session_messages: 250      # Fewer messages per session

  embeddings:
    cache_size: 512                # Smaller cache
```

### Faster Embeddings

```yaml
memory:
  embeddings:
    # Use quantized model (faster but less accurate)
    model: "all-MiniLM-L6-v2-quant"
    cache_size: 4096               # Larger cache
```

## Memory with Multi-User

### Per-User Memory

In family/team mode, each user has separate memory:

```
User: Alice
  ├─ Preferences: "Prefers markdown"
  ├─ Facts: "Works as engineer"
  └─ Conversations: [Session 1, Session 2, ...]

User: Bob
  ├─ Preferences: "Prefers tables"
  ├─ Facts: "Works as manager"
  └─ Conversations: [Session 1, Session 2, ...]
```

### Shared Memory

Optional shared memory for group:

```yaml
memory:
  multi_user:
    # Separate memory per user
    per_user_memory: true

    # Shared group memory
    shared_memory: true

    # What's shareable
    shared_types:
      - "company_facts"
      - "project_info"
```

## Troubleshooting

### Memory Not Being Saved

1. Check memory is enabled: Settings > Memory > Enabled
2. Check disk space available
3. Check database file permissions
4. Review logs: `~/.config/nexibot/logs/memory.log`

### Search Returns No Results

1. Check memory has entries: Settings > Memory > Statistics
2. Lower similarity threshold: `similarity_threshold: 0.3`
3. Check search terms match memory content
4. Try different search terms

### Memory Growing Too Large

1. Reduce `max_memories` limit
2. Enable auto-delete: `retention_days: 90`
3. Clear old memories: `nexibot memory clear --older-than 180`
4. Reduce `cache_size` for embeddings

### Slow Search Performance

1. Disable semantic search (use keyword only)
2. Reduce `max_results`
3. Increase embedding cache size
4. Run database optimization:
   ```bash
   nexibot memory optimize
   ```

## Examples

### Example 1: Build Context Over Time

```
Day 1:
  User: "I'm starting a new Node.js project"
  → NexiBot extracts: Type=Fact, "User working on Node.js"

Day 2:
  User: "How do I add authentication?"
  → Memory search finds: Node.js context
  → Response tailored to Node.js

Day 3:
  User: "What was I building again?"
  → Memory search returns: "Node.js project with auth"
```

### Example 2: Preference Learning

```
Interaction 1: User responds better to bullet points
Interaction 2: User prefers technical jargon
Interaction 3: User asks for examples

Auto-extracted:
  - Preference: Bullet points
  - Preference: Technical detail
  - Preference: Examples

Later:
  NexiBot automatically: Uses bullet points, technical language, provides examples
```

### Example 3: Cross-Session Context

```
Session 1: Discuss project requirements
  └─ Create memory: "Project needs user auth"

Session 2: Discuss architecture
  NexiBot uses memory: "I know it needs user auth, here's the design"

Session 3: Code review
  NexiBot uses memory: References auth requirements from session 1
```

## See Also

- [AGENTIC_WORKFLOW.md](./AGENTIC_WORKFLOW.md) - Using memory in agent tasks
- [SECURITY_GUIDE.md](./SECURITY_GUIDE.md) - Privacy and data protection
- [SETUP_NEXIBOT.md](./SETUP_NEXIBOT.md) - Configuration basics
