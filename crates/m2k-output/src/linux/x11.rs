//! X11 backend: the XTEST extension, over a pure-Rust X connection.
//!
//! Keys are sent by keycode (evdev code + 8), so they mean the same physical keys as on the
//! other backends; the X server auto-repeats held keys. Every call sends its requests in order on
//! the one connection and flushes them before it returns.
//!
//! Text is typed independently of the keyboard layout:
//! - A character the layout has on the first or second level of a key is typed with that key
//!   (and Shift), while the layout has a single group and no modifier other than Caps Lock and
//!   Num Lock is in effect. Caps Lock is taken into account.
//! - Any other character's keysym is put on a spare keycode, one that has no symbols and that no
//!   [`Key`] uses, and that keycode is pressed (the technique xdotool uses). Applications look a
//!   key press up in the keyboard mapping as it is when they get to the event, so a keycode must
//!   keep its symbol for a while after its press: symbols stay where they are until the injector
//!   is dropped (then those keycodes are reset to NoSymbol), keycodes are reused least recently
//!   used first, and [`Injector::max_text_chunk`] keeps each call below half the spare keycodes,
//!   so a keycode is never given another symbol in the call that pressed it nor in the next one.
//!
//! When the connection breaks, calls fail with `Unavailable` while a new connection is attempted
//! in the background (at most once a second). XTest presses outlive the connection that made
//! them, so releases that failed meanwhile go through once it is back.
//!
//! On a Wayland session this only reaches X11 (XWayland) applications.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::ReplyError;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, KeyButMask, Keysym, MOTION_NOTIFY_EVENT, Window,
};
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use crate::keymap;
use crate::{Injector, Key, MouseButton, OutputError, Result};

pub(crate) const NAME: &str = "linux-x11";

/// Upper bound for the wheel notches of one `scroll` call (one click per notch).
const MAX_NOTCHES: u32 = 1000;

/// Most characters one `text()` call takes.
const MAX_TEXT_PIECE: usize = 32;

/// Pause between attempts to reconnect to the X server.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);

const XK_SHIFT_L: Keysym = 0xffe1;

/// A connection to the X server.
struct Link {
    conn: RustConnection,
    root: Window,
    min_keycode: u8,
    max_keycode: u8,
}

impl Link {
    fn open() -> std::result::Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| {
            format!("cannot connect to the X server ({e}); is DISPLAY set to a running X server?")
        })?;
        let setup = conn.setup();
        let root = setup.roots.get(screen).map(|s| s.root).ok_or("the X server has no screen")?;
        let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
        match conn.extension_information(xtest::X11_EXTENSION_NAME) {
            Ok(Some(_)) => {}
            Ok(None) => return Err("the X server does not offer the XTEST extension".into()),
            Err(e) => return Err(format!("cannot query the X server: {e}")),
        }
        Ok(Link { conn, root, min_keycode, max_keycode })
    }
}

/// A key of the layout that types a character.
#[derive(Debug, Clone, Copy)]
struct LayoutKey {
    keycode: u8,
    /// The character is on the second level (Shift).
    shifted: bool,
    /// The two levels are a lower and an upper case letter, which Caps Lock swaps.
    letter: bool,
}

/// The keyboard state layout keys are typed in.
#[derive(Debug, Clone, Copy)]
struct Typing {
    caps_lock: bool,
}

#[derive(Default)]
struct Reconnect {
    /// An attempt in progress.
    pending: Option<Receiver<std::result::Result<Link, String>>>,
    /// No new attempt before this.
    not_before: Option<Instant>,
    /// Why the connection is down.
    error: String,
}

pub(crate) struct XTestInjector {
    /// `None` while the connection is down.
    link: Option<Link>,
    reconnect: Reconnect,
    min_keycode: u8,
    max_keycode: u8,
    keysyms_per_keycode: u8,
    /// Keycodes without any symbol that no [`Key`] uses (ours included), usable for typing.
    spare: Vec<u8>,
    /// Spare keycodes we mapped, least recently used first.
    mapped: VecDeque<(u8, Keysym)>,
    /// Characters on the first two levels of a single-group layout, by keysym.
    layout: HashMap<Keysym, LayoutKey>,
    /// The keycode of Shift_L, for second-level characters.
    shift_keycode: Option<u8>,
    /// The keyboard mapping may have changed (MappingNotify, a new connection): re-read it
    /// before typing.
    mapping_changed: bool,
    down: HashSet<Key>,
    buttons: HashSet<MouseButton>,
}

