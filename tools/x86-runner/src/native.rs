use std::env;
use std::fs;
use std::io::{self, Write};
use std::mem::size_of;
use std::process::{Command, ExitCode};
use std::ptr::NonNull;
use std::time::Duration;

use od_isa::decode;
use od_testkit::HexU64;
use od_testkit::case::{ExpectedExit, Fixture, LoadedFixture};
use od_testkit::result::{
    Backend, BuildMetadata, CompletionStatus, CpuSnapshot, GprSnapshot, MachineMetadata,
    Observation, RESULT_SCHEMA_VERSION, RunResult,
};

use crate::supervisor::{OutputStream, ProcessLimits, Termination, parse_result, run_supervised};

const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_UNSUPPORTED: u8 = 4;
const EXIT_HARNESS_FAILURE: u8 = 5;
const MODELED_FLAGS: u64 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
const RESERVED_FLAG: u64 = 1 << 1;

pub fn run_cli() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--fxsave64-oracle") {
        return crate::fxsave64::run_cli(&args[1..]);
    }
    if let Some(exit) = crate::native_v2::dispatch(&args) {
        return exit;
    }
    if args.first().map(String::as_str) == Some("--native-child") {
        return run_child_cli(&args[1..]);
    }
    if args.len() != 3 || args[0] != "--fixture" || args[2] != "--json" {
        eprintln!("usage: od-x86-runner --fixture CASE.json --json");
        return ExitCode::from(EXIT_INVALID_INPUT);
    }

    let path = &args[1];
    let loaded = match load_fixture(path) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("invalid fixture: {error}");
            return ExitCode::from(EXIT_INVALID_INPUT);
        }
    };
    let limits = ProcessLimits::fixture_default(Duration::from_millis(u64::from(
        loaded.fixture.wall_timeout_ms,
    )));
    let mut command = Command::new(env::current_exe().unwrap_or_else(|_| "od-x86-runner".into()));
    command.arg("--native-child").arg(path);
    let outcome = match run_supervised(&mut command, limits) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("native runner supervision failed: {error}");
            return emit_result(
                unavailable_result(
                    &loaded,
                    CompletionStatus::HarnessError,
                    "native runner supervision failed",
                ),
                EXIT_HARNESS_FAILURE,
            );
        }
    };

    match outcome.termination {
        Termination::Exited { code: Some(code) }
            if code == 0
                || code == i32::from(EXIT_INVALID_INPUT)
                || code == i32::from(EXIT_UNSUPPORTED) =>
        {
            match parse_result(&outcome.stdout) {
                Ok(result)
                    if result.case_id == loaded.fixture.case_id
                        && result.input_sha256 == loaded.input_sha256
                        && result.backend == Backend::NativeX86 =>
                {
                    let exit_code = match result.status {
                        CompletionStatus::Completed => 0,
                        CompletionStatus::Unsupported => EXIT_UNSUPPORTED,
                        CompletionStatus::InvalidInput => EXIT_INVALID_INPUT,
                        _ => EXIT_HARNESS_FAILURE,
                    };
                    emit_result(result, exit_code)
                }
                Ok(_) => {
                    eprintln!("native child returned a result for a different fixture");
                    emit_result(
                        unavailable_result(
                            &loaded,
                            CompletionStatus::HarnessError,
                            "native child returned an inconsistent result",
                        ),
                        EXIT_HARNESS_FAILURE,
                    )
                }
                Err(error) => {
                    eprintln!("native child returned invalid JSON: {error}");
                    emit_result(
                        unavailable_result(
                            &loaded,
                            CompletionStatus::HarnessError,
                            "native child returned invalid JSON",
                        ),
                        EXIT_HARNESS_FAILURE,
                    )
                }
            }
        }
        Termination::TimedOut => emit_result(
            unavailable_result(
                &loaded,
                CompletionStatus::TimedOut,
                "native child exceeded the fixture wall timeout",
            ),
            EXIT_HARNESS_FAILURE,
        ),
        Termination::Signaled { signal } => {
            eprintln!("native child terminated by signal {signal}");
            emit_result(
                unavailable_result(
                    &loaded,
                    CompletionStatus::Signaled,
                    "native child terminated by an unexpected signal",
                ),
                EXIT_HARNESS_FAILURE,
            )
        }
        Termination::OutputLimitExceeded { stream } => {
            let reason = match stream {
                OutputStream::Stdout => "native child exceeded the result output limit",
                OutputStream::Stderr => "native child exceeded the diagnostic output limit",
            };
            eprintln!("{reason}");
            emit_result(
                unavailable_result(&loaded, CompletionStatus::HarnessError, reason),
                EXIT_HARNESS_FAILURE,
            )
        }
        Termination::Exited { code } => {
            eprintln!("native child exited unexpectedly with status {code:?}");
            emit_result(
                unavailable_result(
                    &loaded,
                    CompletionStatus::HarnessError,
                    "native child exited without a supported result status",
                ),
                EXIT_HARNESS_FAILURE,
            )
        }
    }
}

