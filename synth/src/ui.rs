use crate::keyboard::KeyboardState;
use crossbeam_channel::Sender;
use eframe::egui;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use synth_fft::{SpectrumFrame, FFT_SIZES};
use synth_state::{AudioMessage, OscType, SharedParams};

pub struct SynthApp {
    params: Arc<SharedParams>,
    tx: Sender<AudioMessage>,
    keyboard: KeyboardState,
    audio_device: String,
    sample_rate: u32,
    midi_port: Option<String>,
    was_focused: bool,
    reload_count: Arc<AtomicU64>,
    last_seen_reload: u64,
    flash_frames: u32,
    spectrum: Arc<Mutex<SpectrumFrame>>,
    onset_flash: f32,
}

impl SynthApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        params: Arc<SharedParams>,
        tx: Sender<AudioMessage>,
        audio_device: String,
        sample_rate: u32,
        midi_port: Option<String>,
        reload_count: Arc<AtomicU64>,
        spectrum: Arc<Mutex<SpectrumFrame>>,
    ) -> Self {
        Self {
            params,
            tx,
            keyboard: KeyboardState::default(),
            audio_device,
            sample_rate,
            midi_port,
            was_focused: true,
            reload_count,
            last_seen_reload: 0,
            flash_frames: 0,
            spectrum,
            onset_flash: 0.0,
        }
    }

    /// Draw the spectrum section: live FFT controls, the bar visualizer, band
    /// meters, and an onset indicator. Reads the latest frame published by the
    /// analysis thread.
    fn spectrum_ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Spectrum analyzer:");

        // Live FFT controls.
        ui.horizontal(|ui| {
            let mut size = self.params.fft_size();
            egui::ComboBox::from_label("FFT size")
                .selected_text(format!("{size}"))
                .show_ui(ui, |ui| {
                    for &s in FFT_SIZES.iter() {
                        ui.selectable_value(&mut size, s as u32, format!("{s}"));
                    }
                });
            if size != self.params.fft_size() {
                self.params.set_fft_size(size);
            }

            let mut osc_on = self.params.osc_enabled();
            if ui.checkbox(&mut osc_on, "OSC → thea").changed() {
                self.params.set_osc_enabled(osc_on);
            }
        });

        let mut smoothing = self.params.fft_smoothing();
        if ui
            .add(egui::Slider::new(&mut smoothing, 0.0..=0.95).text("Smoothing"))
            .changed()
        {
            self.params.set_fft_smoothing(smoothing);
        }

        // Snapshot the latest frame (clone keeps the lock held only briefly).
        let frame = self.spectrum.lock().map(|f| f.clone()).unwrap_or_default();

        draw_spectrum_bars(ui, &frame.magnitudes);

        // Band meters.
        meter(ui, "Bass", frame.bass, egui::Color32::from_rgb(220, 90, 90));
        meter(ui, "Mid", frame.mid, egui::Color32::from_rgb(220, 200, 90));
        meter(
            ui,
            "Treble",
            frame.treble,
            egui::Color32::from_rgb(120, 200, 220),
        );

        // Onset indicator (decays over a few frames) + peak readout.
        if frame.onset {
            self.onset_flash = 1.0;
        } else {
            self.onset_flash = (self.onset_flash - 0.08).max(0.0);
        }
        ui.horizontal(|ui| {
            let c = egui::Color32::from_rgb(
                (60.0 + 195.0 * self.onset_flash) as u8,
                (60.0 + 100.0 * self.onset_flash) as u8,
                60,
            );
            ui.colored_label(c, "● ONSET");
            ui.label(format!("peak: {:.0} Hz", frame.peak_hz));
        });
    }
}

/// Draw a downsampled magnitude spectrum as vertical bars on an allocated rect.
fn draw_spectrum_bars(ui: &mut egui::Ui, mags: &[f32]) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 110.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(18));
    if mags.is_empty() {
        return;
    }

    let n_bars = 96usize.min(mags.len());
    let group = mags.len() / n_bars;
    let bar_w = rect.width() / n_bars as f32;
    for i in 0..n_bars {
        // Peak-hold within each group of bins so transients stay visible.
        let lo = i * group;
        let hi = (lo + group).min(mags.len());
        let v = mags[lo..hi].iter().cloned().fold(0.0f32, f32::max);
        let h = (v * rect.height()).clamp(0.0, rect.height());
        let x0 = rect.left() + i as f32 * bar_w;
        let bar = egui::Rect::from_min_max(
            egui::pos2(x0 + 0.5, rect.bottom() - h),
            egui::pos2(x0 + bar_w - 0.5, rect.bottom()),
        );
        // Color ramps low→high energy: teal → green → amber.
        let color = egui::Color32::from_rgb(
            (60.0 + 195.0 * v) as u8,
            (120.0 + 100.0 * v) as u8,
            (200.0 - 140.0 * v) as u8,
        );
        painter.rect_filled(bar, 0.0, color);
    }
}

/// A labeled 0..1 horizontal level meter.
fn meter(ui: &mut egui::Ui, label: &str, value: f32, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.add_sized([46.0, 14.0], egui::Label::new(label));
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 14.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, egui::Color32::from_gray(28));
        let w = (value.clamp(0.0, 1.0)) * rect.width();
        let fill =
            egui::Rect::from_min_max(rect.left_top(), egui::pos2(rect.left() + w, rect.bottom()));
        painter.rect_filled(fill, 2.0, color);
    });
}

