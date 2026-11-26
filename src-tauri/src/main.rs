// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The desktop shell: a thin layer that exposes `m2k_core::Runtime` to the web UI and adds the
//! things only a desktop app has (tray, single instance, autostart, close-to-tray).
//!
//! Lifecycle, per platform:
//! - Closing the main window with "Keep running when the window is closed" on destroys the window
//!   (and its webview: the tray footprint stays small) and keeps the app alive in the tray. The
//!   "last window closed" exit request that follows is refused once (`closing_to_tray`). Every
//!   other way out (Quit in the tray or app menu, macOS Cmd+Q / Dock "Quit" / logout through
//!   AppKit's `terminate:`, Windows shutdown / logoff through `WM_ENDSESSION`) ends in
//!   `RunEvent::Exit`, which shuts the runtime down and so releases every held key before the
//!   process ends.
//! - Without a usable tray (creating it failed, or a Linux desktop without a tray host such as
//!   stock GNOME) the app never hides: it starts with its window and closing the window quits.
//! - macOS: the Dock icon reopens the window; the menu-bar icon opens its menu on a left click,
//!   as status items do there.
//! - Linux: AppIndicator icons deliver no clicks, so the menu's "Open" item is the way back.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use m2k_core::{Action, Config, EngineEvent, Profile, Runtime, RuntimeEvent, Snapshot, store};
use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_dialog::{DialogExt as _, MessageDialogKind};

const EVENT: &str = "m2k:event";
const MAIN_WINDOW: &str = "main";
const TRAY_ID: &str = "main";
const PROFILE_FILE_FORMAT: &str = "midi-2-keyboard-profiles";

/// Given (with this process's id) to the elevated copy started by "Restart as administrator".
const AFTER_ELEVATION: &str = "--after-elevation=";

/// How to get back to a window that was closed to the tray, in the words of each platform.
const HOW_TO_REOPEN: &str = if cfg!(target_os = "macos") {
    "Click its icon in the menu bar (or in the Dock) to open it again, or choose Quit in that menu \
     to exit."
} else if cfg!(windows) {
    "Click its icon in the notification area (it may be tucked away behind the ^ arrow) to open it \
     again, or simply start MIDI 2 Keyboard once more. To exit, right-click the icon and choose \
     Quit."
} else {
    "Choose \"Open MIDI 2 Keyboard\" in the menu of its icon on the panel to open it again, or Quit \
     there to exit."
};

/// What the tray menu currently shows, to avoid rebuilding it when nothing changed.
#[derive(Default, PartialEq, Clone)]
struct TrayView {
    paused: bool,
    selected: Option<String>,
    profiles: Vec<(String, String)>,
}

struct AppState {
    runtime: Runtime,
    tray: Mutex<TrayView>,
    /// A tray icon exists (creating one can fail, e.g. without an AppIndicator library).
    has_tray: bool,
    /// Set once the user really wants out (as opposed to closing the window to the tray).
    quitting: AtomicBool,
    /// The main window is closing *to the tray*: the exit request that follows when the last
    /// window is gone is ours to refuse, once. Nothing else is ever refused.
    closing_to_tray: AtomicBool,
}

