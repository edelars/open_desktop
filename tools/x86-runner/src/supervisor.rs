use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const MAX_CPU_SECONDS: u64 = 30;
const MAX_ADDRESS_SPACE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct ProcessLimits {
    pub wall_timeout: Duration,
    pub cpu_seconds: u64,
    pub address_space_bytes: u64,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

impl ProcessLimits {
    pub const fn fixture_default(wall_timeout: Duration) -> Self {
        Self {
            wall_timeout,
            cpu_seconds: 2,
            address_space_bytes: 256 * 1024 * 1024,
            stdout_bytes: od_testkit::MAX_RESULT_JSON_BYTES,
            stderr_bytes: 64 * 1024,
        }
    }

    fn validate(self) -> io::Result<Self> {
        if self.wall_timeout.is_zero()
            || self.wall_timeout
                > Duration::from_millis(u64::from(od_testkit::case::MAX_WALL_TIMEOUT_MS))
            || self.cpu_seconds == 0
            || self.cpu_seconds > MAX_CPU_SECONDS
            || self.address_space_bytes == 0
            || self.address_space_bytes > MAX_ADDRESS_SPACE_BYTES
            || self.stdout_bytes == 0
            || self.stdout_bytes > od_testkit::MAX_RESULT_JSON_BYTES
            || self.stderr_bytes == 0
            || self.stderr_bytes > MAX_STDERR_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "process limits are outside the supported bounds",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Termination {
    Exited { code: Option<i32> },
    Signaled { signal: i32 },
    TimedOut,
    OutputLimitExceeded { stream: OutputStream },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessOutcome {
    pub termination: Termination,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub enum SupervisorError {
    InvalidLimits(io::Error),
    Spawn(io::Error),
    Wait(io::Error),
    Reader(io::Error),
    ReaderPanicked,
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimits(error) => write!(formatter, "invalid process limits: {error}"),
            Self::Spawn(error) => write!(formatter, "could not spawn child: {error}"),
            Self::Wait(error) => write!(formatter, "could not wait for child: {error}"),
            Self::Reader(error) => write!(formatter, "could not read child output: {error}"),
            Self::ReaderPanicked => formatter.write_str("child output reader panicked"),
        }
    }
}

impl std::error::Error for SupervisorError {}

pub fn run_supervised(
    command: &mut Command,
    limits: ProcessLimits,
) -> Result<ProcessOutcome, SupervisorError> {
    let limits = limits.validate().map_err(SupervisorError::InvalidLimits)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_linux_limits(command, limits);
    configure_process_group(command);
    let mut child = command.spawn().map_err(SupervisorError::Spawn)?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SupervisorError::Reader(io::Error::other(
            "child stdout pipe was not created",
        )));
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SupervisorError::Reader(io::Error::other(
            "child stderr pipe was not created",
        )));
    };

    let stdout_exceeded = Arc::new(AtomicBool::new(false));
    let stderr_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_reader(stdout, limits.stdout_bytes, Arc::clone(&stdout_exceeded));
    let stderr_reader = spawn_reader(stderr, limits.stderr_bytes, Arc::clone(&stderr_exceeded));

    let started = Instant::now();
    let mut timed_out = false;
    let mut observed_status: Option<ExitStatus> = None;
    loop {
        if stdout_exceeded.load(Ordering::Relaxed) || stderr_exceeded.load(Ordering::Relaxed) {
            kill_process_group(&mut child);
            break;
        }
        if started.elapsed() >= limits.wall_timeout {
            timed_out = true;
            kill_process_group(&mut child);
            break;
        }
        match child.try_wait() {
            Err(error) => {
                kill_process_group(&mut child);
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(SupervisorError::Wait(error));
            }
            Ok(Some(status)) => {
                observed_status = Some(status);
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
        }
    }
    let status = match observed_status {
        Some(status) => {
            // try_wait reaps exited children; wait returns the cached status and
            // also documents the mandatory parent-side wait invariant.
            child.wait().map_err(SupervisorError::Wait)?;
            status
        }
        None => child.wait().map_err(SupervisorError::Wait)?,
    };

    // A grandchild may inherit either pipe after the supervised process exits.
    // Drain readers only until the same wall-clock deadline, then terminate the
    // process group so the pipe readers can finish.
    while !stdout_reader.is_finished() || !stderr_reader.is_finished() {
        if stdout_exceeded.load(Ordering::Relaxed) || stderr_exceeded.load(Ordering::Relaxed) {
            kill_process_group(&mut child);
            break;
        }
        if started.elapsed() >= limits.wall_timeout {
            timed_out = true;
            kill_process_group(&mut child);
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    let stdout = stdout_reader
        .join()
        .map_err(|_| SupervisorError::ReaderPanicked)?
        .map_err(SupervisorError::Reader)?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| SupervisorError::ReaderPanicked)?
        .map_err(SupervisorError::Reader)?;
    let termination = if timed_out {
        Termination::TimedOut
    } else if stdout_exceeded.load(Ordering::Relaxed) {
        Termination::OutputLimitExceeded {
            stream: OutputStream::Stdout,
        }
    } else if stderr_exceeded.load(Ordering::Relaxed) {
        Termination::OutputLimitExceeded {
            stream: OutputStream::Stderr,
        }
    } else {
        classify_exit(status)
    };

    Ok(ProcessOutcome {
        termination,
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn kill_process_group(child: &mut std::process::Child) {
    let process_group = -(child.id() as libc::pid_t);
    // SAFETY: kill targets the process group created for this child.
    let _ = unsafe { libc::kill(process_group, libc::SIGKILL) };
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn spawn_reader<R>(
    reader: R,
    max_bytes: usize,
    exceeded: Arc<AtomicBool>,
) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = reader;
        let mut output = Vec::with_capacity(max_bytes.min(8192));
        let mut buffer = [0u8; 4096];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                return Ok(output);
            }
            let remaining = max_bytes.saturating_sub(output.len());
            output.extend_from_slice(&buffer[..count.min(remaining)]);
            if count > remaining {
                exceeded.store(true, Ordering::Relaxed);
                return Ok(output);
            }
        }
    })
}

#[cfg(target_os = "linux")]
fn configure_linux_limits(command: &mut Command, limits: ProcessLimits) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(move || {
            set_limit(libc::RLIMIT_CPU, limits.cpu_seconds as libc::rlim_t)?;
            set_limit(libc::RLIMIT_AS, limits.address_space_bytes as libc::rlim_t)?;
            set_limit(libc::RLIMIT_CORE, 0)?;
            Ok(())
        });
    }
}

