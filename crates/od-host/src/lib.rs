//! Host operating-system primitives and the engine's host ABI boundary.

use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutableMemoryError {
    UnsupportedPlatform,
    AllocationFailed,
}

/// A page of memory populated with generated code and mapped executable.
///
/// The object owns the mapping. Callers must ensure no generated-code pointer
/// is used after this value is dropped.
pub struct ExecutableMemory {
    ptr: NonNull<u8>,
    len: usize,
}

impl ExecutableMemory {
    pub fn new(code: &[u8]) -> Result<Self, ExecutableMemoryError> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use std::ffi::c_void;
            const PROT_READ: i32 = 1;
            const PROT_WRITE: i32 = 2;
            const PROT_EXEC: i32 = 4;
            const MAP_PRIVATE: i32 = 0x0002;
            const MAP_ANON: i32 = 0x1000;
            const MAP_JIT: i32 = 0x0800;

            unsafe extern "C" {
                fn mmap(
                    addr: *mut c_void,
                    len: usize,
                    prot: i32,
                    flags: i32,
                    fd: i32,
                    offset: i64,
                ) -> *mut c_void;
                fn pthread_jit_write_protect_np(enabled: i32);
                fn sys_icache_invalidate(start: *mut c_void, len: usize);
            }

            let len = code.len().max(4);
            let raw = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    len,
                    PROT_READ | PROT_WRITE | PROT_EXEC,
                    MAP_PRIVATE | MAP_ANON | MAP_JIT,
                    -1,
                    0,
                )
            };
            if raw as isize == -1 {
                return Err(ExecutableMemoryError::AllocationFailed);
            }
            let ptr =
                NonNull::new(raw.cast::<u8>()).ok_or(ExecutableMemoryError::AllocationFailed)?;
            unsafe {
                pthread_jit_write_protect_np(0);
                std::ptr::copy_nonoverlapping(code.as_ptr(), ptr.as_ptr(), code.len());
                pthread_jit_write_protect_np(1);
                sys_icache_invalidate(ptr.as_ptr().cast(), code.len());
            }
            return Ok(Self { ptr, len });
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = code;
            Err(ExecutableMemoryError::UnsupportedPlatform)
        }
    }

    /// Returns the start of the owned mapping. The pointer remains valid only
    /// while this `ExecutableMemory` value is alive.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        use std::ffi::c_void;
        unsafe extern "C" {
            fn munmap(addr: *mut c_void, len: usize) -> i32;
        }
        unsafe {
            munmap(self.ptr.as_ptr().cast(), self.len);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FragmentExecutionError {
    Jit(od_jit::JitError),
    Memory(ExecutableMemoryError),
}

/// Owns published executable code for repeated execution of one validated
/// x86 fragment. The mapping stays alive for every call to `execute`.
pub struct CompiledFragment {
    memory: ExecutableMemory,
}

impl CompiledFragment {
    /// Decodes, lowers, and publishes a fragment in executable host memory.
    pub fn compile(code: &[u8]) -> Result<Self, FragmentExecutionError> {
        let output = od_jit::compile(code).map_err(FragmentExecutionError::Jit)?;
        let native_code = output
            .host_code
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect::<Vec<_>>();
        let memory = ExecutableMemory::new(&native_code).map_err(FragmentExecutionError::Memory)?;
        Ok(Self { memory })
    }

    /// Executes the published fragment while it remains owned by this value.
    pub fn execute(
        &self,
        state: &mut od_isa::CpuState,
        budget: u64,
    ) -> Result<od_isa::ExecutionResult, FragmentExecutionError> {
        let stop_requested = AtomicU8::new(0);
        self.execute_with_stop(state, budget, &stop_requested)
    }

    /// Executes the fragment with a caller-owned cooperative-stop flag.
    pub fn execute_with_stop(
        &self,
        state: &mut od_isa::CpuState,
        budget: u64,
        stop_requested: &AtomicU8,
    ) -> Result<od_isa::ExecutionResult, FragmentExecutionError> {
        // SAFETY: `self.memory` owns the live mapping containing the compiled
        // output, and remains alive for the duration of this call. The stop
        // byte is an aligned AtomicU8 borrowed for the same duration.
        unsafe {
            od_jit::execute_at(
                self.memory.as_ptr(),
                state,
                budget,
                (stop_requested as *const AtomicU8).cast::<u8>(),
            )
        }
        .map_err(FragmentExecutionError::Jit)
    }
}

/// Compiles and runs the restricted x86 fragment through the host JIT memory
/// backend. This is a headless prototype entry point, not a VM host ABI.
pub fn execute_x86_fragment(
    code: &[u8],
    state: &mut od_isa::CpuState,
    budget: u64,
) -> Result<od_isa::ExecutionResult, FragmentExecutionError> {
    let stop_requested = AtomicU8::new(0);
    execute_x86_fragment_with_stop(code, state, budget, &stop_requested)
}

pub fn execute_x86_fragment_with_stop(
    code: &[u8],
    state: &mut od_isa::CpuState,
    budget: u64,
    stop_requested: &AtomicU8,
) -> Result<od_isa::ExecutionResult, FragmentExecutionError> {
    CompiledFragment::compile(code)?.execute_with_stop(state, budget, stop_requested)
}

pub const ABI_VERSION_V0: u32 = 0x0000_0001;
pub const STATUS_OK: u32 = 0;
pub const STATUS_INVALID_ARGUMENT: u32 = 1;
pub const STATUS_INVALID_CODE: u32 = 2;
pub const STATUS_BUSY: u32 = 3;
pub const STATUS_UNSUPPORTED_HOST: u32 = 4;
pub const STATUS_ALLOCATION_FAILED: u32 = 5;
pub const STATUS_INVALID_STATE: u32 = 6;

const MAX_FRAGMENT_LENGTH: usize = 4096;

/// Stable C representation for the S0 fragment register state.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OdCpuStateV0 {
    pub gpr: [u64; 4],
    pub rip: u64,
    pub rflags: u64,
}

