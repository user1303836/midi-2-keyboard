//! uinput backend: a virtual keyboard + mouse created through `/dev/uinput`.
//!
//! Works under Wayland and X11 alike because the compositor / X server reads the virtual device
//! like real hardware (so it also auto-repeats held keys). Limits:
//! - No absolute pointer positioning: a relative device has no screen coordinates, and Wayland
//!   exposes no global coordinate space to clients.
//! - `text()` is best effort: it presses the keys that produce each ASCII character on a US
//!   layout (Shift handled), other characters are skipped and reported. The kernel queues only
//!   about 128 events per reader, so long texts should be paced by the caller
//!   (~25 characters per call).

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, RelativeAxisCode};

use crate::keymap;
use crate::{Injector, Key, MouseButton, OutputError, Result};

pub(crate) const NAME: &str = "linux-uinput";
const DEVICE_NAME: &str = "midi-2-keyboard virtual input";
const UINPUT_PATH: &str = "/dev/uinput";

/// Time the compositor / X server needs to notice a new input device and open it. Events
/// written before that are lost, so creation waits once.
const SETTLE: Duration = Duration::from_millis(250);

/// Highest `KEY_*` code declared (`KEY_MICMUTE`); codes from 0x100 on are buttons, which would
/// change how the device is classified.
const MAX_KEY_CODE: u16 = 248;

const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_SIDE: u16 = 0x113;
const BTN_EXTRA: u16 = 0x114;

/// Whether `/dev/uinput` can be opened for writing.
pub(crate) enum Access {
    Ok,
    Missing,
    Denied,
    Failed(String),
}

pub(crate) fn access() -> Access {
    match OpenOptions::new().write(true).open(UINPUT_PATH) {
        Ok(_) => Access::Ok,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Access::Missing,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Access::Denied,
        Err(e) => Access::Failed(e.to_string()),
    }
}

/// Exact instructions for making `/dev/uinput` usable, empty when it already is.
pub(crate) fn access_hint(access: &Access) -> String {
    const RULE: &str = "KERNEL==\"uinput\", SUBSYSTEM==\"misc\", MODE=\"0660\", \
         GROUP=\"input\", OPTIONS+=\"static_node=uinput\", TAG+=\"uaccess\"";
    let setup = format!(
        "Run once in a terminal:\n\
         \n  echo '{RULE}' | sudo tee /etc/udev/rules.d/60-midi-2-keyboard-uinput.rules\n  \
         echo uinput | sudo tee /etc/modules-load.d/midi-2-keyboard-uinput.conf\n  \
         sudo modprobe uinput\n  \
         sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=misc --action=change\n\
         \nthen restart midi-2-keyboard. The rule gives the logged-in desktop user access \
         (\"uaccess\"); on systems without systemd-logind also run \
         `sudo usermod -aG input $USER` and log out and back in."
    );
    match access {
        Access::Ok => String::new(),
        Access::Missing => {
            format!("/dev/uinput does not exist: the uinput kernel module is not loaded. {setup}")
        }
        Access::Denied => format!("midi-2-keyboard may not write to /dev/uinput. {setup}"),
        Access::Failed(error) => format!("/dev/uinput cannot be opened ({error}). {setup}"),
    }
}

pub(crate) struct UinputInjector {
    device: VirtualDevice,
    /// Keys held through this injector.
    down: HashSet<Key>,
    /// Buttons held through this injector.
    buttons: HashSet<MouseButton>,
}

impl UinputInjector {
    pub(crate) fn new() -> std::result::Result<Self, String> {
        let state = access();
        if !matches!(state, Access::Ok) {
            return Err(access_hint(&state));
        }
        let device = build().map_err(|e| format!("could not create the uinput device: {e}"))?;
        std::thread::sleep(SETTLE);
        Ok(Self { device, down: HashSet::new(), buttons: HashSet::new() })
    }

    fn emit(&mut self, events: &[InputEvent]) -> Result<()> {
        self.device.emit(events).map_err(|e| {
            OutputError::Unavailable(format!("writing to the uinput device failed: {e}"))
        })
    }

    fn key_event(&mut self, code: u16, value: i32) -> Result<()> {
        self.emit(&[InputEvent::new(EventType::KEY.0, code, value)])
    }

