import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings, ChannelToolPolicy, NexiBotConfig } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import { ChannelCard } from '../shared/ChannelCard';
import { PairingSection } from '../shared/PairingSection';
import { ToolPolicySection } from '../shared/ToolPolicySection';
import { TagInput } from '../shared/TagInput';
import { notifyError, notifyInfo } from '../../../shared/notify';

const DEFAULT_TOOL_POLICY: ChannelToolPolicy = {
  denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
  allowed_tools: [],
  admin_bypass: true,
};

function cloneDefaultToolPolicy(): ChannelToolPolicy {
  return {
    denied_tools: [...DEFAULT_TOOL_POLICY.denied_tools],
    allowed_tools: [...DEFAULT_TOOL_POLICY.allowed_tools],
    admin_bypass: DEFAULT_TOOL_POLICY.admin_bypass,
  };
}

type ExtendedChannelSecurityDef = {
  key: string;
  label: string;
  allowlistKey?: string;
  allowlistLabel?: string;
  adminKey?: string;
  adminLabel?: string;
  /** Extra list field rendered as a textarea (e.g. a secondary allowlist). */
  extra_allowlist_field?: { key: string; label: string; placeholder?: string };
  credentialFields?: ExtendedChannelCredentialField[];
};

type ExtendedChannelCredentialField = {
  key: string;
  label: string;
  type: 'text' | 'password' | 'number' | 'checkbox';
  placeholder?: string;
  min?: number;
};

const EXTENDED_CHANNEL_SECURITY_DEFS: ExtendedChannelSecurityDef[] = [
  {
    key: 'bluebubbles',
    label: 'BlueBubbles',
    allowlistKey: 'allowed_handles',
    allowlistLabel: 'Allowed Handles',
    adminKey: 'admin_handles',
    adminLabel: 'Admin Handles',
    credentialFields: [
      { key: 'server_url', label: 'Server URL', type: 'text', placeholder: 'http://localhost:1234' },
      { key: 'password', label: 'Password', type: 'password', placeholder: 'BlueBubbles password' },
    ],
  },
  {
    key: 'google_chat',
    label: 'Google Chat',
    allowlistKey: 'allowed_spaces',
    allowlistLabel: 'Allowed Spaces',
    adminKey: 'admin_user_ids',
    adminLabel: 'Admin User IDs',
    credentialFields: [
      { key: 'incoming_webhook_url', label: 'Incoming Webhook URL', type: 'text', placeholder: 'https://chat.googleapis.com/v1/spaces/...'},
      { key: 'verification_token', label: 'Verification Token', type: 'password', placeholder: 'Verification token' },
    ],
  },
  {
    key: 'mattermost',
    label: 'Mattermost',
    allowlistKey: 'allowed_channel_ids',
    allowlistLabel: 'Allowed Channel IDs',
    adminKey: 'admin_user_ids',
    adminLabel: 'Admin User IDs',
    credentialFields: [
      { key: 'server_url', label: 'Server URL', type: 'text', placeholder: 'https://mattermost.example.com' },
      { key: 'bot_token', label: 'Bot Token', type: 'password', placeholder: 'Bot token' },
    ],
  },
  {
    key: 'messenger',
    label: 'Messenger',
    allowlistKey: 'allowed_sender_ids',
    allowlistLabel: 'Allowed Sender IDs',
    adminKey: 'admin_sender_ids',
    adminLabel: 'Admin Sender IDs',
    credentialFields: [
      { key: 'page_access_token', label: 'Page Access Token', type: 'password', placeholder: 'Meta page access token' },
      { key: 'verify_token', label: 'Verify Token', type: 'password', placeholder: 'Webhook verify token' },
      { key: 'app_secret', label: 'App Secret', type: 'password', placeholder: 'Meta app secret' },
    ],
  },
  {
    key: 'instagram',
    label: 'Instagram',
    allowlistKey: 'allowed_sender_ids',
    allowlistLabel: 'Allowed Sender IDs',
    adminKey: 'admin_sender_ids',
    adminLabel: 'Admin Sender IDs',
    credentialFields: [
      { key: 'access_token', label: 'Access Token', type: 'password', placeholder: 'Instagram Graph API token' },
      { key: 'instagram_account_id', label: 'Instagram Account ID', type: 'text', placeholder: '17841400000000000' },
      { key: 'verify_token', label: 'Verify Token', type: 'password', placeholder: 'Webhook verify token' },
      { key: 'app_secret', label: 'App Secret', type: 'password', placeholder: 'Meta app secret' },
    ],
  },
  {
    key: 'line',
    label: 'LINE',
    allowlistKey: 'allowed_user_ids',
    allowlistLabel: 'Allowed User IDs',
    adminKey: 'admin_user_ids',
    adminLabel: 'Admin User IDs',
    credentialFields: [
      { key: 'channel_access_token', label: 'Channel Access Token', type: 'password', placeholder: 'LINE channel access token' },
      { key: 'channel_secret', label: 'Channel Secret', type: 'password', placeholder: 'LINE channel secret' },
    ],
  },
  {
    key: 'twilio',
    label: 'Twilio',
    allowlistKey: 'allowed_numbers',
    allowlistLabel: 'Allowed Numbers',
    adminKey: 'admin_numbers',
    adminLabel: 'Admin Numbers',
    credentialFields: [
      { key: 'account_sid', label: 'Account SID', type: 'text', placeholder: 'ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' },
      { key: 'auth_token', label: 'Auth Token', type: 'password', placeholder: 'Twilio auth token' },
      { key: 'from_number', label: 'From Number', type: 'text', placeholder: '+15551234567' },
      { key: 'webhook_url', label: 'Webhook URL', type: 'text', placeholder: 'https://example.com/api/twilio/webhook' },
    ],
  },
  {
    key: 'mastodon',
    label: 'Mastodon',
    allowlistKey: 'allowed_account_ids',
    allowlistLabel: 'Allowed Account IDs',
    adminKey: 'admin_account_ids',
    adminLabel: 'Admin Account IDs',
    credentialFields: [
      { key: 'instance_url', label: 'Instance URL', type: 'text', placeholder: 'https://mastodon.social' },
      { key: 'access_token', label: 'Access Token', type: 'password', placeholder: 'Mastodon access token' },
      { key: 'respond_to_mentions', label: 'Respond To Mentions', type: 'checkbox' },
    ],
  },
  {
    key: 'rocketchat',
    label: 'Rocket.Chat',
    allowlistKey: 'allowed_room_ids',
    allowlistLabel: 'Allowed Room IDs',
    adminKey: 'admin_user_ids',
    adminLabel: 'Admin User IDs',
    credentialFields: [
      { key: 'server_url', label: 'Server URL', type: 'text', placeholder: 'https://chat.example.com' },
      { key: 'username', label: 'Username', type: 'text', placeholder: 'nexibot' },
      { key: 'password', label: 'Password', type: 'password', placeholder: 'Rocket.Chat password' },
    ],
  },
  {
    key: 'webchat',
    label: 'WebChat',
    allowlistKey: 'allowed_origins',
    allowlistLabel: 'Allowed Origins',
    extra_allowlist_field: { key: 'allowed_session_ids', label: 'Allowed Session IDs', placeholder: 'session-id-1\nsession-id-2' },
    adminKey: 'admin_session_ids',
    adminLabel: 'Admin Session IDs',
    credentialFields: [
      { key: 'port', label: 'Port', type: 'number', min: 1 },
      { key: 'require_api_key', label: 'Require API Key', type: 'checkbox' },
      { key: 'api_key', label: 'API Key', type: 'password', placeholder: 'WebChat API key' },
      { key: 'max_connections', label: 'Max Connections', type: 'number', min: 1 },
      { key: 'session_timeout_minutes', label: 'Session Timeout (minutes)', type: 'number', min: 1 },
    ],
  },
];

