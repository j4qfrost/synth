//! OSC output: ship spectrum bands + onsets to `thea` for audio-reactive lighting.
//!
//! Namespace (one UDP message each, fire-and-forget):
//!
//! | Address                  | Args    | Meaning                          |
//! |--------------------------|---------|----------------------------------|
//! | `/synth/spectrum/bands`  | `f f f` | bass, mid, treble (each 0.0–1.0) |
//! | `/synth/onset`           | `f i`   | flux (≥0), onset (0 or 1)        |
//! | `/synth/spectrum/peak`   | `f`     | peak frequency (Hz)              |
//!
//! Target defaults to `127.0.0.1:9000`; override with `SYNTH_OSC_ADDR`
//! (e.g. `snowman:9000`). Must match `thea_io`'s OSC listener.

use rosc::{encoder, OscMessage, OscPacket, OscType};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use synth_fft::SpectrumFrame;

pub const DEFAULT_ADDR: &str = "127.0.0.1:9000";

pub struct OscOut {
    socket: UdpSocket,
    addr: SocketAddr,
}

impl OscOut {
    /// Bind an ephemeral UDP socket aimed at `SYNTH_OSC_ADDR` (or the default).
    /// Returns `None` if the address can't be resolved or the socket can't bind
    /// — OSC output is best-effort and never fatal to the synth.
    pub fn from_env() -> Option<Self> {
        let target = std::env::var("SYNTH_OSC_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
        let addr = match target.to_socket_addrs().ok().and_then(|mut a| a.next()) {
            Some(a) => a,
            None => {
                log::warn!("OSC target '{target}' did not resolve — OSC output disabled");
                return None;
            }
        };
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        log::info!("OSC spectrum output → {target}");
        Some(Self { socket, addr })
    }

    /// Send one frame's bands/onset/peak. Errors are logged at debug and ignored.
    pub fn send(&self, f: &SpectrumFrame) {
        self.emit(
            "/synth/spectrum/bands",
            vec![
                OscType::Float(f.bass),
                OscType::Float(f.mid),
                OscType::Float(f.treble),
            ],
        );
        self.emit(
            "/synth/onset",
            vec![OscType::Float(f.flux), OscType::Int(f.onset as i32)],
        );
        self.emit("/synth/spectrum/peak", vec![OscType::Float(f.peak_hz)]);
    }

    fn emit(&self, addr: &str, args: Vec<OscType>) {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.to_string(),
            args,
        });
        match encoder::encode(&packet) {
            Ok(buf) => {
                if let Err(e) = self.socket.send_to(&buf, self.addr) {
                    log::debug!("OSC send failed: {e}");
                }
            }
            Err(e) => log::debug!("OSC encode failed: {e:?}"),
        }
    }
}
