//! Real-time spectrum analysis for the synth.
//!
//! This is a **stable** crate (compiled once, like `synth-state`) — it is *not*
//! hot-reloaded. The FFT lives here, isolated from the `synth-dsp` dylib, so a
//! reload can never corrupt FFT plan state or the realtime audio buffer.
//!
//! The audio thread never calls into here. Instead the synth binary copies each
//! rendered buffer into a lock-free ring; a dedicated analysis thread owns an
//! [`Analyzer`], pulls a window of samples, and produces a [`SpectrumFrame`] for
//! the GUI and for OSC output to `thea`.
//!
//! Pipeline per frame: Hann window → real FFT (`rustfft`) → magnitude spectrum
//! (coherent-gain normalized) → per-band energy (bass/mid/treble) → spectral-flux
//! onset → peak frequency. Magnitudes are temporally smoothed (EMA) and returned
//! in normalized dB (`0.0..=1.0`) so the GUI can draw them and OSC can ship them
//! without further scaling.

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

/// Band crossover frequencies (Hz). Bass `[20, 250)`, mid `[250, 4000)`,
/// treble `[4000, 20000)`.
pub const BASS_LO: f32 = 20.0;
pub const BASS_HI: f32 = 250.0;
pub const MID_HI: f32 = 4000.0;
pub const TREBLE_HI: f32 = 20_000.0;

/// dB floor used to map magnitudes into `0.0..=1.0`. Anything at or below this is
/// `0.0`; `0 dB` (full-scale) maps to `1.0`.
pub const DB_FLOOR: f32 = -90.0;

/// Valid FFT sizes the UI can select. All powers of two.
pub const FFT_SIZES: [usize; 4] = [512, 1024, 2048, 4096];

/// Default FFT size — a good balance of frequency resolution and latency.
pub const DEFAULT_FFT_SIZE: usize = 2048;

/// One analysis result. Cheap to clone (a few `Vec`s + scalars); the analysis
/// thread replaces the shared copy, the GUI clones it per repaint.
#[derive(Clone, Debug)]
pub struct SpectrumFrame {
    /// Normalized magnitude per FFT bin (`0.0..=1.0`, dB-mapped, smoothed).
    /// Length is `fft_size / 2`. Bin `k` is centered at `k * sample_rate / fft_size` Hz.
    pub magnitudes: Vec<f32>,
    /// Bass-band energy, normalized `0.0..=1.0` (dB-mapped).
    pub bass: f32,
    /// Mid-band energy, normalized `0.0..=1.0` (dB-mapped).
    pub mid: f32,
    /// Treble-band energy, normalized `0.0..=1.0` (dB-mapped).
    pub treble: f32,
    /// Spectral flux this frame — raw onset strength (`>= 0.0`).
    pub flux: f32,
    /// `true` when `flux` exceeds the adaptive threshold (an onset fired).
    pub onset: bool,
    /// Frequency (Hz) of the loudest bin.
    pub peak_hz: f32,
    /// Sample rate the frame was computed at.
    pub sample_rate: f32,
    /// FFT size the frame was computed at.
    pub fft_size: usize,
}

impl Default for SpectrumFrame {
    fn default() -> Self {
        Self {
            magnitudes: Vec::new(),
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            flux: 0.0,
            onset: false,
            peak_hz: 0.0,
            sample_rate: 0.0,
            fft_size: 0,
        }
    }
}

/// Reusable spectrum analyzer. Stateful across calls (it keeps the previous
/// magnitude spectrum for smoothing + flux), but the input is always an explicit
/// window slice, which keeps it deterministic and unit-testable.
pub struct Analyzer {
    fft_size: usize,
    sample_rate: f32,
    /// Temporal smoothing factor for the displayed magnitudes (EMA). `0.0` = no
    /// smoothing (raw), approaching `1.0` = very sticky.
    smoothing: f32,

    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    window_gain: f32,
    buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,

    /// Linear (pre-dB) magnitude spectrum of the previous frame — for flux.
    prev_lin: Vec<f32>,
    /// Smoothed normalized magnitudes carried across frames.
    smoothed: Vec<f32>,
    /// EMA of recent flux, the adaptive onset threshold baseline.
    flux_avg: f32,
}

impl Analyzer {
    /// Build an analyzer for `fft_size` (rounded to the nearest valid power of
    /// two) at `sample_rate`, with temporal `smoothing` in `0.0..=1.0`.
    pub fn new(fft_size: usize, sample_rate: f32, smoothing: f32) -> Self {
        let fft_size = sanitize_fft_size(fft_size);
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let window = hann(fft_size);
        let window_gain: f32 = window.iter().sum::<f32>().max(1e-9);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        let half = fft_size / 2;
        Self {
            fft_size,
            sample_rate,
            smoothing: smoothing.clamp(0.0, 0.99),
            fft,
            window,
            window_gain,
            buffer: vec![Complex::new(0.0, 0.0); fft_size],
            scratch,
            prev_lin: vec![0.0; half],
            smoothed: vec![0.0; half],
            flux_avg: 0.0,
        }
    }

    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Update the temporal smoothing factor live (UI slider).
    pub fn set_smoothing(&mut self, smoothing: f32) {
        self.smoothing = smoothing.clamp(0.0, 0.99);
    }