impl XTestInjector {
    pub(crate) fn new() -> std::result::Result<Self, String> {
        let link = Link::open()?;
        let mut injector = Self {
            min_keycode: link.min_keycode,
            max_keycode: link.max_keycode,
            link: Some(link),
            reconnect: Reconnect::default(),
            keysyms_per_keycode: 1,
            spare: Vec::new(),
            mapped: VecDeque::new(),
            layout: HashMap::new(),
            shift_keycode: None,
            mapping_changed: false,
            down: HashSet::new(),
            buttons: HashSet::new(),
        };
        injector.refresh_mapping().map_err(|e| e.to_string())?;
        Ok(injector)
    }

    // -----------------------------------------------------------------------------------------
    // Connection
    // -----------------------------------------------------------------------------------------

    /// The live connection (`Unavailable` while it is down).
    fn live(&self) -> Result<&Link> {
        self.link.as_ref().ok_or_else(|| self.down_error())
    }

    fn down_error(&self) -> OutputError {
        OutputError::Unavailable(format!(
            "lost the connection to the X server ({}); trying to reconnect",
            self.reconnect.error
        ))
    }

    /// The connection failed: drop it (the next call starts reconnecting).
    fn lost(&mut self, error: impl std::fmt::Display) -> OutputError {
        if self.link.take().is_some() {
            log::warn!("lost the connection to the X server: {error}");
        }
        self.reconnect.error = error.to_string();
        self.reconnect.not_before = None;
        self.down_error()
    }

    /// Makes sure there is a connection: adopts a finished reconnection attempt or starts one, on
    /// a thread of its own (connecting can take long: an X server started on demand, a remote
    /// display). `Unavailable` until a new connection is up.
    fn connect(&mut self) -> Result<()> {
        if self.link.is_some() {
            return Ok(());
        }
        if let Some(pending) = &self.reconnect.pending {
            let outcome = match pending.try_recv() {
                Ok(outcome) => outcome,
                Err(TryRecvError::Empty) => return Err(self.down_error()),
                Err(TryRecvError::Disconnected) => Err("the reconnection attempt failed".into()),
            };
            self.reconnect.pending = None;
            match outcome {
                Ok(link) => {
                    self.adopt(link);
                    return Ok(());
                }
                Err(error) => self.reconnect.error = error,
            }
        }
        let now = Instant::now();
        if self.reconnect.not_before.is_none_or(|at| now >= at) {
            self.reconnect.not_before = Some(now + RECONNECT_INTERVAL);
            let (sender, receiver) = mpsc::channel();
            let spawned =
                std::thread::Builder::new().name("m2k-x11-reconnect".into()).spawn(move || {
                    let _ = sender.send(Link::open());
                });
            if spawned.is_ok() {
                self.reconnect.pending = Some(receiver);
            }
        }
        Err(self.down_error())
    }

    fn adopt(&mut self, link: Link) {
        self.min_keycode = link.min_keycode;
        self.max_keycode = link.max_keycode;
        self.link = Some(link);
        self.reconnect = Reconnect::default();
        // It may be a new X server: re-read the mapping before typing.
        self.mapping_changed = true;
        log::info!("reconnected to the X server");
    }

    /// Sends one XTest event (X errors about it are ignored).
    fn fake(&mut self, kind: u8, detail: u8, root: Window, x: i16, y: i16) -> Result<()> {
        let sent = self
            .live()?
            .conn
            .xtest_fake_input(kind, detail, CURRENT_TIME, root, x, y, 0)
            .map(|cookie| cookie.ignore_error());
        sent.map_err(|e| self.lost(e))
    }

