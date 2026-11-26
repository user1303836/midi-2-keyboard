//! Per-platform code tables for every [`Key`].
//!
//! All three tables are compiled on every platform (the matches are exhaustive), so adding a
//! `Key` variant fails to build until it has been placed in each of them. `None` means the key
//! cannot be expressed on that platform; the backends report it through `supports()` and
//! `OutputError::Unsupported`.
//!
//! Sources: USB HID usage tables, Chromium's `dom_code_data.inc` (USB / evdev / Windows scan code
//! / macOS keycode per `KeyboardEvent.code`), `WinUser.h`, HIToolbox `Events.h`,
//! IOKit `ev_keymap.h` and `linux/input-event-codes.h`.

use crate::Key;

// ---------------------------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------------------------

/// How a key is sent with `SendInput`.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WinKey {
    /// Virtual-key code. For `layout` keys this is the US-layout value, used only when the
    /// active layout does not map the scan code.
    pub vk: u16,
    /// Set-1 make code without the `E0` prefix (`extended` says whether it has one).
    pub scan: u16,
    /// The scan code carries the `E0` prefix (`KEYEVENTF_EXTENDEDKEY`).
    pub extended: bool,
    /// The virtual-key code depends on the keyboard layout (writing-system and IME keys).
    /// The backend asks the layout of the foreground window which VK the physical key
    /// produces, so `KeyQ` types "a" on AZERTY exactly like the physical key does.
    pub layout: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl WinKey {
    /// The scan code in the `0xE0xx` form `MapVirtualKeyEx` understands.
    pub fn scan_ex(self) -> u32 {
        if self.extended { 0xE000 | u32::from(self.scan) } else { u32::from(self.scan) }
    }
}

/// Layout-independent key.
#[cfg_attr(not(windows), allow(dead_code))]
const fn w(vk: u16, scan: u16) -> Option<WinKey> {
    Some(WinKey { vk, scan, extended: false, layout: false })
}
/// Layout-independent key whose scan code has the `E0` prefix.
#[cfg_attr(not(windows), allow(dead_code))]
const fn wx(vk: u16, scan: u16) -> Option<WinKey> {
    Some(WinKey { vk, scan, extended: true, layout: false })
}
/// Key whose virtual-key code comes from the active layout (`vk` is the US fallback).
#[cfg_attr(not(windows), allow(dead_code))]
const fn wl(vk: u16, scan: u16) -> Option<WinKey> {
    Some(WinKey { vk, scan, extended: false, layout: true })
}

