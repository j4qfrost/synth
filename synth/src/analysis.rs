//! Spectrum analysis thread.
//!
//! Owns the consumer end of the audio tap plus a [`synth_fft::Analyzer`]. Each
//! tick it drains the newest samples, runs the FFT, publishes the result into a
//! shared [`SpectrumFrame`] for the GUI, and ships bands/onsets over OSC to
//! `thea`.
//!
//! This runs OFF the realtime audio thread, so allocation and the FFT itself are
//! fine here — the audio callback only ever does a wait-free ring write.

use crate::osc_out::OscOut;
use ringbuf::traits::Consumer;
use ringbuf::HeapCons;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use synth_fft::{sanitize_fft_size, Analyzer, SpectrumFrame};
use synth_state::SharedParams;

/// How often the analysis loop wakes to drain the ring. ~8 ms (≈125 Hz upper
/// bound); the real spectrum update rate is gated by how fast audio arrives.
const TICK: Duration = Duration::from_millis(8);

/// Spawn the analysis thread. Returns its join handle; the thread loops for the
/// life of the process.
pub fn start(
    mut consumer: HeapCons<f32>,
    params: Arc<SharedParams>,
    sample_rate: f32,
    shared: Arc<Mutex<SpectrumFrame>>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("spectrum-analysis".into())
        .spawn(move || {
            let mut size = sanitize_fft_size(params.fft_size() as usize);
            params.set_fft_size(size as u32);
            let mut analyzer = Analyzer::new(size, sample_rate, params.fft_smoothing());
            let mut window = vec![0.0f32; size];
            // Drain buffer sized to the largest supported FFT.
            let mut drain = vec![0.0f32; 4096];
            let osc = OscOut::from_env();

            loop {
                // Honor a live FFT-size change from the UI.
                let want = sanitize_fft_size(params.fft_size() as usize);
                if want != size {
                    size = want;
                    analyzer.set_fft_size(size);
                    window = vec![0.0f32; size];
                }
                analyzer.set_smoothing(params.fft_smoothing());

                let n = consumer.pop_slice(&mut drain);
                if n == 0 {
                    thread::sleep(TICK);
                    continue;
                }
                slide_window(&mut window, &drain[..n]);

                let frame = analyzer.analyze(&window);
                if params.osc_enabled() {
                    if let Some(osc) = &osc {
                        osc.send(&frame);
                    }
                }
                if let Ok(mut guard) = shared.lock() {
                    *guard = frame;
                }
                thread::sleep(TICK);
            }
        })
        .expect("spawn spectrum-analysis thread")
}

/// Append `input` to the sliding `window`, keeping the newest `window.len()`
/// samples (newest last).
fn slide_window(window: &mut [f32], input: &[f32]) {
    let size = window.len();
    let n = input.len();
    if n >= size {
        window.copy_from_slice(&input[n - size..]);
    } else {
        window.copy_within(n.., 0);
        window[size - n..].copy_from_slice(input);
    }
}

#[cfg(test)]
mod tests {
    use super::slide_window;

    #[test]
    fn slide_keeps_newest_samples() {
        let mut w = vec![0.0; 4];
        slide_window(&mut w, &[1.0, 2.0]);
        assert_eq!(w, vec![0.0, 0.0, 1.0, 2.0]);
        slide_window(&mut w, &[3.0, 4.0]);
        assert_eq!(w, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn slide_truncates_oversized_input() {
        let mut w = vec![0.0; 3];
        slide_window(&mut w, &[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(w, vec![3.0, 4.0, 5.0]);
    }
}
