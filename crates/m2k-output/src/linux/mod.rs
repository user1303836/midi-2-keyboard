//! Linux backends: uinput (any session) and X11 XTEST.
//!
//! `LinuxBackend::Auto`: on Wayland, uinput when `/dev/uinput` is writable, otherwise XTEST
//! through XWayland (reaches X11 applications only; `permissions()` says so). On X11, XTEST,
//! falling back to uinput.

pub(crate) mod platform;
mod uinput;
mod x11;

use crate::{Injector, InjectorOptions, LinuxBackend, OutputError, Result};

pub(crate) fn create(options: &InjectorOptions) -> Result<Box<dyn Injector>> {
    let uinput = || uinput::UinputInjector::new().map(|i| Box::new(i) as Box<dyn Injector>);
    let x11 = || x11::XTestInjector::new().map(|i| Box::new(i) as Box<dyn Injector>);
    match options.linux_backend {
        LinuxBackend::Uinput => uinput().map_err(OutputError::Unavailable),
        LinuxBackend::X11 => x11().map_err(OutputError::Unavailable),
        LinuxBackend::Auto if is_wayland() => uinput().or_else(|uinput_error| {
            x11()
                .inspect(|_| {
                    log::warn!(
                        "uinput is not available, falling back to XTEST: only X11 (XWayland) \
                         applications will receive input. {uinput_error}"
                    )
                })
                .map_err(|x11_error| {
                    OutputError::Unavailable(format!(
                        "{uinput_error}\n\n(The X11 fallback failed as well: {x11_error})"
                    ))
                })
        }),
        LinuxBackend::Auto => x11().or_else(|x11_error| {
            uinput().map_err(|uinput_error| {
                OutputError::Unavailable(format!(
                    "No X server could be used ({x11_error}).\n\n{uinput_error}"
                ))
            })
        }),
    }
}

/// [`crate::release_held`]. uinput needs nothing: the kernel releases whatever a virtual device
/// holds when the device goes away with the process. XTest presses outlive the connection that
/// made them, so they are released through a connection of their own.
pub(crate) fn emergency_release(
    keys: impl Iterator<Item = crate::Key>,
    buttons: impl Iterator<Item = crate::MouseButton>,
) {
    x11::emergency_release(keys, buttons);
}

/// Whether this is a Wayland session (where X11 APIs only see XWayland clients).
pub(crate) fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
}