impl AppState {
    /// Whether the tray gives the user a way back, so the app may live there without a window.
    fn tray_usable(&self) -> bool {
        self.has_tray && tray_host::present()
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[tauri::command]
fn snapshot(state: State<'_, AppState>) -> Snapshot {
    state.runtime.snapshot()
}

#[tauri::command]
fn save_config(app: AppHandle, state: State<'_, AppState>, config: Config) -> Result<Config, String> {
    let saved = state.runtime.set_config(config).map_err(|error| error.to_string())?;
    sync_autostart(&app, saved.settings.launch_at_login);
    refresh_tray(&app);
    Ok(saved)
}

#[tauri::command]
fn set_paused(state: State<'_, AppState>, paused: bool) {
    state.runtime.set_paused(paused);
}

#[tauri::command]
fn set_learning(state: State<'_, AppState>, learning: bool) {
    state.runtime.set_learning(learning);
}

#[tauri::command]
fn send_midi(state: State<'_, AppState>, device: String, bytes: Vec<u8>) {
    state.runtime.send_midi(&device, &bytes);
}

#[tauri::command]
fn test_action(state: State<'_, AppState>, action: Action, delay_ms: u32) {
    state.runtime.test_action(action, delay_ms);
}

#[tauri::command]
fn release_all(state: State<'_, AppState>) {
    state.runtime.release_all();
}

#[tauri::command]
fn rescan_devices(state: State<'_, AppState>) {
    state.runtime.rescan_devices();
}

#[tauri::command]
fn request_permissions(state: State<'_, AppState>) {
    state.runtime.request_permissions();
}

/// Applications with a visible window, for the "pick an app" lists in the UI.
#[tauri::command(async)]
fn running_apps() -> Vec<m2k_output::platform::ForegroundApp> {
    m2k_output::platform::running_apps()
}

#[tauri::command]
fn reveal_config(state: State<'_, AppState>) -> Result<(), String> {
    reveal(&state.runtime.config_path()).map_err(|error| error.to_string())
}

#[tauri::command(async)]
fn export_profiles(app: AppHandle, profiles: Vec<Profile>, suggested_name: String) -> Result<bool, String> {
    let Some(file) = app
        .dialog()
        .file()
        .set_title("Export profiles")
        .set_file_name(suggested_name)
        .add_filter("MIDI 2 Keyboard profiles", &["json"])
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = file.into_path().map_err(|error| error.to_string())?;
    let document = serde_json::json!({
        "format": PROFILE_FILE_FORMAT,
        "version": m2k_core::CONFIG_VERSION,
        "profiles": profiles,
    });
    let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    std::fs::write(&path, text).map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(true)
}

#[tauri::command(async)]
fn import_profiles(app: AppHandle) -> Result<Option<Vec<Profile>>, String> {
    let Some(file) = app
        .dialog()
        .file()
        .set_title("Import profiles")
        .add_filter("MIDI 2 Keyboard profiles", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|error| error.to_string())?;
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    parse_profiles(&text).map(Some)
}

/// Turns the text of a profile file into complete, repaired profiles (used for the bundled
/// examples, which the UI holds as text).
#[tauri::command]
fn parse_profile_text(text: String) -> Result<Vec<Profile>, String> {
    parse_profiles(&text)
}

/// Accepts an exported profile file, a whole config file, or a bare profile object.
fn parse_profiles(text: &str) -> Result<Vec<Profile>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("this is not a JSON file ({error})"))?;
    // A full config (or an export, which has the same `profiles` array): run it through the
    // normal loader so older schema versions are migrated and everything is repaired.
    if value.get("profiles").is_some_and(|p| p.is_array()) {
        let mut wrapper = serde_json::json!({ "version": value.get("version").cloned().unwrap_or(1.into()) });
        wrapper["profiles"] = value["profiles"].clone();
        let (config, _) = store::parse(&wrapper.to_string())
            .map_err(|error| format!("the profiles in this file could not be read ({error})"))?;
        return Ok(config.profiles);
    }
    if value.get("mappings").is_some() {
        // A bare profile: give it the same treatment (defaults, repairs, clamped ranges).
        let wrapper = serde_json::json!({ "version": 1, "profiles": [value] });
        let (config, _) = store::parse(&wrapper.to_string())
            .map_err(|error| format!("this profile could not be read ({error})"))?;
        return Ok(config.profiles);
    }
    Err("this file does not contain MIDI 2 Keyboard profiles".into())
}

/// Starts the app again with administrator rights (the user answers the UAC prompt) and quits
/// this instance. Needed to control programs that themselves run as administrator.
#[tauri::command(async)]
fn restart_as_admin(app: AppHandle) -> Result<(), String> {
    // The elevated copy waits until this process has ended (see `main`), so the single-instance
    // lock is given up only once the relaunch is certain: a declined prompt leaves this
    // instance fully in charge.
    let argument = format!("{AFTER_ELEVATION}{}", std::process::id());
    m2k_output::platform::relaunch_elevated(&[argument])?;
    tauri_plugin_single_instance::destroy(&app);
    quit_app(&app);
    Ok(())
}

#[tauri::command]
fn quit(app: AppHandle) {
    quit_app(&app);
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Shows `path` in the system file manager.
fn reveal(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer").arg("/select,").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn()?;
    }
    Ok(())
}