    fn tap(&mut self, code: u16) -> Result<()> {
        self.key_event(code, 1)?;
        self.key_event(code, 0)
    }
}

fn build() -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in 1..=MAX_KEY_CODE {
        keys.insert(KeyCode(code));
    }
    for button in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA] {
        keys.insert(KeyCode(button));
    }
    let mut axes = AttributeSet::<RelativeAxisCode>::new();
    for axis in [
        RelativeAxisCode::REL_X,
        RelativeAxisCode::REL_Y,
        RelativeAxisCode::REL_WHEEL,
        RelativeAxisCode::REL_HWHEEL,
    ] {
        axes.insert(axis);
    }
    VirtualDevice::builder()?
        .name(DEVICE_NAME)
        .input_id(InputId::new(BusType::BUS_USB, 0x4d32, 0x4b21, 1))
        .with_keys(&keys)?
        .with_relative_axes(&axes)?
        .build()
}

impl Injector for UinputInjector {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports(&self, key: Key) -> bool {
        keymap::evdev(key).is_some_and(|code| code <= MAX_KEY_CODE)
    }

    fn repeats_held_keys(&self) -> bool {
        true
    }

    fn key_down(&mut self, key: Key) -> Result<()> {
        let code = evdev_code(key)?;
        // evdev value 2 is an auto-repeat.
        let value = if self.down.insert(key) { 1 } else { 2 };
        self.key_event(code, value)
    }

    fn key_up(&mut self, key: Key) -> Result<()> {
        let code = evdev_code(key)?;
        self.down.remove(&key);
        self.key_event(code, 0)
    }

    fn text(&mut self, text: &str) -> Result<()> {
        let shift = |key: Key| keymap::evdev(key).unwrap_or_default();
        let (lshift, rshift) = (shift(Key::ShiftLeft), shift(Key::ShiftRight));
        // Shift keys as the caller holds them, and as they currently are.
        let held = (self.down.contains(&Key::ShiftLeft), self.down.contains(&Key::ShiftRight));
        let mut now = held;
        let mut skipped = 0usize;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' && chars.peek() == Some(&'\n') {
                continue; // "\r\n" is one Enter
            }
            let Some((key, needs_shift)) = us_layout(c) else {
                skipped += 1;
                continue;
            };
            let shifted = now.0 || now.1;
            if needs_shift && !shifted {
                self.key_event(lshift, 1)?;
                now.0 = true;
            } else if !needs_shift && shifted {
                if now.0 {
                    self.key_event(lshift, 0)?;
                }
                if now.1 {
                    self.key_event(rshift, 0)?;
                }
                now = (false, false);
            }
            self.tap(evdev_code(key)?)?;
        }
        // Put Shift back the way the caller holds it.
        if now.0 != held.0 {
            self.key_event(lshift, i32::from(held.0))?;
        }
        if now.1 != held.1 {
            self.key_event(rshift, i32::from(held.1))?;
        }
        if skipped > 0 {
            return Err(OutputError::Unsupported(
                format!("typing {skipped} character(s) outside US-layout ASCII"),
                NAME,
            ));
        }
        Ok(())
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let code = match button {
            MouseButton::Left => BTN_LEFT,
            MouseButton::Right => BTN_RIGHT,
            MouseButton::Middle => BTN_MIDDLE,
            MouseButton::Back => BTN_SIDE,
            MouseButton::Forward => BTN_EXTRA,
        };
        if down {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(&button);
        }
        self.key_event(code, i32::from(down))
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut events = Vec::with_capacity(2);
        if dx != 0 {
            events.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, dx));
        }
        if dy != 0 {
            events.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, dy));
        }
        if events.is_empty() { Ok(()) } else { self.emit(&events) }
    }

    fn mouse_move_absolute(&mut self, _x: i32, _y: i32) -> Result<()> {
        Err(OutputError::Unsupported(
            "moving the pointer to a screen position (use relative movement, or the X11 \
             backend on X11 sessions)"
                .into(),
            NAME,
        ))
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        // REL_WHEEL > 0 scrolls up, REL_HWHEEL > 0 scrolls right.
        let mut events = Vec::with_capacity(2);
        if dy != 0 {
            events.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_WHEEL.0, dy));
        }
        if dx != 0 {
            events.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_HWHEEL.0, dx));
        }
        if events.is_empty() { Ok(()) } else { self.emit(&events) }
    }
}

