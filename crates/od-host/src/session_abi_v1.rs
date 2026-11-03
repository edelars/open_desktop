//! C ABI proxy for one persistent, thread-affine S0-09 session.
//!
//! The C handle owns synchronization and a worker join handle. `Session` is
//! created, used and dropped inside the worker closure and never crosses it.

use crate::session::{Session, SessionCreateError};
use crate::{ExecutableMemoryError, PublishedArenaError};
use od_isa::{LongModeState, decode};
use od_jit::block::{AdmittedInstructionBoundaries, CodeView};
use od_jit::dispatch::DispatchExit;
use od_jit::runtime::RuntimeExitTag;
use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Write};
use std::mem::{align_of, size_of};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};

const ABI_VERSION: u32 = 0x0001_0000;
const CAP_S0_09: u64 = 1;
const MAX_CODE: usize = 4096;
const COMPLETION_BYTE: u8 = 1;

const OK: u32 = 0;
const INVALID_ARGUMENT: u32 = 1;
const INVALID_CODE: u32 = 2;
const BUSY: u32 = 3;
const UNSUPPORTED_HOST: u32 = 4;
const ALLOCATION_FAILED: u32 = 5;
const INVALID_STATE: u32 = 6;
const INVALID_REQUEST_ID: u32 = 7;
const CODE_MISMATCH: u32 = 8;
const NO_ACTIVE_RUN: u32 = 9;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CpuState {
    gpr: [u64; 4],
    rip: u64,
    rflags: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CreateOptions {
    struct_size: u32,
    code_length: u32,
    first_entry_rip: u64,
    code: [u8; MAX_CODE],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RunRequest {
    struct_size: u32,
    request_id: u32,
    instruction_budget: u64,
    initial_state: CpuState,
    code_length: u32,
    reserved: u32,
    code: [u8; MAX_CODE],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RunToken {
    struct_size: u32,
    request_id: u32,
    completion_read_fd: i32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RunResult {
    struct_size: u32,
    request_id: u32,
    state: CpuState,
    reason: u32,
    reserved: u32,
    instructions_executed: u64,
}

const _: [(); 48] = [(); size_of::<CpuState>()];
const _: [(); 4112] = [(); size_of::<CreateOptions>()];
const _: [(); 4168] = [(); size_of::<RunRequest>()];
const _: [(); 16] = [(); size_of::<RunToken>()];
const _: [(); 72] = [(); size_of::<RunResult>()];
const _: [(); 8] = [(); align_of::<CpuState>()];
const _: [(); 8] = [(); align_of::<CreateOptions>()];
const _: [(); 8] = [(); align_of::<RunRequest>()];
const _: [(); 4] = [(); align_of::<RunToken>()];
const _: [(); 8] = [(); align_of::<RunResult>()];
const _: [(); 32] = [(); std::mem::offset_of!(CpuState, rip)];
const _: [(); 40] = [(); std::mem::offset_of!(CpuState, rflags)];
const _: [(); 8] = [(); std::mem::offset_of!(CreateOptions, first_entry_rip)];
const _: [(); 16] = [(); std::mem::offset_of!(CreateOptions, code)];
const _: [(); 16] = [(); std::mem::offset_of!(RunRequest, initial_state)];
const _: [(); 64] = [(); std::mem::offset_of!(RunRequest, code_length)];
const _: [(); 72] = [(); std::mem::offset_of!(RunRequest, code)];
const _: [(); 8] = [(); std::mem::offset_of!(RunToken, completion_read_fd)];
const _: [(); 8] = [(); std::mem::offset_of!(RunResult, state)];
const _: [(); 56] = [(); std::mem::offset_of!(RunResult, reason)];
const _: [(); 64] = [(); std::mem::offset_of!(RunResult, instructions_executed)];

#[derive(Clone, Copy)]
struct Completed {
    status: u32,
    result: RunResult,
}

struct Invocation {
    id: u32,
    stop: Arc<AtomicU8>,
    waiter: bool,
    notification_finished: bool,
    completed: Option<Completed>,
}

enum Life {
    Idle,
    Running(Invocation),
    Failed,
}

struct SharedState {
    life: Life,
    high_water: u32,
}

struct Shared {
    state: Mutex<SharedState>,
    changed: Condvar,
}

struct OwnedRun {
    id: u32,
    budget: u64,
    state: CpuState,
    code: Vec<u8>,
    stop: Arc<AtomicU8>,
    writer: OwnedFd,
}

enum Command {
    Run(OwnedRun),
    Shutdown,
}

struct Proxy {
    code: Vec<u8>,
    shared: Arc<Shared>,
    command: mpsc::Sender<Command>,
    owner: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn valid_input<T>(ptr: *const c_void, explicit_size: u32, expected_size: usize) -> bool {
    !ptr.is_null()
        && (ptr as usize) % align_of::<T>() == 0
        && usize::try_from(explicit_size).ok() == Some(expected_size)
}

fn valid_output<T>(ptr: *mut c_void, explicit_size: u32, expected_size: usize) -> bool {
    !ptr.is_null()
        && (ptr as usize) % align_of::<T>() == 0
        && usize::try_from(explicit_size).ok() == Some(expected_size)
}

fn overlaps(left: *const u8, left_size: usize, right: *const u8, right_size: usize) -> bool {
    let left_start = left as usize;
    let right_start = right as usize;
    let Some(left_end) = left_start.checked_add(left_size) else { return true };
    let Some(right_end) = right_start.checked_add(right_size) else { return true };
    left_start < right_end && right_start < left_end
}

fn code_is_zero_padded(code: &[u8], tail: &[u8]) -> bool {
    code.len() <= MAX_CODE && tail.iter().all(|byte| *byte == 0)
}

fn valid_rip(code: &[u8], rip: u64) -> bool {
    let Ok(view) = CodeView::new(0, code) else {
        return false;
    };
    let Ok(instructions) = decode(code) else {
        return false;
    };
    let Ok(code_end) = u64::try_from(code.len()) else {
        return false;
    };
    view.code_end() == code_end
        && (rip == code_end
            || instructions
                .iter()
                .any(|instruction| u64::try_from(instruction.offset).ok() == Some(rip)))
}

fn create_status(error: SessionCreateError) -> u32 {
    match error {
        SessionCreateError::CodeTooLarge { .. }
        | SessionCreateError::CodeBounds(_)
        | SessionCreateError::Decode(_)
        | SessionCreateError::Admission(_) => INVALID_CODE,
        SessionCreateError::Cache(_) => ALLOCATION_FAILED,
        SessionCreateError::Arena(PublishedArenaError::Mapping(
            ExecutableMemoryError::MappingAlreadyLive,
        )) => BUSY,
        SessionCreateError::Arena(PublishedArenaError::Mapping(
            ExecutableMemoryError::UnsupportedPlatform,
        )) => UNSUPPORTED_HOST,
        SessionCreateError::Arena(PublishedArenaError::Mapping(
            ExecutableMemoryError::AllocationFailed,
        )) => ALLOCATION_FAILED,
        SessionCreateError::Arena(PublishedArenaError::MappingAlreadyLive) => BUSY,
        SessionCreateError::Arena(_) => INVALID_STATE,
    }
}

fn result_state(state: &LongModeState) -> CpuState {
    CpuState {
        gpr: [state.gpr[0], state.gpr[1], state.gpr[2], state.gpr[3]],
        rip: state.rip,
        rflags: state.rflags,
    }
}

fn write_completion(fd: OwnedFd) -> io::Result<()> {
    // SAFETY: ownership of this descriptor was moved into exactly one accepted
    // command; no other code closes or writes it.
    let mut writer = File::from(fd);
    writer.write_all(&[COMPLETION_BYTE])?;
    writer.flush()?;
    drop(writer);
    Ok(())
}

fn owner_loop(code: Vec<u8>, shared: Arc<Shared>, receiver: mpsc::Receiver<Command>, ready: mpsc::SyncSender<u32>) {
    let mut session = match Session::new(&code) {
        Ok(session) => {
            if ready.send(OK).is_err() {
                return;
            }
            session
        }
        Err(error) => {
            let _ = ready.send(create_status(error));
            return;
        }
    };

    struct FailureWake(Arc<Shared>, bool);
    impl Drop for FailureWake {
        fn drop(&mut self) {
            if self.1 {
                let mut state = lock(&self.0.state);
                if let Life::Running(invocation) = &mut state.life {
                    invocation.stop.store(0, Ordering::Release);
                    invocation.completed = Some(Completed { status: INVALID_STATE, result: RunResult::default() });
                    invocation.notification_finished = true;
                } else {
                    state.life = Life::Failed;
                }
                self.0.changed.notify_all();
            }
        }
    }
    let mut failure_wake = FailureWake(Arc::clone(&shared), true);

    while let Ok(command) = receiver.recv() {
        match command {
            Command::Shutdown => { failure_wake.1 = false; break; }
            Command::Run(run) => {
                let mut guest = LongModeState {
                    gpr: [run.state.gpr[0], run.state.gpr[1], run.state.gpr[2], run.state.gpr[3], 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    rip: run.state.rip,
                    rflags: run.state.rflags,
                    ..LongModeState::default()
                };
                let (status, result) = match session.run(&mut guest, run.budget, &run.stop) {
                    Ok(outcome) => {
                        let reason = match outcome.dispatch.exit {
                            DispatchExit::CodeEnd => Some(0),
                            DispatchExit::BudgetExhausted => Some(1),
                            DispatchExit::Runtime(RuntimeExitTag::StopRequested) => Some(2),
                            _ => None,
                        };
                        match reason {
                            Some(reason) => (OK, RunResult {
                                struct_size: size_of::<RunResult>() as u32,
                                request_id: run.id,
                                state: result_state(&guest),
                                reason,
                                reserved: 0,
                                instructions_executed: outcome.dispatch.retired_instructions,
                            }),
                            None => (INVALID_STATE, RunResult::default()),
                        }
                    }
                    Err(_) => (INVALID_STATE, RunResult::default()),
                };

                {
                    let mut state = lock(&shared.state);
                    if let Life::Running(invocation) = &mut state.life {
                        if invocation.id == run.id {
                            invocation.completed = Some(Completed { status, result });
                            invocation.stop.store(0, Ordering::Release);
                        }
                    }
                    shared.changed.notify_all();
                }

                let notified = write_completion(run.writer).is_ok();
                let mut state = lock(&shared.state);
                if let Life::Running(invocation) = &mut state.life {
                        if invocation.id == run.id {
                        invocation.notification_finished = true;
                        if !notified {
                            invocation.completed = Some(Completed { status: INVALID_STATE, result: RunResult::default() });
                        }
                    }
                }
                shared.changed.notify_all();
                drop(run.code);
            }
        }
    }
    drop(session);
}

#[unsafe(no_mangle)]
pub extern "C" fn od_session_abi_version() -> u32 { ABI_VERSION }

#[unsafe(no_mangle)]
pub extern "C" fn od_session_abi_capabilities() -> u64 {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) { CAP_S0_09 } else { 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_session_create_v1(options: *const c_void, options_size: u32, out_handle: *mut *mut c_void) -> u32 {
    if !valid_input::<CreateOptions>(options, options_size, size_of::<CreateOptions>())
        || !valid_output::<*mut c_void>(out_handle.cast(), size_of::<*mut c_void>() as u32, size_of::<*mut c_void>())
        || overlaps(options.cast(), size_of::<CreateOptions>(), out_handle.cast(), size_of::<*mut c_void>())
    { return INVALID_ARGUMENT; }
    // SAFETY: pointer shape and explicit size have been checked; accessibility
    // and lifetime are caller obligations of this ABI.
    let options = unsafe { &*options.cast::<CreateOptions>() };
    if options.struct_size as usize != size_of::<CreateOptions>()
        || options.code_length as usize > MAX_CODE
        || !code_is_zero_padded(&options.code[..options.code_length as usize], &options.code[options.code_length as usize..])
    { return INVALID_ARGUMENT; }
    // SAFETY: out_handle was validated and the ABI requires a writable slot.
    if unsafe { !(*out_handle).is_null() } { return INVALID_ARGUMENT; }
    let source_code = &options.code[..options.code_length as usize];
    let mut code = Vec::new();
    if code.try_reserve_exact(source_code.len()).is_err() { return ALLOCATION_FAILED; }
    code.extend_from_slice(source_code);
    let code_view = match CodeView::new(0, &code) { Ok(view) => view, Err(_) => return INVALID_CODE };
    let decoded = match decode(&code) { Ok(decoded) => decoded, Err(_) => return INVALID_CODE };
    if AdmittedInstructionBoundaries::from_fixture_decode(code_view, &decoded).is_err() { return INVALID_CODE; }
    if !valid_rip(&code, options.first_entry_rip) { return INVALID_ARGUMENT; }
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) { return UNSUPPORTED_HOST; }

    let (commands, receiver) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let shared = Arc::new(Shared { state: Mutex::new(SharedState { life: Life::Idle, high_water: 0 }), changed: Condvar::new() });
    let owner_shared = Arc::clone(&shared);
    let mut owner_code = Vec::new();
    if owner_code.try_reserve_exact(code.len()).is_err() { return ALLOCATION_FAILED; }
    owner_code.extend_from_slice(&code);
    let proxy = Box::new(Proxy { code, shared, command: commands, owner: Mutex::new(None) });
    let owner = match thread::Builder::new().name("od-session-owner".into()).spawn(move || owner_loop(owner_code, owner_shared, receiver, ready_tx)) {
        Ok(owner) => owner,
        Err(_) => return ALLOCATION_FAILED,
    };
    let init_status = match ready_rx.recv() { Ok(status) => status, Err(_) => INVALID_STATE };
    if init_status != OK {
        let _ = owner.join();
        return init_status;
    }
    *lock(&proxy.owner) = Some(owner);
    // SAFETY: pointer slot is valid and null; Box ownership transfers to C.
    unsafe { *out_handle = Box::into_raw(proxy).cast(); }
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_session_begin_run_v1(handle: *mut c_void, request: *const c_void, request_size: u32, out_token: *mut c_void, out_token_size: u32) -> u32 {
    if handle.is_null() || !valid_input::<RunRequest>(request, request_size, size_of::<RunRequest>()) || !valid_output::<RunToken>(out_token, out_token_size, size_of::<RunToken>()) || overlaps(request.cast(), size_of::<RunRequest>(), out_token.cast(), size_of::<RunToken>()) || overlaps(request.cast(), size_of::<RunRequest>(), handle.cast(), size_of::<Proxy>()) || overlaps(out_token.cast(), size_of::<RunToken>(), handle.cast(), size_of::<Proxy>()) { return INVALID_ARGUMENT; }
    // SAFETY: the handle is an active proxy created by this ABI; request
    // accessibility is the caller's obligation for this call.
    let proxy = unsafe { &*handle.cast::<Proxy>() };
    let input = unsafe { &*request.cast::<RunRequest>() };
    if input.struct_size as usize != size_of::<RunRequest>() || input.code_length as usize > MAX_CODE || input.reserved != 0 || !code_is_zero_padded(&input.code[..input.code_length as usize], &input.code[input.code_length as usize..]) { return INVALID_ARGUMENT; }
    let mut shared = lock(&proxy.shared.state);
    if input.request_id == 0 || input.request_id <= shared.high_water { return INVALID_REQUEST_ID; }
    shared.high_water = input.request_id;
    if matches!(&shared.life, Life::Failed) { return INVALID_STATE; }
    if matches!(&shared.life, Life::Running(invocation) if invocation.completed.is_some_and(|completed| completed.status != OK)) { return INVALID_STATE; }
    if !matches!(&shared.life, Life::Idle) { return BUSY; }
    let request_code = &input.code[..input.code_length as usize];
    if request_code != proxy.code { return CODE_MISMATCH; }
    if !valid_rip(request_code, input.initial_state.rip) { return INVALID_ARGUMENT; }
    let mut candidate_code = Vec::new();
    if candidate_code.try_reserve_exact(request_code.len()).is_err() { return ALLOCATION_FAILED; }
    candidate_code.extend_from_slice(request_code);
    let (read_fd, write_fd) = match create_cloexec_pipe() { Ok(pipe) => pipe, Err(_) => return ALLOCATION_FAILED };
    let stop = Arc::new(AtomicU8::new(0));
    let run = OwnedRun { id: input.request_id, budget: input.instruction_budget, state: input.initial_state, code: candidate_code, stop: Arc::clone(&stop), writer: write_fd };
    shared.life = Life::Running(Invocation { id: input.request_id, stop, waiter: false, notification_finished: false, completed: None });
    if proxy.command.send(Command::Run(run)).is_err() {
        if let Life::Running(invocation) = &mut shared.life {
            invocation.completed = Some(Completed { status: INVALID_STATE, result: RunResult::default() });
            invocation.notification_finished = true;
        }
        proxy.shared.changed.notify_all();
    }
    // SAFETY: output pointer was validated and this call exclusively owns it.
    unsafe { out_token.cast::<RunToken>().write(RunToken { struct_size: size_of::<RunToken>() as u32, request_id: input.request_id, completion_read_fd: read_fd.into_raw_fd(), reserved: 0 }); }
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_session_request_stop_v1(handle: *mut c_void, request_id: u32) -> u32 {
    if handle.is_null() { return INVALID_ARGUMENT; }
    // SAFETY: caller must synchronize destruction against in-flight calls.
    let proxy = unsafe { &*handle.cast::<Proxy>() };
    let shared = lock(&proxy.shared.state);
    match &shared.life {
        Life::Running(invocation) if invocation.completed.is_some() => NO_ACTIVE_RUN,
        Life::Running(invocation) if invocation.id != request_id => INVALID_REQUEST_ID,
        Life::Running(invocation) => { invocation.stop.store(1, Ordering::Release); OK }
        Life::Idle | Life::Failed => NO_ACTIVE_RUN,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_session_wait_run_v1(handle: *mut c_void, request_id: u32, out_result: *mut c_void, out_result_size: u32) -> u32 {
    if handle.is_null() || !valid_output::<RunResult>(out_result, out_result_size, size_of::<RunResult>()) || overlaps(out_result.cast(), size_of::<RunResult>(), handle.cast(), size_of::<Proxy>()) { return INVALID_ARGUMENT; }
    // SAFETY: caller synchronizes destruction with this operation.
    let proxy = unsafe { &*handle.cast::<Proxy>() };
    let mut state = lock(&proxy.shared.state);
    match &mut state.life {
        Life::Idle => return NO_ACTIVE_RUN,
        Life::Failed => return INVALID_STATE,
        Life::Running(invocation) if invocation.id != request_id => return INVALID_REQUEST_ID,
        Life::Running(invocation) if invocation.waiter => return BUSY,
        Life::Running(invocation) => invocation.waiter = true,
    }
    loop {
        let finished = matches!(&state.life, Life::Running(invocation) if invocation.notification_finished);
        if finished { break; }
        state = proxy.shared.changed.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(&state.life, Life::Failed) { return INVALID_STATE; }
    }
    let completion = match &state.life { Life::Running(invocation) => invocation.completed.unwrap_or(Completed { status: INVALID_STATE, result: RunResult::default() }), _ => return INVALID_STATE };
    if completion.status == OK {
        // SAFETY: output is validated, writable, and exclusive to this call.
        unsafe { out_result.cast::<RunResult>().write(completion.result); }
        state.life = Life::Idle;
    } else { state.life = Life::Failed; }
    proxy.shared.changed.notify_all();
    completion.status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_session_destroy_v1(handle: *mut c_void) -> u32 {
    if handle.is_null() { return INVALID_ARGUMENT; }
    // SAFETY: caller excludes new calls and synchronizes all other operations.
    let proxy = unsafe { &*handle.cast::<Proxy>() };
    if !matches!(&lock(&proxy.shared.state).life, Life::Idle | Life::Failed) { return BUSY; }
    let _ = proxy.command.send(Command::Shutdown);
    if let Some(owner) = lock(&proxy.owner).take() { if owner.join().is_err() { return INVALID_STATE; } }
    // SAFETY: caller supplied the unique live handle and destroy invalidates it.
    drop(unsafe { Box::from_raw(handle.cast::<Proxy>()) });
    OK
}

unsafe extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn fcntl(fd: i32, command: i32, ...) -> i32;
}

fn create_cloexec_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // F_SETFD and FD_CLOEXEC are 2 and 1 on Darwin and other supported Unix hosts.
    const F_SETFD: i32 = 2;
    const FD_CLOEXEC: i32 = 1;
    // SAFETY: `fds` is a valid two-element output array.
    if unsafe { pipe(fds.as_mut_ptr()) } != 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: pipe returned two fresh descriptors, now owned by these values.
    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    // SAFETY: pipe returned two fresh descriptors, now owned by these values.
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    for fd in [read_fd.as_raw_fd(), write_fd.as_raw_fd()] {
        // SAFETY: fds are the two valid descriptors returned by pipe.
        if unsafe { fcntl(fd, F_SETFD, FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read_fd, write_fd))
}


#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;
    use std::ptr;

    const MOV_RAX_7: [u8; 10] = [0x48, 0xb8, 7, 0, 0, 0, 0, 0, 0, 0];
    const SPIN: [u8; 2] = [0x75, 0xfe];

    fn assert_state_eq(actual: CpuState, expected: CpuState) {
        assert_eq!(actual.gpr, expected.gpr);
        assert_eq!(actual.rip, expected.rip);
        assert_eq!(actual.rflags, expected.rflags);
    }

    fn options(code: &[u8], first_entry_rip: u64) -> CreateOptions {
        let mut options = CreateOptions { struct_size: size_of::<CreateOptions>() as u32, code_length: code.len() as u32, first_entry_rip, code: [0; MAX_CODE] };
        options.code[..code.len()].copy_from_slice(code);
        options
    }

    unsafe fn create(code: &[u8]) -> Result<*mut c_void, u32> {
        let options = options(code, 0);
        let mut handle = ptr::null_mut();
        let status = unsafe { od_session_create_v1((&options as *const CreateOptions).cast(), size_of::<CreateOptions>() as u32, &mut handle) };
        if status == OK { Ok(handle) } else { Err(status) }
    }

    fn request(code: &[u8], id: u32, rip: u64, rax: u64, budget: u64) -> RunRequest {
        let mut request = RunRequest { struct_size: size_of::<RunRequest>() as u32, request_id: id, instruction_budget: budget, initial_state: CpuState { gpr: [rax, 2, 3, 4], rip, rflags: 2 }, code_length: code.len() as u32, reserved: 0, code: [0; MAX_CODE] };
        request.code[..code.len()].copy_from_slice(code);
        request
    }

    unsafe fn begin(handle: *mut c_void, request: &RunRequest, token: &mut RunToken) -> u32 {
        unsafe { od_session_begin_run_v1(handle, (request as *const RunRequest).cast(), size_of::<RunRequest>() as u32, (token as *mut RunToken).cast(), size_of::<RunToken>() as u32) }
    }

    unsafe fn wait(handle: *mut c_void, id: u32, result: &mut RunResult) -> u32 {
        unsafe { od_session_wait_run_v1(handle, id, (result as *mut RunResult).cast(), size_of::<RunResult>() as u32) }
    }

    #[test]
    fn s3_38f_abi_create_preflight_runs_before_map_jit_lease() {
        let _guard = crate::tests::jit_test_lock();
        let invalid = options(&MOV_RAX_7, 1);
        let mut handle = ptr::null_mut();
        assert_eq!(unsafe { od_session_create_v1((&invalid as *const CreateOptions).cast(), size_of::<CreateOptions>() as u32, &mut handle) }, INVALID_ARGUMENT);
        assert!(handle.is_null());
        let handle = unsafe { create(&MOV_RAX_7) }.unwrap();
        assert_eq!(unsafe { od_session_destroy_v1(handle) }, OK);
    }

    #[test]
    fn s3_38f_proxy_stop_request_ids_and_destroy_busy_are_exact() {
        let _guard = crate::tests::jit_test_lock();
        let handle = unsafe { create(&SPIN) }.unwrap();
        let run = request(&SPIN, 1, 0, 0, u64::MAX);
        let mut token = RunToken { struct_size: 91, request_id: 92, completion_read_fd: 93, reserved: 94 };
        assert_eq!(unsafe { begin(handle, &run, &mut token) }, OK);
        assert_ne!(unsafe { fcntl(token.completion_read_fd, 1) } & 1, 0);
        let accepted_token = token;
        assert_eq!(unsafe { begin(handle, &run, &mut token) }, INVALID_REQUEST_ID);
        assert_eq!(token.struct_size, accepted_token.struct_size);
        assert_eq!(token.request_id, accepted_token.request_id);
        assert_eq!(token.completion_read_fd, accepted_token.completion_read_fd);
        assert_eq!(token.reserved, accepted_token.reserved);
        assert_eq!(unsafe { od_session_destroy_v1(handle) }, BUSY);
        assert_eq!(unsafe { od_session_request_stop_v1(handle, 0) }, INVALID_REQUEST_ID);
        assert_eq!(unsafe { od_session_request_stop_v1(handle, 2) }, INVALID_REQUEST_ID);
        assert_eq!(unsafe { od_session_request_stop_v1(handle, 1) }, OK);
        let mut unchanged = RunResult { struct_size: 31, request_id: 32, state: CpuState { gpr: [33, 34, 35, 36], rip: 37, rflags: 38 }, reason: 39, reserved: 40, instructions_executed: 41 };
        let before = unchanged;
        assert_eq!(unsafe { wait(handle, 2, &mut unchanged) }, INVALID_REQUEST_ID);
        assert_eq!(unchanged.struct_size, before.struct_size);
        assert_eq!(unchanged.request_id, before.request_id);
        assert_state_eq(unchanged.state, before.state);
        assert_eq!(unchanged.reason, before.reason);
        assert_eq!(unchanged.reserved, before.reserved);
        assert_eq!(unchanged.instructions_executed, before.instructions_executed);
        assert_eq!(unsafe { wait(handle, 1, &mut unchanged) }, OK);
        assert_eq!(unchanged.reason, 2);
        assert_eq!(unsafe { od_session_request_stop_v1(handle, 1) }, NO_ACTIVE_RUN);
        assert_eq!(unsafe { begin(handle, &run, &mut token) }, INVALID_REQUEST_ID);
        assert_eq!(unsafe { od_session_destroy_v1(handle) }, OK);
        drop(unsafe { OwnedFd::from_raw_fd(token.completion_read_fd) });
    }

    #[test]
    fn s3_38f_proxy_reuses_code_with_fresh_state_and_exclusive_lease() {
        let _guard = crate::tests::jit_test_lock();
        let handle = unsafe { create(&MOV_RAX_7) }.unwrap();
        let second = unsafe { create(&MOV_RAX_7) };
        let second_status = match second { Ok(second_handle) => { let _ = unsafe { od_session_destroy_v1(second_handle) }; OK }, Err(status) => status };
        assert_eq!(second_status, BUSY);
        for (id, input_rax, input_rcx) in [(1, 99, 22), (2, 42, 33)] {
            let mut run = request(&MOV_RAX_7, id, 0, input_rax, 1);
            run.initial_state.gpr[1] = input_rcx;
            let mut token = RunToken::default();
            assert_eq!(unsafe { begin(handle, &run, &mut token) }, OK);
            let mut result = RunResult::default();
            assert_eq!(unsafe { wait(handle, id, &mut result) }, OK);
            assert_eq!(result.state.gpr[0], 7);
            assert_eq!(result.state.gpr[1], input_rcx);
            assert_eq!(result.state.rip, MOV_RAX_7.len() as u64);
            drop(unsafe { OwnedFd::from_raw_fd(token.completion_read_fd) });
        }
        assert_eq!(unsafe { od_session_destroy_v1(handle) }, OK);
    }
}