    fn fake_key(&mut self, keycode: u8, down: bool) -> Result<()> {
        self.fake(if down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT }, keycode, NONE, 0, 0)
    }

    fn fake_button(&mut self, button: u8, down: bool) -> Result<()> {
        let kind = if down { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT };
        self.fake(kind, button, NONE, 0, 0)
    }

    /// Sends what is buffered and drains incoming events (only MappingNotify matters).
    fn finish(&mut self) -> Result<()> {
        let link = self.live()?;
        let mut changed = false;
        let result = link.conn.flush().and_then(|()| {
            while let Some(event) = link.conn.poll_for_event()? {
                if let Event::MappingNotify(_) = event {
                    changed = true;
                }
            }
            Ok(())
        });
        self.mapping_changed |= changed;
        result.map_err(|e| self.lost(e))
    }

    fn keycode(&self, key: Key) -> Result<u8> {
        keymap::evdev(key)
            .and_then(|code| u8::try_from(code + 8).ok())
            .filter(|keycode| (self.min_keycode..=self.max_keycode).contains(keycode))
            .ok_or_else(|| OutputError::Unsupported(key.label(), NAME))
    }

    // -----------------------------------------------------------------------------------------
    // Text
    // -----------------------------------------------------------------------------------------

    /// Re-reads the keyboard mapping: which keycodes are free, which of ours still hold what we
    /// put there (a layout switch replaces the whole map), and which characters the layout has.
    fn refresh_mapping(&mut self) -> Result<()> {
        let (min, max) = (self.min_keycode, self.max_keycode);
        let count = max.saturating_sub(min).saturating_add(1);
        let reply = self
            .live()?
            .conn
            .get_keyboard_mapping(min, count)
            .map_err(ReplyError::from)
            .and_then(|cookie| cookie.reply());
        let reply = match reply {
            Ok(reply) => reply,
            Err(ReplyError::ConnectionError(e)) => return Err(self.lost(e)),
            Err(e) => {
                return Err(OutputError::Other(format!(
                    "the X server did not report its keyboard mapping ({e})"
                )));
            }
        };
        let per = usize::from(reply.keysyms_per_keycode.max(1));
        self.keysyms_per_keycode = reply.keysyms_per_keycode.max(1);
        // Keycodes the engine presses as keys of their own (F24, say, which many layouts leave
        // without a symbol) must never type characters.
        let reserved: HashSet<u8> =
            Key::ALL.iter().filter_map(|&key| self.keycode(key).ok()).collect();
        let mut spare = Vec::new();
        let mut ours = Vec::new();
        let mut layout = HashMap::new();
        let mut second_level = Vec::new();
        let mut shift_keycode = None;
        let mut single_group = true;
        for (index, row) in reply.keysyms.chunks(per).enumerate() {
            let Some(keycode) = min.checked_add(index as u8) else { break };
            let column = |i: usize| row.get(i).copied().unwrap_or(0);
            let mine = self.mapped.iter().find(|&&(k, _)| k == keycode).map(|&(_, sym)| sym);
            // XKB servers store our `[sym; n]` as a one-level key and report it back with
            // NoSymbol in some columns, so "ours" means: only our symbol, and at least once.
            if let Some(sym) = mine
                && row.contains(&sym)
                && row.iter().all(|&s| s == sym || s == 0)
            {
                ours.push(keycode);
                spare.push(keycode);
                continue;
            }
            if row.iter().all(|&s| s == 0) {
                if !reserved.contains(&keycode) {
                    spare.push(keycode);
                }
                continue;
            }
            // XKB reports a key of a one-group layout as G1L1 G1L2 G1L1 G1L2 ...: a second group
            // shows in columns 3 and 4. Which group is active is not known here.
            if (0..2).any(|i| column(2 + i) != 0 && column(2 + i) != column(i)) {
                single_group = false;
            }
            let (first, second) = (column(0), column(1));
            if first == XK_SHIFT_L && shift_keycode.is_none() {
                shift_keycode = Some(keycode);
            }
            let letter = is_case_pair(first, second);
            if first != 0 {
                layout.entry(first).or_insert(LayoutKey { keycode, shifted: false, letter });
            }
            if second != 0 && second != first {
                second_level.push((second, LayoutKey { keycode, shifted: true, letter }));
            }
        }
        // A character on the first level of some key is typed without Shift.
        for (sym, key) in second_level {
            layout.entry(sym).or_insert(key);
        }
        self.mapped.retain(|(keycode, _)| ours.contains(keycode));
        self.spare = spare;
        self.layout = if single_group { layout } else { HashMap::new() };
        self.shift_keycode = shift_keycode;
        self.mapping_changed = false;
        Ok(())
    }

    /// Whether layout keys can type right now (`None`: spare keycodes only): the layout has a
    /// single group and no modifier other than Caps Lock and Num Lock is in effect.
    ///
    /// The core QueryPointer, not XKB's GetState: once a connection enables XKB, the server no
    /// longer sends it the core MappingNotify when a new keymap is loaded (setxkbmap), which
    /// `refresh_mapping` depends on.
    fn layout_typing(&mut self) -> Result<Option<Typing>> {
        if self.layout.is_empty() || self.down.iter().any(|key| key.is_modifier()) {
            return Ok(None);
        }
        let link = self.live()?;
        let reply = link
            .conn
            .query_pointer(link.root)
            .map_err(ReplyError::from)
            .and_then(|cookie| cookie.reply());
        let mask = match reply {
            Ok(reply) => reply.mask,
            Err(ReplyError::ConnectionError(e)) => return Err(self.lost(e)),
            Err(_) => return Ok(None),
        };
        let other = KeyButMask::SHIFT
            | KeyButMask::CONTROL
            | KeyButMask::MOD1
            | KeyButMask::MOD3
            | KeyButMask::MOD4
            | KeyButMask::MOD5;
        if u16::from(mask) & u16::from(other) != 0 {
            return Ok(None);
        }
        Ok(Some(Typing { caps_lock: u16::from(mask) & u16::from(KeyButMask::LOCK) != 0 }))
    }

    /// The layout key that types `c` (keysym `sym`) and whether it takes Shift.
    fn layout_key(&self, c: char, sym: Keysym, typing: Typing) -> Option<(u8, bool)> {
        let key = self.layout.get(&sym)?;
        if typing.caps_lock && (c.is_lowercase() || c.is_uppercase()) && !key.letter {
            return None; // Caps Lock would change its case
        }
        // Caps Lock swaps the levels of letter keys.
        let shift = key.shifted != (typing.caps_lock && key.letter);
        if shift && self.shift_keycode.is_none() {
            return None;
        }
        Some((key.keycode, shift))
    }

    /// A spare keycode that currently produces `sym`, mapping one when needed.
    fn keycode_for(&mut self, sym: Keysym) -> Result<u8> {
        if let Some(position) = self.mapped.iter().position(|&(_, s)| s == sym)
            && let Some(entry) = self.mapped.remove(position)
        {
            self.mapped.push_back(entry);
            return Ok(entry.0);
        }
        let free = self.spare.iter().copied().find(|k| !self.mapped.iter().any(|(m, _)| m == k));
        let keycode = match free.or_else(|| self.mapped.pop_front().map(|(k, _)| k)) {
            Some(keycode) => keycode,
            None => {
                return Err(OutputError::Unsupported(
                    "typing characters that are not on the keyboard layout (the X server has \
                     no free keycode)"
                        .into(),
                    NAME,
                ));
            }
        };
        let row = vec![sym; usize::from(self.keysyms_per_keycode)];
        let sent = self
            .live()?
            .conn
            .change_keyboard_mapping(1, keycode, self.keysyms_per_keycode, &row)
            .map(|cookie| cookie.ignore_error());
        sent.map_err(|e| self.lost(e))?;
        self.mapped.push_back((keycode, sym));
        Ok(keycode)
    }

    fn type_chars(
        &mut self,
        text: &str,
        typing: Option<Typing>,
        held: &HashSet<u8>,
        shifted: &mut bool,
    ) -> Result<()> {
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            let (keycode, shift) = match c {
                '\r' | '\n' => {
                    if c == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    (self.keycode(Key::Enter)?, Some(false))
                }
                '\t' => (self.keycode(Key::Tab)?, Some(false)),
                c => {
                    let Some(sym) = keysym(c) else { continue }; // other control characters
                    match typing.and_then(|typing| self.layout_key(c, sym, typing)) {
                        Some((keycode, shift)) => (keycode, Some(shift)),
                        // A spare keycode carries its symbol on every level: Shift is irrelevant.
                        None => (self.keycode_for(sym)?, None),
                    }
                }
            };
            if let (Some(want), Some(shift_keycode)) = (shift, self.shift_keycode)
                && want != *shifted
            {
                self.fake_key(shift_keycode, want)?;
                *shifted = want;
            }
            if held.contains(&keycode) {
                // A key held through the injector stays held: release it and press it again.
                self.fake_key(keycode, false)?;
                self.fake_key(keycode, true)?;
            } else {
                self.fake_key(keycode, true)?;
                self.fake_key(keycode, false)?;
            }
        }
        Ok(())
    }
}