impl eframe::App for SynthApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Focus-loss safety: kill all notes when window blurs (computer-keyboard
        // releases get dropped by the OS).
        let focused = ctx.input(|i| i.focused);
        if self.was_focused && !focused {
            self.keyboard.release_all(&self.tx);
            let _ = self.tx.try_send(AudioMessage::AllNotesOff);
        }
        self.was_focused = focused;

        if focused {
            self.keyboard.poll(ctx, &self.tx);
        }

        // Detect hot-reload events for UI feedback.
        let current_reload = self.reload_count.load(Ordering::Relaxed);
        if current_reload != self.last_seen_reload {
            self.last_seen_reload = current_reload;
            self.flash_frames = 60; // ~1 second @ 60fps
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Rust Polyphonic Synth");

            ui.separator();

            ui.label(format!(
                "Audio: {} @ {} Hz",
                self.audio_device, self.sample_rate
            ));
            ui.label(match &self.midi_port {
                Some(p) => format!("MIDI: {p}"),
                None => "MIDI: (none — using computer keyboard)".to_string(),
            });

            ui.horizontal(|ui| {
                ui.label(format!("DSP reloads: {current_reload}"));
                if self.flash_frames > 0 {
                    self.flash_frames -= 1;
                    let intensity = (self.flash_frames as f32 / 60.0).clamp(0.0, 1.0);
                    let color = egui::Color32::from_rgb(
                        (80.0 + 175.0 * intensity) as u8,
                        (200.0 + 55.0 * intensity) as u8,
                        80,
                    );
                    ui.colored_label(color, "  RELOADED");
                }
            });

            ui.separator();

            ui.label("Oscillator:");
            ui.horizontal(|ui| {
                let mut osc = self.params.osc_type();
                if ui.radio_value(&mut osc, OscType::Sine, "Sine").clicked()
                    || ui.radio_value(&mut osc, OscType::Saw, "Saw").clicked()
                {
                    self.params.set_osc_type(osc);
                }
            });

            ui.separator();
            ui.label("Envelope (ADSR):");

            let mut a = self.params.attack();
            if ui
                .add(
                    egui::Slider::new(&mut a, 0.001..=2.0)
                        .text("Attack (s)")
                        .logarithmic(true),
                )
                .changed()
            {
                self.params.set_attack(a);
            }

            let mut d = self.params.decay();
            if ui
                .add(
                    egui::Slider::new(&mut d, 0.001..=2.0)
                        .text("Decay (s)")
                        .logarithmic(true),
                )
                .changed()
            {
                self.params.set_decay(d);
            }

            let mut s = self.params.sustain();
            if ui
                .add(egui::Slider::new(&mut s, 0.0..=1.0).text("Sustain"))
                .changed()
            {
                self.params.set_sustain(s);
            }

            let mut r = self.params.release();
            if ui
                .add(
                    egui::Slider::new(&mut r, 0.001..=4.0)
                        .text("Release (s)")
                        .logarithmic(true),
                )
                .changed()
            {
                self.params.set_release(r);
            }

            ui.separator();
            ui.label("Filter:");

            let mut cutoff = self.params.cutoff_hz();
            if ui
                .add(
                    egui::Slider::new(&mut cutoff, 20.0..=20000.0)
                        .text("Cutoff (Hz)")
                        .logarithmic(true),
                )
                .changed()
            {
                self.params.set_cutoff_hz(cutoff);
            }

            let mut res = self.params.resonance();
            if ui
                .add(egui::Slider::new(&mut res, 0.0..=0.99).text("Resonance"))
                .changed()
            {
                self.params.set_resonance(res);
            }

            ui.separator();
            ui.label("LFO:");

            let mut lfo_rate = self.params.lfo_rate_hz();
            if ui
                .add(
                    egui::Slider::new(&mut lfo_rate, 0.05..=20.0)
                        .text("LFO Rate (Hz)")
                        .logarithmic(true),
                )
                .changed()
            {
                self.params.set_lfo_rate_hz(lfo_rate);
            }

            let mut lfo_depth = self.params.lfo_depth_oct();
            if ui
                .add(egui::Slider::new(&mut lfo_depth, 0.0..=4.0).text("LFO Depth (oct)"))
                .changed()
            {
                self.params.set_lfo_depth_oct(lfo_depth);
            }

            ui.separator();

            let mut mv = self.params.master_volume();
            if ui
                .add(egui::Slider::new(&mut mv, 0.0..=1.0).text("Master volume"))
                .changed()
            {
                self.params.set_master_volume(mv);
            }

            ui.separator();
            self.spectrum_ui(ui);

            ui.separator();

            if ui.button("Panic — All Notes Off").clicked() {
                self.keyboard.release_all(&self.tx);
                let _ = self.tx.try_send(AudioMessage::AllNotesOff);
            }

            ui.separator();
            ui.label("Computer keyboard: A S D F G H J K = white keys C4–C5");
            ui.label("Black keys:               W E   T Y U");

            let held: Vec<u8> = self.keyboard.held_notes().collect();
            if !held.is_empty() {
                ui.label(format!("Held: {held:?}"));
            }
        });

        ctx.request_repaint();
    }
}
