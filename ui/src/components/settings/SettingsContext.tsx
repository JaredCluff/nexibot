import { createContext, useContext, useState, useEffect, useCallback, ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Skill, SkillTemplate } from '../../shared/types';

// ─── Config types (local to settings) ────────────────────────────────────────

export interface ChannelToolPolicy {
  denied_tools: string[];
  allowed_tools: string[];
  admin_bypass: boolean;
}

export interface GatewayConfig {
  enabled: boolean;
  port: number;
  bind_address: string;
  auth_mode: 'token' | 'password' | 'open' | 'tailscale_proxy';
  max_connections: number;
  tls_enabled: boolean;
}

export interface SandboxConfig {
  enabled: boolean;
  image: string;
  non_root_user: string;
  memory_limit: string;
  cpu_limit: number;
  network_mode: string;
  timeout_seconds: number;
  blocked_paths: string[];
  seccomp_profile?: string;
  apparmor_profile?: string;
}

export interface AgentConfigEntry {
  id: string;
  name: string;
  avatar?: string;
  model?: string;
  primary_model?: string;
  backup_model?: string;
  provider?: string;
  soul_path?: string;
  system_prompt?: string;
  is_default: boolean;
  channel_bindings: Array<{ channel: string; peer_id: string | null }>;
  capabilities: Array<{ name: string; enabled: boolean; config?: Record<string, unknown> }>;
  workspace?: Record<string, unknown>;
}