function parseLineList(value: string): string[] {
  return value
    .split('\n')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function normalizeDmPolicy(value: unknown): 'Allowlist' | 'Pairing' | 'Open' {
  if (value === 'Pairing' || value === 'Open') return value;
  return 'Allowlist';
}

function parseNumberField(value: string, fallback: number): number {
  const parsed = parseInt(value, 10);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function formatChannelLabel(channel: string): string {
  return channel
    .replace(/_/g, ' ')
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

interface TelegramStatus {
  enabled: boolean;
  has_token: boolean;
  bot_running: boolean;
  last_error: string | null;
}

export function ChannelsTab() {
  const { config, setConfig, pairingRequests, runtimeAllowlist } = useSettings();
  const [sendingTelegramTest, setSendingTelegramTest] = useState(false);
  const [telegramStatus, setTelegramStatus] = useState<TelegramStatus | null>(null);
  const [chatIdError, setChatIdError] = useState<string | null>(null);

  useEffect(() => {
    const load = () => {
      invoke<TelegramStatus>('get_telegram_status').then(setTelegramStatus).catch((e) => console.warn('Failed to fetch Telegram status:', e));
    };
    load();
    const interval = setInterval(load, 10_000);
    return () => clearInterval(interval);
  }, []);

  if (!config) return null;

  const configAny = config as Record<string, any>;
  const dedicatedPairingChannels = new Set([
    'telegram',
    'whatsapp',
    'discord',
    'slack',
    'signal',
    ...EXTENDED_CHANNEL_SECURITY_DEFS.map((def) => def.key),
  ]);
  const extraPairingChannels = Array.from(
    new Set([
      ...pairingRequests.map((r) => r.channel),
      ...Object.keys(runtimeAllowlist.channels),
    ])
  )
    .filter((channel) => !dedicatedPairingChannels.has(channel))
    .sort();

  const updateExtendedChannel = (channelKey: string, patch: Record<string, unknown>) => {
    const existing = (configAny[channelKey] ?? {}) as Record<string, unknown>;
    // Extended channels (bluebubbles, google_chat, etc.) are not modelled in
    // NexiBotConfig yet. We merge at the Record level and cast back to the
    // known config type — this is the only place dynamic keys need escaping.
    const merged: Record<string, unknown> = {
      ...configAny,
      [channelKey]: {
        ...existing,
        ...patch,
      },
    };
    setConfig(merged as unknown as NexiBotConfig);
  };

  return (
    <div className="tab-content">
      {/* ─── Telegram ─────────────────────────────────────────────────── */}
      <ChannelCard
        name="Telegram"
        tooltip="Chat with NexiBot through a Telegram bot. Supports text and voice messages."
        guideTitle="How to set up Telegram Bot"
        guideContent={
          <ol>
            <li>Open Telegram and search for <strong>@BotFather</strong></li>
            <li>Send <code>/newbot</code> and follow the prompts to create your bot</li>
            <li>Copy the bot token (looks like <code>123456:ABC-DEF...</code>)</li>
            <li>Paste it in the Bot Token field below and enable the toggle</li>
            <li>Restart NexiBot for the bot to come online</li>
            <li>Find your bot in Telegram by its username and start chatting!</li>
          </ol>
        }
        enabled={config.telegram?.enabled ?? false}
        onToggle={async (checked) => {
          try {
            await invoke('set_telegram_enabled', { enabled: checked });
            setConfig({ ...config, telegram: { ...(config.telegram ?? { enabled: false, bot_token: '', allowed_chat_ids: [], voice_enabled: false, dm_policy: 'Allowlist' as const }), enabled: checked } });
          } catch (error) {
            notifyError('Telegram', `Failed to update: ${error}`);
          }
        }}
      >
        {telegramStatus && config.telegram?.enabled && (
          <div style={{ marginBottom: 8 }}>
            <div className="status-indicator">
              <span className={`status-dot ${telegramStatus.bot_running ? 'healthy' : 'unhealthy'}`} />
              <span>
                {telegramStatus.bot_running
                  ? 'Bot running (polling for messages)'
                  : !telegramStatus.has_token
                    ? 'Bot stopped — no token configured'
                    : 'Bot not running'}
              </span>
            </div>
            {telegramStatus.last_error && (
              <div className="warning-banner" style={{ marginTop: 4, fontSize: 12 }}>
                {telegramStatus.last_error}
              </div>
            )}
          </div>
        )}
        <label className="field">
          <span>Bot Token <InfoTip text="The bot token from Telegram's @BotFather." /></span>
          <input
            type="password"
            value={config.telegram.bot_token || ''}
            placeholder="123456:ABC-DEF1234ghIkl..."
            onChange={(e) => setConfig({ ...config, telegram: { ...config.telegram, bot_token: e.target.value } })}
            onBlur={async (e) => {
              try { await invoke('set_telegram_bot_token', { token: e.target.value }); } catch (error) { notifyError('Telegram', `Failed to save bot token: ${error}`); }
            }}
          />
        </label>
        <label className="field">
          <span>Allowed Chat IDs (one per line, empty = allow all) <InfoTip text="Restrict which Telegram chats can interact with the bot." /></span>
          <textarea
            rows={3}
            value={(config.telegram.allowed_chat_ids ?? []).join('\n')}
            placeholder={"123456789\n-987654321"}
            onChange={(e) => {
              const lines = e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0);
              const invalid = lines.filter(s => !/^-?\d+$/.test(s));
              setChatIdError(invalid.length > 0 ? `Invalid chat ID(s): ${invalid.join(', ')}` : null);
              setConfig({
                ...config,
                telegram: {
                  ...config.telegram,
                  allowed_chat_ids: lines.map(s => parseInt(s)).filter(n => !isNaN(n)),
                },
              });
            }}
            onBlur={async () => {
              try { await invoke('set_telegram_allowed_chat_ids', { chatIds: config.telegram.allowed_chat_ids ?? [] }); } catch (error) { notifyError('Telegram', `Failed to save chat IDs: ${error}`); }
            }}
          />
          {chatIdError && <span style={{ color: 'var(--error)', fontSize: '12px' }}>{chatIdError}</span>}
        </label>
        <label className="field">
          <span>Admin Chat IDs (one per line) <InfoTip text="Chat IDs that bypass DM policy entirely. Use with caution." /></span>
          <textarea
            rows={2}
            value={(config.telegram.admin_chat_ids ?? []).join('\n')}
            placeholder={"123456789"}
            onChange={(e) => setConfig({
              ...config,
              telegram: {
                ...config.telegram,
                admin_chat_ids: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0).map(s => parseInt(s)).filter(n => !isNaN(n)),
              },
            })}
          />
        </label>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.telegram.voice_enabled ?? false}
              onChange={async (e) => {
                try {
                  await invoke('set_telegram_voice_enabled', { enabled: e.target.checked });
                  setConfig({ ...config, telegram: { ...config.telegram, voice_enabled: e.target.checked } });
                } catch (error) { notifyError('Telegram', `Failed to update voice setting: ${error}`); }
              }}
            />
            Enable Voice Messages (requires STT backend) <InfoTip text="Process voice messages sent to the bot." />
          </label>
        </div>
        <label className="field">
          <span>DM Policy <InfoTip text="How unknown senders are handled." /></span>
          <select
            value={config.telegram.dm_policy || 'Allowlist'}
            onChange={async (e) => {
              const policy = e.target.value as 'Allowlist' | 'Pairing';
              try {
                await invoke('set_telegram_dm_policy', { policy });
                setConfig({ ...config, telegram: { ...config.telegram, dm_policy: policy } });
              } catch (error) { notifyError('Telegram', `Failed to update DM policy: ${error}`); }
            }}
          >
            <option value="Allowlist">Allowlist (manual chat IDs; empty = allow all)</option>
            <option value="Pairing">Pairing (approve via code)</option>
          </select>
        </label>
        {config.telegram.dm_policy === 'Pairing' && (
          <PairingSection channel="telegram"  allowlistLabel="Chat ID" />
        )}
        <ToolPolicySection
          policy={config.telegram?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, telegram: { ...config.telegram, tool_policy: policy } })}
        />
        <button
          className="btn btn-secondary"
          disabled={sendingTelegramTest}
          onClick={async () => {
            setSendingTelegramTest(true);
            try { const result = await invoke<string>('send_telegram_test_message'); notifyInfo('Telegram', result); }
            catch (error) { notifyError('Telegram', `Failed to send test message: ${error}`); }
            finally { setSendingTelegramTest(false); }
          }}
        >
          {sendingTelegramTest ? 'Sending…' : 'Send Test Message'}
        </button>
        <div className="info-text">Sends a message to the first allowed chat ID. Bot token changes require app restart.</div>
      </ChannelCard>

      {/* ─── WhatsApp ─────────────────────────────────────────────────── */}
      <ChannelCard
        name="WhatsApp"
        tooltip="Send and receive WhatsApp messages through Meta's Cloud API."
        guideTitle="How to set up WhatsApp Cloud API"
        guideContent={
          <ol>
            <li>Create a <a href="https://business.facebook.com" target="_blank" rel="noopener noreferrer">Meta Business account</a></li>
            <li>Go to <a href="https://developers.facebook.com" target="_blank" rel="noopener noreferrer">Meta Developers</a> and create an app with WhatsApp product</li>
            <li>Get your <strong>Phone Number ID</strong> and generate a <strong>permanent access token</strong></li>
            <li>Choose a <strong>Verify Token</strong> (any secret string)</li>
            <li>Set up your webhook URL pointing to <code>{"https://your-domain:{port}/whatsapp/webhook"}</code></li>
          </ol>
        }
        enabled={config.whatsapp?.enabled ?? false}
        onToggle={async (checked) => {
          try {
            await invoke('set_whatsapp_enabled', { enabled: checked });
            setConfig({ ...config, whatsapp: { ...(config.whatsapp ?? { enabled: false, phone_number_id: '', access_token: '', verify_token: '', app_secret: '', allowed_phone_numbers: [], dm_policy: 'Allowlist' as const }), enabled: checked } });
          } catch (error) { notifyError('WhatsApp', `Failed to update: ${error}`); }
        }}
      >
        <label className="field">
          <span>Phone Number ID <InfoTip text="Your WhatsApp Business phone number ID from the Meta Developer Dashboard." /></span>
          <input type="text" value={config.whatsapp.phone_number_id || ''} placeholder="1234567890"
            onChange={(e) => setConfig({ ...config, whatsapp: { ...config.whatsapp, phone_number_id: e.target.value } })}
            onBlur={async (e) => { try { await invoke('set_whatsapp_phone_number_id', { phoneNumberId: e.target.value }); } catch (error) { notifyError('WhatsApp', `Failed to save phone number ID: ${error}`); } }}
          />
        </label>
        <label className="field">
          <span>Access Token <InfoTip text="A permanent access token from Meta Developer Dashboard." /></span>
          <input type="password" value={config.whatsapp.access_token || ''} placeholder="EAAG..."
            onChange={(e) => setConfig({ ...config, whatsapp: { ...config.whatsapp, access_token: e.target.value } })}
            onBlur={async (e) => { try { await invoke('set_whatsapp_access_token', { accessToken: e.target.value }); } catch (error) { notifyError('WhatsApp', `Failed to save access token: ${error}`); } }}
          />
        </label>
        <label className="field">
          <span>Webhook Verify Token <InfoTip text="A secret string you choose for webhook verification." /></span>
          <input type="text" value={config.whatsapp.verify_token || ''} placeholder="my-secret-verify-token"
            onChange={(e) => setConfig({ ...config, whatsapp: { ...config.whatsapp, verify_token: e.target.value } })}
            onBlur={async (e) => { try { await invoke('set_whatsapp_verify_token', { verifyToken: e.target.value }); } catch (error) { notifyError('WhatsApp', `Failed to save verify token: ${error}`); } }}
          />
        </label>
        <label className="field">
          <span>App Secret <InfoTip text="Meta app secret for validating X-Hub-Signature-256 on incoming webhooks." /></span>
          <input type="password" value={config.whatsapp.app_secret || ''} placeholder="Meta app secret"
            onChange={(e) => setConfig({ ...config, whatsapp: { ...config.whatsapp, app_secret: e.target.value } })}
            onBlur={async (e) => { try { await invoke('set_whatsapp_app_secret', { appSecret: e.target.value }); } catch (error) { notifyError('WhatsApp', `Failed to save app secret: ${error}`); } }}
          />
        </label>
        <label className="field">
          <span>Allowed Phone Numbers (one per line, empty = allow all) <InfoTip text="Restrict which phone numbers can message the bot." /></span>
          <textarea rows={3} value={(config.whatsapp.allowed_phone_numbers ?? []).join('\n')} placeholder={"15551234567\n447911123456"}
            onChange={(e) => setConfig({ ...config, whatsapp: { ...config.whatsapp, allowed_phone_numbers: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
            onBlur={async () => { try { await invoke('set_whatsapp_allowed_numbers', { numbers: config.whatsapp.allowed_phone_numbers ?? [] }); } catch (error) { notifyError('WhatsApp', `Failed to save allowed numbers: ${error}`); } }}
          />
        </label>
        <label className="field">
          <span>Admin Phone Numbers (one per line) <InfoTip text="Phone numbers that bypass DM policy entirely. Use with caution." /></span>
          <textarea rows={2} value={(config.whatsapp.admin_phone_numbers ?? []).join('\n')} placeholder={"15551234567"}
            onChange={(e) => setConfig({ ...config, whatsapp: { ...config.whatsapp, admin_phone_numbers: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <label className="field">
          <span>DM Policy <InfoTip text="How unknown senders are handled." /></span>
          <select value={config.whatsapp.dm_policy || 'Allowlist'}
            onChange={async (e) => {
              const policy = e.target.value as 'Allowlist' | 'Pairing';
              try { await invoke('set_whatsapp_dm_policy', { policy }); setConfig({ ...config, whatsapp: { ...config.whatsapp, dm_policy: policy } }); }
              catch (error) { notifyError('WhatsApp', `Failed to update DM policy: ${error}`); }
            }}
          >
            <option value="Allowlist">Allowlist (manual phone numbers; empty = allow all)</option>
            <option value="Pairing">Pairing (approve via code)</option>
          </select>
        </label>
        {config.whatsapp.dm_policy === 'Pairing' && (
          <PairingSection channel="whatsapp"  />
        )}
        <ToolPolicySection
          policy={config.whatsapp?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, whatsapp: { ...config.whatsapp, tool_policy: policy } })}
        />
        <div className="info-text">
          Webhook URL: <code>http://your-server:{config.webhooks?.port ?? 18791}/whatsapp/webhook</code>
          <br />WhatsApp requires HTTPS and a publicly-accessible URL.
        </div>
      </ChannelCard>

      {/* ─── Discord ──────────────────────────────────────────────────── */}
      <ChannelCard
        name="Discord"
        tooltip="Run a Discord bot that responds in servers and DMs."
        guideTitle="How to set up Discord Bot"
        guideContent={
          <ol>
            <li>Go to the <a href="https://discord.com/developers/applications" target="_blank" rel="noopener noreferrer">Discord Developer Portal</a></li>
            <li>Create a New Application, then go to the Bot tab</li>
            <li>Click "Reset Token" to generate a bot token and copy it</li>
            <li>Enable the "Message Content Intent" under Privileged Gateway Intents</li>
            <li>Use the OAuth2 URL Generator to invite the bot to your server with the "bot" scope and "Send Messages" + "Read Messages" permissions</li>
          </ol>
        }
        enabled={config.discord?.enabled ?? false}
        onToggle={async (checked) => {
          try {
            await invoke('set_discord_enabled', { enabled: checked });
            setConfig({ ...config, discord: { ...(config.discord ?? { enabled: false, bot_token: '', allowed_guild_ids: [], allowed_channel_ids: [], admin_user_ids: [], dm_policy: 'Allowlist' as const }), enabled: checked } });
          } catch (error) {
            notifyError('Discord', `Failed to update: ${error}`);
          }
        }}
      >
        <label className="field">
          <span>Bot Token <InfoTip text="The bot token from Discord Developer Portal." /></span>
          <input type="password" value={config.discord?.bot_token || ''} placeholder="MTIz..."
            onChange={(e) => setConfig({ ...config, discord: { ...config.discord, bot_token: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Allowed Guild IDs (one per line, empty = all guilds) <InfoTip text="Restrict which Discord servers the bot responds in." /></span>
          <textarea rows={2} value={(config.discord?.allowed_guild_ids ?? []).join('\n')} placeholder="123456789012345678"
            onChange={(e) => setConfig({ ...config, discord: { ...config.discord, allowed_guild_ids: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0).map(s => parseInt(s)).filter(n => !isNaN(n)) } })}
          />
        </label>
        <label className="field">
          <span>Allowed Channel IDs (one per line, empty = all channels) <InfoTip text="Restrict which channels the bot responds in." /></span>
          <textarea rows={2} value={(config.discord?.allowed_channel_ids ?? []).join('\n')} placeholder="123456789012345678"
            onChange={(e) => setConfig({ ...config, discord: { ...config.discord, allowed_channel_ids: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0).map(s => parseInt(s)).filter(n => !isNaN(n)) } })}
          />
        </label>
        <label className="field">
          <span>Admin User IDs (one per line) <InfoTip text="Discord user IDs that can use admin commands." /></span>
          <textarea rows={2} value={(config.discord?.admin_user_ids ?? []).join('\n')} placeholder="123456789012345678"
            onChange={(e) => setConfig({ ...config, discord: { ...config.discord, admin_user_ids: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0).map(s => parseInt(s)).filter(n => !isNaN(n)) } })}
          />
        </label>
        <label className="field">
          <span>DM Policy <InfoTip text="How direct messages from unknown users are handled." /></span>
          <select value={config.discord?.dm_policy || 'Allowlist'}
            onChange={async (e) => {
              const policy = e.target.value as 'Allowlist' | 'Pairing' | 'Open';
              try {
                await invoke('set_discord_dm_policy', { policy });
                setConfig({ ...config, discord: { ...config.discord, dm_policy: policy } });
              } catch (error) { notifyError('Discord', `Failed to update DM policy: ${error}`); }
            }}
          >
            <option value="Allowlist">Allowlist (admin IDs when set; empty = allow all DMs)</option>
            <option value="Pairing">Pairing (approve via code)</option>
            <option value="Open">Open (allow all DMs)</option>
          </select>
        </label>
        {config.discord?.dm_policy === 'Pairing' && (
          <PairingSection channel="discord" allowlistLabel="User ID" />
        )}
        <ToolPolicySection
          policy={config.discord?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, discord: { ...config.discord, tool_policy: policy } })}
        />
        <div className="info-text">Changes are saved with global Save. Enabling/disabling the Discord bot requires app restart.</div>
      </ChannelCard>

      {/* ─── Slack ────────────────────────────────────────────────────── */}
      <ChannelCard
        name="Slack"
        tooltip="Run a Slack bot that responds in channels and direct messages."
        guideTitle="How to set up Slack Bot"
        guideContent={
          <ol>
            <li>Go to <a href="https://api.slack.com/apps" target="_blank" rel="noopener noreferrer">Slack API</a> and create a new app</li>
            <li>Under "Socket Mode", enable it and generate an App-Level Token with <code>connections:write</code> scope</li>
            <li>Under "OAuth & Permissions", add bot scopes: <code>chat:write</code>, <code>app_mentions:read</code>, <code>im:history</code></li>
            <li>Install the app to your workspace and copy the Bot User OAuth Token</li>
            <li>Under "Event Subscriptions", subscribe to <code>message.im</code> and <code>app_mention</code></li>
          </ol>
        }
        enabled={config.slack?.enabled ?? false}
        onToggle={async (checked) => {
          try {
            await invoke('set_slack_enabled', { enabled: checked });
            setConfig({ ...config, slack: { ...(config.slack ?? { enabled: false, bot_token: '', app_token: '', signing_secret: '', allowed_channel_ids: [], admin_user_ids: [], dm_policy: 'Allowlist' as const }), enabled: checked } });
          } catch (error) {
            notifyError('Slack', `Failed to update: ${error}`);
          }
        }}
      >
        <label className="field">
          <span>Bot Token <InfoTip text="Bot User OAuth Token starting with xoxb-" /></span>
          <input type="password" value={config.slack?.bot_token || ''} placeholder="xoxb-..."
            onChange={(e) => setConfig({ ...config, slack: { ...config.slack, bot_token: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>App Token <InfoTip text="App-Level Token for Socket Mode, starting with xapp-" /></span>
          <input type="password" value={config.slack?.app_token || ''} placeholder="xapp-..."
            onChange={(e) => setConfig({ ...config, slack: { ...config.slack, app_token: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Signing Secret <InfoTip text="Used to verify incoming requests from Slack." /></span>
          <input type="password" value={config.slack?.signing_secret || ''} placeholder="Signing secret"
            onChange={(e) => setConfig({ ...config, slack: { ...config.slack, signing_secret: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Allowed Channel IDs (one per line, empty = all) <InfoTip text="Restrict which Slack channels the bot responds in." /></span>
          <textarea rows={2} value={(config.slack?.allowed_channel_ids ?? []).join('\n')} placeholder="C01234ABCDE"
            onChange={(e) => setConfig({ ...config, slack: { ...config.slack, allowed_channel_ids: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <label className="field">
          <span>Admin User IDs (one per line) <InfoTip text="Slack user IDs that can use admin commands." /></span>
          <textarea rows={2} value={(config.slack?.admin_user_ids ?? []).join('\n')} placeholder="U01234ABCDE"
            onChange={(e) => setConfig({ ...config, slack: { ...config.slack, admin_user_ids: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <label className="field">
          <span>DM Policy <InfoTip text="How direct messages from unknown users are handled." /></span>
          <select value={config.slack?.dm_policy || 'Allowlist'}
            onChange={async (e) => {
              const policy = e.target.value as 'Allowlist' | 'Pairing' | 'Open';
              try {
                await invoke('set_slack_dm_policy', { policy });
                setConfig({ ...config, slack: { ...config.slack, dm_policy: policy } });
              } catch (error) { notifyError('Slack', `Failed to update DM policy: ${error}`); }
            }}
          >
            <option value="Allowlist">Allowlist (admin IDs when set; empty = allow all DMs)</option>
            <option value="Pairing">Pairing (approve via code)</option>
            <option value="Open">Open (allow all DMs)</option>
          </select>
        </label>
        {config.slack?.dm_policy === 'Pairing' && (
          <PairingSection channel="slack" allowlistLabel="User ID" />
        )}
        <ToolPolicySection
          policy={config.slack?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, slack: { ...config.slack, tool_policy: policy } })}
        />
        <div className="info-text">Slack events are received at <code>/slack/events</code>. Save changes, then restart if the channel was newly enabled.</div>
      </ChannelCard>

      {/* ─── Signal ───────────────────────────────────────────────────── */}
      <ChannelCard
        name="Signal"
        tooltip="Receive and reply to Signal messages via Signal CLI REST API."
        guideTitle="How to set up Signal"
        guideContent={
          <ol>
            <li>Install <a href="https://github.com/bbernhard/signal-cli-rest-api" target="_blank" rel="noopener noreferrer">signal-cli-rest-api</a> via Docker</li>
            <li>Register or link a phone number with signal-cli</li>
            <li>Enter the API URL (default: <code>http://localhost:8080</code>) and the phone number below</li>
          </ol>
        }
        enabled={config.signal?.enabled ?? false}
        onToggle={(checked) => setConfig({ ...config, signal: { ...(config.signal ?? { enabled: false, api_url: 'http://localhost:8080', phone_number: '', allowed_numbers: [], admin_numbers: [], dm_policy: 'Allowlist' }), enabled: checked } })}
      >
        <label className="field">
          <span>API URL <InfoTip text="URL of the signal-cli REST API server." /></span>
          <input type="text" value={config.signal?.api_url || ''} placeholder="http://localhost:8080"
            onChange={(e) => setConfig({ ...config, signal: { ...config.signal, api_url: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Phone Number <InfoTip text="The Signal phone number registered with signal-cli (with country code)." /></span>
          <input type="text" value={config.signal?.phone_number || ''} placeholder="+15551234567"
            onChange={(e) => setConfig({ ...config, signal: { ...config.signal, phone_number: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Allowed Numbers (one per line, empty = allow all) <InfoTip text="Restrict which phone numbers can message the bot." /></span>
          <textarea rows={2} value={(config.signal?.allowed_numbers ?? []).join('\n')} placeholder={"+15551234567"}
            onChange={(e) => setConfig({ ...config, signal: { ...config.signal, allowed_numbers: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <label className="field">
          <span>Admin Numbers (one per line) <InfoTip text="Phone numbers that can use admin commands." /></span>
          <textarea rows={2} value={(config.signal?.admin_numbers ?? []).join('\n')} placeholder={"+15551234567"}
            onChange={(e) => setConfig({ ...config, signal: { ...config.signal, admin_numbers: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <label className="field">
          <span>DM Policy <InfoTip text="How messages from unknown numbers are handled." /></span>
          <select value={config.signal?.dm_policy || 'Allowlist'}
            onChange={async (e) => {
              const policy = e.target.value as 'Allowlist' | 'Pairing' | 'Open';
              try {
                await invoke('set_signal_dm_policy', { policy });
                setConfig({ ...config, signal: { ...config.signal, dm_policy: policy } });
              } catch (error) { notifyError('Signal', `Failed to update DM policy: ${error}`); }
            }}
          >
            <option value="Allowlist">Allowlist (allowed numbers; empty = allow all)</option>
            <option value="Pairing">Pairing (approve via code)</option>
            <option value="Open">Open (allow all)</option>
          </select>
        </label>
        {config.signal?.dm_policy === 'Pairing' && (
          <PairingSection channel="signal" allowlistLabel="Phone" />
        )}
        <ToolPolicySection
          policy={config.signal?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, signal: { ...config.signal, tool_policy: policy } })}
        />
        <div className="info-text">Signal listener reads config dynamically, but first-time enable still requires restart.</div>
      </ChannelCard>

      {/* ─── Microsoft Teams ──────────────────────────────────────────── */}
      <ChannelCard
        name="Microsoft Teams"
        tooltip="Run a Teams bot via Microsoft Bot Framework."
        guideTitle="How to set up Microsoft Teams"
        guideContent={
          <ol>
            <li>Register a bot in the <a href="https://dev.botframework.com" target="_blank" rel="noopener noreferrer">Bot Framework Portal</a></li>
            <li>Note your App ID and create an App Password</li>
            <li>Optionally restrict to specific tenant/team IDs</li>
            <li>Set the messaging endpoint to your server URL</li>
          </ol>
        }
        enabled={config.teams?.enabled ?? false}
        onToggle={(checked) => setConfig({ ...config, teams: { ...(config.teams ?? { enabled: false, app_id: '', app_password: '', tenant_id: undefined, allowed_team_ids: [] }), enabled: checked } })}
      >
        <label className="field">
          <span>App ID <InfoTip text="Microsoft App ID from Bot Framework registration." /></span>
          <input type="text" value={config.teams?.app_id || ''} placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
            onChange={(e) => setConfig({ ...config, teams: { ...config.teams, app_id: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>App Password <InfoTip text="Microsoft App Password (client secret) for authentication." /></span>
          <input type="password" value={config.teams?.app_password || ''} placeholder="App password"
            onChange={(e) => setConfig({ ...config, teams: { ...config.teams, app_password: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Tenant ID (optional) <InfoTip text="Restrict to a specific Azure AD tenant. Leave empty for multi-tenant." /></span>
          <input type="text" value={config.teams?.tenant_id || ''} placeholder="Optional tenant ID"
            onChange={(e) => setConfig({ ...config, teams: { ...config.teams, tenant_id: e.target.value || undefined } })}
          />
        </label>
        <label className="field">
          <span>Allowed Team IDs (one per line, empty = all) <InfoTip text="Restrict which Teams the bot responds in." /></span>
          <textarea rows={2} value={(config.teams?.allowed_team_ids ?? []).join('\n')} placeholder="Team ID"
            onChange={(e) => setConfig({ ...config, teams: { ...config.teams, allowed_team_ids: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <label className="field">
          <span>Admin User IDs (AAD Object IDs) <InfoTip text="AAD object IDs of admin users who bypass tool policy restrictions." /></span>
        </label>
        <TagInput
          tags={config.teams?.admin_user_ids ?? []}
          onChange={(tags) => setConfig({ ...config, teams: { ...config.teams, admin_user_ids: tags } })}
          placeholder="Add AAD object ID..."
        />
        <ToolPolicySection
          policy={config.teams?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, teams: { ...config.teams, tool_policy: policy } })}
        />
        <div className="info-text">Teams webhook endpoint: <code>/api/teams/messages</code>. Save changes before testing.</div>
      </ChannelCard>

      {/* ─── Matrix ───────────────────────────────────────────────────── */}
      <ChannelCard
        name="Matrix"
        tooltip="Connect to a Matrix homeserver for encrypted, decentralized messaging."
        guideTitle="How to set up Matrix"
        guideContent={
          <ol>
            <li>Create a bot account on your Matrix homeserver (e.g., via Element)</li>
            <li>Generate an access token for the bot account</li>
            <li>Enter the homeserver URL, access token, and bot user ID below</li>
            <li>Invite the bot to rooms where it should respond</li>
          </ol>
        }
        enabled={config.matrix?.enabled ?? false}
        onToggle={(checked) => setConfig({ ...config, matrix: { ...(config.matrix ?? { enabled: false, homeserver_url: '', access_token: '', user_id: '', allowed_room_ids: [], command_prefix: undefined }), enabled: checked } })}
      >
        <label className="field">
          <span>Homeserver URL <InfoTip text="The URL of your Matrix homeserver (e.g., https://matrix.org)." /></span>
          <input type="text" value={config.matrix?.homeserver_url || ''} placeholder="https://matrix.org"
            onChange={(e) => setConfig({ ...config, matrix: { ...config.matrix, homeserver_url: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Access Token <InfoTip text="Access token for the bot's Matrix account." /></span>
          <input type="password" value={config.matrix?.access_token || ''} placeholder="syt_..."
            onChange={(e) => setConfig({ ...config, matrix: { ...config.matrix, access_token: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>User ID <InfoTip text="The full Matrix user ID of the bot (e.g., @nexibot:matrix.org)." /></span>
          <input type="text" value={config.matrix?.user_id || ''} placeholder="@nexibot:matrix.org"
            onChange={(e) => setConfig({ ...config, matrix: { ...config.matrix, user_id: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Allowed Room IDs (one per line, empty = all joined rooms) <InfoTip text="Restrict which rooms the bot responds in." /></span>
          <textarea rows={2} value={(config.matrix?.allowed_room_ids ?? []).join('\n')} placeholder="!roomid:matrix.org"
            onChange={(e) => setConfig({ ...config, matrix: { ...config.matrix, allowed_room_ids: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <label className="field">
          <span>Command Prefix (optional) <InfoTip text="If set, the bot only responds to messages starting with this prefix (e.g., '!nexi')." /></span>
          <input type="text" value={config.matrix?.command_prefix || ''} placeholder="!nexi (optional)"
            onChange={(e) => setConfig({ ...config, matrix: { ...config.matrix, command_prefix: e.target.value || undefined } })}
          />
        </label>
        <label className="field">
          <span>Admin User IDs <InfoTip text="Matrix user IDs of admin users who bypass tool policy restrictions." /></span>
        </label>
        <TagInput
          tags={config.matrix?.admin_user_ids ?? []}
          onChange={(tags) => setConfig({ ...config, matrix: { ...config.matrix, admin_user_ids: tags } })}
          placeholder="@admin:matrix.org"
        />
        <ToolPolicySection
          policy={config.matrix?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, matrix: { ...config.matrix, tool_policy: policy } })}
        />
        <div className="info-text">Matrix sync uses long-polling. Save changes; newly enabling the channel requires restart.</div>
      </ChannelCard>

      {/* ─── Email ────────────────────────────────────────────────────── */}
      <ChannelCard
        name="Email"
        tooltip="Send and receive email messages via IMAP/SMTP."
        guideTitle="How to set up Email"
        guideContent={
          <ol>
            <li>Use a dedicated email address for the bot (e.g., nexibot@yourdomain.com)</li>
            <li>Enable IMAP access on the email account</li>
            <li>Enter the IMAP (receiving) and SMTP (sending) server details below</li>
            <li>If using Gmail, generate an App Password instead of your regular password</li>
          </ol>
        }
        enabled={config.email?.enabled ?? false}
        onToggle={(checked) => setConfig({ ...config, email: { ...(config.email ?? { enabled: false, imap_host: '', imap_port: 993, imap_username: '', imap_password: '', smtp_host: '', smtp_port: 587, smtp_username: '', smtp_password: '', from_address: '', allowed_senders: [], poll_interval_seconds: 30, folder: 'INBOX' }), enabled: checked } })}
      >
        <h4 style={{ margin: '8px 0 4px' }}>IMAP (Receiving)</h4>
        <div className="settings-row">
          <label className="field">
            <span>Host <InfoTip text="IMAP server hostname (e.g., imap.gmail.com)." /></span>
            <input type="text" value={config.email?.imap_host || ''} placeholder="imap.gmail.com"
              onChange={(e) => setConfig({ ...config, email: { ...config.email, imap_host: e.target.value } })}
            />
          </label>
          <label className="field">
            <span>Port <InfoTip text="IMAP server port. 993 for SSL, 143 for STARTTLS." /></span>
            <input type="number" value={config.email?.imap_port ?? 993}
              onChange={(e) => setConfig({ ...config, email: { ...config.email, imap_port: parseInt(e.target.value) || 993 } })}
            />
          </label>
        </div>
        <div className="settings-row">
          <label className="field">
            <span>Username <InfoTip text="IMAP login username (usually the email address)." /></span>
            <input type="text" value={config.email?.imap_username || ''} placeholder="nexibot@example.com"
              onChange={(e) => setConfig({ ...config, email: { ...config.email, imap_username: e.target.value } })}
            />
          </label>
          <label className="field">
            <span>Password <InfoTip text="IMAP password or app-specific password." /></span>
            <input type="password" value={config.email?.imap_password || ''} placeholder="Password"
              onChange={(e) => setConfig({ ...config, email: { ...config.email, imap_password: e.target.value } })}
            />
          </label>
        </div>

        <h4 style={{ margin: '12px 0 4px' }}>SMTP (Sending)</h4>
        <div className="settings-row">
          <label className="field">
            <span>Host <InfoTip text="SMTP server hostname (e.g., smtp.gmail.com)." /></span>
            <input type="text" value={config.email?.smtp_host || ''} placeholder="smtp.gmail.com"
              onChange={(e) => setConfig({ ...config, email: { ...config.email, smtp_host: e.target.value } })}
            />
          </label>
          <label className="field">
            <span>Port <InfoTip text="SMTP server port. 587 for STARTTLS, 465 for SSL." /></span>
            <input type="number" value={config.email?.smtp_port ?? 587}
              onChange={(e) => setConfig({ ...config, email: { ...config.email, smtp_port: parseInt(e.target.value) || 587 } })}
            />
          </label>
        </div>
        <div className="settings-row">
          <label className="field">
            <span>Username <InfoTip text="SMTP login username." /></span>
            <input type="text" value={config.email?.smtp_username || ''} placeholder="nexibot@example.com"
              onChange={(e) => setConfig({ ...config, email: { ...config.email, smtp_username: e.target.value } })}
            />
          </label>
          <label className="field">
            <span>Password <InfoTip text="SMTP password." /></span>
            <input type="password" value={config.email?.smtp_password || ''} placeholder="Password"
              onChange={(e) => setConfig({ ...config, email: { ...config.email, smtp_password: e.target.value } })}
            />
          </label>
        </div>

        <label className="field">
          <span>From Address <InfoTip text="The 'From' address used when sending replies." /></span>
          <input type="text" value={config.email?.from_address || ''} placeholder="nexibot@example.com"
            onChange={(e) => setConfig({ ...config, email: { ...config.email, from_address: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Allowed Senders (one per line, empty = allow all) <InfoTip text="Only process emails from these addresses." /></span>
          <textarea rows={2} value={(config.email?.allowed_senders ?? []).join('\n')} placeholder="user@example.com"
            onChange={(e) => setConfig({ ...config, email: { ...config.email, allowed_senders: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <div className="settings-row">
          <label className="field">
            <span>Poll Interval (seconds) <InfoTip text="How often to check for new emails." /></span>
            <input type="number" min={5} value={config.email?.poll_interval_seconds ?? 30}
              onChange={(e) => setConfig({ ...config, email: { ...config.email, poll_interval_seconds: parseInt(e.target.value) || 30 } })}
            />
          </label>
          <label className="field">
            <span>Folder <InfoTip text="IMAP folder to monitor. Default: INBOX." /></span>
            <input type="text" value={config.email?.folder || 'INBOX'} placeholder="INBOX"
              onChange={(e) => setConfig({ ...config, email: { ...config.email, folder: e.target.value } })}
            />
          </label>
        </div>
        <label className="field">
          <span>DM Policy <InfoTip text="How messages from unknown senders are handled." /></span>
          <select value={config.email?.dm_policy || 'Allowlist'}
            onChange={(e) => {
              const policy = e.target.value as 'Allowlist' | 'Open';
              setConfig({ ...config, email: { ...config.email, dm_policy: policy } });
            }}
          >
            <option value="Allowlist">Allowlist (allowed senders; empty = allow all)</option>
            <option value="Open">Open (allow all)</option>
          </select>
        </label>
        <ToolPolicySection
          policy={config.email?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, email: { ...config.email, tool_policy: policy } })}
        />
      </ChannelCard>

      {/* ─── Gmail ───────────────────────────────────────────────────── */}
      <ChannelCard
        name="Gmail"
        tooltip="Send and receive email via Google's Gmail API with OAuth2 authentication."
        guideTitle="How to set up Gmail"
        guideContent={
          <ol>
            <li>Go to <a href="https://console.cloud.google.com/" target="_blank" rel="noopener noreferrer">Google Cloud Console</a> and create a project</li>
            <li>Enable the <strong>Gmail API</strong> under APIs &amp; Services</li>
            <li>Create OAuth 2.0 credentials (Desktop app type)</li>
            <li>Complete the OAuth consent screen and add your email as a test user</li>
            <li>Use the OAuth Playground or a script to exchange the authorization code for a refresh token</li>
            <li>Enter the Client ID, Client Secret, and Refresh Token below</li>
          </ol>
        }
        enabled={config.gmail?.enabled ?? false}
        onToggle={(checked) => setConfig({ ...config, gmail: { ...(config.gmail ?? { enabled: false, client_id: '', client_secret: '', refresh_token: '', from_address: '', allowed_senders: [], label: 'INBOX', poll_interval_seconds: 30, max_messages_per_poll: 10, dm_policy: 'Allowlist' }), enabled: checked } })}
      >
        <label className="field">
          <span>Client ID <InfoTip text="Google OAuth2 Client ID from Cloud Console." /></span>
          <input type="text" value={config.gmail?.client_id || ''} placeholder="xxxx.apps.googleusercontent.com"
            onChange={(e) => setConfig({ ...config, gmail: { ...config.gmail, client_id: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Client Secret <InfoTip text="Google OAuth2 Client Secret." /></span>
          <input type="password" value={config.gmail?.client_secret || ''} placeholder="GOCSPX-..."
            onChange={(e) => setConfig({ ...config, gmail: { ...config.gmail, client_secret: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Refresh Token <InfoTip text="Long-lived OAuth2 refresh token used to obtain access tokens." /></span>
          <input type="password" value={config.gmail?.refresh_token || ''} placeholder="1//0..."
            onChange={(e) => setConfig({ ...config, gmail: { ...config.gmail, refresh_token: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>From Address <InfoTip text="The email address used in the From header when sending replies." /></span>
          <input type="text" value={config.gmail?.from_address || ''} placeholder="nexibot@gmail.com"
            onChange={(e) => setConfig({ ...config, gmail: { ...config.gmail, from_address: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>Allowed Senders (one per line, empty = allow all) <InfoTip text="Only process emails from these addresses." /></span>
          <textarea rows={2} value={(config.gmail?.allowed_senders ?? []).join('\n')} placeholder="user@example.com"
            onChange={(e) => setConfig({ ...config, gmail: { ...config.gmail, allowed_senders: e.target.value.split('\n').map(s => s.trim()).filter(s => s.length > 0) } })}
          />
        </label>
        <div className="settings-row">
          <label className="field">
            <span>Poll Interval (seconds) <InfoTip text="How often to check for new emails." /></span>
            <input type="number" min={5} value={config.gmail?.poll_interval_seconds ?? 30}
              onChange={(e) => setConfig({ ...config, gmail: { ...config.gmail, poll_interval_seconds: parseInt(e.target.value) || 30 } })}
            />
          </label>
          <label className="field">
            <span>Max Messages per Poll <InfoTip text="Maximum number of messages to fetch per poll cycle." /></span>
            <input type="number" min={1} value={config.gmail?.max_messages_per_poll ?? 10}
              onChange={(e) => setConfig({ ...config, gmail: { ...config.gmail, max_messages_per_poll: parseInt(e.target.value) || 10 } })}
            />
          </label>
        </div>
        <label className="field">
          <span>Label <InfoTip text="Gmail label to monitor. Default: INBOX." /></span>
          <input type="text" value={config.gmail?.label || 'INBOX'} placeholder="INBOX"
            onChange={(e) => setConfig({ ...config, gmail: { ...config.gmail, label: e.target.value } })}
          />
        </label>
        <label className="field">
          <span>DM Policy <InfoTip text="How messages from unknown senders are handled." /></span>
          <select value={config.gmail?.dm_policy || 'Allowlist'}
            onChange={(e) => {
              const policy = e.target.value as 'Allowlist' | 'Pairing' | 'Open';
              setConfig({ ...config, gmail: { ...config.gmail, dm_policy: policy } });
            }}
          >
            <option value="Allowlist">Allowlist (allowed senders; empty = allow all)</option>
            <option value="Open">Open (allow all)</option>
          </select>
        </label>
        <ToolPolicySection
          policy={config.gmail?.tool_policy ?? DEFAULT_TOOL_POLICY}
          onChange={(policy) => setConfig({ ...config, gmail: { ...config.gmail, tool_policy: policy } })}
        />
      </ChannelCard>

      <div className="settings-group">
        <h3>Additional Channel Security</h3>
        <p className="group-description">
          Configure DM acceptance and tool policy for channels that run in the backend but do not
          yet have dedicated full cards.
        </p>
        {EXTENDED_CHANNEL_SECURITY_DEFS.map((def) => {
          const channelCfg = (configAny[def.key] ?? {}) as Record<string, any>;
          const dmPolicy = normalizeDmPolicy(channelCfg.dm_policy);
          const toolPolicy = (channelCfg.tool_policy as ChannelToolPolicy | undefined)
            ?? cloneDefaultToolPolicy();
          const enabled = Boolean(channelCfg.enabled ?? false);

          const allowlistValues = def.allowlistKey
            ? (Array.isArray(channelCfg[def.allowlistKey]) ? channelCfg[def.allowlistKey] : []).map(String)
            : [];
          const adminValues = def.adminKey
            ? (Array.isArray(channelCfg[def.adminKey]) ? channelCfg[def.adminKey] : []).map(String)
            : [];

          return (
            <div key={def.key} className="mcp-server-card" style={{ marginBottom: 12 }}>
              <div className="mcp-server-header" style={{ marginBottom: 8 }}>
                <span className="mcp-server-name">{def.label}</span>
              </div>

              <div className="inline-toggle">
                <label className="toggle-label">
                  <input
                    type="checkbox"
                    checked={enabled}
                    onChange={(e) => updateExtendedChannel(def.key, { enabled: e.target.checked })}
                  />
                  Enable {def.label}
                </label>
              </div>

              {(def.credentialFields ?? []).length > 0 && (
                <>
                  <h4 style={{ margin: '10px 0 4px' }}>Credentials</h4>
                  {def.credentialFields?.map((field) => {
                    const rawValue = channelCfg[field.key];
                    if (field.type === 'checkbox') {
                      return (
                        <div key={`${def.key}-${field.key}`} className="inline-toggle">
                          <label className="toggle-label">
                            <input
                              type="checkbox"
                              checked={Boolean(rawValue ?? false)}
                              onChange={(e) => updateExtendedChannel(def.key, {
                                [field.key]: e.target.checked,
                              })}
                            />
                            {field.label}
                          </label>
                        </div>
                      );
                    }

                    if (field.type === 'number') {
                      const numericValue = Number(rawValue);
                      const fallback = Number.isFinite(numericValue)
                        ? numericValue
                        : (field.min ?? 0);
                      return (
                        <label key={`${def.key}-${field.key}`} className="field">
                          <span>{field.label}</span>
                          <input
                            type="number"
                            min={field.min}
                            value={fallback}
                            onChange={(e) => updateExtendedChannel(def.key, {
                              [field.key]: parseNumberField(e.target.value, fallback),
                            })}
                          />
                        </label>
                      );
                    }

                    return (
                      <label key={`${def.key}-${field.key}`} className="field">
                        <span>{field.label}</span>
                        <input
                          type={field.type}
                          value={typeof rawValue === 'string' ? rawValue : ''}
                          placeholder={field.placeholder}
                          onChange={(e) => updateExtendedChannel(def.key, {
                            [field.key]: e.target.value,
                          })}
                        />
                      </label>
                    );
                  })}
                </>
              )}

              {def.allowlistKey && (
                <label className="field">
                  <span>
                    {def.allowlistLabel ?? 'Allowed IDs'} (one per line, empty = allow all)
                  </span>
                  <textarea
                    rows={2}
                    value={allowlistValues.join('\n')}
                    onChange={(e) => updateExtendedChannel(def.key, {
                      [def.allowlistKey]: parseLineList(e.target.value),
                    })}
                  />
                </label>
              )}

              {def.extra_allowlist_field && (() => {
                const f = def.extra_allowlist_field;
                const vals = (Array.isArray(channelCfg[f.key]) ? channelCfg[f.key] : []).map(String);
                return (
                  <label className="field">
                    <span>{f.label} (one per line, empty = allow all)</span>
                    <textarea
                      rows={2}
                      value={vals.join('\n')}
                      placeholder={f.placeholder}
                      onChange={(e) => updateExtendedChannel(def.key, {
                        [f.key]: parseLineList(e.target.value),
                      })}
                    />
                  </label>
                );
              })()}

              {def.adminKey && (
                <label className="field">
                  <span>{def.adminLabel ?? 'Admin IDs'} (one per line)</span>
                  <textarea
                    rows={2}
                    value={adminValues.join('\n')}
                    onChange={(e) => updateExtendedChannel(def.key, {
                      [def.adminKey]: parseLineList(e.target.value),
                    })}
                  />
                </label>
              )}

              <label className="field">
                <span>DM Policy <InfoTip text="How unknown senders are handled." /></span>
                <select
                  value={dmPolicy}
                  onChange={(e) => updateExtendedChannel(def.key, {
                    dm_policy: e.target.value as 'Allowlist' | 'Pairing' | 'Open',
                  })}
                >
                  <option value="Allowlist">Allowlist (configured IDs; empty = allow all)</option>
                  <option value="Pairing">Pairing (approve via code)</option>
                  <option value="Open">Open (allow all senders)</option>
                </select>
              </label>

              {dmPolicy === 'Pairing' && (
                <PairingSection
                  channel={def.key}
                  allowlistLabel={def.key === 'webchat' ? 'Session ID' : def.allowlistLabel}
                />
              )}

              <ToolPolicySection
                policy={toolPolicy}
                onChange={(policy) => updateExtendedChannel(def.key, { tool_policy: policy })}
              />
            </div>
          );
        })}
      </div>

      {extraPairingChannels.length > 0 && (
        <div className="settings-group">
          <h3>Additional Pairing</h3>
          <p className="group-description">
            Runtime pairing queues for channels without dedicated cards in this tab.
          </p>
          {extraPairingChannels.map((channel) => (
            <div key={channel} className="mcp-server-card" style={{ marginBottom: 12 }}>
              <div className="mcp-server-header" style={{ marginBottom: 8 }}>
                <span className="mcp-server-name">{formatChannelLabel(channel)}</span>
              </div>
              <PairingSection channel={channel} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