fn run_child_cli(args: &[String]) -> ExitCode {
    if args.len() != 1 {
        eprintln!("native child requires one fixture path");
        return ExitCode::from(EXIT_INVALID_INPUT);
    }
    let loaded = match load_fixture(&args[0]) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("invalid fixture: {error}");
            return ExitCode::from(EXIT_INVALID_INPUT);
        }
    };
    let result = match execute_fixture(&loaded) {
        Ok(result) => result,
        Err(NativeFailure::Unsupported(reason)) => {
            return emit_result(
                unavailable_result(&loaded, CompletionStatus::Unsupported, &reason),
                EXIT_UNSUPPORTED,
            );
        }
        Err(NativeFailure::Harness(reason)) => {
            eprintln!("native harness failure: {reason}");
            return emit_result(
                unavailable_result(&loaded, CompletionStatus::HarnessError, &reason),
                EXIT_HARNESS_FAILURE,
            );
        }
    };
    emit_result(result, 0)
}

fn load_fixture(path: &str) -> Result<LoadedFixture, String> {
    let input = fs::read(path).map_err(|error| format!("could not read fixture: {error}"))?;
    Fixture::from_json(&input).map_err(|error| error.to_string())
}

enum NativeFailure {
    Unsupported(String),
    Harness(String),
}

