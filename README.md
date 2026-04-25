# synth

Polyphonic Rust synthesizer with **hot-reloadable DSP**. Edit oscillator math, save, hear the change live without losing held notes.

## Architecture

| Crate | Role |
|---|---|
| `synth-state` | Stable data types (`Voice`, `VoiceManager`, `Envelope`, `SharedParams`, `AudioMessage`). Compiled once. |
| `synth-dsp` | `dylib` with the hot-reloadable functions: `process_buffer`, `render_mono`, oscillator math, ADSR, voice stealing. |
| `synth` | Binary. Audio (cpal) + GUI (eframe/egui) + MIDI (midir) + computer-keyboard fallback. Stays alive across reloads. |

The audio callback in `synth/src/audio.rs` makes one `hot::process_buffer()` call per buffer. `hot-lib-reloader` swaps the dylib atomically when the file on disk changes.

## Run

Two terminals:

```bash
# Terminal 1 — auto-rebuild dylib on save
./dev.sh                       # or: cargo watch -w synth-dsp/src -x 'build -p synth-dsp'

# Terminal 2 — run the synth
cargo run -p synth
```

## Play

- Computer keyboard: `A S D F G H J K` = white keys C4–C5
- Black keys: `W E   T Y U`
- Or plug in a MIDI controller — first port is auto-detected

## v1 features

- Sine + saw oscillators (naive saw, aliasing acceptable)
- Per-voice ADSR envelope, all four params live-tweakable
- 8-voice polyphony with oldest-voice stealing
- MIDI note-on/note-off
- Computer-keyboard fallback
- Panic / all-notes-off (also fires on window blur)

## Deferred

Filter, LFO, effects, presets, velocity sensitivity, pitch bend, sustain pedal, band-limited oscillators.
