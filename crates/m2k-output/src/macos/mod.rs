//! macOS backend: Quartz events posted to the HID event tap.
//!
//! - Posted modifier keys do not apply to later events by themselves, so the injector tracks
//!   the keys it holds and sets the flags of every event (keyboard, mouse and scroll) itself,
//!   including the left/right device bits. Modifier keys are posted as `FlagsChanged` events
//!   like the real keys; numpad / function / navigation keys carry their usual flags.
//! - A key-down for a key that is already down is marked as auto-repeat.
//! - Volume, media, brightness and eject keys are `NSEventTypeSystemDefined` events
//!   (subtype 8), the way Apple keyboards deliver them.
//! - Text: `CGEventKeyboardSetUnicodeString`, at most 20 UTF-16 units per event without
//!   splitting surrogate pairs, posted without modifier flags; `\n` / `\t` are real keys, also
//!   without modifiers. The events carry the keycode of a letter key the injector does not hold
//!   (applications that track keys by keycode see it go down and up).
//! - Mouse: clicks get a click count (double-clicks work), moves while a button is held are
//!   drag events, relative moves carry their deltas for games. Coordinates are global display
//!   points (the space macOS positions the pointer in), clamped to the attached displays.
//! - Caps Lock is toggled through IOKit (the HID system owns the lock state, a posted key event
//!   would not change it); the posted key event is only the fallback.
//! - Modifiers held through the injector apply to the events it posts, not to the user's own
//!   keyboard and mouse: "pad holds Cmd, click with the real mouse" is a plain click on macOS
//!   (unlike Windows and Linux, where held keys are seat-wide).
//! - Every event carries [`crate::INJECTION_TAG`] in `kCGEventSourceUserData`.
//! - macOS silently drops posted events without the Accessibility permission, and keystrokes
//!   while Secure Event Input is on (a password field or a terminal's Secure Keyboard Entry has
//!   focus). Presses, releases, text and pointer moves then report [`OutputError::Permission`].
//!   A release is checked against an almost fresh answer: one macOS dropped leaves the key down
//!   in the application that got the press, and the error makes the engine retry it until it
//!   gets through. (A release whose press was refused too holds nothing; the engine knows that
//!   from the press's error.)

pub(crate) mod platform;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_application_services::AXIsProcessTrusted;
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGDisplayBounds, CGError, CGEvent, CGEventField, CGEventFilterMask, CGEventFlags,
    CGEventSource, CGEventSourceStateID, CGEventSuppressionState, CGEventTapLocation, CGEventType,
    CGGetActiveDisplayList, CGMouseButton, CGScrollEventUnit,
};

use crate::keymap::{self, MacKey};
use crate::{INJECTION_TAG, Injector, InjectorOptions, Key, MouseButton, OutputError, Result};

const NAME: &str = "macos-cgevent";

/// `CGEventKeyboardSetUnicodeString` takes at most 20 UTF-16 units per event.
const TEXT_CHUNK: usize = 20;

/// How long a cached Accessibility check stays valid for presses, text and pointer input.
const TRUST_TTL: Duration = Duration::from_secs(2);

/// ... and for releases, which must not be reported as delivered when they were not.
const RELEASE_TRUST_TTL: Duration = Duration::from_millis(100);

/// Subtype of the system-defined events that carry the media keys
/// (`NX_SUBTYPE_AUX_CONTROL_BUTTONS`).
const AUX_CONTROL_BUTTONS: i16 = 8;

// Device-dependent modifier bits (IOKit `NX_DEVICE*KEYMASK`): which physical key is held.
const DEVICE_LCTL: u64 = 0x0000_0001;
const DEVICE_LSHIFT: u64 = 0x0000_0002;
const DEVICE_RSHIFT: u64 = 0x0000_0004;
const DEVICE_LCMD: u64 = 0x0000_0008;
const DEVICE_RCMD: u64 = 0x0000_0010;
const DEVICE_LALT: u64 = 0x0000_0020;
const DEVICE_RALT: u64 = 0x0000_0040;
const DEVICE_RCTL: u64 = 0x0000_2000;

/// The event source, created once. Quartz event services may be used from any thread and CF
/// reference counting is thread safe; the injector is `Send` but not `Sync`, so the source is
/// only ever used by one thread at a time.
struct Source(CFRetained<CGEventSource>);

