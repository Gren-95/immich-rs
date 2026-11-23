//! Ctrl-C handling matching the original: while progress bars are shown, and in watch mode
//! (which also prints `Exiting...`), SIGINT ends the process with exit code 0.

#[cfg(unix)]
mod imp {
    use std::sync::atomic::{AtomicU8, Ordering};

    const SIGINT: i32 = 2;
    const QUIET: u8 = 1;
    const ANNOUNCE: u8 = 2;

    static MODE: AtomicU8 = AtomicU8::new(0);

    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
        fn write(fd: i32, buffer: *const std::ffi::c_void, count: usize) -> isize;
        fn _exit(status: i32) -> !;
    }

    /// Only async-signal-safe calls (`write`, `_exit`) are made here.
    extern "C" fn on_sigint(_signum: i32) {
        if MODE.load(Ordering::SeqCst) == ANNOUNCE {
            let message = b"Exiting...\n";
            // SAFETY: writing a static buffer to stdout is async-signal-safe.
            unsafe {
                write(1, message.as_ptr().cast(), message.len());
            }
        }
        // SAFETY: `_exit` is async-signal-safe and never returns.
        unsafe { _exit(0) }
    }

    pub fn exit_on_interrupt(announce: bool) {
        let mode = if announce { ANNOUNCE } else { QUIET };
        // Never downgrade from announcing to quiet.
        MODE.fetch_max(mode, Ordering::SeqCst);
        let handler: extern "C" fn(i32) = on_sigint;
        // SAFETY: installs a handler with the C ABI `void (*)(int)` signature `signal` expects.
        unsafe {
            signal(SIGINT, handler as usize);
        }
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn exit_on_interrupt(_announce: bool) {}
}

/// Makes Ctrl-C exit with code 0, printing `Exiting...` first when `announce` is set.
pub fn exit_on_interrupt(announce: bool) {
    imp::exit_on_interrupt(announce);
}
