//! Audio preprocessing for the voice pipeline.
//!
//! Provides noise gate, normalization, and basic audio filtering
//! to improve STT accuracy.

use tracing::debug;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for audio preprocessing.
#[derive(Debug, Clone)]
pub struct PreprocessingConfig {
    /// Amplitude threshold below which samples are gated to zero.
    pub noise_gate_threshold: f32,
    /// Whether to normalise audio to a target RMS level.
    pub normalize: bool,
    /// Target RMS level for normalisation.
    pub target_rms: f32,
    /// High-pass filter cutoff frequency in Hz (removes low rumble / DC offset).
    pub high_pass_cutoff_hz: f32,
    /// Master switch -- when false, `preprocess_audio` is a no-op.
    pub enabled: bool,
}

impl Default for PreprocessingConfig {
    fn default() -> Self {
        Self {
            noise_gate_threshold: 0.02,
            normalize: true,
            target_rms: 0.1,
            high_pass_cutoff_hz: 80.0,
            enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Individual processing stages
// ---------------------------------------------------------------------------

/// Apply a noise gate that zeroes samples below `threshold`.
///
/// A smooth 5 ms ramp is applied at gate open/close transitions to avoid
/// audible clicks.
pub fn apply_noise_gate(samples: &mut [f32], threshold: f32) {
    if samples.is_empty() {
        return;
    }

    // 5 ms ramp at 16 kHz = 80 samples
    let ramp_samples: usize = 80;

    // First pass: determine gate state per sample (open = above threshold).
    let gate_open: Vec<bool> = samples.iter().map(|s| s.abs() >= threshold).collect();

    // Second pass: apply gain with ramp at transitions.
    let mut gain: f32 = if gate_open[0] { 1.0 } else { 0.0 };
    let ramp_step = 1.0 / ramp_samples as f32;

    for (i, sample) in samples.iter_mut().enumerate() {
        let target = if gate_open[i] { 1.0 } else { 0.0 };
        if (gain - target).abs() > f32::EPSILON {
            if target > gain {
                gain = (gain + ramp_step).min(1.0);
            } else {
                gain = (gain - ramp_step).max(0.0);
            }
        } else {
            gain = target;
        }
        *sample *= gain;
    }
}

/// Normalise audio so that its RMS level matches `target_rms`.
///
/// Samples are clamped to [-1.0, 1.0] after scaling.
pub fn normalize_audio(samples: &mut [f32], target_rms: f32) {
    let current_rms = calculate_rms(samples);
    if current_rms < 1e-8 {
        // Silence -- nothing to normalise.
        return;
    }

    let scale = target_rms / current_rms;
    debug!(
        "[PREPROCESS] Normalizing: current_rms={:.6}, target_rms={:.4}, scale={:.4}",
        current_rms, target_rms, scale
    );

    for sample in samples.iter_mut() {
        *sample = (*sample * scale).clamp(-1.0, 1.0);
    }
}

/// Apply a first-order RC high-pass filter.
///
/// Transfer function: `y[n] = alpha * (y[n-1] + x[n] - x[n-1])`
/// where `alpha = RC / (RC + dt)`, `RC = 1 / (2 * PI * cutoff_hz)`.
pub fn apply_high_pass_filter(samples: &mut [f32], cutoff_hz: f32, sample_rate: u32) {
    if samples.is_empty() || cutoff_hz <= 0.0 || sample_rate == 0 {
        return;
    }

    let dt = 1.0 / sample_rate as f32;
    let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
    let alpha = rc / (rc + dt);

    let mut prev_x = samples[0];
    let mut prev_y = samples[0];

    // Process from the second sample onward; the first sample is left as-is
    // (initial condition).
    for i in 1..samples.len() {
        let x = samples[i];
        let y = alpha * (prev_y + x - prev_x);
        samples[i] = y;
        prev_x = x;
        prev_y = y;
    }
}

// ---------------------------------------------------------------------------
// Combined pipeline
// ---------------------------------------------------------------------------

/// Run the full preprocessing pipeline on raw audio samples in-place.
///
/// Processing order:
/// 1. High-pass filter (remove low-frequency rumble / DC offset)
/// 2. Noise gate (silence very quiet passages)
/// 3. Normalisation (bring overall level to target RMS)
pub fn preprocess_audio(samples: &mut [f32], config: &PreprocessingConfig, sample_rate: u32) {
    if !config.enabled || samples.is_empty() {
        return;
    }

    debug!(
        "[PREPROCESS] Processing {} samples (sample_rate={})",
        samples.len(),
        sample_rate
    );

    // 1. High-pass filter
    apply_high_pass_filter(samples, config.high_pass_cutoff_hz, sample_rate);

    // 2. Noise gate
    apply_noise_gate(samples, config.noise_gate_threshold);

    // 3. Normalisation
    if config.normalize {
        normalize_audio(samples, config.target_rms);
    }
}

// ---------------------------------------------------------------------------
// Analysis utilities
// ---------------------------------------------------------------------------

/// Calculate the Root Mean Square (RMS) level of the samples.
pub fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// Calculate the peak absolute amplitude.
#[allow(dead_code)]
pub fn calculate_peak(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max)
}

/// Estimate the Signal-to-Noise Ratio in dB.
///
/// Uses `noise_floor` as the assumed RMS of the noise. If the signal RMS is
/// below or equal to the noise floor, returns 0.0 (no signal).
#[allow(dead_code)]
pub fn calculate_snr_estimate(samples: &[f32], noise_floor: f32) -> f32 {
    if noise_floor <= 0.0 {
        return 0.0;
    }
    let signal_rms = calculate_rms(samples);
    if signal_rms <= noise_floor {
        return 0.0;
    }
    20.0 * (signal_rms / noise_floor).log10()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noise_gate_zeros_quiet_samples() {
        let threshold = 0.05;
        // Use a longer signal so the ramp (80 samples at 16kHz) has room to
        // fully open.  100 quiet samples, then 100 loud samples, then 100 quiet.
        let mut samples: Vec<f32> = Vec::with_capacity(300);
        for _ in 0..100 {
            samples.push(0.01);
        } // below threshold
        for _ in 0..100 {
            samples.push(0.2);
        } // above threshold
        for _ in 0..100 {
            samples.push(0.01);
        } // below threshold again

        apply_noise_gate(&mut samples, threshold);

        // First quiet region should be fully gated (gain starts at 0)
        assert!(
            samples[0].abs() < 0.001,
            "sample[0] should be gated: {}",
            samples[0]
        );
        assert!(
            samples[50].abs() < 0.001,
            "sample[50] should be gated: {}",
            samples[50]
        );

        // Well into the loud region (past the 80-sample ramp), gain should be ~1.0
        assert!(
            samples[190].abs() > 0.15,
            "sample[190] should be preserved: {}",
            samples[190]
        );

        // Well into the trailing quiet region (past ramp-down), should be gated
        assert!(
            samples[290].abs() < 0.001,
            "sample[290] should be gated: {}",
            samples[290]
        );
    }

    #[test]
    fn test_normalize_scales_correctly() {
        // Create a signal with known RMS
        let mut samples: Vec<f32> = (0..1000)
            .map(|i| 0.01 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();

        let rms_before = calculate_rms(&samples);
        assert!(rms_before < 0.05, "Pre-norm RMS should be low");

        normalize_audio(&mut samples, 0.1);
        let rms_after = calculate_rms(&samples);

        // RMS should now be close to 0.1 (within tolerance for clamping effects)
        assert!(
            (rms_after - 0.1).abs() < 0.02,
            "Post-norm RMS should be ~0.1, got {}",
            rms_after
        );
    }

    #[test]
    fn test_high_pass_removes_dc_offset() {
        // Signal with a large DC offset
        let mut samples: Vec<f32> = (0..16000)
            .map(|i| 0.5 + 0.1 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();

        // Mean before should be around 0.5
        let mean_before: f32 = samples.iter().sum::<f32>() / samples.len() as f32;
        assert!(
            mean_before.abs() > 0.3,
            "Mean before HP should be ~0.5, got {}",
            mean_before
        );

        apply_high_pass_filter(&mut samples, 80.0, 16000);

        // After high-pass, DC offset should be mostly removed.
        // Check the latter half of the signal (after filter settles).
        let latter_half = &samples[8000..];
        let mean_after: f32 = latter_half.iter().sum::<f32>() / latter_half.len() as f32;
        assert!(
            mean_after.abs() < 0.05,
            "Mean after HP should be near 0, got {}",
            mean_after
        );
    }

    #[test]
    fn test_preprocess_full_pipeline() {
        let config = PreprocessingConfig::default();

        // Signal: 440 Hz sine at low amplitude with DC offset
        let mut samples: Vec<f32> = (0..16000)
            .map(|i| 0.3 + 0.005 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();

        let rms_before = calculate_rms(&samples);
        preprocess_audio(&mut samples, &config, 16000);
        let rms_after = calculate_rms(&samples);

        // The signal should have been processed (RMS changed)
        assert!(
            (rms_after - rms_before).abs() > 0.001,
            "Pipeline should change the signal: before={}, after={}",
            rms_before,
            rms_after
        );

        // Peak should not exceed 1.0
        let peak = calculate_peak(&samples);
        assert!(peak <= 1.0, "Peak should be <= 1.0, got {}", peak);
    }

    #[test]
    fn test_rms_calculation() {
        // Constant signal: RMS should equal absolute value
        let samples = vec![0.5_f32; 100];
        let rms = calculate_rms(&samples);
        assert!(
            (rms - 0.5).abs() < 0.001,
            "RMS of constant 0.5 should be 0.5, got {}",
            rms
        );

        // Silence
        let silence = vec![0.0_f32; 100];
        assert_eq!(calculate_rms(&silence), 0.0);

        // Empty
        assert_eq!(calculate_rms(&[]), 0.0);
    }

    #[test]
    fn test_peak_calculation() {
        let samples = vec![-0.8, 0.3, 0.5, -0.2, 0.9];
        assert!((calculate_peak(&samples) - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_snr_estimate() {
        // Signal well above noise floor
        let signal: Vec<f32> = vec![0.5; 100];
        let snr = calculate_snr_estimate(&signal, 0.01);
        assert!(snr > 30.0, "SNR should be high: {}", snr);

        // Signal at noise floor
        let quiet: Vec<f32> = vec![0.01; 100];
        let snr_low = calculate_snr_estimate(&quiet, 0.01);
        assert_eq!(snr_low, 0.0, "SNR should be 0 when signal == noise");

        // Invalid noise floor
        assert_eq!(calculate_snr_estimate(&signal, 0.0), 0.0);
    }

    #[test]
    fn test_disabled_preprocessing_is_noop() {
        let config = PreprocessingConfig {
            enabled: false,
            ..Default::default()
        };
        let original = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let mut samples = original.clone();
        preprocess_audio(&mut samples, &config, 16000);
        assert_eq!(
            samples, original,
            "Disabled preprocessing should not modify samples"
        );
    }
}