fn execute_fixture(loaded: &LoadedFixture) -> Result<RunResult, NativeFailure> {
    let fixture = &loaded.fixture;
    if fixture.expected_exit != ExpectedExit::ReachedCodeEnd {
        return Err(NativeFailure::Unsupported(
            "native profile currently supports reached_code_end fixtures only".to_string(),
        ));
    }
    if fixture.initial_state.rip.get() != 0 {
        return Err(NativeFailure::Unsupported(
            "native profile requires an initial RIP offset of zero".to_string(),
        ));
    }
    let mut code = fixture
        .program
        .bytes()
        .map_err(|error| NativeFailure::Unsupported(error.to_string()))?;
    decode(&code).map_err(|error| {
        NativeFailure::Unsupported(format!("decoder rejected program: {error:?}"))
    })?;
    code.push(0xC3); // Wrapper return; RET is never accepted from fixture bytes.

    let page_size = page_size()
        .map_err(|error| NativeFailure::Harness(format!("could not read page size: {error}")))?;
    let mut code_mapping = GuardedMapping::new(code.len(), page_size)
        .map_err(|error| NativeFailure::Harness(format!("code mapping failed: {error}")))?;
    // SAFETY: the mapping has a writable interior sized to contain `code`.
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), code_mapping.interior, code.len());
    }
    code_mapping
        .protect(libc::PROT_READ | libc::PROT_EXEC)
        .map_err(|error| NativeFailure::Harness(format!("code mprotect failed: {error}")))?;

    let state_mapping = GuardedMapping::new(size_of::<NativeIo>(), page_size)
        .map_err(|error| NativeFailure::Harness(format!("state mapping failed: {error}")))?;
    let native_io = NativeIo {
        input_gpr: [
            fixture.initial_state.rax.get(),
            fixture.initial_state.rcx.get(),
            fixture.initial_state.rdx.get(),
            fixture.initial_state.rbx.get(),
        ],
        input_rflags: (fixture.initial_state.rflags.get() & MODELED_FLAGS) | RESERVED_FLAG,
        output_gpr: [0; 4],
        output_rflags: 0,
    };
    // SAFETY: the state mapping is writable, aligned by mmap, and large enough.
    unsafe { state_mapping.interior.cast::<NativeIo>().write(native_io) };

    let stack_bytes = page_size
        .checked_mul(16)
        .ok_or_else(|| NativeFailure::Harness("stack size overflow".to_string()))?;
    let stack_mapping = GuardedMapping::new(stack_bytes, page_size)
        .map_err(|error| NativeFailure::Harness(format!("stack mapping failed: {error}")))?;
    let stack_top = unsafe { stack_mapping.interior.add(stack_mapping.interior_len) };
    let state_ptr = state_mapping.interior.cast::<NativeIo>();
    // SAFETY: bytes were decoded by od-isa and the wrapper appends the sole RET;
    // code, state, and stack are distinct guarded mappings with suitable access.
    unsafe {
        od_native_trampoline(code_mapping.interior, state_ptr, stack_top);
    }
    // SAFETY: the assembly wrote the output half of the initialized NativeIo.
    let io = unsafe { state_ptr.read() };
    let rflags = (io.output_rflags & MODELED_FLAGS) | RESERVED_FLAG;
    Ok(completed_result(
        loaded,
        io.output_gpr,
        rflags,
        code.len() - 1,
    ))
}

fn completed_result(loaded: &LoadedFixture, gpr: [u64; 4], rflags: u64, rip: usize) -> RunResult {
    RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend: Backend::NativeX86,
        status: CompletionStatus::Completed,
        expected_exit: loaded.fixture.expected_exit,
        exit_reason: Observation::Available {
            value: od_testkit::result::ExitReason::ReachedCodeEnd,
        },
        instructions_executed: Observation::Unavailable {
            reason: "native retired instruction count is not sampled".to_string(),
        },
        snapshot: CpuSnapshot {
            gpr: Observation::Available {
                value: GprSnapshot {
                    rax: HexU64(gpr[0]),
                    rcx: HexU64(gpr[1]),
                    rdx: HexU64(gpr[2]),
                    rbx: HexU64(gpr[3]),
                },
            },
            rip: Observation::Available {
                value: HexU64(rip as u64),
            },
            rflags: Observation::Available {
                value: HexU64(rflags),
            },
            memory: Observation::Unavailable {
                reason: "S0-09 register-only profile does not capture memory".to_string(),
            },
            exception: Observation::Unavailable {
                reason: "S0-09 profile does not capture exceptions".to_string(),
            },
        },
        build: build_metadata(),
        machine: machine_metadata(),
    }
}

fn unavailable_result(loaded: &LoadedFixture, status: CompletionStatus, reason: &str) -> RunResult {
    RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend: Backend::NativeX86,
        status,
        expected_exit: loaded.fixture.expected_exit,
        exit_reason: unavailable(reason),
        instructions_executed: unavailable(reason),
        snapshot: CpuSnapshot {
            gpr: unavailable(reason),
            rip: unavailable(reason),
            rflags: unavailable(reason),
            memory: unavailable(reason),
            exception: unavailable(reason),
        },
        build: build_metadata(),
        machine: machine_metadata(),
    }
}

fn unavailable<T>(reason: &str) -> Observation<T> {
    Observation::Unavailable {
        reason: reason.to_string(),
    }
}

