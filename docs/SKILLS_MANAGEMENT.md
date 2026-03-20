# Skills Management Guide

Complete guide to creating, managing, and using skills in NexiBot - modular capabilities that extend the agent's functionality.

## What Are Skills?

Skills are modular capability packages inspired by OpenClaw. Each skill is a folder containing:

```
my-skill/
├── SKILL.md              # YAML frontmatter + markdown instructions
├── scripts/              # Optional executable scripts
│   ├── setup.sh
│   └── helper.py
├── references/           # Optional documentation files
│   ├── api_docs.md
│   └── examples.txt
└── assets/              # Optional templates and resources
    ├── prompt_template.txt
    └── config_template.json
```

Skills allow you to:
- Extend NexiBot with custom capabilities
- Package complex operations as single commands
- Share reproducible workflows
- Integrate with external tools and APIs

## Skill Structure

### SKILL.md Format

Every skill has a `SKILL.md` file with YAML frontmatter:

```yaml
---
name: "Weather Check"
description: "Check weather for any location"
author: "John Doe"
version: "1.0.0"
source: "local"

# Whether user can invoke via /command
user_invocable: true

# Prevent model from auto-invoking skill
disable_model_invocation: false

# Required permissions/capabilities
requirements:
  - "internet_access"
  - "location_permission"

# OpenClaw compatibility fields
metadata:
  bins:
    - "curl"
    - "jq"
  env:
    - "OPENWEATHER_API_KEY"
  config:
    - "~/.config/weather.json"

# Command dispatch mode: "prompt", "script", "tool"
command_dispatch: "script"
command_tool: "execute"
command_arg_mode: "positional"
---

# Markdown Instructions

This skill checks weather conditions for any location.

## How to Use

Invoke with: `/weather <city>`

Example:
```
/weather San Francisco
```

Returns current temperature, conditions, and forecast.

## Configuration

Set your OpenWeather API key:

```bash
export OPENWEATHER_API_KEY="your_key_here"
```

## What It Does

1. Takes location as argument
2. Queries OpenWeather API
3. Parses JSON response
4. Formats output for display

---
```

### YAML Frontmatter Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Skill display name |
| `description` | String | Yes | Brief description (one sentence) |
| `author` | String | No | Creator name |
| `version` | String | No | Semantic version (e.g., 1.0.0) |
| `source` | String | No | "local" or "clawhub" |
| `user_invocable` | Boolean | No | Allow `/command` invocation (default: true) |
| `disable_model_invocation` | Boolean | No | Prevent model auto-invoke (default: false) |
| `requirements` | Array | No | List of required capabilities |
| `metadata` | Object | No | OpenClaw requirements (bins, env, config) |
| `command_dispatch` | String | No | "prompt", "script", or "tool" |
| `command_tool` | String | No | Tool name for dispatch |
| `command_arg_mode` | String | No | "positional", "named", "json" |

### Markdown Content

After frontmatter, write markdown documentation explaining:
- How to use the skill
- What it does
- Configuration requirements
- Examples
- Troubleshooting

## Creating a Custom Skill

### Step 1: Create Skill Directory

```bash
# Create skill folder
mkdir -p ~/.config/nexibot/skills/my-weather-skill

# Navigate to it
cd ~/.config/nexibot/skills/my-weather-skill
```

### Step 2: Create SKILL.md

```bash
cat > SKILL.md << 'EOF'
---
name: "Weather Check"
description: "Check weather for any location using OpenWeather API"
author: "Jane Doe"
version: "1.0.0"
source: "local"
user_invocable: true
requirements:
  - "internet_access"
metadata:
  bins:
    - "curl"
    - "jq"
  env:
    - "OPENWEATHER_API_KEY"
---

# Weather Check Skill

Check the current weather and forecast for any location worldwide.

## Usage

`/weather <city_name>`

## Examples

```
/weather Tokyo
/weather New York
/weather London
```

## Configuration

1. Sign up at [OpenWeather](https://openweathermap.org/api)
2. Get your free API key
3. Set environment variable:

```bash
export OPENWEATHER_API_KEY="your_key_here"
```

## What It Returns

- Current temperature
- Weather condition (sunny, rainy, etc.)
- Humidity and wind speed
- 5-day forecast

---
EOF
```

### Step 3: Create Script (Optional)

```bash
mkdir -p scripts

cat > scripts/get_weather.sh << 'EOF'
#!/bin/bash

CITY=$1
API_KEY=${OPENWEATHER_API_KEY}

if [ -z "$CITY" ]; then
  echo "Usage: get_weather.sh <city>"
  exit 1
fi

if [ -z "$API_KEY" ]; then
  echo "Error: OPENWEATHER_API_KEY not set"
  exit 1
fi

curl -s "https://api.openweathermap.org/data/2.5/weather?q=${CITY}&appid=${API_KEY}&units=metric" | \
  jq '{
    city: .name,
    temperature: .main.temp,
    condition: .weather[0].main,
    humidity: .main.humidity,
    wind_speed: .wind.speed
  }'
EOF

chmod +x scripts/get_weather.sh
```

### Step 4: Create Assets (Optional)

