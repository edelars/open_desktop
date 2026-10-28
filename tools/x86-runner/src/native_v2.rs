//! Linux/x86-64 physical integer oracle. The supervised child is a tracer;
//! a disposable tracee executes fixture instructions and never returns to Rust.
//! ptrace/waitpid/mmap/process_vm_readv contracts: Linux man-pages 6.18.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use od_isa::long_decode::{
    self, ArithmeticKind, BitKind, BitOffset, FlagEffects, LogicKind, OpcodeMap, Operand,
    Operation, Prefixes,
};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory};
use od_isa::{LongModeState, OperandSize};
use od_testkit::HexU64;
use od_testkit::case::MemoryPermissions;
use od_testkit::memory::FlatMemory;
use od_testkit::result::{Backend, MemoryRegionSnapshot, Observation};
use od_testkit::v2::{self, Fixture, LoadedFixture, Outcome, RunResult, Snapshot};

use crate::rep_trace::rep_stos_iteration_is_intermediate;
use crate::supervisor::{ProcessLimits, Termination, run_supervised};

const LOW_ADDRESS: u64 = 0x10000;
const HIGH_ADDRESS: u64 = 0x8000_0000;
const MAX_RESERVATION: u64 = 64 * 1024 * 1024;

fn unavailable<T>(reason: &str) -> Observation<T> {
    Observation::Unavailable {
        reason: reason.into(),
    }
}

fn load(path: &str) -> Result<LoadedFixture, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = vec![];
    file.take(v2::MAX_FIXTURE_JSON_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Fixture::from_json(&bytes).map_err(|e| e.to_string())
}

/// V1 retains its existing loader/trampoline. V2 identity is checked again by
/// the supervising parent after parsing the tracer's single result document.
pub(crate) fn dispatch(args: &[String]) -> Option<ExitCode> {
    if args.first().map(String::as_str) == Some("--native-v2-child") {
        return Some(if args.len() == 2 {
            match load(&args[1]) {
                Ok(loaded) => emit(execute(&loaded)),
                Err(e) => {
                    eprintln!("invalid v2 fixture: {e}");
                    ExitCode::from(3)
                }
            }
        } else {
            ExitCode::from(3)
        });
    }
    if args.len() != 3 || args[0] != "--fixture" || args[2] != "--json" {
        return None;
    }
    let loaded = load(&args[1]).ok()?;
    let mut command = Command::new(match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            return Some(emit(empty_result(
                &loaded,
                Outcome::HarnessError {
                    reason: e.to_string(),
                },
            )));
        }
    });
    command.arg("--native-v2-child").arg(&args[1]);
    let limits = ProcessLimits::fixture_default(Duration::from_millis(u64::from(
        loaded.fixture.wall_timeout_ms,
    )));
    let result = match run_supervised(&mut command, limits) {
        Ok(output) => match output.termination {
            Termination::Exited {
                code: Some(0 | 4 | 5),
            } => {
                match RunResult::from_json(&output.stdout).and_then(|r| {
                    r.validate_for(&loaded)?;
                    Ok(r)
                }) {
                    Ok(result) if result.backend == Backend::NativeX86 => result,
                    _ => empty_result(
                        &loaded,
                        Outcome::HarnessError {
                            reason: "native tracer returned an invalid or inconsistent result"
                                .into(),
                        },
                    ),
                }
            }
            Termination::TimedOut => empty_result(
                &loaded,
                Outcome::TimedOut {
                    reason: "supervised native tracer exceeded wall timeout".into(),
                },
            ),
            Termination::Signaled { signal } => empty_result(
                &loaded,
                Outcome::Signaled {
                    signal: signal as u32,
                    reason: "native tracer terminated by host signal".into(),
                },
            ),
            other => empty_result(
                &loaded,
                Outcome::HarnessError {
                    reason: format!("native tracer failed: {other:?}"),
                },
            ),
        },
        Err(error) => empty_result(
            &loaded,
            Outcome::HarnessError {
                reason: error.to_string(),
            },
        ),
    };
    Some(emit(result))
}

fn emit(result: RunResult) -> ExitCode {
    let code = match result.outcome {
        Outcome::StopAddress {}
        | Outcome::InstructionBudgetExhausted {}
        | Outcome::RepIterationBudgetExhausted {} => 0,
        Outcome::Unsupported { .. } => 4,
        _ => 5,
    };
    match result.to_json() {
        Ok(bytes) => match io::stdout()
            .lock()
            .write_all(&bytes)
            .and_then(|_| io::stdout().lock().write_all(b"\n"))
        {
            Ok(()) => ExitCode::from(code),
            Err(_) => ExitCode::from(5),
        },
        Err(error) => {
            eprintln!("invalid native v2 result: {error}");
            ExitCode::from(5)
        }
    }
}

fn empty_result(loaded: &LoadedFixture, outcome: Outcome) -> RunResult {
    RunResult {
        schema_version: v2::SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend: Backend::NativeX86,
        outcome,
        instructions_executed: unavailable("no verified trace-step count"),
        snapshot: unavailable("native state was not captured"),
        build: crate::native::build_metadata(),
        machine: crate::native::machine_metadata(),
    }
}

fn terminal_signal_result(loaded: &LoadedFixture, status: i32) -> Option<RunResult> {
    libc::WIFSIGNALED(status).then(|| empty_result(loaded,Outcome::Signaled {
        signal:libc::WTERMSIG(status) as u32,
        reason:"tracee terminated by a host signal; no final snapshot or exact count is available".into(),
    }))
}

#[derive(Debug)]
struct MappingPlan {
    page_size: u64,
    start: u64,
    length: usize,
    pages: BTreeMap<u64, i32>,
    initial: Vec<(u64, Vec<u8>)>,
}

impl MappingPlan {
    fn new(fixture: &Fixture, page_size: u64) -> Result<Self, String> {
        if !page_size.is_power_of_two() {
            return Err("unsupported system page size".into());
        }
        let mut pages = BTreeMap::new();
        let mut initial = vec![];
        for region in &fixture.memory {
            let start = region.guest_address.get();
            let end = start
                .checked_add(u64::from(region.length))
                .ok_or("mapping range overflow")?;
            let permissions = region.permissions;
            if !permissions.read || (permissions.write && permissions.execute) {
                return Err(
                    "native regions must be readable and cannot be writable+executable".into(),
                );
            }
            if start < LOW_ADDRESS + page_size || end > HIGH_ADDRESS {
                return Err(
                    "native mappings must fit the supported low canonical address range".into(),
                );
            }
            let protection = libc::PROT_READ
                | if permissions.write {
                    libc::PROT_WRITE
                } else {
                    0
                }
                | if permissions.execute {
                    libc::PROT_EXEC
                } else {
                    0
                };
            let mut page = start & !(page_size - 1);
            while page < end {
                if pages
                    .insert(page, protection)
                    .is_some_and(|previous| previous != protection)
                {
                    return Err(
                        "logical regions sharing a host page require identical permissions".into(),
                    );
                }
                page += page_size;
            }
            initial.push((
                start,
                v2::decode_bytes(&region.initial_bytes_hex).map_err(|e| e.to_string())?,
            ));
        }
        let start = pages.first_key_value().ok_or("empty mapping plan")?.0 - page_size;
        let end = pages.last_key_value().unwrap().0 + 2 * page_size;
        if end - start > MAX_RESERVATION {
            return Err("native fixture mapping envelope exceeds 64MiB".into());
        }
        Ok(Self {
            page_size,
            start,
            length: (end - start) as usize,
            pages,
            initial,
        })
    }

    /// Called in the forked tracee before TRACEME. Reservations cannot replace
    /// host mappings; unmapped page gaps and both outer pages remain PROT_NONE.
    unsafe fn install(&self) -> bool {
        // SAFETY: the OS reserves the checked numeric range, without clobbering.
        let mapping = unsafe {
            libc::mmap(
                self.start as *mut libc::c_void,
                self.length,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE,
                -1,
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            return false;
        }
        if mapping as u64 != self.start {
            // SAFETY: older kernels may ignore NOREPLACE and return another mapping.
            unsafe { libc::munmap(mapping, self.length) };
            return false;
        }
        for &page in self.pages.keys() {
            // SAFETY: every page belongs to this newly created reservation.
            if unsafe {
                libc::mprotect(
                    page as *mut libc::c_void,
                    self.page_size as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                )
            } != 0
            {
                return false;
            }
        }
        for (address, bytes) in &self.initial {
            // SAFETY: validated logical regions are nonoverlapping and wholly writable.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), *address as *mut u8, bytes.len())
            };
        }
        for (&page, &protection) in &self.pages {
            // SAFETY: only fixture pages are protected; executable pages cease to be writable.
            if unsafe {
                libc::mprotect(
                    page as *mut libc::c_void,
                    self.page_size as usize,
                    protection,
                )
            } != 0
            {
                return false;
            }
        }
        true
    }
}

/// No tracee returns from this branch after its guest registers are installed.
/// Parent/child are single-threaded; no allocator lock is inherited while held.
unsafe fn tracee(plan: &MappingPlan, parent: libc::pid_t) -> ! {
    // Cover the interval before EXITKILL is installed. The parent identity
    // check closes the race where the tracer died before PR_SET_PDEATHSIG.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0
        || unsafe { libc::getppid() } != parent
    {
        unsafe { libc::_exit(109) }
    };
    // SAFETY: called only in the disposable forked child; inherited descriptors
    // (including supervisor pipes) must never be writable by fixture instructions.
    if unsafe { libc::syscall(libc::SYS_close_range, 0u32, u32::MAX, 0u32) } != 0 {
        unsafe { libc::_exit(110) }
    };
    if !unsafe { plan.install() } {
        unsafe { libc::_exit(111) }
    };
    if unsafe {
        libc::ptrace(
            libc::PTRACE_TRACEME,
            0,
            std::ptr::null_mut::<libc::c_void>(),
            std::ptr::null_mut::<libc::c_void>(),
        )
    } < 0
    {
        unsafe { libc::_exit(112) }
    };
    // TRACEME does not stop. The tracer waits for exactly this SIGSTOP before SETREGS.
    unsafe { libc::kill(libc::getpid(), libc::SIGSTOP) };
    // Reached only if the tracer incorrectly resumes host code. Never return to Rust ABI.
    unsafe { libc::_exit(113) }
}

struct Tracee {
    pid: libc::pid_t,
    reaped: bool,
}
impl Tracee {
    fn wait(&mut self, deadline: Instant) -> Result<i32, String> {
        let status = wait(self.pid, deadline)?;
        self.reaped = libc::WIFEXITED(status) || libc::WIFSIGNALED(status);
        Ok(status)
    }
}
impl Drop for Tracee {
    fn drop(&mut self) {
        // A waitpid exit result has already released this PID. Never signal a
        // potentially reused PID after the owned child was reaped.
        if self.reaped {
            return;
        }
        // SAFETY: pid is the exclusively owned child. SIGKILL works in ptrace-stop.
        unsafe { libc::kill(self.pid, libc::SIGKILL) };
        loop {
            let mut status = 0;
            let waited = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if waited == self.pid && (libc::WIFEXITED(status) || libc::WIFSIGNALED(status)) {
                break;
            }
            if waited < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                break;
            }
        }
    }
}