fn emit_result(result: RunResult, exit_code: u8) -> ExitCode {
    match result.to_json() {
        Ok(json) => {
            let mut stdout = io::stdout().lock();
            if stdout
                .write_all(&json)
                .and_then(|_| stdout.write_all(b"\n"))
                .is_ok()
            {
                ExitCode::from(exit_code)
            } else {
                eprintln!("could not write native result JSON");
                ExitCode::from(EXIT_HARNESS_FAILURE)
            }
        }
        Err(error) => {
            eprintln!("could not serialize native result: {error}");
            ExitCode::from(EXIT_HARNESS_FAILURE)
        }
    }
}

pub(crate) fn build_metadata() -> BuildMetadata {
    BuildMetadata {
        revision: option_env!("OPEN_DESKTOP_GIT_REVISION")
            .unwrap_or("unavailable")
            .to_string(),
        target: "x86_64-unknown-linux-gnu".to_string(),
        profile: if cfg!(debug_assertions) {
            "debug".to_string()
        } else {
            "release".to_string()
        },
        compiler: option_env!("RUSTC_VERSION")
            .unwrap_or("rustc version unavailable at build time")
            .to_string(),
    }
}

pub(crate) fn machine_metadata() -> MachineMetadata {
    let cpu_model = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                (key.trim() == "model name").then(|| value.trim().to_string())
            })
        });
    let kernel = uname().ok();
    MachineMetadata {
        host_os: env::consts::OS.to_string(),
        host_arch: env::consts::ARCH.to_string(),
        cpu_model,
        kernel,
        page_size: page_size().ok().map(|value| value as u64),
    }
}

fn uname() -> io::Result<String> {
    // SAFETY: `utsname` is a plain C struct initialized by uname.
    let mut name = unsafe { std::mem::zeroed::<libc::utsname>() };
    // SAFETY: `name` points to a valid writable utsname structure.
    if unsafe { libc::uname(&mut name) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Linux fills `release` with a NUL-terminated byte string.
    let release = unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) };
    Ok(release.to_string_lossy().into_owned())
}

fn page_size() -> io::Result<usize> {
    // SAFETY: sysconf has no pointer arguments or memory side effects.
    let value = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(value).map_err(|_| io::Error::last_os_error())
}

struct GuardedMapping {
    base: NonNull<u8>,
    total_len: usize,
    interior: *mut u8,
    interior_len: usize,
}

impl GuardedMapping {
    fn new(requested_len: usize, page_size: usize) -> io::Result<Self> {
        if requested_len == 0 || page_size == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty mapping"));
        }
        let pages = requested_len
            .checked_add(page_size - 1)
            .ok_or_else(|| io::Error::other("mapping size overflow"))?
            / page_size;
        let interior_len = pages
            .checked_mul(page_size)
            .ok_or_else(|| io::Error::other("mapping size overflow"))?;
        let total_len = interior_len
            .checked_add(page_size * 2)
            .ok_or_else(|| io::Error::other("mapping size overflow"))?;
        // SAFETY: mmap allocates an anonymous private reservation with no access.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                total_len,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let base = NonNull::new(raw.cast::<u8>()).expect("mmap returned a non-null mapping");
        // SAFETY: interior starts one page into the reserved region.
        let interior = unsafe { base.as_ptr().add(page_size) };
        // SAFETY: the interior lies within the reserved mapping and is page-sized.
        if unsafe {
            libc::mprotect(
                interior.cast(),
                interior_len,
                libc::PROT_READ | libc::PROT_WRITE,
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            // SAFETY: base/total_len are the mapping returned by mmap above.
            unsafe { libc::munmap(base.as_ptr().cast(), total_len) };
            return Err(error);
        }
        Ok(Self {
            base,
            total_len,
            interior,
            interior_len,
        })
    }