```bash
mkdir -p assets

cat > assets/prompt_template.txt << 'EOF'
You are a weather expert. When the user asks about weather:
1. Use the /weather skill to get current conditions
2. Format the response in a friendly way
3. Provide relevant advice (e.g., "bring an umbrella" for rain)
EOF
```

### Step 5: Test Locally

```bash
# In NexiBot, go to Settings > Skills
# Click "Reload Skills"
# Try: /weather Tokyo

# Or test script directly
export OPENWEATHER_API_KEY="test_key"
./scripts/get_weather.sh "San Francisco"
```

## Managing Skills

### Enable/Disable Skills

In **Settings > Skills**, toggle individual skills on/off without uninstalling.

### Update Skill

1. Edit `SKILL.md` or scripts
2. In **Settings > Skills**, click **Reload Skills**
3. Changes take effect immediately (hot-reload)

### Delete Skill

**Via File System:**
```bash
rm -rf ~/.config/nexibot/skills/skill-name
```

**Via GUI:**
In **Settings > Skills**, find skill and click **Delete**.

### View Skill Details

**Settings > Skills > [Skill Name]**

Shows:
- Name and description
- Author and version
- Requirements
- Script availability
- Security scanning results

## Skill Security

### Security Scanning

NexiBot automatically scans skills for dangerous patterns:

```
Dangerous patterns blocked:
- eval() or exec() calls
- unsafe subprocess spawning
- Direct OS command execution without validation
- Hardcoded credentials
- Network calls to untrusted hosts
```

### Security Levels

Depend on your **Security Level** setting in Settings > Security:

| Level | Script Execution | External Calls | Requirements |
|-------|------------------|-----------------|----------------|
| Standard | Allowed | Blocked | Must declare |
| Enhanced | Restricted | Blocked | Strict validation |
| Maximum | Prohibited | Blocked | No external access |

### Creating Secure Skills

1. **Declare Dependencies**: List required binaries and env vars in `metadata`
2. **Validate Input**: Check arguments for path traversal, injection
3. **Use Safe Calls**: Prefer libraries over shell exec
4. **Limit Scope**: Only access files/APIs needed
5. **Respect Permissions**: Use environment variables, not hardcoded secrets

### Example: Safe Weather Skill

```bash
#!/bin/bash
set -euo pipefail  # Exit on error

CITY="$1"

# Validate input (no special chars)
if ! [[ "$CITY" =~ ^[a-zA-Z\ \-]+$ ]]; then
  echo "Invalid city name"
  exit 1
fi

# Use API key from environment
if [ -z "${OPENWEATHER_API_KEY:-}" ]; then
  echo "OPENWEATHER_API_KEY not set"
  exit 1
fi

# Make API call safely
response=$(curl -s --max-time 10 \
  "https://api.openweathermap.org/data/2.5/weather?q=${CITY}&appid=${OPENWEATHER_API_KEY}&units=metric")

# Parse safely with jq
echo "$response" | jq -r '.main.temp'
```

## Registering Skills

### Local Skills

Skills in `~/.config/nexibot/skills/` are automatically discovered.

### ClawHub Skills

From the ClawHub marketplace:

1. **Settings > Skills > Browse ClawHub**
2. Search for skill
3. Click **Install**
4. Skill downloads and registers automatically

### Third-Party Skills

From a custom repository:

```bash
# Download skill
git clone https://github.com/user/nexibot-skill.git \
  ~/.config/nexibot/skills/nexibot-skill

# Or: curl + unzip
curl -L https://example.com/skill.zip -o skill.zip
unzip skill.zip -d ~/.config/nexibot/skills/

# Reload
# In Settings > Skills, click "Reload Skills"
```

## Skill Execution Model

### User Invocation

User types: `/weather Tokyo`

1. NexiBot checks skill is registered and user_invocable=true
2. Loads SKILL.md and checks requirements
3. If script exists, executes with argument "Tokyo"
4. Returns output to user
5. Output is included in conversation context

### Model Invocation

During a conversation, the model may call skills automatically:

User: "What's the weather in Paris?"

1. Model sees /weather skill available
2. If disable_model_invocation=false, model can call it
3. Model decides to invoke: `/weather Paris`
4. Script executes, returns weather data
5. Model incorporates result into response

### Tool Dispatch

Skills can be dispatched as tools:

```yaml
command_dispatch: "tool"
command_tool: "execute_script"
command_arg_mode: "json"
```

Model can call with JSON arguments:

```json
{
  "skill": "weather",
  "args": {
    "city": "London",
    "units": "celsius"
  }
}
```

## Example Skills

### 1. Calculator Skill

```yaml
---
name: "Calculator"
description: "Perform mathematical calculations"
user_invocable: true
metadata:
  bins:
    - "bc"
---

# Calculator Skill

Perform complex mathematical calculations.

## Usage

`/calc <expression>`

## Examples

```
/calc "2 + 2"
/calc "sqrt(16)"
/calc "sin(3.14159)"
```
```

**Script:**
```bash
#!/bin/bash
echo "scale=10; $1" | bc -l
```

### 2. Note-Taking Skill

