//! Windows backend: `SendInput`.
//!
//! Keys are physical keys, so the virtual-key code of writing-system keys is taken from the
//! keyboard layout of the foreground window (the same translation Windows applies to the real
//! key): `KeyQ` types "a" on AZERTY, and Ctrl+`KeyW` is Ctrl+Z there, exactly like the physical
//! key. The modes differ only in what each event carries:
//!
//! - `Auto`: virtual key + scan code (+ `KEYEVENTF_EXTENDEDKEY`). Applications read the virtual
//!   key, raw-input / DirectInput games read the scan code. Numpad digits always type digits.
//! - `ScanCode`: `KEYEVENTF_SCANCODE`; Windows derives the virtual key from the layout as for
//!   hardware. Numpad keys go through Windows' NumLock translation and often arrive as
//!   navigation keys (observed on Windows 10 even with NumLock on), so prefer `Auto` for digits.
//!   Pause has no single scan code, and keys the active layout does not map, fall back to the
//!   `Auto` form.
//! - `VirtualKey`: virtual key only, scan code 0.
//!
//! Text uses `KEYEVENTF_UNICODE` (one event pair per UTF-16 unit, so characters outside the BMP
//! are sent as surrogate pairs) and is independent of the layout. Relative mouse movement is
//! subject to the pointer speed / acceleration settings like a real mouse (games reading raw
//! input get the exact counts); absolute movement addresses the whole virtual desktop.
//!
//! Every event carries [`crate::INJECTION_TAG`] in `dwExtraInfo`. Windows refuses input into
//! windows of more privileged processes (UIPI) and on secure desktops; that is reported as
//! [`OutputError::Permission`].

pub(crate) mod platform;

use std::collections::HashSet;
use std::mem::size_of;

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE,
    MAPVK_VSC_TO_VK_EX, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, MapVirtualKeyExW, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WHEEL_DELTA, XBUTTON1, XBUTTON2,
};

use crate::keymap::{self, WinKey};
use crate::{
    INJECTION_TAG, Injector, InjectorOptions, Key, MouseButton, OutputError, Result, WindowsKeyMode,
};

const NAME: &str = "windows-sendinput";

/// Events per `SendInput` call when typing text. One call is atomic (not interleaved with other
/// input); this bounds how long that block is.
const CHUNK: usize = 256;

/// Upper bound for the wheel notches of one `scroll` call (one event per notch).
const MAX_NOTCHES: u32 = 1000;

pub(crate) fn create(options: &InjectorOptions) -> Result<Box<dyn Injector>> {
    Ok(Box::new(SendInputInjector {
        mode: options.windows_key_mode,
        inputs: Vec::new(),
        down: HashSet::new(),
        buttons: HashSet::new(),
    }))
}

pub(crate) struct SendInputInjector {
    mode: WindowsKeyMode,
    /// Reused event buffer.
    inputs: Vec<INPUT>,
    /// Keys and buttons held through this injector, released on drop as a safety net.
    down: HashSet<Key>,
    buttons: HashSet<MouseButton>,
}

impl SendInputInjector {
    fn push_key(&mut self, key: Key, up: bool, layout: &mut Layout) -> Result<()> {
        let wk = keymap::windows(key).ok_or_else(|| unsupported(key))?;
        let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
        if wk.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        let (vk, scan) = match self.mode {
            // Windows flags an injected VK_RSHIFT as extended, which makes it the "fake shift"
            // (E0 36) that raw-input readers ignore. By scan code it arrives like the real key.
            WindowsKeyMode::Auto if key == Key::ShiftRight => {
                flags |= KEYEVENTF_SCANCODE;
                (0, wk.scan)
            }
            WindowsKeyMode::Auto => (layout.vk(wk), wk.scan),
            WindowsKeyMode::VirtualKey => (layout.vk(wk), 0),
            WindowsKeyMode::ScanCode => {
                if key != Key::Pause && layout.maps_scan(wk) {
                    flags |= KEYEVENTF_SCANCODE;
                    (0, wk.scan)
                } else {
                    (layout.vk(wk), wk.scan)
                }
            }
        };
        self.inputs.push(key_input(vk, scan, flags));
        Ok(())
    }

    fn push_unicode(&mut self, unit: u16) {
        self.inputs.push(key_input(0, unit, KEYEVENTF_UNICODE));
        self.inputs.push(key_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }

    fn push_notches(&mut self, notches: i32, flag: u32) {
        let delta = if notches > 0 { WHEEL_DELTA as i32 } else { -(WHEEL_DELTA as i32) };
        for _ in 0..notches.unsigned_abs().min(MAX_NOTCHES) {
            self.inputs.push(mouse_input(0, 0, delta as u32, flag));
        }
    }

    /// Sends and clears the buffer.
    fn send(&mut self) -> Result<()> {
        let result = self.inputs.chunks(CHUNK).try_for_each(send_inputs);
        self.inputs.clear();
        result
    }

    fn key(&mut self, key: Key, up: bool) -> Result<()> {
        if up {
            self.down.remove(&key);
        } else if keymap::windows(key).is_some() {
            self.down.insert(key);
        }
        self.inputs.clear();
        self.push_key(key, up, &mut Layout::default())?;
        self.send()
    }

    fn mouse(&mut self, dx: i32, dy: i32, data: u32, flags: u32) -> Result<()> {
        self.inputs.clear();
        self.inputs.push(mouse_input(dx, dy, data, flags));
        self.send()
    }
}

impl Injector for SendInputInjector {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports(&self, key: Key) -> bool {
        keymap::windows(key).is_some()
    }

