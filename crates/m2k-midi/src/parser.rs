//! Bytes -> [`MidiMessage`].
//!
//! CONTRACT (signatures are relied upon by the engine and the CLI; keep them stable):
//!
//! - [`parse_message`] decodes exactly one complete message as delivered by OS MIDI APIs.
//! - [`Parser`] is a streaming parser for arbitrary byte chunks: running status, real-time bytes
//!   interleaved inside other messages (including inside SysEx), SysEx split across chunks,
//!   SysEx terminated by a new status byte instead of `F7`, stray data bytes.
//! - [`encode`] is the inverse of [`parse_message`].

use crate::message::MidiMessage;

/// Largest System Exclusive message (including `F0` and `F7`) the streaming [`Parser`]
/// assembles. Anything longer is dropped so a misbehaving device cannot exhaust memory; it is
/// reported as [`MidiMessage::Unknown`] carrying the first [`OVERSIZED_SYSEX_PREFIX`] bytes.
pub const MAX_SYSEX_LEN: usize = 1 << 20;

/// How many leading bytes of an oversized SysEx are kept for its [`MidiMessage::Unknown`] report.
pub const OVERSIZED_SYSEX_PREFIX: usize = 16;

/// Number of data bytes that follow `status`, for statuses that carry data. `0` for everything
/// else (single-byte messages, SysEx, undefined statuses).
fn data_len(status: u8) -> usize {
    match status {
        0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 2,
        0xC0..=0xDF | 0xF1 | 0xF3 => 1,
        _ => 0,
    }
}

/// Decodes the messages that consist of a status byte only. `None` for anything else.
fn decode_single(status: u8) -> Option<MidiMessage> {
    Some(match status {
        0xF6 => MidiMessage::TuneRequest,
        0xF8 => MidiMessage::Clock,
        0xFA => MidiMessage::Start,
        0xFB => MidiMessage::Continue,
        0xFC => MidiMessage::Stop,
        0xFE => MidiMessage::ActiveSensing,
        0xFF => MidiMessage::Reset,
        _ => return None,
    })
}

/// Decodes a status byte that carries data together with exactly `data_len(status)` data
/// bytes (each `< 0x80`). Total: anything unexpected becomes [`MidiMessage::Unknown`].
fn decode(status: u8, data: &[u8]) -> MidiMessage {
    let channel = (status & 0x0F) + 1;
    match (status, data) {
        (0x80..=0x8F, &[note, velocity]) => MidiMessage::NoteOff { channel, note, velocity },
        (0x90..=0x9F, &[note, velocity]) => MidiMessage::NoteOn { channel, note, velocity },
        (0xA0..=0xAF, &[note, pressure]) => MidiMessage::PolyPressure { channel, note, pressure },
        (0xB0..=0xBF, &[controller, value]) => {
            MidiMessage::ControlChange { channel, controller, value }
        }
        (0xC0..=0xCF, &[program]) => MidiMessage::ProgramChange { channel, program },
        (0xD0..=0xDF, &[pressure]) => MidiMessage::ChannelPressure { channel, pressure },
        (0xE0..=0xEF, &[lsb, msb]) => MidiMessage::PitchBend { channel, value: join14(lsb, msb) },
        (0xF1, &[byte]) => MidiMessage::MtcQuarterFrame { piece: byte >> 4, value: byte & 0x0F },
        (0xF2, &[lsb, msb]) => MidiMessage::SongPosition { position: join14(lsb, msb) },
        (0xF3, &[song]) => MidiMessage::SongSelect { song },
        _ => {
            let mut bytes = Vec::with_capacity(1 + data.len());
            bytes.push(status);
            bytes.extend_from_slice(data);
            MidiMessage::Unknown { data: bytes }
        }
    }
}

fn join14(lsb: u8, msb: u8) -> u16 {
    u16::from(lsb & 0x7F) | (u16::from(msb & 0x7F) << 7)
}

/// Decodes one complete MIDI message. Never panics; undecodable input becomes
/// [`MidiMessage::Unknown`].
///
/// "Complete" is strict: the bytes must be exactly one message (a status byte followed by the
/// right number of data bytes, or `F0 .. F7` with only data bytes in between). Empty input,
/// truncated messages, trailing bytes, data bytes without a status, undefined statuses
/// (`F4 F5 F9 FD`) and a lone `F7` all yield `Unknown` carrying the input. Use [`Parser`] for
/// byte streams (running status, interleaved real-time bytes, chunked SysEx).
pub fn parse_message(bytes: &[u8]) -> MidiMessage {
    let unknown = || MidiMessage::Unknown { data: bytes.to_vec() };
    let Some((&status, data)) = bytes.split_first() else {
        return unknown();
    };
    if status < 0x80 {
        return unknown();
    }
    if status == 0xF0 {
        return match data.split_last() {
            Some((&0xF7, payload)) if payload.iter().all(|&b| b < 0x80) => {
                MidiMessage::SysEx { data: bytes.to_vec() }
            }
            _ => unknown(),
        };
    }
    if data.iter().any(|&b| b >= 0x80) {
        return unknown();
    }
    if let Some(message) = decode_single(status) {
        return if data.is_empty() { message } else { unknown() };
    }
    let needed = data_len(status);
    if needed == 0 || data.len() != needed {
        return unknown();
    }
    decode(status, data)
}