/// Virtual-key codes (`WinUser.h`). Letters and digits are their ASCII upper-case values.
#[allow(dead_code)]
pub(crate) mod vk {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const PAUSE: u16 = 0x13;
    pub const CAPITAL: u16 = 0x14;
    /// Also `VK_HANGUL`.
    pub const KANA: u16 = 0x15;
    /// Also `VK_KANJI`.
    pub const HANJA: u16 = 0x19;
    pub const ESCAPE: u16 = 0x1B;
    pub const CONVERT: u16 = 0x1C;
    pub const NONCONVERT: u16 = 0x1D;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const SNAPSHOT: u16 = 0x2C;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const HELP: u16 = 0x2F;
    pub const LWIN: u16 = 0x5B;
    pub const RWIN: u16 = 0x5C;
    pub const APPS: u16 = 0x5D;
    pub const SLEEP: u16 = 0x5F;
    pub const NUMPAD0: u16 = 0x60;
    pub const MULTIPLY: u16 = 0x6A;
    pub const ADD: u16 = 0x6B;
    pub const SUBTRACT: u16 = 0x6D;
    pub const DECIMAL: u16 = 0x6E;
    pub const DIVIDE: u16 = 0x6F;
    pub const F1: u16 = 0x70;
    pub const F13: u16 = 0x7C;
    pub const NUMLOCK: u16 = 0x90;
    pub const SCROLL: u16 = 0x91;
    pub const LSHIFT: u16 = 0xA0;
    pub const RSHIFT: u16 = 0xA1;
    pub const LCONTROL: u16 = 0xA2;
    pub const RCONTROL: u16 = 0xA3;
    pub const LMENU: u16 = 0xA4;
    pub const RMENU: u16 = 0xA5;
    pub const BROWSER_BACK: u16 = 0xA6;
    pub const BROWSER_FORWARD: u16 = 0xA7;
    pub const BROWSER_REFRESH: u16 = 0xA8;
    pub const BROWSER_STOP: u16 = 0xA9;
    pub const BROWSER_SEARCH: u16 = 0xAA;
    pub const BROWSER_FAVORITES: u16 = 0xAB;
    pub const BROWSER_HOME: u16 = 0xAC;
    pub const VOLUME_MUTE: u16 = 0xAD;
    pub const VOLUME_DOWN: u16 = 0xAE;
    pub const VOLUME_UP: u16 = 0xAF;
    pub const MEDIA_NEXT_TRACK: u16 = 0xB0;
    pub const MEDIA_PREV_TRACK: u16 = 0xB1;
    pub const MEDIA_STOP: u16 = 0xB2;
    pub const MEDIA_PLAY_PAUSE: u16 = 0xB3;
    pub const LAUNCH_MAIL: u16 = 0xB4;
    pub const LAUNCH_MEDIA_SELECT: u16 = 0xB5;
    pub const LAUNCH_APP1: u16 = 0xB6;
    pub const LAUNCH_APP2: u16 = 0xB7;
    pub const OEM_1: u16 = 0xBA;
    pub const OEM_PLUS: u16 = 0xBB;
    pub const OEM_COMMA: u16 = 0xBC;
    pub const OEM_MINUS: u16 = 0xBD;
    pub const OEM_PERIOD: u16 = 0xBE;
    pub const OEM_2: u16 = 0xBF;
    pub const OEM_3: u16 = 0xC0;
    pub const ABNT_C1: u16 = 0xC1;
    pub const ABNT_C2: u16 = 0xC2;
    pub const OEM_4: u16 = 0xDB;
    pub const OEM_5: u16 = 0xDC;
    pub const OEM_6: u16 = 0xDD;
    pub const OEM_7: u16 = 0xDE;
    pub const OEM_102: u16 = 0xE2;
}