// SAFETY: see above.
unsafe impl Send for Source {}

pub(crate) fn create(_options: &InjectorOptions) -> Result<Box<dyn Injector>> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok_or_else(|| {
        OutputError::Unavailable("macOS refused to create a Quartz event source".into())
    })?;
    let src = Some(&*source);
    CGEventSource::set_user_data(src, i64::from(INJECTION_TAG));
    // Posting must never mute the user's own keyboard and mouse (the default suppresses local
    // hardware events for 0.25 s after each posted event).
    CGEventSource::set_local_events_suppression_interval(src, 0.0);
    let all = CGEventFilterMask::PermitLocalMouseEvents
        | CGEventFilterMask::PermitLocalKeyboardEvents
        | CGEventFilterMask::PermitSystemDefinedEvents;
    for state in [
        CGEventSuppressionState::EventSuppressionStateSuppressionInterval,
        CGEventSuppressionState::EventSuppressionStateRemoteMouseDrag,
    ] {
        CGEventSource::set_local_events_filter_during_suppression_state(src, all, state);
    }
    if !unsafe { AXIsProcessTrusted() } {
        log::warn!("Accessibility permission missing: macOS will drop the injected input");
    }
    Ok(Box::new(CgInjector {
        source: Source(source),
        down: HashSet::new(),
        buttons: 0,
        click: None,
        trusted: None,
        caps_via_iokit: false,
    }))
}

struct Click {
    button: MouseButton,
    at: Instant,
    pos: CGPoint,
    count: i64,
}

pub(crate) struct CgInjector {
    source: Source,
    /// Keys held through this injector (modifiers included).
    down: HashSet<Key>,
    /// Bit per [`MouseButton`] held through this injector.
    buttons: u8,
    /// The last button press, for click counting.
    click: Option<Click>,
    /// Cached `AXIsProcessTrusted` result.
    trusted: Option<(Instant, bool)>,
    /// The last Caps Lock press was applied through IOKit (so its release posts nothing).
    caps_via_iokit: bool,
}

impl CgInjector {
    fn src(&self) -> Option<&CGEventSource> {
        Some(&self.source.0)
    }

    /// Posts `event` (tagged) to the HID event tap.
    fn post(&self, event: &CGEvent) {
        CGEvent::set_integer_value_field(
            Some(event),
            CGEventField::EventSourceUserData,
            i64::from(INJECTION_TAG),
        );
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
    }

    /// `Permission` when macOS drops what we just posted (`keyboard`: also Secure Event
    /// Input). Events are posted regardless, so a wrong answer never costs functionality.
    /// `release`: checked with an almost fresh answer (see the module documentation).
    fn check_delivery(&mut self, keyboard: bool, release: bool) -> Result<()> {
        let max_age = if release { RELEASE_TRUST_TTL } else { TRUST_TTL };
        let now = Instant::now();
        let trusted = match self.trusted {
            Some((at, value)) if now.duration_since(at) < max_age => value,
            _ => {
                let value = unsafe { AXIsProcessTrusted() };
                self.trusted = Some((now, value));
                value
            }
        };
        if !trusted {
            return Err(OutputError::Permission(platform::ACCESSIBILITY_HINT.into()));
        }
        // SAFETY: plain query without arguments.
        if keyboard && unsafe { IsSecureEventInputEnabled() } != 0 {
            return Err(OutputError::Permission(platform::SECURE_INPUT_HINT.into()));
        }
        Ok(())
    }

    /// Flags of the modifier keys currently held.
    fn modifier_flags(&self) -> u64 {
        self.down.iter().filter_map(|&key| modifier_bits(key)).fold(0, |acc, bits| acc | bits)
    }

    /// Caps Lock as macOS currently sees it, so typed letters follow it like real keys do.
    fn caps_lock(&self) -> u64 {
        CGEventSource::flags_state(CGEventSourceStateID::HIDSystemState).0
            & CGEventFlags::MaskAlphaShift.0
    }

