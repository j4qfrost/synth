use crate::keyboard::KeyboardState;
use crossbeam_channel::Sender;
use eframe::egui;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
}

impl SynthApp {
    pub fn new(
        params: Arc<SharedParams>,
        tx: Sender<AudioMessage>,
        audio_device: String,
        sample_rate: u32,
        midi_port: Option<String>,
        reload_count: Arc<AtomicU64>,
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
        }
    }
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
                .add(egui::Slider::new(&mut a, 0.001..=2.0).text("Attack (s)").logarithmic(true))
                .changed()
            {
                self.params.set_attack(a);
            }

            let mut d = self.params.decay();
            if ui
                .add(egui::Slider::new(&mut d, 0.001..=2.0).text("Decay (s)").logarithmic(true))
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
                .add(egui::Slider::new(&mut r, 0.001..=4.0).text("Release (s)").logarithmic(true))
                .changed()
            {
                self.params.set_release(r);
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
