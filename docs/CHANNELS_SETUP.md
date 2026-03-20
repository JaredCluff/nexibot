# Channels Setup Guide

Complete guide to connecting NexiBot to various messaging platforms and communication channels.

## Overview

NexiBot supports connections to 8 different messaging channels, allowing you to interact with NexiBot through your preferred platform. Each channel has different setup requirements and features.

## Supported Channels

| Channel | Status | Difficulty | Features |
|---------|--------|-----------|----------|
| Telegram | Fully Integrated | Easy | Inline results, file sharing |
| Discord | Fully Integrated | Easy | Thread replies, reactions |
| WhatsApp | Fully Integrated | Medium | Media messages, groups |
| Slack | Fully Integrated | Easy | Threading, app mentions |
| Signal | Fully Integrated | Hard | Encrypted, no API keys |
| Teams | Fully Integrated | Medium | Adaptive cards, rich formatting |
| Matrix | Fully Integrated | Hard | Federated, open source |
| Email | Fully Integrated | Medium | IMAP/SMTP, threading |

## Telegram Setup

### Prerequisites
- Telegram account
- BotFather bot (built-in to Telegram)

### Step 1: Create Bot

1. Open Telegram and search for **@BotFather**
2. Start a chat: `/start`
3. Create new bot: `/newbot`
4. Follow prompts:
   - **Bot name**: NexiBot
   - **Username**: @nexibot_yourname (must be unique)
5. Copy the **API token** (looks like `123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11`)

### Step 2: Configure in NexiBot

1. **Settings > Channels > Telegram**
2. Paste API token in **Bot Token** field
3. Toggle **Enabled**
4. Click **Test Connection**
5. Expected: Green checkmark appears

### Step 3: Start Using

1. Open Telegram
2. Search for your bot (@nexibot_yourname)
3. Start chat and send a message
4. NexiBot responds!

### Features & Commands

```
/start          - Initialize bot, show help
/help           - Show all available commands
/chat <query>   - Send message to NexiBot
/search <term>  - Search knowledge base
/memory         - View saved preferences
/settings       - Show channel settings
```

### Configuration Options

```yaml
telegram:
  enabled: true
  token: "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"

  # Webhook mode (for production)
  webhook:
    enabled: false
    url: "https://example.com/telegram"
    port: 8443

  # Polling mode (for development)
  polling:
    enabled: true
    timeout: 30

  # Bot settings
  parse_mode: "Markdown"         # or "HTML"
  max_message_length: 4096
```

### Webhook Setup (Production)

For high-volume deployments:

