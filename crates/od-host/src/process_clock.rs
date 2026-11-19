//! CPU time summed across the current host process's threads.

use std::io;
use std::time::Duration;

pub fn process_cpu_time() -> io::Result<Duration> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is writable for the whole timespec; the call does not retain it.
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) {
        return Err(io::Error::other("invalid process CPU time"));
    }
    Ok(Duration::new(time.tv_sec as u64, time.tv_nsec as u32))
}