    fn post_keycode(&self, key: Key, code: u16, down: bool, repeat: bool) -> Result<()> {
        let event = CGEvent::new_keyboard_event(self.src(), code, down)
            .ok_or_else(|| failed("create a keyboard event"))?;
        let ev = Some(&*event);
        if modifier_bits(key).is_some() || key == Key::CapsLock {
            CGEvent::set_type(ev, CGEventType::FlagsChanged);
        }
        let flags = self.modifier_flags() | key_flags(key) | self.caps_lock();
        CGEvent::set_flags(ev, CGEventFlags(flags));
        if repeat {
            CGEvent::set_integer_value_field(ev, CGEventField::KeyboardEventAutorepeat, 1);
        }
        self.post(&event);
        Ok(())
    }

    fn post_system_key(&self, kind: u32, down: bool, repeat: bool) -> Result<()> {
        autoreleasepool(|_| {
            // NX_KEYDOWN (0xA) / NX_KEYUP (0xB) in both the flags and data1, as the HID driver
            // sends them; bit 0 of data1 marks a repeat.
            let state: isize = if down { 0xA } else { 0xB };
            let data1 = ((kind as isize) << 16) | (state << 8) | isize::from(repeat);
            let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                NSEventType::SystemDefined,
                CGPoint::new(0.0, 0.0),
                NSEventModifierFlags((state as usize) << 8),
                0.0,
                0,
                None,
                AUX_CONTROL_BUTTONS,
                data1,
                -1,
            );
            let cg = event.as_deref().and_then(NSEvent::CGEvent);
            match cg {
                Some(cg) => {
                    self.post(&cg);
                    Ok(())
                }
                None => Err(failed("create a system key event")),
            }
        })
    }

    fn key(&mut self, key: Key, down: bool) -> Result<()> {
        let mac = keymap::macos(key).ok_or_else(|| unsupported(key))?;
        let repeat = down && self.down.contains(&key);
        if down {
            self.down.insert(key);
        } else {
            self.down.remove(&key);
        }
        if key == Key::CapsLock {
            if down && !repeat {
                self.caps_via_iokit = iokit::toggle_caps_lock();
            }
            if self.caps_via_iokit {
                // The lock state flipped through IOKit; the release has nothing to undo.
                return if down { self.check_delivery(false, false) } else { Ok(()) };
            }
        }
        match mac {
            MacKey::Code(code) => self.post_keycode(key, code, down, repeat)?,
            MacKey::System(kind) => self.post_system_key(kind, down, repeat)?,
        }
        self.check_delivery(true, !down)
    }

    /// The keycode text events carry: macOS takes the characters from the event's string, but
    /// applications that track keys by keycode see that key go down and up, so not one the
    /// injector holds.
    fn text_keycode(&self) -> u16 {
        [Key::KeyA, Key::KeyS, Key::KeyD, Key::KeyF]
            .into_iter()
            .filter(|key| !self.down.contains(key))
            .find_map(|key| match keymap::macos(key) {
                Some(MacKey::Code(code)) => Some(code),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// A key typed as part of a text (Enter, Tab): without modifiers, like the characters; a
    /// key the injector holds stays held (released and pressed again).
    fn post_text_key(&self, key: Key) -> Result<()> {
        let Some(MacKey::Code(code)) = keymap::macos(key) else { return Ok(()) };
        let order = if self.down.contains(&key) { [false, true] } else { [true, false] };
        for down in order {
            let event = CGEvent::new_keyboard_event(self.src(), code, down)
                .ok_or_else(|| failed("create a keyboard event"))?;
            CGEvent::set_flags(Some(&event), CGEventFlags(0));
            self.post(&event);
        }
        Ok(())
    }

    /// Posts a chunk of text and clears it.
    fn flush_text(&self, chunk: &mut Vec<u16>) -> Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        let code = self.text_keycode();
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(self.src(), code, down)
                .ok_or_else(|| failed("create a keyboard event"))?;
            // SAFETY: the pointer and length describe `chunk`, alive for the call.
            unsafe {
                CGEvent::keyboard_set_unicode_string(Some(&event), chunk.len() as _, chunk.as_ptr())
            };
            // No modifiers: held Cmd/Ctrl would turn the text into shortcuts.
            CGEvent::set_flags(Some(&event), CGEventFlags(0));
            self.post(&event);
        }
        chunk.clear();
        Ok(())
    }

    fn cursor(&self) -> CGPoint {
        CGEvent::new(self.src())
            .map(|event| CGEvent::location(Some(&event)))
            .unwrap_or(CGPoint::new(0.0, 0.0))
    }

    fn post_move(&self, target: CGPoint, dx: i64, dy: i64) -> Result<()> {
        let other = [MouseButton::Middle, MouseButton::Back, MouseButton::Forward]
            .into_iter()
            .position(|b| self.buttons & button_bit(b) != 0)
            .map(|index| index as u32 + 2); // button numbers 2, 3, 4
        let (kind, button) = if self.buttons & button_bit(MouseButton::Left) != 0 {
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        } else if self.buttons & button_bit(MouseButton::Right) != 0 {
            (CGEventType::RightMouseDragged, CGMouseButton::Right)
        } else if let Some(number) = other {
            (CGEventType::OtherMouseDragged, CGMouseButton(number))
        } else {
            (CGEventType::MouseMoved, CGMouseButton::Left)
        };
        let event = CGEvent::new_mouse_event(self.src(), kind, target, button)
            .ok_or_else(|| failed("create a mouse event"))?;
        let ev = Some(&*event);
        if let (CGEventType::OtherMouseDragged, Some(number)) = (kind, other) {
            CGEvent::set_integer_value_field(
                ev,
                CGEventField::MouseEventButtonNumber,
                i64::from(number),
            );
        }
        CGEvent::set_integer_value_field(ev, CGEventField::MouseEventDeltaX, dx);
        CGEvent::set_integer_value_field(ev, CGEventField::MouseEventDeltaY, dy);
        CGEvent::set_flags(ev, CGEventFlags(self.modifier_flags()));
        self.post(&event);
        Ok(())
    }

    /// Click count for a press: consecutive presses of one button within the system
    /// double-click interval and a few points of each other count up.
    fn press_count(&mut self, button: MouseButton, pos: CGPoint) -> i64 {
        let now = Instant::now();
        let seconds = autoreleasepool(|_| NSEvent::doubleClickInterval());
        let interval = Duration::from_secs_f64(seconds.clamp(0.0, 5.0));
        let count = match &self.click {
            Some(last)
                if last.button == button
                    && now.duration_since(last.at) <= interval
                    && (last.pos.x - pos.x).abs() <= 4.0
                    && (last.pos.y - pos.y).abs() <= 4.0 =>
            {
                last.count + 1
            }
            _ => 1,
        };
        self.click = Some(Click { button, at: now, pos, count });
        count
    }
}

