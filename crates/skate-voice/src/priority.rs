//! Audio threads ahead of ordinary ones, whatever the window does: time-critical, and opted out of power throttling
//! (EcoQoS). A background window loses Windows' foreground boost and its QoS drops, and an audio thread at normal
//! priority falls behind whenever anything else wants the CPU (a compile, a browser): the game's mixer played the gaps
//! as static, and voice lost speech (the 10-27 two-copy run: 220 ms once). cpal 0.15.3 means to raise its device
//! threads but hands `SetThreadPriority` a thread id where Windows wants a handle, so they ran at normal priority too.
//! Raised here: the game's mixer (player_audio), the voice thread and the microphone's callback.

/// `THREAD_PRIORITY_TIME_CRITICAL`.
pub const TIME_CRITICAL: i32 = 15;

/// Raises the calling thread (nothing to do off Windows).
pub fn raise() -> std::io::Result<()> {
    #[cfg(windows)]
    let raised = windows::raise();
    #[cfg(not(windows))]
    let raised = Ok(());
    raised
}

/// The calling thread's priority (Windows; 0 elsewhere).
pub fn current() -> i32 {
    // SAFETY: the calling thread's pseudo-handle.
    #[cfg(windows)]
    let priority = unsafe { windows::GetThreadPriority(windows::GetCurrentThread()) };
    #[cfg(not(windows))]
    let priority = 0;
    priority
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;

    #[repr(C)]
    struct PowerThrottlingState {
        version: u32,
        control_mask: u32,
        state_mask: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn GetCurrentThread() -> *mut c_void;
        pub(super) fn GetThreadPriority(thread: *mut c_void) -> i32;
        fn SetThreadPriority(thread: *mut c_void, priority: i32) -> i32;
        fn SetThreadInformation(thread: *mut c_void, class: i32, information: *const c_void, size: u32) -> i32;
    }

    /// `THREAD_INFORMATION_CLASS::ThreadPowerThrottling`.
    const THREAD_POWER_THROTTLING: i32 = 3;
    /// `THREAD_POWER_THROTTLING_EXECUTION_SPEED`: controlled, and left clear, means never EcoQoS.
    const EXECUTION_SPEED: u32 = 1;

    pub(super) fn raise() -> std::io::Result<()> {
        let never_throttle = PowerThrottlingState {
            version: 1,
            control_mask: EXECUTION_SPEED,
            state_mask: 0,
        };
        // SAFETY: the calling thread's pseudo-handle, and a struct with the SDK's layout and size.
        let raised = unsafe {
            let thread = GetCurrentThread();
            SetThreadPriority(thread, super::TIME_CRITICAL) != 0
                && SetThreadInformation(
                    thread,
                    THREAD_POWER_THROTTLING,
                    (&raw const never_throttle).cast(),
                    size_of::<PowerThrottlingState>() as u32,
                ) != 0
        };
        if raised { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
}