/// Stable C result. `reason` uses 0=code end, 1=budget, 2=stop request.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OdExecutionResultV0 {
    pub state: OdCpuStateV0,
    pub reason: u32,
    pub reserved: u32,
    pub instructions_executed: u64,
}

struct FragmentInstance {
    code: Vec<u8>,
    running: AtomicBool,
    stop_requested: AtomicU8,
    lifecycle: Mutex<()>,
}

/// Returns the version of this public host ABI (major in bits 31..16).
#[unsafe(no_mangle)]
pub extern "C" fn od_host_abi_version() -> u32 {
    ABI_VERSION_V0
}

/// Creates an immutable S0 fragment handle by copying and validating `code`.
/// The caller owns the returned handle and releases it with
/// `od_fragment_destroy`. The input buffer may be released after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_fragment_create(
    code: *const u8,
    code_length: usize,
    out_handle: *mut *mut std::ffi::c_void,
) -> u32 {
    if out_handle.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    unsafe {
        *out_handle = std::ptr::null_mut();
    }
    if code_length > MAX_FRAGMENT_LENGTH || (code.is_null() && code_length != 0) {
        return STATUS_INVALID_ARGUMENT;
    }
    let bytes = if code_length == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(code, code_length) }.to_vec()
    };
    if od_isa::decode(&bytes).is_err() {
        return STATUS_INVALID_CODE;
    }
    let instance = Box::new(FragmentInstance {
        code: bytes,
        running: AtomicBool::new(false),
        stop_requested: AtomicU8::new(0),
        lifecycle: Mutex::new(()),
    });
    unsafe {
        *out_handle = Box::into_raw(instance).cast::<std::ffi::c_void>();
    }
    STATUS_OK
}

