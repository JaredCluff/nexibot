# NexiBot Desktop App Setup Guide

Complete guide to installing, configuring, and using the NexiBot desktop application for macOS, Windows, and Linux.

## System Requirements

### macOS
- **OS Version**: macOS 11 (Big Sur) or later
- **Architecture**: Apple Silicon (ARM64) or Intel (x86-64)
- **RAM**: 4GB minimum (8GB recommended)
- **Disk Space**: 2GB for app + models
- **Dependencies**: None (self-contained)

### Windows
- **OS Version**: Windows 10 or later
- **Architecture**: x86-64
- **RAM**: 4GB minimum (8GB recommended)
- **Disk Space**: 2GB for app + models
- **Runtime**: Visual C++ Redistributable 2019+ (included in installer)

### Linux
- **OS**: Ubuntu 20.04+, Fedora 32+, or equivalent
- **Architecture**: x86-64
- **RAM**: 4GB minimum (8GB recommended)
- **Disk Space**: 2GB for app + models
- **Dependencies**:
  ```bash
  # Ubuntu/Debian
  sudo apt-get install -y libssl-dev libxdo-dev libxclip-dev

  # Fedora
  sudo dnf install -y openssl-devel libxdo-devel libxclip-devel
  ```

## Installation

### Option 1: Installer (Recommended)

