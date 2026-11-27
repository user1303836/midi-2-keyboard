//! Small platform shims for the engine thread.

/// Gives the engine thread a slightly higher scheduling priority (Windows), so its timers wake
/// it on time while other programs keep the CPU busy (a build, a game). It only ever runs for
/// microseconds at a time. No-op elsewhere (raising priority needs privileges there).
pub fn raise_thread_priority() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
        };
        // SAFETY: the pseudo handle of the calling thread; no pointers involved.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
        }
    }
}

/// Raises the system timer resolution to 1 ms for as long as it lives (Windows), so the
/// engine's millisecond timers (tap durations, key stagger, repeats) fire on time instead of on
/// the default 15.6 ms scheduler tick. No-op elsewhere.
pub struct TimerResolution {
    #[cfg(windows)]
    active: bool,
}

impl TimerResolution {
    pub fn begin() -> Self {
        #[cfg(windows)]
        {
            opt_out_of_power_throttling();
            // SAFETY: plain Win32 call without pointer arguments. 0 = TIMERR_NOERROR.
            let active = unsafe { windows_sys::Win32::Media::timeBeginPeriod(1) } == 0;
            TimerResolution { active }
        }
        #[cfg(not(windows))]
        {
            TimerResolution {}
        }
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.active {
            // SAFETY: matches the successful timeBeginPeriod(1) above.
            unsafe {
                windows_sys::Win32::Media::timeEndPeriod(1);
            }
        }
    }
}

/// Keeps the process out of Windows' power throttling, once per process. Windows 11 stops
/// honouring a process's timer resolution while none of its windows is visible, and the tray is
/// this app's normal state: a 20 ms repeat would land on 15.6 ms ticks. Efficiency-mode
/// throttling can also slow a process it takes for background work. For an input tool that
/// idles at 0 % both opt-outs cost nothing.
///
/// One request carries both: each one replaces the whole setting, so a second would undo the
/// first. Windows 10 does not know the timer flag and refuses that request, which then goes out
/// for the speed alone. A failure leaves things as they were and is ignored.
#[cfg(windows)]
fn opt_out_of_power_throttling() {
    use std::sync::Once;

    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetProcessInformation,
    };

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let opt_out = |mechanisms: u32| {
            // Taken over (ControlMask) and switched off (StateMask 0).
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: mechanisms,
                StateMask: 0,
            };
            // SAFETY: the pseudo handle of this process and a pointer to a local struct of the
            // size passed.
            unsafe {
                SetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                ) != 0
            }
        };
        let speed = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        if !opt_out(speed | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION) {
            opt_out(speed);
        }
    });
}
