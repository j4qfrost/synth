//! Stable data types shared between `synth` (binary) and `synth-dsp` (dylib).
//!
//! Anything whose memory layout the binary needs to know about — every type that
//! flows across the dylib boundary as `&mut` or as a struct field — must live
//! here, NOT in the dylib. Reloading the dylib does not change these definitions.
//!
//! Methods on these types are limited to constructors and atomic getters/setters.
//! Anything you might want to tweak live (oscillator math, envelope curves, voice
//! stealing) belongs in `synth-dsp` as a free function over `&mut Self`.

use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

pub const NUM_VOICES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum OscType {
    Sine = 0,
    Saw = 1,
}

impl OscType {
    pub fn as_u32(self) -> u32 {
        self as u32
    }

    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => OscType::Saw,
            _ => OscType::Sine,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum AudioMessage {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    AllNotesOff,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EnvStage {
    Idle = 0,
    Attack = 1,
    Decay = 2,
    Sustain = 3,
    Release = 4,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Envelope {
    pub stage: EnvStage,
    pub level: f32,
    pub release_start_level: f32,
    pub time_in_stage: f32,
}

impl Envelope {
    pub const fn new() -> Self {
        Self {
            stage: EnvStage::Idle,
            level: 0.0,
            release_start_level: 0.0,
            time_in_stage: 0.0,
        }
    }
}

impl Default for Envelope {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct FilterState {
    pub low: f32,
    pub band: f32,
}

impl FilterState {
    pub const fn new() -> Self {
        Self {
            low: 0.0,
            band: 0.0,
        }
    }
}

impl Default for FilterState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct LfoState {
    pub phase: f32,
}

impl LfoState {
    pub const fn new() -> Self {
        Self { phase: 0.0 }
    }
}

impl Default for LfoState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Voice {
    pub note: u8,
    pub velocity: f32,
    pub phase: f32,
    pub freq: f32,
    pub env: Envelope,
    pub age: u64,
    pub filter: FilterState,
}

impl Voice {
    pub const fn new() -> Self {
        Self {
            note: 0,
            velocity: 0.0,
            phase: 0.0,
            freq: 0.0,
            env: Envelope::new(),
            age: 0,
            filter: FilterState::new(),
        }
    }
}

impl Default for Voice {
    fn default() -> Self {
        Self::new()
    }
}

#[repr(C)]
pub struct VoiceManager {
    pub voices: [Voice; NUM_VOICES],
    pub sample_rate: f32,
    pub next_age: u64,
    pub params: Arc<SharedParams>,
    pub lfo: LfoState,
}

impl VoiceManager {
    pub fn new(sample_rate: f32, params: Arc<SharedParams>) -> Self {
        Self {
            voices: [Voice::new(); NUM_VOICES],
            sample_rate,
            next_age: 1,
            params,
            lfo: LfoState::new(),
        }
    }
}

/// Shared parameters — UI writes, audio thread reads. All values stored as
/// `f32::to_bits()` in `AtomicU32` so reads are wait-free.
pub struct SharedParams {
    attack: AtomicU32,
    decay: AtomicU32,
    sustain: AtomicU32,
    release: AtomicU32,
    master_volume: AtomicU32,
    osc_type: AtomicU32,
    pub midi_connected: AtomicBool,
    cutoff_hz: AtomicU32,
    resonance: AtomicU32,
    lfo_rate_hz: AtomicU32,
    lfo_depth_oct: AtomicU32,
}

impl SharedParams {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            attack: AtomicU32::new(0.01_f32.to_bits()),
            decay: AtomicU32::new(0.1_f32.to_bits()),
            sustain: AtomicU32::new(0.7_f32.to_bits()),
            release: AtomicU32::new(0.3_f32.to_bits()),
            master_volume: AtomicU32::new(0.5_f32.to_bits()),
            osc_type: AtomicU32::new(OscType::Sine.as_u32()),
            midi_connected: AtomicBool::new(false),
            cutoff_hz: AtomicU32::new(8000.0_f32.to_bits()),
            resonance: AtomicU32::new(0.0_f32.to_bits()),
            lfo_rate_hz: AtomicU32::new(1.0_f32.to_bits()),
            lfo_depth_oct: AtomicU32::new(0.0_f32.to_bits()),
        })
    }

    fn get(field: &AtomicU32) -> f32 {
        f32::from_bits(field.load(Ordering::Relaxed))
    }

    fn set(field: &AtomicU32, value: f32) {
        field.store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn attack(&self) -> f32 {
        Self::get(&self.attack)
    }
    pub fn decay(&self) -> f32 {
        Self::get(&self.decay)
    }
    pub fn sustain(&self) -> f32 {
        Self::get(&self.sustain)
    }
    pub fn release(&self) -> f32 {
        Self::get(&self.release)
    }
    pub fn master_volume(&self) -> f32 {
        Self::get(&self.master_volume)
    }
    pub fn osc_type(&self) -> OscType {
        OscType::from_u32(self.osc_type.load(Ordering::Relaxed))
    }
    pub fn cutoff_hz(&self) -> f32 {
        Self::get(&self.cutoff_hz)
    }
    pub fn resonance(&self) -> f32 {
        Self::get(&self.resonance)
    }
    pub fn lfo_rate_hz(&self) -> f32 {
        Self::get(&self.lfo_rate_hz)
    }
    pub fn lfo_depth_oct(&self) -> f32 {
        Self::get(&self.lfo_depth_oct)
    }

    pub fn set_attack(&self, v: f32) {
        Self::set(&self.attack, v);
    }
    pub fn set_decay(&self, v: f32) {
        Self::set(&self.decay, v);
    }
    pub fn set_sustain(&self, v: f32) {
        Self::set(&self.sustain, v);
    }
    pub fn set_release(&self, v: f32) {
        Self::set(&self.release, v);
    }
    pub fn set_master_volume(&self, v: f32) {
        Self::set(&self.master_volume, v);
    }
    pub fn set_osc_type(&self, t: OscType) {
        self.osc_type.store(t.as_u32(), Ordering::Relaxed);
    }
    pub fn set_cutoff_hz(&self, v: f32) {
        Self::set(&self.cutoff_hz, v);
    }
    pub fn set_resonance(&self, v: f32) {
        Self::set(&self.resonance, v);
    }
    pub fn set_lfo_rate_hz(&self, v: f32) {
        Self::set(&self.lfo_rate_hz, v);
    }
    pub fn set_lfo_depth_oct(&self, v: f32) {
        Self::set(&self.lfo_depth_oct, v);
    }
}
