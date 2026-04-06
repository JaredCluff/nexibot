import { useState, useRef, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import GuardrailsPanel from './GuardrailsPanel';
import MessageList, { extractDisplayText } from './MessageList';
import VoiceBar from './VoiceBar';
import SlashCommandPalette from './SlashCommandPalette';
import { notifyError } from '../shared/notify';
import type {
  Message, ToolIndicator, SessionOverrides, AvailableModel, BackgroundTaskUI,
} from './chat-types';
import './Chat.css';

// ─── Static data ──────────────────────────────────────────────────────────────

const FALLBACK_DISPLAY: Record<string, string> = {
  'claude-opus-4-6': 'Opus 4.6',
  'claude-sonnet-4-6': 'Sonnet 4.6',
  'claude-haiku-4-5': 'Haiku 4.5',
  'claude-haiku-4-5-20251001': 'Haiku 4.5',
  'gpt-4o': 'GPT-4o',
  'gpt-4o-mini': 'GPT-4o mini',
  'o1': 'o1',
  'o1-mini': 'o1 mini',
  'o3-mini': 'o3 mini',
};

const MODEL_SHORTHAND: Record<string, string> = {
  opus: 'claude-opus-4-6',
  sonnet: 'claude-sonnet-4-6',
  haiku: 'claude-haiku-4-5-20251001',
  gpt4o: 'gpt-4o',
  gpt4: 'gpt-4o',
  'gpt4o-mini': 'gpt-4o-mini',
  o1: 'o1',
  'o3-mini': 'o3-mini',
};

const PROVIDER_FOR_MODEL: Record<string, string> = {
  'claude-opus-4-6': 'Anthropic',
  'claude-sonnet-4-6': 'Anthropic',
  'claude-haiku-4-5': 'Anthropic',
  'claude-haiku-4-5-20251001': 'Anthropic',
  'gpt-4o': 'OpenAI',
  'gpt-4o-mini': 'OpenAI',
  'o1': 'OpenAI',
  'o1-mini': 'OpenAI',
  'o3-mini': 'OpenAI',
};

const SLASH_COMMANDS = [
  { cmd: '/model',      usage: '/model [alias|reset]',                 desc: 'Switch AI model or list available models' },
  { cmd: '/think',      usage: '/think [budget]',                      desc: 'Toggle extended thinking (Anthropic only)' },
  { cmd: '/provider',   usage: '/provider [claude|openai|auto]',       desc: 'Set API provider for this session' },
  { cmd: '/verbose',    usage: '/verbose',                             desc: 'Toggle verbose tool output' },
  { cmd: '/compact',    usage: '/compact',                             desc: 'Compress history to save context tokens' },
  { cmd: '/remind',     usage: '/remind daily 09:00 <task>',           desc: 'Create a scheduled recurring task' },
  { cmd: '/guardrails', usage: '/guardrails',                          desc: 'Configure security guardrails' },
  { cmd: '/yolo',       usage: '/yolo [seconds] [reason]',             desc: 'Request elevated yolo mode (requires approval)' },
  { cmd: '/new',        usage: '/new',                                 desc: 'Start a fresh conversation' },
  { cmd: '/help',       usage: '/help',                                desc: 'Show all available commands' },
];

const AUTH_ERROR_PATTERNS = [
  'No Claude authentication configured',
  'authentication_error',
  'invalid x-api-key',
  'Invalid API Key',
  'Could not resolve authentication',
  'OAuth token expired',
  'token refresh failed',
  'Incorrect API key provided',
  'invalid_api_key',
  'No OpenAI authentication configured',
  'OpenAI token expired',
];

function isAuthError(error: string): boolean {
  const lower = error.toLowerCase();
  return AUTH_ERROR_PATTERNS.some(p => lower.includes(p.toLowerCase()));
}

function makeId(): string {
  return typeof crypto !== 'undefined' && crypto.randomUUID
    ? crypto.randomUUID()
    : Math.random().toString(36).slice(2);
}

// ─── Props ────────────────────────────────────────────────────────────────────

interface ChatProps {
  sessionId?: string;
  onSessionChange?: (id: string) => void;
  onAuthRequired?: (reason?: string) => void;
  onOpenInCanvas?: (code: string, language: string) => void;
}

interface ToolApprovalRequest {
  request_id: string;
  tool_name: string;
  reason: string;
  details?: string;
  timeout_secs: number;
}

function toolDisplayName(toolName: string): string {
  const names: Record<string, string> = {
    nexibot_execute: 'Execute Command',
    nexibot_fetch: 'Fetch URL',
    nexibot_filesystem: 'Filesystem',
    nexibot_memory: 'Memory',
    nexibot_soul: 'Soul',
    nexibot_settings: 'Settings',
    nexibot_browser: 'Browser',
    computer_use: 'Computer Use',
  };
  return names[toolName] ?? toolName;
}

// ─── Component ────────────────────────────────────────────────────────────────

const MAX_MESSAGES = 1000;

function Chat({ sessionId, onSessionChange, onAuthRequired, onOpenInCanvas }: ChatProps) {
  const [messages, setMessagesRaw] = useState<Message[]>([]);
  // Capped setter — keeps only the most recent MAX_MESSAGES entries.
  const setMessages = useCallback(
    (updater: Message[] | ((prev: Message[]) => Message[])) => {
      setMessagesRaw((prev) => {
        const next = typeof updater === 'function' ? updater(prev) : updater;
        return next.length > MAX_MESSAGES ? next.slice(next.length - MAX_MESSAGES) : next;
      });
    },
    []
  );
  const [input, setInput] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [isRecording, setIsRecording] = useState(false);

  // Panels
  const [showGuardrails, setShowGuardrails] = useState(false);

  // Session overrides
  const [overrides, setOverrides] = useState<SessionOverrides>({
    model: null, thinking_budget: null, verbose: false, provider: null,
  });

  // Agents
  const [agents, setAgents] = useState<{ id: string; name: string; avatar: string | null; model: string | null; is_default: boolean; channel_bindings: { channel: string; peer_id: string | null }[] }[]>([]);
  const [activeAgent, setActiveAgent] = useState<string>('');
  const [agentSwitching, setAgentSwitching] = useState(false);

  // Background tasks
  const [activeTasks, setActiveTasks] = useState<BackgroundTaskUI[]>([]);
  const [pendingToolApprovals, setPendingToolApprovals] = useState<ToolApprovalRequest[]>([]);
  const [submittingToolApproval, setSubmittingToolApproval] = useState(false);
  const pendingToolApproval = pendingToolApprovals[0] ?? null;

  // Context usage
  const [contextUsage, setContextUsage] = useState<{
    estimated_tokens: number; context_window: number; usage_percent: number; model: string;
  } | null>(null);

  // Streaming
  const [streamingText, setStreamingText] = useState('');
  const [activeTools, setActiveTools] = useState<ToolIndicator[]>([]);
  const [thinkingIndicator, setThinkingIndicator] = useState(false);
  const [loopProgress, setLoopProgress] = useState<{ iteration: number; total: number } | null>(null);
  const streamingTextRef = useRef('');
  const activeToolsRef = useRef<ToolIndicator[]>([]);

  // Cancel streaming — resolved by Stop button to abort the Promise.race in sendMessage
  const abortRef = useRef<((partial: string) => void) | null>(null);

  // Model display names
  const [modelDisplayNames, setModelDisplayNames] = useState<Record<string, string>>(FALLBACK_DISPLAY);

  // Copy feedback — tracks which message was just copied
  const [copiedId, setCopiedId] = useState<string | null>(null);

  // Slash-command palette
  const [paletteIndex, setPaletteIndex] = useState(0);

  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Load session overrides, agents, and available model names on mount
  useEffect(() => {
    invoke<SessionOverrides>('get_session_overrides').then(setOverrides)
      .catch((e) => notifyError('Settings', `Failed to load session overrides: ${e}`));

    invoke<typeof agents>('list_agents').then((list) => {
      setAgents(list);
      if (list.length > 0) {
        invoke<string>('get_active_gui_agent').then(setActiveAgent)
          .catch((e) => notifyError('Agents', `Failed to get active agent: ${e}`));
      }
    }).catch((e) => notifyError('Agents', `Failed to list agents: ${e}`));

    invoke<AvailableModel[]>('get_available_models').then((models) => {
      const map: Record<string, string> = { ...FALLBACK_DISPLAY };
      for (const m of models) {
        map[m.id] = m.display_name;
        if (m.alias) map[m.alias] = m.display_name;
      }
      setModelDisplayNames(map);
    }).catch((e) => notifyError('Models', `Failed to load available models: ${e}`));
  }, []);

  // Reload messages when session changes
  useEffect(() => {
    if (!sessionId) return;
    invoke<{ id: string; title: string | null; started_at: string; last_activity: string; messages: { role: string; content: string; timestamp: string }[] }>('get_current_session')
      .then((session) => {
        const loaded: Message[] = session.messages
          .filter((m) => m.role !== 'system')
          .map((m) => ({
            id: makeId(),
            role: m.role as 'user' | 'assistant',
            content: extractDisplayText(m.content),
            timestamp: new Date(),
          }));
        setMessages(loaded);
      })
      .catch((e) => notifyError('Session', `Failed to load conversation history: ${e}`));
  }, [sessionId]);

  // Background task events
  useEffect(() => {
    const unsubs: UnlistenFn[] = [];

    listen<BackgroundTaskUI>('task:started', (e) => {
      setActiveTasks(prev => [...prev, { ...e.payload, status: 'running' }]);
    }).then(fn => unsubs.push(fn));

    listen<{ task_id: string; progress: string }>('task:progress', (e) => {
      setActiveTasks(prev => prev.map(t =>
        t.id === e.payload.task_id ? { ...t, progress: e.payload.progress } : t
      ));
    }).then(fn => unsubs.push(fn));

    listen<{ task_id: string; summary: string }>('task:complete', (e) => {
      setActiveTasks(prev => prev.map(t =>
        t.id === e.payload.task_id ? { ...t, status: 'completed' as const } : t
      ));
      setTimeout(() => {
        setActiveTasks(prev => prev.filter(t => t.id !== e.payload.task_id));
      }, 5000);
    }).then(fn => unsubs.push(fn));

    listen<{ task_id: string; error: string }>('task:failed', (e) => {
      setActiveTasks(prev => prev.map(t =>
        t.id === e.payload.task_id ? { ...t, status: 'failed' as const } : t
      ));
      setTimeout(() => {
        setActiveTasks(prev => prev.filter(t => t.id !== e.payload.task_id));
      }, 5000);
    }).then(fn => unsubs.push(fn));

    return () => unsubs.forEach(fn => fn());
  }, []);

  // Context usage after each message
  useEffect(() => {
    if (messages.length === 0) return;
    invoke<typeof contextUsage>('get_context_usage').then(setContextUsage)
      .catch((e) => console.warn('Failed to get context usage:', e));
  }, [messages.length]);

  // Auto-compact events
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    listen<{ status: string; message: string }>('compact:status', (event) => {
      const { status, message } = event.payload;
      if (status === 'auto_compacting' || status === 'auto_complete') {
        setMessages((prev) => {
          if (prev.length > 0 && prev[prev.length - 1].content.startsWith('[Auto-compact]')) {
            const updated = [...prev];
            updated[updated.length - 1] = { ...updated[updated.length - 1], content: `[Auto-compact] ${message}` };
            return updated;
          }
          return [...prev, { id: makeId(), role: 'assistant' as const, content: `[Auto-compact] ${message}`, timestamp: new Date() }];
        });
      }
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  // Scheduler task events
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    listen<{ task_id: string; task_name: string; response: string; timestamp: string; success: boolean }>('scheduler:task-complete', (event) => {
      const { task_name, response, success } = event.payload;
      setMessages((prev) => [...prev, {
        id: makeId(),
        role: 'assistant' as const,
        content: `[Scheduled: ${task_name}] ${success ? response : `Failed: ${response}`}`,
        timestamp: new Date(),
        isError: !success,
      }]);
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  // Tool-approval requests (guardrails / external-action confirmation)
  useEffect(() => {
    const unsubs: UnlistenFn[] = [];

    listen<ToolApprovalRequest>('chat:tool-approval-request', (event) => {
      const payload = event.payload;
      if (!payload?.request_id || !payload?.tool_name || !payload?.reason) return;
      setPendingToolApprovals((prev) => {
        if (prev.some((req) => req.request_id === payload.request_id)) {
          return prev;
        }
        return [...prev, payload];
      });
      setSubmittingToolApproval(false);
    }).then((fn) => unsubs.push(fn));

    listen<{ request_id: string }>('chat:tool-approval-expired', (event) => {
      const requestId = event.payload?.request_id;
      if (!requestId) return;
      setPendingToolApprovals((prev) =>
        prev.filter((req) => req.request_id !== requestId)
      );
      setSubmittingToolApproval(false);
    }).then((fn) => unsubs.push(fn));

    return () => unsubs.forEach((fn) => fn());
  }, []);

  const submitToolApproval = useCallback(async (approved: boolean) => {
    if (!pendingToolApproval) return;
    const requestId = pendingToolApproval.request_id;
    setSubmittingToolApproval(true);
    try {
      await invoke<boolean>('respond_tool_approval', { requestId, approved });
    } catch (error) {
      notifyError('Tool Approval', `Failed to submit decision: ${error}`);
    } finally {
      setPendingToolApprovals((prev) =>
        prev.filter((req) => req.request_id !== requestId)
      );
      setSubmittingToolApproval(false);
    }
  }, [pendingToolApproval]);

  // Auto-resize textarea to content
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = 'auto';
    ta.style.height = Math.min(ta.scrollHeight, 200) + 'px';
  }, [input]);

  // ─── Helpers ────────────────────────────────────────────────────────────────

  const addSystemMessage = (content: string, isError = false) => {
    setMessages((prev) => [...prev, { id: makeId(), role: 'assistant', content, timestamp: new Date(), isError }]);
  };

  const displayName = (modelId: string) => modelDisplayNames[modelId] ?? modelId;

  const copyMessage = useCallback((msg: Message) => {
    const text = extractDisplayText(msg.content);
    navigator.clipboard.writeText(text).then(() => {
      setCopiedId(msg.id);
      setTimeout(() => setCopiedId(prev => prev === msg.id ? null : prev), 2000);
    }).catch((e) => notifyError('Copy Failed', String(e)));
  }, []);

  const retryMessage = useCallback((msg: Message) => {
    setMessages(prev => {
      const idx = prev.findIndex(m => m.id === msg.id);
      return idx >= 0 ? prev.slice(0, idx) : prev;
    });
    setInput(extractDisplayText(msg.content));
    setTimeout(() => textareaRef.current?.focus(), 0);
  }, []);

  // ─── VoiceBar callbacks ──────────────────────────────────────────────────────

  const handleVoiceMessageAdd = useCallback((role: 'user' | 'assistant', content: string, isError?: boolean) => {
    setMessages((prev) => [...prev, { id: makeId(), role, content, timestamp: new Date(), isError }]);
  }, []);

  // ─── Stream cancellation ─────────────────────────────────────────────────────

  const handleCancelStream = useCallback(() => {
    if (abortRef.current) {
      abortRef.current(streamingTextRef.current);
      abortRef.current = null;
    }
    invoke('cancel_message').catch(() => {});
  }, []);

  // ─── Slash commands ──────────────────────────────────────────────────────────

  const handleSlashCommand = async (trimmed: string): Promise<boolean> => {

    if (trimmed === '/help') {
      setInput('');
      const lines = SLASH_COMMANDS.map(c => `\`${c.usage}\` — ${c.desc}`).join('\n');
      addSystemMessage(`**Available commands**\n\n${lines}`);
      return true;
    }

    if (trimmed === '/new') {
      setInput('');
      setMessages([]);
      setStreamingText('');
      setActiveTools([]);
      streamingTextRef.current = '';
      activeToolsRef.current = [];
      try {
        await invoke('create_named_session', { name: `Chat ${new Date().toLocaleTimeString()}` });
      } catch { /* session may reset on next message anyway */ }
      addSystemMessage('Started a new conversation.');
      return true;
    }

    if (trimmed === '/guardrails') {
      setInput('');
      setShowGuardrails(true);
      return true;
    }

    if (trimmed === '/compact') {
      setInput('');
      setIsLoading(true);
      addSystemMessage('Compacting conversation history…');
      try {
        const result = await invoke<{
          success: boolean;
          messages_before: number;
          messages_after: number;
          tokens_before: number;
          tokens_after: number;
          error?: string;
        }>('compact_conversation');
        setMessages((prev) => {
          const updated = [...prev];
          updated[updated.length - 1] = {
            ...updated[updated.length - 1],
            content: result.success
              ? `Compacted: ${result.messages_before} → ${result.messages_after} messages (~${result.tokens_before.toLocaleString()} → ~${result.tokens_after.toLocaleString()} tokens)`
              : `Compaction failed: ${result.error ?? 'Unknown error'}`,
            isError: !result.success,
          };
          return updated;
        });
      } catch (error) {
        setMessages((prev) => {
          const updated = [...prev];
          updated[updated.length - 1] = { ...updated[updated.length - 1], content: `Compaction error: ${error}`, isError: true };
          return updated;
        });
      } finally {
        setIsLoading(false);
      }
      return true;
    }

    if (trimmed.startsWith('/model')) {
      setInput('');
      const arg = trimmed.slice(6).trim();

      if (!arg) {
        try {
          const models = await invoke<AvailableModel[]>('get_available_models');
          const currentLabel = overrides.model ? (modelDisplayNames[overrides.model] ?? overrides.model) : 'Sonnet 4.6 (default)';
          const grouped: Record<string, AvailableModel[]> = {};
          for (const m of models) {
            if (!grouped[m.provider]) grouped[m.provider] = [];
            grouped[m.provider].push(m);
          }
          let out = `**Current model:** ${currentLabel}\n\n`;
          for (const [provider, list] of Object.entries(grouped)) {
            out += `**${provider}**${list[0]?.available ? '' : ' *(no API key)*'}\n`;
            for (const m of list) {
              const alias = m.alias ? `\`${m.alias}\`` : m.id;
              const cur = overrides.model === m.id ? ' ← current' : '';
              const unavail = !m.available ? ' *(configure in Settings → Models)*' : '';
              out += `  ${alias} — ${m.display_name}${cur}${unavail}\n`;
            }
            out += '\n';
          }
          out += 'Use `/model <alias>` to switch, `/model reset` to clear.';
          addSystemMessage(out);
        } catch {
          addSystemMessage(`**Current model:** ${overrides.model ? displayName(overrides.model) : 'Sonnet 4.6 (default)'}\n\nUse \`/model opus\`, \`/model sonnet\`, \`/model haiku\`, \`/model gpt4o\`, or \`/model reset\`.`);
        }
        return true;
      }

      if (arg === 'reset') {
        try {
          const result = await invoke<SessionOverrides>('reset_session_overrides');
          setOverrides(result);
          addSystemMessage('Session overrides reset to defaults.');
        } catch (error) {
          addSystemMessage(`Failed to reset: ${error}`, true);
        }
        return true;
      }

      try {
        const result = await invoke<SessionOverrides>('set_session_model', { model: arg });
        setOverrides(result);
        addSystemMessage(`Model switched to **${result.model ? displayName(result.model) : arg}**`);
        if (result.model && PROVIDER_FOR_MODEL[result.model] === 'OpenAI' && result.thinking_budget != null) {
          addSystemMessage('Note: Extended thinking is not supported by OpenAI models.');
        }
      } catch (error) {
        addSystemMessage(`${error}`, true);
      }
      return true;
    }

    if (trimmed.startsWith('/think')) {
      setInput('');
      const arg = trimmed.slice(6).trim();
      try {
        const budget = arg ? parseInt(arg) : undefined;
        if (arg && (isNaN(budget!) || budget! <= 0)) {
          addSystemMessage('Usage: `/think` (toggle) or `/think 10000` (set token budget)');
          return true;
        }
        const result = await invoke<SessionOverrides>('toggle_thinking', { budget: budget ?? null });
        setOverrides(result);
        addSystemMessage(result.thinking_budget != null
          ? `Extended thinking **enabled** (budget: ${result.thinking_budget.toLocaleString()} tokens)`
          : 'Extended thinking **disabled**');
      } catch (error) {
        addSystemMessage(`Failed: ${error}`, true);
      }
      return true;
    }

    if (trimmed === '/verbose') {
      setInput('');
      try {
        const result = await invoke<SessionOverrides>('toggle_verbose');
        setOverrides(result);
        addSystemMessage(`Verbose mode **${result.verbose ? 'enabled' : 'disabled'}**`);
      } catch (error) {
        addSystemMessage(`Failed: ${error}`, true);
      }
      return true;
    }

    if (trimmed.startsWith('/provider')) {
      setInput('');
      const arg = trimmed.slice(9).trim();
      if (!arg) {
        addSystemMessage(`**Current provider:** ${overrides.provider ?? 'auto (from model)'}\n\nUsage: \`/provider claude\`, \`/provider openai\`, \`/provider auto\``);
        return true;
      }
      try {
        const result = await invoke<SessionOverrides>('set_session_provider', { provider: arg });
        setOverrides(result);
        addSystemMessage(`Provider set to **${result.provider ?? 'auto'}**`);
        if (result.provider === 'OpenAI' && result.thinking_budget != null) {
          addSystemMessage('Note: Extended thinking is not supported by OpenAI models.');
        }
      } catch (error) {
        addSystemMessage(`${error}`, true);
      }
      return true;
    }

    if (trimmed.startsWith('/remind ')) {
      setInput('');
      const rest = trimmed.slice(8).trim();
      const schedulePatterns = [
        /^(daily\s+\d{1,2}:\d{2})\s+(.+)$/i,
        /^(hourly)\s+(.+)$/i,
        /^(every\s+\d+m)\s+(.+)$/i,
        /^(weekly\s+\w+\s+\d{1,2}:\d{2})\s+(.+)$/i,
      ];
      let schedule = '', prompt = '';
      for (const pattern of schedulePatterns) {
        const match = rest.match(pattern);
        if (match) { schedule = match[1]; prompt = match[2]; break; }
      }
      if (!schedule || !prompt) {
        addSystemMessage('Usage: `/remind daily 09:00 Check my calendar`\n\nFormats: `daily HH:MM`, `hourly`, `every Nm`, `weekly DAY HH:MM`');
        return true;
      }
      try {
        const task = await invoke<{ id: string; name: string }>('add_scheduled_task', {
          name: prompt.slice(0, 50), schedule, prompt,
        });
        addSystemMessage(`Scheduled: "${task.name}" (${schedule})`);
      } catch (error) {
        addSystemMessage(`Failed to create task: ${error}`, true);
      }
      return true;
    }

    if (trimmed.startsWith('/yolo')) {
      setInput('');
      const arg = trimmed.slice(5).trim();
      let durationSecs: number | null = null;
      let reason: string | null = null;
      if (arg) {
        const parts = arg.split(/\s+(.+)/);
        const first = Number(parts[0]);
        if (!isNaN(first) && first > 0) {
          durationSecs = first;
          reason = parts[1] ?? null;
        } else {
          reason = arg;
        }
      }
      try {
        const result = await invoke<{ ok: boolean; message: string }>('request_yolo_mode', {
          durationSecs,
          reason,
        });
        addSystemMessage(result.ok ? `⚡ ${result.message}` : `Yolo request failed: ${result.message}`, !result.ok);
      } catch (error) {
        addSystemMessage(`Yolo request error: ${error}`, true);
      }
      return true;
    }

    return false;
  };

  // ─── Message sending ─────────────────────────────────────────────────────────

  // Stable refs so sendMessage never stale-closes over handleSlashCommand or onAuthRequired.
  // Updated inline during render (safe — refs don't trigger re-renders).
  const handleSlashCommandRef = useRef(handleSlashCommand);
  handleSlashCommandRef.current = handleSlashCommand;
  const onAuthRequiredRef = useRef(onAuthRequired);
  onAuthRequiredRef.current = onAuthRequired;

  const sendMessage = useCallback(async () => {
    if (!input.trim() || isLoading) return;
    const trimmed = input.trim();

    if (trimmed.startsWith('/')) {
      const handled = await handleSlashCommandRef.current(trimmed);
      if (handled) return;
    }

    const userMessage: Message = { id: makeId(), role: 'user', content: input, timestamp: new Date() };
    setMessages((prev) => [...prev, userMessage]);
    setInput('');
    setIsLoading(true);
    setStreamingText('');
    setActiveTools([]);
    streamingTextRef.current = '';
    activeToolsRef.current = [];

    const unsubs: UnlistenFn[] = [];

    const abortPromise = new Promise<{ response: string; error?: string }>((resolve) => {
      abortRef.current = (partial: string) => resolve({ response: partial, error: 'cancelled' });
    });

    try {
      unsubs.push(await listen<{ text: string }>('chat:text-chunk', (event) => {
        streamingTextRef.current += event.payload.text;
        setStreamingText(streamingTextRef.current);
      }));

      unsubs.push(await listen<{ name: string; id: string }>('chat:tool-start', (event) => {
        const tool: ToolIndicator = { name: event.payload.name, id: event.payload.id, status: 'running' };
        activeToolsRef.current = [...activeToolsRef.current, tool];
        setActiveTools([...activeToolsRef.current]);
      }));

      unsubs.push(await listen<{ name: string; id: string; success: boolean }>('chat:tool-result', (event) => {
        activeToolsRef.current = activeToolsRef.current.map((t) =>
          t.id === event.payload.id ? { ...t, status: event.payload.success ? 'done' : 'error', retryCountdown: undefined } : t
        );
        setActiveTools([...activeToolsRef.current]);
      }));

      unsubs.push(await listen<{
        name: string; id: string; kind: string; message: string;
        retry_after_secs: number; attempt: number; max_attempts: number;
      }>('chat:tool-error', (event) => {
        const { id, kind, message, retry_after_secs, attempt, max_attempts } = event.payload;
        const isRetrying = attempt < max_attempts;
        activeToolsRef.current = activeToolsRef.current.map((t) =>
          t.id === id
            ? {
                ...t,
                status: isRetrying ? 'retrying' : 'error',
                errorKind: kind as import('./chat-types').ToolErrorKind,
                errorMessage: message,
                retryCountdown: isRetrying ? retry_after_secs : undefined,
                attempt,
                maxAttempts: max_attempts,
              }
            : t
        );
        setActiveTools([...activeToolsRef.current]);
      }));

      unsubs.push(await listen<void>('chat:thinking', () => {
        setThinkingIndicator(true);
      }));

      unsubs.push(await listen<{ iteration: number; total: number; elapsed_secs: number }>('chat:progress', (event) => {
        setLoopProgress({ iteration: event.payload.iteration, total: event.payload.total });
      }));

      unsubs.push(await listen<{ from_model: string; to_model: string; reason: string }>('chat:model-fallback', (event) => {
        const notice = `ℹ️ Model switched: ${event.payload.from_model} → ${event.payload.to_model} (${event.payload.reason})`;
        setMessages((prev) => [...prev, { id: makeId(), role: 'assistant' as const, content: notice, timestamp: new Date() }]);
      }));

      const completePromise = new Promise<{ response: string; error?: string; model_used?: string }>((resolve) => {
        listen<{ response: string; error?: string; model_used?: string }>('chat:complete', (event) => {
          setThinkingIndicator(false);
          setLoopProgress(null);
          resolve(event.payload);
        }).then((unsub) => unsubs.push(unsub));
      });

      // Safety valve: if chat:complete never fires (backend crash/hang), unblock after 5 min.
      let timeoutId: ReturnType<typeof setTimeout> | undefined;
      const timeoutPromise = new Promise<{ response: string; error?: string }>((resolve) => {
        timeoutId = setTimeout(() => {
          resolve({ response: streamingTextRef.current, error: 'Response timed out.' });
        }, 5 * 60 * 1000);
      });

      // invokeFailPromise resolves (not rejects) with an error payload so
      // Promise.race() can pick it up and reset the UI immediately instead of
      // waiting for the 5-minute safety-valve timeout.
      const invokeFailPromise = invoke('send_message_with_events', {
        request: { message: trimmed, use_streaming: true },
      }).then(() => new Promise<{ response: string; error?: string }>(() => {/* resolved by chat:complete */}))
        .catch((err): { response: string; error: string } => {
          notifyError('Send Failed', String(err));
          return { response: '', error: String(err) };
        });

      const result = await Promise.race([completePromise, abortPromise, timeoutPromise, invokeFailPromise]);
      clearTimeout(timeoutId);
      for (const unsub of unsubs) unsub();
      abortRef.current = null;

      if (result.error === 'cancelled') {
        const partial = result.response.trim();
        if (partial) {
          setMessages((prev) => [...prev, {
            id: makeId(),
            role: 'assistant',
            content: partial + '\n\n*(response stopped)*',
            timestamp: new Date(),
          }]);
        }
        setStreamingText(''); setActiveTools([]); setThinkingIndicator(false); setLoopProgress(null);
        streamingTextRef.current = ''; activeToolsRef.current = [];
        setIsLoading(false);
        return;
      }

      if (result.error && isAuthError(result.error)) {
        onAuthRequiredRef.current?.(result.error);
        setStreamingText(''); setActiveTools([]); setThinkingIndicator(false); setLoopProgress(null);
        streamingTextRef.current = ''; activeToolsRef.current = [];
        setIsLoading(false);
        return;
      }

      const finalText = result.error ? `Error: ${result.error}` : result.response || streamingTextRef.current;
      setMessages((prev) => [...prev, {
        id: makeId(),
        role: 'assistant',
        content: finalText,
        timestamp: new Date(),
        isError: !!result.error,
        toolIndicators: activeToolsRef.current.length > 0 ? [...activeToolsRef.current] : undefined,
        model: (result as any).model_used || undefined,
      }]);
      setStreamingText(''); setActiveTools([]); setThinkingIndicator(false); setLoopProgress(null);
      streamingTextRef.current = ''; activeToolsRef.current = [];
    } catch (error) {
      for (const unsub of unsubs) unsub();
      abortRef.current = null;
      const errorStr = String(error);
      if (isAuthError(errorStr)) {
        onAuthRequiredRef.current?.(errorStr);
      } else {
        setMessages((prev) => [...prev, { id: makeId(), role: 'assistant', content: `Error: ${error}`, timestamp: new Date(), isError: true }]);
      }
      setStreamingText(''); setActiveTools([]); setThinkingIndicator(false); setLoopProgress(null);
      streamingTextRef.current = ''; activeToolsRef.current = [];
    } finally {
      setIsLoading(false);
      setSubmittingToolApproval(false);
    }
  }, [input, isLoading]);

  // ─── Keyboard ────────────────────────────────────────────────────────────────

  const filteredCommands = input.startsWith('/') && !input.includes(' ')
    ? SLASH_COMMANDS.filter(c => c.cmd.startsWith(input.toLowerCase()))
    : [];
  const showPalette = filteredCommands.length > 0;

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (showPalette) {
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setPaletteIndex(i => Math.max(0, i - 1));
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setPaletteIndex(i => Math.min(filteredCommands.length - 1, i + 1));
        return;
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && filteredCommands.length > 0 && input !== filteredCommands[paletteIndex]?.cmd)) {
        e.preventDefault();
        const chosen = filteredCommands[Math.min(paletteIndex, filteredCommands.length - 1)];
        if (chosen) { setInput(chosen.cmd + ' '); setPaletteIndex(0); }
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        setInput('');
        return;
      }
    }

    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  };

  // Reset palette index when input changes
  useEffect(() => { setPaletteIndex(0); }, [input]);

  // ─── Derived state ───────────────────────────────────────────────────────────

  const hasOverrides = overrides.model != null || overrides.thinking_budget != null || overrides.verbose || overrides.provider != null;
  const runningTasks = activeTasks.filter(t => t.status === 'running');
  const lastUserMsgId = messages.reduceRight((found, m) => found ?? (m.role === 'user' ? m.id : null), null as string | null);

  // ─── Render ──────────────────────────────────────────────────────────────────

  return (
    <div className="chat-container">

      {/* Agent selector */}
      {agents.length > 1 && (
        <div className="agent-selector-bar">
          <span className="agent-selector-label">Agent:</span>
          <select
            className="agent-selector-select"
            value={activeAgent}
            disabled={agentSwitching}
            onChange={async (e) => {
              const agentId = e.target.value;
              setAgentSwitching(true);
              try {
                await invoke('set_active_gui_agent', { agentId });
                setActiveAgent(agentId);
              } catch (err) {
                notifyError('Agent Switch', `Failed to switch agent: ${err}`);
              } finally {
                setAgentSwitching(false);
              }
            }}
          >
            {agents.map((a) => (
              <option key={a.id} value={a.id}>{a.name}{a.is_default ? ' (default)' : ''}</option>
            ))}
          </select>
        </div>
      )}

      {/* Session overrides + context bar */}
      {(hasOverrides || contextUsage) && (
        <div className="session-overrides-bar">
          {overrides.model && (
            <span className="override-badge model-badge">{displayName(overrides.model)}</span>
          )}
          {overrides.thinking_budget != null && (
            <span className="override-badge thinking-badge">Thinking: {overrides.thinking_budget.toLocaleString()}</span>
          )}
          {overrides.verbose && (
            <span className="override-badge verbose-badge">Verbose</span>
          )}
          {overrides.provider && (
            <span className="override-badge provider-badge">{overrides.provider}</span>
          )}
          {contextUsage && (
            <div
              className={`context-bar${contextUsage.usage_percent > 80 ? ' context-bar--warn' : contextUsage.usage_percent > 60 ? ' context-bar--caution' : ''}`}
              title={`Context: ${contextUsage.estimated_tokens.toLocaleString()} / ${contextUsage.context_window.toLocaleString()} tokens (${contextUsage.usage_percent}%)`}
            >
              <div className="context-bar__fill" style={{ width: `${Math.min(contextUsage.usage_percent, 100)}%` }} />
            </div>
          )}
          {hasOverrides && (
            <button className="override-reset-btn" onClick={async () => {
              try { const r = await invoke<SessionOverrides>('reset_session_overrides'); setOverrides(r); }
              catch (e) { notifyError('Session', `Failed to reset overrides: ${e}`); }
            }} title="Reset overrides">Reset</button>
          )}
        </div>
      )}

      <MessageList
        messages={messages}
        streamingText={streamingText}
        activeTools={activeTools}
        isLoading={isLoading}
        lastUserMsgId={lastUserMsgId}
        copiedId={copiedId}
        onCopyMessage={copyMessage}
        onRetryMessage={retryMessage}
        onOpenInCanvas={onOpenInCanvas}
      />

      {/* Background task pills */}
      {runningTasks.length > 0 && (
        <div className="active-tasks-bar" role="status" aria-live="polite">
          {runningTasks.map(task => (
            <span key={task.id} className="task-pill">
              ⏳ {task.progress || task.description}
            </span>
          ))}
        </div>
      )}

      {/* Thinking / loop-progress indicators (shown during streaming) */}
      {(thinkingIndicator || loopProgress) && (
        <div className="active-tasks-bar" role="status" aria-live="polite">
          {thinkingIndicator && !loopProgress && (
            <span className="task-pill">🧠 Thinking…</span>
          )}
          {loopProgress && (
            <span className="task-pill">
              🔁 Step {loopProgress.iteration} of {loopProgress.total}
            </span>
          )}
        </div>
      )}

      <VoiceBar
        isLoading={isLoading}
        onMessageAdd={handleVoiceMessageAdd}
        onRecordingChange={setIsRecording}
      />

      {/* Guardrails panel */}
      {showGuardrails && (
        <GuardrailsPanel
          onClose={() => setShowGuardrails(false)}
          onApplied={() => {
            setShowGuardrails(false);
            setMessages((prev) => [...prev, { id: makeId(), role: 'assistant', content: 'Guardrails configuration updated.', timestamp: new Date() }]);
          }}
        />
      )}

      {pendingToolApproval && (
        <div className="tool-approval-bar" role="status" aria-live="assertive">
          <div className="tool-approval-content">
            <strong>Approve: {toolDisplayName(pendingToolApproval.tool_name)}</strong>
            <span>{pendingToolApproval.reason}</span>
            {pendingToolApproval.details && (
              <pre className="tool-approval-details">{pendingToolApproval.details}</pre>
            )}
          </div>
          <div className="tool-approval-actions">
            <button
              className="tool-approval-deny"
              onClick={() => void submitToolApproval(false)}
              disabled={submittingToolApproval}
            >
              Deny
            </button>
            <button
              className="tool-approval-approve"
              onClick={() => void submitToolApproval(true)}
              disabled={submittingToolApproval}
            >
              {submittingToolApproval ? 'Submitting…' : 'Approve'}
            </button>
          </div>
        </div>
      )}

      {/* Input area */}
      <div className="input-area">
        <SlashCommandPalette
          commands={filteredCommands}
          selectedIndex={paletteIndex}
          onSelect={(cmd) => {
            setInput(cmd + ' ');
            setPaletteIndex(0);
            textareaRef.current?.focus();
          }}
        />

        <textarea
          ref={textareaRef}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Message NexiBot… (type / for commands)"
          disabled={isRecording}
          rows={1}
        />

        {/* Send / Stop */}
        {isLoading ? (
          <button className="stop-stream-btn" onClick={handleCancelStream} aria-label="Stop">
            ■ Stop
          </button>
        ) : (
          <button onClick={sendMessage} disabled={!input.trim()} aria-label="Send message">
            Send
          </button>
        )}
      </div>
    </div>
  );
}

export default Chat;
