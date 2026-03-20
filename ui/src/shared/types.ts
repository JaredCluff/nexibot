// =============================================================================
// Shared TypeScript interfaces for NexiBot UI
// =============================================================================
// Extracted from Chat.tsx, Settings.tsx, HistorySidebar.tsx, Onboarding.tsx,
// AuthPrompt.tsx, GuardrailsPanel.tsx, and NotificationToast.tsx.
// These are the canonical definitions -- components should import from here
// once migrated to use the backend abstraction layer.
// =============================================================================

// ---------------------------------------------------------------------------
// Chat interfaces
// ---------------------------------------------------------------------------

export interface Message {
  role: 'user' | 'assistant';
  content: string;
  toolIndicators?: ToolIndicator[];
}

export interface ToolIndicator {
  name: string;
  id: string;
  status: 'running' | 'done' | 'error';
}

export interface SendMessageResponse {
  response: string;
  error?: string;
}

export interface VoiceStatus {
  state: string;
  stt_backend: string;
  tts_backend: string;
  is_sleeping: boolean;
}

export interface PushToTalkResponse {
  transcript: string;
  response: string;
  error?: string;
}

export interface SessionOverrides {
  model: string | null;
  thinking_budget: number | null;
  verbose: boolean;
  provider: string | null;
}

export interface AvailableModel {
  id: string;
  display_name: string;
  alias: string | null;
  provider: string;
  tier: string;
  available: boolean;
}

// ---------------------------------------------------------------------------
// Provider / auth status
// ---------------------------------------------------------------------------

export interface ProviderStatus {
  anthropic_configured: boolean;
  openai_configured: boolean;
  ollama_configured: boolean;
  ollama_url: string;
}

// ---------------------------------------------------------------------------
// Conversation history
// ---------------------------------------------------------------------------

export interface ConversationSession {
  session_id: string;
  title: string;
  started_at: string;
  last_active: string;
  message_count: number;
}

export interface CurrentSession {
  messages: { role: string; content: string }[];
}

// ---------------------------------------------------------------------------
// Compact result
// ---------------------------------------------------------------------------

export interface CompactResult {
  success: boolean;
  messages_before: number;
  messages_after: number;
  tokens_before: number;
  tokens_after: number;
  error?: string;
}

// ---------------------------------------------------------------------------
// Scheduled tasks
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

export interface SkillMetadata {
  name: string;
  description: string;
  user_invocable: boolean;
}

export interface Skill {
  id: string;
  metadata: SkillMetadata;
  content: string;
  path: string;
  scripts: string[];
  references: string[];
}

export interface SkillTemplate {
  id: string;
  name: string;
  description: string;
  content: string;
  user_invocable: boolean;
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Intelligent routing
// ---------------------------------------------------------------------------

export interface RoutingPurposes {
  quick_chat?: string;
  code_simple?: string;
  code_complex?: string;
  reasoning?: string;
  long_context?: string;
  agentic?: string;
  voice_default?: string;
}

export interface RoutingConfig {
  enabled: boolean;
  voice_latency_bias: boolean;
  purposes: RoutingPurposes;
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
  };
  vad: {
    enabled: boolean;
    threshold: number;
    min_speech_duration_ms: number;
    min_silence_duration_ms: number;
  };
  stt: {
    enabled: boolean;
    backend: string;
    deepgram_api_key?: string;
    openai_api_key?: string;
    sensevoice_model_path?: string;
  };
  tts: {
    enabled: boolean;
    backend: string;
    macos_voice: string;
    elevenlabs_api_key?: string;
    cartesia_api_key?: string;
    piper_model_path?: string;
    piper_voice?: string;
    espeak_voice?: string;
    windows_voice?: string;
  };
  mcp: {
    enabled: boolean;
    servers: MCPServerConfig[];
  };
  computer_use: {
    enabled: boolean;
    display_width: number;
    display_height: number;
    require_confirmation: boolean;
  };
  guardrails: GuardrailsConfig;
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
  };
  cerebras?: {
    api_key?: string;
    model?: string;
    max_tokens?: number;
  };
  routing: RoutingConfig;
  webhooks: {
    enabled: boolean;
    port: number;
    auth_token?: string;
    endpoints: WebhookEndpoint[];
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
    dm_policy: string;
  };
  whatsapp: {
    enabled: boolean;
    phone_number_id: string;
    access_token: string;
    verify_token: string;
    app_secret: string;
    allowed_phone_numbers: string[];
    admin_phone_numbers: string[];
    dm_policy: string;
  };
}

