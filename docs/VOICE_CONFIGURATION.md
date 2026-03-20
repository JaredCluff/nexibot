# Voice Configuration Guide

Complete guide to setting up and configuring NexiBot's voice pipeline for speech input, text-to-speech output, wake word detection, and voice activity detection.

## Voice Pipeline Overview

NexiBot's voice system is fully local-first with cloud fallback:

```
┌─────────────────────────────────────────────────────┐
│              Voice Input (User Speaking)             │
└────────────────┬────────────────────────────────────┘
                 ↓
    ┌────────────────────────────┐
    │   Wake Word Detection       │ (ONNX local)
    │   "hey nexibot"             │
    └────────┬───────────────────┘
             ↓
    ┌────────────────────────────┐
    │   Voice Activity Detection  │ (VAD)
    │   (Remove silence)          │
    └────────┬───────────────────┘
             ↓
    ┌────────────────────────────┐
    │   Speech to Text (STT)      │ (Local or Cloud)
    │   "what's the weather"      │
    └────────┬───────────────────┘
             ↓
    ┌────────────────────────────┐
    │   NexiBot Processing        │
    │   (Generate response)       │
    └────────┬───────────────────┘
             ↓
    ┌────────────────────────────┐
    │   Text to Speech (TTS)      │ (Local or Cloud)
    │   "It's 72 degrees..."      │
    └────────┬───────────────────┘
             ↓
┌─────────────────────────────────────────────────────┐
│           Voice Output (Speaker/Headphones)         │
└─────────────────────────────────────────────────────┘
```

## System Requirements

### Audio Hardware

- **Microphone**: Any USB or built-in microphone
- **Speaker**: Any output device
- **Quality**: Standard quality sufficient (not professional audio required)

### Software

- **macOS**: 11.0+ (native Speech framework built-in)
- **Windows**: 10+ (Windows Speech API built-in)
- **Linux**: ALSA, PulseAudio, or PipeWire

### Models

Downloaded automatically on first use:
- OpenWakeWord ONNX (~30MB)
- Silero VAD ONNX (~1MB)
- Optional: SenseVoice STT ONNX (~80MB)
- Optional: Piper TTS ONNX (~100-500MB per voice)

## Initial Setup

### Step 1: Enable Voice

**Settings > Voice**
- Toggle **Audio Input**: ON
- Toggle **Wake Word**: OFF (start without)

### Step 2: Select Microphone

**Settings > Voice > Input Device**
- Choose your microphone from dropdown
- Click **Test Record** to verify

Expected:
- Recording starts, play a sound (clap, speak)
- Recording stops
- Playback of recorded sound

If nothing records:
1. Check system audio settings
2. Grant microphone permission (macOS/Windows)
3. Try different device
4. Restart NexiBot

### Step 3: Choose STT Backend

**Settings > Voice > STT Backend**

Options:

| Backend | Cost | Latency | Accuracy | Offline | Setup |
|---------|------|---------|----------|---------|-------|
| macOS Speech | Free | Fast | Medium | Yes | None |
| Windows Speech | Free | Fast | Medium | Yes | None |
| SenseVoice (Local) | Free | Fast | High | Yes | Download model |
| Deepgram (Cloud) | $0.02/min | Medium | Very High | No | API key |
| OpenAI (Cloud) | $0.06/min | Medium | Very High | No | API key |

**Recommended:**
- **Offline use**: macOS/Windows Speech (built-in)
- **Best accuracy**: SenseVoice (local) or Deepgram (cloud)
- **Enterprise**: OpenAI or Deepgram

### Step 4: Choose TTS Backend

**Settings > Voice > TTS Backend**

Options:

| Backend | Cost | Latency | Quality | Voices | Offline | Setup |
|---------|------|---------|---------|--------|---------|-------|
| macOS `say` | Free | Very Fast | Medium | 40+ | Yes | None |
| Windows SAPI | Free | Very Fast | Medium | 10+ | Yes | None |
| Piper (Local) | Free | Fast | Medium-High | 50+ | Yes | Download |
| ElevenLabs | $0.03/min | Fast | Very High | 100+ | No | API key |
| Cerebras | $0.01/min | Fast | High | 50+ | No | API key |

**Recommended:**
- **Quick responses**: macOS/Windows (built-in)
- **Multiple voices**: Piper (local) or ElevenLabs (cloud)
- **Best quality**: ElevenLabs
- **Budget**: Cerebras