#[cfg(target_os = "linux")]
fn set_limit(resource: libc::__rlimit_resource_t, value: libc::rlim_t) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: `limit` points to a valid rlimit value for this process.
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_linux_limits(_command: &mut Command, _limits: ProcessLimits) {}

fn classify_exit(status: ExitStatus) -> Termination {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Termination::Signaled { signal };
        }
    }
    Termination::Exited {
        code: status.code(),
    }
}

pub fn parse_result(
    stdout: &[u8],
) -> Result<od_testkit::result::RunResult, od_testkit::FormatError> {
    od_testkit::result::RunResult::from_json(stdout)
}

#[cfg(test)]
mod tests {
    use super::{OutputStream, ProcessLimits, Termination, parse_result, run_supervised};
    use std::process::Command;
    use std::time::Duration;

    fn shell(script: &str, timeout: Duration, stdout_bytes: usize) -> super::ProcessOutcome {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        run_supervised(
            &mut command,
            ProcessLimits {
                wall_timeout: timeout,
                cpu_seconds: 2,
                address_space_bytes: 256 * 1024 * 1024,
                stdout_bytes,
                stderr_bytes: 1024,
            },
        )
        .expect("test child should be supervised")
    }

    #[test]
    fn waits_for_normal_child_exit_and_captures_bounded_channels() {
        let outcome = shell(
            "printf 'result'; printf 'diagnostic' >&2",
            Duration::from_secs(1),
            32,
        );
        assert_eq!(outcome.termination, Termination::Exited { code: Some(0) });
        assert_eq!(outcome.stdout, b"result");
        assert_eq!(outcome.stderr, b"diagnostic");
    }

    #[test]
    fn kills_and_waits_for_a_hung_child() {
        let outcome = shell("sleep 5", Duration::from_millis(50), 32);
        assert_eq!(outcome.termination, Termination::TimedOut);
    }

    #[test]
    fn times_out_when_a_grandchild_keeps_a_pipe_open() {
        let started = std::time::Instant::now();
        let outcome = shell("sleep 5 & exit 0", Duration::from_millis(100), 32);
        assert_eq!(outcome.termination, Termination::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn applies_cpu_address_space_and_core_limits_before_exec() {
        let outcome = shell(
            "ulimit -t; ulimit -v; ulimit -c",
            Duration::from_secs(1),
            64,
        );
        assert_eq!(outcome.termination, Termination::Exited { code: Some(0) });
        assert_eq!(outcome.stdout, b"2\n262144\n0\n");
    }

    #[test]
    fn reports_an_unexpected_child_signal() {
        let outcome = shell("kill -TERM $$", Duration::from_secs(1), 32);
        assert_eq!(outcome.termination, Termination::Signaled { signal: 15 });
    }

    #[test]
    fn kills_a_child_that_exceeds_the_stdout_limit() {
        let outcome = shell("head -c 4096 /dev/zero", Duration::from_secs(1), 32);
        assert_eq!(
            outcome.termination,
            Termination::OutputLimitExceeded {
                stream: OutputStream::Stdout
            }
        );
        assert_eq!(outcome.stdout.len(), 32);
    }

    #[test]
    fn rejects_corrupt_result_json() {
        let outcome = shell("printf '{'", Duration::from_secs(1), 32);
        assert_eq!(outcome.termination, Termination::Exited { code: Some(0) });
        assert!(parse_result(&outcome.stdout).is_err());
    }

    #[test]
    fn rejects_an_early_closed_result_channel() {
        let outcome = shell("exec 1>&-; exit 0", Duration::from_secs(1), 32);
        assert_eq!(outcome.termination, Termination::Exited { code: Some(0) });
        assert!(parse_result(&outcome.stdout).is_err());
    }

    #[test]
    fn rejects_limits_above_the_runner_caps_before_spawning() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 99"]);
        let limits = ProcessLimits {
            wall_timeout: Duration::from_secs(1),
            cpu_seconds: 2,
            address_space_bytes: 256 * 1024 * 1024,
            stdout_bytes: od_testkit::MAX_RESULT_JSON_BYTES + 1,
            stderr_bytes: 1024,
        };
        assert!(matches!(
            run_supervised(&mut command, limits),
            Err(super::SupervisorError::InvalidLimits(_))
        ));
    }
}
