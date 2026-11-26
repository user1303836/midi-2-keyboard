//! X11 backend: the XTEST extension, over a pure-Rust X connection.
//!
//! Keys are sent by keycode (evdev code + 8), so they mean the same physical keys as on the
//! other backends; the X server auto-repeats held keys. Text is typed layout-independently by
//! mapping each character's keysym onto a spare keycode (one without symbols) and pressing that
//! (the technique xdotool uses). Mapped keycodes are reused least-recently-used first and kept
//! until the injector is dropped, so a client that is still translating an earlier key press
//! never sees its keycode change underneath it; they are reset to NoSymbol on drop.
//!
//! On a Wayland session this only reaches X11 (XWayland) applications.

use std::collections::{HashSet, VecDeque};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, Keysym, MOTION_NOTIFY_EVENT, Window,
};
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use crate::keymap;
use crate::{Injector, Key, MouseButton, OutputError, Result};

pub(crate) const NAME: &str = "linux-x11";

/// Upper bound for the wheel notches of one `scroll` call (one click per notch).
const MAX_NOTCHES: u32 = 1000;

pub(crate) struct XTestInjector {
    conn: RustConnection,
    root: Window,
    min_keycode: u8,
    max_keycode: u8,
    keysyms_per_keycode: u8,
    /// Keycodes without any symbol (ours included), usable for typing.
    spare: Vec<u8>,
    /// Spare keycodes we mapped, least recently used first.
    mapped: VecDeque<(u8, Keysym)>,
    /// A MappingNotify arrived: re-read the keyboard mapping before typing.
    mapping_changed: bool,
    down: HashSet<Key>,
    buttons: HashSet<MouseButton>,
}

impl XTestInjector {
    pub(crate) fn new() -> std::result::Result<Self, String> {
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
        let mut injector = Self {
            conn,
            root,
            min_keycode,
            max_keycode,
            keysyms_per_keycode: 1,
            spare: Vec::new(),
            mapped: VecDeque::new(),
            mapping_changed: false,
            down: HashSet::new(),
            buttons: HashSet::new(),
        };
        injector.refresh_mapping().map_err(|e| e.to_string())?;
        Ok(injector)
    }

    /// Re-reads the keyboard mapping: which keycodes are free, and which of ours still hold
    /// what we put there (a layout switch replaces the whole map).
    fn refresh_mapping(&mut self) -> Result<()> {
        let count = self.max_keycode.saturating_sub(self.min_keycode).saturating_add(1);
        let reply = self
            .conn
            .get_keyboard_mapping(self.min_keycode, count)
            .map_err(lost)?
            .reply()
            .map_err(reply_lost)?;
        let per = usize::from(reply.keysyms_per_keycode.max(1));
        self.keysyms_per_keycode = reply.keysyms_per_keycode.max(1);
        let mut spare = Vec::new();
        let mut ours = Vec::new();
        for (index, row) in reply.keysyms.chunks(per).enumerate() {
            let Some(keycode) = self.min_keycode.checked_add(index as u8) else { break };
            let mine = self.mapped.iter().find(|&&(k, _)| k == keycode).map(|&(_, sym)| sym);
            // XKB servers store our `[sym; n]` as a one-level key and report it back with
            // NoSymbol in some columns, so "ours" means: only our symbol, and at least once.
            if let Some(sym) = mine
                && row.contains(&sym)
                && row.iter().all(|&s| s == sym || s == 0)
            {
                ours.push(keycode);
                spare.push(keycode);
            } else if row.iter().all(|&s| s == 0) {
                spare.push(keycode);
            }
        }
        self.mapped.retain(|(keycode, _)| ours.contains(keycode));
        self.spare = spare;
        self.mapping_changed = false;
        Ok(())
    }

