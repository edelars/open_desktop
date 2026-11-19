//! S4-41c-1: the opt-in `--stop-on-sigterm` of `od-runner linux-direct`.
//!
//! VMHost stops a Linux run by sending `SIGTERM` to this process (S4-41b). Without the option the
//! signal keeps its default action (the process dies, no report). With it, the handler only sets a
//! flag; the board looks at the flag at its next quantum boundary (`LoadedFirmwareBoard::set_host_stop`),
//! ends the run as an ordinary budget exit, and the runner writes its report with
//! `stop.host_stop: true` and exits with code 0.
//!
//! The handler does one atomic store into an already allocated flag: no allocation, no lock, no
//! I/O. `OnceLock::get` is an atomic load. Only `SIGTERM` is handled; `SIGINT` and every other
//! signal keep their default action.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// The flag the handler sets; `None` until [`install`] succeeded.
pub fn installed() -> Option<Arc<AtomicBool>> {
    FLAG.get().cloned()
}

#[cfg(unix)]
mod sys {
    use std::os::raw::c_int;

    pub const SIGTERM: c_int = 15;
    /// `SIG_ERR` is `(sighandler_t)-1`.
    pub const SIG_ERR: usize = usize::MAX;

    unsafe extern "C" {
        pub fn signal(signum: c_int, handler: usize) -> usize;
    }
}

#[cfg(unix)]
extern "C" fn on_sigterm(_signal: std::os::raw::c_int) {
    if let Some(flag) = FLAG.get() {
        flag.store(true, Ordering::Release);
    }
}

/// Installs the `SIGTERM` handler and returns the flag it sets. Idempotent: every call returns
/// the same flag and (re)installs the same handler. The flag cell is created first, so the
/// handler never sees an empty cell. After an `Err` there is no handler (the process keeps the
/// default action); the caller must not run with the option then.
#[cfg(unix)]
pub fn install() -> Result<Arc<AtomicBool>, String> {
    let flag = FLAG
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone();
    let handler = on_sigterm as extern "C" fn(std::os::raw::c_int) as usize;
    // SAFETY: `signal` is the C library function with this signature; `handler` is an
    // `extern "C"` function that only performs an atomic store.
    let previous = unsafe { sys::signal(sys::SIGTERM, handler) };
    if previous == sys::SIG_ERR {
        return Err("signal(SIGTERM) failed".to_string());
    }
    Ok(flag)
}

#[cfg(not(unix))]
pub fn install() -> Result<Arc<AtomicBool>, String> {
    Err("--stop-on-sigterm needs a Unix host".to_string())
}

/// The flag is process-global, so the tests that raise or read it take this lock and clear it.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Gives `SIGTERM` its default action back (the tests must not leave a test process that ignores
/// the termination signal of a CI timeout).
#[cfg(all(test, unix))]
pub(crate) fn restore_default_for_test() {
    // SAFETY: `signal` is the C library function; SIG_DFL is 0.
    unsafe { sys::signal(sys::SIGTERM, 0) };
}

#[cfg(test)]
pub(crate) fn clear_for_test() {
    if let Some(flag) = FLAG.get() {
        flag.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_handler_sets_only_the_installed_flag() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_for_test();
        unsafe extern "C" {
            fn raise(signum: std::os::raw::c_int) -> std::os::raw::c_int;
        }
        let flag = install().expect("installs");
        assert!(!flag.load(Ordering::Acquire));
        // A second install returns the same flag.
        assert!(Arc::ptr_eq(&flag, &install().unwrap()));
        assert!(Arc::ptr_eq(&flag, &installed().unwrap()));
        // SAFETY: `raise` is the C library function; the handler is installed above.
        assert_eq!(unsafe { raise(sys::SIGTERM) }, 0);
        assert!(flag.load(Ordering::Acquire));
        clear_for_test();
        restore_default_for_test();
    }
}