### Step 5: Enable Wake Word (Optional)

Wake word allows hands-free activation.

**Settings > Voice > Wake Word**
- Toggle: ON
- Phrase: "hey nexibot" (default)
- Sensitivity: 0.5 (0-1 scale)

Sensitivity tuning:
- **0.3**: Very sensitive (false positives)
- **0.5**: Default
- **0.7**: Less sensitive (requires clear speech)
- **0.9**: Very conservative (clear phrase required)

### Step 6: Test Everything

```
1. Settings > Voice > Record Test
   → Speak and hear playback

2. Settings > Voice > STT Test
   → Speak "hello world"
   → See transcription appear

3. Settings > Voice > TTS Test
   → Enter text
   → Hear it spoken aloud

4. Settings > Voice > Wake Word Test (if enabled)
   → Say "hey nexibot"
   → See detection indicator
```

## Voice Pipeline Components

### Wake Word Detection

Automatically triggers voice capture when you say the wake phrase.

**Configuration:**

```yaml
voice:
  wakeword:
    enabled: false
    phrase: "hey nexibot"              # Or customize
    threshold: 0.5                     # 0-1 (higher = stricter)
    models_dir: "~/.cache/nexibot/models/openwakeword"
    auto_download_models: true

    # AND-logic: Require 2 confirmations
    dual_confirmation: true            # Reduce false positives
    dual_confirmation_delay: 1.0       # Seconds between detections
```

**Tuning Wake Word:**

Too many false positives?
- Increase `threshold` to 0.7
- Enable `dual_confirmation`
- Choose less common phrase (not "ok" or "hey")

Not detecting phrase?
- Lower `threshold` to 0.3
- Check microphone is working
- Speak clearly at normal volume
- Make sure it's enabled

**Custom Wake Phrases:**

```yaml
wakeword:
  phrase: "computer"        # Works best: 2 syllables
  # Other ideas: "nexus", "beacon", "zephyr"
```

### Voice Activity Detection (VAD)

Automatically detects when you're speaking and stop when silent.

**Configuration:**

```yaml
voice:
  vad:
    enabled: true
    model: "silero"                    # ONNX-based
    threshold: 0.5                     # 0-1 (higher = stricter)
    min_silence_duration: 2.0          # Seconds
    initial_silence: 1.0               # Seconds before recording starts

    # Low confidence handling
    low_confidence_timeout: 5.0        # Continue listening if unsure
```

**How it works:**
1. Microphone captures audio
2. VAD analyzes each chunk
3. Silent parts removed automatically
4. Speaking parts sent to STT
5. Long silence (2+ sec) ends recording

**Tuning VAD:**

Background noise causing false stops?
- Increase `min_silence_duration` to 3.0

STT capturing background noise?
- Increase `threshold` to 0.7
- Enable noise gate (below)

Recording cuts off too early?
- Increase `min_silence_duration`
- Lower `threshold`

### Speech-to-Text (STT)

Converts spoken words to text.

#### macOS Speech (Built-in)

```yaml
voice:
  stt:
    backend: "macos"
    language: "en-US"
    continuous: true                   # Keep listening
```

**Pros:** Fast, free, no setup
**Cons:** Limited languages, offline only within app

#### SenseVoice (Local ONNX)

Open-source local STT with high accuracy.

```yaml
voice:
  stt:
    backend: "sensevoice"
    model: "sensevoice-small"          # or "sensevoice-large"
    language: "en"
    device: "cpu"                      # or "cuda" for GPU
    initial_prompt: "The user is talking about..."
```

**Setup:**

```bash
# Models auto-download on first use
# Or manually:
# ~/Library/Application Support/ai.nexibot.desktop/models/sensevoice/
```

**Pros:** Offline, free, high accuracy, multilingual
**Cons:** Slower (1-3 sec per utterance), requires model download

#### Cloud STT (Deepgram, OpenAI)

```yaml
voice:
  stt:
    backend: "deepgram"
    api_key: "${DEEPGRAM_API_KEY}"
    language: "en"
    model: "nova-2"                    # Deepgram's best model

  # Or OpenAI Whisper
  stt:
    backend: "openai"
    api_key: "${OPENAI_API_KEY}"
    model: "whisper-1"
```

**Pros:** Very high accuracy, multilingual, real-time
**Cons:** Requires internet, costs money, privacy implications

### Text-to-Speech (TTS)

Converts text responses to speech.

#### macOS `say` (Built-in)

