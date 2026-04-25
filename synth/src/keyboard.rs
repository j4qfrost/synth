use synth_state::AudioMessage;
use crossbeam_channel::Sender;
use egui::Key;
use std::collections::HashSet;

/// Maps the QWERTY keyboard to a one-octave-plus piano layout starting at C4 (MIDI 60).
/// White keys: A S D F G H J K = C D E F G A B C
/// Black keys: W E   T Y U     = C# D#  F# G# A#
fn key_to_note(key: Key) -> Option<u8> {
    Some(match key {
        Key::A => 60, // C4
        Key::W => 61, // C#4
        Key::S => 62, // D4
        Key::E => 63, // D#4
        Key::D => 64, // E4
        Key::F => 65, // F4
        Key::T => 66, // F#4
        Key::G => 67, // G4
        Key::Y => 68, // G#4
        Key::H => 69, // A4
        Key::U => 70, // A#4
        Key::J => 71, // B4
        Key::K => 72, // C5
        _ => return None,
    })
}

#[derive(Default)]
pub struct KeyboardState {
    held: HashSet<u8>,
}

impl KeyboardState {
    pub fn poll(&mut self, ctx: &egui::Context, tx: &Sender<AudioMessage>) {
        // Snapshot input events; egui will not fire repeated key-down events for a held
        // key (good — avoids retriggers).
        ctx.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        ..
                    } => {
                        if let Some(note) = key_to_note(*key) {
                            if self.held.insert(note) {
                                let _ = tx.try_send(AudioMessage::NoteOn { note, velocity: 100 });
                            }
                        }
                    }
                    egui::Event::Key {
                        key,
                        pressed: false,
                        ..
                    } => {
                        if let Some(note) = key_to_note(*key) {
                            if self.held.remove(&note) {
                                let _ = tx.try_send(AudioMessage::NoteOff { note });
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    /// Release every held note. Call when the window loses focus or user hits Panic.
    pub fn release_all(&mut self, tx: &Sender<AudioMessage>) {
        for note in self.held.drain() {
            let _ = tx.try_send(AudioMessage::NoteOff { note });
        }
    }

    pub fn held_notes(&self) -> impl Iterator<Item = u8> + '_ {
        self.held.iter().copied()
    }
}