/// Executes the fragment synchronously. `state` is copied in and the complete
/// result is copied to `out_result`. `od_fragment_request_stop` may be called
/// concurrently from another thread; generated code checks it at each guest
/// instruction boundary. `destroy` and concurrent executions are rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_fragment_execute(
    handle: *mut std::ffi::c_void,
    state: *const OdCpuStateV0,
    budget: u64,
    out_result: *mut OdExecutionResultV0,
) -> u32 {
    if handle.is_null() || state.is_null() || out_result.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    let instance = unsafe { &*handle.cast::<FragmentInstance>() };
    let start_guard = match instance.lifecycle.lock() {
        Ok(guard) => guard,
        Err(_) => return STATUS_INVALID_STATE,
    };
    if instance.running.load(Ordering::Acquire) {
        return STATUS_BUSY;
    }
    instance.running.store(true, Ordering::Release);
    drop(start_guard);

    let input = unsafe { *state };
    let rip = match usize::try_from(input.rip) {
        Ok(rip) => rip,
        Err(_) => {
            if let Ok(_guard) = instance.lifecycle.lock() {
                instance.stop_requested.store(0, Ordering::Release);
                instance.running.store(false, Ordering::Release);
            } else {
                instance.stop_requested.store(0, Ordering::Release);
                instance.running.store(false, Ordering::Release);
            }
            return STATUS_INVALID_ARGUMENT;
        }
    };
    let mut cpu_state = od_isa::CpuState {
        gpr: input.gpr,
        rip,
        rflags: input.rflags,
    };
    let execution = execute_x86_fragment_with_stop(
        &instance.code,
        &mut cpu_state,
        budget,
        &instance.stop_requested,
    );

    let finish_guard = match instance.lifecycle.lock() {
        Ok(guard) => guard,
        Err(_) => {
            instance.running.store(false, Ordering::Release);
            return STATUS_INVALID_STATE;
        }
    };
    instance.stop_requested.store(0, Ordering::Release);
    instance.running.store(false, Ordering::Release);
    drop(finish_guard);

    let result = match execution {
        Ok(result) => result,
        Err(FragmentExecutionError::Memory(ExecutableMemoryError::UnsupportedPlatform)) => {
            return STATUS_UNSUPPORTED_HOST;
        }
        Err(FragmentExecutionError::Memory(ExecutableMemoryError::AllocationFailed)) => {
            return STATUS_ALLOCATION_FAILED;
        }
        Err(FragmentExecutionError::Jit(od_jit::JitError::Decode(_))) => {
            return STATUS_INVALID_CODE;
        }
        Err(FragmentExecutionError::Jit(_)) => return STATUS_INVALID_STATE,
    };
    let reason = match result.reason {
        od_isa::ExitReason::ReachedCodeEnd => 0,
        od_isa::ExitReason::InstructionBudgetExhausted => 1,
        od_isa::ExitReason::StopRequested => 2,
    };
    unsafe {
        *out_result = OdExecutionResultV0 {
            state: OdCpuStateV0 {
                gpr: result.state.gpr,
                rip: result.state.rip as u64,
                rflags: result.state.rflags,
            },
            reason,
            reserved: 0,
            instructions_executed: result.instructions_executed,
        };
    }
    STATUS_OK
}

/// Requests cooperative stop. A request made before execution is retained and
/// stops the next run; a request during execution is observed at a guest
/// instruction boundary. The pending request is cleared when that run ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_fragment_request_stop(handle: *mut std::ffi::c_void) -> u32 {
    if handle.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    let instance = unsafe { &*handle.cast::<FragmentInstance>() };
    let _guard = match instance.lifecycle.lock() {
        Ok(guard) => guard,
        Err(_) => return STATUS_INVALID_STATE,
    };
    instance.stop_requested.store(1, Ordering::Release);
    STATUS_OK
}

