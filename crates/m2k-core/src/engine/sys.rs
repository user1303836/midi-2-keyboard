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