impl Injector for CgInjector {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports(&self, key: Key) -> bool {
        keymap::macos(key).is_some()
    }

    fn key_down(&mut self, key: Key) -> Result<()> {
        self.key(key, true)
    }

    fn key_up(&mut self, key: Key) -> Result<()> {
        self.key(key, false)
    }

    fn text(&mut self, text: &str) -> Result<()> {
        let mut chunk: Vec<u16> = Vec::with_capacity(TEXT_CHUNK);
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            let key = match c {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    Some(Key::Enter)
                }
                '\n' => Some(Key::Enter),
                '\t' => Some(Key::Tab),
                _ => None,
            };
            if let Some(key) = key {
                self.flush_text(&mut chunk)?;
                self.post_text_key(key)?;
                continue;
            }
            let mut units = [0u16; 2];
            let units = c.encode_utf16(&mut units);
            if chunk.len() + units.len() > TEXT_CHUNK {
                self.flush_text(&mut chunk)?;
            }
            chunk.extend_from_slice(units);
        }
        self.flush_text(&mut chunk)?;
        self.check_delivery(true, false)
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let pos = self.cursor();
        let (kind, cg_button, number) = match (button, down) {
            (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left, 0),
            (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left, 0),
            (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right, 1),
            (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right, 1),
            (other, down) => {
                let number = match other {
                    MouseButton::Middle => 2,
                    MouseButton::Back => 3,
                    _ => 4,
                };
                let kind =
                    if down { CGEventType::OtherMouseDown } else { CGEventType::OtherMouseUp };
                (kind, CGMouseButton(number as u32), number)
            }
        };
        let clicks = if down {
            self.press_count(button, pos)
        } else {
            self.click.as_ref().filter(|c| c.button == button).map_or(1, |c| c.count)
        };
        if down {
            self.buttons |= button_bit(button);
        } else {
            self.buttons &= !button_bit(button);
        }
        let event = CGEvent::new_mouse_event(self.src(), kind, pos, cg_button)
            .ok_or_else(|| failed("create a mouse event"))?;
        let ev = Some(&*event);
        CGEvent::set_integer_value_field(ev, CGEventField::MouseEventButtonNumber, number);
        CGEvent::set_integer_value_field(ev, CGEventField::MouseEventClickState, clicks);
        CGEvent::set_flags(ev, CGEventFlags(self.modifier_flags()));
        self.post(&event);
        self.check_delivery(false, !down)
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        let from = self.cursor();
        let target =
            clamp_to_displays(CGPoint::new(from.x + f64::from(dx), from.y + f64::from(dy)));
        self.post_move(target, i64::from(dx), i64::from(dy))?;
        self.check_delivery(false, false)
    }

    fn mouse_move_absolute(&mut self, x: i32, y: i32) -> Result<()> {
        let from = self.cursor();
        let target = clamp_to_displays(CGPoint::new(f64::from(x), f64::from(y)));
        self.post_move(target, (target.x - from.x) as i64, (target.y - from.y) as i64)?;
        self.check_delivery(false, false)
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        // wheel1 > 0 scrolls up; wheel2 > 0 scrolls left. Synthetic events are not affected
        // by the "natural scrolling" preference.
        let event = CGEvent::new_scroll_wheel_event2(
            self.src(),
            CGScrollEventUnit::Line,
            2,
            dy,
            dx.saturating_neg(),
            0,
        )
        .ok_or_else(|| failed("create a scroll event"))?;
        CGEvent::set_flags(Some(&event), CGEventFlags(self.modifier_flags()));
        self.post(&event);
        self.check_delivery(false, false)
    }
}

