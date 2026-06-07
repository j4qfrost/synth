# synth

Polyphonic Rust synthesizer with **hot-reloadable DSP**. Edit oscillator math, save, hear the change live without losing held notes.

## Architecture

| Crate | Role |
|---|---|
| `synth-state` | Stable data types (`Voice`, `VoiceManager`, `Envelope`, `SharedParams`, `AudioMessage`). Compiled once. |
| `synth-dsp` | `dylib` with the hot-reloadable functions: `process_buffer`, `render_mono`, oscillator math, ADSR, voice stealing. |
| `synth-fft` | Stable spectrum analyzer (`rustfft`): Hann-windowed FFT → magnitudes, bass/mid/treble energy, spectral-flux onset, peak freq. Compiled once — kept out of the hot-reload dylib so a reload can't corrupt FFT state. |
| `synth` | Binary. Audio (cpal) + GUI (eframe/egui) + MIDI (midir) + computer-keyboard fallback + spectrum analysis thread + OSC out. Stays alive across reloads. |

The audio callback in `synth/src/audio.rs` makes one `hot::process_buffer()` call per buffer. `hot-lib-reloader` swaps the dylib atomically when the file on disk changes.

### Spectrum signal path

The audio callback never runs the FFT. After rendering each buffer it copies the
output into a lock-free SPSC ring (`ringbuf`); a dedicated **analysis thread**
(`synth/src/analysis.rs`) drains the ring, runs `synth-fft`, and publishes a
`SpectrumFrame` that the GUI draws and that `synth/src/osc_out.rs` ships to
[`thea`](https://github.com/j4qfrost/thea) over OSC. Nothing allocates or blocks
on the realtime thread — it only ever does a wait-free ring write.

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

## Spectrum analyzer

Real-time FFT spectrum analysis with a live bar visualizer, bass/mid/treble
meters, and onset detection. All on a dedicated thread off the audio path.

- **FFT size** — 512 / 1024 / 2048 / 4096, switchable live.
- **Smoothing** — temporal EMA on the displayed magnitudes (`0.0`–`0.95`).
- **OSC → thea** — toggle streaming of bands + onsets to a lighting rig.

### OSC namespace (synth → thea)

Fire-and-forget UDP. Target defaults to `127.0.0.1:9000`; override with the
`SYNTH_OSC_ADDR` env var (e.g. `SYNTH_OSC_ADDR=snowman:9000 cargo run -p synth`).
This is the wire contract `thea`'s audio-reactive cue layer consumes.

| Address                 | Args    | Meaning                            |
|-------------------------|---------|------------------------------------|
| `/synth/spectrum/bands` | `f f f` | bass, mid, treble (each `0.0–1.0`) |
| `/synth/onset`          | `f i`   | spectral flux (`≥0`), onset (`0|1`)|
| `/synth/spectrum/peak`  | `f`     | peak frequency (Hz)                |

## Deferred

Effects, presets, velocity sensitivity, pitch bend, sustain pedal, band-limited
oscillators. (Filter + LFO shipped since v1.)