impl Injector for XTestInjector {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports(&self, key: Key) -> bool {
        self.keycode(key).is_ok()
    }

    fn repeats_held_keys(&self) -> bool {
        true
    }

    fn max_text_chunk(&self) -> Option<usize> {
        // Fewer than half the spare keycodes per call (see the module documentation).
        Some(match self.spare.len() {
            0 => MAX_TEXT_PIECE,
            spare => ((spare - 1) / 2).clamp(1, MAX_TEXT_PIECE),
        })
    }

    fn key_down(&mut self, key: Key) -> Result<()> {
        let keycode = self.keycode(key)?;
        self.connect()?;
        self.down.insert(key);
        self.fake_key(keycode, true)?;
        self.finish()
    }

    fn key_up(&mut self, key: Key) -> Result<()> {
        let keycode = self.keycode(key)?;
        self.down.remove(&key);
        self.connect()?;
        self.fake_key(keycode, false)?;
        self.finish()
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.connect()?;
        self.finish()?;
        if self.mapping_changed {
            self.refresh_mapping()?;
        }
        let typing = self.layout_typing()?;
        let held: HashSet<u8> =
            self.down.iter().filter_map(|&key| self.keycode(key).ok()).collect();
        let mut shifted = false;
        let typed = self.type_chars(text, typing, &held, &mut shifted);
        // Never leave our own Shift down, also when typing failed halfway.
        let released = match (shifted, self.shift_keycode) {
            (true, Some(shift_keycode)) => self.fake_key(shift_keycode, false),
            _ => Ok(()),
        };
        let flushed = self.finish();
        typed.and(released).and(flushed)
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        if down {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(&button);
        }
        self.connect()?;
        self.fake_button(button_number(button), down)?;
        self.finish()
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        self.connect()?;
        // detail 1 = relative motion.
        self.fake(MOTION_NOTIFY_EVENT, 1, NONE, clamp16(dx), clamp16(dy))?;
        self.finish()
    }

