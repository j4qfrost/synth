use anyhow::Result;
use crossbeam_channel::Sender;
use midir::{Ignore, MidiInput, MidiInputConnection};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use synth_state::{AudioMessage, SharedParams};

pub struct MidiHandle {
    pub port_name: Option<String>,
    _conn: Option<MidiInputConnection<()>>,
}

pub fn start_midi(tx: Sender<AudioMessage>, params: Arc<SharedParams>) -> Result<MidiHandle> {
    let mut input = MidiInput::new("synth-midi-in")?;
    input.ignore(Ignore::None);

    let ports = input.ports();
    if ports.is_empty() {
        log::info!("no MIDI input ports found; using computer keyboard only");
        params.midi_connected.store(false, Ordering::Relaxed);
        return Ok(MidiHandle {
            port_name: None,
            _conn: None,
        });
    }

    let port = &ports[0];
    let port_name = input.port_name(port).unwrap_or_else(|_| "<unknown>".into());
    log::info!("opening MIDI input: {port_name}");

    let conn = input.connect(
        port,
        "synth-midi-conn",
        move |_timestamp, message, _| {
            // MIDI status nibble: 0x90 = note on, 0x80 = note off
            if message.len() < 3 {
                return;
            }
            let status = message[0] & 0xF0;
            let note = message[1];
            let velocity = message[2];
            let msg = match status {
                0x90 if velocity > 0 => Some(AudioMessage::NoteOn { note, velocity }),
                0x90 => Some(AudioMessage::NoteOff { note }), // note-on with vel=0
                0x80 => Some(AudioMessage::NoteOff { note }),
                _ => None,
            };
            if let Some(m) = msg {
                let _ = tx.try_send(m);
            }
        },
        (),
    )?;

    params.midi_connected.store(true, Ordering::Relaxed);

    Ok(MidiHandle {
        port_name: Some(port_name),
        _conn: Some(conn),
    })
}