/// Shows the main window, creating it first if it does not exist.
///
/// While the app sits in the tray there is no window at all: a hidden webview would keep a few
/// hundred megabytes of browser processes alive for nothing. The engine does not need it.
fn show_main_window(app: &AppHandle) {
    let window = app.get_webview_window(MAIN_WINDOW).or_else(|| {
        let config = app.config().app.windows.iter().find(|w| w.label == MAIN_WINDOW)?.clone();
        match tauri::WebviewWindowBuilder::from_config(app, &config).and_then(|builder| builder.build()) {
            Ok(window) => Some(window),
            Err(error) => {
                log::error!("could not create the main window: {error}");
                None
            }
        }
    });
    if let Some(window) = window {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    if let Some(state) = app.try_state::<AppState>() {
        state.runtime.set_monitoring(true);
    }
}

/// The window is gone (closed to the tray, or never opened): stop collecting monitor events.
fn main_window_gone(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.runtime.set_monitoring(false);
        state.runtime.set_window_focused(false);
    }
}

/// The first time the window is closed to the tray, say so: otherwise "I closed it and my
/// controller still types" (or "where did it go?") is a nasty surprise.
fn explain_tray_once(app: &AppHandle, runtime: &Runtime) {
    let marker = runtime.config_path().with_file_name("tray-hint-shown");
    if marker.exists() {
        return;
    }
    let _ = std::fs::write(&marker, b"shown\n");
    app.dialog()
        .message(format!(
            "MIDI 2 Keyboard keeps running in the background so your mappings stay active.\n\n\
             {HOW_TO_REOPEN}\n\n\
             To make closing the window quit the app instead, turn off \"Keep running when the \
             window is closed\" in Settings."
        ))
        .title("MIDI 2 Keyboard is still running")
        .kind(MessageDialogKind::Info)
        .show(|_| {});
}

fn quit_app(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.quitting.store(true, Ordering::SeqCst);
        state.runtime.shutdown();
    }
    app.exit(0);
}

/// Keeps the OS "launch at login" registration in line with the setting (release builds only, so
/// a development build never registers itself).
fn sync_autostart(app: &AppHandle, enabled: bool) {
    if cfg!(debug_assertions) {
        return;
    }
    let manager = app.autolaunch();
    let current = manager.is_enabled().unwrap_or(false);
    let result = match (enabled, current) {
        (true, false) => manager.enable(),
        (false, true) => manager.disable(),
        _ => Ok(()),
    };
    if let Err(error) = result {
        log::warn!("could not update launch-at-login: {error}");
    }
}

fn tray_view(runtime: &Runtime) -> TrayView {
    let config = runtime.config();
    let status = runtime.status();
    TrayView {
        paused: status.paused,
        selected: status.selected_profile.or(config.selected_profile),
        profiles: config
            .profiles
            .iter()
            .filter(|profile| !profile.always_on)
            .map(|profile| (profile.id.clone(), profile.name.clone()))
            .collect(),
    }
}

fn build_tray_menu(app: &AppHandle, view: &TrayView) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let open = MenuItemBuilder::with_id("open", "Open MIDI 2 Keyboard").build(app)?;
    let pause = CheckMenuItemBuilder::with_id("pause", "Paused").checked(view.paused).build(app)?;
    let release = MenuItemBuilder::with_id("release", "Release all keys").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let mut profiles = SubmenuBuilder::new(app, "Profile");
    for (id, name) in &view.profiles {
        let item = CheckMenuItemBuilder::with_id(format!("profile:{id}"), name)
            .checked(view.selected.as_deref() == Some(id.as_str()))
            .build(app)?;
        profiles = profiles.item(&item);
    }
    let profiles = profiles.build()?;
    MenuBuilder::new(app)
        .item(&open)
        .separator()
        .item(&pause)
        .item(&profiles)
        .item(&release)
        .separator()
        .item(&quit)
        .build()
}