export interface NexiBotConfig {
  config_version: number;
  claude: {
    api_key?: string;
    model: string;
    fallback_model?: string;
    max_tokens: number;
    system_prompt: string;
  };
  k2k: {
    enabled: boolean;
    local_agent_url: string;
    router_url?: string;
    private_key_pem?: string;
    client_id: string;
    supermemory_enabled: boolean;
    supermemory_auto_extract: boolean;
    kn_base_url?: string;
    kn_auth_token?: string;
  };
  audio: {
    enabled: boolean;
    input_device?: string;
    sample_rate: number;
    channels: number;
  };
  wakeword: {
    enabled: boolean;
    wake_word: string;
    threshold: number;
    model_path?: string;
    sleep_timeout_seconds: number;
    conversation_timeout_seconds: number;
    stt_wakeword_enabled: boolean;
    stt_require_both: boolean;
    voice_response_enabled: boolean;
    unload_models_after_idle_secs: number;
  };
  vad: {
    enabled: boolean;
    threshold: number;
    min_speech_duration_ms: number;
    min_silence_duration_ms: number;
    require_silero: boolean;
    push_to_talk: boolean;
  };
  stt: {
    enabled: boolean;
    backend: string;
    deepgram_api_key?: string;
    openai_api_key?: string;
    sensevoice_model_path?: string;
    deepgram_rate_limit: {
      enabled: boolean;
      calls_per_minute: number;
      monthly_budget_secs: number;
      block_on_budget_exhausted: boolean;
    };
    preferred_language?: string;
  };
  tts: {
    enabled: boolean;
    backend: string;
    macos_voice: string;
    elevenlabs_api_key?: string;
    cartesia_api_key?: string;
    cartesia_voice_id?: string;
    cartesia_model?: string;
    cartesia_speed?: number;
    piper_model_path?: string;
    piper_voice?: string;
    espeak_voice?: string;
    windows_voice?: string;
    auto_language_detection: boolean;
  };
  mcp: {
    enabled: boolean;
    servers: MCPServerConfig[];
    tool_search: {
      enabled: boolean;
      top_k: number;
      similarity_threshold: number;
    };
  };
  computer_use: {
    enabled: boolean;
    display_width: number;
    display_height: number;
    require_confirmation: boolean;
  };
  guardrails: {
    security_level: string;
    block_destructive_commands: boolean;
    block_sensitive_data_sharing: boolean;
    detect_prompt_injection: boolean;
    block_prompt_injection: boolean;
    confirm_external_actions: boolean;
    dangers_acknowledged: boolean;
    server_permissions: Record<string, any>;
    default_tool_permission: string;
    dangerous_tool_patterns: string[];
    use_dcg: boolean;
  };
  autonomous_mode: {
    enabled: boolean;
    filesystem: { read: string; write: string; delete: string };
    execute: { run_command: string; run_python: string; run_node: string };
    fetch: { get_requests: string; post_requests: string };
    browser: { navigate: string; interact: string };
    computer_use: { level: string };
    mcp: Record<string, { level: string }>;
    settings_modification: { level: string };
    memory_modification: { level: string };
    soul_modification: { level: string };
    nats_publish: { level: string };
  };
  routing: {
    enabled: boolean;
    voice_latency_bias: boolean;
    purposes: {
      quick_chat?: string | null;
      code_simple?: string | null;
      code_complex?: string | null;
      reasoning?: string | null;
      long_context?: string | null;
      agentic?: string | null;
      voice_default?: string | null;
    };
  };
  defense: {
    enabled: boolean;
    deberta_enabled: boolean;
    deberta_threshold: number;
    deberta_model_path?: string;
    llama_guard_enabled: boolean;
    llama_guard_mode: string;
    llama_guard_api_url: string;
    allow_remote_llama_guard: boolean;
    fail_open: boolean;
  };
  execute: {
    enabled: boolean;
    allowed_commands: string[];
    blocked_commands: string[];
    default_timeout_ms: number;
    max_output_bytes: number;
    working_directory?: string;
    use_dcg: boolean;
    skill_runtime_exec_enabled: boolean;
  };
  filesystem: {
    enabled: boolean;
    allowed_paths: string[];
    blocked_paths: string[];
    max_read_bytes: number;
    max_write_bytes: number;
  };
  fetch: {
    enabled: boolean;
    allowed_domains: string[];
    blocked_domains: string[];
    max_response_bytes: number;
    default_timeout_ms: number;
  };
  openai: {
    api_key?: string;
    model: string;
    max_tokens: number;
    organization_id?: string;
    use_bridge: boolean;
  };
  webhooks: {
    enabled: boolean;
    port: number;
    auth_token?: string;
    endpoints: WebhookEndpoint[];
    tls: {
      enabled: boolean;
      auto_generate: boolean;
      cert_path?: string;
      key_path?: string;
    };
    rate_limit: {
      max_attempts: number;
      window_secs: number;
      lockout_secs: number;
    };
  };
  search: {
    brave_api_key?: string;
    tavily_api_key?: string;
    search_priority: string[];
    default_result_count: number;
  };
  browser: {
    enabled: boolean;
    headless: boolean;
    default_timeout_ms: number;
    chrome_path?: string;
    viewport_width: number;
    viewport_height: number;
    require_confirmation: boolean;
    allowed_domains: string[];
    use_guardrails: boolean;
  };
  telegram: {
    enabled: boolean;
    bot_token: string;
    allowed_chat_ids: number[];
    admin_chat_ids: number[];
    voice_enabled: boolean;
    voice_response: boolean;
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  whatsapp: {
    enabled: boolean;
    phone_number_id: string;
    access_token: string;
    verify_token: string;
    app_secret: string;
    allowed_phone_numbers: string[];
    admin_phone_numbers: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  discord: {
    enabled: boolean;
    bot_token: string;
    allowed_guild_ids: number[];
    allowed_channel_ids: number[];
    admin_user_ids: number[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  slack: {
    enabled: boolean;
    bot_token: string;
    app_token: string;
    signing_secret: string;
    allowed_channel_ids: string[];
    admin_user_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  signal: {
    enabled: boolean;
    api_url: string;
    phone_number: string;
    allowed_numbers: string[];
    admin_numbers: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  teams: {
    enabled: boolean;
    app_id: string;
    app_password: string;
    tenant_id?: string;
    allowed_team_ids: string[];
    admin_user_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  matrix: {
    enabled: boolean;
    homeserver_url: string;
    access_token: string;
    user_id: string;
    allowed_room_ids: string[];
    command_prefix?: string;
    admin_user_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  email: {
    enabled: boolean;
    imap_host: string;
    imap_port: number;
    imap_username: string;
    imap_password: string;
    smtp_host: string;
    smtp_port: number;
    smtp_username: string;
    smtp_password: string;
    from_address: string;
    allowed_senders: string[];
    poll_interval_seconds: number;
    folder: string;
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  rocketchat: {
    enabled: boolean;
    server_url: string;
    username: string;
    password: string;
    allowed_room_ids: string[];
    admin_user_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  webchat?: {
    enabled: boolean;
    port: number;
    allowed_origins: string[];
    require_api_key: boolean;
    api_key?: string;
    max_connections: number;
    session_timeout_minutes: number;
    allowed_session_ids: string[];
    admin_session_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  startup: {
    nexibot_at_login: boolean;
    k2k_agent_at_login: boolean;
    k2k_agent_binary: string;
  };
  google?: {
    api_key?: string;
    default_model: string;
    use_bridge: boolean;
  };
  deepseek?: {
    api_key?: string;
    api_url: string;
    default_model: string;
    use_bridge: boolean;
  };
  github_copilot?: {
    token?: string;
    api_url: string;
  };
  minimax?: {
    api_key?: string;
    api_url: string;
    default_model: string;
  };
  cerebras?: {
    api_key?: string;
    model: string;
    max_tokens: number;
  };
  qwen?: {
    api_key?: string;
    api_url: string;
    default_model: string;
  };
  ollama?: {
    enabled: boolean;
    url: string;
    model: string;
  };
  lmstudio?: {
    url: string;
    model: string;
  };
  gmail?: {
    enabled: boolean;
    client_id: string;
    client_secret: string;
    refresh_token: string;
    from_address: string;
    allowed_senders: string[];
    label: string;
    poll_interval_seconds: number;
    max_messages_per_poll: number;
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  bluebubbles?: {
    enabled: boolean;
    server_url: string;
    password: string;
    allowed_handles: string[];
    admin_handles: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  google_chat?: {
    enabled: boolean;
    incoming_webhook_url: string;
    hmac_secret: string;
    verification_token: string;
    allowed_spaces: string[];
    admin_user_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  mattermost?: {
    enabled: boolean;
    server_url: string;
    bot_token: string;
    team_name?: string;
    allowed_channel_ids: string[];
    admin_user_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  messenger?: {
    enabled: boolean;
    page_access_token: string;
    verify_token: string;
    app_secret: string;
    allowed_sender_ids: string[];
    admin_sender_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  instagram?: {
    enabled: boolean;
    access_token: string;
    instagram_account_id: string;
    verify_token: string;
    app_secret: string;
    allowed_sender_ids: string[];
    admin_sender_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  line?: {
    enabled: boolean;
    channel_access_token: string;
    channel_secret: string;
    allowed_user_ids: string[];
    admin_user_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  twilio?: {
    enabled: boolean;
    account_sid: string;
    auth_token: string;
    from_number: string;
    webhook_url: string;
    allowed_numbers: string[];
    admin_numbers: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  mastodon?: {
    enabled: boolean;
    instance_url: string;
    access_token: string;
    respond_to_mentions: boolean;
    allowed_account_ids: string[];
    admin_account_ids: string[];
    dm_policy: 'Allowlist' | 'Pairing' | 'Open';
    tool_policy: ChannelToolPolicy;
  };
  logging: {
    enabled: boolean;
    level: string;
    file_enabled: boolean;
    file_path?: string;
    max_file_size_mb: number;
    max_files: number;
    console_enabled: boolean;
    redact_secrets: boolean;
    ring_buffer_size: number;
  };
  nats: {
    enabled: boolean;
    url: string;
    inbound_subject: string;
  };
  defaults?: {
    provider: string;
    model: string;
    backup_model?: string;
  };
  agents: AgentConfigEntry[];
  scheduled_tasks: {
    enabled: boolean;
    tasks: ScheduledTask[];
  };
  yolo_mode: {
    default_duration_secs?: number;
    allow_model_request: boolean;
  };
  session_encryption: {
    enabled: boolean;
    passphrase_keyring_key?: string;
  };
  lsp: {
    servers: Record<string, { command: string; args: string[]; extensions: string[] }>;
  };
  network_policy: {
    version: number;
    default_action: 'Deny' | 'Allow';
    endpoints: Record<string, { hosts: string[]; ports: number[]; allowed_methods: string[] }>;
  };
  managed_policy: {
    enabled: boolean;
    kn_server_url: string;
    service_token?: string;
  };
  external_skill_dirs: string[];
  auto_discover_formats: string[];
  agent_engine_url?: string;
  gateway: GatewayConfig;
  sandbox: SandboxConfig;
  key_vault: {
    enabled: boolean;
    intercept_chat_input: boolean;
    intercept_config: boolean;
    intercept_tool_results: boolean;
    restore_tool_inputs: boolean;
    remote_sync_url?: string;
  };
  gated_shell: {
    enabled: boolean;
    debug_mode: boolean;
    record_sessions: boolean;
    recordings_dir?: string;
    shell_binary: string;
    command_timeout_secs: number;
    max_output_bytes: number;
    max_audit_entries: number;
    sentinel_prefix: string;
    policy: {
      deny_patterns: string[];
      max_concurrent_sessions: number;
    };
    discovery: {
      enabled: boolean;
      track_env_changes: boolean;
      min_secret_length: number;
      extra_patterns: Array<{ name: string; pattern: string; format: string }>;
    };
    plugins: {
      enabled: boolean;
      plugin_dir?: string;
      trusted_keys: string[];
    };
    tmux: {
      enabled: boolean;
      poll_interval_ms: number;
      content_stable_ms: number;
      wait_timeout_ms: number;
      max_sessions: number;
      custom_agents: Array<{ name: string; ready_pattern?: string; running_pattern?: string; approval_pattern?: string; error_pattern?: string }>;
    };
  };
}

export interface VaultEntry {
  proxy_key: string;
  format: string;
  label: string | null;
  created_at: string;
  last_used: string | null;
  use_count: number;
}

export interface WebhookEndpoint {
  id: string;
  name: string;
  action: 'TriggerTask' | 'SendMessage';
  target: string;
}

export interface MCPServerConfig {
  name: string;
  enabled: boolean;
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface MCPServerInfo {
  name: string;
  command: string;
  enabled: boolean;
  status: string | { Error: string };
  tool_count: number;
  tools: MCPDiscoveredTool[];
}

export interface MCPDiscoveredTool {
  name: string;
  prefixed_name: string;
  description: string;
  server_name: string;
}

export interface PlatformInfo {
  os: string;
  available_stt_backends: string[];
  available_tts_backends: string[];
}

export interface DefenseStatus {
  enabled: boolean;
  initialized: boolean;
  deberta_loaded: boolean;
  llama_guard_loaded: boolean;
  deberta_threshold: number;
  llama_guard_mode: string;
  llama_guard_api_url: string;
}

export interface SoulTemplate {
  name: string;
  description: string;
  path: string;
}

export interface Soul {
  path: string;
  content: string;
  last_modified: string;
  version: string;
}

export interface HeartbeatConfig {
  enabled: boolean;
  interval_seconds: number;
}

export interface ScheduledTask {
  id: string;
  name: string;
  schedule: string;
  prompt: string;
  enabled: boolean;
  run_if_missed: boolean;
  last_run: string | null;
}

export interface TaskExecutionResult {
  task_id: string;
  task_name: string;
  response: string;
  timestamp: string;
  success: boolean;
}

export interface AvailableModel {
  id: string;
  display_name: string;
  provider: string;
  available: boolean;
  size_score?: number;
}

export interface MCPPreset {
  name: string;
  description: string;
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface PairingRequest {
  id: string;
  code: string;
  channel: string;
  display_name: string | null;
  created_at: string;
  expires_at: string; // ISO 8601 from Rust DateTime<Utc>
}

export interface RuntimeAllowlist {
  telegram: number[];
  whatsapp: string[];
  channels: Record<string, string[]>;
}

export interface AgentInfo {
  id: string;
  name: string;
  avatar: string | null;
  model: string | null;
  is_default: boolean;
  channel_bindings: { channel: string; peer_id: string | null }[];
}

export interface OAuthProfile {
  provider: string;
  profile_name: string;
  expires_at: number; // Unix timestamp (seconds) from Rust u64
  token_type: string;
  scope: string | null;
}

export interface OAuthStatus {
  provider: string;
  profile_name: string;
  is_expiring: boolean;
  expires_at: number; // Unix timestamp (seconds) from Rust u64
  has_refresh_token: boolean;
}

export interface SubscriptionInfo {
  provider: string;
  status: 'Active' | 'Inactive' | 'Expired' | 'Pending';
  tier: string;
  expires_at: number | null; // Unix timestamp (seconds) from Rust Option<u64>
}

export type ToolPermissions = Record<string, { default_permission: string; tool_overrides: Record<string, string> }>;

export interface VoiceServiceStatus {
  state: string;
  stt_backend: string;
  tts_backend: string;
  is_sleeping: boolean;
  voice_response_enabled: boolean;
  wakeword_enabled: boolean;
}

// ─── Context value ───────────────────────────────────────────────────────────

export interface SettingsContextValue {
  config: NexiBotConfig | null;
  setConfig: (config: NexiBotConfig) => void;
  updateConfig: (updater: (prev: NexiBotConfig) => NexiBotConfig) => void;
  saveConfig: () => Promise<void>;
  loadConfig: () => Promise<void>;
  hasUnsavedChanges: boolean;
  isSaving: boolean;
  saveMessage: string;
  setSaveMessage: (msg: string) => void;
  platformInfo: PlatformInfo | null;
  pairingRequests: PairingRequest[];
  runtimeAllowlist: RuntimeAllowlist;
  loadPairingData: () => Promise<void>;
  // MCP
  mcpServers: MCPServerInfo[];
  loadMcpServers: () => Promise<void>;
  // Models
  availableModels: AvailableModel[];
  modelsLoading: boolean;
  loadModels: () => Promise<void>;
  // Defense
  defenseStatus: DefenseStatus | null;
  // Soul
  soulTemplates: SoulTemplate[];
  currentSoul: string;
  setCurrentSoul: (soul: string) => void;
  // Heartbeat
  heartbeatConfig: HeartbeatConfig | null;
  setHeartbeatConfig: (config: HeartbeatConfig) => void;
  heartbeatRunning: boolean;
  setHeartbeatRunning: (running: boolean) => void;
  // Bridge
  bridgeStatus: string | null;
  setBridgeStatus: (status: string | null) => void;
  // Supermemory
  supermemoryAvailable: boolean | null;
  checkSupermemory: () => Promise<void>;
  checkingSupermemory: boolean;
  // Scheduler
  scheduledTasks: ScheduledTask[];
  schedulerEnabled: boolean;
  setSchedulerEnabled: (enabled: boolean) => void;
  schedulerResults: TaskExecutionResult[];
  loadSchedulerData: () => Promise<void>;
  // Skills
  skills: Skill[];
  skillTemplates: SkillTemplate[];
  loadSkillsData: () => Promise<void>;
  // Startup
  startupConfig: { nexibot_at_login: boolean; k2k_agent_at_login: boolean; k2k_agent_binary: string };
  loadStartupConfig: () => Promise<void>;
  // Agents
  agents: AgentInfo[];
  activeGuiAgent: string;
  setActiveGuiAgent: (id: string) => void;
  loadAgentsData: () => Promise<void>;
  // Accessibility
  accessibilityPermissions: boolean | null;
  checkAccessibility: () => Promise<void>;
  // OAuth
  oauthProfiles: OAuthProfile[];
  oauthStatus: OAuthStatus | null;
  loadOAuthData: () => Promise<void>;
  // Subscriptions
  subscriptions: SubscriptionInfo[];
  loadSubscriptions: () => Promise<void>;
  // Voice service
  voiceServiceStatus: VoiceServiceStatus | null;
  loadVoiceStatus: () => Promise<void>;
  // Tool permissions
  toolPermissions: ToolPermissions;
  loadToolPermissions: () => Promise<void>;
  // Load error
  loadError: string | null;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error('useSettings must be used within SettingsProvider');
  return ctx;
}

// ─── Provider ────────────────────────────────────────────────────────────────

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [config, setConfig] = useState<NexiBotConfig | null>(null);
  const [lastSavedConfigJson, setLastSavedConfigJson] = useState<string>('');
  const [platformInfo, setPlatformInfo] = useState<PlatformInfo | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const [saveMessage, setSaveMessage] = useState('');
  const [mcpServers, setMcpServers] = useState<MCPServerInfo[]>([]);
  const [accessibilityPermissions, setAccessibilityPermissions] = useState<boolean | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [defenseStatus, setDefenseStatus] = useState<DefenseStatus | null>(null);
  const [bridgeStatus, setBridgeStatus] = useState<string | null>(null);
  const [soulTemplates, setSoulTemplates] = useState<SoulTemplate[]>([]);
  const [currentSoul, setCurrentSoul] = useState<string>('');
  const [heartbeatConfig, setHeartbeatConfig] = useState<HeartbeatConfig | null>(null);
  const [heartbeatRunning, setHeartbeatRunning] = useState(false);
  const [supermemoryAvailable, setSupermemoryAvailable] = useState<boolean | null>(null);
  const [checkingSupermemory, setCheckingSupermemory] = useState(false);
  const [scheduledTasks, setScheduledTasks] = useState<ScheduledTask[]>([]);
  const [schedulerEnabled, setSchedulerEnabled] = useState(false);
  const [schedulerResults, setSchedulerResults] = useState<TaskExecutionResult[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [skillTemplates, setSkillTemplates] = useState<SkillTemplate[]>([]);
  const [availableModels, setAvailableModels] = useState<AvailableModel[]>([]);
  const [modelsLoading, setModelsLoading] = useState(true);
  const [startupConfig, setStartupConfig] = useState({ nexibot_at_login: false, k2k_agent_at_login: false, k2k_agent_binary: '' });
  const [pairingRequests, setPairingRequests] = useState<PairingRequest[]>([]);
  const [runtimeAllowlist, setRuntimeAllowlist] = useState<RuntimeAllowlist>({ telegram: [], whatsapp: [], channels: {} });
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [activeGuiAgent, setActiveGuiAgent] = useState<string>('');
  const [oauthProfiles, setOauthProfiles] = useState<OAuthProfile[]>([]);
  const [oauthStatus, setOauthStatus] = useState<OAuthStatus | null>(null);
  const [subscriptions, setSubscriptions] = useState<SubscriptionInfo[]>([]);
  const [voiceServiceStatus, setVoiceServiceStatus] = useState<VoiceServiceStatus | null>(null);
  const [toolPermissions, setToolPermissions] = useState<ToolPermissions>({});

  const loadConfig = useCallback(async () => {
    try {
      const cfg = await invoke<NexiBotConfig>('get_config');
      setConfig(cfg);
      setLastSavedConfigJson(JSON.stringify(cfg));
      setLoadError(null);
    } catch (error) {
      setLoadError(`Failed to load config: ${error}`);
    }
  }, []);

  const loadPlatformInfo = useCallback(async () => {
    try {
      const info = await invoke<PlatformInfo>('get_platform_info');
      setPlatformInfo(info);
    } catch {
      // Platform info is supplementary; silently ignore failures
    }
  }, []);

  const loadMcpServers = useCallback(async () => {
    try {
      const servers = await invoke<MCPServerInfo[]>('list_mcp_servers');
      setMcpServers(servers);
    } catch {
      // MCP server list is non-critical
    }
  }, []);

  const loadModels = useCallback(async () => {
    setModelsLoading(true);
    try {
      const models = await invoke<AvailableModel[]>('get_available_models');
      setAvailableModels(models);
    } catch {
      // Model list is non-critical
    } finally {
      setModelsLoading(false);
    }
  }, []);

  const loadDefenseStatus = useCallback(async () => {
    try {
      const status = await invoke<DefenseStatus>('get_defense_status');
      setDefenseStatus(status);
    } catch { /* not critical */ }
  }, []);

  const loadAdvancedData = useCallback(async () => {
    try {
      const templates = await invoke<SoulTemplate[]>('list_soul_templates');
      setSoulTemplates(templates);
    } catch { /* not critical */ }
    try {
      const soul = await invoke<{ content: string }>('get_soul');
      setCurrentSoul(soul.content || '');
    } catch { /* not critical */ }
    try {
      const status = await invoke<string | { status: string }>('get_bridge_status');
      if (typeof status === 'string') {
        setBridgeStatus(status);
      } else if (status && typeof status === 'object') {
        const key = Object.keys(status)[0];
        setBridgeStatus(key || 'Unknown');
      }
    } catch {
      setBridgeStatus('Unknown');
    }
    try {
      const hbConfig = await invoke<HeartbeatConfig>('get_heartbeat_config');
      setHeartbeatConfig(hbConfig);
      const running = await invoke<boolean>('is_heartbeat_running');
      setHeartbeatRunning(running);
    } catch { /* not critical */ }
  }, []);

  const checkSupermemory = useCallback(async () => {
    setCheckingSupermemory(true);
    setSupermemoryAvailable(null);
    try {
      const available = await invoke<boolean>('is_supermemory_available');
      setSupermemoryAvailable(available);
    } catch {
      setSupermemoryAvailable(false);
    } finally {
      setCheckingSupermemory(false);
    }
  }, []);

  const loadSchedulerData = useCallback(async () => {
    try {
      const tasks = await invoke<ScheduledTask[]>('list_scheduled_tasks');
      setScheduledTasks(tasks);
    } catch { /* not critical */ }
    try {
      const enabled = await invoke<boolean>('get_scheduler_enabled');
      setSchedulerEnabled(enabled);
    } catch { /* not critical */ }
    try {
      const results = await invoke<TaskExecutionResult[]>('get_scheduler_results');
      setSchedulerResults(results);
    } catch { /* not critical */ }
  }, []);

  const loadSkillsData = useCallback(async () => {
    try {
      const loadedSkills = await invoke<Skill[]>('list_skills');
      setSkills(loadedSkills);
    } catch { /* not critical */ }
    try {
      const templates = await invoke<SkillTemplate[]>('list_skill_templates');
      setSkillTemplates(templates);
    } catch { /* not critical */ }
  }, []);

  const loadStartupConfig = useCallback(async () => {
    try {
      const cfg = await invoke<typeof startupConfig>('get_startup_config');
      setStartupConfig(cfg);
    } catch { /* not critical */ }
  }, []);

  const loadPairingData = useCallback(async () => {
    try {
      const reqs = await invoke<PairingRequest[]>('list_pairing_requests');
      setPairingRequests(reqs);
    } catch { /* not critical */ }
    try {
      const al = await invoke<RuntimeAllowlist>('get_runtime_allowlist');
      setRuntimeAllowlist(al);
    } catch { /* not critical */ }
  }, []);

  const loadAgentsData = useCallback(async () => {
    try {
      const list = await invoke<AgentInfo[]>('list_agents');
      setAgents(list);
    } catch { /* not critical */ }
    try {
      const active = await invoke<string>('get_active_gui_agent');
      setActiveGuiAgent(active);
    } catch { /* not critical */ }
  }, []);

  const loadOAuthData = useCallback(async () => {
    try {
      const profiles = await invoke<OAuthProfile[]>('list_oauth_profiles');
      setOauthProfiles(profiles);
    } catch { /* not critical */ }
    try {
      const status = await invoke<OAuthStatus>('get_oauth_status');
      setOauthStatus(status);
    } catch { /* not critical */ }
  }, []);

  const loadSubscriptions = useCallback(async () => {
    try {
      const subs = await invoke<SubscriptionInfo[]>('list_subscriptions');
      setSubscriptions(subs);
    } catch { /* not critical */ }
  }, []);

  const loadVoiceStatus = useCallback(async () => {
    try {
      const status = await invoke<VoiceServiceStatus>('get_voice_status');
      setVoiceServiceStatus(status);
    } catch { /* not critical */ }
  }, []);

  const loadToolPermissions = useCallback(async () => {
    try {
      const perms = await invoke<ToolPermissions>('get_tool_permissions');
      setToolPermissions(perms);
    } catch { /* not critical */ }
  }, []);

  const checkAccessibility = useCallback(async () => {
    try {
      const result = await invoke<boolean>('check_accessibility_permissions');
      setAccessibilityPermissions(result);
    } catch {
      setAccessibilityPermissions(null);
    }
  }, []);

  const saveConfig = useCallback(async () => {
    if (!config) return;
    setIsSaving(true);
    setSaveMessage('');
    try {
      await invoke('update_config', { newConfig: config });
      invoke<AvailableModel[]>('refresh_model_cache').then(models => {
        setAvailableModels(models);
      }).catch((e) => console.warn('Failed to refresh model cache:', e));
      setLastSavedConfigJson(JSON.stringify(config));
      setSaveMessage('Settings saved');
      setTimeout(() => setSaveMessage(''), 3000);
    } catch (error) {
      setSaveMessage(`Failed to save: ${error}`);
    } finally {
      setIsSaving(false);
    }
  }, [config]);

  const updateConfig = useCallback((updater: (prev: NexiBotConfig) => NexiBotConfig) => {
    setConfig(prev => prev ? updater(prev) : prev);
  }, []);

  const hasUnsavedChanges = !!config && JSON.stringify(config) !== lastSavedConfigJson;

  // Load everything on mount
  useEffect(() => {
    loadConfig();
    loadPlatformInfo();
    loadMcpServers();
    loadModels();
    // checkAccessibility() is intentionally NOT called here — invoking it at
    // startup triggers the macOS "control System Events" TCC dialog on every
    // fresh install or after a code-signature change.  It is called lazily
    // when the user opens the Computer Use / Tools settings tab instead.
    loadDefenseStatus();
    loadAdvancedData();
    checkSupermemory();
    loadSchedulerData();
    loadSkillsData();
    loadStartupConfig();
    loadPairingData();
    loadAgentsData();
    loadOAuthData();
    loadSubscriptions();
    loadVoiceStatus();
    loadToolPermissions();
  }, []);

  // Backend config hot-reload signal.
  // This keeps Settings in sync when config changes outside this panel.
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen('config:changed', () => {
      // Avoid clobbering in-progress edits in the settings form.
      if (hasUnsavedChanges) {
        setSaveMessage('Config changed externally. Save or close settings to refresh.');
        return;
      }

      // Reload all data in a single async batch to minimize re-renders.
      // React 18 batches setState calls within the same microtask, so
      // awaiting them sequentially prevents 11 simultaneous re-renders.
      (async () => {
        await loadConfig();
        await Promise.allSettled([
          loadModels(),
          loadDefenseStatus(),
          loadSchedulerData(),
          loadStartupConfig(),
          loadPairingData(),
          loadAgentsData(),
          loadOAuthData(),
          loadSubscriptions(),
          loadVoiceStatus(),
          loadToolPermissions(),
        ]);
      })();
    }).then((fn) => {
      unlisten = fn;
    }).catch(() => {
      // Ignore in non-Tauri test contexts.
    });

    return () => {
      unlisten?.();
    };
  }, [
    hasUnsavedChanges,
    loadAgentsData,
    loadConfig,
    loadDefenseStatus,
    loadModels,
    loadOAuthData,
    loadPairingData,
    loadSchedulerData,
    loadStartupConfig,
    loadSubscriptions,
    loadToolPermissions,
    loadVoiceStatus,
  ]);

  // Poll for new pairing requests every 30s.
  // The backend channel handlers have no AppHandle at the pairing callsite,
  // so they cannot emit an event — polling is the correct pattern here.
  useEffect(() => {
    const interval = setInterval(() => { loadPairingData(); }, 30_000);
    return () => clearInterval(interval);
  }, [loadPairingData]);

  const value: SettingsContextValue = {
    config, setConfig: setConfig as (c: NexiBotConfig) => void, updateConfig, saveConfig, loadConfig,
    hasUnsavedChanges,
    isSaving, saveMessage, setSaveMessage,
    platformInfo,
    pairingRequests, runtimeAllowlist, loadPairingData,
    mcpServers, loadMcpServers,
    availableModels, modelsLoading, loadModels,
    defenseStatus,
    soulTemplates, currentSoul, setCurrentSoul,
    heartbeatConfig, setHeartbeatConfig, heartbeatRunning, setHeartbeatRunning,
    bridgeStatus, setBridgeStatus,
    supermemoryAvailable, checkSupermemory, checkingSupermemory,
    scheduledTasks, schedulerEnabled, setSchedulerEnabled, schedulerResults, loadSchedulerData,
    skills, skillTemplates, loadSkillsData,
    startupConfig, loadStartupConfig,
    agents, activeGuiAgent, setActiveGuiAgent, loadAgentsData,
    accessibilityPermissions, checkAccessibility,
    oauthProfiles, oauthStatus, loadOAuthData,
    subscriptions, loadSubscriptions,
    voiceServiceStatus, loadVoiceStatus,
    toolPermissions, loadToolPermissions,
    loadError,
  };

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}
