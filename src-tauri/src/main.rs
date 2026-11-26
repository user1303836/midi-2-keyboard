// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The desktop shell: a thin layer that exposes `m2k_core::Runtime` to the web UI and adds the
//! things only a desktop app has (tray, single instance, autostart, close-to-tray).

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
    /// Set once the user really wants out (as opposed to closing the window to the tray).
    quitting: AtomicBool,
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
        let profile: Profile = serde_json::from_value(value)
            .map_err(|error| format!("this profile could not be read ({error})"))?;
        return Ok(vec![profile]);
    }
    Err("this file does not contain MIDI 2 Keyboard profiles".into())
}

/// Starts the app again with administrator rights (the user answers the UAC prompt) and quits
/// this instance. Needed to control programs that themselves run as administrator.
#[tauri::command(async)]
fn restart_as_admin(app: AppHandle) -> Result<(), String> {
    // Give up the single-instance lock first, or the elevated copy would be turned away.
    tauri_plugin_single_instance::destroy(&app);
    m2k_output::platform::relaunch_elevated(&[])?;
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

/// The window is gone (closed to the tray): stop collecting monitor events for it.
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
        .message(
            "MIDI 2 Keyboard keeps running in the background so your mappings stay active.\n\n\
             Click its icon in the notification area to open it again, or right-click the icon \
             and choose Quit to exit.\n\n\
             To make closing the window quit the app instead, turn off \"Keep running when the \
             window is closed\" in Settings.",
        )
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
/// while output is paused.
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
        .show_menu_on_left_click(false)
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

fn main() {
    let launched_minimized = std::env::args().any(|arg| arg == "--minimized");

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
                let _ = for_events.emit(EVENT, &event);
                if status_changed {
                    refresh_tray(&for_events);
                }
            });
            let config = runtime.config();
            app.manage(AppState {
                runtime,
                tray: Mutex::new(TrayView::default()),
                quitting: AtomicBool::new(false),
            });

            setup_tray(&handle)?;
            refresh_tray(&handle);
            sync_autostart(&handle, config.settings.launch_at_login);

            // The window is not created at startup (see tauri.conf.json): starting in the tray
            // costs no webview at all.
            if launched_minimized || config.settings.start_minimized {
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
                    if state.runtime.config().settings.close_to_tray {
                        // Let the window (and its webview) really close; `ExitRequested` below
                        // keeps the app itself alive in the tray.
                        main_window_gone(app);
                        explain_tray_once(app, &state.runtime);
                    } else {
                        quit_app(app);
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building midi-2-keyboard");

    app.run(|app, event| match event {
        // Closing the last window is not quitting: the engine lives on in the tray.
        RunEvent::ExitRequested { api, code, .. } => {
            let stay = app.try_state::<AppState>().is_some_and(|state| {
                code.is_none()
                    && !state.quitting.load(Ordering::SeqCst)
                    && state.runtime.config().settings.close_to_tray
            });
            if stay {
                api.prevent_exit();
            }
        }
        RunEvent::Exit => {
            // Whatever the way out, never leave a key held down.
            if let Some(state) = app.try_state::<AppState>() {
                state.runtime.shutdown();
            }
        }
        _ => {}
    });
}