```yaml
voice:
  tts:
    backend: "macos"
    voice: "Samantha"                  # or any macOS voice
    rate: 150                          # Words per minute (default: 150)
    pitch: 1.0                         # 0.5-2.0
```

**Available voices:**
```bash
# List all voices
say -v ? | head -20

# Popular voices:
# Samantha (female, American)
# Victoria (female, British)
# Daniel (male, American)
# Alex (male, American)
```

**Pros:** Fast, free, natural sounding
**Cons:** Limited voices, sometimes robotic

#### Piper (Local ONNX)

```yaml
voice:
  tts:
    backend: "piper"
    voice: "en_US-lessac-medium"       # 50+ voices available
    speaker: 0                         # Speaker variant (if multi-speaker)
    speed: 1.0                         # 0.5-2.0
    noise_scale: 0.667                 # 0-1 (lower = cleaner)
```

**Available voices:**
```bash
# Popular English voices:
# en_US-lessac-medium (female, natural)
# en_US-glow-tts-medium (female, expressive)
# en_US-libritts-high (female, high quality)
# en_GB-northern_english_male (male, British)
```

**Pros:** Offline, free, 50+ voices, high quality
**Cons:** Slower than cloud, requires model download

#### ElevenLabs (Cloud)

```yaml
voice:
  tts:
    backend: "elevenlabs"
    api_key: "${ELEVENLABS_API_KEY}"
    voice_id: "EXAVITQu4vr4xnSDxMaL"   # Or search by name
    model: "eleven_monolingual_v1"     # or "eleven_multilingual_v2"
    stability: 0.5                     # 0-1 (higher = more consistent)
    similarity_boost: 0.75             # 0-1 (higher = more like sample)
```

**Pros:** Excellent quality, 100+ voices, multilingual
**Cons:** Costs money, requires API key

## Advanced Configuration

### Noise Gate

Remove background noise before sending to STT.

```yaml
voice:
  preprocessing:
    noise_gate_enabled: true
    noise_gate_threshold: -30          # dB (lower = more sensitive)
    noise_gate_duration: 0.1           # Seconds

    # Normalization
    normalization_enabled: true
    target_level: -20                  # dB
```

### Audio Device Configuration

```yaml
voice:
  audio:
    # Input device (microphone)
    input_device: null                 # null = default, or device index
    input_sample_rate: 16000           # Hz
    input_channels: 1                  # Mono

    # Output device (speaker)
    output_device: null                # null = default
    output_sample_rate: 44100          # Hz
    output_channels: 2                 # Stereo
```

**List available devices:**

```bash
# macOS
system_profiler SPAudioDataType

# Linux
arecord -l  # Input devices
aplay -l    # Output devices

# Windows
Get-WmiObject win32_sounddevice
```

### Stop Phrases

Phrases that stop recording early.

```yaml
voice:
  stop_phrases:
    - "stop talking"
    - "shut up"
    - "stop"
    - "enough"
    # Custom phrases
    - "that's all"
    - "nevermind"
```

When you say any stop phrase, recording ends immediately without waiting for silence.

### Recording Duration Limits

```yaml
voice:
  recording:
    # Maximum recording duration
    max_duration: 60                   # Seconds (per utterance)

    # Minimum recording duration
    min_duration: 0.5                  # Seconds

    # Timeout for detecting speech start
    initial_timeout: 10                # Seconds
```

## Troubleshooting

### Microphone Not Detected

1. **Check system permissions:**
   - **macOS**: System Preferences > Security & Privacy > Microphone
   - **Windows**: Settings > Privacy & security > Microphone

2. **Restart audio service:**
   ```bash
   # macOS
   killall coreaudiod
   # Wait 5 seconds, audio service restarts

   # Linux
   systemctl --user restart pulseaudio
   ```

3. **Try default device:**
   ```yaml
   voice:
     audio:
       input_device: null  # Use system default
   ```

### STT Not Recognizing Speech

**Problem:** Speech not being converted to text

**Solutions:**
1. Test microphone: Settings > Voice > Record Test
2. Lower VAD threshold: `vad.threshold: 0.3`
3. Disable VAD temporarily: `vad.enabled: false`
4. Try different STT backend (macOS → SenseVoice)
5. Check audio levels: Speak louder

### TTS Not Playing

**Problem:** Text isn't being spoken aloud