    fn protect(&mut self, protection: libc::c_int) -> io::Result<()> {
        // SAFETY: the range is the page-aligned writable interior of this mapping.
        if unsafe { libc::mprotect(self.interior.cast(), self.interior_len, protection) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

impl Drop for GuardedMapping {
    fn drop(&mut self) {
        // SAFETY: base/total_len are the mapping returned by mmap.
        unsafe { libc::munmap(self.base.as_ptr().cast(), self.total_len) };
    }
}

#[repr(C)]
struct NativeIo {
    input_gpr: [u64; 4],
    input_rflags: u64,
    output_gpr: [u64; 4],
    output_rflags: u64,
}

core::arch::global_asm!(
    ".text",
    ".global od_native_trampoline",
    ".type od_native_trampoline,@function",
    "od_native_trampoline:",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov r12, rsp",
    "mov rsp, rdx",
    "and rsp, -16",
    "sub rsp, 64",
    "mov [rsp + 40], rdi",
    "mov [rsp + 48], rsi",
    "mov rsi, [rsp + 48]",
    "mov rax, [rsi + 32]",
    "push rax",
    "popfq",
    "mov rax, [rsi]",
    "mov rcx, [rsi + 8]",
    "mov rdx, [rsi + 16]",
    "mov rbx, [rsi + 24]",
    "call qword ptr [rsp + 40]",
    "mov [rsp], rax",
    "mov [rsp + 8], rcx",
    "mov [rsp + 16], rdx",
    "mov [rsp + 24], rbx",
    "pushfq",
    "pop rax",
    "mov [rsp + 32], rax",
    "mov rsi, [rsp + 48]",
    "mov rax, [rsp]",
    "mov [rsi + 40], rax",
    "mov rax, [rsp + 8]",
    "mov [rsi + 48], rax",
    "mov rax, [rsp + 16]",
    "mov [rsi + 56], rax",
    "mov rax, [rsp + 24]",
    "mov [rsi + 64], rax",
    "mov rax, [rsp + 32]",
    "mov [rsi + 72], rax",
    "mov rsp, r12",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "ret",
    ".size od_native_trampoline, .-od_native_trampoline",
);

unsafe extern "C" {
    fn od_native_trampoline(code: *const u8, state: *mut NativeIo, stack_top: *mut u8);
}

#[cfg(test)]
mod fetch_guard_tests {
    use od_isa::{DecodeError, decode};
    use std::sync::atomic::{AtomicI32, Ordering};

    static SIGNAL_RECORD_FD: AtomicI32 = AtomicI32::new(-1);

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct RawSignalContext {
        signal: i32,
        code: i32,
        faulting_rip: u64,
        fault_address: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum NormalizedEvent {
        SegmentationFault,
        IllegalInstruction,
    }

    struct GuardedPages {
        base: *mut libc::c_void,
        length: usize,
        page_size: usize,
    }

    impl GuardedPages {
        fn new() -> Self {
            let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
            assert!(page_size >= 4096);
            let length = page_size * 2;
            let base = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            assert_ne!(base, libc::MAP_FAILED);
            let guard = unsafe { base.cast::<u8>().add(page_size).cast() };
            assert_eq!(
                unsafe { libc::mprotect(guard, page_size, libc::PROT_NONE) },
                0
            );
            Self {
                base,
                length,
                page_size,
            }
        }

        fn put_before_guard(&self, bytes: &[u8]) -> &[u8] {
            assert!(!bytes.is_empty() && bytes.len() <= 15);
            let start = unsafe { self.base.cast::<u8>().add(self.page_size - bytes.len()) };
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), start, bytes.len());
                std::slice::from_raw_parts(start, bytes.len())
            }
        }
    }

    impl Drop for GuardedPages {
        fn drop(&mut self) {
            unsafe { libc::munmap(self.base, self.length) };
        }
    }

    #[test]
    fn truncated_prefix_at_guard_boundary_returns_decode_error_without_host_fault() {
        let pages = GuardedPages::new();
        let input = pages.put_before_guard(&[0x48]);
        assert!(matches!(
            decode(input),
            Err(DecodeError::Truncated { offset: 0 })
        ));
    }

    #[test]
    fn truncated_immediate_at_guard_boundary_returns_decode_error_without_host_fault() {
        let pages = GuardedPages::new();
        let input = pages.put_before_guard(&[0x48, 0xb8]);
        assert!(matches!(
            decode(input),
            Err(DecodeError::Truncated { offset: 0 })
        ));
    }

    extern "C" fn capture_signal(
        signal: libc::c_int,
        info: *mut libc::siginfo_t,
        context: *mut libc::c_void,
    ) {
        let ucontext = unsafe { &*(context.cast::<libc::ucontext_t>()) };
        let record = RawSignalContext {
            signal,
            code: unsafe { (*info).si_code },
            faulting_rip: ucontext.uc_mcontext.gregs[libc::REG_RIP as usize] as u64,
            fault_address: unsafe { (*info).si_addr() as u64 },
        };
        let fd = SIGNAL_RECORD_FD.load(Ordering::Relaxed);
        unsafe {
            libc::write(
                fd,
                (&record as *const RawSignalContext).cast(),
                std::mem::size_of::<RawSignalContext>(),
            );
            libc::_exit(0);
        }
    }

    fn probe_signal(expected_signal: libc::c_int, trigger_guard_fault: bool) -> RawSignalContext {
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            unsafe { libc::close(fds[0]) };
            SIGNAL_RECORD_FD.store(fds[1], Ordering::Relaxed);
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = capture_signal as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(expected_signal, &action, std::ptr::null_mut());
            }
            if trigger_guard_fault {
                let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
                let mapping = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        page_size,
                        libc::PROT_NONE,
                        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                        -1,
                        0,
                    )
                };
                if mapping == libc::MAP_FAILED {
                    unsafe { libc::_exit(90) };
                }
                unsafe { std::ptr::read_volatile(mapping.cast::<u8>()) };
            } else {
                unsafe { std::arch::asm!("ud2", options(noreturn)) };
            }
            unsafe { libc::_exit(91) };
        }

        unsafe { libc::close(fds[1]) };
        let mut record = std::mem::MaybeUninit::<RawSignalContext>::uninit();
        let read = unsafe {
            libc::read(
                fds[0],
                record.as_mut_ptr().cast(),
                std::mem::size_of::<RawSignalContext>(),
            )
        };
        unsafe { libc::close(fds[0]) };
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        assert_eq!(read as usize, std::mem::size_of::<RawSignalContext>());
        unsafe { record.assume_init() }
    }

    fn normalize_signal(record: RawSignalContext) -> Option<NormalizedEvent> {
        match record.signal {
            libc::SIGSEGV => Some(NormalizedEvent::SegmentationFault),
            libc::SIGILL => Some(NormalizedEvent::IllegalInstruction),
            _ => None,
        }
    }

    #[test]
    fn isolated_guard_fault_preserves_raw_signal_and_rip_then_normalizes_event() {
        let raw = probe_signal(libc::SIGSEGV, true);
        assert_eq!(raw.signal, libc::SIGSEGV);
        assert_ne!(raw.code, 0);
        assert_ne!(raw.faulting_rip, 0);
        assert_ne!(raw.fault_address, 0);
        assert_eq!(
            normalize_signal(raw),
            Some(NormalizedEvent::SegmentationFault)
        );
    }

    #[test]
    fn isolated_illegal_instruction_preserves_raw_signal_and_rip_then_normalizes_event() {
        let raw = probe_signal(libc::SIGILL, false);
        assert_eq!(raw.signal, libc::SIGILL);
        assert_ne!(raw.code, 0);
        assert_ne!(raw.faulting_rip, 0);
        assert_eq!(
            normalize_signal(raw),
            Some(NormalizedEvent::IllegalInstruction)
        );
    }
}