1. Get your public URL (e.g., https://nexibot.example.com)
2. Configure Telegram webhook:

```bash
curl -X POST https://api.telegram.org/bot<TOKEN>/setWebhook \
  -H 'Content-Type: application/json' \
  -d '{
    "url": "https://example.com/api/channels/telegram/webhook",
    "certificate": "/path/to/cert.pem"
  }'
```

3. In config.yaml:

```yaml
telegram:
  webhook:
    enabled: true
    url: "https://example.com/api/channels/telegram/webhook"
    port: 8443
```

## Discord Setup

### Prerequisites
- Discord server (create at discord.com)
- Server Administrator role

### Step 1: Create Application

1. Go to [Discord Developer Portal](https://discord.com/developers/applications)
2. Click **New Application**
3. Name it "NexiBot"
4. Copy **Application ID** and **Public Key**

### Step 2: Create Bot User

1. In left menu, click **Bot**
2. Click **Add Bot**
3. Copy **TOKEN** (keep secret!)

### Step 3: Set Bot Permissions

1. Go to **OAuth2 > URL Generator**
2. Select scopes: `bot`
3. Select permissions:
   - Send Messages
   - Read Messages/View Channels
   - Manage Messages
   - Create Public Threads
   - Send Messages in Threads
   - Embed Links
   - Attach Files
4. Copy generated URL and open in browser
5. Select your server and authorize

### Step 4: Configure in NexiBot

1. **Settings > Channels > Discord**
2. Paste:
   - **Bot Token** (from step 2)
   - **Guild ID** (server ID, right-click server > Copy Server ID)
3. Click **Test Connection**

### Step 5: Grant Permissions

In your Discord server:
1. Right-click server name
2. Go to **Server Settings > Roles**
3. Find **@NexiBot** role
4. Give permissions: Send Messages, Read Messages, Manage Messages

### Features

```
@NexiBot help          - Show commands
@NexiBot ask <query>   - Ask a question
@NexiBot search <term> - Search knowledge
@NexiBot memory show   - Show preferences
```

### Configuration

```yaml
discord:
  enabled: true
  token: "MjM4Dj...YOUR_TOKEN"
  intents:
    - MESSAGE_CONTENT
    - GUILDS
    - GUILD_MESSAGES
    - DIRECT_MESSAGES

  # Thread replies
  use_threads: true
  thread_auto_archive: 3600  # 1 hour

  # Embed rich messages
  use_embeds: true
```

### Thread Replies

NexiBot automatically creates threads for conversations:

1. User asks question
2. NexiBot creates thread
3. Thread shows as "NexiBot's reply"
4. Further messages stay in thread

To disable, set `use_threads: false`.

## WhatsApp Setup

### Prerequisites
- WhatsApp Business Account
- Business Phone Number (WhatsApp-verified)

### Step 1: Create Meta App

1. Go to [Meta App Dashboard](https://developers.facebook.com/)
2. Create New App
3. Choose **Business** as app type
4. Complete setup with business info

### Step 2: Configure WhatsApp Cloud API

1. Go to **WhatsApp > Getting Started**
2. Register phone number (must be WhatsApp-verified)
3. Verify ownership via SMS code
4. Get **Access Token** and **Phone Number ID**

### Step 3: Configure in NexiBot

1. **Settings > Channels > WhatsApp**
2. Enter:
   - **Phone Number ID** (from step 2)
   - **Access Token** (from step 2)
   - **Business Phone Number** (your WhatsApp number)
3. Click **Test Connection**

### Step 4: Test

1. Add bot phone number to contacts
2. Send message: "Hello"
3. Bot responds!

### Configuration

```yaml
whatsapp:
  enabled: true
  phone_number_id: "123456789"
  business_account_id: "abcd1234"
  access_token: "EAA...YOUR_TOKEN"

  # Webhook settings
  webhook:
    url: "https://example.com/api/channels/whatsapp/webhook"
    verify_token: "secret_verify_token_123"

  # Media uploads
  allow_media: true
  max_media_size: 16777216  # 16MB

  # Group chat
  group_support: true
```

### Webhook Setup

1. In Meta App Dashboard, go to **WhatsApp > Configuration**
2. Set Webhook URL: `https://example.com/api/channels/whatsapp/webhook`
3. Set Verify Token (from config.yaml)
4. Subscribe to webhooks: messages, message_status

## Slack Setup

### Prerequisites
- Slack workspace
- Workspace Administrator or app creation permission

### Step 1: Create Slack App

1. Go to [Slack Apps](https://api.slack.com/apps)
2. Click **Create New App**
3. Choose **From scratch**
4. Name: "NexiBot"
5. Select workspace

### Step 2: Configure OAuth Scopes

1. Go to **OAuth & Permissions**
2. Add Bot Token Scopes:
   - `chat:write`
   - `commands`
   - `mentions:read`
   - `app_mentions:read`
   - `channels:read`
   - `groups:read`
   - `im:read`
   - `users:read`
3. Install App to Workspace
4. Copy **Bot Token** (starts with `xoxb-`)

### Step 3: Enable Events

1. Go to **Event Subscriptions**
2. Toggle **Enable Events**: ON
3. Set Request URL: `https://example.com/api/channels/slack/events`
4. Subscribe to bot events:
   - `app_mention`
   - `message.channels`
   - `message.groups`
   - `message.im`

### Step 4: Configure in NexiBot

1. **Settings > Channels > Slack**
2. Enter:
   - **Bot Token** (from step 2)
   - **Signing Secret** (from Basic Information)
   - **Verification Token** (from Basic Information)
3. Click **Test Connection**

### Features

```
@NexiBot help          - Show commands
@NexiBot ask something - Ask a question
!search term           - Search knowledge
```

### Configuration

```yaml
slack:
  enabled: true
  bot_token: "xoxb-YOUR-TOKEN"
  signing_secret: "secret_123"
  verification_token: "token_456"

  # Threaded replies
  use_threads: true

  # Formatting
  use_blocks: true          # Rich formatting
  emoji_reactions: true
```

## Signal Setup

### Prerequisites
- Signal Desktop installed locally
- Signal CLI REST API running
- System capable of running Signal CLI

### Step 1: Install Signal CLI

```bash
# macOS
brew install signal-cli

# Linux
wget https://github.com/AsamK/signal-cli/releases/download/v0.13.0/signal-cli-0.13.0-Linux.tar.gz
tar xf signal-cli-0.13.0-Linux.tar.gz
sudo mv signal-cli-0.13.0 /opt/signal-cli

# Add to PATH
export PATH=$PATH:/opt/signal-cli/bin
```

### Step 2: Register Number

```bash
signal-cli register +1234567890
# Follow prompts to verify via SMS/voice
```

### Step 3: Start REST API Server

```bash
signal-cli -u +1234567890 daemon --socket 127.0.0.1:7583
```

Or as systemd service. See `/opt/signal-cli/examples/systemd/`.

### Step 4: Configure in NexiBot

1. **Settings > Channels > Signal**
2. Enter:
   - **Phone Number**: +1234567890
   - **Signal CLI URL**: http://127.0.0.1:7583
3. Click **Test Connection**

### Configuration

```yaml
signal:
  enabled: true
  phone_number: "+1234567890"
  cli_rest_url: "http://127.0.0.1:7583"

  # Timeout for Signal CLI operations
  timeout: 30

  # Message rate limit
  rate_limit: 5  # messages per second
```

### Permissions

Conversations are limited to opted-in contacts:

1. Send test message to contact
2. They must respond to enable conversation
3. Only then can NexiBot send messages

## Teams Setup

### Prerequisites
- Microsoft Teams workspace
- Teams Administrator or app creation permission

### Step 1: Create Bot Registration

1. Go to [Bot Framework Registration](https://dev.botframework.com/bots)
2. Click **Create**
3. Fill in:
   - **Bot Handle**: NexiBot
   - **Display Name**: NexiBot
   - **Description**: AI assistant
4. Copy **Microsoft App ID** and **Microsoft App Password**

### Step 2: Register with Azure

1. Go to [Azure Portal](https://portal.azure.com)
2. Create **Azure Bot** resource
3. Link to registration from step 1
4. Add **Teams Channel**

### Step 3: Configure in NexiBot

1. **Settings > Channels > Teams**
2. Enter:
   - **Microsoft App ID** (from step 1)
   - **Microsoft App Password** (from step 1)
3. Click **Test Connection**

### Step 4: Add to Teams

1. In Azure Portal, go to **Channels**
2. Click **Teams**
3. Copy bot ID
4. In Teams app, search for bot
5. Install to team

### Configuration

```yaml
teams:
  enabled: true
  microsoft_app_id: "your-app-id"
  microsoft_app_password: "your-app-password"

  # Adaptive cards for rich UI
  use_adaptive_cards: true

  # Message formatting
  markdown_support: true
```

## Matrix Setup

### Prerequisites
- Matrix homeserver (self-hosted or public)
- Matrix account (user ID like @user:example.com)

### Step 1: Create Matrix Account

For public homeserver (matrix.org):

```bash
# Use any Matrix client to register
# Or use curl
curl -X POST https://matrix.org/_matrix/client/r0/register \
  -H 'Content-Type: application/json' \
  -d '{
    "auth": {"type": "m.login.dummy"},
    "user": "nexibot",
    "password": "secure_password"
  }'
```

### Step 2: Generate Access Token

```bash
# Login to get access token
curl -X POST https://matrix.org/_matrix/client/r0/login \
  -H 'Content-Type: application/json' \
  -d '{
    "type": "m.login.password",
    "user": "nexibot",
    "password": "secure_password"
  }'
# Returns: {"access_token": "syt_...", "user_id": "@nexibot:matrix.org"}
```

### Step 3: Configure in NexiBot

1. **Settings > Channels > Matrix**
2. Enter:
   - **Homeserver URL**: https://matrix.org
   - **User ID**: @nexibot:matrix.org
   - **Access Token**: syt_...
3. Click **Test Connection**

### Configuration

```yaml
matrix:
  enabled: true
  homeserver_url: "https://matrix.org"
  user_id: "@nexibot:matrix.org"
  access_token: "syt_YOUR_TOKEN"

  # Optional: custom homeserver
  # homeserver_url: "https://matrix.example.com"

  # Message syncing
  sync_timeout: 30000  # milliseconds

  # Join rooms on invite
  auto_join_rooms: true

  # Encryption support
  encryption_enabled: false  # E2E support in development
```

### Inviting Bot to Rooms

1. Get bot's room alias
2. In any Matrix room: `/invite @nexibot:matrix.org`
3. Bot joins and is ready to chat

## Email Setup

### Prerequisites
- Email account with IMAP/SMTP access
- For Gmail: App Password (not regular password)

### Step 1: Get Credentials

**Gmail:**
1. Enable 2-Factor Authentication
2. Go to [App Passwords](https://myaccount.google.com/apppasswords)
3. Create app password for "Mail"
4. Copy generated password (16 characters)

**Other providers:**
- Get IMAP/SMTP credentials from provider
- Usually in account settings

### Step 2: Configure in NexiBot

1. **Settings > Channels > Email**
2. Enter:
   - **Email Address**: your@email.com
   - **IMAP Server**: imap.gmail.com (for Gmail)
   - **IMAP Port**: 993
   - **SMTP Server**: smtp.gmail.com (for Gmail)
   - **SMTP Port**: 587
   - **Username**: your@email.com
   - **Password**: (app password from step 1)
3. Click **Test Connection**

### Configuration

```yaml
email:
  enabled: true
  address: "your@email.com"

  # IMAP settings (reading emails)
  imap:
    server: "imap.gmail.com"
    port: 993
    username: "your@email.com"
    password: "app_password"
    use_tls: true
    folder: "INBOX"

  # SMTP settings (sending emails)
  smtp:
    server: "smtp.gmail.com"
    port: 587
    username: "your@email.com"
    password: "app_password"
    use_tls: true

  # Email threading
  track_threads: true

  # Auto-responder
  auto_respond: false
  # auto_respond_message: "Thanks for your email..."
```

### Features

- Monitors inbox for new emails
- Groups replies in threads
- Sends responses via SMTP
- Supports attachments

## Multi-Channel Management

### Running Multiple Channels

You can enable multiple channels simultaneously. NexiBot will monitor all of them:

```yaml
# config.yaml
telegram:
  enabled: true
  token: "..."

discord:
  enabled: true
  token: "..."

slack:
  enabled: true
  bot_token: "..."

whatsapp:
  enabled: true
  access_token: "..."
```

### Unified User Identity

NexiBot tracks users across channels:
- Telegram user ID -> NexiBot user ID
- Discord user ID -> NexiBot user ID
- Etc.

This allows consistent memory and preferences across platforms.

### Per-Channel Settings

Different channels can have different behaviors:

```yaml
channels:
  telegram:
    auto_respond_to_all: true
    message_delay: 0

  slack:
    use_threads: true
    thread_auto_archive: 3600

  email:
    auto_respond: true
    include_history: true
```

## Webhook Configuration

For channels using webhooks (Telegram, WhatsApp, Slack), you need a publicly accessible URL.

### Local Testing (via ngrok)

```bash
# Install ngrok
brew install ngrok

# Start ngrok tunnel to port 8000
ngrok http 8000

# Get URL like: https://abc123.ngrok.io
# Use in webhook URL: https://abc123.ngrok.io/api/channels/telegram/webhook
```

### Production (VPS)

Use a real domain with HTTPS:

```
https://nexibot.example.com/api/channels/telegram/webhook
https://nexibot.example.com/api/channels/whatsapp/webhook
https://nexibot.example.com/api/channels/slack/events
```

## Testing Channel Connections

Each channel has a test button in Settings:

```
Settings > Channels > [Channel Name] > Test Connection
```

This sends a test message to verify the setup is working.

## Troubleshooting

### "Connection Failed" Error

1. Verify API token/credentials are correct
2. Check internet connection
3. Check firewall rules
4. For webhooks: verify webhook URL is publicly accessible

### Messages Not Being Received

1. Check channel is **Enabled** in Settings
2. Verify bot has correct permissions in channel
3. Check bot's DM privacy settings
4. Review logs for error messages

### Rate Limiting

Some channels (especially WhatsApp) have rate limits:

```yaml
# Add delays between messages
rate_limit:
  min_interval: 1.0  # 1 second between messages
```

### Webhook Issues

For webhook-based channels:

```bash
# Test webhook manually
curl -X POST https://example.com/api/channels/telegram/webhook \
  -H 'Content-Type: application/json' \
  -d '{"update_id": 1, "message": {"chat": {"id": 123}, "text": "test"}}'
```

## Security Best Practices

1. **Use Environment Variables** for tokens:
   ```bash
   export TELEGRAM_TOKEN="..."
   export DISCORD_TOKEN="..."
   ```

2. **Rotate Tokens Regularly** (monthly recommended)

3. **Use Webhook Verification**:
   - Telegram: Check header `X-Telegram-Bot-Api-Secret-Token`
   - Slack: Verify signing secret
   - WhatsApp: Verify webhook token

4. **Limit Bot Permissions**:
   - Only grant needed scopes
   - Restrict to specific channels/groups
   - Disable dangerous commands

5. **Monitor Activity**:
   - Check logs regularly
   - Review bot's message history
   - Set up alerts for unusual activity
