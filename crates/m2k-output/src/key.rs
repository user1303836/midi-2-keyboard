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
