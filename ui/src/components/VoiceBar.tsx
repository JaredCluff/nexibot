import { useState, useRef, useCallback, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import type { VoiceStatus, PushToTalkResponse } from './chat-types';
import { notifyError } from '../shared/notify';

const ACTIVE_VOICE_STATES = new Set(['Listening', 'Processing', 'Thinking', 'Speaking', 'WakeDetected']);

function voiceStateLabel(state: string, isSleeping: boolean): string {
  if (isSleeping) return 'Sleeping';
  const labels: Record<string, string> = {
    Idle: 'Idle',
    WakeDetected: 'Wake detected…',
    Listening: 'Listening…',
    Processing: 'Transcribing…',
    Thinking: 'Thinking…',
    Speaking: 'Speaking…',
    Sleeping: 'Sleeping',
  };
  return labels[state] ?? state;
}

interface VoiceBarProps {
  isLoading: boolean;
  onMessageAdd: (role: 'user' | 'assistant', content: string, isError?: boolean) => void;
  onRecordingChange: (recording: boolean) => void;
}

export default function VoiceBar({ isLoading, onMessageAdd, onRecordingChange }: VoiceBarProps) {
  const [voiceStatus, setVoiceStatus] = useState<VoiceStatus | null>(null);
  const [voiceToggling, setVoiceToggling] = useState(false);
  const [isRecording, setIsRecording] = useState(false);
  const [recordingTime, setRecordingTime] = useState(0);
  const recordingTimerRef = useRef<number | null>(null);

  // Stable ref so the voice:transcript listener always calls the latest onMessageAdd
  // without needing to re-register the listener on every parent render.
  const onMessageAddRef = useRef(onMessageAdd);
  onMessageAddRef.current = onMessageAdd;

  const setRecording = useCallback((val: boolean) => {
    setIsRecording(val);
    onRecordingChange(val);
  }, [onRecordingChange]);

  // Ensure the PTT timer is always cleared on unmount to prevent leaks.
  useEffect(() => {
    return () => {
      if (recordingTimerRef.current !== null) {
        clearInterval(recordingTimerRef.current);
        recordingTimerRef.current = null;
      }
    };
  }, []);

  // Poll voice status every 500ms
  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const status = await invoke<VoiceStatus>('get_voice_status');
        setVoiceStatus(status);
      } catch {
        setVoiceStatus(null);
      }
    }, 500);
    return () => clearInterval(interval);
  }, []);

  // Voice transcript events → chat messages.
  // Uses a ref for onMessageAdd so the listener is registered once and always
  // calls the latest callback without re-registering on every parent render.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    listen<{ role: string; content: string }>('voice:transcript', (event) => {
      const { role, content } = event.payload;
      onMessageAddRef.current(
        role as 'user' | 'assistant',
        role === 'user' ? `🎙️ ${content}` : content,
      );
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  const handleToggleVoiceService = useCallback(async () => {
    setVoiceToggling(true);
    try {
      if (voiceStatus) {
        await invoke('stop_voice_service');
        setVoiceStatus(null);
      } else {
        await invoke('start_voice_service');
      }
    } catch (e) {
      notifyError('Voice', `Failed to ${voiceStatus ? 'stop' : 'start'} voice service: ${e}`);
    } finally {
      setVoiceToggling(false);
    }
  }, [voiceStatus]);

  const handleToggleTts = useCallback(async () => {
    if (!voiceStatus) return;
    const newVal = !voiceStatus.voice_response_enabled;
    setVoiceStatus(prev => prev ? { ...prev, voice_response_enabled: newVal } : null);
    try {
      await invoke('set_voice_response_enabled', { enabled: newVal });
    } catch {
      setVoiceStatus(prev => prev ? { ...prev, voice_response_enabled: !newVal } : null);
    }
  }, [voiceStatus]);

  const handleToggleWakeword = useCallback(async () => {
    if (!voiceStatus) return;
    const newVal = !voiceStatus.wakeword_enabled;
    setVoiceStatus(prev => prev ? { ...prev, wakeword_enabled: newVal } : null);
    setVoiceToggling(true);
    try {
      await invoke('voice_set_wakeword_enabled', { enabled: newVal });
      await invoke('stop_voice_service');
      await invoke('start_voice_service');
    } catch {
      setVoiceStatus(prev => prev ? { ...prev, wakeword_enabled: !newVal } : null);
    } finally {
      setVoiceToggling(false);
    }
  }, [voiceStatus]);

  const handleForceStop = useCallback(async () => {
    try { await invoke('voice_stop_listening'); } catch { /* best effort */ }
  }, []);

  const handlePttDown = useCallback(async () => {
    if (isRecording || isLoading) return;
    setRecording(true);
    setRecordingTime(0);
    const startTime = Date.now();
    recordingTimerRef.current = window.setInterval(() => {
      setRecordingTime(Math.floor((Date.now() - startTime) / 1000));
    }, 100);
    try {
      await invoke('ptt_start');
    } catch (error) {
      notifyError('Voice', `Push-to-talk failed to start: ${error}`);
      setRecording(false);
      if (recordingTimerRef.current !== null) {
        clearInterval(recordingTimerRef.current);
        recordingTimerRef.current = null;
      }
    }
  }, [isRecording, isLoading, setRecording]);

  const handlePttUp = useCallback(async () => {
    if (!isRecording) return;
    setRecording(false);
    if (recordingTimerRef.current) { clearInterval(recordingTimerRef.current); recordingTimerRef.current = null; }
    try {
      const result = await invoke<PushToTalkResponse>('ptt_stop');
      if (result.error) {
        onMessageAdd('assistant', `Voice: ${result.error}`, true);
      } else {
        if (result.transcript) onMessageAdd('user', result.transcript);
        if (result.response)  onMessageAdd('assistant', result.response);
      }
    } catch (error) {
      onMessageAdd('assistant', `Voice error: ${error}`, true);
    } finally {
      setRecordingTime(0);
    }
  }, [isRecording, setRecording, onMessageAdd]);

  const handlePttCancel = useCallback(async () => {
    if (!isRecording) return;
    setRecording(false);
    if (recordingTimerRef.current) { clearInterval(recordingTimerRef.current); recordingTimerRef.current = null; }
    setRecordingTime(0);
    try { await invoke('ptt_cancel'); } catch { /* best effort */ }
  }, [isRecording, setRecording]);

  const isVoiceActive = voiceStatus ? ACTIVE_VOICE_STATES.has(voiceStatus.state) : false;

  const voiceDotClass = voiceStatus
    ? voiceStatus.is_sleeping ? 'voice-dot--sleeping'
    : isVoiceActive           ? 'voice-dot--active'
    :                           'voice-dot--idle'
    : 'voice-dot--off';

  const voiceLabel = voiceStatus
    ? voiceStateLabel(voiceStatus.state, voiceStatus.is_sleeping)
    : 'Voice off';

  return (
    <div className={`voice-bar${voiceStatus ? ' voice-bar--running' : ''}`}>
      <span className={`voice-dot ${voiceDotClass}`} aria-hidden="true" />
      <span className="voice-bar__label" aria-live="polite" aria-atomic="true">{voiceLabel}</span>

      {voiceStatus && (
        <div className="voice-bar__controls">
          <button
            className={`voice-icon-btn${voiceStatus.wakeword_enabled ? ' voice-icon-btn--on' : ''}`}
            title={voiceStatus.wakeword_enabled ? 'Wake word on — click to disable' : 'Wake word off — click to enable'}
            aria-label={voiceStatus.wakeword_enabled ? 'Disable wake word' : 'Enable wake word'}
            aria-pressed={voiceStatus.wakeword_enabled}
            onClick={handleToggleWakeword}
            disabled={voiceToggling}
          >
            {voiceStatus.wakeword_enabled ? '👂' : '🔕'}
          </button>

          <button
            className={`voice-icon-btn${voiceStatus.voice_response_enabled ? ' voice-icon-btn--on' : ''}`}
            title={voiceStatus.voice_response_enabled ? 'Voice response on — click for text-only' : 'Voice response off — click to enable TTS'}
            aria-label={voiceStatus.voice_response_enabled ? 'Disable voice response' : 'Enable voice response'}
            aria-pressed={voiceStatus.voice_response_enabled}
            onClick={handleToggleTts}
            disabled={voiceToggling}
          >
            {voiceStatus.voice_response_enabled ? '🔊' : '🔇'}
          </button>

          {isVoiceActive && (
            <button className="voice-stop-btn" onClick={handleForceStop} title="Stop and return to idle" aria-label="Stop listening and return to idle">
              ✕ Stop
            </button>
          )}
        </div>
      )}

      {/* PTT button lives in the voice bar so it travels with voice context */}
      <button
        className={`ptt-button${isRecording ? ' recording' : ''}`}
        onMouseDown={handlePttDown}
        onMouseUp={handlePttUp}
        onMouseLeave={handlePttCancel}
        onTouchStart={handlePttDown}
        onTouchEnd={handlePttUp}
        disabled={isLoading && !isRecording}
        title="Hold to talk"
        aria-label={isRecording ? 'Recording — release to send' : 'Hold to talk'}
        aria-pressed={isRecording}
      >
        {isRecording ? (
          <>
            <div className="ptt-wave"><span /><span /><span /></div>
            <span className="ptt-timer">{recordingTime}s</span>
          </>
        ) : '🎙️'}
      </button>

      <button
        className={`voice-power-btn${voiceStatus ? ' voice-power-btn--stop' : ' voice-power-btn--start'}`}
        onClick={handleToggleVoiceService}
        disabled={voiceToggling}
        title={voiceStatus ? 'Stop voice service' : 'Start voice service'}
        aria-label={voiceStatus ? 'Stop voice service' : 'Start voice service'}
      >
        {voiceToggling ? '…' : voiceStatus ? '■ Stop' : '▶ Start Voice'}
      </button>
    </div>
  );
}