```yaml
---
name: "Quick Notes"
description: "Save and retrieve quick notes"
user_invocable: true
metadata:
  config:
    - "~/.notes"
---

# Quick Notes Skill

Save and retrieve quick notes for later retrieval.

## Usage

Save: `/note save "My important note"`
Retrieve: `/note list`
Search: `/note search "keyword"`
```

**Script:**
```bash
#!/bin/bash
NOTES_DIR="$HOME/.notes"
mkdir -p "$NOTES_DIR"

case "$1" in
  save)
    echo "$2" >> "$NOTES_DIR/notes.txt"
    echo "Note saved!"
    ;;
  list)
    tail -20 "$NOTES_DIR/notes.txt"
    ;;
  search)
    grep "$2" "$NOTES_DIR/notes.txt"
    ;;
esac
```

### 3. Code Formatter Skill

```yaml
---
name: "Code Formatter"
description: "Format code in various languages"
user_invocable: true
metadata:
  bins:
    - "prettier"
    - "black"
    - "rustfmt"
---

# Code Formatter

Automatically format code in Python, JavaScript, Rust, and more.

## Usage

`/format <language> <code_block>`

## Examples

Format Python:
```
/format python
def hello( ):
  print( "world" )
```

Format JavaScript:
```
/format javascript
const x=1;const y=2;
```
```

## Sharing Skills

### Package for ClawHub

1. Create well-documented SKILL.md
2. Include examples and troubleshooting
3. Test thoroughly
4. Create GitHub repo: `nexibot-{skillname}`
5. Submit to [ClawHub](https://clawhub.dev)

### Package for Manual Distribution

```bash
# Create tarball
cd ~/.config/nexibot/skills/
tar czf my-skill.tar.gz my-skill/

# Share the file
# Others extract and move to their skills directory:
# tar xzf my-skill.tar.gz -C ~/.config/nexibot/skills/
```

### Version Control

Keep skills in git for version control:

```bash
cd ~/.config/nexibot/skills/my-skill
git init
git add .
git commit -m "Initial release"
git tag v1.0.0
```

## Debugging Skills

### View Skill Logs

```bash
# macOS
tail -f ~/.config/nexibot/logs/skills.log

# Linux
tail -f ~/.local/share/nexibot/logs/skills.log
```

### Test Script Directly

```bash
# Set required environment
export OPENWEATHER_API_KEY="test_key"

# Run script
~/.config/nexibot/skills/weather/scripts/get_weather.sh "Tokyo"
```

### Common Issues

**"Skill not found"**
- Verify folder exists in skills directory
- Check SKILL.md is present
- Reload skills (Settings > Skills > Reload)

**"Script execution denied"**
- Check Security Level in Settings
- Verify script doesn't use blocked patterns
- Run security check: Settings > Skills > [Skill] > Security Scan

**"Missing requirements"**
- Install missing binaries (e.g., `brew install jq`)
- Set environment variables (e.g., `export API_KEY="..."`)
- Check metadata declares requirements

**"Timeout executing script"**
- Script is taking too long
- Increase timeout in config: `skill_execution_timeout: 60`
- Optimize script for performance

## Best Practices

1. **Write Clear Documentation** - Users need to understand how to use your skill
2. **Validate All Input** - Check arguments for injection/traversal attacks
3. **Declare Dependencies** - List all required binaries and env vars
4. **Error Handling** - Exit with clear error messages
5. **Keep It Focused** - One skill = one capability
6. **Test Thoroughly** - Test with various inputs
7. **Version Your Skills** - Use semantic versioning
8. **Provide Examples** - Show usage in documentation
9. **License Your Work** - Specify license for sharing
10. **Monitor Performance** - Keep scripts fast (< 10 seconds)

## Advanced: Creating Python Skills

```python
#!/usr/bin/env python3
"""
Weather skill in Python
"""
import json
import os
import sys
import urllib.request

def get_weather(city: str) -> dict:
    """Fetch weather for a city"""
    api_key = os.environ.get("OPENWEATHER_API_KEY")
    if not api_key:
        raise ValueError("OPENWEATHER_API_KEY not set")

    url = f"https://api.openweathermap.org/data/2.5/weather?q={city}&appid={api_key}&units=metric"

    with urllib.request.urlopen(url, timeout=10) as response:
        data = json.loads(response.read())

    return {
        "city": data["name"],
        "temperature": data["main"]["temp"],
        "condition": data["weather"][0]["main"],
        "humidity": data["main"]["humidity"],
        "wind_speed": data["wind"]["speed"],
    }

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: weather.py <city>")
        sys.exit(1)

    result = get_weather(sys.argv[1])
    print(json.dumps(result, indent=2))
```

SKILL.md for Python version:
```yaml
---
name: "Weather Check (Python)"
description: "Check weather using Python"
metadata:
  bins:
    - "python3"
---
```

## Advanced: Async/Concurrent Skills

For long-running operations, use background execution:

```yaml
---
name: "Batch Process"
description: "Process items in background"
command_dispatch: "tool"
---
```

This allows the skill to run asynchronously while NexiBot continues responding to the user.
