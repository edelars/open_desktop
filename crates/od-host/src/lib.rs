//! Host operating-system primitives and the engine's host ABI boundary.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutableMemoryError {
    UnsupportedPlatform,
    AllocationFailed,
    MappingAlreadyLive,
}

/// A fully initialized, immutable owner of a published native-code mapping.
///
/// On macOS ARM64 this owns the process's single MAP_JIT region. A second
/// simultaneous publication is rejected until this value is dropped. The
/// pointer is only valid while this owner remains alive.
pub struct PublishedCode {
    region: MappedRegion,
}

/// Compatibility name for the published executable mapping API.
pub type ExecutableMemory = PublishedCode;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
static MAP_JIT_REGION_LIVE: AtomicBool = AtomicBool::new(false);

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct MapJitLease;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl MapJitLease {
    fn acquire() -> Result<Self, ExecutableMemoryError> {
        MAP_JIT_REGION_LIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| ExecutableMemoryError::MappingAlreadyLive)
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Drop for MapJitLease {
    fn drop(&mut self) {
        MAP_JIT_REGION_LIVE.store(false, Ordering::Release);
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct MappedRegion {
    ptr: NonNull<u8>,
    len: usize,
    _lease: MapJitLease,
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
struct MappedRegion;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Drop for MappedRegion {
    fn drop(&mut self) {
        use std::ffi::c_void;
        unsafe extern "C" {
            fn munmap(addr: *mut c_void, len: usize) -> i32;
        }
        unsafe {
            // The mapping and its lease are both owned by this value. A failed
            // munmap cannot be reported from Drop; keep teardown best-effort.
            // SAFETY: `ptr` and `len` are the exact successful `mmap` result
            // kept alive by this owner and have not been unmapped earlier.
            let _ = munmap(self.ptr.as_ptr().cast(), self.len);
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct JitWriteAccess;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl JitWriteAccess {
    fn begin() -> Self {
        unsafe extern "C" {
            fn pthread_jit_write_protect_np(enabled: i32);
        }
        // SAFETY: this changes JIT write permission for only the current
        // thread; Drop restores it before the mapping is published.
        unsafe { pthread_jit_write_protect_np(0) };
        Self
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Drop for JitWriteAccess {
    fn drop(&mut self) {
        unsafe extern "C" {
            fn pthread_jit_write_protect_np(enabled: i32);
        }
        // SAFETY: restoring the current thread's JIT write protection is valid
        // on the same macOS ARM64 platform where `begin` disabled it.
        unsafe { pthread_jit_write_protect_np(1) };
    }
}

impl PublishedCode {
    /// Constructs code in a private MAP_JIT mapping, closes write access,
    /// synchronizes the instruction cache, and only then publishes an owner.
    ///
    /// This safe API only publishes bytes. Calling arbitrary bytes remains
    /// unsafe and requires a separately verified entry contract.
    pub fn publish(code: &[u8]) -> Result<Self, ExecutableMemoryError> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use std::ffi::c_void;
            const PROT_READ: i32 = 1;
            const PROT_WRITE: i32 = 2;
            const PROT_EXEC: i32 = 4;
            const MAP_PRIVATE: i32 = 0x0002;
            const MAP_ANON: i32 = 0x1000;
            const MAP_JIT: i32 = 0x0800;
            const MAP_FAILED: *mut c_void = -1_isize as *mut c_void;

            unsafe extern "C" {
                fn mmap(
                    addr: *mut c_void,
                    len: usize,
                    prot: i32,
                    flags: i32,
                    fd: i32,
                    offset: i64,
                ) -> *mut c_void;
                fn sys_icache_invalidate(start: *mut c_void, len: usize);
                fn munmap(addr: *mut c_void, len: usize) -> i32;
            }

            let lease = MapJitLease::acquire()?;
            let len = code.len().max(4);
            // SAFETY: the arguments request one anonymous private MAP_JIT
            // mapping; `len` is nonzero, fd is -1, and offset is zero.
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
            if raw == MAP_FAILED {
                return Err(ExecutableMemoryError::AllocationFailed);
            }
            let Some(ptr) = NonNull::new(raw.cast::<u8>()) else {
                // mmap succeeded but returned an address the public non-null
                // pointer contract cannot represent. Release it immediately.
                // SAFETY: this is the successful raw mapping above with the
                // exact length passed to mmap.
                unsafe {
                    let _ = munmap(raw, len);
                }
                return Err(ExecutableMemoryError::AllocationFailed);
            };
            let region = MappedRegion {
                ptr,
                len,
                _lease: lease,
            };
            unsafe {
                let write_access = JitWriteAccess::begin();
                // SAFETY: `region` owns a mapping of at least `code.len()`
                // bytes, the immutable source slice cannot alias the fresh
                // mapping, and the current thread has write access enabled.
                std::ptr::copy_nonoverlapping(code.as_ptr(), ptr.as_ptr(), code.len());
                drop(write_access);
                // SAFETY: this is the just-written range in the live mapping;
                // invalidating it is required before its entry becomes visible.
                sys_icache_invalidate(ptr.as_ptr().cast(), code.len());
            }
            return Ok(Self { region });
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = code;
            Err(ExecutableMemoryError::UnsupportedPlatform)
        }
    }

    /// Legacy constructor retained for existing `ExecutableMemory` callers.
    pub fn new(code: &[u8]) -> Result<Self, ExecutableMemoryError> {
        Self::publish(code)
    }

    /// Returns the start of the owned mapping. The pointer remains valid only
    /// while this `PublishedCode` value is alive. Invoking it is unsafe.
    pub fn as_ptr(&self) -> *const u8 {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.region.ptr.as_ptr()
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            std::ptr::null()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FragmentExecutionError {
    Jit(od_jit::JitError),
    Memory(ExecutableMemoryError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockEntryError {
    HelperContractFailure,
    InvalidExitTag(u32),
}

/// Decodes a raw generated-entry result before a caller inspects context
/// outputs. Helper failures use a host-only sentinel, never a guest exit tag.
pub fn decode_block_entry_result(
    raw: u32,
) -> Result<od_jit::runtime::RuntimeExitTag, BlockEntryError> {
    if raw == od_jit::runtime::RAW_HELPER_CONTRACT_FAILURE_V1 {
        return Err(BlockEntryError::HelperContractFailure);
    }
    od_jit::runtime::RuntimeExitTag::try_from(raw).map_err(BlockEntryError::InvalidExitTag)
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
        Err(FragmentExecutionError::Memory(ExecutableMemoryError::MappingAlreadyLive)) => {
            return STATUS_BUSY;
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
    use od_jit::arm64::{
        AddressBase, Condition, Encoder, PairIndexMode, WReg, XReg, XRegOrSp, XRegOrZero,
    };
    use od_jit::runtime::{
        ArithmeticFlagsHelper, BlockEntry, HelperOperationTag, RAW_HELPER_CONTRACT_FAILURE_V1,
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET, RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET,
        RuntimeContextV1, RuntimeExitTag,
    };
    use std::sync::atomic::{AtomicU32, AtomicUsize};

    static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn jit_test_lock() -> std::sync::MutexGuard<'static, ()> {
        JIT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    const PROBE_HELPER_SENTINEL: u64 = 0xD15E_A5E0_CAFE_BEEF;
    const PROBE_REGISTER_MISMATCH: u64 = 0xA100;
    static PROBE_HELPER_STATUS: AtomicU32 = AtomicU32::new(0);
    static PROBE_NESTED_CALLS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct ProbeObservation {
        operation: u32,
        lhs: u64,
        rhs: u64,
        flags: u64,
        output_address: usize,
    }

    static PROBE_OBSERVATION: Mutex<Option<ProbeObservation>> = Mutex::new(None);

    extern "C" fn abi_probe_nested(value: u64) -> u64 {
        PROBE_NESTED_CALLS.fetch_add(1, Ordering::Relaxed);
        value.rotate_left(17) ^ 0x55AA_1234_DEAD_BEEF
    }

    unsafe extern "C" fn abi_probe_helper(
        operation: u32,
        lhs: u64,
        rhs: u64,
        flags: u64,
        output: *mut u64,
    ) -> u32 {
        let status = PROBE_HELPER_STATUS.load(Ordering::Relaxed);
        let nested = abi_probe_nested(lhs ^ rhs ^ flags);
        let Ok(mut observation) = PROBE_OBSERVATION.try_lock() else {
            return 2;
        };
        *observation = Some(ProbeObservation {
            operation,
            lhs,
            rhs,
            flags,
            output_address: output as usize,
        });
        if output.is_null() {
            return 2;
        }
        // Deliberately write even for error statuses. The generated caller
        // must ignore staged output unless helper status is Success.
        // SAFETY: the generated probe passes an aligned 8-byte local stack slot.
        unsafe { output.write(nested) };

        // Model the AAPCS64 volatile general-register clobbers used by a
        // nested host call. x18 remains reserved and is intentionally untouched.
        unsafe {
            std::arch::asm!(
                "mov x1, xzr",
                "mov x2, xzr",
                "mov x3, xzr",
                "mov x4, xzr",
                "mov x5, xzr",
                "mov x6, xzr",
                "mov x7, xzr",
                "mov x8, xzr",
                "mov x9, xzr",
                "mov x10, xzr",
                "mov x11, xzr",
                "mov x12, xzr",
                "mov x13, xzr",
                "mov x14, xzr",
                "mov x15, xzr",
                "mov x16, xzr",
                "mov x17, xzr",
                lateout("x1") _,
                lateout("x2") _,
                lateout("x3") _,
                lateout("x4") _,
                lateout("x5") _,
                lateout("x6") _,
                lateout("x7") _,
                lateout("x8") _,
                lateout("x9") _,
                lateout("x10") _,
                lateout("x11") _,
                lateout("x12") _,
                lateout("x13") _,
                lateout("x14") _,
                lateout("x15") _,
                lateout("x16") _,
                lateout("x17") _,
                options(nostack),
            );
        }
        status
    }

    fn xr(number: u8) -> XReg {
        XReg::new(number).unwrap()
    }

    fn emit_u64(encoder: &mut Encoder, register: u8, value: u64) {
        let destination = xr(register);
        encoder.movz_x(destination, value as u16, 0).unwrap();
        for halfword in 1..4 {
            encoder
                .movk_x(
                    destination,
                    (value >> (halfword * 16)) as u16,
                    halfword as u8,
                )
                .unwrap();
        }
    }

    fn emit_pair_push(encoder: &mut Encoder, first: u8, second: u8) {
        encoder
            .stp_x(
                xr(first),
                xr(second),
                AddressBase::Sp,
                -16,
                PairIndexMode::PreIndex,
            )
            .unwrap();
    }

    fn emit_pair_pop(encoder: &mut Encoder, first: u8, second: u8) {
        encoder
            .ldp_x(
                xr(first),
                xr(second),
                AddressBase::Sp,
                16,
                PairIndexMode::PostIndex,
            )
            .unwrap();
    }

    /// Emits an ABI canary wrapper followed by a BlockEntry helper probe.
    /// The wrapper is test-only and has its own AAPCS64 frame before it seeds
    /// x19-x28. The entry under test must preserve x19-x29 and x30.
    fn emit_runtime_abi_probe(helper: ArithmeticFlagsHelper) -> Vec<u8> {
        let mut encoder = Encoder::new();
        let entry = encoder.new_label().unwrap();
        let wrapper_epilogue = encoder.new_label().unwrap();
        let entry_epilogue = encoder.new_label().unwrap();

        // Test wrapper frame chain and save caller's callee-saved GPRs.
        emit_pair_push(&mut encoder, 29, 30);
        encoder
            .adds_imm_x(XRegOrZero::Reg(xr(29)), XRegOrSp::Sp, 0)
            .unwrap();
        for first in [19, 21, 23, 25, 27] {
            emit_pair_push(&mut encoder, first, first + 1);
        }
        // Reserve one aligned slot containing the expected wrapper x29.
        emit_pair_push(&mut encoder, 11, 12);
        encoder.mov_x(xr(16), XRegOrZero::Reg(xr(29)));
        encoder
            .str_x(XRegOrZero::Reg(xr(16)), AddressBase::Sp, 0)
            .unwrap();
        for first in [19, 21, 23, 25, 27] {
            for register in [first, first + 1] {
                encoder
                    .movz_x(xr(register), u16::from(register) + 0x100, 0)
                    .unwrap();
            }
        }
        encoder.bl(entry).unwrap();

        // Compare each seeded callee-saved register. A mismatch returns a
        // separate test sentinel; a passing wrapper preserves the entry tag.
        for first in [19, 21, 23, 25, 27] {
            for register in [first, first + 1] {
                let next = encoder.new_label().unwrap();
                encoder
                    .subs_imm_x(
                        XRegOrZero::Zero,
                        XRegOrSp::Reg(xr(register)),
                        u16::from(register) + 0x100,
                    )
                    .unwrap();
                encoder.b_cond(Condition::Equal, next).unwrap();
                emit_u64(&mut encoder, 0, PROBE_REGISTER_MISMATCH);
                encoder.b(wrapper_epilogue).unwrap();
                encoder.bind(next).unwrap();
            }
        }
        let x29_ok = encoder.new_label().unwrap();
        encoder
            .ldr_x(XRegOrZero::Reg(xr(16)), AddressBase::Sp, 0)
            .unwrap();
        encoder.subs_x(XRegOrZero::Zero, xr(29), XRegOrZero::Reg(xr(16)));
        encoder.b_cond(Condition::Equal, x29_ok).unwrap();
        emit_u64(&mut encoder, 0, PROBE_REGISTER_MISMATCH);
        encoder.bind(x29_ok).unwrap();
        encoder.b(wrapper_epilogue).unwrap();

        encoder.bind(wrapper_epilogue).unwrap();
        emit_pair_pop(&mut encoder, 11, 12);
        for first in [27, 25, 23, 21, 19] {
            emit_pair_pop(&mut encoder, first, first + 1);
        }
        emit_pair_pop(&mut encoder, 29, 30);
        encoder.ret(xr(30));

        encoder.bind(entry).unwrap();
        // Generated entry frame record and saves for every changed x19-x29.
        emit_pair_push(&mut encoder, 29, 30);
        encoder
            .adds_imm_x(XRegOrZero::Reg(xr(29)), XRegOrSp::Sp, 0)
            .unwrap();
        for first in [19, 21, 23, 25, 27] {
            emit_pair_push(&mut encoder, first, first + 1);
        }
        encoder.mov_x(xr(19), XRegOrZero::Reg(xr(0)));
        // Stage helper output in aligned local storage. The context isn't
        // updated or read back until helper status has been accepted.
        emit_pair_push(&mut encoder, 16, 17);
        emit_u64(&mut encoder, 16, PROBE_HELPER_SENTINEL);
        encoder
            .str_x(XRegOrZero::Reg(xr(16)), AddressBase::Sp, 0)
            .unwrap();

        // Fixed probe inputs make no claim about x86 arithmetic semantics.
        encoder
            .movz_x(xr(0), HelperOperationTag::Add as u16, 0)
            .unwrap();
        emit_u64(&mut encoder, 1, 0x1122_3344_5566_7788);
        emit_u64(&mut encoder, 2, 0x8877_6655_4433_2211);
        emit_u64(&mut encoder, 3, 0x0123_4567_89AB_CDEF);
        encoder
            .adds_imm_x(XRegOrZero::Reg(xr(4)), XRegOrSp::Sp, 0)
            .unwrap();
        emit_u64(&mut encoder, 16, helper as usize as u64);
        encoder.blr(xr(16));
        let success = encoder.new_label().unwrap();
        encoder.cbz_w(WReg::new(0).unwrap(), success).unwrap();

        emit_u64(&mut encoder, 0, u64::from(RAW_HELPER_CONTRACT_FAILURE_V1));
        encoder.b(entry_epilogue).unwrap();

        encoder.bind(success).unwrap();
        encoder
            .ldr_x(XRegOrZero::Reg(xr(20)), AddressBase::Sp, 0)
            .unwrap();
        encoder
            .str_x(
                XRegOrZero::Reg(xr(20)),
                AddressBase::Reg(xr(19)),
                RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
            )
            .unwrap();
        encoder.movz_x(xr(20), 1, 0).unwrap();
        encoder
            .str_x(
                XRegOrZero::Reg(xr(20)),
                AddressBase::Reg(xr(19)),
                RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
            )
            .unwrap();
        encoder
            .movz_x(xr(0), RuntimeExitTag::BlockComplete as u16, 0)
            .unwrap();

        // Entry epilogue uses the same saved frame and restores LR/SP.
        encoder.bind(entry_epilogue).unwrap();
        emit_pair_pop(&mut encoder, 16, 17);
        for first in [27, 25, 23, 21, 19] {
            emit_pair_pop(&mut encoder, first, first + 1);
        }
        emit_pair_pop(&mut encoder, 29, 30);
        encoder.ret(xr(30));

        encoder.finish().unwrap().to_le_bytes().unwrap()
    }

    fn call_probe_with_simd_canaries(
        entry: BlockEntry,
        context: &mut RuntimeContextV1,
    ) -> (u32, [u64; 8]) {
        let mut d8 = 0xD008;
        let mut d9 = 0xD009;
        let mut d10 = 0xD00A;
        let mut d11 = 0xD00B;
        let mut d12 = 0xD00C;
        let mut d13 = 0xD00D;
        let mut d14 = 0xD00E;
        let mut d15 = 0xD00F;
        let context_address = context as *mut RuntimeContextV1 as usize as u64;
        let entry_address = entry as usize as u64;
        let raw_result: u64;

        // SAFETY: `entry` points into a live PublishedCode made from the
        // checked Encoder probe above, has the BlockEntry ABI, receives a
        // valid mutable context, and the mapping owner remains alive here.
        unsafe {
            std::arch::asm!(
                "blr x16",
                inlateout("x0") context_address => raw_result,
                inlateout("x16") entry_address => _,
                inout("v8") d8 => d8,
                inout("v9") d9 => d9,
                inout("v10") d10 => d10,
                inout("v11") d11 => d11,
                inout("v12") d12 => d12,
                inout("v13") d13 => d13,
                inout("v14") d14 => d14,
                inout("v15") d15 => d15,
                clobber_abi("C"),
            );
        }
        (raw_result as u32, [d8, d9, d10, d11, d12, d13, d14, d15])
    }

    fn make_probe_context<'a>(
        state: &'a mut od_isa::LongModeState,
        stop: &'a AtomicU8,
    ) -> RuntimeContextV1 {
        RuntimeContextV1 {
            state,
            initial_budget: 17,
            stop_rip_enabled: 0,
            reserved: 0,
            stop_rip: 0,
            stop_requested: (stop as *const AtomicU8).cast::<u8>(),
            remaining_budget: PROBE_HELPER_SENTINEL,
            retired_instructions: 0xBADD_CAFE_1234_5678,
        }
    }

    fn probe_state() -> od_isa::LongModeState {
        let mut state = od_isa::LongModeState::default();
        state.gpr = [
            0x1001, 0x2002, 0x3003, 0x4004, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
        ];
        state.rip = 0x1234_5678;
        state.rflags = 0x8D7;
        state
    }

    fn expected_probe_output() -> u64 {
        (0x1122_3344_5566_7788_u64 ^ 0x8877_6655_4433_2211_u64 ^ 0x0123_4567_89AB_CDEF_u64)
            .rotate_left(17)
            ^ 0x55AA_1234_DEAD_BEEF
    }

    #[test]
    fn generated_entry_obeys_aapcs_and_nested_helper_abi() {
        let _guard = jit_test_lock();
        PROBE_HELPER_STATUS.store(0, Ordering::Relaxed);
        PROBE_NESTED_CALLS.store(0, Ordering::Relaxed);
        *PROBE_OBSERVATION.lock().unwrap() = None;

        let code = emit_runtime_abi_probe(abi_probe_helper);
        let published = PublishedCode::publish(&code).unwrap();
        // SAFETY: the bytes come from the test-only typed Encoder probe and
        // start with its AAPCS64 BlockEntry-compatible canary wrapper.
        let entry: BlockEntry = unsafe { std::mem::transmute(published.as_ptr()) };

        let mut state = probe_state();
        let original_state = state.clone();
        let stop = AtomicU8::new(0);
        let mut context = make_probe_context(&mut state, &stop);
        let context_output_address = std::ptr::addr_of!(context.remaining_budget) as usize;
        let (raw, simd_values) = call_probe_with_simd_canaries(entry, &mut context);

        assert_eq!(raw, RuntimeExitTag::BlockComplete as u32);
        assert_eq!(
            decode_block_entry_result(raw),
            Ok(RuntimeExitTag::BlockComplete)
        );
        assert_eq!(
            simd_values,
            [
                0xD008, 0xD009, 0xD00A, 0xD00B, 0xD00C, 0xD00D, 0xD00E, 0xD00F
            ]
        );
        assert_eq!(context.remaining_budget, expected_probe_output());
        assert_eq!(context.retired_instructions, 1);
        assert_eq!(state, original_state);
        assert_eq!(PROBE_NESTED_CALLS.load(Ordering::Relaxed), 1);
        let observation = PROBE_OBSERVATION.lock().unwrap().unwrap();
        assert_eq!(observation.operation, HelperOperationTag::Add as u32);
        assert_eq!(observation.lhs, 0x1122_3344_5566_7788);
        assert_eq!(observation.rhs, 0x8877_6655_4433_2211);
        assert_eq!(observation.flags, 0x0123_4567_89AB_CDEF);
        assert_eq!(observation.output_address % 16, 0);
        assert_ne!(observation.output_address, context_output_address);
    }

    #[test]
    fn helper_failures_return_host_sentinel_without_guest_commit() {
        let _guard = jit_test_lock();
        let code = emit_runtime_abi_probe(abi_probe_helper);
        let published = PublishedCode::publish(&code).unwrap();
        // SAFETY: see the success-path test; this is the same checked probe.
        let entry: BlockEntry = unsafe { std::mem::transmute(published.as_ptr()) };

        for status in [1, 2, 3, u32::MAX] {
            PROBE_HELPER_STATUS.store(status, Ordering::Relaxed);
            PROBE_NESTED_CALLS.store(0, Ordering::Relaxed);
            *PROBE_OBSERVATION.lock().unwrap() = None;

            let mut state = probe_state();
            let original_state = state.clone();
            let stop = AtomicU8::new(0);
            let mut context = make_probe_context(&mut state, &stop);
            let (raw, simd_values) = call_probe_with_simd_canaries(entry, &mut context);

            assert_eq!(raw, RAW_HELPER_CONTRACT_FAILURE_V1);
            assert_eq!(
                decode_block_entry_result(raw),
                Err(BlockEntryError::HelperContractFailure)
            );
            assert_eq!(
                RuntimeExitTag::try_from(raw),
                Err(RAW_HELPER_CONTRACT_FAILURE_V1)
            );
            assert_eq!(
                simd_values,
                [
                    0xD008, 0xD009, 0xD00A, 0xD00B, 0xD00C, 0xD00D, 0xD00E, 0xD00F
                ]
            );
            assert_eq!(context.remaining_budget, PROBE_HELPER_SENTINEL);
            assert_eq!(context.retired_instructions, 0xBADD_CAFE_1234_5678);
            assert_eq!(state, original_state);
            assert_eq!(PROBE_NESTED_CALLS.load(Ordering::Relaxed), 1);
            let observation = PROBE_OBSERVATION.lock().unwrap().unwrap();
            assert_eq!(observation.operation, HelperOperationTag::Add as u32);
            assert_eq!(observation.output_address % 16, 0);
            assert_ne!(
                observation.output_address,
                std::ptr::addr_of!(context.remaining_budget) as usize
            );
        }
    }

    #[test]
    fn host_entry_decoder_keeps_helper_failure_out_of_guest_exit_tags() {
        assert_eq!(
            decode_block_entry_result(RAW_HELPER_CONTRACT_FAILURE_V1),
            Err(BlockEntryError::HelperContractFailure)
        );
        assert_eq!(
            RuntimeExitTag::try_from(RAW_HELPER_CONTRACT_FAILURE_V1),
            Err(RAW_HELPER_CONTRACT_FAILURE_V1)
        );
        assert_eq!(
            decode_block_entry_result(6),
            Err(BlockEntryError::InvalidExitTag(6))
        );
    }

    #[test]
    fn generated_code_fault_is_confined_to_child_process() {
        let _guard = jit_test_lock();
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
        let _guard = jit_test_lock();
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
        let _guard = jit_test_lock();
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
        let _guard = jit_test_lock();
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
        let _guard = jit_test_lock();
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
        let _guard = jit_test_lock();
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
    fn publication_rejects_second_live_mapping_and_reuses_slot_after_drop() {
        let _guard = jit_test_lock();
        let first = PublishedCode::publish(&[0xC0, 0x03, 0x5F, 0xD6]).unwrap(); // ret
        assert!(!first.as_ptr().is_null());
        assert_eq!(
            PublishedCode::publish(&[0xC0, 0x03, 0x5F, 0xD6]).err(),
            Some(ExecutableMemoryError::MappingAlreadyLive)
        );

        drop(first);
        let second = PublishedCode::publish(&[0xC0, 0x03, 0x5F, 0xD6]).unwrap();
        assert!(!second.as_ptr().is_null());
    }

    #[test]
    fn c_abi_reports_live_process_mapping_as_busy() {
        let _guard = jit_test_lock();
        use std::ffi::c_void;

        let code = [0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0];
        let mut raw_handle: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            unsafe { od_fragment_create(code.as_ptr(), code.len(), &mut raw_handle) },
            STATUS_OK
        );

        let mapping = PublishedCode::publish(&[0xC0, 0x03, 0x5F, 0xD6]).unwrap();
        let initial = OdCpuStateV0::default();
        let mut result = OdExecutionResultV0::default();
        assert_eq!(
            unsafe { od_fragment_execute(raw_handle, &initial, 1, &mut result) },
            STATUS_BUSY
        );

        drop(mapping);
        assert_eq!(
            unsafe { od_fragment_execute(raw_handle, &initial, 1, &mut result) },
            STATUS_OK
        );
        assert_eq!(unsafe { od_fragment_destroy(raw_handle) }, STATUS_OK);
    }
}
