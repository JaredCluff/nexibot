/** Shared types for the Chat component family. */

export interface Message {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  timestamp: Date;
  isError?: boolean;
  toolIndicators?: ToolIndicator[];
  model?: string;
}

export type ToolErrorKind = 'timeout' | 'rate_limit' | 'auth_failed' | 'network_error' | 'server_error' | 'other';

export interface ToolIndicator {
  name: string;
  id: string;
  status: 'running' | 'done' | 'error' | 'retrying';
  /** Set when status === 'error' or 'retrying'. */
  errorKind?: ToolErrorKind;
  /** Plain-English error message shown in the UI. */
  errorMessage?: string;
  /** Seconds remaining before the next retry attempt. */
  retryCountdown?: number;
  /** Current attempt number (1-based). */
  attempt?: number;
  /** Maximum attempts for this tool error. */
  maxAttempts?: number;
}

export interface VoiceStatus {
  state: string;
  stt_backend: string;
  tts_backend: string;
  is_sleeping: boolean;
  voice_response_enabled: boolean;
  wakeword_enabled: boolean;
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

export interface BackgroundTaskUI {
  id: string;
  description: string;
  status: 'running' | 'completed' | 'failed';
  progress?: string;
}