    /// A keycode that currently produces `sym`, mapping a spare one when needed.
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
        self.conn
            .change_keyboard_mapping(1, keycode, self.keysyms_per_keycode, &row)
            .map_err(lost)?
            .ignore_error();
        self.mapped.push_back((keycode, sym));
        Ok(keycode)
    }

    fn fake(&self, kind: u8, detail: u8, root: Window, x: i16, y: i16) -> Result<()> {
        self.conn
            .xtest_fake_input(kind, detail, CURRENT_TIME, root, x, y, 0)
            .map_err(lost)?
            .ignore_error();
        Ok(())
    }

    fn fake_key(&self, keycode: u8, down: bool) -> Result<()> {
        self.fake(if down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT }, keycode, NONE, 0, 0)
    }

    fn fake_button(&self, button: u8, down: bool) -> Result<()> {
        let kind = if down { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT };
        self.fake(kind, button, NONE, 0, 0)
    }

    /// Sends what is buffered and drains incoming events (only MappingNotify matters).
    fn finish(&mut self) -> Result<()> {
        self.conn.flush().map_err(lost)?;
        while let Some(event) = self.conn.poll_for_event().map_err(lost)? {
            if let Event::MappingNotify(_) = event {
                self.mapping_changed = true;
            }
        }
        Ok(())
    }

    fn keycode(&self, key: Key) -> Result<u8> {
        keymap::evdev(key)
            .and_then(|code| u8::try_from(code + 8).ok())
            .filter(|keycode| (self.min_keycode..=self.max_keycode).contains(keycode))
            .ok_or_else(|| OutputError::Unsupported(key.label(), NAME))
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

    fn key_down(&mut self, key: Key) -> Result<()> {
        let keycode = self.keycode(key)?;
        self.down.insert(key);
        self.fake_key(keycode, true)?;
        self.finish()
    }

    fn key_up(&mut self, key: Key) -> Result<()> {
        let keycode = self.keycode(key)?;
        self.down.remove(&key);
        self.fake_key(keycode, false)?;
        self.finish()
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.finish()?;
        if self.mapping_changed {
            self.refresh_mapping()?;
        }
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            let keycode = match c {
                '\r' | '\n' => {
                    if c == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    self.keycode(Key::Enter)?
                }
                '\t' => self.keycode(Key::Tab)?,
                c => match keysym(c) {
                    Some(sym) => self.keycode_for(sym)?,
                    None => continue, // other control characters have no keysym
                },
            };
            self.fake_key(keycode, true)?;
            self.fake_key(keycode, false)?;
        }
        self.finish()
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        if down {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(&button);
        }
        self.fake_button(button_number(button), down)?;
        self.finish()
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        // detail 1 = relative motion.
        self.fake(MOTION_NOTIFY_EVENT, 1, NONE, clamp16(dx), clamp16(dy))?;
        self.finish()
    }

    fn mouse_move_absolute(&mut self, x: i32, y: i32) -> Result<()> {
        self.fake(MOTION_NOTIFY_EVENT, 0, self.root, clamp16(x), clamp16(y))?;
        self.finish()
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
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
        // XTest presses outlive the connection: release what is still held.
        for key in std::mem::take(&mut self.down) {
            if let Ok(keycode) = self.keycode(key) {
                let _ = self.fake_key(keycode, false);
            }
        }
        for button in std::mem::take(&mut self.buttons) {
            let _ = self.fake_button(button_number(button), false);
        }
        let empty = vec![0; usize::from(self.keysyms_per_keycode)];
        for (keycode, _) in std::mem::take(&mut self.mapped) {
            if let Ok(cookie) =
                self.conn.change_keyboard_mapping(1, keycode, self.keysyms_per_keycode, &empty)
            {
                cookie.ignore_error();
            }
        }
        let _ = self.conn.flush();
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

fn clamp16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn lost(e: ConnectionError) -> OutputError {
    OutputError::Unavailable(format!("lost the connection to the X server: {e}"))
}

fn reply_lost(e: ReplyError) -> OutputError {
    OutputError::Unavailable(format!("the X server request failed: {e}"))
}