    fn mouse_move_absolute(&mut self, x: i32, y: i32) -> Result<()> {
        self.connect()?;
        let root = self.live()?.root;
        self.fake(MOTION_NOTIFY_EVENT, 0, root, clamp16(x), clamp16(y))?;
        self.finish()
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.connect()?;
        // Buttons 4/5 scroll up/down, 6/7 left/right: one click per notch.
        for (notches, positive, negative) in [(dy, 4u8, 5u8), (dx, 7, 6)] {
            let button = if notches > 0 { positive } else { negative };
            for _ in 0..notches.unsigned_abs().min(MAX_NOTCHES) {
                self.fake_button(button, true)?;
                self.fake_button(button, false)?;
            }
        }
        self.finish()
    }
}

impl Drop for XTestInjector {
    fn drop(&mut self) {
        let Some(link) = &self.link else { return };
        let fake = |kind: u8, detail: u8| {
            if let Ok(cookie) =
                link.conn.xtest_fake_input(kind, detail, CURRENT_TIME, NONE, 0, 0, 0)
            {
                cookie.ignore_error();
            }
        };
        // XTest presses outlive the connection: release what is still held.
        for &key in &self.down {
            if let Ok(keycode) = self.keycode(key) {
                fake(KEY_RELEASE_EVENT, keycode);
            }
        }
        for &button in &self.buttons {
            fake(BUTTON_RELEASE_EVENT, button_number(button));
        }
        let empty = vec![0; usize::from(self.keysyms_per_keycode)];
        for &(keycode, _) in &self.mapped {
            if let Ok(cookie) =
                link.conn.change_keyboard_mapping(1, keycode, self.keysyms_per_keycode, &empty)
            {
                cookie.ignore_error();
            }
        }
        let _ = link.conn.flush();
    }
}

fn button_number(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::Back => 8,
        MouseButton::Forward => 9,
    }
}

/// The keysym of a character: Latin-1 maps directly, everything else is a Unicode keysym.
fn keysym(c: char) -> Option<Keysym> {
    let code = u32::from(c);
    match code {
        0x20..=0x7E | 0xA0..=0xFF => Some(code),
        _ if c.is_control() => None,
        _ => Some(0x0100_0000 | code),
    }
}

/// The character of a Latin-1 or Unicode keysym.
fn keysym_char(sym: Keysym) -> Option<char> {
    match sym {
        0x20..=0x7E | 0xA0..=0xFF => char::from_u32(sym),
        0x0100_0100..=0x0110_FFFF => char::from_u32(sym - 0x0100_0000),
        _ => None,
    }
}

/// Whether `lower` / `upper` are the lower and upper case of one letter.
fn is_case_pair(lower: Keysym, upper: Keysym) -> bool {
    let (Some(lower), Some(upper)) = (keysym_char(lower), keysym_char(upper)) else {
        return false;
    };
    let mut cased = lower.to_uppercase();
    lower.is_lowercase() && cased.next() == Some(upper) && cased.next().is_none()
}

fn clamp16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}
