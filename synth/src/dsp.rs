//! Hot-loaded proxy to `synth-dsp`.
//!
//! `hot_lib_reloader::hot_module` parses `synth-dsp/src/lib.rs` at compile time
//! and generates wrappers for each `#[no_mangle] pub fn` it finds. Each wrapper
//! takes a brief read-lock, calls into the currently-loaded dylib, and returns.
//!
//! When `cargo build -p synth-dsp` produces a fresh dylib on disk, the reloader
//! atomically swaps to the new one — held audio state (notes, phases, envelopes)
//! is preserved because state lives in `synth-state` types owned by the binary.

#[hot_lib_reloader::hot_module(dylib = "synth_dsp")]
pub mod hot {
    use crossbeam_channel::Receiver;
    use synth_state::{AudioMessage, VoiceManager};

    hot_functions_from_file!("synth-dsp/src/lib.rs");

    #[lib_change_subscription]
    pub fn subscribe() -> hot_lib_reloader::LibReloadObserver {}
}
