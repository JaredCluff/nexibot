import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import { InfoGuide } from '../shared/InfoGuide';
import { notifyError } from '../../../shared/notify';

const STT_BACKEND_LABELS: Record<string, string> = {
  macos_speech: 'macOS Speech (native)',
  windows_speech: 'Windows Speech (native)',
  sensevoice: 'SenseVoice (local)',
  deepgram: 'Deepgram (cloud)',
  openai: 'OpenAI (cloud)',
};

const TTS_BACKEND_LABELS: Record<string, string> = {
  macos_say: 'macOS say (native)',
  windows_sapi: 'Windows SAPI (native)',
  piper: 'Piper (local)',
  espeak: 'espeak-ng (local)',
  elevenlabs: 'ElevenLabs (cloud)',
  cartesia: 'Cartesia (cloud)',
};

export function VoiceTab() {
  const { config, setConfig, platformInfo, voiceServiceStatus, loadVoiceStatus } = useSettings();
  const [testingTts, setTestingTts] = useState(false);
  const [testingStt, setTestingStt] = useState(false);
  const [sttResult, setSttResult] = useState('');
  const [togglingVoice, setTogglingVoice] = useState(false);

  if (!config) return null;

  const sttBackends = platformInfo?.available_stt_backends || ['sensevoice', 'deepgram', 'openai'];
  const ttsBackends = platformInfo?.available_tts_backends || ['piper', 'elevenlabs', 'cartesia'];

  const handleTestTts = async () => {
    setTestingTts(true);
    try {
      await invoke('test_tts', { text: 'Hello, this is a test of text to speech.', voice: config.tts.macos_voice });
    } catch (error) {
      notifyError('Voice', `TTS test failed: ${error}`);
    } finally {
      setTestingTts(false);
    }
  };

  const handleTestStt = async () => {
    setTestingStt(true);
    setSttResult('');
    try {
      const result = await invoke<string>('test_stt');
      setSttResult(result || '(no speech detected)');
    } catch (error) {
      notifyError('Voice', `STT test failed: ${error}`);
    } finally {
      setTestingStt(false);
    }
  };

  const voiceRunning = voiceServiceStatus?.state === 'Listening' || voiceServiceStatus?.state === 'Processing' || voiceServiceStatus?.state === 'Speaking';

  return (
    <div className="tab-content">
      <div className="settings-group">
        <h3>Voice Pipeline <InfoTip text="The voice pipeline handles audio capture, wake word detection, speech-to-text, and text-to-speech. Start or stop the entire pipeline here." /></h3>
        <div className="status-indicator">
          <span className={`status-dot ${voiceRunning ? 'healthy' : 'inactive'}`} />
          <span>{voiceServiceStatus?.state || 'Unknown'}</span>
          {voiceServiceStatus && (
            <span className="hint"> — STT: {voiceServiceStatus.stt_backend} | TTS: {voiceServiceStatus.tts_backend}</span>
          )}
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={voiceRunning}
              disabled={togglingVoice}
              onChange={async (e) => {
                setTogglingVoice(true);
                try {
                  if (e.target.checked) {
                    await invoke('start_voice_service');
                  } else {
                    await invoke('stop_voice_service');
                  }
                  await loadVoiceStatus();
                } catch (error) {
                  notifyError('Voice', `Failed to ${e.target.checked ? 'start' : 'stop'} voice service: ${error}`);
                } finally {
                  setTogglingVoice(false);
                }
              }}
            />
            {togglingVoice ? 'Loading...' : 'Enable Voice Pipeline'} <InfoTip text="Start or stop the full voice pipeline including audio capture, wake word detection, and speech processing." />
          </label>
        </div>
      </div>

      <div className="settings-row">
        <div className="settings-group compact">
          <h3>Speech-to-Text (STT) <InfoTip text="Converts spoken audio to text. Required for voice input, wake word, and push-to-talk features." /></h3>
          <div className="inline-toggle">
            <label className="toggle-label">
              <input
                type="checkbox"
                checked={config.stt.enabled}
                onChange={(e) => setConfig({ ...config, stt: { ...config.stt, enabled: e.target.checked } })}
              />
              Enabled <InfoTip text="Turn on speech recognition. Select a backend below based on your needs — local options work offline, cloud options are more accurate." />
            </label>
            <InfoTip text="The speech recognition engine. macOS Speech is built-in and free. Deepgram and OpenAI offer higher accuracy via cloud APIs." />
            <select
              value={config.stt.backend}
              onChange={(e) => setConfig({ ...config, stt: { ...config.stt, backend: e.target.value } })}
            >
              {sttBackends.map((backend) => (
                <option key={backend} value={backend}>
                  {STT_BACKEND_LABELS[backend] || backend}
                </option>
              ))}
            </select>
          </div>

          {config.stt.backend === 'sensevoice' && (
            <label className="field">
              <span>SenseVoice Model Path <InfoTip text="Path to a local SenseVoice ONNX model file. Leave empty to auto-download the default model." /></span>
              <input
                type="text"
                value={config.stt.sensevoice_model_path || ''}
                onChange={(e) => setConfig({ ...config, stt: { ...config.stt, sensevoice_model_path: e.target.value || undefined } })}
                placeholder="SenseVoice model path (optional)"
              />
            </label>
          )}
          {config.stt.backend === 'deepgram' && (
            <label className="field">
              <span>Deepgram API Key <InfoTip text="API key from deepgram.com. Deepgram offers real-time streaming STT with high accuracy." /></span>
              <input
                type="password"
                value={config.stt.deepgram_api_key || ''}
                onChange={(e) => setConfig({ ...config, stt: { ...config.stt, deepgram_api_key: e.target.value } })}
                placeholder="Deepgram API key"
              />
            </label>
          )}
          {config.stt.backend === 'openai' && (
            <label className="field">
              <span>OpenAI API Key <InfoTip text="API key from platform.openai.com. Uses the Whisper model for transcription." /></span>
              <input
                type="password"
                value={config.stt.openai_api_key || ''}
                onChange={(e) => setConfig({ ...config, stt: { ...config.stt, openai_api_key: e.target.value } })}
                placeholder="OpenAI API key"
              />
            </label>
          )}

          <button className="test-button" onClick={handleTestStt} disabled={testingStt}>
            {testingStt ? 'Listening (3s)...' : 'Test STT'}
          </button>
          {sttResult && <span className="test-result">"{sttResult}"</span>}
        </div>

        <div className="settings-group compact">
          <h3>Text-to-Speech (TTS) <InfoTip text="Converts text responses to spoken audio. Lets NexiBot talk back to you." /></h3>
          <div className="inline-toggle">
            <label className="toggle-label">
              <input
                type="checkbox"
                checked={config.tts.enabled}
                onChange={(e) => setConfig({ ...config, tts: { ...config.tts, enabled: e.target.checked } })}
              />
              Enabled <InfoTip text="Turn on voice output. NexiBot will speak responses aloud using the selected backend." />
            </label>
            <InfoTip text="The voice synthesis engine. macOS Say is built-in and free. ElevenLabs and Cartesia offer premium, natural-sounding voices." />
            <select
              value={config.tts.backend}
              onChange={(e) => setConfig({ ...config, tts: { ...config.tts, backend: e.target.value } })}
            >
              {ttsBackends.map((backend) => (
                <option key={backend} value={backend}>
                  {TTS_BACKEND_LABELS[backend] || backend}
                </option>
              ))}
            </select>
          </div>

          {config.tts.backend === 'macos_say' && (
            <label className="field">
              <span>macOS Voice <InfoTip text="Which macOS system voice to use. Zoe (Premium) sounds the most natural." /></span>
              <select
                value={config.tts.macos_voice}
                onChange={(e) => setConfig({ ...config, tts: { ...config.tts, macos_voice: e.target.value } })}
              >
                <option value="Samantha">Samantha</option>
                <option value="Alex">Alex</option>
                <option value="Zoe">Zoe (Premium)</option>
                <option value="Victoria">Victoria</option>
                <option value="Karen">Karen</option>
              </select>
            </label>
          )}
          {config.tts.backend === 'windows_sapi' && (
            <input
              type="text"
              value={config.tts.windows_voice || ''}
              onChange={(e) => setConfig({ ...config, tts: { ...config.tts, windows_voice: e.target.value || undefined } })}
              placeholder="Windows voice (optional, system default)"
            />
          )}
          {config.tts.backend === 'piper' && (
            <>
              <label className="field">
                <span>Piper Model Path <InfoTip text="Piper is a fast, local TTS engine. Provide a path to the Piper ONNX model file." /></span>
                <input
                  type="text"
                  value={config.tts.piper_model_path || ''}
                  onChange={(e) => setConfig({ ...config, tts: { ...config.tts, piper_model_path: e.target.value || undefined } })}
                  placeholder="Piper ONNX model path (optional)"
                />
              </label>
              <label className="field">
                <span>Piper Voice <InfoTip text="Optionally specify a voice name for the Piper model." /></span>
                <input
                  type="text"
                  value={config.tts.piper_voice || ''}
                  onChange={(e) => setConfig({ ...config, tts: { ...config.tts, piper_voice: e.target.value || undefined } })}
                  placeholder="Piper voice name (optional)"
                />
              </label>
            </>
          )}
          {config.tts.backend === 'espeak' && (
            <label className="field">
              <span>espeak Voice <InfoTip text="The language/voice for eSpeak. Default is 'en' for English." /></span>
              <input
                type="text"
                value={config.tts.espeak_voice || ''}
                onChange={(e) => setConfig({ ...config, tts: { ...config.tts, espeak_voice: e.target.value || undefined } })}
                placeholder="espeak-ng voice/language (default: en)"
              />
            </label>
          )}
          {config.tts.backend === 'elevenlabs' && (
            <label className="field">
              <span>ElevenLabs API Key <InfoTip text="API key from elevenlabs.io. Provides premium, natural-sounding voice synthesis." /></span>
              <input
                type="password"
                value={config.tts.elevenlabs_api_key || ''}
                onChange={(e) => setConfig({ ...config, tts: { ...config.tts, elevenlabs_api_key: e.target.value } })}
                placeholder="ElevenLabs API key"
              />
            </label>
          )}
          {config.tts.backend === 'cartesia' && (
            <label className="field">
              <span>Cartesia API Key <InfoTip text="API key from cartesia.ai. Provides fast, high-quality voice synthesis." /></span>
              <input
                type="password"
                value={config.tts.cartesia_api_key || ''}
                onChange={(e) => setConfig({ ...config, tts: { ...config.tts, cartesia_api_key: e.target.value } })}
                placeholder="Cartesia API key"
              />
            </label>
          )}

          <button className="test-button" onClick={handleTestTts} disabled={testingTts}>
            {testingTts ? 'Speaking...' : 'Test TTS'}
          </button>
        </div>
      </div>

      <div className="settings-group">
        <h3>Wake Word <InfoTip text="Hands-free activation. Say the wake word to start a conversation without clicking anything." /></h3>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.wakeword.enabled}
              onChange={(e) => setConfig({ ...config, wakeword: { ...config.wakeword, enabled: e.target.checked } })}
            />
            Enabled <InfoTip text="When enabled, NexiBot listens for the wake word in the background using a lightweight ONNX model." />
          </label>
        </div>
        {config.wakeword.enabled && (
          <>
            <div className="settings-row">
              <label className="field">
                <span>Wake Word <InfoTip text="The phrase that activates NexiBot. Default is 'hey nexus'." /></span>
                <input
                  type="text"
                  value={config.wakeword.wake_word}
                  onChange={(e) => setConfig({ ...config, wakeword: { ...config.wakeword, wake_word: e.target.value } })}
                />
              </label>
              <label className="field">
                <span>Threshold <InfoTip text="Detection confidence required to trigger (0.0-1.0). Recommended: 0.95." /></span>
                <input
                  type="number" min="0" max="1" step="0.01"
                  value={config.wakeword.threshold}
                  onChange={(e) => setConfig({ ...config, wakeword: { ...config.wakeword, threshold: parseFloat(e.target.value) } })}
                />
              </label>
              <label className="field">
                <span>Sleep Timeout (s) <InfoTip text="Seconds of inactivity before the wake word detector enters sleep mode." /></span>
                <input
                  type="number" min="0"
                  value={config.wakeword.sleep_timeout_seconds}
                  onChange={(e) => setConfig({ ...config, wakeword: { ...config.wakeword, sleep_timeout_seconds: parseInt(e.target.value) } })}
                />
              </label>
              <label className="field">
                <span>Conversation Timeout (s) <InfoTip text="After a voice exchange ends, NexiBot stays in conversation mode for this many seconds." /></span>
                <input
                  type="number" min="0"
                  value={config.wakeword.conversation_timeout_seconds}
                  onChange={(e) => setConfig({ ...config, wakeword: { ...config.wakeword, conversation_timeout_seconds: parseInt(e.target.value) } })}
                />
              </label>
            </div>

            {config.wakeword.wake_word.toLowerCase() !== 'hey nexus' &&
             !config.wakeword.wake_word.toLowerCase().includes('nexi') && (
              <div className="warning-banner" style={{ marginTop: '8px' }}>
                Custom wake words require a trained ONNX model matching your phrase. Without one, the detector will fall back to STT-based detection (slower, ~3s latency). Enable "STT Wake Word" below if you don't have a custom ONNX model.
              </div>
            )}

            <InfoGuide title="How wake word detection works">
              <p>NexiBot uses a lightweight ONNX neural network that runs entirely on your device — no audio is sent to any server.</p>
              <p>The built-in model is trained for <strong>"hey nexus"</strong> and similar variations. It processes 80ms audio frames through three stages: mel spectrogram, embedding, and classification.</p>
              <p><strong>Custom wake words:</strong> If you change the wake word, you need a properly trained ONNX model file. Without it, NexiBot will fall back to STT-based detection (if enabled).</p>
              <p><strong>Troubleshooting:</strong> If the wake word isn't activating, try lowering the threshold (e.g., 0.90), speak clearly, and ensure your microphone is working.</p>
            </InfoGuide>

            <div className="inline-toggle" style={{ marginTop: '8px' }}>
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.wakeword.stt_wakeword_enabled}
                  onChange={(e) => setConfig({ ...config, wakeword: { ...config.wakeword, stt_wakeword_enabled: e.target.checked } })}
                />
                STT Wake Word <InfoTip text="Use local speech-to-text to detect the wake phrase. Higher latency (~3s). Can run alongside ONNX detection." />
              </label>
              <span className="hint">STT-based wake word detection (native speech API, ~3s latency)</span>
            </div>

            {config.wakeword.stt_wakeword_enabled && (
              <div className="inline-toggle" style={{ marginTop: '8px', marginLeft: '20px' }}>
                <label className="toggle-label">
                  <input
                    type="checkbox"
                    checked={config.wakeword.stt_require_both}
                    onChange={(e) => setConfig({ ...config, wakeword: { ...config.wakeword, stt_require_both: e.target.checked } })}
                  />
                  Require both (AND logic) <InfoTip text="When both ONNX and STT wake word are enabled, require BOTH to confirm before activating. Reduces false positives but increases latency." />
                </label>
                <span className="hint">Require both ONNX and STT to confirm before waking</span>
              </div>
            )}

            <div className="inline-toggle" style={{ marginTop: '12px' }}>
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.wakeword.voice_response_enabled}
                  onChange={async (e) => {
                    const enabled = e.target.checked;
                    setConfig({ ...config, wakeword: { ...config.wakeword, voice_response_enabled: enabled } });
                    try {
                      await invoke('set_voice_response_enabled', { enabled });
                    } catch (err) {
                      notifyError('Voice', `Failed to set voice response: ${err}`);
                    }
                  }}
                />
                Voice Response (TTS) <InfoTip text="When enabled, Claude's responses are spoken aloud via text-to-speech. When disabled, responses appear in the chat window only." />
              </label>
              <span className="hint">Speak Claude's responses aloud (uncheck for text-only mode)</span>
            </div>
          </>
        )}
      </div>

      <div className="settings-group">
        <h3>Voice Activity Detection (VAD) <InfoTip text="Detects when you start and stop speaking. Helps separate speech from background noise." /></h3>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.vad.enabled}
              onChange={(e) => setConfig({ ...config, vad: { ...config.vad, enabled: e.target.checked } })}
            />
            Enabled <InfoTip text="When enabled, uses Silero VAD to detect speech boundaries for cleaner transcription." />
          </label>
        </div>
        {config.vad.enabled && (
          <div className="settings-row">
            <label className="field">
              <span>Threshold <InfoTip text="How sensitive the detector is. Range: 0.0-1.0." /></span>
              <input
                type="number" min="0" max="1" step="0.01"
                value={config.vad.threshold}
                onChange={(e) => setConfig({ ...config, vad: { ...config.vad, threshold: parseFloat(e.target.value) } })}
              />
            </label>
            <label className="field">
              <span>Min Speech (ms) <InfoTip text="Minimum duration of speech before it's considered valid." /></span>
              <input
                type="number" min="0"
                value={config.vad.min_speech_duration_ms}
                onChange={(e) => setConfig({ ...config, vad: { ...config.vad, min_speech_duration_ms: parseInt(e.target.value) } })}
              />
            </label>
            <label className="field">
              <span>Min Silence (ms) <InfoTip text="How long a pause must last before speech is considered finished." /></span>
              <input
                type="number" min="0"
                value={config.vad.min_silence_duration_ms}
                onChange={(e) => setConfig({ ...config, vad: { ...config.vad, min_silence_duration_ms: parseInt(e.target.value) } })}
              />
            </label>
          </div>
        )}
      </div>
    </div>
  );
}