/// Windows virtual-key / scan code for `key`.
///
/// Notes on the non-obvious entries:
/// - Navigation keys, right Ctrl/Alt, the Meta keys, ContextMenu, NumpadDivide, NumpadEnter,
///   PrintScreen and the media/browser/launch keys carry the `E0` prefix.
/// - Pause has no single-scan-code form (`E1 1D 45`); it is always sent by virtual key with the
///   scan code Windows itself reports for it (0x45, not extended).
/// - NumLock is 0x45 *without* prefix (Windows reports it as extended in `lParam`, but it is the
///   un-prefixed make code that the layout maps to `VK_NUMLOCK`).
/// - Numpad digits use `VK_NUMPAD*`, so they type digits regardless of the NumLock state.
/// - F13–F23 are 0x64–0x6E and F24 is 0x76.
/// - No virtual key exists for brightness, fast-forward/rewind, eject, power, wake-up and the
///   keypad "=".
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn windows(key: Key) -> Option<WinKey> {
    use vk::*;
    let letter = |c: u8| c as u16;
    match key {
        // ----- Writing system -----
        Key::KeyA => wl(letter(b'A'), 0x1E),
        Key::KeyB => wl(letter(b'B'), 0x30),
        Key::KeyC => wl(letter(b'C'), 0x2E),
        Key::KeyD => wl(letter(b'D'), 0x20),
        Key::KeyE => wl(letter(b'E'), 0x12),
        Key::KeyF => wl(letter(b'F'), 0x21),
        Key::KeyG => wl(letter(b'G'), 0x22),
        Key::KeyH => wl(letter(b'H'), 0x23),
        Key::KeyI => wl(letter(b'I'), 0x17),
        Key::KeyJ => wl(letter(b'J'), 0x24),
        Key::KeyK => wl(letter(b'K'), 0x25),
        Key::KeyL => wl(letter(b'L'), 0x26),
        Key::KeyM => wl(letter(b'M'), 0x32),
        Key::KeyN => wl(letter(b'N'), 0x31),
        Key::KeyO => wl(letter(b'O'), 0x18),
        Key::KeyP => wl(letter(b'P'), 0x19),
        Key::KeyQ => wl(letter(b'Q'), 0x10),
        Key::KeyR => wl(letter(b'R'), 0x13),
        Key::KeyS => wl(letter(b'S'), 0x1F),
        Key::KeyT => wl(letter(b'T'), 0x14),
        Key::KeyU => wl(letter(b'U'), 0x16),
        Key::KeyV => wl(letter(b'V'), 0x2F),
        Key::KeyW => wl(letter(b'W'), 0x11),
        Key::KeyX => wl(letter(b'X'), 0x2D),
        Key::KeyY => wl(letter(b'Y'), 0x15),
        Key::KeyZ => wl(letter(b'Z'), 0x2C),
        Key::Digit1 => wl(letter(b'1'), 0x02),
        Key::Digit2 => wl(letter(b'2'), 0x03),
        Key::Digit3 => wl(letter(b'3'), 0x04),
        Key::Digit4 => wl(letter(b'4'), 0x05),
        Key::Digit5 => wl(letter(b'5'), 0x06),
        Key::Digit6 => wl(letter(b'6'), 0x07),
        Key::Digit7 => wl(letter(b'7'), 0x08),
        Key::Digit8 => wl(letter(b'8'), 0x09),
        Key::Digit9 => wl(letter(b'9'), 0x0A),
        Key::Digit0 => wl(letter(b'0'), 0x0B),
        Key::Backquote => wl(OEM_3, 0x29),
        Key::Minus => wl(OEM_MINUS, 0x0C),
        Key::Equal => wl(OEM_PLUS, 0x0D),
        Key::BracketLeft => wl(OEM_4, 0x1A),
        Key::BracketRight => wl(OEM_6, 0x1B),
        Key::Backslash => wl(OEM_5, 0x2B),
        Key::Semicolon => wl(OEM_1, 0x27),
        Key::Quote => wl(OEM_7, 0x28),
        Key::Comma => wl(OEM_COMMA, 0x33),
        Key::Period => wl(OEM_PERIOD, 0x34),
        Key::Slash => wl(OEM_2, 0x35),
        Key::IntlBackslash => wl(OEM_102, 0x56),
        Key::IntlRo => wl(ABNT_C1, 0x73),
        Key::IntlYen => wl(OEM_5, 0x7D),

        // ----- Functional -----
        Key::Escape => w(ESCAPE, 0x01),
        Key::Tab => w(TAB, 0x0F),
        Key::CapsLock => w(CAPITAL, 0x3A),
        Key::Space => w(SPACE, 0x39),
        Key::Enter => w(RETURN, 0x1C),
        Key::Backspace => w(BACK, 0x0E),
        Key::ContextMenu => wx(APPS, 0x5D),
        Key::ShiftLeft => w(LSHIFT, 0x2A),
        Key::ShiftRight => w(RSHIFT, 0x36),
        Key::ControlLeft => w(LCONTROL, 0x1D),
        Key::ControlRight => wx(RCONTROL, 0x1D),
        Key::AltLeft => w(LMENU, 0x38),
        Key::AltRight => wx(RMENU, 0x38),
        Key::MetaLeft => wx(LWIN, 0x5B),
        Key::MetaRight => wx(RWIN, 0x5C),

        // ----- Navigation / editing -----
        Key::Insert => wx(INSERT, 0x52),
        Key::Delete => wx(DELETE, 0x53),
        Key::Home => wx(HOME, 0x47),
        Key::End => wx(END, 0x4F),
        Key::PageUp => wx(PRIOR, 0x49),
        Key::PageDown => wx(NEXT, 0x51),
        Key::ArrowUp => wx(UP, 0x48),
        Key::ArrowDown => wx(DOWN, 0x50),
        Key::ArrowLeft => wx(LEFT, 0x4B),
        Key::ArrowRight => wx(RIGHT, 0x4D),
        Key::PrintScreen => wx(SNAPSHOT, 0x37),
        Key::ScrollLock => w(SCROLL, 0x46),
        Key::Pause => w(PAUSE, 0x45),

        // ----- Function row -----
        Key::F1 => w(F1, 0x3B),
        Key::F2 => w(F1 + 1, 0x3C),
        Key::F3 => w(F1 + 2, 0x3D),
        Key::F4 => w(F1 + 3, 0x3E),
        Key::F5 => w(F1 + 4, 0x3F),
        Key::F6 => w(F1 + 5, 0x40),
        Key::F7 => w(F1 + 6, 0x41),
        Key::F8 => w(F1 + 7, 0x42),
        Key::F9 => w(F1 + 8, 0x43),
        Key::F10 => w(F1 + 9, 0x44),
        Key::F11 => w(F1 + 10, 0x57),
        Key::F12 => w(F1 + 11, 0x58),
        Key::F13 => w(F13, 0x64),
        Key::F14 => w(F13 + 1, 0x65),
        Key::F15 => w(F13 + 2, 0x66),
        Key::F16 => w(F13 + 3, 0x67),
        Key::F17 => w(F13 + 4, 0x68),
        Key::F18 => w(F13 + 5, 0x69),
        Key::F19 => w(F13 + 6, 0x6A),
        Key::F20 => w(F13 + 7, 0x6B),
        Key::F21 => w(F13 + 8, 0x6C),
        Key::F22 => w(F13 + 9, 0x6D),
        Key::F23 => w(F13 + 10, 0x6E),
        Key::F24 => w(F13 + 11, 0x76),

        // ----- Numpad -----
        Key::NumLock => w(NUMLOCK, 0x45),
        Key::Numpad0 => w(NUMPAD0, 0x52),
        Key::Numpad1 => w(NUMPAD0 + 1, 0x4F),
        Key::Numpad2 => w(NUMPAD0 + 2, 0x50),
        Key::Numpad3 => w(NUMPAD0 + 3, 0x51),
        Key::Numpad4 => w(NUMPAD0 + 4, 0x4B),
        Key::Numpad5 => w(NUMPAD0 + 5, 0x4C),
        Key::Numpad6 => w(NUMPAD0 + 6, 0x4D),
        Key::Numpad7 => w(NUMPAD0 + 7, 0x47),
        Key::Numpad8 => w(NUMPAD0 + 8, 0x48),
        Key::Numpad9 => w(NUMPAD0 + 9, 0x49),
        Key::NumpadAdd => w(ADD, 0x4E),
        Key::NumpadSubtract => w(SUBTRACT, 0x4A),
        Key::NumpadMultiply => w(MULTIPLY, 0x37),
        Key::NumpadDivide => wx(DIVIDE, 0x35),
        Key::NumpadDecimal => w(DECIMAL, 0x53),
        Key::NumpadEnter => wx(RETURN, 0x1C),
        // Windows layouts have no keypad "=": scan 0x59 is VK_CLEAR on US and turns into
        // VK_NUMPAD5 with NumLock on, so it would type "5".
        Key::NumpadEqual => None,
        Key::NumpadComma => wl(ABNT_C2, 0x7E),

        // ----- Media / system -----
        Key::AudioVolumeUp => wx(VOLUME_UP, 0x30),
        Key::AudioVolumeDown => wx(VOLUME_DOWN, 0x2E),
        Key::AudioVolumeMute => wx(VOLUME_MUTE, 0x20),
        Key::MediaPlayPause => wx(MEDIA_PLAY_PAUSE, 0x22),
        Key::MediaStop => wx(MEDIA_STOP, 0x24),
        Key::MediaTrackNext => wx(MEDIA_NEXT_TRACK, 0x19),
        Key::MediaTrackPrevious => wx(MEDIA_PREV_TRACK, 0x10),
        Key::MediaFastForward | Key::MediaRewind => None,
        Key::MediaSelect => wx(LAUNCH_MEDIA_SELECT, 0x6D),
        Key::Eject => None,
        Key::BrightnessUp | Key::BrightnessDown => None,
        Key::BrowserBack => wx(BROWSER_BACK, 0x6A),
        Key::BrowserForward => wx(BROWSER_FORWARD, 0x69),
        Key::BrowserRefresh => wx(BROWSER_REFRESH, 0x67),
        Key::BrowserStop => wx(BROWSER_STOP, 0x68),
        Key::BrowserSearch => wx(BROWSER_SEARCH, 0x65),
        Key::BrowserFavorites => wx(BROWSER_FAVORITES, 0x66),
        Key::BrowserHome => wx(BROWSER_HOME, 0x32),
        Key::LaunchMail => wx(LAUNCH_MAIL, 0x6C),
        Key::LaunchApp1 => wx(LAUNCH_APP1, 0x6B),
        Key::LaunchApp2 => wx(LAUNCH_APP2, 0x21),
        Key::Sleep => wx(SLEEP, 0x5F),
        Key::Power | Key::WakeUp => None,

        // ----- International / IME -----
        Key::Convert => wl(CONVERT, 0x79),
        Key::NonConvert => wl(NONCONVERT, 0x7B),
        Key::KanaMode => wl(KANA, 0x70),
        Key::Lang1 => wl(KANA, 0x72),
        Key::Lang2 => wl(HANJA, 0x71),
        Key::Help => wx(HELP, 0x3B),
    }
}