#### macOS
1. Download `NexiBot_x.x.x_aarch64.dmg` (Apple Silicon) or `_x64.dmg` (Intel) from [Releases](https://github.com/jaredcluff/nexibot/releases)
2. Open the `.dmg` file
3. Drag `NexiBot.app` to the `/Applications` folder
4. Open `/Applications/NexiBot.app` (first launch may take a moment)
5. Grant any system permissions requested (Accessibility, Microphone, Disk Access)

#### Windows
1. Download `NexiBot_x.x.x_x64-setup.exe`
2. Run the installer as Administrator
3. Follow the setup wizard
4. Grant Windows Defender SmartScreen permission if prompted
5. NexiBot will start automatically

#### Linux
1. **DEB Package** (Ubuntu/Debian):
   ```bash
   sudo dpkg -i nexibot_x.x.x_amd64.deb
   nexibot
   ```

2. **AppImage**:
   ```bash
   chmod +x nexibot_x.x.x_amd64.AppImage
   ./nexibot_x.x.x_amd64.AppImage
   ```

3. **Manual Build**:
   ```bash
   cd nexibot && cargo tauri build
   # App will be in src-tauri/target/release/bundle/
   ```

### Option 2: From Source

```bash
# Clone repository
git clone https://github.com/jaredcluff/nexibot.git
cd nexibot/nexibot

# Install dependencies
cd ui && npm install && cd ..
cd anthropic-bridge && npm install && cd ..

# Build development version
cargo tauri dev

# Or build release
cargo tauri build
```

## First-Run Configuration

### 1. Grant System Permissions (macOS)

When you first launch NexiBot, it may request:
- **Accessibility**: Required for Computer Use (screenshots, mouse, keyboard)
- **Microphone**: Required for voice input
- **Full Disk Access**: Optional, improves file search

Grant these in **System Preferences > Security & Privacy**.

### 2. Set Up Authentication

#### Claude API Key (Free)
1. Go to [claude.ai/settings/api-keys](https://claude.ai/settings/api-keys)
2. Create a new API key (starts with `sk-ant-`)
3. In NexiBot **Settings > Authentication > Claude**
4. Paste your API key
5. Click **Save**

#### Claude Pro (OAuth)
1. In NexiBot **Settings > Authentication > Claude**
2. Click **Sign In with Claude** button
3. Browser opens to Anthropic's OAuth page
4. Authorize the app
5. You're set!

#### Other Providers (Optional)
- **OpenAI**: Paste API key in **Settings > Authentication > OpenAI**
- **Ollama**: Configure local Ollama URL in **Settings > LLM**
- **Google Gemini**: Paste API key in **Settings > Authentication > Google**

### 3. Configure Voice (Optional)

If you want voice capabilities:

1. **Settings > Voice**
2. Enable **Audio Input**
3. Select your microphone input device
4. Test **Record Test** to verify microphone
5. Choose STT backend:
   - **macOS Speech** (default, free, offline)
   - **SenseVoice** (local ONNX, high accuracy)
   - **Deepgram** (cloud, requires API key)
6. Choose TTS backend:
   - **macOS `say`** (default, free, offline)
   - **Piper** (local ONNX, many voices)
   - **ElevenLabs** (cloud, high quality)
7. Enable **Wake Word** (optional)
   - Choose wake phrase (default: "hey nexibot")
   - Test with **Test Wake Word** button

### 4. K2K Federation (Optional)

To connect to Knowledge Nexus hub:

1. Get your K2K Hub URL (from your Knowledge Nexus admin)
2. **Settings > K2K Integration**
3. Paste Hub URL
4. Paste your JWT token (if required)
5. **Test Connection** to verify
6. You can now search federated knowledge!

## Settings Overview

### Chat & Model
- **Default Model**: Primary LLM (e.g., Claude Sonnet)
- **Fallback Model**: Used if primary provider fails
- **Max Tokens**: Maximum output length (default: 4096)
- **Temperature**: Randomness (0 = deterministic, 1 = creative)

### Voice
- **Enabled**: Turn voice on/off
- **Input Device**: Microphone selection
- **Sample Rate**: Usually 16000 Hz
- **STT Backend**: Speech-to-text provider
- **TTS Backend**: Text-to-speech provider
- **Wake Word**: "hey nexibot" (customizable)
- **Wake Word Threshold**: Confidence level (0.0-1.0)

### Security & Privacy
- **Security Level**: Standard/Enhanced/Maximum
  - Standard: Balanced protection
  - Enhanced: Stricter checks
  - Maximum: Very restrictive
- **Session Encryption**: Encrypt conversation transcripts
- **Memory Limit**: Max entries before eviction
- **Log Level**: Debug/Info/Warn/Error

### Channels
- **Telegram**: Enable bot, paste token
- **Discord**: Enable bot, paste token
- **WhatsApp**: Paste account credentials
- **Slack**: Paste workspace token
- **Signal**: Configure Signal CLI connection
- **Teams**: Paste bot token
- **Matrix**: Paste homeserver URL and token

### Autonomous Mode
- **Enable Autonomous**: Allow agent to take actions without approval
- **Max Depth**: Prevent infinite loops (default: 3)
- **Concurrency**: Max parallel subagents (default: 2)
- **Timeout**: Execution timeout per agent (default: 60 sec)

### Memory & Habits
- **Memory Enabled**: Store conversations and facts
- **Auto-Extract Facts**: Learn user preferences
- **Memory Retention**: Days to keep memories
- **Max Memories**: Storage limit
- **Search Type**: Hybrid (BM25+vector) recommended

### Desktop
- **Auto-Start**: Launch on system startup
- **Minimize to Tray**: Hide window to system tray
- **Tray Icon Visible**: Always show tray icon
- **Theme**: Light/Dark/System

## Troubleshooting

### Issue: App Won't Start

**macOS:**
```bash
# Check if process is stuck
ps aux | grep nexibot

# Kill stuck process
killall nexibot-tauri

# Clear app cache and try again
rm -rf ~/Library/Application\ Support/ai.nexibot.desktop/cache
open /Applications/NexiBot.app
```

**Windows:**
```powershell
# Check Task Manager for stuck process
# End any NexiBot processes
# Run again
```

### Issue: "Unable to locate API key"

**Solution:**
1. Go to **Settings > Authentication**
2. Paste your API key again
3. Click **Test Connection**
4. Restart app if needed

### Issue: Voice Not Working

**Microphone not detected:**
- Go to **Settings > Voice > Test Record**
- If no sound is captured, check system audio settings
- Restart app and try again

**STT/TTS not working:**
- If using cloud provider, verify API key is set
- If using local (macOS/Piper), check if models are downloaded
- Try switching to different backend temporarily

**Wake word not responding:**
- Increase **Wake Word Threshold** to 0.7+ (less sensitive)
- Make sure microphone is working (test with recording first)
- Speak phrase clearly and at normal volume

### Issue: High Memory Usage

**Solution:**
1. Reduce **Max Memories** in Settings
2. Archive old conversation sessions
3. Disable **Auto-Extract Facts** if not needed
4. Check **Memory Size** in Settings > Advanced

### Issue: Slow Response Times

**Solution:**
1. Switch to faster model (e.g., Claude Haiku)
2. Reduce **Max Tokens** output length
3. Check internet connection (for cloud providers)
4. Reduce context size (fewer previous messages)
5. Disable Voice/Computer Use if not using them

### Issue: Commands Getting Blocked

**Solution:**
1. Check **Security Level** (lower sensitivity)
2. Go to **Settings > Guardrails**
3. Add command to allowlist if needed
4. Review blocked reason in logs

### Issue: Skills Not Loading

**Solution:**
1. Go to **Settings > Skills**
2. Verify skill folder path exists
3. Check skill YAML syntax (must be valid)
4. Restart app
5. Check logs for parsing errors

## Updating to New Versions

### Automatic Updates (Recommended)

NexiBot checks for updates daily:
1. When update available, notification appears
2. Click **Update Now** button
3. App restarts with new version

### Manual Update (macOS)

```bash
# Backup your configuration first (CRITICAL!)
cp ~/Library/Application\ Support/ai.nexibot.desktop/config.yaml \
   ~/Library/Application\ Support/ai.nexibot.desktop/config.yaml.backup

# Quit running app
osascript -e 'tell application "NexiBot" to quit'
sleep 2

# Remove old version
rm -rf /Applications/NexiBot.app

# Install new version (use ditto to preserve signatures!)
ditto ~/Downloads/NexiBot.app /Applications/NexiBot.app

# Relaunch
open -a NexiBot
```

### Manual Update (Windows)

1. Download new installer `.exe`
2. Run installer (will uninstall old version)
3. Follow setup wizard
4. Configuration is preserved automatically

### Manual Update (Linux)

```bash
# DEB package
sudo dpkg -i nexibot_x.x.x_amd64.deb

# AppImage
chmod +x nexibot_x.x.x_amd64.AppImage
./nexibot_x.x.x_amd64.AppImage
```

## Data Locations

Configuration and data are stored in platform-standard locations:

### macOS
```
~/Library/Application Support/ai.nexibot.desktop/
├── config.yaml                 # Main configuration
├── auth-profiles.json          # OAuth tokens
├── models/                      # Downloaded ONNX models
└── skill-cache/                # Cached skills

~/.config/nexibot/
├── memory/                      # Memory database
├── sessions/                    # Conversation history
└── logs/                        # Application logs
```

### Windows
```
C:\Users\{username}\AppData\Local\nexibot\
├── config.yaml
├── auth-profiles.json
└── models/

C:\Users\{username}\AppData\Roaming\nexibot\
├── memory/
├── sessions/
└── logs/
```

### Linux
```
~/.config/nexibot/
├── config.yaml
├── auth-profiles.json
└── models/

~/.local/share/nexibot/
├── memory/
├── sessions/
└── logs/
```

## Backup & Recovery

### Backing Up Your Data

```bash
# Create dated backup
BACKUP_DIR=~/nexibot_backup_$(date +%Y%m%d)
mkdir -p $BACKUP_DIR

# macOS
cp -r ~/Library/Application\ Support/ai.nexibot.desktop $BACKUP_DIR/
cp -r ~/.config/nexibot $BACKUP_DIR/

# Linux
cp -r ~/.config/nexibot $BACKUP_DIR/
cp -r ~/.local/share/nexibot $BACKUP_DIR/
```

### Restoring from Backup

```bash
# Quit NexiBot
# Then restore files from backup
cp -r $BACKUP_DIR/ai.nexibot.desktop \
   ~/Library/Application\ Support/  # macOS

# Relaunch NexiBot
```

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Cmd+,` (macOS) / `Ctrl+,` (Windows/Linux) | Open Settings |
| `Cmd+N` / `Ctrl+N` | New Conversation |
| `Cmd+W` / `Ctrl+W` | Close Current Tab |
| `Cmd+Shift+D` / `Ctrl+Shift+D` | Clear Chat |
| `Space` (when focused on input) | Send Message |
| `Shift+Space` | Newline in message |
| `Cmd+K` / `Ctrl+K` | Quick Command Palette |

## Performance Optimization

### For Lower-End Hardware
1. Use lighter model (Claude Haiku)
2. Disable embeddings (Settings > Memory)
3. Reduce max memories to 1000
4. Disable voice features if not needed
5. Use cloud STT/TTS instead of local

### For High Performance
1. Enable voice/computer use features
2. Increase max context size
3. Use larger model (Claude Sonnet)
4. Enable all memory features
5. Use local ONNX models for offline capabilities

## Uninstalling

### macOS
```bash
# Backup config first!
cp -r ~/Library/Application\ Support/ai.nexibot.desktop ~/nexibot_backup

# Remove application
rm -rf /Applications/NexiBot.app

# Optional: Remove data
rm -rf ~/Library/Application\ Support/ai.nexibot.desktop
rm -rf ~/.config/nexibot
```

### Windows
1. Go to **Control Panel > Programs > Programs and Features**
2. Find "NexiBot" or "Knowledge Nexus Agent"
3. Click **Uninstall**
4. Choose whether to keep or remove data

### Linux
```bash
# DEB package
sudo dpkg -r nexibot

# AppImage: just delete the file
rm nexibot_x.x.x_amd64.AppImage
```

## Getting Help

- **Documentation**: Check this file and related guides
- **Settings Issues**: Look in **Settings > Logs** for error messages
- **Voice Problems**: Test microphone in **Settings > Voice > Record Test**
- **Model Issues**: Test connection in **Settings > Authentication**
- **Reporting Bugs**: Include logs from `~/.config/nexibot/logs/`