// ---------------------------------------------------------------------------
// Guardrails
// ---------------------------------------------------------------------------

export interface GuardrailsConfig {
  security_level: string;
  block_destructive_commands: boolean;
  block_sensitive_data_sharing: boolean;
  detect_prompt_injection: boolean;
  block_prompt_injection: boolean;
  confirm_external_actions: boolean;
  dangers_acknowledged: boolean;
  server_permissions: Record<string, unknown>;
  default_tool_permission: string;
  dangerous_tool_patterns: string[];
  use_dcg: boolean;
}

// ---------------------------------------------------------------------------
// MCP (Model Context Protocol) servers
// ---------------------------------------------------------------------------

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

export interface MCPPreset {
  name: string;
  description: string;
  command: string;
  args: string[];
  env: Record<string, string>;
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

export interface WebhookEndpoint {
  id: string;
  name: string;
  action: string;
  target: string;
}

// ---------------------------------------------------------------------------
// Platform info
// ---------------------------------------------------------------------------

export interface PlatformInfo {
  os: string;
  available_stt_backends: string[];
  available_tts_backends: string[];
}

// ---------------------------------------------------------------------------
// Defense / security status
// ---------------------------------------------------------------------------

export interface DefenseStatus {
  enabled: boolean;
  deberta_loaded: boolean;
  deberta_healthy: boolean;
  llama_guard_available: boolean;
}

// ---------------------------------------------------------------------------
// Soul / personality
// ---------------------------------------------------------------------------

export interface SoulTemplate {
  name: string;
  description: string;
}

export interface SoulContent {
  content: string;
}

// ---------------------------------------------------------------------------
// Heartbeat
// ---------------------------------------------------------------------------

export interface HeartbeatConfig {
  enabled: boolean;
  interval_seconds: number;
}

// ---------------------------------------------------------------------------
// Bridge status
// ---------------------------------------------------------------------------

export interface BridgeStatus {
  status: string;
}

// ---------------------------------------------------------------------------
// Smart Key Vault
// ---------------------------------------------------------------------------

export interface KeyVaultConfig {
  enabled: boolean;
  intercept_chat_input: boolean;
  intercept_config: boolean;
  intercept_tool_results: boolean;
  restore_tool_inputs: boolean;
  remote_sync_url?: string;
}

export interface VaultEntryInfo {
  proxy_key: string;
  format: string;
  label: string | null;
  created_at: string;
  last_used: string | null;
  use_count: number;
}

// ---------------------------------------------------------------------------
// Notification toast
// ---------------------------------------------------------------------------

export interface Toast {
  id: number;
  level: 'info' | 'warning' | 'error' | 'success';
  title: string;
  message: string;
  createdAt: number;
}

export interface ToastEventPayload {
  level: string;
  title: string;
  message: string;
}

// ---------------------------------------------------------------------------
// Streaming event payloads
// ---------------------------------------------------------------------------

export interface TextChunkPayload {
  text: string;
}

export interface ToolStartPayload {
  name: string;
  id: string;
}

export interface ToolResultPayload {
  name: string;
  id: string;
  success: boolean;
}

export interface ChatCompletePayload {
  response: string;
  error?: string;
}

export interface CompactStatusPayload {
  status: string;
  message: string;
}

export interface SchedulerTaskCompletePayload {
  task_name: string;
  response: string;
  success: boolean;
}
