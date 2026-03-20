//! Windows platform bridge
//!
//! Provides Windows SAPI speech helpers and platform detection utilities.


/// Check if Windows SAPI is available (always true on Windows)
#[allow(dead_code)]
pub fn has_windows_sapi() -> bool {
    cfg!(target_os = "windows")
}

/// Synthesize speech to WAV bytes using Windows SAPI
///
/// Uses COM ISpVoice interface to generate speech audio.
#[cfg(target_os = "windows")]
pub fn sapi_synthesize(text: &str, voice_name: Option<&str>) -> anyhow::Result<Vec<u8>> {
    use std::process::Command;

    // Use PowerShell to access SAPI — avoids direct COM FFI complexity
    // This generates a WAV file and returns its bytes
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("nexibot_sapi_{}.wav", uuid::Uuid::new_v4()));
    let temp_path = temp_file.to_string_lossy().to_string();

    let voice_selection = if let Some(name) = voice_name {
        // Escape single quotes for PowerShell single-quoted string context (double them)
        let safe_name = name.replace('\'', "''");
        format!(
            r#"$voice = $synth.GetInstalledVoices() | Where-Object {{ $_.VoiceInfo.Name -like '*{}*' }} | Select-Object -First 1; if ($voice) {{ $synth.SelectVoice($voice.VoiceInfo.Name) }}"#,
            safe_name
        )
    } else {
        String::new()
    };

    let script = format!(
        r#"
Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
{voice_selection}
$synth.SetOutputToWaveFile('{temp_path}')
$synth.Speak('{text}')
$synth.Dispose()
"#,
        voice_selection = voice_selection,
        temp_path = temp_path,
        text = text.replace('\'', "''"),
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run PowerShell SAPI: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("SAPI synthesis failed: {}", stderr);
    }

    let audio_bytes = std::fs::read(&temp_file)
        .map_err(|e| anyhow::anyhow!("Failed to read SAPI output: {}", e))?;
    let _ = std::fs::remove_file(&temp_file);

    Ok(audio_bytes)
}

#[cfg(not(target_os = "windows"))]
pub fn sapi_synthesize(_text: &str, _voice_name: Option<&str>) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("Windows SAPI only available on Windows")
}

/// Recognize speech from WAV audio using Windows SAPI
#[cfg(target_os = "windows")]
pub fn sapi_recognize(audio: &[f32]) -> anyhow::Result<String> {
    use std::process::Command;

    // Write audio to a temporary WAV file
    let temp_dir = std::env::temp_dir();
    let wav_path = temp_dir.join(format!("nexibot_sapi_stt_{}.wav", uuid::Uuid::new_v4()));
    write_wav_file(&wav_path, audio, 16000)?;

    let wav_path_str = wav_path.to_string_lossy().to_string();

    let script = format!(
        r#"
Add-Type -AssemblyName System.Speech
$recognizer = New-Object System.Speech.Recognition.SpeechRecognitionEngine
$recognizer.SetInputToWaveFile('{wav_path}')
$recognizer.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar))
try {{
    $result = $recognizer.Recognize()
    if ($result) {{ Write-Output $result.Text }}
}} catch {{
    Write-Error $_.Exception.Message
}} finally {{
    $recognizer.Dispose()
}}
"#,
        wav_path = wav_path_str,
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run PowerShell SAPI STT: {}", e))?;

    let _ = std::fs::remove_file(&wav_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("SAPI recognition failed: {}", stderr);
    }

    let transcript = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(transcript)
}

#[cfg(not(target_os = "windows"))]
pub fn sapi_recognize(_audio: &[f32]) -> anyhow::Result<String> {
    anyhow::bail!("Windows SAPI only available on Windows")
}

/// Write f32 audio samples to a WAV file
fn write_wav_file(path: &std::path::Path, samples: &[f32], sample_rate: u32) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)?;
    for &sample in samples {
        let s = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(())
}