/// Rebuilds the tray menu / tooltip if what it shows is out of date. Safe to call from any thread.
fn refresh_tray(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    if !state.has_tray {
        return;
    }
    let view = tray_view(&state.runtime);
    {
        let mut current = state.tray.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *current == view {
            return;
        }
        *current = view.clone();
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(tray) = handle.tray_by_id(TRAY_ID) else { return };
        if let Ok(menu) = build_tray_menu(&handle, &view) {
            let _ = tray.set_menu(Some(menu));
        }
        if let Some(icon) = tray_icon(view.paused) {
            let _ = tray.set_icon(Some(icon));
        }
        let profile = view
            .profiles
            .iter()
            .find(|(id, _)| view.selected.as_deref() == Some(id.as_str()))
            .map(|(_, name)| name.as_str())
            .unwrap_or("no profile");
        let state_text = if view.paused { "Paused" } else { "Active" };
        let _ = tray.set_tooltip(Some(format!("MIDI 2 Keyboard — {state_text} · {profile}")));
    });
}

/// The tray icon doubles as a status light: the normal icon while live, a greyed "pause" one
/// while output is paused. Both are full-colour on every platform: as macOS template images
/// (alpha only) the two would be the same filled square, so a monochrome template pair would
/// need artwork of its own.
fn tray_icon(paused: bool) -> Option<tauri::image::Image<'static>> {
    let bytes: &[u8] = if paused {
        include_bytes!("../icons/tray-paused.png")
    } else {
        include_bytes!("../icons/tray-active.png")
    };
    tauri::image::Image::from_bytes(bytes).ok()
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let view = TrayView::default();
    let menu = build_tray_menu(app, &view)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("MIDI 2 Keyboard")
        .menu(&menu)
        // Status items open their menu on a left click on macOS; on Windows the left click opens
        // the window (below) and the right click shows the menu.
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            let Some(state) = app.try_state::<AppState>() else { return };
            match id {
                "open" => show_main_window(app),
                "pause" => {
                    let paused = state.runtime.status().paused;
                    state.runtime.set_paused(!paused);
                }
                "release" => state.runtime.release_all(),
                "quit" => quit_app(app),
                _ => {
                    if let Some(profile) = id.strip_prefix("profile:") {
                        state.runtime.select_profile(profile);
                    }
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            // macOS shows the menu instead; Linux (AppIndicator) delivers no clicks at all.
            if cfg!(target_os = "macos") {
                return;
            }
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = tray_icon(false).or_else(|| app.default_window_icon().cloned()) {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

/// Whether the desktop actually shows tray icons.
#[cfg(target_os = "linux")]
mod tray_host {
    //! An AppIndicator icon is only visible when the desktop runs a tray host: KDE, Cinnamon,
    //! XFCE, MATE, ... do, stock GNOME needs an extension. Without one the icon goes nowhere and
    //! nothing reports an error, so look for a host explicitly.

    use gtk::gio;
    use gtk::glib::{self, ToVariant as _};
    use x11rb::protocol::xproto::ConnectionExt as _;

    pub fn present() -> bool {
        status_notifier_host() || xembed_tray()
    }

    /// A StatusNotifierItem watcher on the session bus (what AppIndicator icons register with).
    fn status_notifier_host() -> bool {
        let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
            return false;
        };
        bus.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&("org.kde.StatusNotifierWatcher",).to_variant()),
            glib::VariantTy::new("(b)").ok(),
            gio::DBusCallFlags::NONE,
            500,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|reply| reply.get::<(bool,)>())
        .is_some_and(|(owned,)| owned)
    }

    /// A classic X11 (XEmbed) tray, which AppIndicator falls back to. Never on Wayland.
    fn xembed_tray() -> bool {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"));
        if wayland {
            return false;
        }
        let Ok((conn, screen)) = x11rb::connect(None) else { return false };
        let name = format!("_NET_SYSTEM_TRAY_S{screen}");
        let Some(atom) = conn.intern_atom(true, name.as_bytes()).ok().and_then(|c| c.reply().ok())
        else {
            return false;
        };
        atom.atom != x11rb::NONE
            && conn
                .get_selection_owner(atom.atom)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|reply| reply.owner != x11rb::NONE)
    }
}

/// Whether the desktop actually shows tray icons.
#[cfg(not(target_os = "linux"))]
mod tray_host {
    /// Windows and macOS always show the icon once it exists.
    pub fn present() -> bool {
        true
    }
}

/// Waits (up to 10 s) until process `pid` has ended.
#[cfg(windows)]
fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: plain Win32 calls on a handle owned here.
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            WaitForSingleObject(process, 10_000);
            CloseHandle(process);
        }
    }
}