/// Encodes a message back to wire bytes.
///
/// For every message produced by [`parse_message`] this returns the original bytes. Messages
/// built by hand never make it panic: channels are clamped to `1..=16`, 7-bit fields to
/// `0..=127`, 14-bit fields to `0..=16383`, and a SysEx payload gets its missing `F0` / `F7`
/// framing added. `Unknown` is returned verbatim.
pub fn encode(message: &MidiMessage) -> Vec<u8> {
    fn status(kind: u8, channel: u8) -> u8 {
        kind | (channel.clamp(1, 16) - 1)
    }
    fn d(value: u8) -> u8 {
        value.min(0x7F)
    }
    fn split14(value: u16) -> [u8; 2] {
        let value = value.min(0x3FFF);
        [(value & 0x7F) as u8, (value >> 7) as u8]
    }

    match *message {
        MidiMessage::NoteOff { channel, note, velocity } => {
            vec![status(0x80, channel), d(note), d(velocity)]
        }
        MidiMessage::NoteOn { channel, note, velocity } => {
            vec![status(0x90, channel), d(note), d(velocity)]
        }
        MidiMessage::PolyPressure { channel, note, pressure } => {
            vec![status(0xA0, channel), d(note), d(pressure)]
        }
        MidiMessage::ControlChange { channel, controller, value } => {
            vec![status(0xB0, channel), d(controller), d(value)]
        }
        MidiMessage::ProgramChange { channel, program } => vec![status(0xC0, channel), d(program)],
        MidiMessage::ChannelPressure { channel, pressure } => {
            vec![status(0xD0, channel), d(pressure)]
        }
        MidiMessage::PitchBend { channel, value } => {
            let [lsb, msb] = split14(value);
            vec![status(0xE0, channel), lsb, msb]
        }
        MidiMessage::SysEx { ref data } => {
            if data.len() >= 2 && data[0] == 0xF0 && data[data.len() - 1] == 0xF7 {
                return data.clone();
            }
            let payload = data.strip_prefix(&[0xF0]).unwrap_or(data);
            let payload = payload.strip_suffix(&[0xF7]).unwrap_or(payload);
            let mut bytes = Vec::with_capacity(payload.len() + 2);
            bytes.push(0xF0);
            bytes.extend_from_slice(payload);
            bytes.push(0xF7);
            bytes
        }
        MidiMessage::MtcQuarterFrame { piece, value } => {
            vec![0xF1, ((piece & 0x07) << 4) | (value & 0x0F)]
        }
        MidiMessage::SongPosition { position } => {
            let [lsb, msb] = split14(position);
            vec![0xF2, lsb, msb]
        }
        MidiMessage::SongSelect { song } => vec![0xF3, d(song)],
        MidiMessage::TuneRequest => vec![0xF6],
        MidiMessage::Clock => vec![0xF8],
        MidiMessage::Start => vec![0xFA],
        MidiMessage::Continue => vec![0xFB],
        MidiMessage::Stop => vec![0xFC],
        MidiMessage::ActiveSensing => vec![0xFE],
        MidiMessage::Reset => vec![0xFF],
        MidiMessage::Unknown { ref data } => data.clone(),
    }
}

/// Streaming parser. Feed it arbitrary chunks of a MIDI byte stream.
///
/// Rules (MIDI 1.0):
/// - **Running status**: after a channel message, further data bytes reuse its status.
/// - **Real-time bytes** (`F8..=FF`) are emitted the moment they arrive, wherever they appear
///   (between a status and its data, inside SysEx), without disturbing the message in progress
///   or the running status. The undefined `F9` / `FD` are emitted as `Unknown`.
/// - **System common** (`F0..=F7`) and SysEx clear running status; data bytes after a completed
///   system common message are dropped.
/// - **SysEx** may span any number of chunks. `F7` completes it. Any other non-real-time status
///   byte also completes it (the `F7` was lost): the SysEx is emitted with an `F7` appended, then
///   the new status byte is processed. Longer than [`MAX_SYSEX_LEN`] -> dropped, reported as
///   `Unknown` (see the constant).
/// - **Garbage**: data bytes with no running status are dropped. A message cut short by a new
///   status byte is emitted as `Unknown` (status plus the data bytes received). Undefined system
///   common statuses `F4` / `F5` and a stray `F7` are emitted as `Unknown`.
///
/// Allocation happens only for messages that own bytes (SysEx and `Unknown`).
#[derive(Debug, Default)]
pub struct Parser {
    /// Status of the message being assembled, or the running status after a complete channel
    /// message. `0` when there is none.
    status: u8,
    /// Data bytes received so far for `status`.
    data: [u8; 2],
    len: usize,
    /// A status byte arrived and none of its data bytes yet.
    awaiting_data: bool,
    /// Inside a System Exclusive message.
    in_sysex: bool,
    /// The SysEx so far, starting with `F0`. Only a short prefix once it became oversized.
    sysex: Vec<u8>,
    sysex_oversized: bool,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes, invoking `emit` for every completed message, in stream order.
    pub fn feed(&mut self, bytes: &[u8], emit: &mut dyn FnMut(MidiMessage)) {
        for &byte in bytes {
            self.push(byte, emit);
        }
    }