// ---------------------------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------------------------

/// How a key is posted on macOS.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacKey {
    /// A virtual keycode (`kVK_*`) posted as a keyboard event.
    Code(u16),
    /// An `NX_KEYTYPE_*` posted as an `NSEventTypeSystemDefined` event (subtype 8). This is how
    /// the media, volume and brightness keys of Apple keyboards arrive; posting their `kVK_*`
    /// keycodes as keyboard events does not change the volume.
    System(u32),
}

/// macOS keycode or system key type for `key`. Keys absent from Apple keyboards (PrintScreen,
/// ScrollLock, Pause, F21–F24, the browser/launch keys, Japanese conversion keys) have none.
/// Insert maps to `kVK_Help`, which is what a PC keyboard's Insert key produces on a Mac, and
/// NumLock to `kVK_ANSI_KeypadClear`, which sits in its place on Apple keypads.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn macos(key: Key) -> Option<MacKey> {
    let code = |c: u16| Some(MacKey::Code(c));
    let system = |t: u32| Some(MacKey::System(t));
    match key {
        Key::KeyA => code(0x00),
        Key::KeyB => code(0x0B),
        Key::KeyC => code(0x08),
        Key::KeyD => code(0x02),
        Key::KeyE => code(0x0E),
        Key::KeyF => code(0x03),
        Key::KeyG => code(0x05),
        Key::KeyH => code(0x04),
        Key::KeyI => code(0x22),
        Key::KeyJ => code(0x26),
        Key::KeyK => code(0x28),
        Key::KeyL => code(0x25),
        Key::KeyM => code(0x2E),
        Key::KeyN => code(0x2D),
        Key::KeyO => code(0x1F),
        Key::KeyP => code(0x23),
        Key::KeyQ => code(0x0C),
        Key::KeyR => code(0x0F),
        Key::KeyS => code(0x01),
        Key::KeyT => code(0x11),
        Key::KeyU => code(0x20),
        Key::KeyV => code(0x09),
        Key::KeyW => code(0x0D),
        Key::KeyX => code(0x07),
        Key::KeyY => code(0x10),
        Key::KeyZ => code(0x06),
        Key::Digit1 => code(0x12),
        Key::Digit2 => code(0x13),
        Key::Digit3 => code(0x14),
        Key::Digit4 => code(0x15),
        Key::Digit5 => code(0x17),
        Key::Digit6 => code(0x16),
        Key::Digit7 => code(0x1A),
        Key::Digit8 => code(0x1C),
        Key::Digit9 => code(0x19),
        Key::Digit0 => code(0x1D),
        Key::Backquote => code(0x32),
        Key::Minus => code(0x1B),
        Key::Equal => code(0x18),
        Key::BracketLeft => code(0x21),
        Key::BracketRight => code(0x1E),
        Key::Backslash => code(0x2A),
        Key::Semicolon => code(0x29),
        Key::Quote => code(0x27),
        Key::Comma => code(0x2B),
        Key::Period => code(0x2F),
        Key::Slash => code(0x2C),
        Key::IntlBackslash => code(0x0A), // kVK_ISO_Section
        Key::IntlRo => code(0x5E),        // kVK_JIS_Underscore
        Key::IntlYen => code(0x5D),       // kVK_JIS_Yen

        Key::Escape => code(0x35),
        Key::Tab => code(0x30),
        Key::CapsLock => code(0x39),
        Key::Space => code(0x31),
        Key::Enter => code(0x24),
        Key::Backspace => code(0x33), // kVK_Delete
        Key::ContextMenu => code(0x6E),
        Key::ShiftLeft => code(0x38),
        Key::ShiftRight => code(0x3C),
        Key::ControlLeft => code(0x3B),
        Key::ControlRight => code(0x3E),
        Key::AltLeft => code(0x3A),
        Key::AltRight => code(0x3D),
        Key::MetaLeft => code(0x37),
        Key::MetaRight => code(0x36),

        Key::Insert => code(0x72), // kVK_Help
        Key::Delete => code(0x75), // kVK_ForwardDelete
        Key::Home => code(0x73),
        Key::End => code(0x77),
        Key::PageUp => code(0x74),
        Key::PageDown => code(0x79),
        Key::ArrowUp => code(0x7E),
        Key::ArrowDown => code(0x7D),
        Key::ArrowLeft => code(0x7B),
        Key::ArrowRight => code(0x7C),
        Key::PrintScreen | Key::ScrollLock | Key::Pause => None,

        Key::F1 => code(0x7A),
        Key::F2 => code(0x78),
        Key::F3 => code(0x63),
        Key::F4 => code(0x76),
        Key::F5 => code(0x60),
        Key::F6 => code(0x61),
        Key::F7 => code(0x62),
        Key::F8 => code(0x64),
        Key::F9 => code(0x65),
        Key::F10 => code(0x6D),
        Key::F11 => code(0x67),
        Key::F12 => code(0x6F),
        Key::F13 => code(0x69),
        Key::F14 => code(0x6B),
        Key::F15 => code(0x71),
        Key::F16 => code(0x6A),
        Key::F17 => code(0x40),
        Key::F18 => code(0x4F),
        Key::F19 => code(0x50),
        Key::F20 => code(0x5A),
        Key::F21 | Key::F22 | Key::F23 | Key::F24 => None,

        Key::NumLock => code(0x47), // kVK_ANSI_KeypadClear
        Key::Numpad0 => code(0x52),
        Key::Numpad1 => code(0x53),
        Key::Numpad2 => code(0x54),
        Key::Numpad3 => code(0x55),
        Key::Numpad4 => code(0x56),
        Key::Numpad5 => code(0x57),
        Key::Numpad6 => code(0x58),
        Key::Numpad7 => code(0x59),
        Key::Numpad8 => code(0x5B),
        Key::Numpad9 => code(0x5C),
        Key::NumpadAdd => code(0x45),
        Key::NumpadSubtract => code(0x4E),
        Key::NumpadMultiply => code(0x43),
        Key::NumpadDivide => code(0x4B),
        Key::NumpadDecimal => code(0x41),
        Key::NumpadEnter => code(0x4C),
        Key::NumpadEqual => code(0x51),
        Key::NumpadComma => code(0x5F), // kVK_JIS_KeypadComma

        Key::AudioVolumeUp => system(0),       // NX_KEYTYPE_SOUND_UP
        Key::AudioVolumeDown => system(1),     // NX_KEYTYPE_SOUND_DOWN
        Key::AudioVolumeMute => system(7),     // NX_KEYTYPE_MUTE
        Key::MediaPlayPause => system(16),     // NX_KEYTYPE_PLAY
        Key::MediaTrackNext => system(17),     // NX_KEYTYPE_NEXT
        Key::MediaTrackPrevious => system(18), // NX_KEYTYPE_PREVIOUS
        Key::MediaFastForward => system(19),   // NX_KEYTYPE_FAST
        Key::MediaRewind => system(20),        // NX_KEYTYPE_REWIND
        Key::Eject => system(14),              // NX_KEYTYPE_EJECT
        Key::BrightnessUp => system(2),        // NX_KEYTYPE_BRIGHTNESS_UP
        Key::BrightnessDown => system(3),      // NX_KEYTYPE_BRIGHTNESS_DOWN
        Key::MediaStop | Key::MediaSelect => None,
        Key::BrowserBack
        | Key::BrowserForward
        | Key::BrowserRefresh
        | Key::BrowserStop
        | Key::BrowserSearch
        | Key::BrowserFavorites
        | Key::BrowserHome
        | Key::LaunchMail
        | Key::LaunchApp1
        | Key::LaunchApp2 => None,
        Key::Sleep | Key::Power | Key::WakeUp => None,

        Key::Convert | Key::NonConvert | Key::KanaMode => None,
        Key::Lang1 => code(0x68), // kVK_JIS_Kana
        Key::Lang2 => code(0x66), // kVK_JIS_Eisu
        Key::Help => code(0x72),
    }
}