**Solutions:**
1. Check speaker volume
2. Test TTS: Settings > Voice > TTS Test
3. Verify output device: Settings > Voice > Output Device
4. Restart audio service
5. Try different TTS backend (macOS → ElevenLabs)

### Wake Word Not Detecting

**Problem:** "Hey nexibot" isn't waking up the agent

**Solutions:**
1. Enable testing: Settings > Voice > Wake Word Test
2. Speak clearly at normal volume
3. Lower sensitivity: `threshold: 0.3`
4. Disable dual_confirmation temporarily to test
5. Try without background noise
6. Check microphone is working

### Latency Too High

**Problem:** Slow response time from voice input to response

**Causes:**
- Cloud services (network delay)
- Slow STT model (SenseVoice large is slower)
- Processing overhead

**Solutions:**
1. Use local backends (macOS speech, Piper)
2. Switch to SenseVoice small model (faster)
3. Use faster cloud provider (Deepgram > OpenAI)
4. Reduce audio quality (lower sample rate)

### Audio Quality Issues

**Problem:** Robotic voice, poor quality, distortion

**Solutions:**
1. For macOS: Try different voice (Victoria instead of Samantha)
2. For Piper: Try different model (lessac instead of glow)
3. For cloud: Increase similarity_boost (ElevenLabs)
4. Check microphone isn't clipping (too loud)
5. Reduce background noise

## Performance Tips

### Fastest Setup (Minimal Latency)

```yaml
voice:
  # Wake word off
  wakeword:
    enabled: false

  # macOS speech (built-in)
  stt:
    backend: "macos"
  tts:
    backend: "macos"

  # Minimal preprocessing
  vad:
    min_silence_duration: 1.0
  preprocessing:
    noise_gate_enabled: false
```

### Best Quality Setup

```yaml
voice:
  # Wake word with confirmation
  wakeword:
    enabled: true
    dual_confirmation: true

  # High-accuracy STT
  stt:
    backend: "deepgram"
    model: "nova-2"

  # Best TTS quality
  tts:
    backend: "elevenlabs"
    model: "eleven_multilingual_v2"
    stability: 0.7

  # Good preprocessing
  vad:
    threshold: 0.5
  preprocessing:
    noise_gate_enabled: true
    normalization_enabled: true
```

### Budget-Friendly Setup (Free)

```yaml
voice:
  # Local everything
  wakeword:
    enabled: true
    phrase: "computer"

  stt:
    backend: "sensevoice"

  tts:
    backend: "piper"
    voice: "en_US-lessac-medium"

  # Good balance
  vad:
    threshold: 0.5
  preprocessing:
    noise_gate_enabled: true
```

## Privacy Considerations

### Local-Only Mode

For complete privacy, use only local components:

```yaml
voice:
  # All local
  wakeword:
    enabled: true

  stt:
    backend: "sensevoice"     # Runs locally

  tts:
    backend: "piper"          # Runs locally

  # Audio never leaves device
```

### Cloud Service Privacy

When using cloud services:

- **Deepgram**: Transcriptions not stored by default
- **OpenAI Whisper**: Similar privacy policy to ChatGPT
- **ElevenLabs**: Voice samples stored for speaker identification

For maximum privacy, use local backends only.

## Environment Variables

```bash
# Voice enabled
export NEXIBOT_VOICE_ENABLED=true

# STT configuration
export NEXIBOT_STT_BACKEND=sensevoice
export DEEPGRAM_API_KEY="your-key"
export OPENAI_API_KEY="your-key"

# TTS configuration
export NEXIBOT_TTS_BACKEND=piper
export ELEVENLABS_API_KEY="your-key"

# Wake word
export NEXIBOT_WAKEWORD_ENABLED=true
export NEXIBOT_WAKEWORD_PHRASE="hey nexus"
```

## Voice with Channels

Use voice with messaging channels for phone-like interface:

```yaml
# Telegram voice messages
telegram:
  enabled: true
  voice_enabled: true        # Accept voice messages
  send_voice_responses: true # Reply with voice

# WhatsApp voice
whatsapp:
  enabled: true
  voice_enabled: true

# Email voice transcription
email:
  enabled: true
  transcribe_attachments: true  # Convert audio files to text
```

## See Also

- [Setup NexiBot](./SETUP_NEXIBOT.md) - Installation guide
- [Channels Setup](./CHANNELS_SETUP.md) - Messaging channel integration
- [Memory and Context](./MEMORY_AND_CONTEXT.md) - Conversation memory
