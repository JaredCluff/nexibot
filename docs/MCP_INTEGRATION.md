# Model Context Protocol (MCP) Integration Guide

Complete guide to configuring and using MCP (Model Context Protocol) servers with NexiBot for extended tool capabilities.

## What is MCP?

Model Context Protocol is an open protocol developed by Anthropic for connecting AI models to external data sources and tools. MCP allows NexiBot to:

- Access tools from any MCP-compatible server
- Query remote databases and APIs
- Run code in sandboxed environments
- Integrate with third-party services
- Extend capabilities without code changes

## Architecture

```
NexiBot (MCP Client)
    ↓
[MCP Server Manager]
    ↓
  ┌─┴─────────────────────────────┐
  ↓                               ↓
MCP Server 1                   MCP Server 2
(Local Exec)                   (Database)
  ├─ run-command                  ├─ query-db
  ├─ read-file                    ├─ list-tables
  └─ write-file                   └─ update-record
```

## MCP Servers

### What's Available?

**Official Anthropic MCP Servers:**
- `filesystem` - Read/write files securely
- `bash` - Execute bash commands
- `sqlite` - Query SQLite databases
- `memory` - External memory storage

**Third-Party Servers:**
- GitHub (issues, PRs, repos)
- AWS (EC2, S3, DynamoDB, etc.)
- Stripe (payments, customers)
- Slack (messages, users, channels)
- Linear (issues, projects)
- Postgres (SQL queries)
- And hundreds more at [MCP Servers](https://github.com/modelcontextprotocol/servers)

### Browse Available Servers

```bash
# Find MCP servers on GitHub
# https://github.com/search?q=mcp+server

# Or check the official registry
# https://github.com/modelcontextprotocol/servers
```

## Configuration

### File Location

MCP configuration is in your NexiBot config file:

- **macOS**: `~/Library/Application Support/ai.nexibot.desktop/config.yaml`
- **Windows**: `AppData\Local\nexibot\config.yaml`
- **Linux**: `~/.config/nexibot/config.yaml`

### Basic Configuration

```yaml
mcp:
  enabled: true
  servers:
    - name: "filesystem"
      type: "stdio"
      command: "npx"
      args:
        - "@modelcontextprotocol/server-filesystem"
        - "/home/user/allowed_directory"

    - name: "sqlite"
      type: "stdio"
      command: "node"
      args:
        - "/path/to/mcp-sqlite-server.js"
        - "/path/to/database.db"

    - name: "bash"
      type: "stdio"
      command: "python3"
      args:
        - "-m"
        - "mcp.server.bash"
```

### Server Types

#### Stdio (Local Process)

Starts a local process and communicates via stdin/stdout.

```yaml
servers:
  - name: "filesystem"
    type: "stdio"
    command: "npx"
    args:
      - "@modelcontextprotocol/server-filesystem"
      - "/home/user/data"
```

**When to use:**
- Local tools
- Private tools
- Maximum performance

#### SSE (Server-Sent Events)

Connects to HTTP(S) server with SSE transport.

```yaml
servers:
  - name: "github"
    type: "sse"
    url: "https://mcp-server.example.com/sse"
    auth:
      type: "bearer"
      token: "${GITHUB_TOKEN}"
```

**When to use:**
- Cloud services
- Remote APIs
- Multi-client sharing

## Setting Up MCP Servers

### Example 1: Filesystem Server

Allows NexiBot to securely read/write files.

```bash
# Install
npm install -g @modelcontextprotocol/server-filesystem

# Configuration
```

```yaml
mcp:
  servers:
    - name: "filesystem"
      type: "stdio"
      command: "npx"
      args:
        - "@modelcontextprotocol/server-filesystem"
        - "/home/user/Documents"  # Restricted root path
        - "/home/user/Projects"
```

**Available Tools:**
- `read_file` - Read file contents
- `read_multiple_files` - Read multiple files at once
- `write_file` - Create or update file
- `append_file` - Append to file
- `delete_file` - Delete file
- `list_directory` - List directory contents
- `move_file` - Move/rename file
- `search_files` - Search for files by pattern

**Example Usage:**
```
User: Read my todo list
→ NexiBot calls read_file tool with "Documents/todo.md"
→ Returns file contents
→ NexiBot summarizes: "You have 5 tasks, 2 marked urgent"
```

### Example 2: SQLite Server

Query and modify SQLite databases.

```bash
# Installation
pip install mcp-server-sqlite

# Configuration
```

```yaml
mcp:
  servers:
    - name: "sqlite"
      type: "stdio"
      command: "python3"
      args:
        - "-m"
        - "mcp.server.sqlite"
        - "/path/to/mydata.db"
```

**Available Tools:**
- `query` - Execute SELECT queries
- `insert` - Insert records
- `update` - Update records
- `delete` - Delete records
- `create_table` - Create new table
- `list_tables` - List all tables
- `describe_table` - Get table schema

**Example Usage:**
```
User: How many orders did we receive this month?
→ NexiBot calls query tool: "SELECT COUNT(*) FROM orders WHERE MONTH(date) = MONTH(NOW())"
→ Returns count: 42
→ NexiBot responds: "You received 42 orders this month"
```

### Example 3: GitHub Server

Access GitHub repositories, issues, and PRs.

```bash
# Installation
npm install -g @modelcontextprotocol/server-github

# Configuration
```

```yaml
mcp:
  servers:
    - name: "github"
      type: "stdio"
      command: "npx"
      args:
        - "@modelcontextprotocol/server-github"
      env:
        GITHUB_TOKEN: "${GITHUB_TOKEN}"
        GITHUB_OWNER: "your-username"
        GITHUB_REPO: "your-repo"
```

**Available Tools:**
- `search_repositories` - Find repositories
- `get_repository` - Get repo details
- `list_issues` - List GitHub issues
- `create_issue` - Create new issue
- `update_issue` - Update issue
- `list_pull_requests` - List PRs
- `create_pull_request` - Create PR
- `get_file` - Read file from repo
- `list_files` - List repository files

**Example Usage:**
```
User: Create an issue for the bug we discussed
→ NexiBot calls create_issue tool
→ Issue created: #42 in your repo
→ NexiBot responds with link
```

### Example 4: Bash Server

Execute shell commands safely.

```yaml
mcp:
  servers:
    - name: "bash"
      type: "stdio"
      command: "python3"
      args:
        - "-m"
        - "mcp.server.bash"
      env:
        # Optional: restrict allowed commands
        ALLOWED_COMMANDS: "ls,grep,cat,echo"
```

**Available Tools:**
- `execute_command` - Run shell command
- `get_working_directory` - Get current directory
- `change_directory` - Change directory

**Example Usage:**
```
User: How much disk space do we have?
→ NexiBot calls execute_command: "df -h"
→ Returns disk usage info
→ NexiBot explains: "Root partition is 85% full"
```

## Registering MCP Servers

### Via Configuration File

Edit your config.yaml and add servers under `mcp.servers`.

### Via Settings UI

1. **Settings > MCP Servers**
2. Click **Add Server**
3. Fill in:
   - Server Name
   - Type (stdio or sse)
   - Command/URL
   - Arguments
4. Click **Save**

### Via CLI Command

```bash
# NexiBot CLI (if available)
nexibot mcp add-server \
  --name "filesystem" \
  --type stdio \
  --command "npx" \
  --args "@modelcontextprotocol/server-filesystem" \
  --args "/home/user/data"
```

## Tool Discovery

Once MCP servers are running, NexiBot discovers available tools automatically.

### View Available Tools

**In Settings:**
1. **Settings > MCP Servers**
2. Select server
3. See list of tools

**Via CLI:**
```
nexibot mcp list-tools
```

### Tool Availability

The model knows about all discovered tools and can:
- Call them automatically during conversations
- Explain what they do
- Help with multi-step workflows

## Debugging MCP Issues

### Check Server Status

**Settings > MCP Servers**

Shows for each server:
- Status (Running, Error, Not Started)
- Last error message
- Tool count
- Last connection time

### Enable Debug Logging

```yaml
mcp:
  debug: true
  log_level: "debug"
```

Then check logs:

```bash
# macOS
tail -f ~/.config/nexibot/logs/mcp.log

# Linux
tail -f ~/.local/share/nexibot/logs/mcp.log
```

### Manual Server Testing

```bash
# Test if server starts
npx @modelcontextprotocol/server-filesystem /home/user/data

# You should see JSON initialization
# Press Ctrl+C to exit
```

### Common Issues

**"Server failed to start"**
- Check command is correct
- Verify working directory exists
- Check environment variables are set
- Look at stderr output

**"No tools available"**
- Server started but didn't initialize
- Check server logs
- Verify authentication (if required)
- Try restarting NexiBot

**"Tool calls fail/timeout"**
- Server might be hanging
- Check for permission issues
- Increase timeout in config:
  ```yaml
  mcp:
    timeout: 30  # seconds
  ```

## Tool Execution Model

When NexiBot uses an MCP tool:

1. **Discovery** - Model sees tool in context
2. **Decision** - Model decides to call tool
3. **Invocation** - Model sends tool call with arguments
4. **Execution** - MCP server executes tool
5. **Result** - NexiBot receives result
6. **Context** - Result added to conversation
7. **Response** - Model generates response using result

### Example Flow

```
User: "Show me my top 10 customers by revenue"
  ↓
[NexiBot sees sql-query tool available]
  ↓
Model generates: query tool call
  "SELECT customer, SUM(amount) as revenue FROM orders GROUP BY customer ORDER BY revenue DESC LIMIT 10"
  ↓
SQLite server executes query
  ↓
Returns: [[customer1, 50000], [customer2, 45000], ...]
  ↓
Model formats results
  ↓
Response: "Your top customers are... (shows table)"
```

## Advanced Configuration

### Environment Variables

Use environment variables in config:

```yaml
mcp:
  servers:
    - name: "github"
      type: "stdio"
      command: "npx"
      args:
        - "@modelcontextprotocol/server-github"
      env:
        GITHUB_TOKEN: "${GITHUB_TOKEN}"
        API_BASE_URL: "${GITHUB_API_URL}"
```

Set variables before starting:

```bash
export GITHUB_TOKEN="ghp_..."
nexibot start
```

Or in shell profile:

```bash
# ~/.bashrc or ~/.zshrc
export GITHUB_TOKEN="ghp_..."
```

### Timeouts and Limits

```yaml
mcp:
  timeout: 30              # Max seconds per tool call
  max_concurrent: 5        # Max parallel tool calls
  max_retries: 2           # Retry failed calls
  retry_delay: 1           # Delay between retries (seconds)
```

### Security Controls

```yaml
mcp:
  servers:
    - name: "bash"
      type: "stdio"
      command: "python3"
      args:
        - "-m"
        - "mcp.server.bash"

      # Restrict which tools can be called
      allowed_tools:
        - "execute_command"
        - "get_working_directory"

      # Or explicitly block tools
      blocked_tools:
        - "execute_command"  # Never call this

      # Restrict what commands can run
      command_allowlist:
        - "ls"
        - "grep"
        - "cat"
        - "echo"
```

## Creating Custom MCP Servers

### Basic Server Template

```python
"""
Simple MCP server in Python
"""
from mcp.server import Server
from mcp.server.stdio import StdioTransport
import asyncio

class MyServer:
    def __init__(self):
        self.server = Server("my-server")
        self.setup_tools()

    def setup_tools(self):
        @self.server.call_tool()
        async def my_tool(arguments: dict) -> str:
            """A simple tool"""
            name = arguments.get("name", "World")
            return f"Hello, {name}!"

        @self.server.list_tools()
        async def list_tools():
            return [
                {
                    "name": "my_tool",
                    "description": "A simple greeting tool",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"}
                        }
                    }
                }
            ]

    async def run(self):
        async with StdioTransport(self.server) as transport:
            await transport.start()

if __name__ == "__main__":
    server = MyServer()
    asyncio.run(server.run())
```

Configure in NexiBot:

```yaml
mcp:
  servers:
    - name: "my-server"
      type: "stdio"
      command: "python3"
      args:
        - "/path/to/my_server.py"
```

### Node.js Server Template

```javascript
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

const server = new Server({
  name: "my-server",
  version: "1.0.0",
});

server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [
    {
      name: "my_tool",
      description: "A simple greeting tool",
      inputSchema: {
        type: "object",
        properties: {
          name: { type: "string" },
        },
      },
    },
  ],
}));

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  if (request.params.name === "my_tool") {
    const name = request.params.arguments?.name || "World";
    return {
      content: [
        {
          type: "text",
          text: `Hello, ${name}!`,
        },
      ],
    };
  }
  throw new Error(`Unknown tool: ${request.params.name}`);
});

const transport = new StdioServerTransport();
await server.connect(transport);
```

## Using MCP with NexiBot

### Automatic Tool Integration

Once configured, the model automatically:
- Knows about tools
- Can call them during conversation
- Understands their capabilities
- Handles errors gracefully

### Example Workflows

**Data Analysis:**
```
User: "Analyze database for customer churn"
→ SQLite server queries relevant data
→ Model analyzes patterns
→ NexiBot explains findings
```

**Code Development:**
```
User: "Create a README for my GitHub repo"
→ Filesystem server reads project files
→ Model generates README
→ GitHub server commits file
```

**System Administration:**
```
User: "Check server disk usage"
→ Bash server runs "df -h"
→ Model parses output
→ NexiBot alerts you if usage > 90%
```

## Performance Optimization

### Caching Tool Results

```yaml
mcp:
  cache:
    enabled: true
    ttl: 300  # Cache results for 5 minutes
```

### Parallel Tool Execution

```yaml
mcp:
  max_concurrent: 5  # Run up to 5 tools in parallel
```

### Lazy Loading

```yaml
mcp:
  lazy_start: true  # Only start servers when needed
  auto_shutdown: true  # Stop unused servers after 10 minutes
```

## Best Practices

1. **Keep Servers Close**: Run MCP servers on same machine (stdio) for best performance
2. **Minimize Tools**: Only expose tools you actually need
3. **Set Timeouts**: Prevent hanging by setting reasonable timeouts
4. **Monitor Logs**: Regularly check for MCP errors
5. **Test Manually**: Test server works before configuring
6. **Use Allowlists**: Restrict what tools can do for security
7. **Validate Input**: MCP servers should validate tool arguments
8. **Handle Errors**: Server should return clear error messages
9. **Document Tools**: Provide clear descriptions for model
10. **Update Regularly**: Keep MCP servers and dependencies updated

## Troubleshooting Checklist

- [ ] MCP enabled in config (`mcp.enabled: true`)
- [ ] Server command is correct
- [ ] Required binaries installed (npm, python, etc.)
- [ ] Working directory exists
- [ ] Environment variables set (tokens, API keys)
- [ ] Server logs show successful startup
- [ ] Tool list shows available tools
- [ ] Tool calls execute and return results
- [ ] No permission issues blocking execution
- [ ] Network access available (for remote servers)

## Resources

- [MCP Specification](https://spec.modelcontextprotocol.io/)
- [Official MCP Servers](https://github.com/modelcontextprotocol/servers)
- [Community Servers](https://github.com/topics/mcp-server)
- [MCP SDK Docs](https://github.com/modelcontextprotocol/sdk)