    /// Rebuild the FFT plan + buffers for a new size, resetting smoothing/flux
    /// history. No-op if the (sanitized) size is unchanged.
    pub fn set_fft_size(&mut self, fft_size: usize) {
        let fft_size = sanitize_fft_size(fft_size);
        if fft_size == self.fft_size {
            return;
        }
        let mut planner = FftPlanner::<f32>::new();
        self.fft = planner.plan_fft_forward(fft_size);
        self.window = hann(fft_size);
        self.window_gain = self.window.iter().sum::<f32>().max(1e-9);
        self.buffer = vec![Complex::new(0.0, 0.0); fft_size];
        self.scratch = vec![Complex::new(0.0, 0.0); self.fft.get_inplace_scratch_len()];
        let half = fft_size / 2;
        self.prev_lin = vec![0.0; half];
        self.smoothed = vec![0.0; half];
        self.flux_avg = 0.0;
        self.fft_size = fft_size;
    }

    /// Analyze one window. `window` must hold exactly `fft_size` time-domain
    /// samples (newest last); shorter/longer input is zero-padded or truncated.
    pub fn analyze(&mut self, window: &[f32]) -> SpectrumFrame {
        let n = self.fft_size;
        let half = n / 2;

        // Window the input into the complex FFT buffer.
        for i in 0..n {
            let s = window.get(i).copied().unwrap_or(0.0);
            self.buffer[i] = Complex::new(s * self.window[i], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.buffer, &mut self.scratch);

        // Coherent-gain-normalized linear magnitudes. The factor of 2 accounts
        // for the energy in the discarded negative-frequency half.
        let norm = 2.0 / self.window_gain;
        let mut lin = vec![0.0f32; half];
        for (m, c) in lin.iter_mut().zip(self.buffer.iter()) {
            *m = c.norm() * norm;
        }

        // Peak bin → frequency.
        let mut peak_k = 0usize;
        let mut peak_v = 0.0f32;
        for (k, &m) in lin.iter().enumerate() {
            if m > peak_v {
                peak_v = m;
                peak_k = k;
            }
        }
        let peak_hz = peak_k as f32 * self.sample_rate / n as f32;

        // Spectral flux: sum of positive bin-to-bin increases vs the last frame.
        let mut flux = 0.0f32;
        for (cur, prev) in lin.iter().zip(self.prev_lin.iter()) {
            let d = cur - prev;
            if d > 0.0 {
                flux += d;
            }
        }
        self.prev_lin.copy_from_slice(&lin);

        // Adaptive onset: fire when flux clearly exceeds its running average.
        let onset = flux > self.flux_avg * 1.5 + 1e-4;
        self.flux_avg = 0.9 * self.flux_avg + 0.1 * flux;

        // Per-band energy (RMS over linear bins in range), then dB-normalized.
        let bass = self.band_energy(&lin, BASS_LO, BASS_HI);
        let mid = self.band_energy(&lin, BASS_HI, MID_HI);
        let treble = self.band_energy(&lin, MID_HI, TREBLE_HI);

        // dB-map + temporal smoothing of the displayed magnitudes.
        let a = self.smoothing;
        let mut mags = vec![0.0f32; half];
        for k in 0..half {
            let target = lin_to_norm_db(lin[k]);
            self.smoothed[k] = a * self.smoothed[k] + (1.0 - a) * target;
            mags[k] = self.smoothed[k];
        }

        SpectrumFrame {
            magnitudes: mags,
            bass,
            mid,
            treble,
            flux,
            onset,
            peak_hz,
            sample_rate: self.sample_rate,
            fft_size: n,
        }
    }

    fn band_energy(&self, lin: &[f32], lo_hz: f32, hi_hz: f32) -> f32 {
        let n = self.fft_size as f32;
        let lo = (lo_hz * n / self.sample_rate).floor().max(0.0) as usize;
        let hi = ((hi_hz * n / self.sample_rate).ceil() as usize).min(lin.len());
        if hi <= lo {
            return 0.0;
        }
        let mut sum_sq = 0.0f32;
        for &m in &lin[lo..hi] {
            sum_sq += m * m;
        }
        let rms = (sum_sq / (hi - lo) as f32).sqrt();
        lin_to_norm_db(rms)
    }
}

/// Round an arbitrary size to the nearest supported FFT size.
pub fn sanitize_fft_size(n: usize) -> usize {
    *FFT_SIZES
        .iter()
        .min_by_key(|&&s| (s as isize - n as isize).unsigned_abs())
        .unwrap_or(&DEFAULT_FFT_SIZE)
}

/// Periodic Hann window of length `n`.
fn hann(n: usize) -> Vec<f32> {
    if n <= 1 {
        return vec![1.0; n.max(1)];
    }
    (0..n)
        .map(|i| {
            let x = std::f32::consts::PI * i as f32 / n as f32;
            let s = x.sin();
            s * s // sin^2(pi i / n) == 0.5 - 0.5 cos(2 pi i / n)
        })
        .collect()
}

/// Map a linear amplitude (`~0.0..=1.0` for full-scale) to normalized dB in
/// `0.0..=1.0` against [`DB_FLOOR`].
fn lin_to_norm_db(lin: f32) -> f32 {
    if lin <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * lin.log10();
    ((db - DB_FLOOR) / -DB_FLOOR).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (TAU * freq * i as f32 / SR).sin())
            .collect()
    }

    #[test]
    fn sanitize_rounds_to_supported_sizes() {
        assert_eq!(sanitize_fft_size(2000), 2048);
        assert_eq!(sanitize_fft_size(700), 512);
        assert_eq!(sanitize_fft_size(900), 1024);
        assert_eq!(sanitize_fft_size(100_000), 4096);
    }

    #[test]
    fn peak_lands_on_the_input_frequency() {
        let n = 4096;
        let mut a = Analyzer::new(n, SR, 0.0);
        let freq = 1000.0;
        let frame = a.analyze(&sine(freq, n, 1.0));
        let bin_hz = SR / n as f32;
        // Peak frequency within one bin of the true frequency.
        assert!(
            (frame.peak_hz - freq).abs() <= bin_hz,
            "peak_hz={} expected~{} (bin={})",
            frame.peak_hz,
            freq,
            bin_hz
        );
        // Full-scale sine ⇒ peak magnitude near the top of the 0..1 range.
        let peak = frame.magnitudes.iter().cloned().fold(0.0f32, f32::max);
        assert!(peak > 0.9, "peak magnitude {peak} too low");
    }

    #[test]
    fn energy_routes_to_the_correct_band() {
        let n = 4096;
        let mut a = Analyzer::new(n, SR, 0.0);

        let low = a.analyze(&sine(100.0, n, 1.0));
        assert!(low.bass > low.mid && low.bass > low.treble, "{low:?}");

        let mut a = Analyzer::new(n, SR, 0.0);
        let midf = a.analyze(&sine(1000.0, n, 1.0));
        assert!(midf.mid > midf.bass && midf.mid > midf.treble, "{midf:?}");

        let mut a = Analyzer::new(n, SR, 0.0);
        let hi = a.analyze(&sine(10_000.0, n, 1.0));
        assert!(hi.treble > hi.bass && hi.treble > hi.mid, "{hi:?}");
    }

    #[test]
    fn onset_fires_on_a_transient_not_on_steady_state() {
        let n = 2048;
        let mut a = Analyzer::new(n, SR, 0.0);
        // Prime with silence so the flux baseline is ~0.
        for _ in 0..4 {
            let _ = a.analyze(&vec![0.0; n]);
        }
        // A sudden tone burst should trip an onset.
        let burst = a.analyze(&sine(440.0, n, 1.0));
        assert!(
            burst.onset,
            "expected onset on transient, flux={}",
            burst.flux
        );
        // Holding the same tone should settle (flux drops, no repeated onset).
        let _ = a.analyze(&sine(440.0, n, 1.0));
        let steady = a.analyze(&sine(440.0, n, 1.0));
        assert!(!steady.onset, "steady tone should not keep firing onsets");
    }

    #[test]
    fn set_fft_size_rebuilds_and_changes_resolution() {
        let mut a = Analyzer::new(512, SR, 0.0);
        assert_eq!(a.fft_size(), 512);
        a.set_fft_size(2048);
        assert_eq!(a.fft_size(), 2048);
        let frame = a.analyze(&sine(1000.0, 2048, 1.0));
        assert_eq!(frame.magnitudes.len(), 1024);
    }

    #[test]
    fn silence_is_silent() {
        let n = 1024;
        let mut a = Analyzer::new(n, SR, 0.0);
        let frame = a.analyze(&vec![0.0; n]);
        assert_eq!(frame.bass, 0.0);
        assert_eq!(frame.mid, 0.0);
        assert_eq!(frame.treble, 0.0);
        assert!(frame.magnitudes.iter().all(|&m| m == 0.0));
    }
}