impl Drop for UinputInjector {
    fn drop(&mut self) {
        // The kernel releases everything when the device goes away; do it explicitly anyway so
        // nothing depends on that.
        for key in std::mem::take(&mut self.down) {
            let _ = self.key_up(key);
        }
        for button in std::mem::take(&mut self.buttons) {
            let _ = self.mouse_button(button, false);
        }
    }
}

fn evdev_code(key: Key) -> Result<u16> {
    keymap::evdev(key)
        .filter(|&code| code <= MAX_KEY_CODE)
        .ok_or_else(|| OutputError::Unsupported(key.label(), NAME))
}

/// The key (and whether Shift is needed) that types `c` on a US layout.
fn us_layout(c: char) -> Option<(Key, bool)> {
    let plain = |key| Some((key, false));
    let shifted = |key| Some((key, true));
    if c.is_ascii_lowercase() {
        return plain(letter(c.to_ascii_uppercase()));
    }
    if c.is_ascii_uppercase() {
        return shifted(letter(c));
    }
    match c {
        '\n' | '\r' => plain(Key::Enter),
        '\t' => plain(Key::Tab),
        ' ' => plain(Key::Space),
        '0' => plain(Key::Digit0),
        '1' => plain(Key::Digit1),
        '2' => plain(Key::Digit2),
        '3' => plain(Key::Digit3),
        '4' => plain(Key::Digit4),
        '5' => plain(Key::Digit5),
        '6' => plain(Key::Digit6),
        '7' => plain(Key::Digit7),
        '8' => plain(Key::Digit8),
        '9' => plain(Key::Digit9),
        ')' => shifted(Key::Digit0),
        '!' => shifted(Key::Digit1),
        '@' => shifted(Key::Digit2),
        '#' => shifted(Key::Digit3),
        '$' => shifted(Key::Digit4),
        '%' => shifted(Key::Digit5),
        '^' => shifted(Key::Digit6),
        '&' => shifted(Key::Digit7),
        '*' => shifted(Key::Digit8),
        '(' => shifted(Key::Digit9),
        '`' => plain(Key::Backquote),
        '~' => shifted(Key::Backquote),
        '-' => plain(Key::Minus),
        '_' => shifted(Key::Minus),
        '=' => plain(Key::Equal),
        '+' => shifted(Key::Equal),
        '[' => plain(Key::BracketLeft),
        '{' => shifted(Key::BracketLeft),
        ']' => plain(Key::BracketRight),
        '}' => shifted(Key::BracketRight),
        '\\' => plain(Key::Backslash),
        '|' => shifted(Key::Backslash),
        ';' => plain(Key::Semicolon),
        ':' => shifted(Key::Semicolon),
        '\'' => plain(Key::Quote),
        '"' => shifted(Key::Quote),
        ',' => plain(Key::Comma),
        '<' => shifted(Key::Comma),
        '.' => plain(Key::Period),
        '>' => shifted(Key::Period),
        '/' => plain(Key::Slash),
        '?' => shifted(Key::Slash),
        _ => None,
    }
}

fn letter(upper: char) -> Key {
    match upper {
        'A' => Key::KeyA,
        'B' => Key::KeyB,
        'C' => Key::KeyC,
        'D' => Key::KeyD,
        'E' => Key::KeyE,
        'F' => Key::KeyF,
        'G' => Key::KeyG,
        'H' => Key::KeyH,
        'I' => Key::KeyI,
        'J' => Key::KeyJ,
        'K' => Key::KeyK,
        'L' => Key::KeyL,
        'M' => Key::KeyM,
        'N' => Key::KeyN,
        'O' => Key::KeyO,
        'P' => Key::KeyP,
        'Q' => Key::KeyQ,
        'R' => Key::KeyR,
        'S' => Key::KeyS,
        'T' => Key::KeyT,
        'U' => Key::KeyU,
        'V' => Key::KeyV,
        'W' => Key::KeyW,
        'X' => Key::KeyX,
        'Y' => Key::KeyY,
        _ => Key::KeyZ,
    }
}