impl Drop for CgInjector {
    fn drop(&mut self) {
        // Posted presses outlive this process in the window server: release what is still
        // held, ordinary keys before modifiers.
        let mut held: Vec<Key> = self.down.iter().copied().collect();
        held.sort_by_key(|&key| modifier_bits(key).is_some());
        for key in held {
            let _ = self.key(key, false);
        }
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Back,
            MouseButton::Forward,
        ] {
            if self.buttons & button_bit(button) != 0 {
                let _ = self.mouse_button(button, false);
            }
        }
    }
}

/// Device-independent plus device-dependent flag bits of a modifier key.
fn modifier_bits(key: Key) -> Option<u64> {
    Some(match key {
        Key::ShiftLeft => CGEventFlags::MaskShift.0 | DEVICE_LSHIFT,
        Key::ShiftRight => CGEventFlags::MaskShift.0 | DEVICE_RSHIFT,
        Key::ControlLeft => CGEventFlags::MaskControl.0 | DEVICE_LCTL,
        Key::ControlRight => CGEventFlags::MaskControl.0 | DEVICE_RCTL,
        Key::AltLeft => CGEventFlags::MaskAlternate.0 | DEVICE_LALT,
        Key::AltRight => CGEventFlags::MaskAlternate.0 | DEVICE_RALT,
        Key::MetaLeft => CGEventFlags::MaskCommand.0 | DEVICE_LCMD,
        Key::MetaRight => CGEventFlags::MaskCommand.0 | DEVICE_RCMD,
        _ => return None,
    })
}

/// Flags real keyboards set on their own for some keys: keypad keys and arrows carry
/// "numeric pad", function and navigation keys carry "function".
fn key_flags(key: Key) -> u64 {
    let arrow = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight);
    let keypad = matches!(
        key,
        Key::NumLock
            | Key::Numpad0
            | Key::Numpad1
            | Key::Numpad2
            | Key::Numpad3
            | Key::Numpad4
            | Key::Numpad5
            | Key::Numpad6
            | Key::Numpad7
            | Key::Numpad8
            | Key::Numpad9
            | Key::NumpadAdd
            | Key::NumpadSubtract
            | Key::NumpadMultiply
            | Key::NumpadDivide
            | Key::NumpadDecimal
            | Key::NumpadEnter
            | Key::NumpadEqual
            | Key::NumpadComma
    );
    let function = arrow
        || matches!(
            key,
            Key::F1
                | Key::F2
                | Key::F3
                | Key::F4
                | Key::F5
                | Key::F6
                | Key::F7
                | Key::F8
                | Key::F9
                | Key::F10
                | Key::F11
                | Key::F12
                | Key::F13
                | Key::F14
                | Key::F15
                | Key::F16
                | Key::F17
                | Key::F18
                | Key::F19
                | Key::F20
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::Delete
                | Key::Insert
                | Key::Help
        );
    let mut flags = 0;
    if arrow || keypad {
        flags |= CGEventFlags::MaskNumericPad.0;
    }
    if function {
        flags |= CGEventFlags::MaskSecondaryFn.0;
    }
    flags
}