// ---------------------------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------------------------

/// Linux evdev key code (`KEY_*` from `input-event-codes.h`). X11 keycodes are this plus 8.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn evdev(key: Key) -> Option<u16> {
    let code = match key {
        Key::KeyA => 30,
        Key::KeyB => 48,
        Key::KeyC => 46,
        Key::KeyD => 32,
        Key::KeyE => 18,
        Key::KeyF => 33,
        Key::KeyG => 34,
        Key::KeyH => 35,
        Key::KeyI => 23,
        Key::KeyJ => 36,
        Key::KeyK => 37,
        Key::KeyL => 38,
        Key::KeyM => 50,
        Key::KeyN => 49,
        Key::KeyO => 24,
        Key::KeyP => 25,
        Key::KeyQ => 16,
        Key::KeyR => 19,
        Key::KeyS => 31,
        Key::KeyT => 20,
        Key::KeyU => 22,
        Key::KeyV => 47,
        Key::KeyW => 17,
        Key::KeyX => 45,
        Key::KeyY => 21,
        Key::KeyZ => 44,
        Key::Digit1 => 2,
        Key::Digit2 => 3,
        Key::Digit3 => 4,
        Key::Digit4 => 5,
        Key::Digit5 => 6,
        Key::Digit6 => 7,
        Key::Digit7 => 8,
        Key::Digit8 => 9,
        Key::Digit9 => 10,
        Key::Digit0 => 11,
        Key::Backquote => 41,     // KEY_GRAVE
        Key::Minus => 12,         // KEY_MINUS
        Key::Equal => 13,         // KEY_EQUAL
        Key::BracketLeft => 26,   // KEY_LEFTBRACE
        Key::BracketRight => 27,  // KEY_RIGHTBRACE
        Key::Backslash => 43,     // KEY_BACKSLASH
        Key::Semicolon => 39,     // KEY_SEMICOLON
        Key::Quote => 40,         // KEY_APOSTROPHE
        Key::Comma => 51,         // KEY_COMMA
        Key::Period => 52,        // KEY_DOT
        Key::Slash => 53,         // KEY_SLASH
        Key::IntlBackslash => 86, // KEY_102ND
        Key::IntlRo => 89,        // KEY_RO
        Key::IntlYen => 124,      // KEY_YEN

        Key::Escape => 1,
        Key::Tab => 15,
        Key::CapsLock => 58,
        Key::Space => 57,
        Key::Enter => 28,
        Key::Backspace => 14,
        Key::ContextMenu => 127, // KEY_COMPOSE (the PC "Application" key)
        Key::ShiftLeft => 42,
        Key::ShiftRight => 54,
        Key::ControlLeft => 29,
        Key::ControlRight => 97,
        Key::AltLeft => 56,
        Key::AltRight => 100,
        Key::MetaLeft => 125,
        Key::MetaRight => 126,

        Key::Insert => 110,
        Key::Delete => 111,
        Key::Home => 102,
        Key::End => 107,
        Key::PageUp => 104,
        Key::PageDown => 109,
        Key::ArrowUp => 103,
        Key::ArrowDown => 108,
        Key::ArrowLeft => 105,
        Key::ArrowRight => 106,
        Key::PrintScreen => 99, // KEY_SYSRQ
        Key::ScrollLock => 70,
        Key::Pause => 119,

        Key::F1 => 59,
        Key::F2 => 60,
        Key::F3 => 61,
        Key::F4 => 62,
        Key::F5 => 63,
        Key::F6 => 64,
        Key::F7 => 65,
        Key::F8 => 66,
        Key::F9 => 67,
        Key::F10 => 68,
        Key::F11 => 87,
        Key::F12 => 88,
        Key::F13 => 183,
        Key::F14 => 184,
        Key::F15 => 185,
        Key::F16 => 186,
        Key::F17 => 187,
        Key::F18 => 188,
        Key::F19 => 189,
        Key::F20 => 190,
        Key::F21 => 191,
        Key::F22 => 192,
        Key::F23 => 193,
        Key::F24 => 194,

        Key::NumLock => 69,
        Key::Numpad0 => 82,
        Key::Numpad1 => 79,
        Key::Numpad2 => 80,
        Key::Numpad3 => 81,
        Key::Numpad4 => 75,
        Key::Numpad5 => 76,
        Key::Numpad6 => 77,
        Key::Numpad7 => 71,
        Key::Numpad8 => 72,
        Key::Numpad9 => 73,
        Key::NumpadAdd => 78,
        Key::NumpadSubtract => 74,
        Key::NumpadMultiply => 55,
        Key::NumpadDivide => 98,
        Key::NumpadDecimal => 83,
        Key::NumpadEnter => 96,
        Key::NumpadEqual => 117,
        Key::NumpadComma => 121,

        Key::AudioVolumeUp => 115,
        Key::AudioVolumeDown => 114,
        Key::AudioVolumeMute => 113,
        Key::MediaPlayPause => 164,
        Key::MediaStop => 166,          // KEY_STOPCD
        Key::MediaTrackNext => 163,     // KEY_NEXTSONG
        Key::MediaTrackPrevious => 165, // KEY_PREVIOUSSONG
        Key::MediaFastForward => 208,
        Key::MediaRewind => 168,
        Key::MediaSelect => 171, // KEY_CONFIG (HID AL Consumer Control Configuration)
        Key::Eject => 161,       // KEY_EJECTCD
        Key::BrightnessUp => 225,
        Key::BrightnessDown => 224,
        Key::BrowserBack => 158,      // KEY_BACK
        Key::BrowserForward => 159,   // KEY_FORWARD
        Key::BrowserRefresh => 173,   // KEY_REFRESH
        Key::BrowserStop => 128,      // KEY_STOP
        Key::BrowserSearch => 217,    // KEY_SEARCH
        Key::BrowserFavorites => 156, // KEY_BOOKMARKS
        Key::BrowserHome => 172,      // KEY_HOMEPAGE
        Key::LaunchMail => 155,       // KEY_MAIL
        Key::LaunchApp1 => 144,       // KEY_FILE (HID AL Local Machine Browser)
        Key::LaunchApp2 => 140,       // KEY_CALC
        Key::Sleep => 142,
        Key::Power => 116,
        Key::WakeUp => 143,

        Key::Convert => 92,    // KEY_HENKAN
        Key::NonConvert => 94, // KEY_MUHENKAN
        Key::KanaMode => 93,   // KEY_KATAKANAHIRAGANA
        Key::Lang1 => 122,     // KEY_HANGEUL
        Key::Lang2 => 123,     // KEY_HANJA
        Key::Help => 138,
    };
    Some(code)
}