#[cfg(not(windows))]
fn wait_for_exit(_pid: u32) {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let launched_minimized = args.iter().any(|arg| arg == "--minimized");
    // Started by "Restart as administrator": let the previous instance finish quitting first,
    // or its single-instance lock would turn this one away.
    if let Some(pid) = args.iter().find_map(|arg| arg.strip_prefix(AFTER_ELEVATION)?.parse().ok()) {
        wait_for_exit(pid);
    }

    let app = tauri::Builder::default()
        // Must be registered first: a second launch just brings the running instance forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main_window(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            snapshot,
            save_config,
            set_paused,
            set_learning,
            send_midi,
            test_action,
            release_all,
            rescan_devices,
            request_permissions,
            running_apps,
            reveal_config,
            export_profiles,
            import_profiles,
            parse_profile_text,
            restart_as_admin,
            quit,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let for_events = handle.clone();
            let runtime = Runtime::start(&store::default_path(), move |event: RuntimeEvent| {
                // Pause state and profile selection are mirrored in the tray.
                let status_changed = matches!(&event, RuntimeEvent::Engine { events }
                    if events.iter().any(|e| matches!(e, EngineEvent::Status(_))));
                // Without a window nobody listens; the UI reads a fresh snapshot when it opens.
                if for_events.get_webview_window(MAIN_WINDOW).is_some() {
                    let _ = for_events.emit(EVENT, &event);
                }
                if status_changed {
                    refresh_tray(&for_events);
                }
            });
            let config = runtime.config();
            // A missing tray must not keep the app from starting: it then simply never hides.
            let has_tray = match setup_tray(&handle) {
                Ok(()) => true,
                Err(error) => {
                    log::error!("could not create the tray icon: {error}");
                    false
                }
            };
            app.manage(AppState {
                runtime,
                tray: Mutex::new(TrayView::default()),
                has_tray,
                quitting: AtomicBool::new(false),
                closing_to_tray: AtomicBool::new(false),
            });

            refresh_tray(&handle);
            sync_autostart(&handle, config.settings.launch_at_login);

            // The window is not created at startup (see tauri.conf.json): starting in the tray
            // costs no webview at all. Without a usable tray it always opens.
            let tray_usable = handle.state::<AppState>().tray_usable();
            if (launched_minimized || config.settings.start_minimized) && tray_usable {
                main_window_gone(&handle);
            } else {
                show_main_window(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != MAIN_WINDOW {
                return;
            }
            let app = window.app_handle();
            let Some(state) = app.try_state::<AppState>() else { return };
            match event {
                WindowEvent::Focused(focused) => state.runtime.set_window_focused(*focused),
                WindowEvent::CloseRequested { .. } => {
                    if state.runtime.config().settings.close_to_tray && state.tray_usable() {
                        // Let the window (and its webview) really close; the exit request that
                        // follows once no window is left is refused in `RunEvent::ExitRequested`.
                        state.closing_to_tray.store(true, Ordering::SeqCst);
                        explain_tray_once(app, &state.runtime);
                    } else {
                        quit_app(app);
                    }
                }
                WindowEvent::Destroyed => main_window_gone(app),
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building midi-2-keyboard");

    app.run(|app, event| match event {
        // The one exit request refused: the one caused by our own window closing to the tray.
        // Quit (tray or app menu), Cmd+Q, logout and the like all go through.
        RunEvent::ExitRequested { api, code, .. } => {
            let to_tray = code.is_none()
                && app.try_state::<AppState>().is_some_and(|state| {
                    state.closing_to_tray.swap(false, Ordering::SeqCst)
                        && !state.quitting.load(Ordering::SeqCst)
                });
            if to_tray {
                api.prevent_exit();
            }
        }
        // macOS: clicking the Dock icon while no window is open brings the window back.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { has_visible_windows, .. } => {
            if !has_visible_windows {
                show_main_window(app);
            }
        }
        RunEvent::Exit => {
            // Whatever the way out (including WM_ENDSESSION and AppKit's terminate:), never
            // leave a key held down. Waits until the engine has released everything.
            if let Some(state) = app.try_state::<AppState>() {
                state.runtime.shutdown();
            }
        }
        _ => {}
    });
}