/// Destroys an idle fragment. The caller must prevent new calls that use this
/// handle before destroying it; active execution returns `STATUS_BUSY`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_fragment_destroy(handle: *mut std::ffi::c_void) -> u32 {
    if handle.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    let instance = unsafe { &*handle.cast::<FragmentInstance>() };
    let guard = match instance.lifecycle.lock() {
        Ok(guard) => guard,
        Err(_) => return STATUS_INVALID_STATE,
    };
    if instance.running.load(Ordering::Acquire) {
        return STATUS_BUSY;
    }
    drop(guard);
    unsafe {
        drop(Box::from_raw(handle.cast::<FragmentInstance>()));
    }
    STATUS_OK
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_code_fault_is_confined_to_child_process() {
        const CHILD_MODE: &str = "OD_HOST_GENERATED_FAULT_CHILD";

        if std::env::var_os(CHILD_MODE).is_some() {
            let code = [0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0];
            let output = od_jit::compile(&code).unwrap();
            let native_code = output
                .host_code
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<_>>();
            let memory = ExecutableMemory::new(&native_code).unwrap();
            type Entry =
                unsafe extern "C" fn(*mut od_isa::CpuState, u64, *mut u64, *const u8) -> u32;
            let entry: Entry = unsafe { std::mem::transmute(memory.as_ptr()) };
            let stop_requested = AtomicU8::new(0);
            let mut instructions_executed = 0;

            // The invalid non-null state pointer violates the private JIT
            // entry contract. The generated prologue dereferences it; this
            // child process must take the resulting host fault.
            unsafe {
                entry(
                    1_usize as *mut od_isa::CpuState,
                    1,
                    &mut instructions_executed,
                    (&stop_requested as *const AtomicU8).cast::<u8>(),
                );
            }
            std::process::exit(99);
        }

        use std::os::unix::process::ExitStatusExt;
        use std::process::{Command, Stdio};

        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::generated_code_fault_is_confined_to_child_process",
            ])
            .env(CHILD_MODE, "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(
            status.signal(),
            Some(11),
            "child should terminate from SIGSEGV"
        );
    }

    #[test]
    fn native_jit_matches_interpreter_for_s0_09_programs() {
        let programs: &[(&[u8], [u64; 4])] = &[
            (
                &[0x48, 0xB8, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
                [0, 0, 0, 0],
            ),
            (&[0x48, 0x03, 0xC1], [u64::MAX, 1, 0, 0]),
            (&[0x48, 0x2B, 0xC1, 0x75, 0xFB], [3, 1, 0, 0]),
            (&[0x48, 0x3B, 0xC1, 0x74, 0x00], [9, 9, 0, 0]),
        ];

        for (code, initial_gpr) in programs {
            let mut expected = od_isa::CpuState {
                gpr: *initial_gpr,
                ..od_isa::CpuState::default()
            };
            let expected_result = od_isa::interpret(code, &mut expected, 32).unwrap();
            let mut actual = od_isa::CpuState {
                gpr: *initial_gpr,
                ..od_isa::CpuState::default()
            };
            let actual_result = execute_x86_fragment(code, &mut actual, 32).unwrap();
            assert_eq!(actual_result, expected_result, "program {code:02x?}");
        }
    }

    #[test]
    fn native_jit_supports_manifest_rip_budget_and_invalid_rip() {
        let code = [
            0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0, 0x48, 0xB9, 2, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut state = od_isa::CpuState {
            rip: 10,
            ..od_isa::CpuState::default()
        };
        let result = execute_x86_fragment(&code, &mut state, 4).unwrap();
        assert_eq!(result.state.gpr, [0, 2, 0, 0]);
        assert_eq!(result.instructions_executed, 1);

        let mut invalid = od_isa::CpuState {
            rip: 1,
            ..od_isa::CpuState::default()
        };
        assert_eq!(
            execute_x86_fragment(&code, &mut invalid, 4),
            Err(FragmentExecutionError::Jit(od_jit::JitError::InvalidEntry))
        );

        let mut loop_state = od_isa::CpuState::default();
        let loop_result = execute_x86_fragment(&[0x75, 0xFE], &mut loop_state, 3).unwrap();
        assert_eq!(
            loop_result.reason,
            od_isa::ExitReason::InstructionBudgetExhausted
        );
        assert_eq!(loop_result.instructions_executed, 3);
        assert_eq!(loop_result.state.rip, 0);
    }

    #[test]
    fn compiled_fragment_reuses_published_code() {
        let code = [0x48, 0x2B, 0xC1, 0x75, 0xFB];
        let fragment = CompiledFragment::compile(&code).unwrap();
        let initial = od_isa::CpuState {
            gpr: [3, 1, 0, 0],
            ..od_isa::CpuState::default()
        };

        let mut first_state = initial.clone();
        let first = fragment.execute(&mut first_state, 32).unwrap();
        let mut second_state = initial;
        let second = fragment.execute(&mut second_state, 32).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.reason, od_isa::ExitReason::ReachedCodeEnd);
        assert_eq!(first.state.gpr[0], 0);
        assert_eq!(first.instructions_executed, 6);
    }

    #[test]
    fn c_abi_create_execute_stop_destroy_lifecycle() {
        use std::ffi::c_void;
        use std::thread;

        let code = [0x75, 0xFE]; // jnz to itself; initial ZF is clear.
        let mut raw_handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(od_host_abi_version(), ABI_VERSION_V0);
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut raw_handle) },
            STATUS_OK
        );
        assert!(!raw_handle.is_null());

        let handle_address = raw_handle as usize;
        let runner = thread::spawn(move || {
            let handle = handle_address as *mut c_void;
            let initial = OdCpuStateV0::default();
            let mut result = OdExecutionResultV0::default();
            let status = unsafe { od_fragment_execute(handle, &initial, u64::MAX, &mut result) };
            (status, result)
        });

        let instance = unsafe { &*raw_handle.cast::<FragmentInstance>() };
        while !instance.running.load(Ordering::Acquire) {
            thread::yield_now();
        }
        assert_eq!(unsafe { od_fragment_request_stop(raw_handle) }, STATUS_OK);
        let (status, result) = runner.join().unwrap();
        assert_eq!(status, STATUS_OK);
        assert_eq!(result.reason, 2);
        assert!(result.instructions_executed < u64::MAX);
        assert_eq!(unsafe { od_fragment_destroy(raw_handle) }, STATUS_OK);
    }

    #[test]
    fn c_abi_stop_requested_before_execute_is_honored_once() {
        use std::ffi::c_void;

        let code = [0x75, 0xFE]; // jnz to itself; initial ZF is clear.
        let mut raw_handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut raw_handle) },
            STATUS_OK
        );
        assert_eq!(unsafe { od_fragment_request_stop(raw_handle) }, STATUS_OK);

        let initial = OdCpuStateV0::default();
        let mut result = OdExecutionResultV0::default();
        assert_eq!(
            unsafe { od_fragment_execute(raw_handle, &initial, u64::MAX, &mut result) },
            STATUS_OK
        );
        assert_eq!(result.reason, 2);
        assert_eq!(result.instructions_executed, 0);

        assert_eq!(
            unsafe { od_fragment_execute(raw_handle, &initial, 1, &mut result) },
            STATUS_OK
        );
        assert_eq!(result.reason, 1);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(unsafe { od_fragment_destroy(raw_handle) }, STATUS_OK);
    }

    #[test]
    fn c_abi_rejects_rdtsc_without_profile_controls_or_source() {
        use std::ffi::c_void;

        let code = [0x0f, 0x31];
        let mut raw_handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut raw_handle) },
            STATUS_INVALID_CODE
        );
        assert!(raw_handle.is_null());
    }

    #[test]
    fn c_abi_rejects_rdmsr_and_wrmsr_without_an_explicit_msr_profile() {
        use std::ffi::c_void;

        for code in [&[0x0f, 0x32][..], &[0x0f, 0x30][..]] {
            let mut raw_handle: *mut c_void = std::ptr::null_mut();
            assert_eq!(
                unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut raw_handle) },
                STATUS_INVALID_CODE
            );
            assert!(raw_handle.is_null());
        }
    }

    #[test]
    fn c_abi_rejects_cli_without_its_local_long64_profile() {
        use std::ffi::c_void;

        let code = [0xfa];
        let mut raw_handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut raw_handle) },
            STATUS_INVALID_CODE
        );
        assert!(raw_handle.is_null());
    }

    #[test]
    fn cvttsd2si_remains_closed_to_fragment_and_c_abi_paths() {
        use std::ffi::c_void;

        let code = [0xf2, 0x0f, 0x2c, 0xc0];
        let mut state = od_isa::CpuState::default();
        assert!(execute_x86_fragment(&code, &mut state, 1).is_err());

        let mut handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut handle) },
            STATUS_INVALID_CODE
        );
        assert!(handle.is_null());
    }

    #[test]
    fn cvtss2si_m32_unmasked_remains_closed_to_fragment_and_c_abi_paths() {
        use std::ffi::c_void;

        let code = [0xf3, 0x0f, 0x2d, 0x00];
        let mut state = od_isa::CpuState::default();
        let before = state.clone();
        assert!(execute_x86_fragment(&code, &mut state, 1).is_err());
        assert_eq!(state, before);

        let mut handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut handle) },
            STATUS_INVALID_CODE
        );
        assert!(handle.is_null());
    }

    #[test]
    fn cvtsd2si_m64_r32_unmasked_remains_closed_to_fragment_and_c_abi_paths() {
        use std::ffi::c_void;

        let code = [0xf2, 0x0f, 0x2d, 0x00];
        let mut state = od_isa::CpuState::default();
        let before = state.clone();
        assert!(execute_x86_fragment(&code, &mut state, 1).is_err());
        assert_eq!(state, before);

        let mut handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut handle) },
            STATUS_INVALID_CODE
        );
        assert!(handle.is_null());
    }

    #[test]
    fn cvttss2si_m32_r32_unmasked_remains_closed_to_fragment_and_c_abi_paths() {
        use std::ffi::c_void;

        let code = [0xf3, 0x0f, 0x2c, 0x00];
        let mut state = od_isa::CpuState::default();
        let before = state.clone();
        assert!(execute_x86_fragment(&code, &mut state, 1).is_err());
        assert_eq!(state, before);

        let mut handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut handle) },
            STATUS_INVALID_CODE
        );
        assert!(handle.is_null());
    }
}
