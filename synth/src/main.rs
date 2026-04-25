mod audio;
mod dsp;
mod keyboard;
mod midi;
mod ui;

use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use synth_state::SharedParams;
use ui::SynthApp;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let (tx, rx) = crossbeam_channel::bounded(1024);
    let params = SharedParams::new();

    let audio_handle = audio::start_audio(rx, params.clone())?;
    let midi_handle = midi::start_midi(tx.clone(), params.clone())?;

    // Watcher thread: bumps a counter every time the DSP dylib is hot-swapped.
    // The UI polls the counter to flash a "RELOADED" indicator.
    let reload_count = Arc::new(AtomicU64::new(0));
    {
        let count = reload_count.clone();
        let observer = dsp::hot::subscribe();
        std::thread::spawn(move || loop {
            let token = observer.wait_for_about_to_reload();
            log::info!("DSP dylib changed — reloading");
            drop(token);
            observer.wait_for_reload();
            count.fetch_add(1, Ordering::Relaxed);
            log::info!("DSP reloaded");
        });
    }

    let device_name = audio_handle.device_name.clone();
    let sample_rate = audio_handle.sample_rate;
    let midi_port = midi_handle.port_name.clone();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 580.0])
            .with_title("Rust Synth (hot-reload)"),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Rust Synth",
        native_options,
        Box::new(move |_cc| {
            Box::new(AppHolder {
                inner: SynthApp::new(
                    params,
                    tx,
                    device_name,
                    sample_rate,
                    midi_port,
                    reload_count,
                ),
                _audio: audio_handle,
                _midi: midi_handle,
            })
        }),
    );

    Ok(())
}

struct AppHolder {
    inner: SynthApp,
    _audio: audio::AudioHandle,
    _midi: midi::MidiHandle,
}

impl eframe::App for AppHolder {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.inner.update(ctx, frame);
    }
}