fn request(request: libc::c_uint, pid: libc::pid_t, data: *mut libc::c_void) -> Result<(), String> {
    // SAFETY: every caller uses its owned stopped tracee and a request-specific
    // buffer or integer option value; the unused addr argument is null.
    if unsafe { libc::ptrace(request, pid, std::ptr::null_mut::<libc::c_void>(), data) } < 0 {
        Err(format!(
            "ptrace request {request}: {}",
            io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn wait(pid: libc::pid_t, deadline: Instant) -> Result<i32, String> {
    loop {
        let mut status = 0;
        // SAFETY: wait only for the owned tracee; no unrelated children are reaped.
        let found = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if found == pid {
            return Ok(status);
        }
        if found < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(io::Error::last_os_error().to_string());
        }
        if Instant::now() >= deadline {
            return Err("tracee wait exceeded wall deadline".into());
        }
        std::thread::sleep(Duration::from_micros(100));
    }
}

fn registers(pid: libc::pid_t) -> Result<libc::user_regs_struct, String> {
    // SAFETY: kernel writes all fields of this ABI-defined register structure.
    let mut regs = unsafe { std::mem::zeroed::<libc::user_regs_struct>() };
    request(
        libc::PTRACE_GETREGS,
        pid,
        (&mut regs as *mut libc::user_regs_struct).cast(),
    )?;
    Ok(regs)
}

fn state(regs: &libc::user_regs_struct) -> LongModeState {
    LongModeState {
        gpr: [
            regs.rax, regs.rcx, regs.rdx, regs.rbx, regs.rsp, regs.rbp, regs.rsi, regs.rdi,
            regs.r8, regs.r9, regs.r10, regs.r11, regs.r12, regs.r13, regs.r14, regs.r15,
        ],
        rip: regs.rip,
        rflags: (regs.eflags & v2::INTEGER_FLAGS) | 2,
        fs_base: 0,
        gs_base: 0,
        fs_selector: 0,
        gs_selector: 0,
        ds_selector: 0,
        es_selector: 0,
        ds_unusable: false,
        es_unusable: false,
        gdtr: None,
        fs_cache: None,
        gs_cache: None,

        x87: None,
        sse: None,
    }
}

fn set_state(pid: libc::pid_t, input: &LongModeState) -> Result<(), String> {
    let mut regs = registers(pid)?;
    [
        regs.rax, regs.rcx, regs.rdx, regs.rbx, regs.rsp, regs.rbp, regs.rsi, regs.rdi, regs.r8,
        regs.r9, regs.r10, regs.r11, regs.r12, regs.r13, regs.r14, regs.r15,
    ] = input.gpr;
    regs.rip = input.rip;
    // Preserve kernel-owned selectors/bases and system flags. Clear debugger TF/RF.
    regs.eflags = (regs.eflags & !(v2::INTEGER_FLAGS | (1 << 8) | (1 << 16))) | input.rflags;
    regs.orig_rax = u64::MAX; // Disable syscall restart after the initial SIGSTOP.
    request(
        libc::PTRACE_SETREGS,
        pid,
        (&mut regs as *mut libc::user_regs_struct).cast(),
    )?;
    if state(&registers(pid)?) != *input {
        return Err("full initial GPR/RSP/RIP/flags readback differs".into());
    }
    Ok(())
}

fn read_remote(pid: libc::pid_t, address: u64, length: usize) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; length];
    let local = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: length,
    };
    let remote = libc::iovec {
        iov_base: address as *mut libc::c_void,
        iov_len: length,
    };
    // SAFETY: local iovec points to a writable owned buffer; remote address is
    // read by the kernel, never dereferenced in this tracer's address space.
    let read = unsafe { libc::process_vm_readv(pid, &local, 1, &remote, 1, 0) };
    if read != length as isize {
        return Err(format!(
            "snapshot read at {address:#x}: expected {length}, got {read}: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("String write");
    }
    result
}

fn capture(
    pid: libc::pid_t,
    fixture: &Fixture,
    exception_known: bool,
    rflags_defined: u64,
) -> Result<Snapshot, String> {
    let observed = state(&registers(pid)?);
    let memory = fixture
        .memory
        .iter()
        .map(|r| {
            Ok(MemoryRegionSnapshot {
                region_id: r.region_id.clone(),
                guest_address: r.guest_address,
                bytes_hex: hex(&read_remote(pid, r.guest_address.get(), r.length as usize)?),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Snapshot {
        state: (&observed).into(),
        rflags_defined: HexU64(rflags_defined),
        memory,
        exception: if exception_known {
            Observation::Available { value: None }
        } else {
            unavailable("host signal is not an architectural exception observation")
        },
    })
}

fn permission(permissions: MemoryPermissions, kind: AccessKind) -> bool {
    match kind {
        AccessKind::Fetch => permissions.execute,
        AccessKind::Read => permissions.read,
        AccessKind::Write => permissions.write && !permissions.execute,
    }
}

fn admits_range(fixture: &Fixture, address: u64, length: usize, kind: AccessKind) -> bool {
    let Some(end) = address.checked_add(length as u64) else {
        return false;
    };
    (address..end).all(|byte| {
        fixture.memory.iter().any(|r| {
            r.guest_address.get() <= byte
                && byte < r.guest_address.get() + u64::from(r.length)
                && permission(r.permissions, kind)
        })
    })
}

#[cfg(test)]
fn admission(
    fixture: &Fixture,
    decoder: &mut FlatMemory,
    actual: &LongModeState,
) -> Result<FlagEffects, String> {
    admission_with_tracee_memory(fixture, decoder, actual, None)
}

fn admission_with_tracee_memory(
    fixture: &Fixture,
    decoder: &mut FlatMemory,
    actual: &LongModeState,
    tracee_pid: Option<libc::pid_t>,
) -> Result<FlagEffects, String> {
    let instruction = long_decode::decode(decoder, actual.rip, AccessContext::default())
        .map_err(|e| format!("native instruction outside admitted decode slice: {e:?}"))?;
    if matches!(
        instruction.operation,
        Operation::RepScas { .. } | Operation::RepCmps { .. }
    ) {
        return Err("REP string comparison is outside the native execution profile".into());
    }
    if matches!(instruction.operation, Operation::RepMovs { .. }) {
        return Err("REP MOVS is outside the native execution profile".into());
    }
    if matches!(
        instruction.operation,
        Operation::Fninit
            | Operation::Fnclex
            | Operation::FnstswAx
            | Operation::FnstcwM16Rax
            | Operation::FnstswM16Rax
            | Operation::FldcwM16Rax
            | Operation::FldM32FpRax
            | Operation::FldM64FpRax
            | Operation::Fchs
            | Operation::FxchSti { .. }
            | Operation::FldSti { .. }
            | Operation::FfreeSti { .. }
            | Operation::Fabs
            | Operation::Ftst
            | Operation::Fxam
            | Operation::Frndint
            | Operation::FstM32FpRax
            | Operation::FstpM32FpRax
            | Operation::FstM64FpRax
            | Operation::FstpM64FpRax
            | Operation::FstpM80FpRax
            | Operation::FistM32IntRax
            | Operation::FistpM32IntRax
            | Operation::FistpM64IntRax
            | Operation::FistpM16IntRax
            | Operation::FistM16IntRax
            | Operation::FildM32IntRax
            | Operation::FildM16IntRax
            | Operation::FildM64IntRax
            | Operation::FcomSt { .. }
            | Operation::Fcomi { .. }
            | Operation::Fcomip { .. }
            | Operation::Fucomi { .. }
            | Operation::Fucomip { .. }
            | Operation::Fcmovb { .. }
            | Operation::Fcmovnb { .. }
            | Operation::Fcmovnu { .. }
            | Operation::FcompSt { .. }
            | Operation::Fcompp
            | Operation::Fucompp
            | Operation::Fucom
            | Operation::Fucomp
            | Operation::FmulSt0St { .. }
            | Operation::FdivSt0St { .. }
            | Operation::FdivrSt0St { .. }
            | Operation::FdivrStSt0 { .. }
            | Operation::FdivStSt0 { .. }
            | Operation::FdivpStSt0 { .. }
            | Operation::FdivrpStSt0 { .. }
            | Operation::FmulStSt0 { .. }
            | Operation::FaddSt0St { .. }
            | Operation::FsubSt0St { .. }
            | Operation::FsubrSt0St { .. }
            | Operation::FsubStSt0 { .. }
            | Operation::FsubrStSt0 { .. }
            | Operation::FaddStSt0 { .. }
            | Operation::FaddpStSt0 { .. }
            | Operation::FmulpStSt0 { .. }
            | Operation::FsubpStSt0 { .. }
            | Operation::FsubrpStSt0 { .. }
            | Operation::FstSti { .. }
            | Operation::FstpSti { .. }
    ) {
        return Err("x87 instruction is not admitted by the native backend".into());
    }
    if matches!(
        instruction.operation,
        Operation::MovFsLoad { .. }
            | Operation::LoadFsSelector
            | Operation::LoadGsSelector
            | Operation::MovFsSelectorToRax
            | Operation::MovGsSelectorToRax
            | Operation::VerwGdt
    ) {
        return Err("FS/GS segment form is not admitted by the native backend".into());
    }
    if !admits_range(
        fixture,
        actual.rip,
        instruction.length as usize,
        AccessKind::Fetch,
    ) {
        return Err("instruction fetch outside fixture executable bytes".into());
    }
    let next = actual.rip.wrapping_add(u64::from(instruction.length));
    let admit_operand = |operand: Operand, kind: AccessKind| -> Result<(), String> {
        if let Operand::Memory(memory) = operand {
            let address = memory
                .access_address_48(actual, next)
                .map_err(|e| format!("native memory address outside positive profile: {e:?}"))?;
            if !admits_range(fixture, address, memory.size.bytes(), kind) {
                return Err("native memory access outside fixture range or permission".into());
            }
        }
        Ok(())
    };
    match instruction.operation {
        Operation::MovImmediate { dst, .. } => admit_operand(dst, AccessKind::Write),
        Operation::Mov { dst, src } => {
            admit_operand(dst, AccessKind::Write)?;
            admit_operand(src, AccessKind::Read)
        }
        Operation::Extend { src, .. } => admit_operand(src, AccessKind::Read),
        // LEA forms an effective offset only. It neither reads the address nor
        // requires it to be canonical/mapped. XCHG is decoded only for registers.
        Operation::Lea { .. }
        | Operation::Xchg { .. }
        | Operation::Nop
        | Operation::SignExtendAccumulator { .. }
        | Operation::Bswap { .. } => Ok(()),
        Operation::PopFlags => {
            if actual.rflags & (3 << 12) != 0 {
                return Err("native POPF current IOPL is outside the CPL3/IOPL0 profile".into());
            }
            let rsp = actual.gpr[od_isa::Register::Rsp as usize];
            let width = instruction.operand_size;
            if !admits_range(fixture, rsp, width.bytes(), AccessKind::Read) {
                return Err("native POPF stack read outside readable fixture memory".into());
            }
            let value = if let Some(pid) = tracee_pid {
                let bytes = read_remote(pid, rsp, width.bytes())?;
                let mut value = [0u8; 8];
                value[..width.bytes()].copy_from_slice(&bytes);
                u64::from_le_bytes(value)
            } else {
                decoder
                    .read_integer(rsp, AccessContext::default(), width)
                    .map_err(|e| format!("native POPF fixture stack read failed: {e:?}"))?
            };
            let unsupported_flags = (1 << 8)
                | (1 << 14)
                | if width == OperandSize::Qword {
                    1 << 18
                } else {
                    0
                };
            if (value | actual.rflags) & unsupported_flags != 0 {
                return Err("native POPF fixture sets out-of-profile TF, NT, or AC".into());
            }
            Ok(())
        }
        Operation::PushFlags => {
            let rsp = actual.gpr[od_isa::Register::Rsp as usize];
            let width = instruction.operand_size.bytes();
            let address = rsp.wrapping_sub(width as u64);
            if !admits_range(fixture, address, width, AccessKind::Write) {
                return Err("native PUSHF stack write outside writable fixture memory".into());
            }
            Ok(())
        }
        Operation::Movs { size } => {
            let admitted = instruction.opcode.map == OpcodeMap::Primary
                && ((instruction.opcode.byte == 0xa4
                    && instruction.prefixes == Prefixes::default()
                    && size == OperandSize::Byte)
                    || (instruction.opcode.byte == 0xa5
                        && instruction.prefixes
                            == (Prefixes {
                                operand_override: true,
                                ..Prefixes::default()
                            })
                        && size == OperandSize::Word)
                    || (instruction.opcode.byte == 0xa5
                        && instruction.prefixes == Prefixes::default()
                        && size == OperandSize::Dword)
                    || (instruction.opcode.byte == 0xa5
                        && instruction.prefixes.group1.is_none()
                        && instruction.prefixes.segment.is_none()
                        && !instruction.prefixes.address_override
                        && instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                        && size == OperandSize::Qword));
            if !admitted {
                return Err(
                    "native MOVS admission requires a supported MOVSB, MOVSW, MOVSD, or MOVSQ form"
                        .into(),
                );
            }
            let implicit_memory = |register| {
                Operand::Memory(long_decode::MemoryOperand {
                    base: long_decode::AddressBase::Register(register),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size,
                })
            };
            admit_operand(implicit_memory(od_isa::Register::Rsi), AccessKind::Read)?;
            admit_operand(implicit_memory(od_isa::Register::Rdi), AccessKind::Write)
        }
        Operation::Stos { size } => {
            if instruction.opcode.map != OpcodeMap::Primary
                || instruction.opcode.byte != 0xaa
                || instruction.prefixes != Prefixes::default()
                || size != OperandSize::Byte
            {
                return Err("native STOS admission requires a bare STOSB form".into());
            }
            let destination = Operand::Memory(long_decode::MemoryOperand {
                base: long_decode::AddressBase::Register(od_isa::Register::Rdi),
                index: None,
                scale: 1,
                displacement: 0,
                address_size: OperandSize::Qword,
                size,
            });
            admit_operand(destination, AccessKind::Write)
        }
        Operation::RepStos { size } => {
            if instruction.opcode.map != OpcodeMap::Primary
                || instruction.opcode.byte != 0xaa
                || instruction.length != 2
                || instruction.prefixes
                    != (long_decode::Prefixes {
                        group1: Some(0xf3),
                        ..long_decode::Prefixes::default()
                    })
                || size != OperandSize::Byte
            {
                return Err("native REP STOS admission requires exact F3 AA bytes".into());
            }
            let count = actual.gpr[od_isa::Register::Rcx as usize];
            if count == 0 {
                Ok(())
            } else if count > 8 {
                return Err("native REP STOS admission is limited to eight iterations".into());
            } else {
                let rdi = actual.gpr[od_isa::Register::Rdi as usize];
                let start = if actual.rflags & (1 << 10) == 0 {
                    rdi
                } else {
                    rdi.checked_sub(count - 1)
                        .ok_or_else(|| "native REP STOS destination range underflows".to_string())?
                };
                if !admits_range(fixture, start, count as usize, AccessKind::Write) {
                    return Err(
                        "native REP STOS destination range is unmapped or not writable".into(),
                    );
                }
                Ok(())
            }
        }
        Operation::StackRegister { push, .. } => {
            let rsp = actual.gpr[od_isa::Register::Rsp as usize];
            let length = instruction.operand_size.bytes();
            let address = if push {
                rsp.wrapping_sub(length as u64)
            } else {
                rsp
            };
            if !admits_range(
                fixture,
                address,
                length,
                if push {
                    AccessKind::Write
                } else {
                    AccessKind::Read
                },
            ) {
                return Err("native stack access outside fixture range or permission".into());
            }
            Ok(())
        }
        Operation::PushImmediate { .. } => {
            let length = instruction.operand_size.bytes();
            let address = actual.gpr[od_isa::Register::Rsp as usize].wrapping_sub(length as u64);
            if !admits_range(fixture, address, length, AccessKind::Write) {
                return Err("native stack access outside fixture range or permission".into());
            }
            Ok(())
        }
        Operation::PushMemory { source }
            if instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xff
                && instruction.prefixes.group1.is_none()
                && !matches!(instruction.prefixes.segment, Some(0x64 | 0x65))
                && instruction.operand_size == source.size
                && match source.size {
                    OperandSize::Qword => {
                        !instruction.prefixes.operand_override
                            || instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                    }
                    OperandSize::Word => {
                        instruction.prefixes.operand_override
                            && !instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                    }
                    _ => false,
                } =>
        {
            let source_address = source.access_address_48(actual, next).map_err(|e| {
                format!("native PUSH source address outside positive profile: {e:?}")
            })?;
            let length = source.size.bytes() as usize;
            if !admits_range(fixture, source_address, length, AccessKind::Read) {
                return Err("native PUSH source outside readable fixture memory".into());
            }
            if let Some(pid) = tracee_pid {
                let _source = read_remote(pid, source_address, length)?;
            } else {
                let mut source_bytes = vec![0; length];
                decoder
                    .read(source_address, AccessContext::default(), &mut source_bytes)
                    .map_err(|e| format!("native PUSH fixture source read failed: {e:?}"))?;
            }

            let address = actual.gpr[od_isa::Register::Rsp as usize].wrapping_sub(length as u64);
            let Some(last) = address.checked_add(length as u64 - 1) else {
                return Err("native PUSH stack range overflows".into());
            };
            if !(((address as i64) >> 47 == 0 || (address as i64) >> 47 == -1)
                && ((last as i64) >> 47 == 0 || (last as i64) >> 47 == -1)
                && admits_range(fixture, address, length, AccessKind::Write))
            {
                return Err("native PUSH stack access outside fixture range or permission".into());
            }
            Ok(())
        }
        Operation::PopMemory { destination }
            if instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0x8f
                && instruction.prefixes.group1.is_none()
                && instruction.operand_size == destination.size
                && match destination.size {
                    OperandSize::Qword => {
                        !instruction.prefixes.operand_override
                            || instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                    }
                    OperandSize::Word => {
                        instruction.prefixes.operand_override
                            && !instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                    }
                    _ => false,
                }
                && !matches!(instruction.prefixes.segment, Some(0x64 | 0x65)) =>
        {
            let old_rsp = actual.gpr[od_isa::Register::Rsp as usize];
            let length = destination.size.bytes();
            if !admits_range(fixture, old_rsp, length, AccessKind::Read) {
                return Err("native POP source stack outside readable fixture memory".into());
            }
            let mut source_bytes = vec![0; length];
            if let Some(pid) = tracee_pid {
                source_bytes.copy_from_slice(&read_remote(pid, old_rsp, length)?);
            } else {
                decoder
                    .read(old_rsp, AccessContext::default(), &mut source_bytes)
                    .map_err(|e| format!("native POP fixture stack read failed: {e:?}"))?;
            }
            let mut post_pop = actual.clone();
            post_pop.gpr[od_isa::Register::Rsp as usize] = old_rsp.wrapping_add(length as u64);
            let destination_address = destination
                .access_address_48(&post_pop, next)
                .map_err(|e| format!("native POP destination address invalid: {e:?}"))?;
            if !admits_range(fixture, destination_address, length, AccessKind::Write) {
                return Err("native POP destination outside writable fixture memory".into());
            }
            Ok(())
        }
        Operation::Call { displacement }
            if instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xe8
                && instruction.prefixes == Prefixes::default() =>
        {
            let address = actual.gpr[od_isa::Register::Rsp as usize].wrapping_sub(8);
            if !admits_range(fixture, address, 8, AccessKind::Write) {
                return Err("native CALL stack access outside fixture range or permission".into());
            }
            let target = next.wrapping_add_signed(i64::from(displacement));
            if !((target as i64) >> 47 == 0 || (target as i64) >> 47 == -1) {
                return Err("native CALL target is noncanonical".into());
            }
            if fixture.stop_rip.map(HexU64::get) != Some(target)
                && !admits_range(fixture, target, 1, AccessKind::Fetch)
            {
                return Err("native CALL target outside fixture code/stop boundary".into());
            }
            if fixture.stop_rip.map(HexU64::get) != Some(next)
                && !admits_range(fixture, next, 1, AccessKind::Fetch)
            {
                return Err("native CALL return address outside fixture code/stop boundary".into());
            }
            Ok(())
        }
        Operation::Return { .. }
            if instruction.opcode.map == OpcodeMap::Primary
                && matches!(instruction.opcode.byte, 0xc2 | 0xc3)
                && (instruction.prefixes == Prefixes::default()
                    || (instruction.opcode.byte == 0xc3
                        && instruction.prefixes
                            == (Prefixes {
                                operand_override: true,
                                ..Prefixes::default()
                            }))) =>
        {
            let address = actual.gpr[od_isa::Register::Rsp as usize];
            if !admits_range(fixture, address, 8, AccessKind::Read) {
                return Err("native RET stack access outside fixture range or permission".into());
            }
            let bytes = if let Some(pid) = tracee_pid {
                read_remote(pid, address, 8)?
            } else {
                let mut bytes = [0; 8];
                decoder
                    .read(address, AccessContext::default(), &mut bytes)
                    .map_err(|e| format!("native RET fixture stack read failed: {e:?}"))?;
                bytes.to_vec()
            };
            let target = u64::from_le_bytes(bytes.try_into().expect("8-byte return target"));
            if !((target as i64) >> 47 == 0 || (target as i64) >> 47 == -1) {
                return Err("native RET target is noncanonical".into());
            }
            if fixture.stop_rip.map(HexU64::get) != Some(target)
                && !admits_range(fixture, target, 1, AccessKind::Fetch)
            {
                return Err("native RET target outside fixture code/stop boundary".into());
            }
            Ok(())
        }
        Operation::IndirectCall { operand }
            if instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xff
                && instruction.prefixes.group1.is_none()
                && !matches!(instruction.prefixes.segment, Some(0x64 | 0x65))
                && !instruction.prefixes.operand_override
                && match operand {
                    Operand::Register(_) => {
                        instruction.prefixes.segment.is_none()
                            && !instruction.prefixes.address_override
                            && matches!(instruction.prefixes.rex, None | Some(0x41))
                    }
                    Operand::Memory(_) => true,
                    Operand::Immediate(_) => false,
                } =>
        {
            let target = match operand {
                Operand::Register(register) => actual.read(register),
                Operand::Memory(memory) => {
                    let address = memory.access_address_48(actual, next).map_err(|e| {
                        format!("native indirect CALL source address invalid: {e:?}")
                    })?;
                    if !admits_range(fixture, address, 8, AccessKind::Read) {
                        return Err(
                            "native indirect CALL source outside readable fixture memory".into(),
                        );
                    }
                    let bytes = if let Some(pid) = tracee_pid {
                        read_remote(pid, address, 8)?
                    } else {
                        let mut bytes = [0; 8];
                        decoder
                            .read(address, AccessContext::default(), &mut bytes)
                            .map_err(|e| {
                                format!("native indirect CALL fixture source read failed: {e:?}")
                            })?;
                        bytes.to_vec()
                    };
                    u64::from_le_bytes(bytes.try_into().expect("8-byte indirect CALL target"))
                }
                Operand::Immediate(_) => {
                    return Err("native indirect CALL immediate operand is invalid".into());
                }
            };
            if !((target as i64) >> 47 == 0 || (target as i64) >> 47 == -1) {
                return Err("native indirect CALL target is noncanonical".into());
            }
            if fixture.stop_rip.map(HexU64::get) != Some(target)
                && !admits_range(fixture, target, 1, AccessKind::Fetch)
            {
                return Err(
                    "native indirect CALL target outside fixture code/stop boundary".into(),
                );
            }
            if fixture.stop_rip.map(HexU64::get) != Some(next)
                && !admits_range(fixture, next, 1, AccessKind::Fetch)
            {
                return Err(
                    "native indirect CALL return address outside fixture code/stop boundary".into(),
                );
            }
            let address = actual.gpr[od_isa::Register::Rsp as usize].wrapping_sub(8);
            let Some(last) = address.checked_add(7) else {
                return Err("native indirect CALL stack range overflows".into());
            };
            if !(((address as i64) >> 47 == 0 || (address as i64) >> 47 == -1)
                && ((last as i64) >> 47 == 0 || (last as i64) >> 47 == -1)
                && admits_range(fixture, address, 8, AccessKind::Write))
            {
                return Err(
                    "native indirect CALL stack access outside fixture range or permission".into(),
                );
            }
            Ok(())
        }
        Operation::Bit { kind, dst, offset } => {
            let destination = match (dst, offset) {
                (Operand::Memory(mut memory), BitOffset::Register(view)) => {
                    let raw = actual.read(view) & view.size().mask();
                    let signed = match view.size() {
                        od_isa::OperandSize::Word => raw as u16 as i16 as i128,
                        od_isa::OperandSize::Dword => raw as u32 as i32 as i128,
                        od_isa::OperandSize::Qword => raw as i64 as i128,
                        od_isa::OperandSize::Byte => raw as u8 as i8 as i128,
                    };
                    let width = memory.size.bytes() as i128 * 8;
                    let delta = signed.div_euclid(width) * memory.size.bytes() as i128;
                    memory.displacement = memory.displacement.wrapping_add(delta as i64);
                    Operand::Memory(memory)
                }
                _ => dst,
            };
            admit_operand(destination, AccessKind::Read)?;
            if kind != BitKind::Test {
                admit_operand(destination, AccessKind::Write)?;
            }
            Ok(())
        }
        Operation::BitScan { src, .. } => admit_operand(src, AccessKind::Read),
        Operation::Arithmetic { kind, dst, src } => {
            // Admission uses actual hardware GPRs only to bound accesses.
            // It does not evaluate the instruction's result or flags.
            admit_operand(dst, AccessKind::Read)?;
            if kind != ArithmeticKind::Cmp {
                admit_operand(dst, AccessKind::Write)?;
            }
            admit_operand(src, AccessKind::Read)
        }
        Operation::Logic { kind, dst, src } => {
            admit_operand(dst, AccessKind::Read)?;
            if kind != LogicKind::Test {
                admit_operand(dst, AccessKind::Write)?;
            }
            admit_operand(src, AccessKind::Read)
        }
        Operation::Shift {
            dst, source, count, ..
        } => {
            // Shift/rotate destinations are read-modify-write. SHLD/SHRD also
            // consume their register source; immediate/CL counts are values,
            // not memory operands.
            admit_operand(dst, AccessKind::Read)?;
            admit_operand(dst, AccessKind::Write)?;
            if let Some(source) = source {
                admit_operand(source, AccessKind::Read)?;
            }
            admit_operand(count, AccessKind::Read)
        }
        Operation::SetCondition { dst, .. }
            if instruction.opcode.map == OpcodeMap::Escape0f
                && (0x90..=0x9f).contains(&instruction.opcode.byte)
                && instruction.prefixes.group1.is_none() =>
        {
            admit_operand(dst, AccessKind::Write)
        }
        Operation::ConditionalMove { src, .. }
            if instruction.opcode.map == OpcodeMap::Escape0f
                && (0x40..=0x4f).contains(&instruction.opcode.byte)
                && instruction.prefixes.group1.is_none() =>
        {
            // CMOVcc reads the source even when its condition is false.
            admit_operand(src, AccessKind::Read)
        }
        Operation::Mul { operand, .. }
        | Operation::Divide {
            divisor: operand, ..
        } => {
            // MUL/IMUL and DIV/IDIV consume one explicit r/m operand. Their
            // accumulator/high-half registers are implicit and never map memory.
            admit_operand(operand, AccessKind::Read)
        }
        Operation::Imul { src, .. } | Operation::ImulImmediate { src, .. } => {
            admit_operand(src, AccessKind::Read)
        }
        Operation::Branch {
            condition,
            displacement,
        } if instruction.opcode.map == OpcodeMap::Primary
            && instruction.prefixes == Prefixes::default() =>
        {
            let address = if condition.evaluate(actual.rflags) {
                next.wrapping_add_signed(i64::from(displacement))
            } else {
                next
            };
            if fixture.stop_rip.map(HexU64::get) != Some(address)
                && !admits_range(fixture, address, 1, AccessKind::Fetch)
            {
                return Err("native Jcc target outside fixture code/stop boundary".into());
            }
            Ok(())
        }
        Operation::Branch {
            condition,
            displacement,
        } if instruction.opcode.map == OpcodeMap::Escape0f
            && instruction.prefixes == Prefixes::default() =>
        {
            let address = if condition.evaluate(actual.rflags) {
                next.wrapping_add_signed(i64::from(displacement))
            } else {
                next
            };
            if fixture.stop_rip.map(HexU64::get) != Some(address)
                && !admits_range(fixture, address, 1, AccessKind::Fetch)
            {
                return Err("native Jcc target outside fixture code/stop boundary".into());
            }
            Ok(())
        }
        Operation::Jump { displacement }
            if instruction.opcode.map == OpcodeMap::Primary
                && instruction.prefixes == Prefixes::default() =>
        {
            let address = next.wrapping_add_signed(i64::from(displacement));
            if fixture.stop_rip.map(HexU64::get) != Some(address)
                && !admits_range(fixture, address, 1, AccessKind::Fetch)
            {
                return Err("native JMP target outside fixture code/stop boundary".into());
            }
            Ok(())
        }
        Operation::IndirectJump { operand }
            if instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xff
                && instruction.prefixes.group1.is_none()
                && !matches!(instruction.prefixes.segment, Some(0x64 | 0x65))
                && !instruction.prefixes.operand_override =>
        {
            let target = match operand {
                Operand::Register(view) => actual.read(view),
                Operand::Memory(memory) => {
                    let address = memory.access_address_48(actual, next).map_err(|e| {
                        format!("native indirect JMP source address invalid: {e:?}")
                    })?;
                    if !admits_range(fixture, address, 8, AccessKind::Read) {
                        return Err(
                            "native indirect JMP source outside readable fixture memory".into()
                        );
                    }
                    let bytes = if let Some(pid) = tracee_pid {
                        read_remote(pid, address, 8)?
                    } else {
                        let mut bytes = [0; 8];
                        decoder
                            .read(address, AccessContext::default(), &mut bytes)
                            .map_err(|e| {
                                format!("native indirect JMP fixture source read failed: {e:?}")
                            })?;
                        bytes.to_vec()
                    };
                    u64::from_le_bytes(bytes.try_into().expect("8-byte indirect target"))
                }
                Operand::Immediate(_) => {
                    return Err("native indirect JMP immediate operand is invalid".into());
                }
            };
            if !((target as i64) >> 47 == 0 || (target as i64) >> 47 == -1) {
                return Err("native indirect JMP target is noncanonical".into());
            }
            if fixture.stop_rip.map(HexU64::get) != Some(target)
                && !admits_range(fixture, target, 1, AccessKind::Fetch)
            {
                return Err("native indirect JMP target outside fixture code/stop boundary".into());
            }
            Ok(())
        }
        _ => Err(
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
                .into(),
        ),
    }?;
    let effects = match instruction.operation {
        Operation::Shift { count, .. } => {
            let raw_count = match count {
                Operand::Immediate(value) => value as u8,
                Operand::Register(view) => actual.read(view) as u8,
                Operand::Memory(_) => {
                    return Err("memory shift count is outside native profile".into());
                }
            };
            instruction.flag_effects_for_count(raw_count)
        }
        _ => instruction.flag_effects(),
    };
    Ok(effects)
}

fn trace(loaded: &LoadedFixture, plan: &MappingPlan) -> Result<RunResult, String> {
    let fixture = &loaded.fixture;
    let deadline = Instant::now() + Duration::from_millis(u64::from(fixture.wall_timeout_ms));
    let input = fixture
        .initial_state
        .to_cpu_for(fixture.profile)
        .map_err(|e| e.to_string())?;
    let mut decoder = FlatMemory::from_fixture(fixture).map_err(|e| e.to_string())?;
    // SAFETY: called in the single-threaded supervised tracer. The forked child
    // only installs its own fixture mappings and enters a ptrace stop.
    let parent = unsafe { libc::getpid() };
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        unsafe { tracee(plan, parent) }
    };
    let mut owned = Tracee { pid, reaped: false };
    let status = owned.wait(deadline)?;
    if let Some(result) = terminal_signal_result(loaded, status) {
        return Ok(result);
    }
    if !libc::WIFSTOPPED(status) || libc::WSTOPSIG(status) != libc::SIGSTOP {
        return Err(format!(
            "tracee failed before initial SIGSTOP: status {status:#x}"
        ));
    }
    request(
        libc::PTRACE_SETOPTIONS,
        pid,
        libc::PTRACE_O_EXITKILL as usize as *mut libc::c_void,
    )?;
    set_state(pid, &input)?;
    let mut rflags_defined = od_isa::INTEGER_FLAGS;
    let initial = capture(pid, fixture, true, rflags_defined)?;
    if initial
        .memory
        .iter()
        .zip(&fixture.memory)
        .any(|(actual, expected)| {
            actual.bytes_hex != expected.initial_bytes_hex.to_ascii_lowercase()
        })
    {
        return Err("initial tracee memory readback differs".into());
    }
    let mut count = 0u64;
    let (outcome, exception_known) = loop {
        let current = state(&registers(pid)?);
        if fixture.stop_rip.map(HexU64::get) == Some(current.rip) {
            break (Outcome::StopAddress {}, true);
        }
        if count == fixture.instruction_budget {
            break (Outcome::InstructionBudgetExhausted {}, true);
        }
        if Instant::now() >= deadline {
            break (
                Outcome::TimedOut {
                    reason: "native instruction loop reached wall deadline".into(),
                },
                true,
            );
        }
        let effects = match admission_with_tracee_memory(fixture, &mut decoder, &current, Some(pid))
        {
            Ok(effects) => effects,
            Err(reason) => break (Outcome::Unsupported { reason }, true),
        };
        let is_rep_stos = matches!(
            long_decode::decode(&mut decoder, current.rip, AccessContext::default()),
            Ok(instruction) if matches!(instruction.operation, Operation::RepStos { .. })
        );
        request(libc::PTRACE_SINGLESTEP, pid, std::ptr::null_mut())?;
        let status = owned.wait(deadline)?;
        if let Some(result) = terminal_signal_result(loaded, status) {
            return Ok(result);
        }
        if !libc::WIFSTOPPED(status) {
            return Err(format!(
                "tracee terminated during a step: status {status:#x}"
            ));
        }
        let signal = libc::WSTOPSIG(status);
        // SAFETY: GETSIGINFO fills an ABI-defined initialized siginfo_t buffer.
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        request(
            libc::PTRACE_GETSIGINFO,
            pid,
            (&mut info as *mut libc::siginfo_t).cast(),
        )?;
        if signal != libc::SIGTRAP || info.si_code != libc::TRAP_TRACE {
            break (
                Outcome::Signaled {
                    signal: signal as u32,
                    reason: format!(
                        "unexpected tracee host signal; si_code={}; no guest exception inferred",
                        info.si_code
                    ),
                },
                false,
            );
        }
        if is_rep_stos {
            let after = state(&registers(pid)?);
            if rep_stos_iteration_is_intermediate(
                current.rip,
                current.gpr[od_isa::Register::Rcx as usize],
                after.rip,
                after.gpr[od_isa::Register::Rcx as usize],
            ) {
                continue;
            }
        }
        count += 1;
        rflags_defined = effects.apply(rflags_defined);
    };
    let snapshot = capture(pid, fixture, exception_known, rflags_defined)?;
    drop(owned); // Kill and reap before returning a result to the outer supervisor.
    let mut result = empty_result(loaded, outcome);
    result.instructions_executed = if exception_known {
        Observation::Available {
            value: HexU64(count),
        }
    } else {
        unavailable("unexpected host stop prevents an exact retired instruction count")
    };
    result.snapshot = Observation::Available { value: snapshot };
    result.validate_for(loaded).map_err(|e| e.to_string())?;
    Ok(result)
}

fn execute(loaded: &LoadedFixture) -> RunResult {
    if matches!(
        loaded.fixture.profile,
        v2::Profile::LongFsBaseV1
            | v2::Profile::LongGsBaseV1
            | v2::Profile::LongFsSelectorV1
            | v2::Profile::LongGsSelectorV1
            | v2::Profile::LongVerwGdtV1
            | v2::Profile::LongFsLoadV1
            | v2::Profile::LongGsLoadV1
            | v2::Profile::LongNullDsEsV1
            | v2::Profile::LongX87StateV1
            | v2::Profile::LongX87SseStateV1
            | v2::Profile::LongMovupsRegV1
            | v2::Profile::LongMovapsRegV1
            | v2::Profile::LongMovapsReverseRegV1
            | v2::Profile::LongMovapdRegV1
            | v2::Profile::LongMovapdReverseRegV1
            | v2::Profile::LongMovupdRegV1
            | v2::Profile::LongMovupdReverseRegV1
            | v2::Profile::LongXorpsRegV1
            | v2::Profile::LongXorpdRegV1
            | v2::Profile::LongAndpdRegV1
            | v2::Profile::LongPxorRegV1
            | v2::Profile::LongPandRegV1
            | v2::Profile::LongPandnRegV1
            | v2::Profile::LongPorRegV1
            | v2::Profile::LongPaddbRegV1
            | v2::Profile::LongPaddwRegV1
            | v2::Profile::LongPsubwRegV1
            | v2::Profile::LongPadduswRegV1
            | v2::Profile::LongPsubuswRegV1
            | v2::Profile::LongPsubbRegV1
            | v2::Profile::LongPaddusbRegV1
            | v2::Profile::LongPaddsbRegV1
            | v2::Profile::LongPsubsbRegV1
            | v2::Profile::LongPaddswRegV1
            | v2::Profile::LongPsubswRegV1
            | v2::Profile::LongPadddRegV1
            | v2::Profile::LongPsubdRegV1
            | v2::Profile::LongPaddqRegV1
            | v2::Profile::LongPsubqRegV1
            | v2::Profile::LongPcmpeqbRegV1
            | v2::Profile::LongPcmpeqwRegV1
            | v2::Profile::LongPcmpeqdRegV1
            | v2::Profile::LongPcmpgtbRegV1
            | v2::Profile::LongPcmpgtwRegV1
            | v2::Profile::LongPcmpgtdRegV1
            | v2::Profile::LongPunpcklbwRegV1
            | v2::Profile::LongPunpckhbwRegV1
            | v2::Profile::LongPunpcklwdRegV1
            | v2::Profile::LongPunpckhwdRegV1
            | v2::Profile::LongPunpckldqRegV1
            | v2::Profile::LongPunpckhdqRegV1
            | v2::Profile::LongPunpcklqdqRegV1
            | v2::Profile::LongPunpckhqdqRegV1
            | v2::Profile::LongPacksswbRegV1
            | v2::Profile::LongPackuswbRegV1
            | v2::Profile::LongPackssdwRegV1
            | v2::Profile::LongPsllwImmRegV1
            | v2::Profile::LongPslldImmRegV1
            | v2::Profile::LongPsllqImmRegV1
            | v2::Profile::LongPslldqImmRegV1
            | v2::Profile::LongPsrldqImmRegV1
            | v2::Profile::LongPsrlqImmRegV1
            | v2::Profile::LongPsrldImmRegV1
            | v2::Profile::LongPsrlwImmRegV1
            | v2::Profile::LongPsrawImmRegV1
            | v2::Profile::LongPsradImmRegV1
            | v2::Profile::LongPsubusbRegV1
            | v2::Profile::LongAndpsRegV1
            | v2::Profile::LongOrpsRegV1
            | v2::Profile::LongAndnpsRegV1
            | v2::Profile::LongMovssRegV1
            | v2::Profile::LongMovsdRegV1
            | v2::Profile::LongMovsdMemLoadV1
            | v2::Profile::LongMovsdMemStoreV1
            | v2::Profile::LongMovssStoreRegV1
            | v2::Profile::LongMovssMemLoadV1
            | v2::Profile::LongMovssMemStoreV1
            | v2::Profile::LongMovupsMemLoadV1
            | v2::Profile::LongMovupsMemStoreV1
            | v2::Profile::LongMovupsMemBaseV1
    ) {
        return empty_result(
            loaded,
            Outcome::Unsupported {
                reason:
                    "this profile is local-only; native state does not observe every profile field"
                        .into(),
            },
        );
    }
    // SAFETY: sysconf has no pointer parameters.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let plan = match u64::try_from(page)
        .ok()
        .and_then(|page| MappingPlan::new(&loaded.fixture, page).ok())
    {
        Some(plan) => plan,
        None => {
            return empty_result(
                loaded,
                Outcome::Unsupported {
                    reason: MappingPlan::new(&loaded.fixture, page.max(0) as u64)
                        .err()
                        .unwrap_or_else(|| "unsupported system page size".into()),
                },
            );
        }
    };
    match trace(loaded, &plan) {
        Ok(result) => result,
        Err(reason) if reason == "tracee wait exceeded wall deadline" => {
            empty_result(loaded, Outcome::TimedOut { reason })
        }
        Err(reason) => empty_result(loaded, Outcome::HarnessError { reason }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_testkit::case::{MemoryRegionSpec, Provenance};

    fn region(
        id: &str,
        address: u64,
        bytes: &[u8],
        write: bool,
        execute: bool,
    ) -> MemoryRegionSpec {
        MemoryRegionSpec {
            region_id: id.into(),
            guest_address: HexU64(address),
            length: bytes.len() as u32,
            permissions: MemoryPermissions {
                read: true,
                write,
                execute,
            },
            initial_bytes_hex: hex(bytes),
            comparison_mask_hex: "ff".repeat(bytes.len()),
        }
    }

    fn fixture(code: &[u8]) -> LoadedFixture {
        let mut state = LongModeState::default();
        for (index, register) in state.gpr.iter_mut().enumerate() {
            *register = 0x12340000 + index as u64 * 0x1111;
        }
        state.gpr[3] = 0x200000;
        state.gpr[4] = 0x301ff8;
        state.rip = 0x100000;
        state.rflags = 0x8d7;
        let input = Fixture {
            schema_version: 2,
            case_id: "native-v2-unit".into(),
            profile: v2::Profile::LongIntegerV1,
            provenance: Provenance {
                kind: "manual".into(),
                source: "handwritten test bytes".into(),
                specification: "Intel SDM".into(),
                specification_version: "093".into(),
            },
            seed: None,
            converted_from_v1_sha256: None,
            initial_state: (&state).into(),
            memory: vec![
                region("code", 0x100000, code, false, true),
                region("data", 0x200000, &[0; 16], true, false),
                region("stack", 0x300000, &[0; 8192], true, false),
            ],
            stop_rip: Some(HexU64(0x100000 + code.len() as u64)),
            instruction_budget: 32,
            wall_timeout_ms: 2000,
            rflags_mask: HexU64(v2::INTEGER_FLAGS),
            expected_exit: v2::ExpectedExit::StopAddress,
        };
        Fixture::from_json(&input.to_json().unwrap()).unwrap()
    }

    #[test]
    fn physical_rep_stosb_counts_as_one_instruction_after_all_iterations() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for (name, expected_rdi, expected_memory) in [
            (
                "s2_19a_rep_stosb_df0",
                0x0020_0005,
                "00118888885566778899aabbccddeeff",
            ),
            (
                "s2_19a_rep_stosb_df1",
                0x0020_0004,
                "00112233448888888899aabbccddeeff",
            ),
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let result = execute(&loaded);
            assert_eq!(
                result.outcome,
                Outcome::StopAddress {},
                "{name}: {result:?}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(1) },
                "{name}"
            );
            let Observation::Available { value: snapshot } = result.snapshot else {
                panic!("snapshot unavailable: {name}");
            };
            assert_eq!(snapshot.state.rip, loaded.fixture.stop_rip.unwrap());
            assert_eq!(
                snapshot.state.gpr[od_isa::Register::Rcx as usize],
                HexU64(0)
            );
            assert_eq!(
                snapshot.state.gpr[od_isa::Register::Rdi as usize],
                HexU64(expected_rdi)
            );
            assert_eq!(
                snapshot
                    .memory
                    .iter()
                    .find(|region| region.region_id == "data")
                    .unwrap()
                    .bytes_hex,
                expected_memory,
                "{name}"
            );
        }
    }

    #[test]
    fn direct_control_flow_fixtures_are_admitted_only_to_their_selected_target() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_16a_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 68);
    }

    #[test]
    fn indirect_jmp_fixtures_admit_register_targets_and_reject_noncanonical_native_fault() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in ["s2_16d_jmp_rax_target", "s2_16d_jmp_r8_target"] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} indirect JMP admission: {error}"));
            let Operation::IndirectJump { operand } =
                long_decode::decode(&mut decoder, actual.rip, AccessContext::default())
                    .unwrap()
                    .operation
            else {
                panic!("{name} should decode as indirect JMP")
            };
            actual.rip = match operand {
                Operand::Register(view) => actual.read(view),
                _ => panic!("{name} must use register source"),
            };
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} target decode admission: {error}"));
        }

        let loaded = Fixture::from_json(
            &std::fs::read(directory.join("s2_16d_jmp_noncanonical.case.json")).unwrap(),
        )
        .unwrap();
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("noncanonical")
        );
    }

    #[test]
    fn indirect_call_register_fixtures_admit_selected_targets_and_bounded_stack() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in ["s2_17d_call_rax", "s2_17d_call_r8", "s2_17d_call_budget"] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let mut retired = 0;
            while loaded.fixture.stop_rip.map(HexU64::get) != Some(actual.rip)
                && retired < loaded.fixture.instruction_budget
            {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} at {:#x}: {error}", actual.rip));
                assert!(matches!(
                    od_isa::execute::step(&mut actual, &mut decoder, AccessContext::default()),
                    Ok(od_isa::execute::StepOutcome::Branched)
                ));
                retired += 1;
            }
            assert_eq!(retired, loaded.fixture.instruction_budget, "{name}");
        }

        for (name, expected_error) in [
            ("s2_17d_call_noncanonical", "noncanonical"),
            ("s2_17d_call_stack_fault", "stack access"),
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_err()
                    .contains(expected_error),
                "{name}"
            );
        }
    }

    #[test]
    fn indirect_call_memory_fixtures_admit_live_source_target_and_bounded_stack() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in [
            "s2_17e_call_mem_base",
            "s2_17e_call_mem_rex_base",
            "s2_17e_call_mem_sib",
            "s2_17e_call_mem_rip_relative",
            "s2_17e_call_mem_addr32",
            "s2_17e_call_mem_budget",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let mut retired = 0;
            while loaded.fixture.stop_rip.map(HexU64::get) != Some(actual.rip)
                && retired < loaded.fixture.instruction_budget
            {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} at {:#x}: {error}", actual.rip));
                assert!(matches!(
                    od_isa::execute::step(&mut actual, &mut decoder, AccessContext::default()),
                    Ok(od_isa::execute::StepOutcome::Advanced
                        | od_isa::execute::StepOutcome::Branched)
                ));
                retired += 1;
            }
            assert_eq!(retired, loaded.fixture.instruction_budget.min(3), "{name}");
        }

        for (name, expected_error) in [
            ("s2_17e_call_mem_read_fault", "source"),
            ("s2_17e_call_mem_noncanonical", "noncanonical"),
            ("s2_17e_call_mem_stack_fault", "stack access"),
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_err()
                    .contains(expected_error),
                "{name}"
            );
        }
    }

    #[test]
    fn indirect_memory_jmp_fixtures_admit_exact_read_and_selected_target() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_16e_jmp_mem_")
                || name.contains("fault")
                || name.contains("noncanonical")
                || !name.ends_with(".case.json")
            {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should admit source and target: {error}"));
            let instruction =
                long_decode::decode(&mut decoder, actual.rip, AccessContext::default()).unwrap();
            let Operation::IndirectJump {
                operand: Operand::Memory(memory),
            } = instruction.operation
            else {
                panic!("{name} should use a memory source")
            };
            let address = memory
                .access_address_48(
                    &actual,
                    actual.rip.wrapping_add(u64::from(instruction.length)),
                )
                .unwrap();
            let mut bytes = [0; 8];
            decoder
                .read(address, AccessContext::default(), &mut bytes)
                .unwrap();
            actual.rip = u64::from_le_bytes(bytes);
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} target decode: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 5);
    }

    #[test]
    fn setcc_literal_fixtures_are_admitted_with_no_flag_effects() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_16b_set") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            assert_eq!(effects, FlagEffects::default(), "{name}");
            admitted += 1;
        }
        assert_eq!(admitted, 79);
    }

    #[test]
    fn cmovcc_literal_fixtures_are_admitted_with_no_flag_effects() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_16c_cmov") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            assert_eq!(effects, FlagEffects::default(), "{name}");
            admitted += 1;
        }
        assert_eq!(admitted, 70);
    }

    #[test]
    fn push_pop_literal_fixtures_are_admitted_only_within_bounded_stack_memory() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17a_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 8);

        let raw = std::fs::read(directory.join("s2_17a_push_rax.case.json")).unwrap();
        let mut fixture = Fixture::from_json(&raw).unwrap().fixture;
        fixture.initial_state.gpr[od_isa::Register::Rsp as usize] = HexU64(0x300000);
        let mut decoder = FlatMemory::from_fixture(&fixture).unwrap();
        let actual = fixture.initial_state.to_cpu().unwrap();
        assert!(
            admission(&fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("stack access outside fixture range")
        );
    }

    #[test]
    fn bounded_fist_literal_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_success.case.json"
            )
            .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31o_fild_m16int_success.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31o_fild_m16int_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31p_fild_m64int_success.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31p_fild_m64int_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31g_fnstcw_m16int_rc0.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31g_fnstcw_m16int_rc1.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31g_fnstcw_m16int_rc2.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31g_fnstcw_m16int_rc3.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31g_fnstcw_m16int_arbitrary_state.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31g_fnstcw_m16int_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31g_fnstcw_m16int_noncanonical_crossing.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31h_fnstsw_m16_status_0000.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31h_fnstsw_m16_mixed_state.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31h_fnstsw_m16_status_ffff_fnstsw_ax.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31h_fnstsw_m16_noncanonical_crossing.case.json")
                .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31bt_fist_m32int_negative_zero_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31bt_fistp_m32int_positive_zero_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31d_fistp_m16int_rc0_positive.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31d_fistp_m16int_fnstsw_tie.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31bu_fistp_m16int_negative_zero_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31bv_fistp_m64int_negative_zero_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31by_fistp_m64int_negative_fraction_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31bw_fistp_m32int_negative_fraction_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31bz_fistp_m32int_negative_infinity_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31cc_fistp_m32int_negative_qnan_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31cf_fistp_m32int_negative_snan_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31ca_fistp_m16int_negative_infinity_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31cd_fistp_m16int_negative_qnan_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31cg_fistp_m16int_negative_snan_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31cb_fistp_m64int_negative_infinity_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31ce_fistp_m64int_negative_qnan_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31ch_fistp_m64int_negative_snan_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31bx_fistp_m16int_negative_fraction_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31e_fist_m16int_rc0_positive.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31e_fist_m16int_fnstsw_tie.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31l_fstp_m64fp.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31l_fstp_m64fp_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31q_fstp_m80fp.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31q_fstp_m80fp_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31an_fstp_m80fp_zero.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31an_fstp_m80fp_negative_zero_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31ao_fstp_m80fp_infinity.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31ao_fstp_m80fp_negative_infinity_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ar_fstp_m80fp_qnan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ar_fstp_m80fp_negative_indefinite_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31as_fstp_m80fp_snan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31as_fstp_m80fp_negative_snan_payload_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31at_fstp_m80fp_subnormal.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31at_fstp_m80fp_negative_subnormal_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ba_fstp_m80fp_sticky_status.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ba_fstp_m80fp_negative_snan_sticky_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31s_fld_m32fp.case.json")
                .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31s_fld_m32fp_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m32fp_invalid_status.case.json"
            )
            .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31af_fld_m32fp_infinity.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31af_fld_m32fp_negative_infinity_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ah_fld_m32fp_qnan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ah_fld_m32fp_negative_qnan_payload_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aj_fld_m32fp_snan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aj_fld_m32fp_negative_snan_payload_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31t_fld_m64fp.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31t_fld_m64fp_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ak_fld_m64fp_invalid_status.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ae_fld_m64fp_infinity.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ae_fld_m64fp_negative_infinity_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ag_fld_m64fp_qnan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ag_fld_m64fp_negative_qnan_payload_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ai_fld_m64fp_snan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ai_fld_m64fp_negative_snan_payload_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31m_fst_m64fp.case.json")
                .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_31m_fst_m64fp_fnstsw.case.json"
            )
            .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fst_m32fp_positive_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fst_m32fp_negative_zero_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fstp_m32fp_positive_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fstp_m32fp_negative_zero_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fst_m64fp_positive_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fst_m64fp_negative_zero_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fstp_m64fp_positive_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fstp_m64fp_negative_zero_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ap_fst_m64fp_positive_infinity.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ap_fstp_m64fp_negative_infinity_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31au_fst_m64fp_positive_quiet_nan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31au_fstp_m64fp_negative_quiet_nan_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aw_fst_m64fp_positive_signaling_nan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aw_fstp_m64fp_negative_signaling_nan_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aq_fst_m32fp_positive_infinity.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aq_fstp_m32fp_negative_infinity_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31av_fst_m32fp_positive_qnan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31av_fstp_m32fp_negative_qnan_payload_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ax_fst_m32fp_positive_snan.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ax_fstp_m32fp_negative_snan_payload_fnstsw.case.json")
                .as_slice(),
        ] {
            let fixture = Fixture::from_json(bytes).unwrap();
            let actual = fixture.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&fixture.fixture).unwrap();
            assert!(
                admission(&fixture.fixture, &mut decoder, &actual).is_err(),
                "bounded x87 forms must remain outside native admission"
            );
        }
    }

    #[test]
    fn push_pop16_fixtures_admit_word_ranges_and_reject_fault_cases() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17g_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            if name.contains("fault") || name.contains("truncated") {
                assert!(
                    admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                    "{name} must be outside positive native admission"
                );
            } else {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
                admitted += 1;
            }
        }
        assert_eq!(admitted, 7);
    }

    #[test]
    fn push_immediate_literal_fixtures_are_admitted_only_within_bounded_stack_memory() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17b_push_imm") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 8);
    }

    #[test]
    fn push_immediate16_fixtures_admit_two_byte_stack_ranges_and_reject_fault_cases() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17h_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            if name.contains("fault") || name.contains("truncated") {
                assert!(
                    admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                    "{name} must be outside positive native admission"
                );
            } else {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
                admitted += 1;
            }
        }
        assert_eq!(admitted, 8);
    }

    #[test]
    fn push_memory_fixtures_read_qword_sources_and_bound_stack_writes() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17i_push_mem_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            if name.contains("fault") {
                assert!(
                    admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                    "{name} must be outside positive native admission"
                );
            } else {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
                admitted += 1;
            }
        }
        assert_eq!(admitted, 7);

        let raw = std::fs::read(directory.join("s2_17i_push_mem_base.case.json")).unwrap();
        let mut fixture = Fixture::from_json(&raw).unwrap().fixture;
        let mut actual = fixture.initial_state.to_cpu().unwrap();
        actual.gpr[od_isa::Register::Rax as usize] = 0x0021_0000;
        let mut decoder = FlatMemory::from_fixture(&fixture).unwrap();
        assert!(
            admission(&fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("source outside readable fixture memory")
        );

        fixture.initial_state.gpr[od_isa::Register::Rax as usize] = HexU64(0x0020_0000);
        fixture.initial_state.gpr[od_isa::Register::Rsp as usize] = HexU64(0x0030_0000);
        let actual = fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&fixture).unwrap();
        assert!(
            admission(&fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("stack access outside fixture range")
        );
    }

    #[test]
    fn word_push_memory_fixtures_admit_exact_source_and_stack_ranges() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17k_push_mem_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            if name.contains("fault") {
                assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
            } else {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
                admitted += 1;
            }
        }
        assert_eq!(admitted, 6);

        let raw = std::fs::read(directory.join("s2_17k_push_mem_base.case.json")).unwrap();
        let fixture = Fixture::from_json(&raw).unwrap().fixture;
        let mut actual = fixture.initial_state.to_cpu().unwrap();
        actual.gpr[od_isa::Register::Rax as usize] = 0x0021_0000;
        let mut decoder = FlatMemory::from_fixture(&fixture).unwrap();
        assert!(
            admission(&fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("source outside readable fixture memory")
        );

        actual.gpr[od_isa::Register::Rax as usize] = 0x0020_0000;
        actual.gpr[od_isa::Register::Rsp as usize] = 0x0030_0001;
        let mut decoder = FlatMemory::from_fixture(&fixture).unwrap();
        assert!(
            admission(&fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("stack access outside fixture range")
        );
    }

    #[test]
    fn pop_memory_fixtures_read_live_stack_and_admit_postincrement_destinations() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17j_pop_mem_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            if name.contains("store_pf") {
                assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
            } else {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
                admitted += 1;
            }
        }
        assert_eq!(admitted, 6);

        let mut fixture = Fixture::from_json(
            &std::fs::read(directory.join("s2_17j_pop_mem_rsp_post_increment.case.json")).unwrap(),
        )
        .unwrap()
        .fixture;
        fixture.initial_state.gpr[od_isa::Register::Rsp as usize] = HexU64(0x0030_00f8);
        let actual = fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&fixture).unwrap();
        assert!(
            admission(&fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("destination outside writable fixture memory")
        );
    }

    #[test]
    fn word_pop_memory_fixtures_admit_exact_stack_and_postincrement_store_ranges() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17l_pop_mem_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            if name.contains("store_pf") {
                assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
            } else {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
                admitted += 1;
            }
        }
        assert_eq!(admitted, 6);

        let mut fixture = Fixture::from_json(
            &std::fs::read(directory.join("s2_17l_pop_mem_rsp_post_increment.case.json")).unwrap(),
        )
        .unwrap()
        .fixture;
        fixture.initial_state.gpr[od_isa::Register::Rsp as usize] = HexU64(0x0030_00fe);
        let actual = fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&fixture).unwrap();
        assert!(
            admission(&fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("destination outside writable fixture memory")
        );
    }

    #[test]
    fn rexw_overrides_operand_override_for_memory_stack_admission() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for name in [
            "s2_17m_push_mem_base",
            "s2_17m_push_mem_rsp_predecrement",
            "s2_17m_pop_mem_base",
            "s2_17m_pop_mem_rsp_postincrement",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 4);

        for code in [
            &[0x66, 0x40, 0x48, 0xff, 0x33][..], // Last REX.W selects qword PUSH.
            &[0x66, 0x48, 0x40, 0xff, 0x33],     // Last REX clears W: word PUSH.
            &[0x66, 0x40, 0x48, 0x8f, 0x03],     // Last REX.W selects qword POP.
            &[0x66, 0x48, 0x40, 0x8f, 0x03],     // Last REX clears W: word POP.
        ] {
            let loaded = fixture(code);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{code:02x?} should be admissible: {error}"));
        }

        for code in [
            &[0xf0, 0x66, 0x48, 0xff, 0x33][..], // LOCK is not in the profile.
            &[0x64, 0x66, 0x48, 0xff, 0x33],     // FS addressing is not in the profile.
            &[0xf0, 0x66, 0x48, 0x8f, 0x03],
            &[0x65, 0x66, 0x48, 0x8f, 0x03], // GS addressing is not in the profile.
        ] {
            let loaded = fixture(code);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn mixed_prefix_register_stack_fixtures_admit_bounded_word_and_qword_ranges() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17n_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 6);
    }

    #[test]
    fn rep_cmps_wdq_fixtures_remain_outside_native_admission() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut checked = 0;
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_19h_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            if name.contains("_cmp_rax_rax_") || name.contains("_test_rax_") {
                // The producer instruction may be natively admitted. Probe the
                // REP CMPS instruction itself at the following three-byte RIP.
                actual.rip = actual.rip.wrapping_add(3);
            }
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "{name} must remain outside native admission"
            );
            checked += 1;
        }
        assert_eq!(checked, 71);
    }

    #[test]
    fn cmps_addr32_four_widths_are_rejected_by_pre_tracee_admission() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for fixture_name in [
            "s2_18n_pos_b_df0.case.json",
            "s2_18n_pos_w_df0.case.json",
            "s2_18n_pos_d_df0.case.json",
            "s2_18n_pos_q_df0.case.json",
        ] {
            let loaded =
                Fixture::from_json(&std::fs::read(directory.join(fixture_name)).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert_eq!(
                admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
                "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist",
                "{fixture_name} must be rejected by the native allowlist before tracee execution"
            );
        }
    }

    #[test]
    fn movs_addr32_four_widths_are_rejected_by_pre_tracee_admission() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for fixture_name in [
            "s2_18o_pos_b_df0.case.json",
            "s2_18o_pos_w_df0.case.json",
            "s2_18o_pos_d_df0.case.json",
            "s2_18o_pos_q_df0.case.json",
        ] {
            let loaded =
                Fixture::from_json(&std::fs::read(directory.join(fixture_name)).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert_eq!(
                admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
                "native MOVS admission requires a supported MOVSB, MOVSW, MOVSD, or MOVSQ form",
                "{fixture_name} must be rejected by the native allowlist before tracee execution"
            );
        }
    }

    #[test]
    fn rep_movs_addr32_fixtures_are_rejected_before_native_tracee_execution() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut checked = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_19l_rep_movs_addr32_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert_eq!(
                admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
                "REP MOVS is outside the native execution profile",
                "{name}"
            );
            checked += 1;
        }
        assert_eq!(checked, 12);
    }

    #[test]
    fn popfq_fixtures_admit_only_readable_stack_and_profile_flag_inputs() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17o_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 2);

        let mut unreadable = fixture(&[0x9d]);
        unreadable.fixture.memory[2].permissions.read = false;
        let actual = unreadable.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&unreadable.fixture).unwrap();
        assert!(admission(&unreadable.fixture, &mut decoder, &actual).is_err());

        for out_of_profile in [1u64 << 8, 1u64 << 18] {
            let mut loaded = fixture(&[0x9d]);
            let mut bytes = vec![0u8; 8192];
            bytes[8184..].copy_from_slice(&out_of_profile.to_le_bytes());
            loaded.fixture.memory[2].initial_bytes_hex = hex(&bytes);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn popfw_fixtures_admit_only_supported_word_inputs() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_17r_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 1);

        for unsupported in [1u16 << 8, 1u16 << 14] {
            let mut loaded = fixture(&[0x66, 0x9d]);
            let mut bytes = vec![0u8; 8192];
            bytes[8184..8186].copy_from_slice(&unsupported.to_le_bytes());
            loaded.fixture.memory[2].initial_bytes_hex = hex(&bytes);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }

        let loaded = fixture(&[0x66, 0x9d]);
        let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
        actual.rflags = 2 | (1 << 14);
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());

        let loaded = fixture(&[0x66, 0x9d]);
        let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
        actual.rflags = 2 | (3 << 12);
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let error = admission(&loaded.fixture, &mut decoder, &actual).unwrap_err();
        assert!(error.contains("current IOPL"));
    }

    #[test]
    fn pushf_fixtures_admit_only_a_bounded_writable_stack_slot() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !(name.starts_with("s2_17p_")
                || name.starts_with("s2_17q_")
                || name.starts_with("s2_17s_"))
                || !name.ends_with(".case.json")
            {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(&path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admissible: {error}"));
            admitted += 1;
        }
        assert_eq!(admitted, 3);

        // Admission must use the decoded word width: exactly the two bytes
        // below RSP are mapped, so a qword-width assumption would reject it.
        let mut word_slot = fixture(&[0x66, 0x9c]);
        let stack = word_slot
            .fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        stack.guest_address = HexU64(0x301ff6);
        stack.length = 2;
        stack.initial_bytes_hex = "0000".into();
        stack.comparison_mask_hex = "ffff".into();
        word_slot
            .fixture
            .memory
            .retain(|region| region.region_id != "data");
        let actual = word_slot.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&word_slot.fixture).unwrap();
        admission(&word_slot.fixture, &mut decoder, &actual)
            .expect("PUSHFW should admit an exact two-byte writable stack slot");

        let mut unwritable = fixture(&[0x9c]);
        unwritable.fixture.memory[2].permissions.write = false;
        let actual = unwritable.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&unwritable.fixture).unwrap();
        assert!(admission(&unwritable.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn call_ret_fixtures_admit_nested_targets_and_bounded_stack_reads_writes() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in [
            "s2_17c_call_forward",
            "s2_17c_call_nested",
            "s2_17c_call_negative_rel32",
            "s2_17c_call_budget",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let mut retired = 0;
            while loaded.fixture.stop_rip.map(HexU64::get) != Some(actual.rip)
                && retired < loaded.fixture.instruction_budget
            {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} at {:#x}: {error}", actual.rip));
                assert!(matches!(
                    od_isa::execute::step(&mut actual, &mut decoder, AccessContext::default()),
                    Ok(od_isa::execute::StepOutcome::Branched)
                ));
                retired += 1;
            }
            assert_eq!(retired, loaded.fixture.instruction_budget, "{name}");
        }

        for (name, rsp, error) in [
            ("s2_17c_call_forward", Some(0x300000), "stack access"),
            ("s2_17c_ret_noncanonical", Some(0x300100), "stack access"),
            ("s2_17c_call_noncanonical", None, "noncanonical"),
            ("s2_17c_ret_noncanonical", None, "noncanonical"),
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            if let Some(rsp) = rsp {
                actual.gpr[od_isa::Register::Rsp as usize] = rsp;
            }
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_err()
                    .contains(error),
                "{name}"
            );
        }
    }

    #[test]
    fn ret_imm16_fixtures_admit_stack_targets_and_cleanup_paths() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for (name, expected_retired) in [
            ("s2_17f_ret_imm0", 1),
            ("s2_17f_ret_imm_small", 1),
            ("s2_17f_ret_imm_max", 1),
            ("s2_17f_ret_imm_nested_cleanup", 3),
            ("s2_17f_ret_imm_budget", 1),
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let mut retired = 0;
            while loaded.fixture.stop_rip.map(HexU64::get) != Some(actual.rip)
                && retired < loaded.fixture.instruction_budget
            {
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_or_else(|error| panic!("{name} at {:#x}: {error}", actual.rip));
                assert!(matches!(
                    od_isa::execute::step(&mut actual, &mut decoder, AccessContext::default()),
                    Ok(od_isa::execute::StepOutcome::Advanced
                        | od_isa::execute::StepOutcome::Branched)
                ));
                retired += 1;
            }
            assert_eq!(retired, expected_retired, "{name}");
        }

        for (name, expected_error) in [
            ("s2_17f_ret_imm_noncanonical", "noncanonical"),
            ("s2_17f_ret_imm_stack_fault", "stack access"),
            ("s2_17f_ret_imm_read_fault", "stack access"),
            ("s2_17f_ret_imm_truncated", "native instruction outside"),
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_err()
                    .contains(expected_error),
                "{name}"
            );
        }
    }

    #[test]
    fn ret66_fixture_admits_only_bounded_qword_return_target() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for (name, expected_rip) in [
            ("s2_17t_ret66", 0x0010_0002),
            ("s2_17t_ret66_ignored_rex", 0x0010_0003),
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert!(matches!(
                od_isa::execute::step(&mut actual, &mut decoder, AccessContext::default()),
                Ok(od_isa::execute::StepOutcome::Branched)
            ));
            assert_eq!(actual.rip, expected_rip, "{name}");
            assert_eq!(
                actual.gpr[od_isa::Register::Rsp as usize],
                0x0030_0088,
                "{name}"
            );
        }
    }

    #[test]
    fn page_plan_groups_adjacent_logical_regions_and_rejects_incompatible_permissions() {
        let mut loaded = fixture(&[0x48, 0x8b, 0x03]);
        loaded.fixture.memory.retain(|r| r.region_id != "data");
        loaded
            .fixture
            .memory
            .push(region("left", 0x200ffe, &[1, 2], true, false));
        loaded
            .fixture
            .memory
            .push(region("right", 0x201000, &[3, 4, 5, 6, 7, 8], true, false));
        let plan = MappingPlan::new(&loaded.fixture, 4096).unwrap();
        assert_eq!(plan.pages[&0x200000], libc::PROT_READ | libc::PROT_WRITE);
        assert_eq!(plan.pages[&0x201000], libc::PROT_READ | libc::PROT_WRITE);
        assert!(admits_range(&loaded.fixture, 0x200ffe, 8, AccessKind::Read));
        assert!(!admits_range(
            &loaded.fixture,
            0x200ffd,
            8,
            AccessKind::Read
        ));
        loaded
            .fixture
            .memory
            .push(region("different", 0x200000, &[9], false, false));
        assert!(
            MappingPlan::new(&loaded.fixture, 4096)
                .unwrap_err()
                .contains("identical permissions")
        );
    }

    #[test]
    fn page_plan_and_admission_reject_host_escape_and_writable_code() {
        let mut loaded = fixture(&[0x48, 0x8b, 0x03]);
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
        actual.gpr[3] = 0x7fff_1234_0000;
        assert!(
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_err()
                .contains("outside fixture")
        );
        loaded.fixture.memory[0].permissions.write = true;
        assert!(MappingPlan::new(&loaded.fixture, 4096).is_err());
        loaded.fixture.memory[0].permissions.write = false;
        loaded.fixture.memory[0].guest_address = HexU64(0x4000);
        assert!(MappingPlan::new(&loaded.fixture, 4096).is_err());
    }

    #[test]
    fn admission_rejects_partial_ranges_readonly_stores_syscalls_and_truncated_code() {
        let mut loaded = fixture(&[0x48, 0x8b, 0x03]);
        let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
        actual.gpr[3] = 0x20000c;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        loaded.fixture.memory[0] = region("code", 0x100000, &[0x48, 0x89, 0x03], false, true);
        loaded.fixture.memory[1].permissions.write = false;
        actual.gpr[3] = 0x200000;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        for code in [
            &[0x0f, 0x05][..],
            &[0x48, 0x8b][..],
            &[0x64, 0x48, 0x8b, 0x03][..],
        ] {
            let loaded = fixture(code);
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn data_transfer_admission_checks_only_real_memory_accesses() {
        for code in [
            &[0x48, 0xc7, 0x03, 1, 0, 0, 0][..], // MOV qword [RBX],imm32
            &[0x4c, 0x0f, 0xbe, 0x03][..],       // MOVSX R8,byte [RBX]
            &[0x4c, 0x63, 0x03][..],             // MOVSXD R8,dword [RBX]
            &[0x67, 0xa1, 0, 0, 0x20, 0][..],    // MOV EAX,moffs32
        ] {
            let mut loaded = fixture(code);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            // Removing data proves admission is checking the logical range,
            // not just the host page mapping or the nominal MOV family.
            loaded.fixture.memory.retain(|r| r.region_id != "data");
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
        for code in [
            &[0x4c, 0x8d, 0x03][..], // LEA R8,[RBX]
            &[0x86, 0xc4][..],       // XCHG AL,AH
            &[0x41, 0x90][..],       // XCHG EAX,R8D
            &[0x48, 0x90][..],       // NOP alias
        ] {
            let loaded = fixture(code);
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            actual.gpr[3] = 0x0000_8000_0000_0000;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual).unwrap();
        }
        for code in [
            &[0x66, 0x98][..],
            &[0x98][..],
            &[0x48, 0x98][..],
            &[0x66, 0x99][..],
            &[0x99][..],
            &[0x48, 0x99][..],
        ] {
            let mut loaded = fixture(code);
            loaded.fixture.memory.retain(|r| r.region_id != "data");
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects, FlagEffects::default());
        }
        let mut loaded = fixture(&[0x48, 0xc7, 0x03, 1, 0, 0, 0]);
        loaded.fixture.memory[1].permissions.write = false;
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        // Memory XCHG is outside this admission profile until atomic semantics.
        let loaded = fixture(&[0x48, 0x87, 0x03]);
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn arithmetic_admission_requires_read_and_write_except_for_cmp() {
        for code in [
            &[0x48, 0x01, 0x0b][..], // ADD [RBX],RCX
            &[0x48, 0x11, 0x0b][..], // ADC [RBX],RCX
            &[0x48, 0x29, 0x0b][..], // SUB [RBX],RCX
            &[0x48, 0x19, 0x0b][..], // SBB [RBX],RCX
            &[0x48, 0xff, 0x03][..], // INC [RBX]
            &[0x48, 0xff, 0x0b][..], // DEC [RBX]
            &[0x48, 0xf7, 0x1b][..], // NEG [RBX]
        ] {
            let mut loaded = fixture(code);
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            actual.gpr[3] = 0x20000c;
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
            actual.gpr[3] = 0x200000;
            loaded.fixture.memory[1].permissions.write = false;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
        for code in [
            &[0x48, 0x39, 0x0b][..],    // CMP [RBX],RCX
            &[0x48, 0x03, 0x03][..],    // ADD RAX,[RBX]
            &[0x48, 0x83, 0x3b, 1][..], // CMP qword [RBX],imm8
        ] {
            let mut loaded = fixture(code);
            loaded.fixture.memory[1].permissions.write = false;
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            loaded.fixture.memory.retain(|r| r.region_id != "data");
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
        for code in [
            &[0xf0, 0x48, 0x01, 0x0b][..],
            &[0xf3, 0x48, 0x01, 0xc8][..],
            &[0x64, 0x48, 0x01, 0x0b][..],
            &[0x65, 0x48, 0x03, 0x03][..],
        ] {
            let loaded = fixture(code);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn logic_admission_keeps_test_readonly_and_returns_semantic_flag_effects() {
        for code in [
            &[0x48, 0x21, 0x0b][..],
            &[0x48, 0x09, 0x0b],
            &[0x48, 0x31, 0x0b],
            &[0x48, 0xf7, 0x13],
        ] {
            let mut loaded = fixture(code);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            loaded.fixture.memory[1].permissions.write = false;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
        for code in [&[0x48, 0x85, 0x0b][..], &[0x48, 0xf7, 0x03, 1, 0, 0, 0]] {
            let mut loaded = fixture(code);
            loaded.fixture.memory[1].permissions.write = false;
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.apply(od_isa::INTEGER_FLAGS), 0xcc7);
            loaded.fixture.memory.retain(|r| r.region_id != "data");
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn multiply_divide_admission_bounds_only_explicit_read_operands() {
        const MEMORY_CASES: &[(&[u8], u64)] = &[
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_14_mul_64_mem.case.json"),
                0xc03,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_14_div_64_mem.case.json"),
                0x402,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_14_imul_64_mem.case.json"),
                0xc03,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_14_imul_imm8_64_mem.case.json"
                ),
                0xc03,
            ),
        ];
        for (raw, defined) in MEMORY_CASES {
            let mut loaded = Fixture::from_json(raw).unwrap();
            assert!(!loaded.fixture.memory[1].permissions.write);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.apply(od_isa::INTEGER_FLAGS), *defined);

            loaded
                .fixture
                .memory
                .retain(|region| region.region_id != "data");
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual)
                    .unwrap_err()
                    .contains("outside fixture range")
            );
        }

        const REGISTER_CASES: &[(&[u8], u64)] = &[
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_14_mul_64.case.json"),
                0xc03,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_14_imul_2op_64_fits.case.json"
                ),
                0xc03,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_14_imul_imm_full_64.case.json"
                ),
                0xc03,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_14_imul_imm_negative_64.case.json"
                ),
                0xc03,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_14_div_64.case.json"),
                0x402,
            ),
        ];
        for (raw, defined) in REGISTER_CASES {
            let loaded = Fixture::from_json(raw).unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            // Register forms do not dereference an unrelated guest pointer.
            actual.gpr[3] = 0x0000_8000_0000_0000;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.apply(od_isa::INTEGER_FLAGS), *defined);
        }
    }

    #[test]
    fn bswap_register_fixtures_are_admitted_without_data_memory_or_flag_effects() {
        for raw in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bswap_eax.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bswap_r8d.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bswap_r9.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bswap_66_rexw_r9.case.json")
                .as_slice(),
        ] {
            let loaded = Fixture::from_json(raw).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert_eq!(
                admission(&loaded.fixture, &mut decoder, &actual).unwrap(),
                FlagEffects::default()
            );
        }
    }

    #[test]
    fn bt_family_register_fixtures_are_admitted_with_semantic_flag_definedness() {
        for raw in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bt_16.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bt_32.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bt_64.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bts_16.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bts_32.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bts_64.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_btr_16.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_btr_32.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_btr_64.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_btc_16.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_btc_32.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_btc_64.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(raw).unwrap();
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            // Register operands must not map or dereference unrelated guest data.
            actual.gpr[3] = 0x0000_8000_0000_0000;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.defined, 1);
            assert_eq!(effects.undefined, 0x894);
            assert_eq!(effects.apply(od_isa::INTEGER_FLAGS), 0x443);
        }
    }

    #[test]
    fn bt_family_immediate_register_forms_are_admitted_without_data_accesses() {
        for code in [
            &[0x66, 0x41, 0x0f, 0xba, 0xe1, 0x00][..], // BT R9W,0
            &[0x41, 0x0f, 0xba, 0xe9, 0xff],           // BTS R9D,255
            &[0x49, 0x0f, 0xba, 0xf1, 0x40],           // BTR R9,64
            &[0x66, 0x41, 0x0f, 0xba, 0xf9, 0xff],     // BTC R9W,255
        ] {
            let loaded = fixture(code);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.defined, 1);
            assert_eq!(effects.undefined, 0x894);
            assert_eq!(effects.apply(od_isa::INTEGER_FLAGS), 0x443);
        }

        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_15_bti_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.defined, 1, "{name}");
            assert_eq!(effects.undefined, 0x894, "{name}");
            assert_eq!(effects.apply(od_isa::INTEGER_FLAGS), 0x443, "{name}");
            admitted += 1;
        }
        assert_eq!(admitted, 48);
    }

    #[test]
    fn bt_family_immediate_memory_fixtures_are_admitted_with_exact_permissions() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_15_btim_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.defined, 0x001, "{name}");
            assert_eq!(effects.undefined, 0x894, "{name}");
            admitted += 1;
        }
        assert_eq!(admitted, 48);

        let raw = std::fs::read(directory.join("s2_15_btim_bt_32_imm_255.case.json")).unwrap();
        let mut loaded = Fixture::from_json(&raw).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let stack = loaded
            .fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        stack.permissions.write = false;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_ok());

        let raw = std::fs::read(directory.join("s2_15_btim_bts_32_imm_255.case.json")).unwrap();
        let mut loaded = Fixture::from_json(&raw).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let stack = loaded
            .fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        stack.permissions.write = false;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn bt_family_register_memory_fixtures_are_admitted_with_exact_permissions() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut admitted = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("s2_15_btrm_") || !name.ends_with(".case.json") {
                continue;
            }
            let loaded = Fixture::from_json(&std::fs::read(path).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.defined, 1, "{name}");
            assert_eq!(effects.undefined, 0x894, "{name}");
            admitted += 1;
        }
        assert_eq!(admitted, 72);

        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let mut loaded = Fixture::from_json(
            &std::fs::read(directory.join("s2_15_btrm_bt_32_off_20.case.json")).unwrap(),
        )
        .unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let stack = loaded
            .fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        stack.permissions.write = false;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_ok());

        let mut loaded = Fixture::from_json(
            &std::fs::read(directory.join("s2_15_btrm_bts_32_off_20.case.json")).unwrap(),
        )
        .unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let stack = loaded
            .fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        stack.permissions.write = false;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());

        // Register offset 32 moves a dword access four bytes forward. Native
        // admission must validate that computed element, not the encoded base.
        let mut loaded = Fixture::from_json(
            &std::fs::read(directory.join("s2_15_btrm_bt_32_off_20.case.json")).unwrap(),
        )
        .unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let stack = loaded
            .fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        stack.guest_address = HexU64(0x300024);
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_ok());
        let stack = loaded
            .fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        stack.guest_address = HexU64(0x300020);
        stack.length = 4;
        stack.initial_bytes_hex.truncate(8);
        stack.comparison_mask_hex.truncate(8);
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn bsf_bsr_register_fixtures_are_admitted_with_defined_scan_flags() {
        for raw in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_16_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_16_even.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_16_high.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_32_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_32_rex_rb.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_32_alias.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_64_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_64_low.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_64_high.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_16_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_16_low.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_16_high.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_32_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_32_rex_rb.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_32_low.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_64_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_64_high_alias.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_mem_16_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_mem_32_even.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsf_mem_64_high.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_mem_32_zero.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_mem_16_high.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_15_bsr_mem_64_high.case.json")
                .as_slice(),
        ] {
            let loaded = Fixture::from_json(raw).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
            assert_eq!(effects.defined, 0x8d5);
            assert_eq!(effects.undefined, 0);
            assert_eq!(effects.apply(od_isa::INTEGER_FLAGS), od_isa::INTEGER_FLAGS);
        }
    }

    #[test]
    fn bit_scan_memory_admission_requires_readable_mapped_source() {
        let mut loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_15_bsf_mem_32_even.case.json"
        ))
        .unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let stack = loaded
            .fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        stack.permissions.read = false;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());

        loaded
            .fixture
            .memory
            .retain(|region| region.region_id != "stack");
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn shift_admission_uses_the_runtime_count_for_flag_definedness() {
        let mut loaded = fixture(&[0x48, 0xd3, 0x23]); // SHL qword [RBX],CL
        let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();

        actual.gpr[1] = 0;
        let effects = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
        assert_eq!(effects, FlagEffects::default());
        assert_eq!(effects.apply(od_isa::INTEGER_FLAGS), od_isa::INTEGER_FLAGS);

        actual.gpr[1] = 1;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let one = admission(&loaded.fixture, &mut decoder, &actual).unwrap();
        assert_eq!(one.defined, 0x8c5); // SF|ZF|PF|CF|OF
        assert_eq!(one.undefined, 0x10); // AF

        // REX.W uses a six-bit count mask: 64 is an effective zero and 65
        // reduces to the same one-bit flag contract as count one.
        actual.gpr[1] = 64;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap(),
            FlagEffects::default()
        );
        actual.gpr[1] = 65;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap(),
            one
        );

        // RCL byte count 9 reduces modulo the nine-bit carry ring to zero.
        // The destination and CF stay unchanged, while OF is undefined because
        // the masked raw count was not one.
        loaded.fixture.memory[0].initial_bytes_hex = "d2d0".into();
        loaded.fixture.memory[0].length = 2;
        loaded.fixture.memory[0].comparison_mask_hex = "ffff".into();
        loaded.fixture.stop_rip = Some(HexU64(0x100002));
        actual.gpr[1] = 9;
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let instruction =
            long_decode::decode(&mut decoder, actual.rip, AccessContext::default()).unwrap();
        let effects = instruction.flag_effects_for_count(9);
        assert_eq!(effects.defined, 0);
        assert_eq!(effects.undefined, 0x800); // OF
    }

    #[test]
    fn shift_admission_checks_memory_rmw_permissions_and_logical_ranges() {
        for code in [
            &[0x48, 0xc1, 0x23, 0][..],   // SHL qword [RBX],0 still touches memory
            &[0x48, 0xc1, 0x23, 1][..],   // SHL qword [RBX],1
            &[0x48, 0x0f, 0xa4, 0x0b, 1], // SHLD qword [RBX],RCX,1
        ] {
            let mut loaded = fixture(code);
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual).unwrap();

            loaded.fixture.memory[1].permissions.write = false;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());

            loaded
                .fixture
                .memory
                .retain(|region| region.region_id != "data");
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn physical_logic_tracks_undefined_af_until_a_retired_instruction_defines_it() {
        for (code, expected) in [
            (&[0x48, 0x31, 0xc0][..], 0xcc7),
            (
                &[0x48, 0x31, 0xc0, 0x48, 0x89, 0xc2, 0x48, 0xf7, 0xd2],
                0xcc7,
            ),
            (&[0x48, 0x31, 0xc0, 0x48, 0x83, 0xc0, 1], 0xcd7),
        ] {
            let mut loaded = fixture(code);
            loaded.fixture.rflags_mask = HexU64(expected);
            let result = execute(&loaded);
            assert_eq!(result.outcome, Outcome::StopAddress {}, "{result:?}");
            let Observation::Available { value: snapshot } = result.snapshot else {
                panic!("snapshot unavailable")
            };
            assert_eq!(snapshot.rflags_defined.get(), expected);
        }
        let mut loaded = fixture(&[0x48, 0x31, 0x0b]);
        loaded.fixture.initial_state.gpr[3] = HexU64(0x400000);
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        let Observation::Available { value: snapshot } = result.snapshot else {
            panic!("snapshot unavailable")
        };
        assert_eq!(snapshot.rflags_defined.get(), od_isa::INTEGER_FLAGS);
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
    }

    #[test]
    fn physical_initial_readback_preserves_all_sixteen_gprs_rsp_rip_and_profile_flags() {
        let mut loaded = fixture(&[0x48, 0x8b, 0x03]);
        loaded.fixture.stop_rip = Some(loaded.fixture.initial_state.rip);
        let result = execute(&loaded);
        assert_eq!(result.outcome, Outcome::StopAddress {}, "{result:?}");
        let Observation::Available { value: snapshot } = result.snapshot else {
            panic!("snapshot unavailable")
        };
        assert_eq!(snapshot.state, loaded.fixture.initial_state);
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
    }

    #[test]
    fn physical_single_steps_capture_extended_gpr_and_actual_memory_without_guest_stack_wrapper() {
        // MOV R15,R8; MOV [RBX],R8. These bytes have no host-pointer registers.
        let loaded = fixture(&[0x4d, 0x8b, 0xf8, 0x4c, 0x89, 0x03]);
        let result = execute(&loaded);
        assert_eq!(result.outcome, Outcome::StopAddress {}, "{result:?}");
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value: snapshot } = result.snapshot else {
            panic!("snapshot unavailable")
        };
        assert_eq!(snapshot.state.gpr[15], loaded.fixture.initial_state.gpr[8]);
        assert_eq!(snapshot.state.gpr[4], loaded.fixture.initial_state.gpr[4]);
        let data = snapshot
            .memory
            .iter()
            .find(|r| r.region_id == "data")
            .unwrap();
        assert_eq!(
            &data.bytes_hex[..16],
            hex(&loaded.fixture.initial_state.gpr[8].get().to_le_bytes())
        );
    }

    #[test]
    fn physical_branch_loop_stops_at_exact_verified_step_budget() {
        let mut loaded = fixture(&[0x74, 0xfe]);
        loaded.fixture.instruction_budget = 3;
        loaded.fixture.expected_exit = v2::ExpectedExit::InstructionBudgetExhausted;
        let result = execute(&loaded);
        assert_eq!(
            result.outcome,
            Outcome::InstructionBudgetExhausted {},
            "{result:?}"
        );
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(3) }
        );
        let Observation::Available { value: snapshot } = result.snapshot else {
            panic!("snapshot unavailable")
        };
        assert_eq!(snapshot.state.rip, loaded.fixture.initial_state.rip);
    }

    #[test]
    fn wait_deadline_and_cleanup_do_not_leave_a_running_child() {
        // SAFETY: child makes only pause/_exit calls, with no inherited Rust locks.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                libc::pause();
                libc::_exit(0)
            }
        }
        let owned = Tracee { pid, reaped: false };
        assert!(
            wait(pid, Instant::now() + Duration::from_millis(10))
                .unwrap_err()
                .contains("wall deadline")
        );
        drop(owned);
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn terminal_signal_preserves_wtermsig_without_fabricating_snapshot_or_count() {
        let loaded = fixture(&[0x48, 0x8b, 0x03]);
        // SAFETY: disposable child performs only kill/_exit after fork.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                libc::kill(libc::getpid(), libc::SIGKILL);
                libc::_exit(99)
            }
        }
        let mut owned = Tracee { pid, reaped: false };
        let status = owned.wait(Instant::now() + Duration::from_secs(2)).unwrap();
        assert!(owned.reaped);
        let result = terminal_signal_result(&loaded, status).expect("terminal signal result");
        assert!(
            matches!(result.outcome,Outcome::Signaled {signal,..} if signal==libc::SIGKILL as u32)
        );
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
        result.validate_for(&loaded).unwrap();
        drop(owned);
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert!(
            terminal_signal_result(&loaded, 5 << 8).is_none(),
            "ordinary exit remains a harness failure"
        );
    }

    #[test]
    fn owned_child_cleanup_kills_and_reaps_stopped_child() {
        // SAFETY: child does no Rust work and exits only through _exit/SIGKILL.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                libc::kill(libc::getpid(), libc::SIGSTOP);
                libc::_exit(0)
            }
        }
        let owned = Tracee { pid, reaped: false };
        let status = wait(pid, Instant::now() + Duration::from_secs(2)).unwrap();
        assert!(libc::WIFSTOPPED(status));
        drop(owned);
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn movsb_native_admission_accepts_bounded_successes_and_rejects_fault_only_cases() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in [
            "s2_18a_movsb_df0",
            "s2_18a_movsb_df1",
            "s2_18a_movsb_overlap",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be natively admissible: {error}"));
        }

        for name in ["s2_18a_movsb_read_fault", "s2_18a_movsb_write_fault"] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "{name} names an intentionally unmapped implicit access"
            );
        }
    }

    #[test]
    fn movsw_native_admission_accepts_mapped_cases_and_normalized_ignored_rex() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in [
            "s2_18b_movsw_df0",
            "s2_18b_movsw_df1",
            "s2_18b_movsw_overlap",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be natively admissible: {error}"));
        }

        let mut loaded = Fixture::from_json(
            &std::fs::read(directory.join("s2_18b_movsw_df0.case.json")).unwrap(),
        )
        .unwrap();
        loaded.fixture.memory[0].length = 3;
        loaded.fixture.memory[0].initial_bytes_hex = "4866a5".into();
        loaded.fixture.memory[0].comparison_mask_hex = "ffffff".into();
        loaded.fixture.stop_rip = Some(HexU64(0x0010_0003));
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        admission(&loaded.fixture, &mut decoder, &actual)
            .expect("a misplaced REX before 66 is ignored and normalizes to 66 A5");

        for name in ["s2_18b_movsw_read_fault", "s2_18b_movsw_write_fault"] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "{name} names an intentionally unmapped implicit access"
            );
        }
    }

    #[test]
    fn movsd_native_admission_accepts_only_mapped_bare_a5_cases() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in [
            "s2_18c_movsd_df0",
            "s2_18c_movsd_df1",
            "s2_18c_movsd_overlap",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be natively admissible: {error}"));
        }

        for name in ["s2_18c_movsd_read_fault", "s2_18c_movsd_write_fault"] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "{name} names an intentionally unmapped implicit access"
            );
        }

        for bytes in [
            "40a5", "67a5", "26a5", "2ea5", "36a5", "3ea5", "64a5", "65a5", "f2a5", "f3a5",
        ] {
            let mut loaded = Fixture::from_json(
                &std::fs::read(directory.join("s2_18c_movsd_df0.case.json")).unwrap(),
            )
            .unwrap();
            loaded.fixture.memory[0].length = (bytes.len() / 2) as u32;
            loaded.fixture.memory[0].initial_bytes_hex = bytes.into();
            loaded.fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
            loaded.fixture.stop_rip = Some(HexU64(0x0010_0000 + (bytes.len() / 2) as u64));
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "native MOVSD admission accepted prefixed bytes {bytes}"
            );
        }
    }

    #[test]
    fn movsq_native_admission_checks_qword_ranges_and_rexw_override() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in [
            "s2_18d_movsq_df0",
            "s2_18d_movsq_df1",
            "s2_18d_movsq_alias",
            "s2_18d_movsq_overlap",
            "s2_18d_movsq_rexw_overrides_66",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be natively admissible: {error}"));
        }

        for name in ["s2_18d_movsq_read_fault", "s2_18d_movsq_write_fault"] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "{name} names an intentionally unmapped implicit access"
            );
        }

        for bytes in ["40a5", "67 48a5", "2e 48a5", "f3 48a5"] {
            let mut loaded = Fixture::from_json(
                &std::fs::read(directory.join("s2_18d_movsq_df0.case.json")).unwrap(),
            )
            .unwrap();
            let compact = bytes.replace(' ', "");
            loaded.fixture.memory[0].length = (compact.len() / 2) as u32;
            loaded.fixture.memory[0].initial_bytes_hex = compact.clone();
            loaded.fixture.memory[0].comparison_mask_hex = "ff".repeat(compact.len() / 2);
            loaded.fixture.stop_rip = Some(HexU64(0x0010_0000 + (compact.len() / 2) as u64));
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "native MOVSQ admission accepted unsupported bytes {bytes}"
            );
        }
    }

    #[test]
    fn stosb_native_admission_accepts_mapped_cases_and_rejects_faults_and_prefixes() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in [
            "s2_18e_stosb_df0",
            "s2_18e_stosb_df1",
            "s2_18e_stosb_alias_canary",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be natively admissible: {error}"));
        }

        let fault = Fixture::from_json(
            &std::fs::read(directory.join("s2_18e_stosb_write_fault.case.json")).unwrap(),
        )
        .unwrap();
        let actual = fault.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&fault.fixture).unwrap();
        assert!(
            admission(&fault.fixture, &mut decoder, &actual).is_err(),
            "the destination fault fixture has no mapped writable byte"
        );

        for bytes in ["40aa", "66aa", "67aa", "2eaa", "64aa", "f2aa", "f3aa"] {
            let mut loaded = Fixture::from_json(
                &std::fs::read(directory.join("s2_18e_stosb_df0.case.json")).unwrap(),
            )
            .unwrap();
            loaded.fixture.memory[0].length = (bytes.len() / 2) as u32;
            loaded.fixture.memory[0].initial_bytes_hex = bytes.into();
            loaded.fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
            loaded.fixture.stop_rip = Some(HexU64(0x0010_0000 + (bytes.len() / 2) as u64));
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "native STOSB admission accepted prefixed bytes {bytes}"
            );
        }
    }

    #[test]
    fn rep_stosb_native_admission_accepts_small_mapped_ranges_only() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for name in [
            "s2_19a_rep_stosb_zero",
            "s2_19a_rep_stosb_one",
            "s2_19a_rep_stosb_df0",
            "s2_19a_rep_stosb_df1",
        ] {
            let loaded = Fixture::from_json(
                &std::fs::read(directory.join(format!("{name}.case.json"))).unwrap(),
            )
            .unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            admission(&loaded.fixture, &mut decoder, &actual)
                .unwrap_or_else(|error| panic!("{name} should be admitted: {error}"));
        }

        let fault = Fixture::from_json(
            &std::fs::read(directory.join("s2_19a_rep_stosb_fault_middle.case.json")).unwrap(),
        )
        .unwrap();
        let actual = fault.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&fault.fixture).unwrap();
        assert!(admission(&fault.fixture, &mut decoder, &actual).is_err());

        let mut large = Fixture::from_json(
            &std::fs::read(directory.join("s2_19a_rep_stosb_df0.case.json")).unwrap(),
        )
        .unwrap();
        large.fixture.initial_state.gpr[od_isa::Register::Rcx as usize] = HexU64(9);
        let actual = large.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&large.fixture).unwrap();
        assert!(admission(&large.fixture, &mut decoder, &actual).is_err());

        let mut prefixed = Fixture::from_json(
            &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
        )
        .unwrap();
        prefixed.fixture.memory[0].length = 3;
        prefixed.fixture.memory[0].initial_bytes_hex = "f366aa".into();
        prefixed.fixture.memory[0].comparison_mask_hex = "ffffff".into();
        let actual = prefixed.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&prefixed.fixture).unwrap();
        assert!(admission(&prefixed.fixture, &mut decoder, &actual).is_err());

        for bytes in [
            "f366ab",   // REP STOSW
            "f3ab",     // REP STOSD
            "f348ab",   // REP STOSQ
            "f36648ab", // REX.W takes precedence over 66
            "f3ac",     // REP LODSB
            "f366ad",   // REP LODSW
            "f3ad",     // REP LODSD
            "f348ad",   // REP LODSQ
            "f34866ad", // normalized REP LODSW
            "f36648ad", // normalized REP LODSQ
            "66f3ad",   // normalized REP LODSW
        ] {
            let mut unsupported = Fixture::from_json(
                &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
            )
            .unwrap();
            unsupported.fixture.memory[0].length = (bytes.len() / 2) as u32;
            unsupported.fixture.memory[0].initial_bytes_hex = bytes.into();
            unsupported.fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
            unsupported.fixture.stop_rip = Some(HexU64(0x0010_0000 + (bytes.len() / 2) as u64));
            let actual = unsupported.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&unsupported.fixture).unwrap();
            assert!(
                admission(&unsupported.fixture, &mut decoder, &actual).is_err(),
                "native REP STOS unexpectedly admitted {bytes}"
            );
        }
    }

    #[test]
    fn rep_scasb_is_explicitly_rejected_by_native_admission() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let loaded = Fixture::from_json(
            &std::fs::read(directory.join("s2_19e_rep_scasb_one_equal_df0.case.json")).unwrap(),
        )
        .unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "REP SCAS is outside the native execution profile"
        );
    }

    #[test]
    fn rep_scas_wdq_widths_and_prefix_orders_are_explicitly_rejected_by_native_admission() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for fixture_name in [
            "s2_19g_rep_scas_word_one_equal_df0.case.json",
            "s2_19g_rep_scas_dword_one_equal_df0.case.json",
            "s2_19g_rep_scas_qword_one_equal_df0.case.json",
            "s2_19g_rep_scas_normalized_word_rex_before_66.case.json",
            "s2_19g_rep_scas_normalized_qword_rex_after_66.case.json",
        ] {
            let loaded =
                Fixture::from_json(&std::fs::read(directory.join(fixture_name)).unwrap()).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert_eq!(
                admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
                "REP SCAS is outside the native execution profile",
                "{fixture_name}"
            );
        }
    }

    #[test]
    fn fs_base_profile_and_memory_mov_fail_closed_before_native_trace() {
        let code = [0x64, 0x48, 0x8b, 0x00];
        let mut input = fixture(&code).fixture;
        input.profile = v2::Profile::LongFsBaseV1;
        input.initial_state.fs_base = Some(HexU64(0xffff_8000_0000_0000));
        let loaded = Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let old_profile = fixture(&code);
        let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
        assert!(admission(&old_profile.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn gs_base_profile_fails_closed_before_native_trace() {
        let code = [0x65, 0x48, 0x8b, 0x00];
        let mut input = fixture(&code).fixture;
        input.profile = v2::Profile::LongGsBaseV1;
        input.initial_state.gs_base = Some(HexU64(0xffff_8000_0000_0000));
        let loaded = Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let old_profile = fixture(&code);
        let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
        assert!(admission(&old_profile.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn fs_selector_profile_fails_closed_and_legacy_decode_is_rejected() {
        let code = [0x48, 0x8c, 0xe0];
        let mut input = fixture(&code).fixture;
        input.profile = v2::Profile::LongFsSelectorV1;
        input.initial_state.fs_base = Some(HexU64(0xffff_8000_0000_1000));
        input.initial_state.fs_selector = Some(HexU64(0xf123));
        let loaded = Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let old_profile = fixture(&code);
        let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
        assert!(admission(&old_profile.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn gs_selector_profile_fails_closed_and_legacy_decode_is_rejected() {
        let code = [0x48, 0x8c, 0xe8];
        let mut input = fixture(&code).fixture;
        input.profile = v2::Profile::LongGsSelectorV1;
        input.initial_state.gs_base = Some(HexU64(0xffff_8000_0000_1000));
        input.initial_state.gs_selector = Some(HexU64(0xf123));
        let loaded = Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let old_profile = fixture(&code);
        let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
        assert!(admission(&old_profile.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn verw_gdt_profile_fails_closed_before_tracee_and_legacy_decode_is_rejected() {
        let code = [0x0f, 0x00, 0xe8];
        let mut input = fixture(&code).fixture;
        input.profile = v2::Profile::LongVerwGdtV1;
        input.initial_state.gdtr_base = Some(HexU64(0x20_0000));
        input.initial_state.gdtr_limit = Some(HexU64(0x0f));
        let loaded = Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let old_profile = fixture(&code);
        let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
        assert!(admission(&old_profile.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn fs_load_profile_fails_closed_before_tracee_and_legacy_decode_is_rejected() {
        let code = [0x8e, 0xe0];
        let mut input = fixture(&code).fixture;
        input.profile = v2::Profile::LongFsLoadV1;
        input.initial_state.fs_base = Some(HexU64(0xffff_8000_0000_1000));
        input.initial_state.fs_selector = Some(HexU64(0x33));
        input.initial_state.gdtr_base = Some(HexU64(0x20_0000));
        input.initial_state.gdtr_limit = Some(HexU64(0x0f));
        input.initial_state.fs_cache_limit = Some(HexU64(0xffff));
        input.initial_state.fs_cache_access = Some(HexU64(0xf3));
        input.initial_state.fs_cache_flags = Some(HexU64(0));
        let loaded = Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let composition = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22j_fs_base_composition.case.json"
        ))
        .unwrap();
        let result = execute(&composition);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let old_profile = fixture(&code);
        let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
        let error = admission(&old_profile.fixture, &mut decoder, &actual).unwrap_err();
        assert!(error.contains("FS/GS segment form is not admitted"));
    }

    #[test]
    fn gs_load_profile_fails_closed_before_tracee_and_legacy_decode_is_rejected() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22h_gs_load_writable_g0.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let composition = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22i_gs_base_composition.case.json"
        ))
        .unwrap();
        let result = execute(&composition);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let old_profile = fixture(&[0x8e, 0xe8]);
        let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
        let error = admission(&old_profile.fixture, &mut decoder, &actual).unwrap_err();
        assert!(error.contains("FS/GS segment form is not admitted"));
    }

    #[test]
    fn null_ds_es_profile_fails_closed_before_tracee_and_legacy_decode_is_rejected() {
        let code = [0x8e, 0xd8];
        let mut input = fixture(&code).fixture;
        input.profile = v2::Profile::LongNullDsEsV1;
        input.initial_state.ds_selector = Some(HexU64(0x2b));
        input.initial_state.es_selector = Some(HexU64(0x30));
        input.initial_state.ds_unusable = Some(false);
        input.initial_state.es_unusable = Some(false);
        let loaded = Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));

        let old_profile = fixture(&code);
        let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
        assert!(admission(&old_profile.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn x87_sse_snapshot_profile_is_explicitly_native_unsupported() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_34_1_x87_sse_raw_state_snapshot.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    }

    #[test]
    fn movaps_movapd_register_profiles_are_explicitly_native_unsupported() {
        for bytes in [
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_11_movaps_register.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_12_movaps_reverse_register.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_13_movapd_register.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_14_movapd_reverse_register.case.json"
            )[..],
            &include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_2_movups_register.case.json")
                [..],
            &include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_7a_xorps_register.case.json")
                [..],
            &include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_17_xorpd_register.case.json")
                [..],
            &include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_18_andpd_register.case.json")
                [..],
            &include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_7b_andps_register.case.json")
                [..],
            &include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_7c_orps_register.case.json")
                [..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_7d_andnps_register.case.json"
            )[..],
            &include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_8a_movss_register.case.json")
                [..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_8b_movss_store_register.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_8c_movss_memory_load_rax.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_8d_movss_memory_store_rax.case.json"
            )[..],
            &include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_9a_movsd_register.case.json")
                [..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_9c_movsd_memory_load_rax.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_9d_movsd_memory_store_rax.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_3_movups_store_register.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_4_movups_memory_load_rax.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_5_movups_memory_store_rax.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_6_movups_memory_base.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_6c_movups_sib_load.case.json"
            )[..],
            &include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_34_6c_movups_sib_store.case.json"
            )[..],
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let result = execute(&loaded);
            assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
            assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        }
    }

    #[test]
    fn movupd_register_profile_is_native_unsupported_with_unavailable_snapshot() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_34_15_movupd_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    }

    #[test]
    fn movupd_reverse_register_profile_is_native_unsupported_with_unavailable_snapshot() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_34_16_movupd_reverse_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    }

    #[test]
    fn x87_instructions_are_rejected_by_native_profile_and_instruction_admission() {
        for code in [
            [0xdb, 0x00],
            [0xdb, 0x10],
            [0xdb, 0x18],
            [0xdb, 0x28],
            [0xdb, 0xe3],
            [0xdb, 0xe2],
            [0xdb, 0xe9],
            [0xdb, 0xf1],
            [0xdf, 0xe0],
            [0xdf, 0xe9],
            [0xdf, 0xf1],
            [0xd9, 0xe0],
            [0xd9, 0xe4],
            [0xd9, 0xfc],
            [0xd9, 0xc0],
            [0xd9, 0x10],
            [0xd9, 0x18],
            [0xd9, 0x28],
            [0xdd, 0x38],
            [0xdd, 0xd0],
            [0xdd, 0xd1],
            [0xdd, 0xd2],
            [0xdd, 0xd3],
            [0xdd, 0xd4],
            [0xdd, 0xd5],
            [0xdd, 0xd6],
            [0xdd, 0xd7],
            [0xdd, 0xd8],
            [0xdd, 0xd9],
            [0xdd, 0xda],
            [0xdd, 0xdb],
            [0xdd, 0xdc],
            [0xdd, 0xdd],
            [0xdd, 0xde],
            [0xdd, 0xdf],
            [0xdd, 0xe1],
            [0xd8, 0xd0],
            [0xd8, 0xd1],
            [0xd8, 0xd2],
            [0xd8, 0xd3],
            [0xd8, 0xd4],
            [0xd8, 0xd5],
            [0xd8, 0xd6],
            [0xd8, 0xd7],
            [0xd8, 0xd9],
            [0xde, 0xd9],
            [0xda, 0xe9],
            [0xd8, 0xc0],
            [0xd8, 0xc1],
            [0xd8, 0xc2],
            [0xd8, 0xc3],
            [0xd8, 0xc4],
            [0xd8, 0xc5],
            [0xd8, 0xc6],
            [0xd8, 0xc7],
            [0xd8, 0xc8],
            [0xd8, 0xc9],
            [0xd8, 0xca],
            [0xd8, 0xcb],
            [0xd8, 0xcc],
            [0xd8, 0xcd],
            [0xd8, 0xce],
            [0xd8, 0xcf],
            [0xd8, 0xe0],
            [0xd8, 0xe1],
            [0xd8, 0xe2],
            [0xd8, 0xe3],
            [0xd8, 0xe4],
            [0xd8, 0xe5],
            [0xd8, 0xe6],
            [0xd8, 0xe7],
            [0xd8, 0xe8],
            [0xd8, 0xe9],
            [0xd8, 0xea],
            [0xd8, 0xeb],
            [0xd8, 0xec],
            [0xd8, 0xed],
            [0xd8, 0xee],
            [0xd8, 0xef],
            [0xd8, 0xf0],
            [0xd8, 0xf1],
            [0xd8, 0xf2],
            [0xd8, 0xf3],
            [0xd8, 0xf4],
            [0xd8, 0xf5],
            [0xd8, 0xf6],
            [0xd8, 0xf7],
            [0xd8, 0xf8],
            [0xd8, 0xf9],
            [0xd8, 0xfa],
            [0xd8, 0xfb],
            [0xd8, 0xfc],
            [0xd8, 0xfd],
            [0xd8, 0xfe],
            [0xd8, 0xff],
            [0xdc, 0xc0],
            [0xdc, 0xc1],
            [0xdc, 0xc2],
            [0xdc, 0xc3],
            [0xdc, 0xc4],
            [0xdc, 0xc5],
            [0xdc, 0xc6],
            [0xdc, 0xc7],
            [0xdc, 0xc8],
            [0xdc, 0xc9],
            [0xdc, 0xca],
            [0xdc, 0xcb],
            [0xdc, 0xcc],
            [0xdc, 0xcd],
            [0xdc, 0xce],
            [0xdc, 0xcf],
            [0xdc, 0xf0],
            [0xdc, 0xf1],
            [0xdc, 0xf2],
            [0xdc, 0xf3],
            [0xdc, 0xf4],
            [0xdc, 0xf5],
            [0xdc, 0xf6],
            [0xdc, 0xf7],
            [0xdc, 0xf8],
            [0xdc, 0xf9],
            [0xdc, 0xfa],
            [0xdc, 0xfb],
            [0xdc, 0xfc],
            [0xdc, 0xfd],
            [0xdc, 0xfe],
            [0xdc, 0xff],
            [0xdc, 0xe0],
            [0xdc, 0xe1],
            [0xdc, 0xe2],
            [0xdc, 0xe3],
            [0xdc, 0xe4],
            [0xdc, 0xe5],
            [0xdc, 0xe6],
            [0xdc, 0xe7],
            [0xdc, 0xe8],
            [0xdc, 0xe9],
            [0xdc, 0xea],
            [0xdc, 0xeb],
            [0xdc, 0xec],
            [0xdc, 0xed],
            [0xdc, 0xee],
            [0xdc, 0xef],
            [0xde, 0xc0],
            [0xde, 0xc1],
            [0xde, 0xc2],
            [0xde, 0xc3],
            [0xde, 0xc4],
            [0xde, 0xc5],
            [0xde, 0xc6],
            [0xde, 0xc7],
            [0xde, 0xc8],
            [0xde, 0xc9],
            [0xde, 0xca],
            [0xde, 0xcb],
            [0xde, 0xcc],
            [0xde, 0xcd],
            [0xde, 0xce],
            [0xde, 0xcf],
            [0xde, 0xe0],
            [0xde, 0xe1],
            [0xde, 0xe2],
            [0xde, 0xe3],
            [0xde, 0xe4],
            [0xde, 0xe5],
            [0xde, 0xe6],
            [0xde, 0xe7],
            [0xde, 0xe8],
            [0xde, 0xe9],
            [0xde, 0xea],
            [0xde, 0xeb],
            [0xde, 0xec],
            [0xde, 0xed],
            [0xde, 0xee],
            [0xde, 0xef],
            [0xde, 0xf0],
            [0xde, 0xf1],
            [0xde, 0xf2],
            [0xde, 0xf3],
            [0xde, 0xf4],
            [0xde, 0xf5],
            [0xde, 0xf6],
            [0xde, 0xf7],
            [0xde, 0xf8],
            [0xde, 0xf9],
            [0xde, 0xfa],
            [0xde, 0xfb],
            [0xde, 0xfc],
            [0xde, 0xfd],
            [0xde, 0xfe],
            [0xde, 0xff],
            [0xd9, 0xe0],
            [0xd9, 0xe1],
            [0xd9, 0xe4],
            [0xd8, 0xd1],
            [0xd8, 0xd8],
            [0xd8, 0xd9],
            [0xd8, 0xda],
            [0xd8, 0xdb],
            [0xd8, 0xdc],
            [0xd8, 0xdd],
            [0xd8, 0xde],
            [0xd8, 0xdf],
            [0xdd, 0xe9],
        ] {
            let mut input = fixture(&code).fixture;
            input.profile = v2::Profile::LongX87StateV1;
            input.instruction_budget = 1;
            input.initial_state.x87 = Some(v2::X87Snapshot::from(
                &od_isa::state::X87State::fixture_seed(),
            ));
            input.initial_state.rip = HexU64(0x1000);
            input.memory[0].guest_address = HexU64(0x1000);
            input.memory[0].length = 2;
            input.memory[0].initial_bytes_hex = code
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            input.memory[0].comparison_mask_hex = "ffff".into();
            if matches!(
                code,
                [0xd8, 0xe0]
                    | [0xd8, 0xf1]
                    | [0xd8, 0xf9]
                    | [0xd8, 0xe8]
                    | [0xdc, 0xf9]
                    | [0xdc, 0xe0]
                    | [0xdc, 0xe8]
                    | [0xde, 0xe0]
                    | [0xde, 0xc8]
                    | [0xde, 0xe8]
                    | [0xdb, 0x18]
                    | [0xd9, 0xe4]
                    | [0xd8, 0xd1]
                    | [0xd8, 0xd8]
                    | [0xd8, 0xd9]
                    | [0xd8, 0xda]
                    | [0xd8, 0xdb]
                    | [0xd8, 0xdc]
                    | [0xd8, 0xdd]
                    | [0xd8, 0xde]
                    | [0xd8, 0xdf]
                    | [0xdd, 0xe9]
                    | [0xda, 0xe9]
            ) {
                input.expected_exit = v2::ExpectedExit::Unsupported;
                input.stop_rip = None;
            } else {
                input.stop_rip = Some(HexU64(0x1002));
            }
            let json = input
                .to_json()
                .unwrap_or_else(|error| panic!("code={code:02x?}: {error:?}"));
            let loaded = Fixture::from_json(&json)
                .unwrap_or_else(|error| panic!("code={code:02x?}: {error:?}"));
            assert!(matches!(
                execute(&loaded).outcome,
                Outcome::Unsupported { .. }
            ));

            let old_profile = fixture(&code);
            let actual = old_profile.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&old_profile.fixture).unwrap();
            let error = admission(&old_profile.fixture, &mut decoder, &actual).unwrap_err();
            assert!(error.contains("x87 instruction is not admitted"));
        }
        for opcode in [0xdb, 0xdf] {
            for modrm in (0xe8u8..=0xef).chain(0xf0..=0xf7) {
                let code = [opcode, modrm];
                let mut input = fixture(&code).fixture;
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.x87 = Some(v2::X87Snapshot::from(
                    &od_isa::state::X87State::fixture_seed(),
                ));
                input.initial_state.rip = HexU64(0x1000);
                input.memory[0].guest_address = HexU64(0x1000);
                input.memory[0].length = 2;
                input.memory[0].initial_bytes_hex = code
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                input.memory[0].comparison_mask_hex = "ffff".into();
                input.stop_rip = Some(HexU64(0x1002));
                let loaded = Fixture::from_json(&input.to_json().unwrap())
                    .unwrap_or_else(|error| panic!("{code:02x?}: {error:?}"));
                assert!(matches!(
                    execute(&loaded).outcome,
                    Outcome::Unsupported { .. }
                ));
                let actual = loaded.fixture.initial_state.to_cpu().unwrap();
                let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let error = admission(&loaded.fixture, &mut decoder, &actual).unwrap_err();
                assert!(
                    error.contains("x87 instruction is not admitted"),
                    "{code:02x?}: {error}"
                );
            }
        }
    }

    #[test]
    fn s2_31az_subnormal_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31az_fld_m32_subnormal_fst_m64fp.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31az_fld_m32_largest_negative_subnormal_fstp_m64fp_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31ba_bs_sticky_status_m80_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ba_fstp_m80fp_sticky_status.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ba_fstp_m80fp_negative_snan_sticky_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bi_fstp_m80fp_negative_snan_sticky_ue_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bs_fstp_m80fp_negative_snan_sticky_oe_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bb_max_finite_m32_fld_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bb_fld_m32fp_positive_max_finite.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bb_fst_m32fp_positive_max_finite.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bb_fstp_m32fp_negative_max_finite_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bc_max_finite_m64_fld_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bc_fld_m64fp_positive_max_finite.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bc_fst_m64fp_positive_max_finite.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bc_fstp_m64fp_negative_max_finite_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bd_exact_m32_subnormal_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bd_fst_m32fp_positive_min_subnormal.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bd_fstp_m32fp_negative_min_subnormal_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31be_exact_m64_subnormal_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31be_fst_m64fp_positive_min_subnormal.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31be_fstp_m64fp_negative_min_subnormal_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bf_masked_inexact_m32_subnormal_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bf_fst_m32fp_positive_1_5_minsub_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bj_fst_m32fp_positive_0_75_minsub_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bf_fstp_m32fp_negative_1_5_minsub_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bl_special_binary80_subnormal_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bl_fst_m32fp_negative_min_subnormal_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31bn_binary80_minimum_normal_m32_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bn_fst_m32fp_positive_minimum_normal_nearest_tie_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bn_fstp_m32fp_negative_minimum_normal_directed_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bo_binary80_minimum_normal_m64_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bo_fst_m64fp_positive_minimum_normal_nearest_tie_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bo_fstp_m64fp_negative_minimum_normal_directed_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bp_binary80_overflow_to_infinity_m32_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bp_fst_m32fp_positive_infinity_overflow_nearest_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bp_fstp_m32fp_negative_infinity_overflow_directed_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bq_br_binary80_m64_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bq_fst_m64fp_positive_infinity_overflow_nearest_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bq_fstp_m64fp_negative_infinity_overflow_directed_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31br_m64fp_sticky_oe_exact_store_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bm_binary80_subnormal_m64_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bm_fst_m64fp_positive_min_subnormal_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bm_fstp_m64fp_negative_signed_zero_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bg_masked_inexact_m64_subnormal_store_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bg_fst_m64fp_positive_1_5_minsub.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bk_fst_m64fp_positive_0_75_minsub_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bg_fstp_m64fp_negative_1_5_minsub_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31bh_sticky_ue_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bh_fst_m32fp_sticky_ue_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cf_masked_signaling_nan_fist_m32_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cf_fistp_m32int_negative_snan_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cg_masked_signaling_nan_fist_m16_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cg_fistp_m16int_negative_snan_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31ch_masked_signaling_nan_fistp_m64_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ch_fistp_m64int_negative_snan_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31ci_fist_snan_fixture_remains_outside_native_admission() {
        let bytes =
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ci_fist_snan_fnstsw.case.json")
                .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31ci_fistp_exact_incoming_ie_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ci_fistp_exact_ie_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cj_finite_overflow_fistp_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cj_fistp_m32int_positive_overflow_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cj_roundback_fist_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cj_fist_m32int_positive_roundback_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cr_subnormal_fist_m16_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cr_fistp_m16int_negative_subnormal_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cs_subnormal_fistp_m64_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cs_fld_fistp_m64int_negative_subnormal_fnstsw_fnclex.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31ct_pseudo_infinity_fistp_m32_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ct_fistp_m32int_negative_pseudo_infinity_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cx_pseudo_nan_fistp_m16_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cx_fistp_m16int_negative_pseudo_nan_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cw_pseudo_nan_fistp_m32_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cw_fistp_m32int_negative_pseudo_nan_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cz_unnormal_fistp_m32_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cz_fistp_m32int_negative_unnormal.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31da_unnormal_fistp_m16_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31da_fistp_m16int_negative_unnormal.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31db_unnormal_fistp_m64_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31db_fistp_m64int_negative_unnormal.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cu_pseudo_infinity_fistp_m16_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cu_fistp_m16int_negative_pseudo_infinity_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cv_pseudo_infinity_fistp_m64_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cv_fistp_m64int_negative_pseudo_infinity_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cy_pseudo_nan_fistp_m64_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cy_fistp_m64int_negative_pseudo_nan_fnstsw.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31ck_m16_overflow_and_roundback_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ck_fistp_m16int_positive_overflow_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ck_fist_m16int_positive_roundback_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_31cl_m64_finite_overflow_fixture_remains_outside_native_admission() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31f_fistp_m64int_positive_overflow.case.json"
        )
        .as_slice();
        let loaded = Fixture::from_json(bytes).unwrap();
        let actual = loaded.fixture.initial_state.to_cpu().unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
    }

    #[test]
    fn s2_31cm_cn_co_bounded_integer_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cm_fistp_m32int_loaded_max_subnormal_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cm_fistp_m64int_loaded_max_subnormal_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cn_fistp_m32int_sticky_ue_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cn_fistp_m64int_sticky_ue_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31co_fistp_m32int_sticky_oe_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31co_fistp_m64int_sticky_oe_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cp_fistp_m32int_sticky_ze_fnstsw.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cp_fistp_m64int_sticky_ze_fnstsw.case.json").as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(admission(&loaded.fixture, &mut decoder, &actual).is_err());
        }
    }

    #[test]
    fn s2_18j_cld_std_fixtures_remain_outside_native_admission() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_18j_cld_df0.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_18j_std_df0.case.json")
                .as_slice(),
        ] {
            let loaded = Fixture::from_json(bytes).unwrap();
            let actual = loaded.fixture.initial_state.to_cpu().unwrap();
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            assert!(
                admission(&loaded.fixture, &mut decoder, &actual).is_err(),
                "CLD/STD must fail native admission before tracee execution"
            );
        }
    }

    #[test]
    fn s2_18k_addr32_lods_forms_fail_native_admission_before_tracee_or_snapshot() {
        for code in [
            &[0x67, 0xac][..],       // LODSB, address-size 32
            &[0x67, 0x66, 0xad][..], // LODSW, address-size 32
            &[0x67, 0xad][..],       // LODSD, address-size 32
            &[0x67, 0x48, 0xad][..], // LODSQ, address-size 32
        ] {
            let loaded = fixture(code);
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            actual.gpr[6] = 0xffff_ffff_0020_0000;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            // `admission` passes no tracee PID and returns only flag effects or
            // an error; rejected instructions cannot produce a guest snapshot.
            let error = admission(&loaded.fixture, &mut decoder, &actual)
                .expect_err("address-size-32 LODS must remain outside native execution");
            assert!(
                error.contains("not in data-transfer/integer arithmetic/direct-branch allowlist"),
                "unexpected admission error for {code:02x?}: {error}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_18l_addr32_stos_forms_fail_native_admission_before_tracee_or_snapshot() {
        for code in [
            &[0x67, 0xaa][..],       // STOSB, address-size 32
            &[0x67, 0x66, 0xab][..], // STOSW, address-size 32
            &[0x67, 0xab][..],       // STOSD, address-size 32
            &[0x67, 0x48, 0xab][..], // STOSQ, address-size 32
        ] {
            let loaded = fixture(code);
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            actual.gpr[7] = 0xffff_ffff_0020_0000;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            // Admission has no tracee PID at this boundary, so the rejected
            // forms cannot begin native execution or produce a snapshot.
            let error = admission(&loaded.fixture, &mut decoder, &actual)
                .expect_err("address-size-32 STOS must remain outside native execution");
            assert!(
                error.contains("native STOS admission requires a bare STOSB form"),
                "unexpected admission error for {code:02x?}: {error}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_18m_addr32_scas_forms_fail_native_admission_before_tracee_or_snapshot() {
        for code in [
            &[0x67, 0xae][..],       // SCASB, address-size 32
            &[0x67, 0x66, 0xaf][..], // SCASW, address-size 32
            &[0x67, 0xaf][..],       // SCASD, address-size 32
            &[0x67, 0x48, 0xaf][..], // SCASQ, address-size 32
        ] {
            let loaded = fixture(code);
            let mut actual = loaded.fixture.initial_state.to_cpu().unwrap();
            // The low EDI and the corresponding 64-bit address are both mapped;
            // rejection is due to the unsupported instruction profile.
            actual.gpr[7] = 0x0000_0000_0020_0004;
            let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let error = admission(&loaded.fixture, &mut decoder, &actual)
                .expect_err("address-size-32 SCAS must remain outside native execution");
            assert_eq!(
                error,
                "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist",
                "unexpected admission error for {code:02x?}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_1_pxor_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_1_pxor_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let reason = admission(&loaded.fixture, &mut decoder, &actual).unwrap_err();
        assert_eq!(
            reason,
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_49_pand_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_49_pand_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_50_pandn_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_50_pandn_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_51_por_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_51_por_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_2_paddb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_2_paddb_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_3_psubb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_3_psubb_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_35_pshufd_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_35_pshufd_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_37_psllw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_37_psllw_imm_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_41_pslld_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_41_pslld_imm_register.case.json"
        ))
        .unwrap();
        assert!(
            matches!(execute(&loaded).outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_44_psllq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_44_psllq_imm_register.case.json"
        ))
        .unwrap();
        assert!(
            matches!(execute(&loaded).outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_45_psrlq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_45_psrlq_imm_register.case.json"
        ))
        .unwrap();
        assert!(
            matches!(execute(&loaded).outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_46_pslldq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_46_pslldq_imm_register.case.json"
        ))
        .unwrap();
        assert!(
            matches!(execute(&loaded).outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_47_psrldq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_47_psrldq_imm_register.case.json"
        ))
        .unwrap();
        assert!(
            matches!(execute(&loaded).outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_42_psrld_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_42_psrld_imm_register.case.json"
        ))
        .unwrap();
        assert!(
            matches!(execute(&loaded).outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_43_psrad_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_43_psrad_imm_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_38_psrlw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_38_psrlw_imm_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_39_psraw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_39_psraw_imm_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_4_paddusb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_4_paddusb_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_5_psubusb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_5_psubusb_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_6_paddw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_6_paddw_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_7_psubw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_7_psubw_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_8_paddusw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_8_paddusw_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_9_psubusw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_9_psubusw_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_10_paddsb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_10_paddsb_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_11_psubsb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_11_psubsb_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_13_psubsw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_13_psubsw_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_14_paddd_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_14_paddd_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_16_paddq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_16_paddq_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_17_psubq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_17_psubq_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_18_pcmpeqb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_18_pcmpeqb_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_19_pcmpeqw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_19_pcmpeqw_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_21_pcmpgtb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_21_pcmpgtb_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_22_pcmpgtw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_22_pcmpgtw_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_23_pcmpgtd_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_23_pcmpgtd_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_24_punpcklbw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_24_punpcklbw_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_25_punpckhbw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_25_punpckhbw_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_26_punpcklwd_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_26_punpcklwd_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_27_punpckhwd_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_27_punpckhwd_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_28_punpckldq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_28_punpckldq_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_29_punpckhdq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_29_punpckhdq_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_30_punpcklqdq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_30_punpcklqdq_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_31_punpckhqdq_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_31_punpckhqdq_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_20_pcmpeqd_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_20_pcmpeqd_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_15_psubd_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_15_psubd_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(
            result.outcome,
            Outcome::Unsupported { ref reason }
                if reason == "this profile is local-only; native state does not observe every profile field"
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_12_paddsw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_12_paddsw_register.case.json"
        ))
        .unwrap();
        assert!(matches!(
            execute(&loaded).outcome,
            Outcome::Unsupported { .. }
        ));
        let actual = loaded
            .fixture
            .initial_state
            .to_cpu_for(loaded.fixture.profile)
            .unwrap();
        let mut decoder = FlatMemory::from_fixture(&loaded.fixture).unwrap();
        assert_eq!(
            admission(&loaded.fixture, &mut decoder, &actual).unwrap_err(),
            "native instruction is not in data-transfer/integer arithmetic/direct-branch allowlist"
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_32_packsswb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_32_packsswb_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_33_packuswb_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_33_packuswb_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn s2_35_34_packssdw_fixture_is_denied_before_native_execution() {
        let loaded = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_34_packssdw_register.case.json"
        ))
        .unwrap();
        let result = execute(&loaded);
        assert!(matches!(result.outcome, Outcome::Unsupported { ref reason }
            if reason == "this profile is local-only; native state does not observe every profile field"));
    }
}
