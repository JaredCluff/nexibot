import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { VoiceTab } from './VoiceTab';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { useSettings } from '../SettingsContext';

vi.mock('../SettingsContext');
vi.mock('../shared/InfoTip', () => ({ InfoTip: () => null }));
vi.mock('../shared/InfoGuide', () => ({
  InfoGuide: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

// ─── Fixtures ─────────────────────────────────────────────────────────────────

const makeConfig = (overrides: Record<string, unknown> = {}) => ({
  wakeword: {
    enabled: true,
    wake_word: 'hey nexus',
    threshold: 0.85,
    model_path: '',
    sleep_timeout_seconds: 30,
    conversation_timeout_seconds: 60,
    stt_wakeword_enabled: false,
    stt_require_both: false,
    voice_response_enabled: true,
    ...((overrides.wakeword as object) ?? {}),
  },
  stt: {
    enabled: true,
    backend: 'macos_speech',
    deepgram_api_key: '',
    openai_api_key: '',
    sensevoice_model_path: '',
    ...((overrides.stt as object) ?? {}),
  },
  tts: {
    enabled: true,
    backend: 'macos_say',
    macos_voice: 'Samantha',
    elevenlabs_api_key: '',
    cartesia_api_key: '',
    piper_model_path: '',
    piper_voice: '',
    espeak_voice: '',
    windows_voice: '',
    ...((overrides.tts as object) ?? {}),
  },
  vad: {
    enabled: true,
    threshold: 0.5,
    min_speech_duration_ms: 250,
    min_silence_duration_ms: 500,
    ...((overrides.vad as object) ?? {}),
  },
});

const idleStatus = {
  state: 'Idle',
  stt_backend: 'macos_speech',
  tts_backend: 'macos_say',
  is_sleeping: false,
  voice_response_enabled: true,
};

const listeningStatus = { ...idleStatus, state: 'Listening' };
const processingStatus = { ...idleStatus, state: 'Processing' };

function setupSettings(configOverrides: Record<string, unknown> = {}, status = idleStatus) {
  const config = makeConfig(configOverrides);
  const setConfig = vi.fn();
  const loadVoiceStatus = vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    platformInfo: {
      available_stt_backends: ['macos_speech', 'sensevoice', 'deepgram', 'openai'],
      available_tts_backends: ['macos_say', 'piper', 'elevenlabs', 'cartesia', 'espeak'],
    },
    voiceServiceStatus: status,
    loadVoiceStatus,
  } as ReturnType<typeof useSettings>);

  return { config, setConfig, loadVoiceStatus };
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('VoiceTab', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.alert = vi.fn();
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  // ── Pipeline status display ────────────────────────────────────────────────

  describe('voice pipeline status', () => {
    it('shows the current voice state label', () => {
      setupSettings({}, idleStatus);
      render(<VoiceTab />);
      expect(screen.getByText('Idle')).toBeInTheDocument();
    });

    it('shows STT and TTS backend in the status hint', () => {
      setupSettings({}, idleStatus);
      render(<VoiceTab />);
      expect(screen.getByText(/STT: macos_speech/)).toBeInTheDocument();
      expect(screen.getByText(/TTS: macos_say/)).toBeInTheDocument();
    });

    it('status dot has inactive class when Idle', () => {
      setupSettings({}, idleStatus);
      const { container } = render(<VoiceTab />);
      expect(container.querySelector('.status-dot.inactive')).toBeInTheDocument();
      expect(container.querySelector('.status-dot.healthy')).not.toBeInTheDocument();
    });

    it('status dot has healthy class when Listening', () => {
      setupSettings({}, listeningStatus);
      const { container } = render(<VoiceTab />);
      expect(container.querySelector('.status-dot.healthy')).toBeInTheDocument();
    });

    it('status dot has healthy class when Processing', () => {
      setupSettings({}, processingStatus);
      const { container } = render(<VoiceTab />);
      expect(container.querySelector('.status-dot.healthy')).toBeInTheDocument();
    });
  });

  // ── Enable/disable voice pipeline ────────────────────────────────────────

  describe('voice pipeline toggle', () => {
    it('calls start_voice_service when checkbox is checked from Idle', async () => {
      const user = userEvent.setup();
      setupSettings({}, idleStatus);
      render(<VoiceTab />);

      const checkbox = screen.getByRole('checkbox', { name: /Enable Voice Pipeline/i });
      expect(checkbox).not.toBeChecked();
      await user.click(checkbox);

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('start_voice_service')
      );
    });

    it('calls stop_voice_service when checkbox is unchecked from Listening', async () => {
      const user = userEvent.setup();
      setupSettings({}, listeningStatus);
      render(<VoiceTab />);

      const checkbox = screen.getByRole('checkbox', { name: /Enable Voice Pipeline/i });
      expect(checkbox).toBeChecked();
      await user.click(checkbox);

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('stop_voice_service')
      );
    });

    it('shows alert when start_voice_service fails', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockRejectedValueOnce(new Error('Microphone permission denied'));
      setupSettings({}, idleStatus);
      render(<VoiceTab />);

      const checkbox = screen.getByRole('checkbox', { name: /Enable Voice Pipeline/i });
      await user.click(checkbox);

      await waitFor(() =>
        expect(vi.mocked(emit)).toHaveBeenCalledWith(
          'notify:toast',
          expect.objectContaining({ message: expect.stringContaining('Microphone permission denied') }),
        )
      );
    });

    it('checkbox is disabled while toggling', async () => {
      const user = userEvent.setup();
      // Make invoke hang to observe disabled state
      vi.mocked(invoke).mockImplementationOnce(() => new Promise(() => {}));
      setupSettings({}, idleStatus);
      render(<VoiceTab />);

      const checkbox = screen.getByRole('checkbox', { name: /Enable Voice Pipeline/i });
      user.click(checkbox); // don't await — let it hang

      await waitFor(() => expect(checkbox).toBeDisabled());
    });
  });

  // ── STT settings ──────────────────────────────────────────────────────────

  describe('STT settings', () => {
    it('calls setConfig with new STT backend on selector change', async () => {
      const user = userEvent.setup();
      const { setConfig, config } = setupSettings();
      render(<VoiceTab />);

      const select = screen.getByDisplayValue('macOS Speech (native)');
      await user.selectOptions(select, 'sensevoice');

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          stt: expect.objectContaining({ backend: 'sensevoice' }),
        })
      );
    });

    it('shows Deepgram API key field when backend is deepgram', () => {
      setupSettings({ stt: { enabled: true, backend: 'deepgram', deepgram_api_key: '' } });
      render(<VoiceTab />);
      expect(screen.getByPlaceholderText(/Deepgram API key/i)).toBeInTheDocument();
    });

    it('hides Deepgram API key field when backend is not deepgram', () => {
      setupSettings({ stt: { enabled: true, backend: 'macos_speech' } });
      render(<VoiceTab />);
      expect(screen.queryByPlaceholderText(/Deepgram API key/i)).not.toBeInTheDocument();
    });

    it('shows OpenAI API key field when backend is openai', () => {
      setupSettings({ stt: { enabled: true, backend: 'openai', openai_api_key: '' } });
      render(<VoiceTab />);
      expect(screen.getByPlaceholderText(/OpenAI API key/i)).toBeInTheDocument();
    });

    it('shows SenseVoice model path field when backend is sensevoice', () => {
      setupSettings({
        stt: { enabled: true, backend: 'sensevoice', sensevoice_model_path: '' },
      });
      render(<VoiceTab />);
      expect(screen.getByPlaceholderText(/SenseVoice model path/i)).toBeInTheDocument();
    });

    it('Test STT button calls test_stt', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockResolvedValueOnce('hello world');
      setupSettings();
      render(<VoiceTab />);

      await user.click(screen.getByRole('button', { name: /Test STT/i }));
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('test_stt');
    });

    it('shows STT result after successful test', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockResolvedValueOnce('hello world');
      setupSettings();
      render(<VoiceTab />);

      await user.click(screen.getByRole('button', { name: /Test STT/i }));
      await waitFor(() =>
        expect(screen.getByText(/"hello world"/)).toBeInTheDocument()
      );
    });

    it('shows (no speech detected) when STT returns empty string', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockResolvedValueOnce('');
      setupSettings();
      render(<VoiceTab />);

      await user.click(screen.getByRole('button', { name: /Test STT/i }));
      await waitFor(() =>
        expect(screen.getByText(/".*no speech detected.*"/i)).toBeInTheDocument()
      );
    });

    it('shows alert when test_stt fails', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockRejectedValueOnce(new Error('No mic'));
      setupSettings();
      render(<VoiceTab />);

      await user.click(screen.getByRole('button', { name: /Test STT/i }));
      await waitFor(() =>
        expect(vi.mocked(emit)).toHaveBeenCalledWith(
          'notify:toast',
          expect.objectContaining({ message: expect.stringContaining('No mic') }),
        )
      );
    });

    it('Test STT button shows Listening label while running', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockImplementationOnce(() => new Promise(() => {}));
      setupSettings();
      render(<VoiceTab />);

      user.click(screen.getByRole('button', { name: /Test STT/i }));
      await waitFor(() =>
        expect(screen.getByRole('button', { name: /Listening/i })).toBeDisabled()
      );
    });
  });

  // ── TTS settings ──────────────────────────────────────────────────────────

  describe('TTS settings', () => {
    it('calls setConfig with new TTS backend on selector change', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings();
      render(<VoiceTab />);

      const ttsSelects = screen.getAllByRole('combobox');
      // TTS select is the second combobox (after STT select)
      await user.selectOptions(ttsSelects[1], 'piper');

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          tts: expect.objectContaining({ backend: 'piper' }),
        })
      );
    });

    it('shows macOS voice selector when backend is macos_say', () => {
      setupSettings({ tts: { enabled: true, backend: 'macos_say', macos_voice: 'Samantha' } });
      render(<VoiceTab />);
      // The macOS voice selector should have the "Samantha" option selected
      expect(screen.getByDisplayValue('Samantha')).toBeInTheDocument();
    });

    it('shows ElevenLabs API key field when backend is elevenlabs', () => {
      setupSettings({ tts: { enabled: true, backend: 'elevenlabs', elevenlabs_api_key: '' } });
      render(<VoiceTab />);
      expect(screen.getByPlaceholderText(/ElevenLabs API key/i)).toBeInTheDocument();
    });

    it('shows Cartesia API key field when backend is cartesia', () => {
      setupSettings({ tts: { enabled: true, backend: 'cartesia', cartesia_api_key: '' } });
      render(<VoiceTab />);
      expect(screen.getByPlaceholderText(/Cartesia API key/i)).toBeInTheDocument();
    });

    it('shows Piper model path field when backend is piper', () => {
      setupSettings({ tts: { enabled: true, backend: 'piper', piper_model_path: '' } });
      render(<VoiceTab />);
      expect(screen.getByPlaceholderText(/Piper ONNX model path/i)).toBeInTheDocument();
    });

    it('shows espeak voice field when backend is espeak', () => {
      setupSettings({ tts: { enabled: true, backend: 'espeak', espeak_voice: '' } });
      render(<VoiceTab />);
      expect(screen.getByPlaceholderText(/espeak-ng voice\/language/i)).toBeInTheDocument();
    });

    it('Test TTS button calls test_tts with correct args', async () => {
      const user = userEvent.setup();
      setupSettings();
      render(<VoiceTab />);

      await user.click(screen.getByRole('button', { name: /Test TTS/i }));
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        'test_tts',
        expect.objectContaining({ text: expect.any(String) })
      );
    });

    it('Test TTS button shows Speaking label while running', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockImplementationOnce(() => new Promise(() => {}));
      setupSettings();
      render(<VoiceTab />);

      user.click(screen.getByRole('button', { name: /Test TTS/i }));
      await waitFor(() =>
        expect(screen.getByRole('button', { name: /Speaking/i })).toBeDisabled()
      );
    });

    it('shows alert when test_tts fails', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockRejectedValueOnce(new Error('No TTS engine'));
      setupSettings();
      render(<VoiceTab />);

      await user.click(screen.getByRole('button', { name: /Test TTS/i }));
      await waitFor(() =>
        expect(vi.mocked(emit)).toHaveBeenCalledWith(
          'notify:toast',
          expect.objectContaining({ message: expect.stringContaining('No TTS engine') }),
        )
      );
    });
  });

  // ── Wake word section ─────────────────────────────────────────────────────

  describe('wake word settings', () => {
    it('shows wake word sub-settings when enabled', () => {
      setupSettings({ wakeword: { enabled: true, wake_word: 'hey nexus', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: false, stt_require_both: false, voice_response_enabled: true } });
      render(<VoiceTab />);
      expect(screen.getByDisplayValue('hey nexus')).toBeInTheDocument();
    });

    it('hides wake word sub-settings when disabled', () => {
      setupSettings({ wakeword: { enabled: false, wake_word: 'hey nexus', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: false, stt_require_both: false, voice_response_enabled: true } });
      render(<VoiceTab />);
      expect(screen.queryByDisplayValue('hey nexus')).not.toBeInTheDocument();
    });

    it('shows custom wake word warning when wake word is not a nexus variant', () => {
      setupSettings({ wakeword: { enabled: true, wake_word: 'computer', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: false, stt_require_both: false, voice_response_enabled: true } });
      render(<VoiceTab />);
      expect(screen.getByText(/Custom wake words require/i)).toBeInTheDocument();
    });

    it('does NOT show custom wake word warning for default wake word', () => {
      setupSettings();
      render(<VoiceTab />);
      expect(screen.queryByText(/Custom wake words require/i)).not.toBeInTheDocument();
    });

    it('calls setConfig when wake word text changes', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings();
      render(<VoiceTab />);

      const input = screen.getByDisplayValue('hey nexus');
      await user.clear(input);
      await user.type(input, 'hey computer');

      expect(setConfig).toHaveBeenCalled();
    });

    it('calls setConfig when threshold changes', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings();
      render(<VoiceTab />);

      const thresholdInput = screen.getByDisplayValue('0.85');
      await user.clear(thresholdInput);
      await user.type(thresholdInput, '0.9');

      expect(setConfig).toHaveBeenCalled();
    });
  });

  // ── Dual-detection (AND logic) ────────────────────────────────────────────

  describe('STT wake word dual-detection', () => {
    it('shows "STT Wake Word" checkbox when wake word is enabled', () => {
      setupSettings();
      render(<VoiceTab />);
      expect(screen.getByRole('checkbox', { name: /STT Wake Word/i })).toBeInTheDocument();
    });

    it('AND logic checkbox is NOT shown when stt_wakeword_enabled is false', () => {
      setupSettings({ wakeword: { enabled: true, wake_word: 'hey nexus', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: false, stt_require_both: false, voice_response_enabled: true } });
      render(<VoiceTab />);
      expect(screen.queryByRole('checkbox', { name: /Require both/i })).not.toBeInTheDocument();
    });

    it('AND logic checkbox IS shown when stt_wakeword_enabled is true', () => {
      setupSettings({ wakeword: { enabled: true, wake_word: 'hey nexus', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: true, stt_require_both: false, voice_response_enabled: true } });
      render(<VoiceTab />);
      expect(screen.getByRole('checkbox', { name: /Require both/i })).toBeInTheDocument();
    });

    it('enabling STT wake word calls setConfig with stt_wakeword_enabled: true', async () => {
      const user = userEvent.setup();
      const { setConfig, config } = setupSettings({ wakeword: { enabled: true, wake_word: 'hey nexus', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: false, stt_require_both: false, voice_response_enabled: true } });
      render(<VoiceTab />);

      await user.click(screen.getByRole('checkbox', { name: /STT Wake Word/i }));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          wakeword: expect.objectContaining({ stt_wakeword_enabled: true }),
        })
      );
    });

    it('AND logic checkbox calls setConfig with stt_require_both: true', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings({ wakeword: { enabled: true, wake_word: 'hey nexus', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: true, stt_require_both: false, voice_response_enabled: true } });
      render(<VoiceTab />);

      await user.click(screen.getByRole('checkbox', { name: /Require both/i }));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          wakeword: expect.objectContaining({ stt_require_both: true }),
        })
      );
    });
  });

  // ── Voice response toggle ─────────────────────────────────────────────────

  describe('voice response (TTS) toggle', () => {
    it('calls set_voice_response_enabled with false when toggled off', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings({ wakeword: { enabled: true, wake_word: 'hey nexus', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: false, stt_require_both: false, voice_response_enabled: true } });
      render(<VoiceTab />);

      await user.click(screen.getByRole('checkbox', { name: /Voice Response/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_voice_response_enabled', {
          enabled: false,
        })
      );
      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          wakeword: expect.objectContaining({ voice_response_enabled: false }),
        })
      );
    });

    it('calls set_voice_response_enabled with true when toggled on', async () => {
      const user = userEvent.setup();
      setupSettings({ wakeword: { enabled: true, wake_word: 'hey nexus', threshold: 0.85, sleep_timeout_seconds: 30, conversation_timeout_seconds: 60, stt_wakeword_enabled: false, stt_require_both: false, voice_response_enabled: false } });
      render(<VoiceTab />);

      await user.click(screen.getByRole('checkbox', { name: /Voice Response/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_voice_response_enabled', {
          enabled: true,
        })
      );
    });
  });

  // ── VAD settings ──────────────────────────────────────────────────────────

  describe('VAD settings', () => {
    it('shows threshold, min speech, min silence fields when VAD is enabled', () => {
      setupSettings({ vad: { enabled: true, threshold: 0.5, min_speech_duration_ms: 250, min_silence_duration_ms: 500 } });
      render(<VoiceTab />);
      expect(screen.getByDisplayValue('0.5')).toBeInTheDocument();
      expect(screen.getByDisplayValue('250')).toBeInTheDocument();
      expect(screen.getByDisplayValue('500')).toBeInTheDocument();
    });

    it('hides VAD parameter fields when VAD is disabled', () => {
      setupSettings({ vad: { enabled: false, threshold: 0.5, min_speech_duration_ms: 250, min_silence_duration_ms: 500 } });
      render(<VoiceTab />);
      expect(screen.queryByDisplayValue('0.5')).not.toBeInTheDocument();
    });

    it('calls setConfig with new VAD threshold', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings();
      render(<VoiceTab />);

      const thresholdInput = screen.getByDisplayValue('0.5');
      await user.clear(thresholdInput);
      await user.type(thresholdInput, '0.7');

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          vad: expect.objectContaining({ threshold: expect.any(Number) }),
        })
      );
    });

    it('toggling VAD off calls setConfig with vad.enabled: false', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings();
      render(<VoiceTab />);

      // Multiple "Enabled" checkboxes exist (STT, TTS, wakeword, VAD) — scope to the VAD section
      const vadHeading = screen.getByRole('heading', { name: /Voice Activity Detection/i });
      const vadSection = vadHeading.closest('.settings-group') as HTMLElement;
      await user.click(within(vadSection).getByRole('checkbox'));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          vad: expect.objectContaining({ enabled: false }),
        })
      );
    });
  });

  // ── Null-safe guard ───────────────────────────────────────────────────────

  it('renders nothing when config is null', () => {
    vi.mocked(useSettings).mockReturnValue({
      config: null,
      setConfig: vi.fn(),
      platformInfo: null,
      voiceServiceStatus: null,
      loadVoiceStatus: vi.fn(),
    } as ReturnType<typeof useSettings>);

    const { container } = render(<VoiceTab />);
    expect(container).toBeEmptyDOMElement();
  });
});