    fn key_down(&mut self, key: Key) -> Result<()> {
        // A key-down for a key that is already down is an auto-repeat: Windows sets the
        // "previous key state" bit itself from its key state table.
        self.key(key, false)
    }

    fn key_up(&mut self, key: Key) -> Result<()> {
        self.key(key, true)
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.inputs.clear();
        let mut layout = Layout::default();
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
            match key {
                Some(key) => {
                    self.push_key(key, false, &mut layout)?;
                    self.push_key(key, true, &mut layout)?;
                }
                None => {
                    let mut units = [0u16; 2];
                    for &unit in c.encode_utf16(&mut units).iter() {
                        self.push_unicode(unit);
                    }
                }
            }
        }
        self.send()
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let (down_flag, up_flag, data) = match button {
            MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
            MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
            MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
            MouseButton::Back => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, u32::from(XBUTTON1)),
            MouseButton::Forward => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, u32::from(XBUTTON2)),
        };
        if down {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(&button);
        }
        self.mouse(0, 0, data, if down { down_flag } else { up_flag })
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        self.mouse(dx, dy, 0, MOUSEEVENTF_MOVE)
    }

    fn mouse_move_absolute(&mut self, x: i32, y: i32) -> Result<()> {
        // Re-read every time: monitors come and go.
        let (left, top, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        if width <= 0 || height <= 0 {
            return Err(OutputError::Unavailable("no display is attached".into()));
        }
        let nx = normalize(i64::from(x) - i64::from(left), width);
        let ny = normalize(i64::from(y) - i64::from(top), height);
        self.mouse(nx, ny, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.inputs.clear();
        self.push_notches(dy, MOUSEEVENTF_WHEEL);
        self.push_notches(dx, MOUSEEVENTF_HWHEEL);
        self.send()
    }
}

impl Drop for SendInputInjector {
    fn drop(&mut self) {
        // Injected presses outlive the injector: release what is still held, ordinary keys
        // before modifiers.
        let mut held: Vec<Key> = self.down.iter().copied().collect();
        held.sort_by_key(|key| key.is_modifier());
        for key in held {
            let _ = self.key(key, true);
        }
        for button in std::mem::take(&mut self.buttons) {
            let _ = self.mouse_button(button, false);
        }
    }
}

/// Maps a pixel offset into the virtual desktop to the 0..=65535 range of
/// `MOUSEEVENTF_ABSOLUTE`. Windows turns a normalized value `n` into pixel
/// `n * size / 65536` (rounded down), so the smallest `n` landing on `offset` is
/// `ceil(offset * 65536 / size)`.
fn normalize(offset: i64, size: i32) -> i32 {
    let size = i64::from(size);
    let offset = offset.clamp(0, size - 1);
    ((offset * 65536 + size - 1) / size).min(65535) as i32
}

fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTION_TAG as usize,
            },
        },
    }
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTION_TAG as usize,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    let sent =
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        return Ok(());
    }
    let error = unsafe { GetLastError() };
    // UIPI does not set a dedicated error code; on current Windows versions it shows up as
    // ERROR_ACCESS_DENIED, or as no error at all.
    if error == ERROR_ACCESS_DENIED || error == 0 {
        let target = match platform::foreground_process_name() {
            Some(name) if !name.is_empty() => format!("the focused window ({name})"),
            _ => "the focused window".to_string(),
        };
        return Err(OutputError::Permission(format!(
            "Windows blocked the input: {target} belongs to a program with higher privileges \
             than midi-2-keyboard (for example one running as administrator), or a secure \
             desktop (UAC prompt, lock screen) is active. Run midi-2-keyboard as administrator \
             to control elevated programs."
        )));
    }
    Err(OutputError::Other(format!(
        "SendInput delivered {sent} of {} events: {}",
        inputs.len(),
        std::io::Error::from_raw_os_error(error as i32)
    )))
}

fn unsupported(key: Key) -> OutputError {
    OutputError::Unsupported(key.label(), NAME)
}

/// The keyboard layout of the foreground window's thread, looked up at most once per call.
#[derive(Default)]
struct Layout {
    hkl: Option<HKL>,
}

impl Layout {
    fn hkl(&mut self) -> HKL {
        *self.hkl.get_or_insert_with(|| unsafe {
            let hwnd = GetForegroundWindow();
            let thread = if hwnd.is_null() {
                0
            } else {
                GetWindowThreadProcessId(hwnd, std::ptr::null_mut())
            };
            GetKeyboardLayout(thread)
        })
    }

    fn map_scan(&mut self, wk: WinKey) -> u32 {
        unsafe { MapVirtualKeyExW(wk.scan_ex(), MAPVK_VSC_TO_VK_EX, self.hkl()) }
    }

    /// The virtual key the physical key produces on the active layout.
    fn vk(&mut self, wk: WinKey) -> u16 {
        if wk.layout {
            let vk = self.map_scan(wk);
            if (1..=0xFE).contains(&vk) {
                return vk as u16;
            }
        }
        wk.vk
    }

    /// Whether the active layout maps the scan code to any key at all.
    fn maps_scan(&mut self, wk: WinKey) -> bool {
        self.map_scan(wk) != 0
    }
}
