//! The key model. This file is a shared contract (config schema + TypeScript bindings).
//!
//! Keys are named after the W3C UI Events `KeyboardEvent.code` values and denote *physical* keys
//! on a US ANSI layout (`KeyA` is the key that types "a" on a US keyboard). The browser reports
//! exactly these names, which makes key capture in the UI a direct mapping.
//!
//! Only append new variants; never rename or remove (the names are persisted in user configs).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub enum Key {
    // ----- Writing system -----
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Backquote,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    /// The extra key next to left Shift on ISO keyboards.
    IntlBackslash,
    /// JIS "ro" key.
    IntlRo,
    /// JIS yen key.
    IntlYen,

    // ----- Functional -----
    Escape,
    Tab,
    CapsLock,
    Space,
    Enter,
    Backspace,
    ContextMenu,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    /// Windows key / Command / Super (left).
    MetaLeft,
    /// Windows key / Command / Super (right).
    MetaRight,

    // ----- Navigation / editing -----
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    PrintScreen,
    ScrollLock,
    Pause,

    // ----- Function row -----
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,

    // ----- Numpad -----
    NumLock,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadDecimal,
    NumpadEnter,
    NumpadEqual,
    NumpadComma,

    // ----- Media / system -----
    AudioVolumeUp,
    AudioVolumeDown,
    AudioVolumeMute,
    MediaPlayPause,
    MediaStop,
    MediaTrackNext,
    MediaTrackPrevious,
    MediaFastForward,
    MediaRewind,
    MediaSelect,
    Eject,
    BrightnessUp,
    BrightnessDown,
    BrowserBack,
    BrowserForward,
    BrowserRefresh,
    BrowserStop,
    BrowserSearch,
    BrowserFavorites,
    BrowserHome,
    LaunchMail,
    /// "My Computer" / file explorer.
    LaunchApp1,
    /// Calculator.
    LaunchApp2,
    Sleep,
    Power,
    WakeUp,

    // ----- International / IME -----
    Convert,
    NonConvert,
    KanaMode,
    /// Korean Hangul/English toggle, Japanese Kana (Mac).
    Lang1,
    /// Korean Hanja, Japanese Eisu (Mac).
    Lang2,
    Help,
}

impl Key {
    /// Whether this is a modifier key (Shift / Control / Alt / Meta, either side).
    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            Key::ShiftLeft
                | Key::ShiftRight
                | Key::ControlLeft
                | Key::ControlRight
                | Key::AltLeft
                | Key::AltRight
                | Key::MetaLeft
                | Key::MetaRight
        )
    }

    /// The W3C code name, identical to the serialized form (e.g. `"ArrowUp"`).
    pub fn code(self) -> String {
        format!("{self:?}")
    }

    /// A short human label for logs and the CLI (`"A"`, `"1"`, `"Ctrl"`, `"Up"`, `"F13"`,
    /// `"Num +"`). Modifier names follow the platform this is built for (`"Win"` / `"Cmd"` /
    /// `"Super"`, `"Alt"` / `"Option"`). The UI has its own, layout-aware labels.
    pub fn label(self) -> String {
        const META: &str = if cfg!(target_os = "macos") {
            "Cmd"
        } else if cfg!(windows) {
            "Win"
        } else {
            "Super"
        };
        const ALT: &str = if cfg!(target_os = "macos") { "Option" } else { "Alt" };

        let code = self.code();
        if let Some(letter) = code.strip_prefix("Key")
            && letter.len() == 1
        {
            return letter.to_string();
        }
        if let Some(digit) = code.strip_prefix("Digit") {
            return digit.to_string();
        }
        if let Some(digit) = code.strip_prefix("Numpad")
            && digit.len() == 1
        {
            return format!("Num {digit}");
        }
        let label = match self {
            Key::ControlLeft => "Ctrl",
            Key::ControlRight => "Right Ctrl",
            Key::ShiftLeft => "Shift",
            Key::ShiftRight => "Right Shift",
            Key::AltLeft => ALT,
            Key::AltRight => return format!("Right {ALT}"),
            Key::MetaLeft => META,
            Key::MetaRight => return format!("Right {META}"),
            Key::ArrowUp => "Up",
            Key::ArrowDown => "Down",
            Key::ArrowLeft => "Left",
            Key::ArrowRight => "Right",
            Key::Escape => "Esc",
            Key::CapsLock => "Caps Lock",
            Key::ContextMenu => "Menu",
            Key::PageUp => "Page Up",
            Key::PageDown => "Page Down",
            Key::PrintScreen => "Print Screen",
            Key::ScrollLock => "Scroll Lock",
            Key::Backquote => "`",
            Key::Minus => "-",
            Key::Equal => "=",
            Key::BracketLeft => "[",
            Key::BracketRight => "]",
            Key::Backslash => "\\",
            Key::Semicolon => ";",
            Key::Quote => "'",
            Key::Comma => ",",
            Key::Period => ".",
            Key::Slash => "/",
            Key::IntlBackslash => "Intl \\",
            Key::IntlRo => "Ro",
            Key::IntlYen => "Yen",
            Key::NumLock => "Num Lock",
            Key::NumpadAdd => "Num +",
            Key::NumpadSubtract => "Num -",
            Key::NumpadMultiply => "Num *",
            Key::NumpadDivide => "Num /",
            Key::NumpadDecimal => "Num .",
            Key::NumpadEnter => "Num Enter",
            Key::NumpadEqual => "Num =",
            Key::NumpadComma => "Num ,",
            Key::AudioVolumeUp => "Volume Up",
            Key::AudioVolumeDown => "Volume Down",
            Key::AudioVolumeMute => "Mute",
            Key::MediaPlayPause => "Play/Pause",
            Key::MediaStop => "Stop",
            Key::MediaTrackNext => "Next Track",
            Key::MediaTrackPrevious => "Previous Track",
            Key::MediaFastForward => "Fast Forward",
            Key::MediaRewind => "Rewind",
            Key::MediaSelect => "Media Select",
            Key::BrightnessUp => "Brightness Up",
            Key::BrightnessDown => "Brightness Down",
            Key::BrowserBack => "Browser Back",
            Key::BrowserForward => "Browser Forward",
            Key::BrowserRefresh => "Browser Refresh",
            Key::BrowserStop => "Browser Stop",
            Key::BrowserSearch => "Browser Search",
            Key::BrowserFavorites => "Browser Favorites",
            Key::BrowserHome => "Browser Home",
            Key::LaunchMail => "Mail",
            Key::LaunchApp1 => "My Computer",
            Key::LaunchApp2 => "Calculator",
            Key::WakeUp => "Wake Up",
            Key::NonConvert => "Non-Convert",
            Key::KanaMode => "Kana",
            Key::Lang1 => "Lang 1",
            Key::Lang2 => "Lang 2",
            _ => return code,
        };
        label.to_string()
    }
}

/// Mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// "Back" side button.
    Back,
    /// "Forward" side button.
    Forward,
}