    /// Ends the stream: a message still in progress (including an unterminated SysEx) is
    /// emitted as [`MidiMessage::Unknown`], then the parser is [`reset`](Self::reset).
    /// Useful when a chunk is known to be self-contained, e.g. injected test input.
    pub fn finish(&mut self, emit: &mut dyn FnMut(MidiMessage)) {
        if self.in_sysex {
            self.in_sysex = false;
            self.sysex_oversized = false;
            emit(MidiMessage::Unknown { data: std::mem::take(&mut self.sysex) });
        }
        self.abort_partial(emit);
        self.reset();
    }

    /// Drops any partially received message and forgets running status.
    pub fn reset(&mut self) {
        self.status = 0;
        self.len = 0;
        self.awaiting_data = false;
        self.in_sysex = false;
        self.sysex = Vec::new();
        self.sysex_oversized = false;
    }

    fn push(&mut self, byte: u8, emit: &mut dyn FnMut(MidiMessage)) {
        if byte >= 0xF8 {
            emit(decode_single(byte).unwrap_or_else(|| MidiMessage::Unknown { data: vec![byte] }));
            return;
        }

        if self.in_sysex {
            if byte < 0x80 {
                self.sysex_push(byte);
                return;
            }
            self.end_sysex(emit);
            if byte == 0xF7 {
                return;
            }
            // Another status byte terminated the SysEx: process it below.
        }

        if byte >= 0x80 {
            self.abort_partial(emit);
            match byte {
                0x80..=0xEF | 0xF1..=0xF3 => {
                    self.status = byte;
                    self.awaiting_data = true;
                }
                0xF0 => {
                    self.status = 0;
                    self.in_sysex = true;
                    self.sysex.clear();
                    self.sysex.push(0xF0);
                }
                0xF6 => {
                    self.status = 0;
                    emit(MidiMessage::TuneRequest);
                }
                // F4 / F5 (undefined system common) and a stray F7.
                _ => {
                    self.status = 0;
                    emit(MidiMessage::Unknown { data: vec![byte] });
                }
            }
            return;
        }

        // A data byte.
        let needed = data_len(self.status);
        if needed == 0 {
            return; // no running status: stray data byte
        }
        if let Some(slot) = self.data.get_mut(self.len) {
            *slot = byte;
        }
        self.len += 1;
        self.awaiting_data = false;
        if self.len >= needed {
            let message = decode(self.status, &self.data[..needed]);
            self.len = 0;
            if self.status >= 0xF0 {
                self.status = 0; // system common messages do not establish running status
            }
            emit(message);
        }
    }

    /// Emits a message cut short by a new status byte as `Unknown`.
    fn abort_partial(&mut self, emit: &mut dyn FnMut(MidiMessage)) {
        if self.status != 0 && (self.awaiting_data || self.len > 0) {
            let mut data = Vec::with_capacity(1 + self.len);
            data.push(self.status);
            data.extend_from_slice(&self.data[..self.len.min(2)]);
            emit(MidiMessage::Unknown { data });
        }
        self.len = 0;
        self.awaiting_data = false;
    }

    fn sysex_push(&mut self, byte: u8) {
        if self.sysex_oversized {
            return;
        }
        // Keep one byte of room for the closing F7.
        if self.sysex.len() + 1 >= MAX_SYSEX_LEN {
            log::warn!(
                "dropping a SysEx message longer than {MAX_SYSEX_LEN} bytes (starts with {:02X?})",
                &self.sysex[..self.sysex.len().min(OVERSIZED_SYSEX_PREFIX)]
            );
            self.sysex_oversized = true;
            self.sysex.truncate(OVERSIZED_SYSEX_PREFIX);
            self.sysex.shrink_to_fit();
            return;
        }
        self.sysex.push(byte);
    }

    fn end_sysex(&mut self, emit: &mut dyn FnMut(MidiMessage)) {
        self.in_sysex = false;
        self.status = 0;
        let mut data = std::mem::take(&mut self.sysex);
        if std::mem::take(&mut self.sysex_oversized) {
            emit(MidiMessage::Unknown { data });
        } else {
            data.push(0xF7);
            emit(MidiMessage::SysEx { data });
        }
    }
}