fn button_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
        MouseButton::Back => 8,
        MouseButton::Forward => 16,
    }
}

/// Keeps `p` on an attached display (the nearest one when it is outside all of them).
fn clamp_to_displays(p: CGPoint) -> CGPoint {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: both pointers are valid for the given capacity.
    let error = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success || count == 0 {
        return p;
    }
    let mut best = p;
    let mut best_distance = f64::MAX;
    for &id in &ids[..(count as usize).min(ids.len())] {
        let r = CGDisplayBounds(id);
        if r.size.width < 1.0 || r.size.height < 1.0 {
            continue;
        }
        let x = p.x.clamp(r.origin.x, r.origin.x + r.size.width - 1.0);
        let y = p.y.clamp(r.origin.y, r.origin.y + r.size.height - 1.0);
        let distance = (x - p.x).powi(2) + (y - p.y).powi(2);
        if distance == 0.0 {
            return p;
        }
        if distance < best_distance {
            best_distance = distance;
            best = CGPoint::new(x, y);
        }
    }
    best
}

fn unsupported(key: Key) -> OutputError {
    OutputError::Unsupported(key.label(), NAME)
}

fn failed(what: &str) -> OutputError {
    OutputError::Other(format!("macOS could not {what}"))
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    /// HIToolbox: whether some application holds Secure Event Input (password fields, Secure
    /// Keyboard Entry), during which macOS drops posted keystrokes.
    fn IsSecureEventInputEnabled() -> u8;
}

/// The Caps Lock state lives in the HID system (`IOHIDSystem`); this is how utilities flip it.
mod iokit {
    use std::ffi::{c_char, c_int, c_void};

    type MachPort = u32;

    /// `kIOHIDParamConnectType`
    const PARAM_CONNECT_TYPE: u32 = 1;
    /// `kIOHIDCapsLockState`
    const CAPS_LOCK_STATE: c_int = 1;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        /// Consumes `matching`.
        fn IOServiceGetMatchingService(main_port: MachPort, matching: *mut c_void) -> MachPort;
        fn IOServiceOpen(
            service: MachPort,
            owning_task: MachPort,
            kind: u32,
            connect: *mut MachPort,
        ) -> c_int;
        fn IOServiceClose(connect: MachPort) -> c_int;
        fn IOObjectRelease(object: MachPort) -> c_int;
        fn IOHIDGetModifierLockState(handle: MachPort, selector: c_int, state: *mut bool) -> c_int;
        fn IOHIDSetModifierLockState(handle: MachPort, selector: c_int, state: bool) -> c_int;
    }

    unsafe extern "C" {
        /// What the `mach_task_self()` macro reads.
        static mach_task_self_: MachPort;
    }

    /// Flips Caps Lock (lock state and LED). `false` when IOKit refuses.
    pub(super) fn toggle_caps_lock() -> bool {
        // SAFETY: plain IOKit calls; every object obtained is released below.
        unsafe {
            let matching = IOServiceMatching(c"IOHIDSystem".as_ptr());
            if matching.is_null() {
                return false;
            }
            let service = IOServiceGetMatchingService(0, matching);
            if service == 0 {
                return false;
            }
            let mut connect: MachPort = 0;
            let opened = IOServiceOpen(service, mach_task_self_, PARAM_CONNECT_TYPE, &mut connect);
            IOObjectRelease(service);
            if opened != 0 {
                return false;
            }
            let mut on = false;
            let toggled = IOHIDGetModifierLockState(connect, CAPS_LOCK_STATE, &mut on) == 0
                && IOHIDSetModifierLockState(connect, CAPS_LOCK_STATE, !on) == 0;
            IOServiceClose(connect);
            toggled
        }
    }
}
