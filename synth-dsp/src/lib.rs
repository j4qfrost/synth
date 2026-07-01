//! Hot-reloadable DSP layer.
//!
//! Edit this file, save, and (with `cargo watch -p synth-dsp` running) the binary
//! will pick up the new code without losing audio, MIDI connection, or held notes.
//!
//! Rules for code in this file:
//! - Only `#[no_mangle] pub fn` items are reachable from the binary's hot-loader.
//!   Helpers can be plain `fn` and they will be re-linked together.
//! - All state lives in `synth-state` types passed in by `&mut`. Do NOT define
//!   new struct fields here that the binary needs to see — the layout would change
//!   between reloads.
//! - The audio thread calls into here; no allocation, no syscalls, no logging.

use crossbeam_channel::Receiver;
use std::f32::consts::{PI, TAU};
use synth_state::{
    AudioMessage, EnvStage, Envelope, FilterState, OscType, Voice, VoiceManager, NUM_VOICES,
};

// === Public hot-reloadable entry points ====================================

/// Drain pending control messages and render `out.len()` mono samples.
/// One call per audio buffer — keeps cross-dylib calls to a minimum.
#[no_mangle]
pub fn process_buffer(vm: &mut VoiceManager, out: &mut [f32], rx: &Receiver<AudioMessage>) {
    while let Ok(msg) = rx.try_recv() {
        handle_message(vm, msg);
    }
    render_mono(vm, out);
}

/// Apply one control message to the voice manager.
#[no_mangle]
pub fn handle_message(vm: &mut VoiceManager, msg: AudioMessage) {
    match msg {
        AudioMessage::NoteOn { note, velocity } => note_on(vm, note, velocity),
        AudioMessage::NoteOff { note } => note_off(vm, note),
        AudioMessage::AllNotesOff => {
            for v in vm.voices.iter_mut() {
                env_note_off(&mut v.env);
            }
        }
    }
}

/// Render `out.len()` mono samples into `out`.
#[no_mangle]
pub fn render_mono(vm: &mut VoiceManager, out: &mut [f32]) {
    let dt = 1.0 / vm.sample_rate;
    let attack = vm.params.attack();
    let decay = vm.params.decay();
    let sustain = vm.params.sustain();
    let release = vm.params.release();
    let master = vm.params.master_volume();
    let osc = vm.params.osc_type();
    let base_cutoff = vm.params.cutoff_hz();
    let resonance = vm.params.resonance();
    let lfo_rate = vm.params.lfo_rate_hz();
    let lfo_depth = vm.params.lfo_depth_oct();
    let sample_rate = vm.sample_rate;

    for sample in out.iter_mut() {
        // Advance LFO once per sample.
        vm.lfo.phase += lfo_rate / sample_rate;
        if vm.lfo.phase >= 1.0 {
            vm.lfo.phase -= 1.0;
        }
        let lfo_value = 1.0 - 4.0 * (vm.lfo.phase - 0.5).abs(); // triangle -1..1
        let effective_cutoff =
            (base_cutoff * 2_f32.powf(lfo_value * lfo_depth)).clamp(20.0, 20_000.0);

        let mut acc = 0.0;
        for v in vm.voices.iter_mut() {
            acc += render_voice_sample(
                v,
                dt,
                osc,
                attack,
                decay,
                sustain,
                release,
                effective_cutoff,
                resonance,
                sample_rate,
            );
        }
        *sample = (acc / NUM_VOICES as f32) * master;
    }
}

// === Internal helpers (re-linked on reload) ================================

pub fn midi_to_freq(note: u8) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

fn note_on(vm: &mut VoiceManager, note: u8, velocity: u8) {
    let age = vm.next_age;
    vm.next_age = vm.next_age.wrapping_add(1);

    // Prefer an idle voice; otherwise steal the oldest active one.
    let mut chosen = 0;
    let mut best_age = u64::MAX;
    for (i, v) in vm.voices.iter().enumerate() {
        if v.env.stage == EnvStage::Idle {
            chosen = i;
            best_age = 0;
            break;
        }
        if v.age < best_age {
            best_age = v.age;
            chosen = i;
        }
    }
    let _ = best_age;

    let v = &mut vm.voices[chosen];
    v.note = note;
    v.velocity = (velocity as f32 / 127.0).clamp(0.0, 1.0);
    v.freq = midi_to_freq(note);
    v.phase = 0.0;
    v.age = age;
    env_note_on(&mut v.env);
}

fn note_off(vm: &mut VoiceManager, note: u8) {
    for v in vm.voices.iter_mut() {
        if v.note == note && v.env.stage != EnvStage::Idle && v.env.stage != EnvStage::Release {
            env_note_off(&mut v.env);
        }
    }
}

fn env_note_on(env: &mut Envelope) {
    env.stage = EnvStage::Attack;
    env.time_in_stage = 0.0;
}

fn env_note_off(env: &mut Envelope) {
    env.release_start_level = env.level;
    env.stage = EnvStage::Release;
    env.time_in_stage = 0.0;
}

fn env_tick(
    env: &mut Envelope,
    dt: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
) -> f32 {
    match env.stage {
        EnvStage::Idle => {
            env.level = 0.0;
        }
        EnvStage::Attack => {
            let a = attack.max(1e-4);
            env.level = (env.time_in_stage / a).min(1.0);
            env.time_in_stage += dt;
            if env.time_in_stage >= a {
                env.stage = EnvStage::Decay;
                env.time_in_stage = 0.0;
                env.level = 1.0;
            }
        }
        EnvStage::Decay => {
            let d = decay.max(1e-4);
            let t = (env.time_in_stage / d).min(1.0);
            env.level = 1.0 + (sustain - 1.0) * t;
            env.time_in_stage += dt;
            if env.time_in_stage >= d {
                env.stage = EnvStage::Sustain;
                env.level = sustain;
            }
        }
        EnvStage::Sustain => {
            env.level = sustain;
        }
        EnvStage::Release => {
            let r = release.max(1e-4);
            let t = (env.time_in_stage / r).min(1.0);
            env.level = env.release_start_level * (1.0 - t);
            env.time_in_stage += dt;
            if env.time_in_stage >= r {
                env.stage = EnvStage::Idle;
                env.level = 0.0;
            }
        }
    }
    env.level
}

fn svf_lowpass(
    state: &mut FilterState,
    input: f32,
    cutoff_hz: f32,
    resonance: f32,
    sample_rate: f32,
) -> f32 {
    let f = 2.0 * (PI * cutoff_hz / sample_rate).sin();
    let q = 1.0 - resonance.clamp(0.0, 0.999);
    state.low += f * state.band;
    let high = input - state.low - q * state.band;
    state.band += f * high;
    state.low
}

#[allow(clippy::too_many_arguments)]
fn render_voice_sample(
    v: &mut Voice,
    dt: f32,
    osc: OscType,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff_hz: f32,
    resonance: f32,
    sample_rate: f32,
) -> f32 {
    if v.env.stage == EnvStage::Idle {
        return 0.0;
    }
    let raw = match osc {
        OscType::Sine => (v.phase * TAU).sin(),
        OscType::Saw => 2.0 * v.phase - 1.0,
    };
    v.phase += v.freq * dt;
    if v.phase >= 1.0 {
        v.phase -= v.phase.floor();
    }
    let filtered = svf_lowpass(&mut v.filter, raw, cutoff_hz, resonance, sample_rate);
    let env = env_tick(&mut v.env, dt, attack, decay, sustain, release);
    filtered * env * v.velocity
}
