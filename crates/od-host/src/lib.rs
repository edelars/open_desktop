//! Host operating-system primitives and the engine's host ABI boundary.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutableMemoryError {
    UnsupportedPlatform,
    AllocationFailed,
    MappingAlreadyLive,
    InvalidBlockArtifact,
}

/// A fully initialized, immutable owner of a published native-code mapping.
///
/// On macOS ARM64 this owns the process's single MAP_JIT region. A second
/// simultaneous publication is rejected until this value is dropped. The
/// pointer is only valid while this owner remains alive.
pub struct PublishedCode {
    region: MappedRegion,
    block_metadata: Option<BlockMetadata>,
    memory_operation_metadata_v2: Option<MemoryOperationMetadataV2>,
}

struct BlockMetadata {
    pc_map: od_jit::statepoints::PcMap,
    entry_rip: u64,
    safe_entry: bool,
}

struct MemoryOperationMetadataV2 {
    entry_rip: u64,
    site: od_jit::runtime::MemorySiteV2,
    source_bytes: [u8; 8],
    source_len: u8,
    compile_flavor: od_jit::translation::CompileFlavor,
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

impl MappedRegion {
    fn allocate(len: usize) -> Result<Self, ExecutableMemoryError> {
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
                fn munmap(addr: *mut c_void, len: usize) -> i32;
            }

            let lease = MapJitLease::acquire()?;
            let len = len.max(4);
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
                // SAFETY: this is the successful raw mapping above with the
                // exact length passed to mmap.
                unsafe {
                    let _ = munmap(raw, len);
                }
                return Err(ExecutableMemoryError::AllocationFailed);
            };
            Ok(Self {
                ptr,
                len,
                _lease: lease,
            })
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = len;
            Err(ExecutableMemoryError::UnsupportedPlatform)
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn ptr(&self) -> NonNull<u8> {
        self.ptr
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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn write_mapping_bytes(destination: NonNull<u8>, bytes: &[u8]) {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn sys_icache_invalidate(start: *mut c_void, len: usize);
    }

    let write_access = JitWriteAccess::begin();
    // SAFETY: callers ensure `destination` covers `bytes.len()` writable bytes
    // in a live MAP_JIT region; the borrowed source cannot alias that region.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination.as_ptr(), bytes.len());
    }
    drop(write_access);
    // SAFETY: this is the just-written range in the live mapping; invalidating
    // it after restoring write protection is required before entry visibility.
    unsafe { sys_icache_invalidate(destination.as_ptr().cast(), bytes.len()) };
}

impl PublishedCode {
    /// Constructs code in a private MAP_JIT mapping, closes write access,
    /// synchronizes the instruction cache, and only then publishes an owner.
    ///
    /// This safe API only publishes bytes. Calling arbitrary bytes remains
    /// unsafe and requires a separately verified entry contract.
    pub fn publish(code: &[u8]) -> Result<Self, ExecutableMemoryError> {
        Self::publish_with_metadata(code, None, None)
    }

    /// Publishes a verifier-checked runtime block and keeps its immutable state
    /// map attached to the executable owner for the mapping's whole lifetime.
    pub fn publish_block(
        block: &od_jit::block_emitter::EmittedRuntimeBlock,
    ) -> Result<Self, ExecutableMemoryError> {
        let code = block
            .to_le_bytes()
            .map_err(|_| ExecutableMemoryError::AllocationFailed)?;
        if usize::try_from(block.pc_map().code_len()).ok() != Some(code.len()) {
            return Err(ExecutableMemoryError::AllocationFailed);
        }
        Self::publish_with_metadata(
            &code,
            Some(BlockMetadata {
                pc_map: block.pc_map().clone(),
                entry_rip: block.entry_rip(),
                safe_entry: block.safe_entry_provenance(),
            }),
            None,
        )
    }

    /// Publishes a compiler-issued one-instruction V2 checked-memory artifact.
    /// The source bytes, immutable site metadata, and V2 identity stay coupled
    /// to this owner for the full mapping lifetime.
    pub fn publish_memory_load_v2(
        block: &od_jit::memory_load_v2::MemoryLoadBlockV2,
    ) -> Result<Self, ExecutableMemoryError> {
        if block.site().operation() != od_jit::runtime::MemorySiteOperationV2::Load
            || block.compile_flavor() != od_jit::translation::CompileFlavor::S3_19bMemoryLoadV2
        {
            return Err(ExecutableMemoryError::InvalidBlockArtifact);
        }
        let code = block
            .code()
            .to_le_bytes()
            .map_err(|_| ExecutableMemoryError::AllocationFailed)?;
        Self::publish_with_metadata(
            &code,
            None,
            Some(MemoryOperationMetadataV2 {
                entry_rip: block.entry_rip(),
                site: *block.site(),
                source_bytes: block.source_bytes_storage().0,
                source_len: block.source_bytes_storage().1,
                compile_flavor: block.compile_flavor(),
            }),
        )
    }

    /// Publishes a compiler-issued one-instruction V2 checked-memory store.
    /// Publication alone does not authorize running it on an arbitrary backend.
    pub fn publish_memory_store_v2(
        block: &od_jit::memory_load_v2::MemoryStoreBlockV2,
    ) -> Result<Self, ExecutableMemoryError> {
        if block.site().operation() != od_jit::runtime::MemorySiteOperationV2::Store
            || block.compile_flavor() != od_jit::translation::CompileFlavor::S3_20bMemoryStoreV2
        {
            return Err(ExecutableMemoryError::InvalidBlockArtifact);
        }
        let code = block
            .code()
            .to_le_bytes()
            .map_err(|_| ExecutableMemoryError::AllocationFailed)?;
        Self::publish_with_metadata(
            &code,
            None,
            Some(MemoryOperationMetadataV2 {
                entry_rip: block.entry_rip(),
                site: *block.site(),
                source_bytes: block.source_bytes_storage().0,
                source_len: block.source_bytes_storage().1,
                compile_flavor: block.compile_flavor(),
            }),
        )
    }

    /// Execute one verified V2 MOV load with invocation-local memory ownership.
    /// This entry is intentionally separate from the V1 block executor.
    pub fn execute_memory_load_v2(
        &self,
        state: &mut od_isa::LongModeState,
        memory: &mut dyn od_isa::memory::GuestMemory,
        access_context: od_isa::memory::AccessContext,
        initial_budget: u64,
        stop_rip: Option<u64>,
        stop_requested: &std::sync::atomic::AtomicU8,
    ) -> Result<MemoryOperationExecutionResultV2, MemoryOperationExecutionErrorV2> {
        // SAFETY: this wrapper fixes the operation to Load, whose checked
        // helper performs no memory write and needs no private-RAM precondition.
        unsafe {
            self.execute_memory_operation_v2(
                state,
                memory,
                access_context,
                initial_budget,
                stop_rip,
                stop_requested,
                od_jit::runtime::MemorySiteOperationV2::Load,
            )
        }
    }

    /// Execute a verified V2 MOV store using only a backend whose ownership
    /// and write semantics satisfy the private synchronous RAM contract.
    ///
    /// # Safety
    /// The supplied backend must be private synchronous ordinary RAM. Its
    /// `GuestMemory::write` implementation must complete the full 8-byte write
    /// or leave RAM unchanged on error. The backing must have no MMIO, DMA,
    /// concurrent readers/writers, shared aliases, or delayed/asynchronous
    /// effects. It must never be executable memory or alias the cached guest
    /// source/code backing while this compiled block exists, and no transition
    /// to executable state may occur until the published code is dropped.
    /// The backend must not call back into this runtime or unwind through the
    /// generated code during the write.
    /// These are caller-owned integration guarantees; this entry cannot infer
    /// them from the `GuestMemory` trait object.
    pub unsafe fn execute_memory_store_v2(
        &self,
        state: &mut od_isa::LongModeState,
        memory: &mut dyn od_isa::memory::GuestMemory,
        access_context: od_isa::memory::AccessContext,
        initial_budget: u64,
        stop_rip: Option<u64>,
        stop_requested: &std::sync::atomic::AtomicU8,
    ) -> Result<MemoryOperationExecutionResultV2, MemoryOperationExecutionErrorV2> {
        // SAFETY: the caller upholds the private synchronous RAM conditions
        // documented above for the full duration of this store.
        unsafe {
            self.execute_memory_operation_v2(
                state,
                memory,
                access_context,
                initial_budget,
                stop_rip,
                stop_requested,
                od_jit::runtime::MemorySiteOperationV2::Store,
            )
        }
    }

    /// # Safety
    /// When `expected_operation` is Store, caller guarantees the private
    /// synchronous RAM requirements of `execute_memory_store_v2`.
    unsafe fn execute_memory_operation_v2(
        &self,
        state: &mut od_isa::LongModeState,
        memory: &mut dyn od_isa::memory::GuestMemory,
        access_context: od_isa::memory::AccessContext,
        initial_budget: u64,
        stop_rip: Option<u64>,
        stop_requested: &std::sync::atomic::AtomicU8,
        expected_operation: od_jit::runtime::MemorySiteOperationV2,
    ) -> Result<MemoryOperationExecutionResultV2, MemoryOperationExecutionErrorV2> {
        let metadata = self
            .memory_operation_metadata_v2
            .as_ref()
            .ok_or(MemoryOperationExecutionErrorV2::NotV2MemoryBlock)?;
        let source_len = usize::from(metadata.source_len);
        if !(3..=8).contains(&source_len) {
            return Err(MemoryOperationExecutionErrorV2::InvalidMetadata);
        }
        let source = &metadata.source_bytes[..source_len];
        if !metadata
            .site
            .matches_source_bytes(source, expected_operation)
            || metadata.site.operation() != expected_operation
            || metadata.site.guest_rip() != metadata.entry_rip
            || metadata.compile_flavor
                != match expected_operation {
                    od_jit::runtime::MemorySiteOperationV2::Load => {
                        od_jit::translation::CompileFlavor::S3_19bMemoryLoadV2
                    }
                    od_jit::runtime::MemorySiteOperationV2::Store => {
                        od_jit::translation::CompileFlavor::S3_20bMemoryStoreV2
                    }
                }
        {
            return Err(MemoryOperationExecutionErrorV2::InvalidMetadata);
        }
        if state.rip != metadata.entry_rip {
            return Err(MemoryOperationExecutionErrorV2::EntryRipMismatch {
                expected: metadata.entry_rip,
                actual: state.rip,
            });
        }

        let mut adapter = od_jit::memory_bridge::CheckedMemoryAdapter::new(memory, access_context);
        let sites = [metadata.site];
        let mut context = od_jit::runtime::RuntimeContextV2 {
            base: od_jit::runtime::RuntimeContextV1 {
                state,
                initial_budget,
                stop_rip_enabled: u32::from(stop_rip.is_some()),
                reserved: 0,
                stop_rip: stop_rip.unwrap_or(0),
                stop_requested: (stop_requested as *const std::sync::atomic::AtomicU8).cast(),
                remaining_budget: initial_budget,
                retired_instructions: 0,
            },
            memory_adapter: adapter.as_opaque_ptr(),
            memory_sites: sites.as_ptr(),
            memory_site_count: 1,
            last_memory_site: u32::MAX,
            last_memory_status: od_jit::memory_bridge::MEMORY_HELPER_INVALID_ARGUMENT,
            reserved: 0,
            staged_load_value: 0,
            memory_outcome: od_jit::memory_bridge::CheckedMemoryOutcomeV1::default(),
        };
        context.memory_outcome.status = od_jit::memory_bridge::MEMORY_HELPER_INVALID_ARGUMENT;

        // SAFETY: the V2 entry type and compiler-issued provenance are coupled
        // to this owner. Context, adapter, site table, state, and stop flag all
        // remain live for the synchronous native call.
        let entry: od_jit::runtime::BlockEntryV2 = unsafe { std::mem::transmute(self.as_ptr()) };
        // SAFETY: context points only to valid invocation-local V2 data.
        let raw = unsafe { entry(&mut context) };
        decode_memory_operation_v2_result(metadata, raw, &context)
    }

    fn publish_with_metadata(
        code: &[u8],
        block_metadata: Option<BlockMetadata>,
        memory_operation_metadata_v2: Option<MemoryOperationMetadataV2>,
    ) -> Result<Self, ExecutableMemoryError> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let region = MappedRegion::allocate(code.len())?;
            // SAFETY: the new region owns at least `code.len()` writable bytes.
            unsafe { write_mapping_bytes(region.ptr(), code) };
            return Ok(Self {
                region,
                block_metadata,
                memory_operation_metadata_v2,
            });
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = code;
            let _ = block_metadata;
            let _ = memory_operation_metadata_v2;
            Err(ExecutableMemoryError::UnsupportedPlatform)
        }
    }

    pub fn pc_map(&self) -> Option<&od_jit::statepoints::PcMap> {
        self.block_metadata
            .as_ref()
            .map(|metadata| &metadata.pc_map)
    }

    /// Invoke only a runtime entry emitted with the production helper and
    /// published together with its state map. The borrow of `self` keeps the
    /// mapping and metadata alive through native execution and result lookup.
    pub fn execute_block(
        &self,
        state: &mut od_isa::LongModeState,
        initial_budget: u64,
        stop_rip: Option<u64>,
        stop_requested: &std::sync::atomic::AtomicU8,
    ) -> Result<BlockExecutionResult, BlockExecutionError> {
        let metadata = self
            .block_metadata
            .as_ref()
            .ok_or(BlockExecutionError::NotRuntimeBlock)?;
        execute_block_at(
            self.as_ptr(),
            metadata,
            state,
            initial_budget,
            stop_rip,
            stop_requested,
        )
    }

    #[cfg(test)]
    fn decode_block_result(
        &self,
        raw: u32,
        state_rip: u64,
        remaining_budget: u64,
        retired_instructions: u64,
    ) -> Result<BlockExecutionResult, BlockExecutionError> {
        let metadata = self
            .block_metadata
            .as_ref()
            .ok_or(BlockExecutionError::NotRuntimeBlock)?;
        decode_block_result(
            metadata,
            raw,
            state_rip,
            remaining_budget,
            retired_instructions,
        )
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

fn execute_block_at(
    code_ptr: *const u8,
    metadata: &BlockMetadata,
    state: &mut od_isa::LongModeState,
    initial_budget: u64,
    stop_rip: Option<u64>,
    stop_requested: &std::sync::atomic::AtomicU8,
) -> Result<BlockExecutionResult, BlockExecutionError> {
    if !metadata.safe_entry {
        return Err(BlockExecutionError::UntrustedHelper);
    }
    if state.rip != metadata.entry_rip {
        return Err(BlockExecutionError::EntryRipMismatch {
            expected: metadata.entry_rip,
            actual: state.rip,
        });
    }

    let mut context = od_jit::runtime::RuntimeContextV1 {
        state,
        initial_budget,
        stop_rip_enabled: u32::from(stop_rip.is_some()),
        reserved: 0,
        stop_rip: stop_rip.unwrap_or(0),
        stop_requested: (stop_requested as *const std::sync::atomic::AtomicU8).cast(),
        remaining_budget: initial_budget,
        retired_instructions: 0,
    };
    // SAFETY: callers keep the owning PublishedCode/ArenaBlock alive for this
    // full call. Metadata only comes from a verifier-checked runtime emitter,
    // the production helper ABI is fixed, and all context references outlive
    // the generated entry.
    let entry: od_jit::runtime::BlockEntry = unsafe { std::mem::transmute(code_ptr) };
    // SAFETY: RuntimeContextV1 pointers above refer to live state and control.
    let raw = unsafe { entry(&mut context) };
    decode_block_result(
        metadata,
        raw,
        state.rip,
        context.remaining_budget,
        context.retired_instructions,
    )
}

fn decode_block_result(
    metadata: &BlockMetadata,
    raw: u32,
    state_rip: u64,
    remaining_budget: u64,
    retired_instructions: u64,
) -> Result<BlockExecutionResult, BlockExecutionError> {
    if raw == od_jit::runtime::RAW_HELPER_CONTRACT_FAILURE_V1 {
        if metadata.pc_map.has_helper_failure_site(state_rip) {
            return Err(BlockExecutionError::HelperContractFailure {
                guest_rip: state_rip,
                remaining_budget,
                retired_instructions,
            });
        }
        return Err(BlockExecutionError::UnknownHelperFailureSite { state_rip });
    }
    let exit = od_jit::runtime::RuntimeExitTag::try_from(raw)
        .map_err(BlockExecutionError::InvalidExitTag)?;
    Ok(BlockExecutionResult {
        exit,
        remaining_budget,
        retired_instructions,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryOperationExecutionExitV2 {
    Complete,
    BudgetExhausted,
    StopRequested,
    StopAddress,
    GuestFault(od_isa::memory::ArchitecturalFault),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryOperationExecutionResultV2 {
    pub exit: MemoryOperationExecutionExitV2,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryOperationMemoryErrorV2 {
    pub kind: u32,
    pub address: u64,
    pub length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryOperationExecutionErrorV2 {
    NotV2MemoryBlock,
    InvalidMetadata,
    EntryRipMismatch { expected: u64, actual: u64 },
    InvalidExitTag(u32),
    InvalidSite { site: u32 },
    InvalidOutcome,
    InvalidAccounting,
    MemoryError(MemoryOperationMemoryErrorV2),
    HelperBoundaryFailure { status: u32 },
}

/// Compatibility names retained for callers that use the load-only V2 API.
pub type MemoryLoadExecutionExitV2 = MemoryOperationExecutionExitV2;
pub type MemoryLoadExecutionResultV2 = MemoryOperationExecutionResultV2;
pub type MemoryLoadMemoryErrorV2 = MemoryOperationMemoryErrorV2;
pub type MemoryLoadExecutionErrorV2 = MemoryOperationExecutionErrorV2;

fn decode_memory_operation_v2_result(
    metadata: &MemoryOperationMetadataV2,
    raw: u32,
    context: &od_jit::runtime::RuntimeContextV2,
) -> Result<MemoryOperationExecutionResultV2, MemoryOperationExecutionErrorV2> {
    use od_jit::memory_bridge::{
        MEMORY_ERROR_NONE, MEMORY_HELPER_ARCHITECTURAL_FAULT, MEMORY_HELPER_INVALID_ARGUMENT,
        MEMORY_HELPER_MEMORY_ERROR, MEMORY_HELPER_SUCCESS,
    };
    let base = &context.base;
    // SAFETY: execute_memory_load_v2 keeps the canonical state alive for this
    // decoding step and stores its pointer only in this invocation context.
    let state_rip = unsafe { (*base.state).rip };
    let original_rip = metadata.entry_rip;
    let unchanged = || {
        state_rip == original_rip
            && base.remaining_budget == base.initial_budget
            && base.retired_instructions == 0
    };
    let result = match raw {
        0 => {
            if context.last_memory_site != 0
                || context.last_memory_status != MEMORY_HELPER_SUCCESS
                || context.memory_outcome.status != MEMORY_HELPER_SUCCESS
                || state_rip != metadata.site.next_rip()
                || base.initial_budget == 0
                || base.remaining_budget != base.initial_budget - 1
                || base.retired_instructions != 1
            {
                return Err(MemoryOperationExecutionErrorV2::InvalidAccounting);
            }
            MemoryOperationExecutionExitV2::Complete
        }
        1 => {
            if context.last_memory_site != u32::MAX
                || context.last_memory_status != MEMORY_HELPER_INVALID_ARGUMENT
                || !unchanged()
            {
                return Err(MemoryOperationExecutionErrorV2::InvalidAccounting);
            }
            MemoryOperationExecutionExitV2::BudgetExhausted
        }
        3 => {
            let before_instruction = context.last_memory_site == u32::MAX
                && context.last_memory_status == MEMORY_HELPER_INVALID_ARGUMENT
                && unchanged();
            let after_instruction = context.last_memory_site == 0
                && context.last_memory_status == MEMORY_HELPER_SUCCESS
                && state_rip == metadata.site.next_rip()
                && base.initial_budget > 0
                && base.remaining_budget == base.initial_budget - 1
                && base.retired_instructions == 1;
            if !before_instruction && !after_instruction {
                return Err(MemoryOperationExecutionErrorV2::InvalidAccounting);
            }
            MemoryOperationExecutionExitV2::StopRequested
        }
        4 => {
            if context.last_memory_site != u32::MAX
                || context.last_memory_status != MEMORY_HELPER_INVALID_ARGUMENT
                || !unchanged()
            {
                return Err(MemoryOperationExecutionErrorV2::InvalidAccounting);
            }
            MemoryOperationExecutionExitV2::StopAddress
        }
        od_jit::runtime::RUNTIME_EXIT_V2_MEMORY_OPERATION => {
            if context.last_memory_site != 0
                || context.memory_outcome.status != context.last_memory_status
            {
                return Err(MemoryOperationExecutionErrorV2::InvalidSite {
                    site: context.last_memory_site,
                });
            }
            if !unchanged() {
                return Err(MemoryOperationExecutionErrorV2::InvalidAccounting);
            }
            match context.last_memory_status {
                MEMORY_HELPER_ARCHITECTURAL_FAULT => {
                    let outcome = context.memory_outcome;
                    if outcome.error_kind != MEMORY_ERROR_NONE
                        || outcome.has_error_code > 1
                        || outcome.has_linear_address > 1
                        || outcome.fault_vector > u8::MAX.into()
                    {
                        return Err(MemoryOperationExecutionErrorV2::InvalidOutcome);
                    }
                    MemoryOperationExecutionExitV2::GuestFault(od_isa::memory::ArchitecturalFault {
                        vector: outcome.fault_vector as u8,
                        error_code: (outcome.has_error_code != 0).then_some(outcome.error_code),
                        linear_address: (outcome.has_linear_address != 0)
                            .then_some(outcome.linear_address),
                    })
                }
                MEMORY_HELPER_MEMORY_ERROR => {
                    let outcome = context.memory_outcome;
                    if !(od_jit::memory_bridge::MEMORY_ERROR_UNMAPPED
                        ..=od_jit::memory_bridge::MEMORY_ERROR_BACKEND)
                        .contains(&outcome.error_kind)
                    {
                        return Err(MemoryOperationExecutionErrorV2::InvalidOutcome);
                    }
                    return Err(MemoryOperationExecutionErrorV2::MemoryError(
                        MemoryOperationMemoryErrorV2 {
                            kind: outcome.error_kind,
                            address: outcome.error_address,
                            length: outcome.error_length,
                        },
                    ));
                }
                status => {
                    return Err(MemoryOperationExecutionErrorV2::HelperBoundaryFailure { status });
                }
            }
        }
        other => return Err(MemoryOperationExecutionErrorV2::InvalidExitTag(other)),
    };
    Ok(MemoryOperationExecutionResultV2 {
        exit: result,
        remaining_budget: base.remaining_budget,
        retired_instructions: base.retired_instructions,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockExecutionResult {
    pub exit: od_jit::runtime::RuntimeExitTag,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockExecutionError {
    NotRuntimeBlock,
    UntrustedHelper,
    EntryRipMismatch {
        expected: u64,
        actual: u64,
    },
    HelperContractFailure {
        guest_rip: u64,
        remaining_budget: u64,
        retired_instructions: u64,
    },
    UnknownHelperFailureSite {
        state_rip: u64,
    },
    InvalidExitTag(u32),
}

/// Production bridge used by the iterative `od-jit` dispatcher. Each call
/// publishes and executes one block, then drops its mapping before returning.
#[derive(Default)]
pub struct PublishedBlockExecutor;

impl od_jit::dispatch::BlockExecutor for PublishedBlockExecutor {
    type Error = PublishedBlockExecutionError;

    fn execute_one_block(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut od_isa::LongModeState,
        incoming_budget: u64,
    ) -> Result<od_jit::dispatch::BlockExecutionReport, Self::Error> {
        let published = PublishedCode::publish_block(block.emitted())
            .map_err(PublishedBlockExecutionError::Publish)?;
        let stop_requested = AtomicU8::new(0);
        let result = published.execute_block(state, incoming_budget, None, &stop_requested);
        // Make the one-live-mapping lifetime boundary explicit before the
        // callback returns to the dispatcher, including execution errors.
        drop(published);
        let result = result.map_err(PublishedBlockExecutionError::Execute)?;
        Ok(od_jit::dispatch::BlockExecutionReport {
            exit: result.exit,
            remaining_budget: result.remaining_budget,
            retired_instructions: result.retired_instructions,
        })
    }
}

impl od_jit::dispatch::ControlledBlockExecutor for PublishedBlockExecutor {
    type Error = PublishedBlockExecutionError;

    fn execute_one_block_controlled(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut od_isa::LongModeState,
        incoming_budget: u64,
        control: od_jit::dispatch::DispatchControl<'_>,
    ) -> Result<od_jit::dispatch::BlockExecutionReport, Self::Error> {
        let published = PublishedCode::publish_block(block.emitted())
            .map_err(PublishedBlockExecutionError::Publish)?;
        let result = published.execute_block(
            state,
            incoming_budget,
            control.stop_rip,
            control.stop_requested,
        );
        // Drop the mapping before the dispatcher reaches an interpreter step.
        drop(published);
        let result = result.map_err(PublishedBlockExecutionError::Execute)?;
        Ok(od_jit::dispatch::BlockExecutionReport {
            exit: result.exit,
            remaining_budget: result.remaining_budget,
            retired_instructions: result.retired_instructions,
        })
    }
}

/// Cache-aware counterpart to `ControlledBlockExecutor`. The cache owns the
/// published mapping across calls; each invocation executes exactly that
/// opaque handle. This implementation is deliberately separate from the
/// release-on-return executor implementations above.
impl od_jit::dispatch::CachedBlockBackend for PublishedBlockExecutor {
    type Error = PublishedBlockExecutionError;
    type Published = PublishedCode;

    fn publish_cached_block(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
    ) -> Result<Self::Published, Self::Error> {
        PublishedCode::publish_block(block.emitted()).map_err(PublishedBlockExecutionError::Publish)
    }

    fn execute_cached_block(
        &mut self,
        published: &Self::Published,
        _block: &od_jit::block_compile::CompiledBlock,
        state: &mut od_isa::LongModeState,
        incoming_budget: u64,
        control: od_jit::dispatch::DispatchControl<'_>,
    ) -> Result<od_jit::dispatch::BlockExecutionReport, Self::Error> {
        let result = published
            .execute_block(
                state,
                incoming_budget,
                control.stop_rip,
                control.stop_requested,
            )
            .map_err(PublishedBlockExecutionError::Execute)?;
        Ok(od_jit::dispatch::BlockExecutionReport {
            exit: result.exit,
            remaining_budget: result.remaining_budget,
            retired_instructions: result.retired_instructions,
        })
    }
}

pub const ARENA_SLOT_BYTES: usize = od_jit::block_compile::MAX_EMITTED_BLOCK_BYTES;
pub const MAX_PUBLISHED_ARENA_BLOCKS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishedArenaError {
    InvalidCapacity { requested: usize, max: usize },
    SizeOverflow,
    MappingAlreadyLive,
    Mapping(ExecutableMemoryError),
    CapacityFull,
    BlockTooLarge { actual: usize, max: usize },
    InvalidBlockMetadata,
    ForeignHandle,
    StaleHandle,
    GenerationExhausted,
    Execution(BlockExecutionError),
}

struct ArenaSlot {
    generation: u64,
    metadata: Option<BlockMetadata>,
}

struct ArenaInner {
    region: MappedRegion,
    mapping_bytes: usize,
    slots: Vec<ArenaSlot>,
    free_slots: Vec<usize>,
}

/// Single MAP_JIT region divided into fixed, bounded block slots. The backend
/// and handles are deliberately Rc-based and therefore neither Send nor Sync.
/// Capacity is a provisional bounded choice; no speed claim is made.
pub struct PublishedArenaBackend {
    arena: Rc<RefCell<ArenaInner>>,
    owner_id: Rc<()>,
}

/// Owns one slot and keeps its arena mapping alive. Dropping the final handle
/// for a backend frees that slot; the mapping lease ends when backend and all
/// handles have dropped.
pub struct PublishedArenaBlock {
    arena: Rc<RefCell<ArenaInner>>,
    owner_id: Rc<()>,
    slot_index: usize,
    generation: u64,
}

impl PublishedArenaBackend {
    pub fn new(max_blocks: usize) -> Result<Self, PublishedArenaError> {
        if max_blocks == 0 || max_blocks > MAX_PUBLISHED_ARENA_BLOCKS {
            return Err(PublishedArenaError::InvalidCapacity {
                requested: max_blocks,
                max: MAX_PUBLISHED_ARENA_BLOCKS,
            });
        }

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let page_size = system_page_size().ok_or(PublishedArenaError::SizeOverflow)?;
            let mapping_size = arena_mapping_size(max_blocks, ARENA_SLOT_BYTES, page_size)
                .ok_or(PublishedArenaError::SizeOverflow)?;
            let region = MappedRegion::allocate(mapping_size).map_err(arena_mapping_error)?;
            let mut slots = Vec::with_capacity(max_blocks);
            slots.resize_with(max_blocks, || ArenaSlot {
                generation: 0,
                metadata: None,
            });
            let mut free_slots = Vec::with_capacity(max_blocks);
            free_slots.extend((0..max_blocks).rev());
            return Ok(Self {
                arena: Rc::new(RefCell::new(ArenaInner {
                    region,
                    mapping_bytes: mapping_size,
                    slots,
                    free_slots,
                })),
                owner_id: Rc::new(()),
            });
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            Err(PublishedArenaError::Mapping(
                ExecutableMemoryError::UnsupportedPlatform,
            ))
        }
    }

    pub fn capacity(&self) -> usize {
        self.arena.borrow().slots.len()
    }

    pub fn available_slots(&self) -> usize {
        self.arena.borrow().free_slots.len()
    }

    /// Actual page-rounded MAP_JIT reservation size for benchmark reporting.
    pub fn reserved_mapping_bytes(&self) -> usize {
        self.arena.borrow().mapping_bytes
    }
}

impl Drop for PublishedArenaBlock {
    fn drop(&mut self) {
        let Ok(mut arena) = self.arena.try_borrow_mut() else {
            return;
        };
        let Some(slot) = arena.slots.get_mut(self.slot_index) else {
            return;
        };
        if slot.generation == self.generation && slot.metadata.take().is_some() {
            // The stack was preallocated to the fixed slot count and each
            // live handle returns at most one slot.
            arena.free_slots.push(self.slot_index);
        }
    }
}

impl od_jit::dispatch::CachedBlockBackend for PublishedArenaBackend {
    type Error = PublishedArenaError;
    type Published = PublishedArenaBlock;

    fn publish_cached_block(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
    ) -> Result<Self::Published, Self::Error> {
        let emitted = block.emitted();
        let code = emitted
            .to_le_bytes()
            .map_err(|_| PublishedArenaError::InvalidBlockMetadata)?;
        if code.is_empty() || code.len() > ARENA_SLOT_BYTES {
            return Err(PublishedArenaError::BlockTooLarge {
                actual: code.len(),
                max: ARENA_SLOT_BYTES,
            });
        }
        if usize::try_from(emitted.pc_map().code_len()).ok() != Some(code.len())
            || !emitted.safe_entry_provenance()
        {
            return Err(PublishedArenaError::InvalidBlockMetadata);
        }
        let metadata = BlockMetadata {
            pc_map: emitted.pc_map().clone(),
            entry_rip: emitted.entry_rip(),
            safe_entry: emitted.safe_entry_provenance(),
        };

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let mut arena = self.arena.borrow_mut();
            let slot_index = *arena
                .free_slots
                .last()
                .ok_or(PublishedArenaError::CapacityFull)?;
            let slot = &arena.slots[slot_index];
            let generation = slot
                .generation
                .checked_add(1)
                .ok_or(PublishedArenaError::GenerationExhausted)?;
            let offset = slot_index
                .checked_mul(ARENA_SLOT_BYTES)
                .ok_or(PublishedArenaError::SizeOverflow)?;
            let end = offset
                .checked_add(code.len())
                .ok_or(PublishedArenaError::SizeOverflow)?;
            if end > arena.region.len {
                return Err(PublishedArenaError::SizeOverflow);
            }

            let slot_base = unsafe { arena.region.ptr().as_ptr().add(offset) };
            let slot_base = NonNull::new(slot_base).ok_or(PublishedArenaError::SizeOverflow)?;
            // All fallible validation and metadata allocation precede slot
            // reservation. Copy/cache sync has no recoverable error path.
            arena.free_slots.pop();
            let slot = &mut arena.slots[slot_index];
            slot.generation = generation;
            slot.metadata = Some(metadata);
            // SAFETY: the checked slot range belongs to this arena; the source
            // `code` vector cannot alias the MAP_JIT mapping.
            unsafe { write_mapping_bytes(slot_base, &code) };
            drop(arena);
            return Ok(PublishedArenaBlock {
                arena: self.arena.clone(),
                owner_id: self.owner_id.clone(),
                slot_index,
                generation,
            });
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = (code, metadata);
            Err(PublishedArenaError::Mapping(
                ExecutableMemoryError::UnsupportedPlatform,
            ))
        }
    }

    fn execute_cached_block(
        &mut self,
        published: &Self::Published,
        _block: &od_jit::block_compile::CompiledBlock,
        state: &mut od_isa::LongModeState,
        incoming_budget: u64,
        control: od_jit::dispatch::DispatchControl<'_>,
    ) -> Result<od_jit::dispatch::BlockExecutionReport, Self::Error> {
        if !Rc::ptr_eq(&self.owner_id, &published.owner_id)
            || !Rc::ptr_eq(&self.arena, &published.arena)
        {
            return Err(PublishedArenaError::ForeignHandle);
        }

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let arena = published.arena.borrow();
            let slot = arena
                .slots
                .get(published.slot_index)
                .ok_or(PublishedArenaError::StaleHandle)?;
            if slot.generation != published.generation {
                return Err(PublishedArenaError::StaleHandle);
            }
            let metadata = slot
                .metadata
                .as_ref()
                .ok_or(PublishedArenaError::StaleHandle)?;
            let offset = published
                .slot_index
                .checked_mul(ARENA_SLOT_BYTES)
                .ok_or(PublishedArenaError::SizeOverflow)?;
            if offset >= arena.region.len {
                return Err(PublishedArenaError::SizeOverflow);
            }
            let code_ptr = unsafe { arena.region.ptr().as_ptr().add(offset) };
            let result = execute_block_at(
                code_ptr,
                metadata,
                state,
                incoming_budget,
                control.stop_rip,
                control.stop_requested,
            )
            .map_err(PublishedArenaError::Execution)?;
            return Ok(od_jit::dispatch::BlockExecutionReport {
                exit: result.exit,
                remaining_budget: result.remaining_budget,
                retired_instructions: result.retired_instructions,
            });
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = (published, state, incoming_budget, control);
            Err(PublishedArenaError::Mapping(
                ExecutableMemoryError::UnsupportedPlatform,
            ))
        }
    }
}

impl od_jit::dispatch::MultiBlockCachedBackend for PublishedArenaBackend {
    fn published_block_capacity(&self) -> usize {
        self.capacity()
    }
}

fn arena_mapping_size(slot_count: usize, slot_bytes: usize, page_bytes: usize) -> Option<usize> {
    if slot_count == 0 || slot_bytes == 0 || page_bytes == 0 {
        return None;
    }
    let used = slot_count.checked_mul(slot_bytes)?;
    let remainder = used % page_bytes;
    if remainder == 0 {
        Some(used)
    } else {
        used.checked_add(page_bytes.checked_sub(remainder)?)
    }
}

fn arena_mapping_error(error: ExecutableMemoryError) -> PublishedArenaError {
    match error {
        ExecutableMemoryError::MappingAlreadyLive => PublishedArenaError::MappingAlreadyLive,
        ExecutableMemoryError::InvalidBlockArtifact => PublishedArenaError::InvalidBlockMetadata,
        other => PublishedArenaError::Mapping(other),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn system_page_size() -> Option<usize> {
    unsafe extern "C" {
        fn getpagesize() -> i32;
    }
    let size = unsafe { getpagesize() };
    usize::try_from(size).ok().filter(|size| *size != 0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishedBlockExecutionError {
    Publish(ExecutableMemoryError),
    Execute(BlockExecutionError),
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
        Err(FragmentExecutionError::Memory(ExecutableMemoryError::InvalidBlockArtifact)) => {
            return STATUS_INVALID_ARGUMENT;
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
    use od_jit::allocator::{Allocation, AllocatorConfig, HelperBoundaryPosition, ValueLocation};
    use od_jit::arm64::{
        AddressBase, Condition, Encoder, PairIndexMode, WReg, XReg, XRegOrSp, XRegOrZero,
    };
    use od_jit::block::{self, AdmittedInstructionBoundaries, BlockLimits, CodeView};
    use od_jit::block_compile::{BlockCompileOutcome, compile_block, compile_block_for_dispatch};
    use od_jit::dispatch::CachedBlockBackend;
    use od_jit::emitter::{
        self, DATA_EFFECTS_UNCOMMITTED_V1, DataEffectContextV2, DataEffectEntry,
        DataEffectOutputKind, DataEffectTerminal, FlagMaterialization,
    };
    use od_jit::ir::{ExitKind, GuestGpr, GuestInstruction, IntWidth, Operation, Program, TempId};
    use od_jit::lowering::{self, LoweringOutcome};
    use od_jit::runtime::{
        ArithmeticFlagsHelper, BlockEntry, HelperOperationTag, RAW_HELPER_CONTRACT_FAILURE_V1,
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET, RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET,
        RuntimeContextV1, RuntimeExitTag,
    };
    use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize};

    static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());
    static DATA_EFFECT_HELPER_STATUS: AtomicU32 = AtomicU32::new(0);
    static RUNTIME_HELPER_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RUNTIME_HELPER_FAIL_AT: AtomicUsize = AtomicUsize::new(0);
    static RUNTIME_HELPER_FAILURE_STATUS: AtomicU32 = AtomicU32::new(2);
    static RUNTIME_HELPER_STOP_ON_CALL: AtomicUsize = AtomicUsize::new(0);
    static RUNTIME_HELPER_STOP_BYTE: AtomicPtr<AtomicU8> = AtomicPtr::new(std::ptr::null_mut());

    fn jit_test_lock() -> std::sync::MutexGuard<'static, ()> {
        JIT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn cached_test_context() -> od_jit::translation::ExecutionContextSnapshot {
        od_jit::translation::ExecutionContextSnapshot::s0_09_fixture(
            od_jit::translation::ExecutionContextId::from_u64(1)
                .expect("test context ID is nonzero"),
        )
    }

    #[test]
    fn v2_mov_load_commits_alias_and_preserves_faulting_state_on_helper_failure() {
        use od_isa::memory::{
            AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError,
        };

        struct Memory<'a> {
            bytes: [u8; 32],
            failure: Option<MemoryError>,
            reads: usize,
            stop_on_read: Option<&'a AtomicU8>,
        }
        impl GuestMemory for Memory<'_> {
            fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
                self.bytes
                    .get(address as usize)
                    .copied()
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Fetch,
                    })
            }

            fn read(
                &mut self,
                address: u64,
                _: AccessContext,
                output: &mut [u8],
            ) -> Result<(), MemoryError> {
                self.reads += 1;
                if let Some(error) = self.failure.clone() {
                    return Err(error);
                }
                let start = address as usize;
                let source =
                    self.bytes
                        .get(start..start + output.len())
                        .ok_or(MemoryError::Unmapped {
                            address,
                            access: AccessKind::Read,
                        })?;
                output.copy_from_slice(source);
                if let Some(stop) = self.stop_on_read {
                    stop.store(1, Ordering::Release);
                }
                Ok(())
            }

            fn write(
                &mut self,
                address: u64,
                _: AccessContext,
                _: &[u8],
            ) -> Result<(), MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn atomic_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn atomic_fetch_add_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn atomic_compare_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let _guard = jit_test_lock();
        let code = CodeView::new(0x9a00, &[0x48, 0x8b, 0x00]).unwrap();
        let compiled = od_jit::memory_load_v2::compile_memory_load_block_v2(code, 0x9a00).unwrap();
        let published = PublishedCode::publish_memory_load_v2(&compiled).unwrap();
        let stop = AtomicU8::new(0);
        let mut memory = Memory {
            bytes: [0; 32],
            failure: None,
            reads: 0,
            stop_on_read: None,
        };
        memory.bytes[4..12].copy_from_slice(&0x8877_6655_4433_2211_u64.to_le_bytes());
        let mut state = od_isa::LongModeState {
            rip: 0x9a00,
            rflags: 0x8d7,
            ..od_isa::LongModeState::default()
        };
        state.gpr[0] = 4;
        let result = published
            .execute_memory_load_v2(
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop,
            )
            .unwrap();
        assert_eq!(result.exit, MemoryOperationExecutionExitV2::Complete);
        assert_eq!(result.remaining_budget, 1);
        assert_eq!(result.retired_instructions, 1);
        assert_eq!(state.rip, 0x9a03);
        assert_eq!(state.gpr[0], 0x8877_6655_4433_2211);
        assert_eq!(state.rflags, 0x8d7);
        assert_eq!(memory.reads, 1);

        stop.store(0, Ordering::Release);
        memory.stop_on_read = Some(&stop);
        let mut stopped_state = od_isa::LongModeState {
            rip: 0x9a00,
            ..od_isa::LongModeState::default()
        };
        stopped_state.gpr[0] = 4;
        let stopped = published
            .execute_memory_load_v2(
                &mut stopped_state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
            .unwrap();
        assert_eq!(stopped.exit, MemoryOperationExecutionExitV2::StopRequested);
        assert_eq!(stopped_state.rip, 0x9a03);
        assert_eq!(stopped_state.gpr[0], 0x8877_6655_4433_2211);
        assert_eq!(stopped.retired_instructions, 1);
        memory.stop_on_read = None;
        assert_eq!(
            published.execute_block(&mut state, 1, None, &stop),
            Err(BlockExecutionError::NotRuntimeBlock),
        );

        let architectural = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(4),
        });
        memory.failure = Some(architectural);
        stop.store(0, Ordering::Release);
        let mut fault_state = od_isa::LongModeState {
            rip: 0x9a00,
            rflags: 0x202,
            ..od_isa::LongModeState::default()
        };
        fault_state.gpr[0] = 4;
        let before = fault_state.clone();
        let fault = published
            .execute_memory_load_v2(
                &mut fault_state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
            .unwrap();
        assert_eq!(
            fault.exit,
            MemoryOperationExecutionExitV2::GuestFault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(4),
            })
        );
        assert_eq!(fault.remaining_budget, 1);
        assert_eq!(fault.retired_instructions, 0);
        assert_eq!(fault_state, before);

        memory.failure = Some(MemoryError::Backend("backend unavailable".into()));
        assert!(matches!(
            published.execute_memory_load_v2(
                &mut fault_state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
            ),
            Err(MemoryOperationExecutionErrorV2::MemoryError(
                MemoryOperationMemoryErrorV2 {
                    kind: od_jit::memory_bridge::MEMORY_ERROR_BACKEND,
                    ..
                }
            ))
        ));
        assert_eq!(fault_state, before);
    }

    #[test]
    fn v2_mov_load_non_alias_and_entry_controls_do_not_touch_memory_early() {
        use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

        struct Memory {
            bytes: [u8; 32],
            reads: usize,
        }
        impl GuestMemory for Memory {
            fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
                self.bytes
                    .get(address as usize)
                    .copied()
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Fetch,
                    })
            }
            fn read(
                &mut self,
                address: u64,
                _: AccessContext,
                output: &mut [u8],
            ) -> Result<(), MemoryError> {
                self.reads += 1;
                let start = address as usize;
                let source =
                    self.bytes
                        .get(start..start + output.len())
                        .ok_or(MemoryError::Unmapped {
                            address,
                            access: AccessKind::Read,
                        })?;
                output.copy_from_slice(source);
                Ok(())
            }
            fn write(
                &mut self,
                address: u64,
                _: AccessContext,
                _: &[u8],
            ) -> Result<(), MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn atomic_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn atomic_fetch_add_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn atomic_compare_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let _guard = jit_test_lock();
        let code = CodeView::new(0x9b00, &[0x48, 0x8b, 0x08]).unwrap();
        let compiled = od_jit::memory_load_v2::compile_memory_load_block_v2(code, 0x9b00).unwrap();
        let published = PublishedCode::publish_memory_load_v2(&compiled).unwrap();
        let stop = AtomicU8::new(0);
        let mut memory = Memory {
            bytes: [0; 32],
            reads: 0,
        };
        memory.bytes[8..16].copy_from_slice(&0x1234_5678_9abc_def0_u64.to_le_bytes());

        let mut state = od_isa::LongModeState {
            rip: 0x9b00,
            ..od_isa::LongModeState::default()
        };
        state.gpr[0] = 8;
        state.gpr[1] = 0xfeed;
        let before = state.clone();
        let requested = AtomicU8::new(1);
        let stopped_before_budget = published
            .execute_memory_load_v2(
                &mut state,
                &mut memory,
                AccessContext::default(),
                0,
                None,
                &requested,
            )
            .unwrap();
        assert_eq!(
            stopped_before_budget.exit,
            MemoryOperationExecutionExitV2::StopRequested
        );
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);

        let exhausted = published
            .execute_memory_load_v2(
                &mut state,
                &mut memory,
                AccessContext::default(),
                0,
                None,
                &stop,
            )
            .unwrap();
        assert_eq!(
            exhausted.exit,
            MemoryOperationExecutionExitV2::BudgetExhausted
        );
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);

        let stopped = published
            .execute_memory_load_v2(
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &requested,
            )
            .unwrap();
        assert_eq!(stopped.exit, MemoryOperationExecutionExitV2::StopRequested);
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);

        let stop_address = published
            .execute_memory_load_v2(
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                Some(0x9b00),
                &stop,
            )
            .unwrap();
        assert_eq!(
            stop_address.exit,
            MemoryOperationExecutionExitV2::StopAddress
        );
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);

        let loaded = published
            .execute_memory_load_v2(
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
            .unwrap();
        assert_eq!(loaded.exit, MemoryOperationExecutionExitV2::Complete);
        assert_eq!(state.gpr[0], 8);
        assert_eq!(state.gpr[1], 0x1234_5678_9abc_def0);
        assert_eq!(state.rip, 0x9b03);
        assert_eq!(memory.reads, 1);

        let mut noncanonical = od_isa::LongModeState {
            rip: 0x9b00,
            ..od_isa::LongModeState::default()
        };
        noncanonical.gpr[0] = 0x0000_8000_0000_0000;
        let before = noncanonical.clone();
        let fault = published
            .execute_memory_load_v2(
                &mut noncanonical,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
            .unwrap();
        assert!(matches!(
            fault.exit,
            MemoryOperationExecutionExitV2::GuestFault(_)
        ));
        assert_eq!(noncanonical, before);
        assert_eq!(memory.reads, 1);
    }

    #[test]
    fn v2_mov_load_matches_interpreter_on_independent_state_and_ram_snapshots() {
        use od_isa::memory::{
            AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError,
        };

        #[derive(Clone, Debug, Eq, PartialEq)]
        struct Ram {
            bytes: [u8; 64],
            reads: usize,
            read_error: Option<MemoryError>,
        }
        impl GuestMemory for Ram {
            fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
                if (0x9c00..0x9c03).contains(&address) {
                    return Ok([0x48, 0x8b, 0x03][(address - 0x9c00) as usize]);
                }
                self.bytes
                    .get(address as usize)
                    .copied()
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Fetch,
                    })
            }

            fn read(
                &mut self,
                address: u64,
                _: AccessContext,
                output: &mut [u8],
            ) -> Result<(), MemoryError> {
                self.reads += 1;
                if let Some(error) = self.read_error.clone() {
                    return Err(error);
                }
                let start = address as usize;
                let source =
                    self.bytes
                        .get(start..start + output.len())
                        .ok_or(MemoryError::Unmapped {
                            address,
                            access: AccessKind::Read,
                        })?;
                output.copy_from_slice(source);
                Ok(())
            }

            fn write(
                &mut self,
                address: u64,
                _: AccessContext,
                _: &[u8],
            ) -> Result<(), MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn atomic_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn atomic_fetch_add_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn atomic_compare_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let _guard = jit_test_lock();
        let code = CodeView::new(0x9c00, &[0x48, 0x8b, 0x03]).unwrap();
        let compiled = od_jit::memory_load_v2::compile_memory_load_block_v2(code, 0x9c00).unwrap();
        let published = PublishedCode::publish_memory_load_v2(&compiled).unwrap();
        let stop = AtomicU8::new(0);
        let base_ram = Ram {
            bytes: {
                let mut bytes = [0; 64];
                bytes[8..16].copy_from_slice(&0xa1b2_c3d4_e5f6_0718_u64.to_le_bytes());
                bytes
            },
            reads: 0,
            read_error: None,
        };
        let base_state = od_isa::LongModeState {
            rip: 0x9c00,
            rflags: 0x246,
            ..od_isa::LongModeState::default()
        };
        let mut native_ram = base_ram.clone();
        let mut interpreter_ram = base_ram.clone();
        let mut native_state = base_state.clone();
        let mut interpreter_state = base_state.clone();
        native_state.gpr[3] = 8;
        interpreter_state.gpr[3] = 8;
        let native = published
            .execute_memory_load_v2(
                &mut native_state,
                &mut native_ram,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
            .unwrap();
        let interpreted = od_isa::execute::step(
            &mut interpreter_state,
            &mut interpreter_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert_eq!(native.exit, MemoryOperationExecutionExitV2::Complete);
        assert_eq!(interpreted, od_isa::execute::StepOutcome::Advanced);
        assert_eq!(native_state, interpreter_state);
        assert_eq!(native_ram, interpreter_ram);

        let architectural = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(8),
        });
        let mut native_ram = base_ram.clone();
        native_ram.read_error = Some(architectural.clone());
        let mut interpreter_ram = native_ram.clone();
        let mut native_state = base_state.clone();
        let mut interpreter_state = base_state.clone();
        native_state.gpr[3] = 8;
        interpreter_state.gpr[3] = 8;
        let native = published
            .execute_memory_load_v2(
                &mut native_state,
                &mut native_ram,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
            .unwrap();
        let interpreted = od_isa::execute::step(
            &mut interpreter_state,
            &mut interpreter_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert_eq!(
            native.exit,
            MemoryOperationExecutionExitV2::GuestFault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(8),
            })
        );
        assert_eq!(
            interpreted,
            od_isa::execute::StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(8),
            })
        );
        assert_eq!(native_state, interpreter_state);
        assert_eq!(native_ram, interpreter_ram);

        let mut native_ram = base_ram.clone();
        native_ram.read_error = Some(MemoryError::Unmapped {
            address: 8,
            access: AccessKind::Read,
        });
        let mut interpreter_ram = native_ram.clone();
        let mut native_state = base_state.clone();
        let mut interpreter_state = base_state.clone();
        native_state.gpr[3] = 8;
        interpreter_state.gpr[3] = 8;
        let native = published.execute_memory_load_v2(
            &mut native_state,
            &mut native_ram,
            AccessContext::default(),
            1,
            None,
            &stop,
        );
        let interpreted = od_isa::execute::step(
            &mut interpreter_state,
            &mut interpreter_ram,
            AccessContext::default(),
        );
        assert!(matches!(
            native,
            Err(MemoryOperationExecutionErrorV2::MemoryError(
                MemoryOperationMemoryErrorV2 {
                    kind: od_jit::memory_bridge::MEMORY_ERROR_UNMAPPED,
                    address: 8,
                    ..
                }
            ))
        ));
        assert!(matches!(
            interpreted,
            Err(MemoryError::Unmapped {
                address: 8,
                access: AccessKind::Read,
            })
        ));
        assert_eq!(native_state, interpreter_state);
        assert_eq!(native_ram, interpreter_ram);
    }

    #[test]
    fn v2_mov_store_matches_interpreter_and_commits_only_after_one_checked_write() {
        use od_isa::memory::{
            AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError,
        };

        #[derive(Clone)]
        struct Ram<'a> {
            bytes: [u8; 64],
            read_count: usize,
            write_attempts: usize,
            committed_writes: usize,
            write_error: Option<MemoryError>,
            stop_on_write: Option<&'a AtomicU8>,
        }
        impl GuestMemory for Ram<'_> {
            fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
                if (0x9d00..0x9d03).contains(&address) {
                    return Ok([0x48, 0x89, 0x00][(address - 0x9d00) as usize]);
                }
                self.bytes
                    .get(address as usize)
                    .copied()
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Fetch,
                    })
            }
            fn read(
                &mut self,
                address: u64,
                _: AccessContext,
                output: &mut [u8],
            ) -> Result<(), MemoryError> {
                self.read_count += 1;
                let start = address as usize;
                let source =
                    self.bytes
                        .get(start..start + output.len())
                        .ok_or(MemoryError::Unmapped {
                            address,
                            access: AccessKind::Read,
                        })?;
                output.copy_from_slice(source);
                Ok(())
            }
            fn write(
                &mut self,
                address: u64,
                _: AccessContext,
                source: &[u8],
            ) -> Result<(), MemoryError> {
                self.write_attempts += 1;
                if let Some(error) = self.write_error.clone() {
                    return Err(error);
                }
                let start = address as usize;
                let destination = self.bytes.get_mut(start..start + source.len()).ok_or(
                    MemoryError::Unmapped {
                        address,
                        access: AccessKind::Write,
                    },
                )?;
                destination.copy_from_slice(source);
                self.committed_writes += 1;
                if let Some(stop) = self.stop_on_write {
                    stop.store(1, Ordering::Release);
                }
                Ok(())
            }
            fn atomic_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn atomic_fetch_add_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn atomic_compare_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let _guard = jit_test_lock();
        let code = CodeView::new(0x9d00, &[0x48, 0x89, 0x00]).unwrap();
        let compiled = od_jit::memory_load_v2::compile_memory_store_block_v2(code, 0x9d00).unwrap();
        assert_eq!(compiled.source_bytes(), &[0x48, 0x89, 0x00]);
        assert_eq!(
            compiled.compile_flavor(),
            od_jit::translation::CompileFlavor::S3_20bMemoryStoreV2
        );
        let published = PublishedCode::publish_memory_store_v2(&compiled).unwrap();
        let stop = AtomicU8::new(0);
        let base_ram = Ram {
            bytes: [0xa5; 64],
            read_count: 0,
            write_attempts: 0,
            committed_writes: 0,
            write_error: None,
            stop_on_write: None,
        };
        let base_state = od_isa::LongModeState {
            rip: 0x9d00,
            rflags: 0x246,
            ..od_isa::LongModeState::default()
        };

        // Store metadata is rejected by the safe load API before it can cast
        // the V2 entry pointer or touch memory.
        let mut rejected_state = base_state.clone();
        let mut rejected_ram = base_ram.clone();
        assert!(matches!(
            published.execute_memory_load_v2(
                &mut rejected_state,
                &mut rejected_ram,
                AccessContext::default(),
                1,
                None,
                &stop,
            ),
            Err(MemoryOperationExecutionErrorV2::InvalidMetadata)
        ));
        assert_eq!(rejected_state, base_state);
        assert_eq!(rejected_ram.write_attempts, 0);

        for (budget, stop_rip, stop_value, expected) in [
            (0, None, 1, MemoryOperationExecutionExitV2::StopRequested),
            (
                0,
                Some(0x9d00),
                0,
                MemoryOperationExecutionExitV2::StopAddress,
            ),
            (0, None, 0, MemoryOperationExecutionExitV2::BudgetExhausted),
        ] {
            let mut guarded_state = base_state.clone();
            let mut guarded_ram = base_ram.clone();
            guarded_state.gpr[0] = 8;
            let before_state = guarded_state.clone();
            let before_ram = guarded_ram.clone();
            let stop = AtomicU8::new(stop_value);
            // SAFETY: this fixture remains private synchronous non-executable
            // RAM; entry guards must return before issuing its write.
            let outcome = unsafe {
                published.execute_memory_store_v2(
                    &mut guarded_state,
                    &mut guarded_ram,
                    AccessContext::default(),
                    budget,
                    stop_rip,
                    &stop,
                )
            }
            .unwrap();
            assert_eq!(outcome.exit, expected);
            assert_eq!(guarded_state, before_state);
            assert_eq!(guarded_ram.bytes, before_ram.bytes);
            assert_eq!(guarded_ram.read_count, before_ram.read_count);
            assert_eq!(guarded_ram.write_attempts, before_ram.write_attempts);
            assert_eq!(guarded_ram.committed_writes, before_ram.committed_writes);
            assert_eq!(guarded_ram.write_attempts, 0);
            assert_eq!(guarded_ram.committed_writes, 0);
        }

        let mut native_ram = base_ram.clone();
        let mut interpreter_ram = base_ram.clone();
        let mut native_state = base_state.clone();
        let mut interpreter_state = base_state.clone();
        native_state.gpr[0] = 8;
        interpreter_state.gpr[0] = 8;
        // SAFETY: this fixture is private synchronous RAM, its write is
        // all-or-nothing, and it has no MMIO, DMA, concurrency, delayed effects,
        // executable/code alias, or later executable transition.
        let native = unsafe {
            published.execute_memory_store_v2(
                &mut native_state,
                &mut native_ram,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
        }
        .unwrap();
        let interpreted = od_isa::execute::step(
            &mut interpreter_state,
            &mut interpreter_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert_eq!(native.exit, MemoryOperationExecutionExitV2::Complete);
        assert_eq!(interpreted, od_isa::execute::StepOutcome::Advanced);
        assert_eq!(native_state, interpreter_state);
        assert_eq!(native_ram.bytes, interpreter_ram.bytes);
        assert_eq!(&native_ram.bytes[8..16], &8_u64.to_le_bytes());
        assert_eq!(native_ram.read_count, interpreter_ram.read_count);
        assert_eq!(native_ram.write_attempts, 1);
        assert_eq!(native_ram.committed_writes, 1);
        assert_eq!(native_state.gpr[0], 8);
        assert_eq!(native_state.rip, 0x9d03);

        for error in [
            MemoryError::Unmapped {
                address: 8,
                access: AccessKind::Write,
            },
            MemoryError::Backend("injected store failure".into()),
        ] {
            let mut native_ram = base_ram.clone();
            native_ram.write_error = Some(error.clone());
            let mut interpreter_ram = native_ram.clone();
            let mut native_state = base_state.clone();
            let mut interpreter_state = base_state.clone();
            native_state.gpr[0] = 8;
            interpreter_state.gpr[0] = 8;
            // SAFETY: same private, synchronous and non-executable RAM fixture
            // as the successful path; its injected errors leave bytes unchanged.
            let native = unsafe {
                published.execute_memory_store_v2(
                    &mut native_state,
                    &mut native_ram,
                    AccessContext::default(),
                    1,
                    None,
                    &stop,
                )
            };
            let interpreted = od_isa::execute::step(
                &mut interpreter_state,
                &mut interpreter_ram,
                AccessContext::default(),
            );
            let expected_kind = match &error {
                MemoryError::Unmapped { .. } => od_jit::memory_bridge::MEMORY_ERROR_UNMAPPED,
                MemoryError::Backend(_) => od_jit::memory_bridge::MEMORY_ERROR_BACKEND,
                _ => unreachable!(),
            };
            assert!(matches!(
                native,
                Err(MemoryOperationExecutionErrorV2::MemoryError(
                    MemoryOperationMemoryErrorV2 { kind, .. }
                )) if kind == expected_kind
            ));
            assert_eq!(interpreted, Err(error));
            assert_eq!(native_state, interpreter_state);
            assert_eq!(native_ram.bytes, interpreter_ram.bytes);
            assert_eq!(native_ram.write_attempts, 1);
            assert_eq!(native_ram.committed_writes, 0);
        }

        let fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(2),
            linear_address: Some(8),
        });
        let mut native_ram = base_ram.clone();
        native_ram.write_error = Some(fault.clone());
        let mut interpreter_ram = native_ram.clone();
        let mut native_state = base_state.clone();
        let mut interpreter_state = base_state.clone();
        native_state.gpr[0] = 8;
        interpreter_state.gpr[0] = 8;
        // SAFETY: same private RAM guarantee; injected architectural faults do
        // not modify memory, as required by GuestMemory's all-or-nothing write.
        let native = unsafe {
            published.execute_memory_store_v2(
                &mut native_state,
                &mut native_ram,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
        }
        .unwrap();
        let interpreted = od_isa::execute::step(
            &mut interpreter_state,
            &mut interpreter_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert_eq!(
            native.exit,
            MemoryOperationExecutionExitV2::GuestFault(ArchitecturalFault {
                vector: 14,
                error_code: Some(2),
                linear_address: Some(8),
            })
        );
        assert_eq!(
            interpreted,
            od_isa::execute::StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(2),
                linear_address: Some(8),
            })
        );
        assert_eq!(native_state, interpreter_state);
        assert_eq!(native_ram.bytes, interpreter_ram.bytes);
        assert_eq!(native_ram.write_attempts, 1);
        assert_eq!(native_ram.committed_writes, 0);

        let late_stop = AtomicU8::new(0);
        let mut native_ram = base_ram.clone();
        native_ram.stop_on_write = Some(&late_stop);
        let mut interpreter_ram = native_ram.clone();
        interpreter_ram.stop_on_write = None;
        let mut native_state = base_state.clone();
        let mut interpreter_state = base_state.clone();
        native_state.gpr[0] = 8;
        interpreter_state.gpr[0] = 8;
        // SAFETY: the same verified private synchronous RAM fixture conditions.
        let native = unsafe {
            published.execute_memory_store_v2(
                &mut native_state,
                &mut native_ram,
                AccessContext::default(),
                1,
                None,
                &late_stop,
            )
        }
        .unwrap();
        let interpreted = od_isa::execute::step(
            &mut interpreter_state,
            &mut interpreter_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert_eq!(native.exit, MemoryOperationExecutionExitV2::StopRequested);
        assert_eq!(interpreted, od_isa::execute::StepOutcome::Advanced);
        assert_eq!(native_state, interpreter_state);
        assert_eq!(native_ram.bytes, interpreter_ram.bytes);
        let mut expected_bytes = base_ram.bytes;
        expected_bytes[8..16].copy_from_slice(&8_u64.to_le_bytes());
        assert_eq!(native_ram.bytes, expected_bytes);
        assert_eq!(native_ram.write_attempts, 1);
        assert_eq!(native_ram.committed_writes, 1);
        assert_eq!(native.retired_instructions, 1);
    }

    #[test]
    fn v2_sib_mov_native_matches_interpreter_for_all_supported_register_scale_combinations() {
        use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

        #[derive(Clone)]
        struct MatrixRam {
            bytes: [u8; 512],
            code: [u8; 8],
            code_len: usize,
            code_rip: u64,
            reads: usize,
            write_attempts: usize,
            writes: usize,
            read_error: Option<MemoryError>,
            write_error: Option<MemoryError>,
            stop_on_write: Option<std::sync::Arc<AtomicU8>>,
        }
        impl GuestMemory for MatrixRam {
            fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
                if (self.code_rip..self.code_rip + self.code_len as u64).contains(&address) {
                    return Ok(self.code[(address - self.code_rip) as usize]);
                }
                self.bytes
                    .get(address as usize)
                    .copied()
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Fetch,
                    })
            }
            fn read(
                &mut self,
                address: u64,
                _: AccessContext,
                output: &mut [u8],
            ) -> Result<(), MemoryError> {
                self.reads += 1;
                if let Some(error) = self.read_error.clone() {
                    return Err(error);
                }
                for (offset, byte) in output.iter_mut().enumerate() {
                    let current =
                        address
                            .checked_add(offset as u64)
                            .ok_or(MemoryError::Unmapped {
                                address,
                                access: AccessKind::Read,
                            })?;
                    if (self.code_rip..self.code_rip + self.code_len as u64).contains(&current) {
                        *byte = self.code[(current - self.code_rip) as usize];
                    } else {
                        *byte = *self
                            .bytes
                            .get(current as usize)
                            .ok_or(MemoryError::Unmapped {
                                address,
                                access: AccessKind::Read,
                            })?;
                    }
                }
                Ok(())
            }
            fn write(
                &mut self,
                address: u64,
                _: AccessContext,
                source: &[u8],
            ) -> Result<(), MemoryError> {
                self.write_attempts += 1;
                if let Some(error) = self.write_error.clone() {
                    return Err(error);
                }
                if (address..address.saturating_add(source.len() as u64)).any(|current| {
                    (self.code_rip..self.code_rip + self.code_len as u64).contains(&current)
                }) {
                    return Err(MemoryError::PermissionDenied {
                        address,
                        access: AccessKind::Write,
                    });
                }
                let start = address as usize;
                let destination = self.bytes.get_mut(start..start + source.len()).ok_or(
                    MemoryError::Unmapped {
                        address,
                        access: AccessKind::Write,
                    },
                )?;
                destination.copy_from_slice(source);
                self.writes += 1;
                if let Some(stop) = &self.stop_on_write {
                    stop.store(1, Ordering::Release);
                }
                Ok(())
            }
            fn atomic_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn atomic_fetch_add_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn atomic_compare_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }
            fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let _guard = jit_test_lock();
        let stop = AtomicU8::new(0);
        for (opcode, is_load) in [(0x8b, true), (0x89, false)] {
            for mode in 0..=2_u8 {
                for data_register in 0..4_u8 {
                    for addressing in 0..=1_u8 {
                        for base in 0..4_u8 {
                            for index in 0..4_u8 {
                                for scale_shift in 0..4_u8 {
                                    if addressing == 0 && (index != 0 || scale_shift != 0) {
                                        continue;
                                    }
                                    let mut code_vec = vec![
                                        0x48,
                                        opcode,
                                        (mode << 6)
                                            | (data_register << 3)
                                            | if addressing == 0 { base } else { 4 },
                                    ];
                                    if addressing == 1 {
                                        code_vec.push((scale_shift << 6) | (index << 3) | base);
                                    }
                                    let displacement = match mode {
                                        0 => 0_i32,
                                        1 => -1,
                                        2 => 1,
                                        _ => unreachable!(),
                                    };
                                    match mode {
                                        0 => {}
                                        1 => code_vec.push(displacement as i8 as u8),
                                        2 => {
                                            code_vec.extend_from_slice(&displacement.to_le_bytes())
                                        }
                                        _ => unreachable!(),
                                    }
                                    let mut code_bytes = [0; 8];
                                    code_bytes[..code_vec.len()].copy_from_slice(&code_vec);
                                    let code = CodeView::new(0xa000, &code_vec).unwrap();
                                    let block = if is_load {
                                        let block =
                                            od_jit::memory_load_v2::compile_memory_load_block_v2(
                                                code, 0xa000,
                                            )
                                            .unwrap();
                                        PublishedCode::publish_memory_load_v2(&block).unwrap()
                                    } else {
                                        let block =
                                            od_jit::memory_load_v2::compile_memory_store_block_v2(
                                                code, 0xa000,
                                            )
                                            .unwrap();
                                        PublishedCode::publish_memory_store_v2(&block).unwrap()
                                    };
                                    let mut initial_ram = MatrixRam {
                                        bytes: [0; 512],
                                        code: code_bytes,
                                        code_len: code_vec.len(),
                                        code_rip: 0xa000,
                                        reads: 0,
                                        write_attempts: 0,
                                        writes: 0,
                                        read_error: None,
                                        write_error: None,
                                        stop_on_write: None,
                                    };
                                    for (offset, byte) in initial_ram.bytes.iter_mut().enumerate() {
                                        *byte = (offset as u8).wrapping_mul(29).wrapping_add(7);
                                    }
                                    let mut initial_state = od_isa::LongModeState {
                                        rip: 0xa000,
                                        rflags: 0x8d7,
                                        ..od_isa::LongModeState::default()
                                    };
                                    initial_state.gpr[0..4].copy_from_slice(&[16, 20, 24, 28]);
                                    let effective_address = initial_state.gpr[base as usize]
                                        .wrapping_add(if addressing == 1 {
                                            initial_state.gpr[index as usize]
                                                .wrapping_mul(1_u64 << scale_shift)
                                        } else {
                                            0
                                        })
                                        .wrapping_add_signed(i64::from(displacement));
                                    let expected_store_bytes =
                                        initial_state.gpr[data_register as usize].to_le_bytes();
                                    let mut native_ram = initial_ram.clone();
                                    let mut interpreted_ram = initial_ram.clone();
                                    let mut native_state = initial_state.clone();
                                    let mut interpreted_state = initial_state.clone();
                                    let native = if is_load {
                                        block
                                            .execute_memory_load_v2(
                                                &mut native_state,
                                                &mut native_ram,
                                                AccessContext::default(),
                                                1,
                                                None,
                                                &stop,
                                            )
                                            .unwrap()
                                    } else {
                                        // SAFETY: this fixture is private synchronous RAM,
                                        // with all-or-nothing writes, no MMIO/DMA/concurrency,
                                        // and no executable/code alias or later transition.
                                        unsafe {
                                            block.execute_memory_store_v2(
                                                &mut native_state,
                                                &mut native_ram,
                                                AccessContext::default(),
                                                1,
                                                None,
                                                &stop,
                                            )
                                        }
                                        .unwrap()
                                    };
                                    let interpreted = od_isa::execute::step(
                                        &mut interpreted_state,
                                        &mut interpreted_ram,
                                        AccessContext::default(),
                                    )
                                    .unwrap();
                                    assert_eq!(
                                        native.exit,
                                        MemoryOperationExecutionExitV2::Complete
                                    );
                                    assert_eq!(interpreted, od_isa::execute::StepOutcome::Advanced);
                                    assert_eq!(native_state, interpreted_state);
                                    assert_eq!(native_ram.bytes, interpreted_ram.bytes);
                                    assert_eq!(native_ram.writes, usize::from(!is_load));
                                    assert_eq!(native_ram.reads, usize::from(is_load));
                                    assert_eq!(native_ram.write_attempts, usize::from(!is_load));
                                    assert_eq!(native_state.rip, 0xa000 + code_vec.len() as u64);
                                    if !is_load {
                                        let start = effective_address as usize;
                                        assert_eq!(
                                            &native_ram.bytes[start..start + 8],
                                            expected_store_bytes.as_slice()
                                        );
                                    }

                                    // STOP, stop-RIP, and zero-budget guards must return
                                    // before any helper access for each admitted site.
                                    for (budget, stop_rip, stop_value, expected) in [
                                        (0, None, 1, MemoryOperationExecutionExitV2::StopRequested),
                                        (
                                            1,
                                            Some(0xa000),
                                            0,
                                            MemoryOperationExecutionExitV2::StopAddress,
                                        ),
                                        (
                                            0,
                                            None,
                                            0,
                                            MemoryOperationExecutionExitV2::BudgetExhausted,
                                        ),
                                    ] {
                                        let mut guarded_state = initial_state.clone();
                                        let before_state = guarded_state.clone();
                                        let mut guarded_ram = initial_ram.clone();
                                        let before_bytes = guarded_ram.bytes;
                                        let local_stop = AtomicU8::new(stop_value);
                                        let guarded = if is_load {
                                            block.execute_memory_load_v2(
                                                &mut guarded_state,
                                                &mut guarded_ram,
                                                AccessContext::default(),
                                                budget,
                                                stop_rip,
                                                &local_stop,
                                            )
                                        } else {
                                            // SAFETY: same private-RAM fixture guarantees
                                            // as the successful store immediately above.
                                            unsafe {
                                                block.execute_memory_store_v2(
                                                    &mut guarded_state,
                                                    &mut guarded_ram,
                                                    AccessContext::default(),
                                                    budget,
                                                    stop_rip,
                                                    &local_stop,
                                                )
                                            }
                                        }
                                        .unwrap();
                                        assert_eq!(guarded.exit, expected);
                                        assert_eq!(guarded_state, before_state);
                                        assert_eq!(guarded_ram.bytes, before_bytes);
                                        assert_eq!(guarded_ram.reads, 0);
                                        assert_eq!(guarded_ram.write_attempts, 0);
                                        assert_eq!(guarded_ram.writes, 0);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // The decoded EA helper preserves wrapping addition before canonical
        // validation. RAX + RCX*2 + disp8 wraps to canonical address zero.
        let wrapping_code_bytes = [0x48, 0x8b, 0x44, 0x48, 0x04];
        let wrapping_code = CodeView::new(0xa100, &wrapping_code_bytes).unwrap();
        let wrapping_block =
            od_jit::memory_load_v2::compile_memory_load_block_v2(wrapping_code, 0xa100).unwrap();
        let wrapping_published = PublishedCode::publish_memory_load_v2(&wrapping_block).unwrap();
        let mut wrapping_state = od_isa::LongModeState {
            rip: 0xa100,
            ..od_isa::LongModeState::default()
        };
        wrapping_state.gpr[0] = u64::MAX - 7;
        wrapping_state.gpr[1] = 2;
        let mut wrapping_ram = MatrixRam {
            bytes: [0x3c; 512],
            code: [0x48, 0x8b, 0x44, 0x48, 0x04, 0, 0, 0],
            code_len: 5,
            code_rip: 0xa100,
            reads: 0,
            write_attempts: 0,
            writes: 0,
            read_error: None,
            write_error: None,
            stop_on_write: None,
        };
        let mut wrapping_reference_state = wrapping_state.clone();
        let mut wrapping_reference_ram = wrapping_ram.clone();
        let wrapping_stop = AtomicU8::new(0);
        let wrapping_native = wrapping_published
            .execute_memory_load_v2(
                &mut wrapping_state,
                &mut wrapping_ram,
                AccessContext::default(),
                1,
                None,
                &wrapping_stop,
            )
            .unwrap();
        let wrapping_interpreted = od_isa::execute::step(
            &mut wrapping_reference_state,
            &mut wrapping_reference_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert_eq!(
            wrapping_native.exit,
            MemoryOperationExecutionExitV2::Complete
        );
        assert_eq!(wrapping_interpreted, od_isa::execute::StepOutcome::Advanced);
        assert_eq!(wrapping_state, wrapping_reference_state);
        assert_eq!(wrapping_ram.bytes, wrapping_reference_ram.bytes);
        assert_eq!(wrapping_state.gpr[0], u64::from_le_bytes([0x3c; 8]));
        drop(wrapping_published);

        // Exercise every signed displacement boundary through the actual
        // native entry, for direct and SIB addressing and both directions.
        for (opcode, is_load) in [(0x8b, true), (0x89, false)] {
            for use_sib in [false, true] {
                for (mode, displacements) in [
                    (1_u8, vec![-128_i32, -1, 0, 1, 127]),
                    (
                        2_u8,
                        vec![i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX],
                    ),
                ] {
                    for displacement in displacements {
                        let reg = if is_load { 0_u8 } else { 3_u8 };
                        let mut code_vec = vec![
                            0x48,
                            opcode,
                            (mode << 6) | (reg << 3) | if use_sib { 4 } else { 0 },
                        ];
                        if use_sib {
                            code_vec.push((1 << 6) | (1 << 3));
                        }
                        if mode == 1 {
                            code_vec.push(displacement as i8 as u8);
                        } else {
                            code_vec.extend_from_slice(&displacement.to_le_bytes());
                        }
                        let mut code_storage = [0; 8];
                        code_storage[..code_vec.len()].copy_from_slice(&code_vec);
                        let code = CodeView::new(0xa200, &code_vec).unwrap();
                        let published = if is_load {
                            let compiled =
                                od_jit::memory_load_v2::compile_memory_load_block_v2(code, 0xa200)
                                    .unwrap();
                            PublishedCode::publish_memory_load_v2(&compiled).unwrap()
                        } else {
                            let compiled =
                                od_jit::memory_load_v2::compile_memory_store_block_v2(code, 0xa200)
                                    .unwrap();
                            PublishedCode::publish_memory_store_v2(&compiled).unwrap()
                        };
                        let base = if displacement == -128 { 200 } else { 100 };
                        let mut state = od_isa::LongModeState {
                            rip: 0xa200,
                            ..od_isa::LongModeState::default()
                        };
                        state.gpr[0] = base;
                        state.gpr[1] = 0;
                        state.gpr[3] = 0x8877_6655_4433_2211;
                        let mut ram = MatrixRam {
                            bytes: [0xa5; 512],
                            code: code_storage,
                            code_len: code_vec.len(),
                            code_rip: 0xa200,
                            reads: 0,
                            write_attempts: 0,
                            writes: 0,
                            read_error: None,
                            write_error: None,
                            stop_on_write: None,
                        };
                        let mut reference_state = state.clone();
                        let mut reference_ram = ram.clone();
                        let native = if is_load {
                            published.execute_memory_load_v2(
                                &mut state,
                                &mut ram,
                                AccessContext::default(),
                                1,
                                None,
                                &wrapping_stop,
                            )
                        } else {
                            // SAFETY: the RAM fixture remains private and
                            // synchronous, with no executable alias or device effects.
                            unsafe {
                                published.execute_memory_store_v2(
                                    &mut state,
                                    &mut ram,
                                    AccessContext::default(),
                                    1,
                                    None,
                                    &wrapping_stop,
                                )
                            }
                        };
                        let interpreted = od_isa::execute::step(
                            &mut reference_state,
                            &mut reference_ram,
                            AccessContext::default(),
                        );
                        match (&native, &interpreted) {
                            (Ok(result), Ok(od_isa::execute::StepOutcome::Advanced)) => {
                                assert_eq!(result.exit, MemoryOperationExecutionExitV2::Complete);
                            }
                            (
                                Err(MemoryOperationExecutionErrorV2::MemoryError(_)),
                                Err(od_isa::memory::MemoryError::Unmapped { .. }),
                            ) => {}
                            pair => panic!("unexpected native/interpreter pair: {pair:?}"),
                        }
                        assert_eq!(state, reference_state);
                        assert_eq!(ram.bytes, reference_ram.bytes);
                        if let Ok(result) = native {
                            assert_eq!(state.rip, 0xa200 + code_vec.len() as u64);
                            assert_eq!(result.retired_instructions, 1);
                        } else {
                            assert_eq!(state.rip, 0xa200);
                        }
                    }
                }
            }
        }

        let fault_code = CodeView::new(0xa100, &wrapping_code_bytes).unwrap();
        let fault_compiled =
            od_jit::memory_load_v2::compile_memory_load_block_v2(fault_code, 0xa100).unwrap();
        let fault_published = PublishedCode::publish_memory_load_v2(&fault_compiled).unwrap();

        // A canonical base plus disp8 reaches a noncanonical final address,
        // faulting before any helper access or state commit.
        let mut noncanonical_state = od_isa::LongModeState {
            rip: 0xa100,
            ..od_isa::LongModeState::default()
        };
        noncanonical_state.gpr[0] = 0x0000_7fff_ffff_fffc;
        noncanonical_state.gpr[1] = 0;
        let noncanonical_before = noncanonical_state.clone();
        let mut noncanonical_ram = MatrixRam {
            bytes: [0x69; 512],
            code: [0x48, 0x8b, 0x44, 0x48, 0x04, 0, 0, 0],
            code_len: 5,
            code_rip: 0xa100,
            reads: 0,
            write_attempts: 0,
            writes: 0,
            read_error: None,
            write_error: None,
            stop_on_write: None,
        };
        let mut noncanonical_reference_state = noncanonical_state.clone();
        let mut noncanonical_reference_ram = noncanonical_ram.clone();
        let noncanonical_before_ram = noncanonical_ram.bytes;
        let noncanonical_native = fault_published
            .execute_memory_load_v2(
                &mut noncanonical_state,
                &mut noncanonical_ram,
                AccessContext::default(),
                1,
                None,
                &wrapping_stop,
            )
            .unwrap();
        let noncanonical_interpreted = od_isa::execute::step(
            &mut noncanonical_reference_state,
            &mut noncanonical_reference_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert!(matches!(
            noncanonical_native.exit,
            MemoryOperationExecutionExitV2::GuestFault(_)
        ));
        assert!(matches!(
            noncanonical_interpreted,
            od_isa::execute::StepOutcome::Fault(_)
        ));
        assert_eq!(noncanonical_state, noncanonical_before);
        assert_eq!(noncanonical_state, noncanonical_reference_state);
        assert_eq!(noncanonical_ram.bytes, noncanonical_before_ram);
        assert_eq!(noncanonical_ram.bytes, noncanonical_reference_ram.bytes);
        assert_eq!(noncanonical_ram.reads, 0);

        // A checked backend boundary failure from a SIB site also preserves
        // CPU and RAM and remains distinct from an architectural guest fault.
        let mut boundary_state = od_isa::LongModeState {
            rip: 0xa100,
            ..od_isa::LongModeState::default()
        };
        boundary_state.gpr[0] = 500;
        boundary_state.gpr[1] = 500;
        let boundary_before = boundary_state.clone();
        let mut boundary_ram = MatrixRam {
            bytes: [0x96; 512],
            code: [0x48, 0x8b, 0x44, 0x48, 0x04, 0, 0, 0],
            code_len: 5,
            code_rip: 0xa100,
            reads: 0,
            write_attempts: 0,
            writes: 0,
            read_error: None,
            write_error: None,
            stop_on_write: None,
        };
        let mut boundary_reference_state = boundary_state.clone();
        let mut boundary_reference_ram = boundary_ram.clone();
        let boundary_before_ram = boundary_ram.bytes;
        let boundary_native = fault_published.execute_memory_load_v2(
            &mut boundary_state,
            &mut boundary_ram,
            AccessContext::default(),
            1,
            None,
            &wrapping_stop,
        );
        let boundary_interpreted = od_isa::execute::step(
            &mut boundary_reference_state,
            &mut boundary_reference_ram,
            AccessContext::default(),
        );
        assert!(matches!(
            boundary_native,
            Err(MemoryOperationExecutionErrorV2::MemoryError(_))
        ));
        assert!(matches!(
            boundary_interpreted,
            Err(od_isa::memory::MemoryError::Unmapped { .. })
        ));
        assert_eq!(boundary_state, boundary_before);
        assert_eq!(boundary_state, boundary_reference_state);
        assert_eq!(boundary_ram.bytes, boundary_before_ram);
        assert_eq!(boundary_ram.bytes, boundary_reference_ram.bytes);

        // A qword beginning near the architectural endpoint is rejected
        // without invoking GuestMemory, even when the start is canonical.
        let mut endpoint_state = od_isa::LongModeState {
            rip: 0xa100,
            ..od_isa::LongModeState::default()
        };
        endpoint_state.gpr[0] = u64::MAX - 4;
        endpoint_state.gpr[1] = 0;
        let endpoint_before = endpoint_state.clone();
        let mut endpoint_ram = MatrixRam {
            bytes: [0x5a; 512],
            code: [0x48, 0x8b, 0x44, 0x48, 0x04, 0, 0, 0],
            code_len: 5,
            code_rip: 0xa100,
            reads: 0,
            write_attempts: 0,
            writes: 0,
            read_error: None,
            write_error: None,
            stop_on_write: None,
        };
        let mut endpoint_reference_state = endpoint_state.clone();
        let mut endpoint_reference_ram = endpoint_ram.clone();
        let endpoint_before_ram = endpoint_ram.bytes;
        let endpoint_native = fault_published.execute_memory_load_v2(
            &mut endpoint_state,
            &mut endpoint_ram,
            AccessContext::default(),
            1,
            None,
            &wrapping_stop,
        );
        let endpoint_interpreted = od_isa::execute::step(
            &mut endpoint_reference_state,
            &mut endpoint_reference_ram,
            AccessContext::default(),
        );
        assert!(matches!(
            endpoint_native,
            Err(MemoryOperationExecutionErrorV2::MemoryError(_))
        ));
        assert!(matches!(
            endpoint_interpreted,
            Err(od_isa::memory::MemoryError::AddressOverflow { .. })
        ));
        assert_eq!(endpoint_state, endpoint_before);
        assert_eq!(endpoint_state, endpoint_reference_state);
        assert_eq!(endpoint_ram.bytes, endpoint_before_ram);
        assert_eq!(endpoint_ram.bytes, endpoint_reference_ram.bytes);
        assert_eq!(endpoint_ram.reads, 0);
        drop(fault_published);

        // Permission and backend failures stay distinct from guest faults and
        // preserve canonical CPU/RAM state for both load and unsafe store.
        for (opcode, is_load, errors) in [
            (
                0x8b_u8,
                true,
                [
                    od_isa::memory::MemoryError::PermissionDenied {
                        address: 11,
                        access: od_isa::memory::AccessKind::Read,
                    },
                    od_isa::memory::MemoryError::Backend("S3-20d injected read".into()),
                ],
            ),
            (
                0x89_u8,
                false,
                [
                    od_isa::memory::MemoryError::PermissionDenied {
                        address: 11,
                        access: od_isa::memory::AccessKind::Write,
                    },
                    od_isa::memory::MemoryError::Backend("S3-20d injected write".into()),
                ],
            ),
        ] {
            let code_bytes = if is_load {
                [0x48, opcode, 0x44, 0x08, 1]
            } else {
                [0x48, opcode, 0x5c, 0x08, 1]
            };
            let code = CodeView::new(0xa300, &code_bytes).unwrap();
            let published = if is_load {
                let compiled =
                    od_jit::memory_load_v2::compile_memory_load_block_v2(code, 0xa300).unwrap();
                PublishedCode::publish_memory_load_v2(&compiled).unwrap()
            } else {
                let compiled =
                    od_jit::memory_load_v2::compile_memory_store_block_v2(code, 0xa300).unwrap();
                PublishedCode::publish_memory_store_v2(&compiled).unwrap()
            };
            for error in errors {
                let mut state = od_isa::LongModeState {
                    rip: 0xa300,
                    ..od_isa::LongModeState::default()
                };
                state.gpr[0] = 10;
                state.gpr[1] = 0;
                state.gpr[3] = 0x0123_4567_89ab_cdef;
                let before_state = state.clone();
                let mut ram = MatrixRam {
                    bytes: [0xc3; 512],
                    code: [
                        code_bytes[0],
                        code_bytes[1],
                        code_bytes[2],
                        code_bytes[3],
                        code_bytes[4],
                        0,
                        0,
                        0,
                    ],
                    code_len: 5,
                    code_rip: 0xa300,
                    reads: 0,
                    write_attempts: 0,
                    writes: 0,
                    read_error: is_load.then(|| error.clone()),
                    write_error: (!is_load).then(|| error.clone()),
                    stop_on_write: None,
                };
                let before_ram = ram.bytes;
                let mut reference_state = state.clone();
                let mut reference_ram = ram.clone();
                let native = if is_load {
                    published.execute_memory_load_v2(
                        &mut state,
                        &mut ram,
                        AccessContext::default(),
                        1,
                        None,
                        &wrapping_stop,
                    )
                } else {
                    // SAFETY: injected all-or-nothing failures preserve the
                    // same private, synchronous, non-executable RAM guarantees.
                    unsafe {
                        published.execute_memory_store_v2(
                            &mut state,
                            &mut ram,
                            AccessContext::default(),
                            1,
                            None,
                            &wrapping_stop,
                        )
                    }
                };
                let interpreted = od_isa::execute::step(
                    &mut reference_state,
                    &mut reference_ram,
                    AccessContext::default(),
                );
                assert!(matches!(
                    native,
                    Err(MemoryOperationExecutionErrorV2::MemoryError(_))
                ));
                assert_eq!(interpreted, Err(error));
                assert_eq!(state, before_state);
                assert_eq!(state, reference_state);
                assert_eq!(ram.bytes, before_ram);
                assert_eq!(ram.bytes, reference_ram.bytes);
                assert_eq!(ram.reads, usize::from(is_load));
                assert_eq!(ram.write_attempts, usize::from(!is_load));
                assert_eq!(ram.writes, 0);
            }
        }

        // A late STOP caused by the successful SIB store is observed only
        // after one write, RIP/retirement commit, and the memory side effect.
        let late_stop = std::sync::Arc::new(AtomicU8::new(0));
        let store_code_bytes = [0x48, 0x89, 0x5c, 0x08, 1];
        let store_code = CodeView::new(0xa400, &store_code_bytes).unwrap();
        let store_compiled =
            od_jit::memory_load_v2::compile_memory_store_block_v2(store_code, 0xa400).unwrap();
        let store_published = PublishedCode::publish_memory_store_v2(&store_compiled).unwrap();
        let mut late_state = od_isa::LongModeState {
            rip: 0xa400,
            ..od_isa::LongModeState::default()
        };
        late_state.gpr[0] = 10;
        late_state.gpr[1] = 0;
        late_state.gpr[3] = 0x8877_6655_4433_2211;
        let mut late_ram = MatrixRam {
            bytes: [0; 512],
            code: [0x48, 0x89, 0x5c, 0x08, 1, 0, 0, 0],
            code_len: 5,
            code_rip: 0xa400,
            reads: 0,
            write_attempts: 0,
            writes: 0,
            read_error: None,
            write_error: None,
            stop_on_write: Some(late_stop.clone()),
        };
        let mut late_reference_state = late_state.clone();
        let mut late_reference_ram = late_ram.clone();
        late_reference_ram.stop_on_write = None;
        // SAFETY: same private synchronous RAM fixture; its successful write
        // is all-or-nothing and remains non-executable throughout this call.
        let late_native = unsafe {
            store_published.execute_memory_store_v2(
                &mut late_state,
                &mut late_ram,
                AccessContext::default(),
                1,
                None,
                &late_stop,
            )
        }
        .unwrap();
        let late_interpreted = od_isa::execute::step(
            &mut late_reference_state,
            &mut late_reference_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert_eq!(
            late_native.exit,
            MemoryOperationExecutionExitV2::StopRequested
        );
        assert_eq!(late_interpreted, od_isa::execute::StepOutcome::Advanced);
        assert_eq!(late_state, late_reference_state);
        assert_eq!(late_ram.bytes, late_reference_ram.bytes);
        assert_eq!(late_ram.write_attempts, 1);
        assert_eq!(late_ram.writes, 1);
        assert_eq!(
            &late_ram.bytes[11..19],
            &0x8877_6655_4433_2211_u64.to_le_bytes()
        );
        drop(store_published);

        // RIP-relative operands use each invocation's guest RIP as the
        // decoder-provided NextRip base. The same bytes are compiled at two
        // sites and independently compared with the committed interpreter.
        for (opcode, is_load) in [(0x8b_u8, true), (0x89_u8, false)] {
            for register in 0..4_u8 {
                for displacement in [i32::MIN, -8, -7, -1, 0, 1, 8, i32::MAX] {
                    for code_rip in [0xa500_u64, 0xa600_u64] {
                        let mut instruction = vec![0x48, opcode, (register << 3) | 5];
                        instruction.extend_from_slice(&displacement.to_le_bytes());
                        let mut code_storage = [0; 8];
                        code_storage[..7].copy_from_slice(&instruction);
                        let code = CodeView::new(code_rip, &instruction).unwrap();
                        let published = if is_load {
                            let compiled = od_jit::memory_load_v2::compile_memory_load_block_v2(
                                code, code_rip,
                            )
                            .unwrap();
                            PublishedCode::publish_memory_load_v2(&compiled).unwrap()
                        } else {
                            let compiled = od_jit::memory_load_v2::compile_memory_store_block_v2(
                                code, code_rip,
                            )
                            .unwrap();
                            PublishedCode::publish_memory_store_v2(&compiled).unwrap()
                        };
                        let mut initial_state = od_isa::LongModeState {
                            rip: code_rip,
                            rflags: 0x8d7,
                            ..od_isa::LongModeState::default()
                        };
                        initial_state.gpr[..4].copy_from_slice(&[
                            0x1122_3344_5566_7788,
                            20,
                            24,
                            28,
                        ]);
                        let mut initial_ram = MatrixRam {
                            bytes: [0x6b; 512],
                            code: code_storage,
                            code_len: 7,
                            code_rip,
                            reads: 0,
                            write_attempts: 0,
                            writes: 0,
                            read_error: None,
                            write_error: None,
                            stop_on_write: None,
                        };
                        let mut native_state = initial_state.clone();
                        let mut interpreted_state = initial_state.clone();
                        let mut native_ram = initial_ram.clone();
                        let mut interpreted_ram = initial_ram.clone();
                        let native = if is_load {
                            published.execute_memory_load_v2(
                                &mut native_state,
                                &mut native_ram,
                                AccessContext::default(),
                                1,
                                None,
                                &stop,
                            )
                        } else {
                            // SAFETY: fixture writes are synchronous, all-or-nothing,
                            // non-executable, and deny every overlap with source code.
                            unsafe {
                                published.execute_memory_store_v2(
                                    &mut native_state,
                                    &mut native_ram,
                                    AccessContext::default(),
                                    1,
                                    None,
                                    &stop,
                                )
                            }
                        };
                        let interpreted = od_isa::execute::step(
                            &mut interpreted_state,
                            &mut interpreted_ram,
                            AccessContext::default(),
                        );
                        match (&native, &interpreted) {
                            (Ok(result), Ok(od_isa::execute::StepOutcome::Advanced)) => {
                                assert_eq!(result.exit, MemoryOperationExecutionExitV2::Complete);
                            }
                            (
                                Err(MemoryOperationExecutionErrorV2::MemoryError(_)),
                                Err(
                                    od_isa::memory::MemoryError::Unmapped { .. }
                                    | od_isa::memory::MemoryError::PermissionDenied { .. },
                                ),
                            ) => {}
                            pair => {
                                panic!("unexpected RIP-relative native/interpreter pair: {pair:?}")
                            }
                        }
                        assert_eq!(native_state, interpreted_state);
                        assert_eq!(native_ram.bytes, interpreted_ram.bytes);
                        if let Ok(result) = native {
                            assert_eq!(native_state.rip, code_rip + 7);
                            assert_eq!(result.retired_instructions, 1);
                            if !is_load {
                                let target =
                                    (code_rip + 7).wrapping_add_signed(i64::from(displacement));
                                assert_eq!(
                                    &native_ram.bytes[target as usize..target as usize + 8],
                                    &initial_state.gpr[register as usize].to_le_bytes()
                                );
                            }
                        } else {
                            assert_eq!(native_state.rip, code_rip);
                            assert_eq!(native_ram.writes, 0);
                        }
                        drop(published);
                        initial_ram.reads = 0;
                    }
                }
            }
        }

        let stale_code_bytes = [0x48, 0x8b, 0x0d, 0x20, 0, 0, 0];
        let stale_code = CodeView::new(0xa700, &stale_code_bytes).unwrap();
        let stale_compiled =
            od_jit::memory_load_v2::compile_memory_load_block_v2(stale_code, 0xa700).unwrap();
        let stale_published = PublishedCode::publish_memory_load_v2(&stale_compiled).unwrap();
        let mut stale_state = od_isa::LongModeState {
            rip: 0xa701,
            ..od_isa::LongModeState::default()
        };
        let stale_before = stale_state.clone();
        let mut stale_ram = MatrixRam {
            bytes: [0x47; 512],
            code: [0x48, 0x8b, 0x0d, 0x20, 0, 0, 0, 0],
            code_len: 7,
            code_rip: 0xa700,
            reads: 0,
            write_attempts: 0,
            writes: 0,
            read_error: None,
            write_error: None,
            stop_on_write: None,
        };
        let stale = stale_published.execute_memory_load_v2(
            &mut stale_state,
            &mut stale_ram,
            AccessContext::default(),
            1,
            None,
            &stop,
        );
        assert!(matches!(
            stale,
            Err(MemoryOperationExecutionErrorV2::EntryRipMismatch { .. })
        ));
        assert_eq!(stale_state, stale_before);
        assert_eq!(stale_ram.reads, 0);
        drop(stale_published);

        // A successful RIP-relative store commits once before observing a
        // STOP raised by the private synchronous RAM write.
        let late_rip_stop = std::sync::Arc::new(AtomicU8::new(0));
        let late_rip_code_bytes = [0x48, 0x89, 0x1d, 0x20, 0, 0, 0];
        let late_rip_code = CodeView::new(0x100, &late_rip_code_bytes).unwrap();
        let late_rip_compiled =
            od_jit::memory_load_v2::compile_memory_store_block_v2(late_rip_code, 0x100).unwrap();
        let late_rip_published =
            PublishedCode::publish_memory_store_v2(&late_rip_compiled).unwrap();
        let mut late_rip_state = od_isa::LongModeState {
            rip: 0x100,
            rflags: 0x8d7,
            ..od_isa::LongModeState::default()
        };
        late_rip_state.gpr[3] = 0x8877_6655_4433_2211;
        let mut late_rip_ram = MatrixRam {
            bytes: [0x5a; 512],
            code: [0x48, 0x89, 0x1d, 0x20, 0, 0, 0, 0],
            code_len: 7,
            code_rip: 0x100,
            reads: 0,
            write_attempts: 0,
            writes: 0,
            read_error: None,
            write_error: None,
            stop_on_write: Some(late_rip_stop.clone()),
        };
        let mut late_rip_reference_state = late_rip_state.clone();
        let mut late_rip_reference_ram = late_rip_ram.clone();
        late_rip_reference_ram.stop_on_write = None;
        let late_rip_native = unsafe {
            late_rip_published.execute_memory_store_v2(
                &mut late_rip_state,
                &mut late_rip_ram,
                AccessContext::default(),
                1,
                None,
                &late_rip_stop,
            )
        }
        .unwrap();
        let late_rip_interpreted = od_isa::execute::step(
            &mut late_rip_reference_state,
            &mut late_rip_reference_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert_eq!(
            late_rip_native.exit,
            MemoryOperationExecutionExitV2::StopRequested
        );
        assert_eq!(late_rip_interpreted, od_isa::execute::StepOutcome::Advanced);
        assert_eq!(late_rip_state, late_rip_reference_state);
        assert_eq!(late_rip_ram.bytes, late_rip_reference_ram.bytes);
        assert_eq!(late_rip_ram.write_attempts, 1);
        assert_eq!(late_rip_ram.writes, 1);
        assert_eq!(
            &late_rip_ram.bytes[0x127..0x12f],
            &0x8877_6655_4433_2211_u64.to_le_bytes()
        );
        drop(late_rip_published);

        // Decoder-derived NextRip plus disp32 reaches the first
        // noncanonical address; reject it before any GuestMemory access.
        let noncanonical_rip = 0x0000_7fff_ffff_fff8_u64;
        let noncanonical_code_bytes = [0x48, 0x8b, 0x0d, 1, 0, 0, 0];
        let noncanonical_code = CodeView::new(noncanonical_rip, &noncanonical_code_bytes).unwrap();
        let noncanonical_compiled = od_jit::memory_load_v2::compile_memory_load_block_v2(
            noncanonical_code,
            noncanonical_rip,
        )
        .unwrap();
        let noncanonical_published =
            PublishedCode::publish_memory_load_v2(&noncanonical_compiled).unwrap();
        let mut noncanonical_state = od_isa::LongModeState {
            rip: noncanonical_rip,
            ..od_isa::LongModeState::default()
        };
        let before_noncanonical_state = noncanonical_state.clone();
        let mut noncanonical_ram = MatrixRam {
            bytes: [0x31; 512],
            code: [0x48, 0x8b, 0x0d, 1, 0, 0, 0, 0],
            code_len: 7,
            code_rip: noncanonical_rip,
            reads: 0,
            write_attempts: 0,
            writes: 0,
            read_error: None,
            write_error: None,
            stop_on_write: None,
        };
        let mut noncanonical_reference_state = noncanonical_state.clone();
        let mut noncanonical_reference_ram = noncanonical_ram.clone();
        let noncanonical_native = noncanonical_published
            .execute_memory_load_v2(
                &mut noncanonical_state,
                &mut noncanonical_ram,
                AccessContext::default(),
                1,
                None,
                &stop,
            )
            .unwrap();
        let noncanonical_interpreted = od_isa::execute::step(
            &mut noncanonical_reference_state,
            &mut noncanonical_reference_ram,
            AccessContext::default(),
        )
        .unwrap();
        assert!(matches!(
            noncanonical_native.exit,
            MemoryOperationExecutionExitV2::GuestFault(_)
        ));
        assert!(matches!(
            noncanonical_interpreted,
            od_isa::execute::StepOutcome::Fault(_)
        ));
        assert_eq!(noncanonical_state, before_noncanonical_state);
        assert_eq!(noncanonical_state, noncanonical_reference_state);
        assert_eq!(noncanonical_ram.bytes, noncanonical_reference_ram.bytes);
        assert_eq!(noncanonical_ram.reads, 0);
    }

    #[test]
    fn v2_rip_relative_mov_success_targets_follow_guest_rip() {
        use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

        struct RipRam<'a> {
            bytes: [u8; 512],
            source: &'a [u8; 7],
            code_rips: [u64; 2],
            writes: usize,
        }
        impl GuestMemory for RipRam<'_> {
            fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
                for rip in self.code_rips {
                    if (rip..rip + 7).contains(&address) {
                        return Ok(self.source[(address - rip) as usize]);
                    }
                }
                self.bytes
                    .get(address as usize)
                    .copied()
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Fetch,
                    })
            }

            fn read(
                &mut self,
                address: u64,
                _: AccessContext,
                destination: &mut [u8],
            ) -> Result<(), MemoryError> {
                let start = address as usize;
                let source = self.bytes.get(start..start + destination.len()).ok_or(
                    MemoryError::Unmapped {
                        address,
                        access: AccessKind::Read,
                    },
                )?;
                destination.copy_from_slice(source);
                Ok(())
            }

            fn write(
                &mut self,
                address: u64,
                _: AccessContext,
                source: &[u8],
            ) -> Result<(), MemoryError> {
                let start = address as usize;
                let destination = self.bytes.get_mut(start..start + source.len()).ok_or(
                    MemoryError::Unmapped {
                        address,
                        access: AccessKind::Write,
                    },
                )?;
                destination.copy_from_slice(source);
                self.writes += 1;
                Ok(())
            }

            fn atomic_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn atomic_fetch_add_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn atomic_compare_exchange_integer(
                &mut self,
                address: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
                _: u64,
            ) -> Result<u64, MemoryError> {
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })
            }

            fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let _guard = jit_test_lock();
        let stop = AtomicU8::new(0);
        for (opcode, is_load) in [(0x8b_u8, true), (0x89_u8, false)] {
            let register = if is_load { 1_u8 } else { 0_u8 };
            let source_bytes = [0x48, opcode, (register << 3) | 5, 0x20, 0, 0, 0];
            let code_rips = [0x100_u64, 0x180_u64];
            for code_rip in code_rips {
                let code = CodeView::new(code_rip, &source_bytes).unwrap();
                let published = if is_load {
                    let compiled =
                        od_jit::memory_load_v2::compile_memory_load_block_v2(code, code_rip)
                            .unwrap();
                    PublishedCode::publish_memory_load_v2(&compiled).unwrap()
                } else {
                    let compiled =
                        od_jit::memory_load_v2::compile_memory_store_block_v2(code, code_rip)
                            .unwrap();
                    PublishedCode::publish_memory_store_v2(&compiled).unwrap()
                };
                let target = code_rip + 7 + 0x20;
                let other_rip = if code_rip == code_rips[0] {
                    code_rips[1]
                } else {
                    code_rips[0]
                };
                let other_target = other_rip + 7 + 0x20;
                let mut initial_state = od_isa::LongModeState {
                    rip: code_rip,
                    rflags: 0x8d7,
                    ..od_isa::LongModeState::default()
                };
                initial_state.gpr[0] = 0x8877_6655_4433_2211;
                let mut initial_ram = RipRam {
                    bytes: [0x5a; 512],
                    source: &source_bytes,
                    code_rips,
                    writes: 0,
                };
                initial_ram.bytes[target as usize..target as usize + 8].copy_from_slice(
                    &if code_rip == code_rips[0] {
                        0x1111_2222_3333_4444_u64.to_le_bytes()
                    } else {
                        0xaaaa_bbbb_cccc_dddd_u64.to_le_bytes()
                    },
                );
                initial_ram.bytes[other_target as usize..other_target as usize + 8]
                    .copy_from_slice(&0xdead_beef_cafe_f00d_u64.to_le_bytes());
                let before_bytes = initial_ram.bytes;
                let mut native_state = initial_state.clone();
                let mut interpreter_state = initial_state;
                let mut native_ram = RipRam {
                    bytes: initial_ram.bytes,
                    source: &source_bytes,
                    code_rips,
                    writes: 0,
                };
                let mut interpreter_ram = RipRam {
                    bytes: initial_ram.bytes,
                    source: &source_bytes,
                    code_rips,
                    writes: 0,
                };
                let native = if is_load {
                    published.execute_memory_load_v2(
                        &mut native_state,
                        &mut native_ram,
                        AccessContext::default(),
                        1,
                        None,
                        &stop,
                    )
                } else {
                    // SAFETY: code is an immutable snapshot held outside the
                    // mutable private data RAM; targets are disjoint from it.
                    unsafe {
                        published.execute_memory_store_v2(
                            &mut native_state,
                            &mut native_ram,
                            AccessContext::default(),
                            1,
                            None,
                            &stop,
                        )
                    }
                }
                .unwrap();
                let interpreted = od_isa::execute::step(
                    &mut interpreter_state,
                    &mut interpreter_ram,
                    AccessContext::default(),
                )
                .unwrap();
                assert_eq!(native.exit, MemoryOperationExecutionExitV2::Complete);
                assert_eq!(interpreted, od_isa::execute::StepOutcome::Advanced);
                assert_eq!(native_state, interpreter_state);
                assert_eq!(native_ram.bytes, interpreter_ram.bytes);
                assert_eq!(native_state.rip, code_rip + 7);
                assert_eq!(native.retired_instructions, 1);
                if is_load {
                    assert_eq!(
                        native_state.gpr[1],
                        if code_rip == code_rips[0] {
                            0x1111_2222_3333_4444
                        } else {
                            0xaaaa_bbbb_cccc_dddd
                        }
                    );
                    assert_eq!(native_ram.bytes, before_bytes);
                    assert_eq!(native_ram.writes, 0);
                } else {
                    let mut expected = before_bytes;
                    expected[target as usize..target as usize + 8]
                        .copy_from_slice(&0x8877_6655_4433_2211_u64.to_le_bytes());
                    assert_eq!(native_ram.bytes, expected);
                    assert_eq!(native_ram.writes, 1);
                    assert_eq!(native_state.gpr, interpreter_state.gpr);
                    assert_eq!(native_state.rflags, interpreter_state.rflags);
                }
                drop(published);
            }
        }
    }

    const PROBE_HELPER_SENTINEL: u64 = 0xD15E_A5E0_CAFE_BEEF;
    const PROBE_REGISTER_MISMATCH: u64 = 0xA100;
    static PROBE_HELPER_STATUS: AtomicU32 = AtomicU32::new(0);
    static PROBE_NESTED_CALLS: AtomicUsize = AtomicUsize::new(0);
    static mut ALLOCATOR_PROBE_OUTPUT: u64 = 0;

    // Keep x29 as a valid frame pointer: it points at this wrapper's saved
    // x29/x30 record while the generated entry runs.
    core::arch::global_asm!(
        ".text",
        ".globl _od_jit_allocator_call_with_gpr_canaries",
        "_od_jit_allocator_call_with_gpr_canaries:",
        "stp x19, x20, [sp, #-16]!",
        "stp x21, x22, [sp, #-16]!",
        "stp x23, x24, [sp, #-16]!",
        "stp x25, x26, [sp, #-16]!",
        "stp x27, x28, [sp, #-16]!",
        "stp x29, x30, [sp, #-16]!",
        "sub sp, sp, #16",
        "mov x12, sp",
        "str x12, [sp]",
        "mov x16, x0",
        "mov x0, x1",
        "mov x19, #0xc019",
        "mov x20, #0xc020",
        "mov x21, #0xc021",
        "mov x22, #0xc022",
        "mov x23, #0xc023",
        "mov x24, #0xc024",
        "mov x25, #0xc025",
        "mov x26, #0xc026",
        "mov x27, #0xc027",
        "mov x28, #0xc028",
        "add x29, sp, #16",
        "blr x16",
        "ldr x12, [sp]",
        "mov x13, sp",
        "cmp x12, x13",
        "b.ne 1f",
        "add x12, sp, #16",
        "cmp x29, x12",
        "b.ne 1f",
        "mov x12, #0xc019",
        "cmp x19, x12",
        "b.ne 1f",
        "mov x12, #0xc020",
        "cmp x20, x12",
        "b.ne 1f",
        "mov x12, #0xc021",
        "cmp x21, x12",
        "b.ne 1f",
        "mov x12, #0xc022",
        "cmp x22, x12",
        "b.ne 1f",
        "mov x12, #0xc023",
        "cmp x23, x12",
        "b.ne 1f",
        "mov x12, #0xc024",
        "cmp x24, x12",
        "b.ne 1f",
        "mov x12, #0xc025",
        "cmp x25, x12",
        "b.ne 1f",
        "mov x12, #0xc026",
        "cmp x26, x12",
        "b.ne 1f",
        "mov x12, #0xc027",
        "cmp x27, x12",
        "b.ne 1f",
        "mov x12, #0xc028",
        "cmp x28, x12",
        "b.ne 1f",
        "b 2f",
        "1:",
        "mov x0, #0",
        "2:",
        "add sp, sp, #16",
        "ldp x29, x30, [sp], #16",
        "ldp x27, x28, [sp], #16",
        "ldp x25, x26, [sp], #16",
        "ldp x23, x24, [sp], #16",
        "ldp x21, x22, [sp], #16",
        "ldp x19, x20, [sp], #16",
        "ret",
    );

    unsafe extern "C" {
        fn od_jit_allocator_call_with_gpr_canaries(entry: usize, context: usize) -> u32;
    }

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

    fn allocator_probe_program() -> Program {
        let mut operations = vec![Operation::BeginInstruction { index: 0 }];
        for temp in 0u32..6 {
            operations.push(Operation::Constant {
                dst: TempId(temp),
                width: IntWidth::W64,
                value: temp as u64 + 1,
            });
        }
        let helper_boundary = operations.len();
        for temp in 0u32..6 {
            operations.push(Operation::WriteGpr {
                register: GuestGpr((temp % 4) as u8),
                src: TempId(temp),
                width: IntWidth::W64,
            });
        }
        operations.push(Operation::CommitInstruction { index: 0 });
        operations.push(Operation::Exit {
            kind: ExitKind::BlockComplete,
            target_rip: 0x2001,
        });
        assert_eq!(helper_boundary, 7);
        Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: 0x2000,
                next_rip: 0x2001,
            }],
            operations,
            code_end: 0x2001,
        }
    }

    fn emit_allocator_helper_probe(helper: ArithmeticFlagsHelper) -> Vec<u8> {
        let program = allocator_probe_program();
        let allocation = Allocation::build(
            &program,
            AllocatorConfig {
                register_homes: 4,
                reserved_prefix_bytes: 0,
            },
            &[HelperBoundaryPosition::BeforeOperation(7)],
        )
        .unwrap();
        assert!(matches!(
            allocation.location(TempId(4)),
            Ok(ValueLocation::Spill(_))
        ));
        assert!(matches!(
            allocation.location(TempId(5)),
            Ok(ValueLocation::Spill(_))
        ));
        assert_eq!(allocation.frame().frame_bytes, 48);

        let mut encoder = Encoder::new();
        for first in [19, 21, 23, 25, 27] {
            emit_pair_push(&mut encoder, first, first + 1);
        }
        emit_pair_push(&mut encoder, 29, 30);
        // Three pairs reserve the allocator's exact 48-byte frame. Their old
        // values are caller-saved and do not need to survive the probe.
        emit_pair_push(&mut encoder, 13, 14);
        emit_pair_push(&mut encoder, 15, 16);
        emit_pair_push(&mut encoder, 17, 20);

        for temp in 0..4 {
            encoder.movz_x(xr(9 + temp), (temp + 1) as u16, 0).unwrap();
        }
        for (temp, value) in [(4, 5), (5, 6)] {
            encoder.movz_x(xr(13), value, 0).unwrap();
            allocation
                .emit_store_scratch(&mut encoder, TempId(temp), xr(13))
                .unwrap();
        }

        allocation
            .emit_save_before_helper(&mut encoder, HelperBoundaryPosition::BeforeOperation(7))
            .unwrap();
        encoder.movz_x(xr(0), 1, 0).unwrap(); // helper operation
        encoder.movz_x(xr(1), 0x1234, 0).unwrap();
        encoder.movz_x(xr(2), 0x5678, 0).unwrap();
        encoder.movz_x(xr(3), 0x9ABC, 0).unwrap();
        emit_u64(
            &mut encoder,
            4,
            std::ptr::addr_of_mut!(ALLOCATOR_PROBE_OUTPUT) as usize as u64,
        );
        emit_u64(&mut encoder, 16, helper as usize as u64);
        encoder.blr(xr(16));
        allocation
            .emit_restore_after_helper(&mut encoder, HelperBoundaryPosition::BeforeOperation(7))
            .unwrap();

        allocation
            .emit_load_scratch(&mut encoder, TempId(4), xr(13))
            .unwrap();
        allocation
            .emit_load_scratch(&mut encoder, TempId(5), xr(14))
            .unwrap();
        encoder.mov_x(xr(0), XRegOrZero::Reg(xr(9)));
        for register in [10, 11, 12, 13, 14] {
            encoder.add_x(xr(0), xr(0), XRegOrZero::Reg(xr(register)));
        }

        emit_pair_pop(&mut encoder, 17, 20);
        emit_pair_pop(&mut encoder, 15, 16);
        emit_pair_pop(&mut encoder, 13, 14);
        emit_pair_pop(&mut encoder, 29, 30);
        for first in [27, 25, 23, 21, 19] {
            emit_pair_pop(&mut encoder, first, first + 1);
        }
        encoder.ret(xr(30));
        encoder.finish().unwrap().to_le_bytes().unwrap()
    }

    fn emit_decoder_data_effects(
        code: &[u8],
        register_homes: u8,
        helper: ArithmeticFlagsHelper,
    ) -> (Vec<u8>, Vec<DataEffectOutputKind>, DataEffectTerminal) {
        let base_rip = 0x3000;
        let view = CodeView::new(base_rip, code).unwrap();
        let built = block::build_block(
            view,
            base_rip,
            od_isa::memory::AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let LoweringOutcome::Program(lowered) = lowering::lower(&built).unwrap() else {
            panic!("native data-effect test expects an admitted block prefix");
        };
        let helper_boundaries: Vec<_> = lowered
            .program
            .operations
            .iter()
            .enumerate()
            .filter_map(|(index, operation)| {
                matches!(operation, Operation::Integer { .. })
                    .then_some(HelperBoundaryPosition::BeforeOperation(index))
            })
            .collect();
        let allocation = Allocation::build(
            &lowered.program,
            AllocatorConfig {
                register_homes,
                reserved_prefix_bytes: 48,
            },
            &helper_boundaries,
        )
        .unwrap();
        let emitted = emitter::emit_data_effects(&lowered.program, &allocation, helper).unwrap();
        assert_eq!(emitted.flags, FlagMaterialization::ArithmeticComplete);
        assert_eq!(emitted.committed_guest_instructions, 0);
        let output_kinds = emitted.outputs.iter().map(|output| output.kind).collect();
        (
            emitted.code.to_le_bytes().unwrap(),
            output_kinds,
            emitted.terminal,
        )
    }

    fn emit_runtime_block_for_code(
        code: &[u8],
        base_rip: u64,
        register_homes: u8,
    ) -> od_jit::block_emitter::EmittedRuntimeBlock {
        let view = CodeView::new(base_rip, code).unwrap();
        let built = block::build_block(
            view,
            base_rip,
            od_isa::memory::AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let LoweringOutcome::Program(lowered) = lowering::lower(&built).unwrap() else {
            panic!("runtime block test expects a nonempty admitted prefix");
        };
        let helper_boundaries: Vec<_> = lowered
            .program
            .operations
            .iter()
            .enumerate()
            .filter_map(|(index, operation)| {
                matches!(operation, Operation::Integer { .. })
                    .then_some(HelperBoundaryPosition::BeforeOperation(index))
            })
            .collect();
        let allocation = Allocation::build(
            &lowered.program,
            AllocatorConfig {
                register_homes,
                reserved_prefix_bytes: 112,
            },
            &helper_boundaries,
        )
        .unwrap();
        od_jit::block_emitter::emit_runtime_block(&lowered.program, &allocation).unwrap()
    }

    unsafe fn emit_runtime_block_for_code_with_helper(
        code: &[u8],
        base_rip: u64,
        register_homes: u8,
        helper: ArithmeticFlagsHelper,
    ) -> od_jit::block_emitter::EmittedRuntimeBlock {
        let view = CodeView::new(base_rip, code).unwrap();
        let built = block::build_block(
            view,
            base_rip,
            od_isa::memory::AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let LoweringOutcome::Program(lowered) = lowering::lower(&built).unwrap() else {
            panic!("runtime helper test expects an admitted prefix");
        };
        let helper_boundaries: Vec<_> = lowered
            .program
            .operations
            .iter()
            .enumerate()
            .filter_map(|(index, operation)| {
                matches!(operation, Operation::Integer { .. })
                    .then_some(HelperBoundaryPosition::BeforeOperation(index))
            })
            .collect();
        let allocation = Allocation::build(
            &lowered.program,
            AllocatorConfig {
                register_homes,
                reserved_prefix_bytes: 112,
            },
            &helper_boundaries,
        )
        .unwrap();
        // SAFETY: delegated to this function's caller, who invokes the result
        // only with a helper that meets `emit_runtime_block_with_helper`'s ABI.
        unsafe {
            od_jit::block_emitter::emit_runtime_block_with_helper(
                &lowered.program,
                &allocation,
                helper,
            )
        }
        .unwrap()
    }

    unsafe extern "C" fn runtime_test_helper(
        operation: u32,
        lhs: u64,
        rhs: u64,
        old_rflags: u64,
        output: *mut u64,
    ) -> u32 {
        let call = RUNTIME_HELPER_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
        if call == RUNTIME_HELPER_FAIL_AT.load(Ordering::Relaxed) {
            return RUNTIME_HELPER_FAILURE_STATUS.load(Ordering::Relaxed);
        }
        // SAFETY: generated code supplies a live frame-local output slot.
        let status = unsafe {
            od_jit::runtime::arithmetic_flags_helper_adapter(
                operation, lhs, rhs, old_rflags, output,
            )
        };
        if status == 0 && call == RUNTIME_HELPER_STOP_ON_CALL.load(Ordering::Relaxed) {
            let pointer = RUNTIME_HELPER_STOP_BYTE.load(Ordering::Acquire);
            if !pointer.is_null() {
                // SAFETY: the test stores a pointer to its live stack AtomicU8
                // while holding JIT_TEST_LOCK and resets it before returning.
                unsafe { (*pointer).store(1, Ordering::Release) };
            }
        }
        status
    }

    unsafe extern "C" fn data_effect_flags_helper(
        operation: u32,
        lhs: u64,
        rhs: u64,
        old_rflags: u64,
        output: *mut u64,
    ) -> u32 {
        // SAFETY: the generated caller passes a live stack output slot.
        let status = unsafe {
            od_jit::runtime::arithmetic_flags_helper_adapter(
                operation, lhs, rhs, old_rflags, output,
            )
        };
        // The helper boundary must reload base pointers and live allocator
        // homes after all caller-saved x7-x17 registers have been clobbered.
        // SAFETY: all named outputs are caller-saved AAPCS64 registers.
        unsafe {
            std::arch::asm!(
                "mov x7, xzr", "mov x8, xzr", "mov x9, xzr", "mov x10, xzr",
                "mov x11, xzr", "mov x12, xzr", "mov x13, xzr", "mov x14, xzr",
                "mov x15, xzr", "mov x16, xzr", "mov x17, xzr",
                lateout("x7") _, lateout("x8") _, lateout("x9") _, lateout("x10") _,
                lateout("x11") _, lateout("x12") _, lateout("x13") _, lateout("x14") _,
                lateout("x15") _, lateout("x16") _, lateout("x17") _,
                options(nostack)
            );
        }
        status
    }

    unsafe extern "C" fn data_effect_failing_helper(
        _operation: u32,
        _lhs: u64,
        _rhs: u64,
        _old_rflags: u64,
        output: *mut u64,
    ) -> u32 {
        if !output.is_null() {
            // SAFETY: test code is called with the emitter's live, aligned
            // helper-output slot and deliberately scribbles it on failure.
            unsafe { output.write(0xBAD0_F17E_DEAD_BEEF) };
        }
        DATA_EFFECT_HELPER_STATUS.load(Ordering::Relaxed)
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
    fn allocated_register_and_spill_values_survive_helper_clobbers() {
        let _guard = jit_test_lock();
        PROBE_HELPER_STATUS.store(0, Ordering::Relaxed);
        PROBE_NESTED_CALLS.store(0, Ordering::Relaxed);
        *PROBE_OBSERVATION.lock().unwrap() = None;
        // SAFETY: this test owns the global probe output under JIT_TEST_LOCK;
        // the generated helper receives its exact address and writes one u64.
        unsafe { std::ptr::addr_of_mut!(ALLOCATOR_PROBE_OUTPUT).write(0) };

        let code = emit_allocator_helper_probe(abi_probe_helper);
        let published = PublishedCode::publish(&code).unwrap();
        // SAFETY: code is emitted from the typed Encoder, saves the AAPCS64
        // callee-saved frame, and returns the sum in w0 under BlockEntry ABI.
        let entry: BlockEntry = unsafe { std::mem::transmute(published.as_ptr()) };
        let mut state = probe_state();
        let stop = AtomicU8::new(0);
        let mut context = make_probe_context(&mut state, &stop);
        let (raw, simd_values) = call_probe_with_simd_canaries(entry, &mut context);
        // SAFETY: the entry points into `published`; the assembly wrapper
        // seeds/checks x19-x29 and SP, then restores the wrapper's own frame.
        let gpr_raw = unsafe {
            od_jit_allocator_call_with_gpr_canaries(
                entry as usize,
                (&mut context as *mut RuntimeContextV1) as usize,
            )
        };

        assert_eq!(raw, 21);
        assert_eq!(gpr_raw, 21);
        assert_eq!(
            simd_values,
            [
                0xD008, 0xD009, 0xD00A, 0xD00B, 0xD00C, 0xD00D, 0xD00E, 0xD00F
            ]
        );
        assert_eq!(PROBE_NESTED_CALLS.load(Ordering::Relaxed), 2);
        let observation = PROBE_OBSERVATION.lock().unwrap().unwrap();
        assert_eq!(observation.lhs, 0x1234);
        assert_eq!(observation.rhs, 0x5678);
        assert_eq!(observation.flags, 0x9ABC);
        // SAFETY: the test lock excludes concurrent access to this probe cell.
        assert_eq!(
            unsafe { std::ptr::addr_of!(ALLOCATOR_PROBE_OUTPUT).read() },
            abi_probe_nested(0x1234 ^ 0x5678 ^ 0x9ABC)
        );
    }

    #[test]
    fn decoder_lowering_allocator_and_data_effect_emitter_match_vectors_without_commit() {
        let _guard = jit_test_lock();
        let mut guest_code = vec![0x48, 0xB8];
        guest_code.extend_from_slice(&u64::MAX.to_le_bytes()); // MOV RAX, -1
        guest_code.extend_from_slice(&[0x48, 0x03, 0xC1]); // ADD RAX, RCX
        guest_code.extend_from_slice(&[0x48, 0x2B, 0xD2]); // SUB RDX, RDX
        guest_code.extend_from_slice(&[0x48, 0x2B, 0xD1]); // SUB RDX, RCX (borrow/wrap)
        guest_code.extend_from_slice(&[0x48, 0xBB]); // MOV RBX, multi-lane immediate
        guest_code.extend_from_slice(&0x1234_5678_9ABC_DEF0_u64.to_le_bytes());
        guest_code.extend_from_slice(&[0x48, 0x03, 0xDB]); // ADD RBX, RBX (alias)
        guest_code.extend_from_slice(&[0x48, 0x3B, 0xDA]); // CMP RBX, RDX

        let initial_gprs = [0x1111, 1, 0x8000_0000_0000_0001, 0xAAAA];
        let pattern = 0x1234_5678_9ABC_DEF0_u64;
        let doubled = pattern.wrapping_add(pattern);
        let expected_temps = [
            u64::MAX,
            u64::MAX,
            1,
            0,
            initial_gprs[2],
            initial_gprs[2],
            0,
            0,
            1,
            u64::MAX,
            pattern,
            pattern,
            pattern,
            doubled,
            doubled,
            u64::MAX,
        ];
        let mut canonical_state = od_isa::LongModeState::default();
        canonical_state.gpr[..initial_gprs.len()].copy_from_slice(&initial_gprs);
        canonical_state.rip = 0xAABB_CCDD;
        let old_flags = 0xA5A5_F0F0_5A5A_0A0A;
        canonical_state.rflags = old_flags;
        let original_canonical = canonical_state.clone();

        for register_homes in 0..=4 {
            let (code, outputs, terminal) =
                emit_decoder_data_effects(&guest_code, register_homes, data_effect_flags_helper);
            assert!(matches!(terminal, DataEffectTerminal::BlockComplete { .. }));
            let temporary_ids: Vec<_> = outputs
                .iter()
                .filter_map(|kind| match kind {
                    DataEffectOutputKind::Temporary(temp) => Some(temp.0),
                    _ => None,
                })
                .collect();
            assert_eq!(
                temporary_ids,
                (0..expected_temps.len() as u32).collect::<Vec<_>>()
            );
            let published = PublishedCode::publish(&code).unwrap();
            // SAFETY: bytes come from `build_block -> lower -> allocator ->
            // emitter`; the staging pointers below are valid for every emitted
            // slot, and the mapping owner remains live through execution.
            let entry: DataEffectEntry = unsafe { std::mem::transmute(published.as_ptr()) };
            let mut staging_gprs = initial_gprs;
            let mut staging_flags = old_flags;
            let mut staging_rip = 0xDEAD_BEEF;
            let mut observed = vec![0xBADD_CAFE_DEAD_BEEF; outputs.len()];
            let mut context = DataEffectContextV2 {
                staging_gprs: staging_gprs.as_mut_ptr(),
                output_values: observed.as_mut_ptr(),
                staging_rflags: &mut staging_flags,
                staging_rip: &mut staging_rip,
            };
            // The assembly wrapper validates x19-x29, valid x29 frame-chain
            // pointer, and exact SP restoration around the generated entry.
            // SAFETY: the emitted entry pointer is owned above and accepts one
            // pointer to this live, non-aliasing staging context.
            let raw = unsafe {
                od_jit_allocator_call_with_gpr_canaries(
                    entry as usize,
                    (&mut context as *mut DataEffectContextV2) as usize,
                )
            };

            assert_eq!(raw, DATA_EFFECTS_UNCOMMITTED_V1);
            assert_eq!(staging_gprs, [0, 1, u64::MAX, doubled]);
            assert_eq!(
                staging_flags & od_isa::integer_flags::DEFINED_ARITHMETIC_FLAGS,
                0x015
            );
            assert_eq!(
                staging_flags & !od_isa::integer_flags::DEFINED_ARITHMETIC_FLAGS,
                old_flags & !od_isa::integer_flags::DEFINED_ARITHMETIC_FLAGS
            );
            if let DataEffectTerminal::BlockComplete { target_rip } = terminal {
                assert_eq!(staging_rip, target_rip);
            }
            assert_eq!(canonical_state, original_canonical);
            for (slot, kind) in outputs.iter().enumerate() {
                match *kind {
                    DataEffectOutputKind::Temporary(temp) => {
                        assert_eq!(observed[slot], expected_temps[temp.0 as usize]);
                    }
                    DataEffectOutputKind::GuestRegisterCandidate {
                        instruction_index,
                        register,
                        ..
                    } => {
                        let expected = match (instruction_index, register.0) {
                            (0, 0) => u64::MAX,
                            (1, 0) | (2, 2) => 0,
                            (3, 2) => u64::MAX,
                            (4, 3) => pattern,
                            (5, 3) => doubled,
                            other => panic!("unexpected staged register candidate {other:?}"),
                        };
                        assert_eq!(observed[slot], expected);
                    }
                    DataEffectOutputKind::CompareScratch {
                        instruction_index, ..
                    } => {
                        assert_eq!(instruction_index, 6);
                        assert_eq!(observed[slot], doubled.wrapping_sub(u64::MAX));
                    }
                }
            }
        }
    }

    #[test]
    fn runtime_entry_commits_instruction_and_published_code_owns_pc_map() {
        let _guard = jit_test_lock();
        let base_rip = 0x5000;
        let immediate = 0xFEDC_BA98_7654_3210_u64;
        let mut bytes = vec![0x48, 0xB8]; // MOV RAX, imm64
        bytes.extend_from_slice(&immediate.to_le_bytes());
        let block = emit_runtime_block_for_code(&bytes, base_rip, 2);
        let published = PublishedCode::publish_block(&block).unwrap();
        let map = published.pc_map().unwrap();
        assert_eq!(map.code_len() as usize, block.to_le_bytes().unwrap().len());
        assert!(map.lookup(0).is_some());
        assert!(map.lookup(map.code_len() - 4).is_some());
        assert_eq!(map.lookup(map.code_len()), None);
        assert_eq!(map.lookup(2), None);
        assert_eq!(map.boundaries().len(), 1);

        let mut state = od_isa::LongModeState::default();
        state.gpr = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        state.rip = base_rip;
        state.rflags = 0xA5A5_F0F0_5A5A_0A0A;
        let preserved = state.clone();
        let stop = AtomicU8::new(0);

        state.rip = base_rip + 1;
        let wrong_entry_state = state.clone();
        let mismatch = published.execute_block(&mut state, 1, None, &stop);
        assert_eq!(
            mismatch,
            Err(BlockExecutionError::EntryRipMismatch {
                expected: base_rip,
                actual: base_rip + 1,
            })
        );
        assert_eq!(state, wrong_entry_state);

        state.rip = base_rip;
        let result = published.execute_block(&mut state, 1, None, &stop).unwrap();
        assert_eq!(result.exit, RuntimeExitTag::BlockComplete);
        assert_eq!(result.remaining_budget, 0);
        assert_eq!(result.retired_instructions, 1);
        assert_eq!(state.gpr[0], immediate);
        assert_eq!(&state.gpr[1..], &preserved.gpr[1..]);
        assert_eq!(state.rip, base_rip + bytes.len() as u64);
        assert_eq!(state.rflags, preserved.rflags);
    }

    #[test]
    fn runtime_boundary_checks_stop_address_stop_and_budget_in_order() {
        let _guard = jit_test_lock();
        let base_rip = 0x6000;
        let mut bytes = vec![0x48, 0xB8];
        bytes.extend_from_slice(&0x1111_u64.to_le_bytes());
        bytes.extend_from_slice(&[0x48, 0xB9]);
        bytes.extend_from_slice(&0x2222_u64.to_le_bytes());
        let block = emit_runtime_block_for_code(&bytes, base_rip, 2);
        let published = PublishedCode::publish_block(&block).unwrap();
        let stop = AtomicU8::new(0);

        let mut state = od_isa::LongModeState::default();
        state.rip = base_rip;
        stop.store(1, Ordering::Release);
        let result = published.execute_block(&mut state, 0, Some(base_rip), &stop);
        assert_eq!(result.unwrap().exit, RuntimeExitTag::StopAddress);
        assert_eq!(state.rip, base_rip);

        stop.store(1, Ordering::Release);
        let result = published.execute_block(&mut state, 0, None, &stop);
        assert_eq!(result.unwrap().exit, RuntimeExitTag::StopRequested);
        assert_eq!(state.rip, base_rip);

        stop.store(0, Ordering::Release);
        let result = published.execute_block(&mut state, 0, None, &stop);
        assert_eq!(result.unwrap().exit, RuntimeExitTag::BudgetExhausted);
        assert_eq!(state.rip, base_rip);

        // With one instruction of budget, the first MOV commits. At the next
        // guest boundary stop_rip is checked before the exhausted budget.
        let result = published
            .execute_block(&mut state, 1, Some(base_rip + 10), &stop)
            .unwrap();
        assert_eq!(result.exit, RuntimeExitTag::StopAddress);
        assert_eq!(result.remaining_budget, 0);
        assert_eq!(result.retired_instructions, 1);
        assert_eq!(state.gpr[0], 0x1111);
        assert_eq!(state.gpr[1], 0);
        assert_eq!(state.rip, base_rip + 10);

        state.rip = base_rip;
        state.gpr[0] = 0;
        let result = published.execute_block(&mut state, 1, None, &stop).unwrap();
        assert_eq!(result.exit, RuntimeExitTag::BudgetExhausted);
        assert_eq!(result.remaining_budget, 0);
        assert_eq!(result.retired_instructions, 1);
        assert_eq!(state.gpr[0], 0x1111);
        assert_eq!(state.rip, base_rip + 10);
    }

    #[test]
    fn helper_failure_after_prefix_keeps_exact_commit_rip_and_map_site() {
        let _guard = jit_test_lock();
        let base_rip = 0x7000;
        let mut bytes = vec![0x48, 0xB8];
        bytes.extend_from_slice(&10_u64.to_le_bytes());
        bytes.extend_from_slice(&[0x48, 0x03, 0xC1]); // ADD RAX, RCX
        bytes.extend_from_slice(&[0x48, 0x2B, 0xD3]); // SUB RDX, RBX
        bytes.extend_from_slice(&[0x48, 0x3B, 0xC2]); // CMP RAX, RDX
        let stop = AtomicU8::new(0);

        for (failure_call, status, committed, expected_rax, expected_rdx, failure_rip) in [
            (2, 1, 2, 15, 20, base_rip + 13),
            (3, u32::MAX, 3, 15, 17, base_rip + 16),
        ] {
            RUNTIME_HELPER_CALLS.store(0, Ordering::Relaxed);
            RUNTIME_HELPER_FAIL_AT.store(failure_call, Ordering::Relaxed);
            RUNTIME_HELPER_FAILURE_STATUS.store(status, Ordering::Relaxed);
            RUNTIME_HELPER_STOP_ON_CALL.store(0, Ordering::Relaxed);
            let block = unsafe {
                emit_runtime_block_for_code_with_helper(&bytes, base_rip, 2, runtime_test_helper)
            };
            assert!(!block.safe_entry_provenance());
            let published = PublishedCode::publish_block(&block).unwrap();
            let mut state = od_isa::LongModeState::default();
            state.rip = base_rip;
            state.rflags = 0x202;
            state.gpr[1] = 5;
            state.gpr[2] = 20;
            state.gpr[3] = 3;
            let mut context = RuntimeContextV1 {
                state: &mut state,
                initial_budget: 10,
                stop_rip_enabled: 0,
                reserved: 0,
                stop_rip: 0,
                stop_requested: (&stop as *const AtomicU8).cast(),
                remaining_budget: u64::MAX,
                retired_instructions: u64::MAX,
            };
            let entry: BlockEntry = unsafe { std::mem::transmute(published.as_ptr()) };
            // SAFETY: generated code has private emitter provenance and the
            // injected test helper honors the scalar AAPCS64 contract.
            let raw = unsafe { entry(&mut context) };

            assert_eq!(raw, RAW_HELPER_CONTRACT_FAILURE_V1);
            assert_eq!(state.rip, failure_rip);
            assert_eq!(state.gpr[0], expected_rax);
            assert_eq!(state.gpr[2], expected_rdx);
            assert_eq!(state.rflags, 0x206);
            assert_eq!(context.retired_instructions, committed);
            assert_eq!(context.remaining_budget, 10 - committed);
            assert!(
                published
                    .pc_map()
                    .unwrap()
                    .has_helper_failure_site(failure_rip)
            );
            assert_eq!(
                published.decode_block_result(
                    raw,
                    state.rip,
                    context.remaining_budget,
                    context.retired_instructions,
                ),
                Err(BlockExecutionError::HelperContractFailure {
                    guest_rip: failure_rip,
                    remaining_budget: 10 - committed,
                    retired_instructions: committed,
                })
            );
            let mut rejected_state = od_isa::LongModeState::default();
            rejected_state.rip = base_rip;
            assert_eq!(
                published.execute_block(&mut rejected_state, 10, None, &stop),
                Err(BlockExecutionError::UntrustedHelper)
            );
        }
        RUNTIME_HELPER_FAIL_AT.store(0, Ordering::Relaxed);
        RUNTIME_HELPER_FAILURE_STATUS.store(2, Ordering::Relaxed);
    }

    #[test]
    fn runtime_samples_stop_set_by_successful_helper_after_commit() {
        let _guard = jit_test_lock();
        let base_rip = 0x7800;
        let mut bytes = vec![0x48, 0xB8];
        bytes.extend_from_slice(&10_u64.to_le_bytes());
        bytes.extend_from_slice(&[0x48, 0x03, 0xC1]); // ADD RAX, RCX
        bytes.extend_from_slice(&[0x48, 0x2B, 0xD3]); // SUB RDX, RBX
        RUNTIME_HELPER_CALLS.store(0, Ordering::Relaxed);
        RUNTIME_HELPER_FAIL_AT.store(0, Ordering::Relaxed);
        RUNTIME_HELPER_STOP_ON_CALL.store(1, Ordering::Relaxed);
        let stop = AtomicU8::new(0);
        RUNTIME_HELPER_STOP_BYTE.store((&stop as *const AtomicU8).cast_mut(), Ordering::Release);
        let block = unsafe {
            emit_runtime_block_for_code_with_helper(&bytes, base_rip, 2, runtime_test_helper)
        };
        let published = PublishedCode::publish_block(&block).unwrap();
        let mut state = od_isa::LongModeState::default();
        state.rip = base_rip;
        state.gpr[1] = 5;
        state.gpr[2] = 20;
        state.gpr[3] = 3;
        let mut context = RuntimeContextV1 {
            state: &mut state,
            initial_budget: 5,
            stop_rip_enabled: 0,
            reserved: 0,
            stop_rip: 0,
            stop_requested: (&stop as *const AtomicU8).cast(),
            remaining_budget: u64::MAX,
            retired_instructions: u64::MAX,
        };
        let entry: BlockEntry = unsafe { std::mem::transmute(published.as_ptr()) };
        // SAFETY: the test helper follows the block emitter's scalar ABI and
        // sets a live AtomicU8 only after successful first arithmetic helper.
        let raw = unsafe { entry(&mut context) };
        RUNTIME_HELPER_STOP_BYTE.store(std::ptr::null_mut(), Ordering::Release);
        RUNTIME_HELPER_STOP_ON_CALL.store(0, Ordering::Relaxed);

        assert_eq!(raw, RuntimeExitTag::StopRequested as u32);
        assert_eq!(stop.load(Ordering::Acquire), 1);
        assert_eq!(state.rip, base_rip + 13);
        assert_eq!(state.gpr[0], 15);
        assert_eq!(state.gpr[2], 20);
        assert_eq!(context.retired_instructions, 2);
        assert_eq!(context.remaining_budget, 3);
        assert_eq!(
            published.decode_block_result(
                raw,
                state.rip,
                context.remaining_budget,
                context.retired_instructions,
            ),
            Ok(BlockExecutionResult {
                exit: RuntimeExitTag::StopRequested,
                remaining_budget: 3,
                retired_instructions: 2,
            })
        );
    }

    #[test]
    fn runtime_jcc_commits_selected_rip_and_retirement_together() {
        let _guard = jit_test_lock();
        let base_rip = 0x7900;
        let stop = AtomicU8::new(0);

        for opcode in [0x74, 0x75] {
            for (lhs, rhs, zero) in [(u64::MAX, 1, true), (1, 1, false)] {
                let code = [0x48, 0x03, 0xC1, opcode, 0xFB]; // ADD; JZ/JNZ to entry
                let block = emit_runtime_block_for_code(&code, base_rip, 2);
                let published = PublishedCode::publish_block(&block).unwrap();
                let mut state = od_isa::LongModeState::default();
                state.rip = base_rip;
                state.gpr[0] = lhs;
                state.gpr[1] = rhs;
                state.gpr[2] = 0xCAFE;
                let old_flags = state.rflags;
                let expected_taken = if opcode == 0x74 { zero } else { !zero };
                let expected_rip = if expected_taken {
                    base_rip
                } else {
                    base_rip + 5
                };
                let result = published.execute_block(&mut state, 2, None, &stop).unwrap();

                assert_eq!(result.exit, RuntimeExitTag::BlockComplete);
                assert_eq!(result.remaining_budget, 0);
                assert_eq!(result.retired_instructions, 2);
                assert_eq!(state.rip, expected_rip);
                assert_eq!(state.gpr[0], lhs.wrapping_add(rhs));
                assert_eq!(state.gpr[2], 0xCAFE);
                assert_eq!(
                    state.rflags,
                    od_isa::integer_flags::updated_arithmetic_flags(
                        od_isa::integer_flags::ArithmeticFlagOperation::Add,
                        lhs,
                        rhs,
                        old_flags,
                    )
                );
                assert!(
                    published
                        .pc_map()
                        .unwrap()
                        .boundaries()
                        .iter()
                        .any(|boundary| matches!(
                            boundary.rip,
                            od_jit::statepoints::GuestRipBoundary::BranchSelection { .. }
                        ))
                );
                let pc_map = published.pc_map().unwrap();
                let committed_boundary = pc_map.boundaries().last().unwrap();
                assert_eq!(
                    pc_map
                        .lookup(committed_boundary.host_offset)
                        .unwrap()
                        .guest_rip,
                    od_jit::statepoints::GuestRipSource::CanonicalState
                );
                assert_eq!(
                    pc_map
                        .ranges()
                        .iter()
                        .filter(|range| {
                            range.guest_rip == od_jit::statepoints::GuestRipSource::CanonicalState
                                && range.phase == od_jit::statepoints::StatePhase::Exit
                        })
                        .count(),
                    2,
                    "branch transfer and return stub use the selected committed RIP"
                );
                assert_eq!(
                    pc_map.ranges().last().unwrap().guest_rip,
                    od_jit::statepoints::GuestRipSource::CanonicalState,
                    "shared epilogue may receive exits at different guest RIPs"
                );
            }
        }
    }

    #[test]
    fn dispatch_candidate_commits_selected_branch_once_with_exact_count() {
        let _guard = jit_test_lock();
        let base_rip = 0x7A00;
        let stop = AtomicU8::new(0);

        let publish_dispatch = |code: &[u8]| {
            let view = CodeView::new(base_rip, code).unwrap();
            let fixture_decode = od_isa::decode(code).unwrap();
            let admission =
                AdmittedInstructionBoundaries::from_fixture_decode(view, &fixture_decode).unwrap();
            let compiled = compile_block_for_dispatch(view, base_rip, &admission).unwrap();
            let BlockCompileOutcome::Compiled(compiled) = compiled else {
                panic!("validated Jcc fixture should produce a runtime block");
            };
            PublishedCode::publish_block(compiled.emitted()).unwrap()
        };

        // The branch target is a forward instruction start in the fixture
        // tail. Runtime returns either that target or the distinct fallthrough
        // as a candidate; it does not execute or retry another block.
        let forward = [0x74, 0x02, 0x74, 0x00, 0x74, 0x00];
        for (zero, expected_offset) in [(true, 4_u64), (false, 2_u64)] {
            let published = publish_dispatch(&forward);
            let mut state = od_isa::LongModeState::default();
            state.rip = base_rip;
            state.rflags = 2 | if zero { 1 << 6 } else { 0 };
            let result = published.execute_block(&mut state, 8, None, &stop).unwrap();
            assert_eq!(result.exit, RuntimeExitTag::DispatchCandidate);
            assert_eq!(state.rip, base_rip + expected_offset);
            assert_eq!(result.retired_instructions, 1);
            assert_eq!(result.remaining_budget, 7);
        }

        // A backward/internal edge commits the MOV prefix and the Jcc exactly
        // once before returning its target for future dispatch.
        let backward = [
            0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0, // MOV RAX, 1
            0x74, 0xF4, // JZ entry
            0x74, 0x00, // validated tail
        ];
        let published = publish_dispatch(&backward);
        let mut state = od_isa::LongModeState::default();
        state.rip = base_rip;
        state.rflags = 2 | (1 << 6);
        let result = published.execute_block(&mut state, 8, None, &stop).unwrap();
        assert_eq!(result.exit, RuntimeExitTag::DispatchCandidate);
        assert_eq!(state.rip, base_rip);
        assert_eq!(state.gpr[0], 1);
        assert_eq!(result.retired_instructions, 2);
        assert_eq!(result.remaining_budget, 6);
        drop(published);

        // Code-end is also returned as a typed candidate for the caller to
        // classify; it is not silently normalized to one-block completion.
        let code_end = [0x75, 0x00];
        let published = publish_dispatch(&code_end);
        let mut state = od_isa::LongModeState::default();
        state.rip = base_rip;
        let result = published.execute_block(&mut state, 8, None, &stop).unwrap();
        assert_eq!(result.exit, RuntimeExitTag::DispatchCandidate);
        assert_eq!(state.rip, base_rip + code_end.len() as u64);
        assert_eq!(result.retired_instructions, 1);
        assert_eq!(result.remaining_budget, 7);
        drop(published);

        // The original one-block compiler has no complete fixture-boundary
        // proof, so it still commits only the MOV prefix and returns at Jcc.
        let mut one_block = vec![0x48, 0xB8];
        one_block.extend_from_slice(&1_u64.to_le_bytes());
        one_block.extend_from_slice(&[0x74, 0x02, 0x74, 0x00, 0x74, 0x00]);
        let view = CodeView::new(base_rip, &one_block).unwrap();
        let compiled = compile_block(view, base_rip).unwrap();
        let BlockCompileOutcome::Compiled(compiled) = compiled else {
            panic!("legacy one-block prefix should compile");
        };
        let published = PublishedCode::publish_block(compiled.emitted()).unwrap();
        let mut state = od_isa::LongModeState::default();
        state.rip = base_rip;
        state.rflags = 2 | (1 << 6);
        let result = published.execute_block(&mut state, 8, None, &stop).unwrap();
        assert_eq!(result.exit, RuntimeExitTag::NeedInterpreter);
        assert_eq!(state.rip, base_rip + 10);
        assert_eq!(state.gpr[0], 1);
        assert_eq!(result.retired_instructions, 1);
        assert_eq!(result.remaining_budget, 7);
    }

    #[test]
    fn cached_dispatch_matches_uncached_native_fixture_and_reuses_owner() {
        let _guard = jit_test_lock();
        use od_jit::translation::{
            AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity, MappingId,
            MappingVersion,
        };
        use std::num::NonZeroU64;

        let base_rip = 0x7B00;
        let code = [0x48, 0xB8, 0x2A, 0, 0, 0, 0, 0, 0, 0];
        let view = CodeView::new(base_rip, &code).unwrap();
        let fixture_decode = od_isa::decode(&code).unwrap();
        let admission =
            AdmittedInstructionBoundaries::from_fixture_decode(view, &fixture_decode).unwrap();
        let nz = |value| NonZeroU64::new(value).unwrap();
        let dependencies = [CodeDependencySpan {
            start_rip: view.base_rip(),
            end_rip: view.code_end(),
            identity: CodeIdentity {
                address_space: AddressSpaceId::new(nz(1)),
                mapping: MappingId::new(nz(2)),
                mapping_version: MappingVersion::new(nz(1)),
                backing: BackingId::new(nz(3)),
                backing_version: BackingVersion::new(nz(1)),
                executable: true,
            },
        }];
        let mut uncached_state = od_isa::LongModeState {
            rip: base_rip,
            rflags: 0xA55,
            ..od_isa::LongModeState::default()
        };
        let mut cached_state = uncached_state.clone();
        let mut executor = PublishedBlockExecutor;
        let uncached =
            od_jit::dispatch::run(view, &admission, &mut uncached_state, 4, &mut executor).unwrap();
        assert_eq!(uncached.exit, od_jit::dispatch::DispatchExit::CodeEnd);
        assert_eq!(uncached_state.gpr[0], 42);

        let mut cache = od_jit::dispatch::SingleEntryBlockCache::new(
            1,
            od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES,
        )
        .unwrap();
        let cached = od_jit::dispatch::run_cached(
            view,
            &admission,
            cached_test_context(),
            &dependencies,
            &mut cached_state,
            4,
            &mut cache,
            &mut executor,
        )
        .unwrap();
        assert_eq!(cached, uncached);
        assert_eq!(cached_state, uncached_state);
        assert_eq!(cache.resident_entries(), 1);
        assert!(cache.resident_logical_bytes() > 0);

        cached_state.rip = base_rip;
        let repeated = od_jit::dispatch::run_cached(
            view,
            &admission,
            cached_test_context(),
            &dependencies,
            &mut cached_state,
            4,
            &mut cache,
            &mut executor,
        )
        .unwrap();
        assert_eq!(repeated, uncached);
        assert_eq!(cached_state, uncached_state);
        cache.clear();
        assert_eq!(cache.resident_entries(), 0);
    }

    #[test]
    fn multi_entry_arena_cache_matches_uncached_native_blocks_and_reuses_key() {
        let _guard = jit_test_lock();
        use od_jit::translation::{
            AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity, MappingId,
            MappingVersion,
        };
        use std::num::NonZeroU64;

        let base_rip = 0x7B80;
        let mut expected_results = Vec::new();
        let mut uncached_executor = PublishedBlockExecutor;
        for value in [41u8, 42u8] {
            let code = [0x48, 0xB8, value, 0, 0, 0, 0, 0, 0, 0];
            let view = CodeView::new(base_rip, &code).unwrap();
            let decoded = od_isa::decode(&code).unwrap();
            let admission =
                AdmittedInstructionBoundaries::from_fixture_decode(view, &decoded).unwrap();
            let initial = od_isa::LongModeState {
                rip: base_rip,
                rflags: 0xA55,
                ..od_isa::LongModeState::default()
            };
            let mut expected_state = initial.clone();
            let expected_result = od_jit::dispatch::run(
                view,
                &admission,
                &mut expected_state,
                4,
                &mut uncached_executor,
            )
            .unwrap();
            expected_results.push((expected_result, expected_state));
        }

        let mut arena = PublishedArenaBackend::new(2).unwrap();
        let mut cache = od_jit::dispatch::MultiEntryBlockCache::new(
            2,
            od_jit::dispatch::MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            od_jit::dispatch::MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap();
        for (index, value) in [41u8, 42u8].into_iter().enumerate() {
            let code = [0x48, 0xB8, value, 0, 0, 0, 0, 0, 0, 0];
            let view = CodeView::new(base_rip, &code).unwrap();
            let decoded = od_isa::decode(&code).unwrap();
            let admission =
                AdmittedInstructionBoundaries::from_fixture_decode(view, &decoded).unwrap();
            let nz = |number| NonZeroU64::new(number).unwrap();
            let dependencies = [CodeDependencySpan {
                start_rip: view.base_rip(),
                end_rip: view.code_end(),
                identity: CodeIdentity {
                    address_space: AddressSpaceId::new(nz(1)),
                    mapping: MappingId::new(nz(2)),
                    mapping_version: MappingVersion::new(nz(1)),
                    backing: BackingId::new(nz(3)),
                    backing_version: BackingVersion::new(nz(1)),
                    executable: true,
                },
            }];
            let mut actual_state = od_isa::LongModeState {
                rip: base_rip,
                rflags: 0xA55,
                ..od_isa::LongModeState::default()
            };
            let actual = od_jit::dispatch::run_multi_cached(
                view,
                &admission,
                cached_test_context(),
                &dependencies,
                &mut actual_state,
                4,
                &mut cache,
                &mut arena,
            )
            .unwrap();
            assert_eq!(actual, expected_results[index].0);
            assert_eq!(actual_state, expected_results[index].1);
        }

        let code = [0x48, 0xB8, 41, 0, 0, 0, 0, 0, 0, 0];
        let view = CodeView::new(base_rip, &code).unwrap();
        let decoded = od_isa::decode(&code).unwrap();
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(view, &decoded).unwrap();
        let nz = |number| NonZeroU64::new(number).unwrap();
        let dependencies = [CodeDependencySpan {
            start_rip: view.base_rip(),
            end_rip: view.code_end(),
            identity: CodeIdentity {
                address_space: AddressSpaceId::new(nz(1)),
                mapping: MappingId::new(nz(2)),
                mapping_version: MappingVersion::new(nz(1)),
                backing: BackingId::new(nz(3)),
                backing_version: BackingVersion::new(nz(1)),
                executable: true,
            },
        }];
        let mut actual_state = od_isa::LongModeState {
            rip: base_rip,
            rflags: 0xA55,
            ..od_isa::LongModeState::default()
        };
        let before = cache.counters();
        let hit = od_jit::dispatch::run_multi_cached(
            view,
            &admission,
            cached_test_context(),
            &dependencies,
            &mut actual_state,
            4,
            &mut cache,
            &mut arena,
        )
        .unwrap();
        assert_eq!(hit, expected_results[0].0);
        assert_eq!(actual_state, expected_results[0].1);
        assert_eq!(cache.counters().successful_publications, 2);
        assert_eq!(cache.counters().hits, before.hits + 1);
        assert_eq!(arena.available_slots(), 0);
        cache.clear();
        assert_eq!(arena.available_slots(), 2);
    }

    #[test]
    fn iterative_dispatch_publishes_and_drops_one_mapping_per_block() {
        let _guard = jit_test_lock();
        let base_rip = 0x7B00;
        let code = [0x74, 0x00, 0x48, 0xB8, 7, 0, 0, 0, 0, 0, 0, 0];
        let view = CodeView::new(base_rip, &code).unwrap();
        let decoded = od_isa::decode(&code).unwrap();
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(view, &decoded).unwrap();
        let mut state = od_isa::LongModeState::default();
        state.rip = base_rip;
        let mut executor = PublishedBlockExecutor;

        let result = od_jit::dispatch::run(view, &admission, &mut state, 5, &mut executor).unwrap();
        assert_eq!(result.exit, od_jit::dispatch::DispatchExit::CodeEnd);
        assert_eq!(result.retired_instructions, 2);
        assert_eq!(result.remaining_budget, 3);
        assert_eq!(result.blocks_executed, 2);
        assert_eq!(state.rip, view.code_end());
        assert_eq!(state.gpr[0], 7);

        // A positive-retirement self-loop is valid and repeatedly reuses the
        // singleton MAP_JIT slot only after each callback drops its mapping.
        let loop_code = [0x74, 0xFE, 0x75, 0x00];
        let loop_view = CodeView::new(base_rip, &loop_code).unwrap();
        let loop_decode = od_isa::decode(&loop_code).unwrap();
        let loop_admission =
            AdmittedInstructionBoundaries::from_fixture_decode(loop_view, &loop_decode).unwrap();
        let mut loop_state = od_isa::LongModeState::default();
        loop_state.rip = base_rip;
        loop_state.rflags = 2 | (1 << 6);
        let loop_result = od_jit::dispatch::run(
            loop_view,
            &loop_admission,
            &mut loop_state,
            3,
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            loop_result.exit,
            od_jit::dispatch::DispatchExit::BudgetExhausted
        );
        assert_eq!(loop_result.retired_instructions, 3);
        assert_eq!(loop_result.remaining_budget, 0);
        assert_eq!(loop_result.blocks_executed, 3);
        assert_eq!(loop_state.rip, base_rip);
    }

    #[test]
    fn native_nop_fallback_resumes_at_exact_rip_and_continues_to_jcc() {
        let _guard = jit_test_lock();
        let base_rip = 0x7B80;
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&7_u64.to_le_bytes()); // MOV RAX,7
        code.extend_from_slice(&[0x48, 0x03, 0xC1]); // ADD RAX,RCX
        code.push(0x90); // exact bare NOP, interpreted once
        code.extend_from_slice(&[0x48, 0x3B, 0xC3]); // CMP RAX,RBX
        code.extend_from_slice(&[0x74, 0x00]); // JZ code_end
        let view = CodeView::new(base_rip, &code).unwrap();
        let admission = od_jit::dispatch::InterpreterFallbackAdmission::from_code(view).unwrap();
        let mut state = od_isa::LongModeState::default();
        state.rip = base_rip;
        state.rflags = 0x202;
        state.gpr[1] = 5;
        state.gpr[3] = 12;
        let stop = AtomicU8::new(0);
        let control = od_jit::dispatch::DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
        };
        let mut executor = PublishedBlockExecutor;

        let result = od_jit::dispatch::run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            10,
            control,
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            result.exit,
            od_jit::dispatch::DispatchExit::CodeEnd,
            "{result:?} {state:?}"
        );
        assert_eq!(result.retired_native, 4); // MOV, ADD, CMP, JZ
        assert_eq!(result.retired_interpreter, 1); // NOP
        assert_eq!(result.remaining_budget, 5);
        assert_eq!(result.blocks_executed, 2);
        assert_eq!(state.rip, view.code_end());
        assert_eq!(state.gpr[0], 12);
        assert_ne!(state.rflags & (1 << 6), 0); // CMP equality drives JZ.
    }

    #[test]
    fn native_fallback_nop_preserves_preceding_arithmetic_flags() {
        let _guard = jit_test_lock();
        let base_rip = 0x7BC0;
        let add = [0x48, 0x03, 0xC1];
        let mut state_before = od_isa::LongModeState::default();
        state_before.rip = base_rip;
        state_before.rflags = 0x202;
        state_before.gpr[0] = u64::MAX;
        state_before.gpr[1] = 1;

        let add_view = CodeView::new(base_rip, &add).unwrap();
        let add_admission =
            od_jit::dispatch::InterpreterFallbackAdmission::from_code(add_view).unwrap();
        let stop = AtomicU8::new(0);
        let mut executor = PublishedBlockExecutor;
        let mut expected = state_before.clone();
        let expected_result = od_jit::dispatch::run_with_nop_fallback(
            add_view,
            &add_admission,
            &mut expected,
            2,
            od_jit::dispatch::DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(expected_result.retired_native, 1);
        assert_eq!(expected_result.retired_interpreter, 0);

        let mut add_then_nop = add.to_vec();
        add_then_nop.push(0x90);
        let view = CodeView::new(base_rip, &add_then_nop).unwrap();
        let admission = od_jit::dispatch::InterpreterFallbackAdmission::from_code(view).unwrap();
        let mut actual = state_before;
        let result = od_jit::dispatch::run_with_nop_fallback(
            view,
            &admission,
            &mut actual,
            2,
            od_jit::dispatch::DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(result.retired_native, 1);
        assert_eq!(result.retired_interpreter, 1);
        assert_eq!(actual.rip, view.code_end());
        assert_eq!(actual.rflags, expected.rflags);
        assert_eq!(actual.gpr, expected.gpr);
    }

    #[test]
    fn native_prefix_stop_is_observed_before_interpreter_nop() {
        let _guard = jit_test_lock();
        struct StopAfterBlock(PublishedBlockExecutor);
        impl od_jit::dispatch::ControlledBlockExecutor for StopAfterBlock {
            type Error = PublishedBlockExecutionError;

            fn execute_one_block_controlled(
                &mut self,
                block: &od_jit::block_compile::CompiledBlock,
                state: &mut od_isa::LongModeState,
                budget: u64,
                control: od_jit::dispatch::DispatchControl<'_>,
            ) -> Result<od_jit::dispatch::BlockExecutionReport, Self::Error> {
                let report =
                    od_jit::dispatch::ControlledBlockExecutor::execute_one_block_controlled(
                        &mut self.0,
                        block,
                        state,
                        budget,
                        control,
                    )?;
                control.stop_requested.store(1, Ordering::Release);
                Ok(report)
            }
        }

        let base_rip = 0x7C20;
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&9_u64.to_le_bytes());
        code.push(0x90);
        let view = CodeView::new(base_rip, &code).unwrap();
        let admission = od_jit::dispatch::InterpreterFallbackAdmission::from_code(view).unwrap();
        let mut state = od_isa::LongModeState::default();
        state.rip = base_rip;
        let stop = AtomicU8::new(0);
        let mut executor = StopAfterBlock(PublishedBlockExecutor);

        let result = od_jit::dispatch::run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            3,
            od_jit::dispatch::DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            result.exit,
            od_jit::dispatch::DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(result.retired_native, 1);
        assert_eq!(result.retired_interpreter, 0);
        assert_eq!(result.remaining_budget, 2);
        assert_eq!(state.rip, base_rip + 10);
        assert_eq!(state.gpr[0], 9);
        assert_eq!(stop.load(Ordering::Acquire), 1);
    }

    #[test]
    fn runtime_unsupported_and_need_interpreter_exits_keep_prefix_state() {
        let _guard = jit_test_lock();
        let base_rip = 0x7A00;
        let mut mov = vec![0x48, 0xB8];
        mov.extend_from_slice(&0x1234_5678_u64.to_le_bytes());
        let mut unsupported_bytes = mov.clone();
        unsupported_bytes.push(0x90); // decoded but not admitted by S0-09
        let mut unresolved_branch_bytes = mov;
        unresolved_branch_bytes.extend_from_slice(&[0x74, 0x20]); // forward target outside view
        let stop = AtomicU8::new(0);

        for (bytes, expected_exit) in [
            (&unsupported_bytes[..], RuntimeExitTag::Unsupported),
            (
                &unresolved_branch_bytes[..],
                RuntimeExitTag::NeedInterpreter,
            ),
        ] {
            let block = emit_runtime_block_for_code(bytes, base_rip, 2);
            let published = PublishedCode::publish_block(&block).unwrap();
            let mut state = od_isa::LongModeState::default();
            state.rip = base_rip;
            state.gpr[1] = 0xBEEF;
            state.rflags = 0xA5A5_F0F0_5A5A_0A0A;
            let original_flags = state.rflags;
            let result = published.execute_block(&mut state, 2, None, &stop).unwrap();

            assert_eq!(result.exit, expected_exit);
            assert_eq!(result.remaining_budget, 1);
            assert_eq!(result.retired_instructions, 1);
            assert_eq!(state.gpr[0], 0x1234_5678);
            assert_eq!(state.gpr[1], 0xBEEF);
            assert_eq!(state.rflags, original_flags);
            assert_eq!(state.rip, base_rip + 10);
        }
    }

    #[test]
    fn generated_jz_jnz_stage_taken_and_not_taken_rips() {
        let _guard = jit_test_lock();
        let base_rip = 0x3000;
        let old_flags = 0xA5A5_F0F0_5A5A_0A0A;

        for opcode in [0x74, 0x75] {
            for (lhs, rhs, zero, expected_result) in [(u64::MAX, 1, true, 0), (1, 1, false, 2)] {
                let code = [0x48, 0x03, 0xC1, opcode, 0xFB]; // ADD; JZ/JNZ back to entry
                let (native_code, _, terminal) =
                    emit_decoder_data_effects(&code, 2, data_effect_flags_helper);
                let DataEffectTerminal::Branch {
                    condition,
                    target_rip,
                    fallthrough_rip,
                } = terminal
                else {
                    panic!("JZ/JNZ program must retain its verified branch terminal");
                };
                assert_eq!(target_rip, base_rip);
                assert_eq!(fallthrough_rip, base_rip + code.len() as u64);

                let published = PublishedCode::publish(&native_code).unwrap();
                // SAFETY: this entry comes from verified decoder output and the
                // matching allocator/emitter; all four staging pointers live.
                let entry: DataEffectEntry = unsafe { std::mem::transmute(published.as_ptr()) };
                let mut staging_gprs = [lhs, rhs, 0x3333, 0x4444];
                let mut staging_flags = old_flags;
                let mut staging_rip = 0xDEAD_BEEF;
                let mut outputs = [0xBADD_CAFE_DEAD_BEEF; 12];
                let mut context = DataEffectContextV2 {
                    staging_gprs: staging_gprs.as_mut_ptr(),
                    output_values: outputs.as_mut_ptr(),
                    staging_rflags: &mut staging_flags,
                    staging_rip: &mut staging_rip,
                };
                // SAFETY: owner is live and the pointers above remain valid.
                let raw = unsafe {
                    od_jit_allocator_call_with_gpr_canaries(
                        entry as usize,
                        (&mut context as *mut DataEffectContextV2) as usize,
                    )
                };

                assert_eq!(raw, DATA_EFFECTS_UNCOMMITTED_V1);
                assert_eq!(staging_gprs[0], expected_result);
                assert_eq!(staging_gprs[1..], [rhs, 0x3333, 0x4444]);
                let expected_flags = od_isa::integer_flags::updated_arithmetic_flags(
                    od_isa::integer_flags::ArithmeticFlagOperation::Add,
                    lhs,
                    rhs,
                    old_flags,
                );
                assert_eq!(staging_flags, expected_flags);
                let taken = match condition {
                    od_jit::ir::BranchCondition::Zero => zero,
                    od_jit::ir::BranchCondition::NotZero => !zero,
                };
                assert_eq!(
                    staging_rip,
                    if taken { target_rip } else { fallthrough_rip }
                );
            }
        }
    }

    #[test]
    fn generated_mov_preserves_eager_arithmetic_flags_before_branch() {
        let _guard = jit_test_lock();
        let base_rip = 0x3000;
        let immediate = 0xFEDC_BA98_7654_3210_u64;
        // ADD RAX, RCX; MOV RDX, imm64; JZ back to the block entry.
        let mut code = vec![0x48, 0x03, 0xC1, 0x48, 0xBA];
        code.extend_from_slice(&immediate.to_le_bytes());
        code.extend_from_slice(&[0x74, 0xF1]);
        let (native_code, _, terminal) =
            emit_decoder_data_effects(&code, 2, data_effect_flags_helper);
        let DataEffectTerminal::Branch { target_rip, .. } = terminal else {
            panic!("JZ must remain a branch terminal after MOV");
        };
        assert_eq!(target_rip, base_rip);

        let published = PublishedCode::publish(&native_code).unwrap();
        // SAFETY: generated code came from the validated block and emitter;
        // all context pointers remain valid until the mapping is dropped.
        let entry: DataEffectEntry = unsafe { std::mem::transmute(published.as_ptr()) };
        let mut staging_gprs = [u64::MAX, 1, 0, 0x4444];
        let mut staging_flags = 0xA5A5_F0F0_5A5A_0A0A;
        let old_flags = staging_flags;
        let mut staging_rip = u64::MAX;
        let mut outputs = [0_u64; 8];
        let mut context = DataEffectContextV2 {
            staging_gprs: staging_gprs.as_mut_ptr(),
            output_values: outputs.as_mut_ptr(),
            staging_rflags: &mut staging_flags,
            staging_rip: &mut staging_rip,
        };
        // SAFETY: the published entry is owned above and receives the live
        // V2 staging context.
        let raw = unsafe {
            od_jit_allocator_call_with_gpr_canaries(
                entry as usize,
                (&mut context as *mut DataEffectContextV2) as usize,
            )
        };

        assert_eq!(raw, DATA_EFFECTS_UNCOMMITTED_V1);
        assert_eq!(staging_gprs, [0, 1, immediate, 0x4444]);
        assert_eq!(
            staging_flags,
            od_isa::integer_flags::updated_arithmetic_flags(
                od_isa::integer_flags::ArithmeticFlagOperation::Add,
                u64::MAX,
                1,
                old_flags,
            ),
            "MOV must preserve the arithmetic flags produced by ADD"
        );
        assert_eq!(staging_rip, base_rip);
    }

    #[test]
    fn helper_error_statuses_do_not_publish_current_instruction_or_terminal() {
        let _guard = jit_test_lock();
        let code = [0x48, 0x03, 0xC1]; // ADD RAX, RCX
        let (native_code, output_kinds, _) =
            emit_decoder_data_effects(&code, 1, data_effect_failing_helper);
        let published = PublishedCode::publish(&native_code).unwrap();
        // SAFETY: the entry came from the checked production emitter and uses
        // live, disjoint staging memory below.
        let entry: DataEffectEntry = unsafe { std::mem::transmute(published.as_ptr()) };
        let original_gprs = [1, 2, 3, 4];
        let original_flags = 0xA5A5_F0F0_5A5A_0A0A;
        let original_rip = 0x3000;
        let mut canonical_state = od_isa::LongModeState::default();
        canonical_state.gpr[..4].copy_from_slice(&original_gprs);
        canonical_state.rflags = original_flags;
        canonical_state.rip = original_rip;
        let original_canonical = canonical_state.clone();

        for status in [1, 2, u32::MAX] {
            DATA_EFFECT_HELPER_STATUS.store(status, Ordering::Relaxed);
            let mut staging_gprs = original_gprs;
            let mut staging_flags = original_flags;
            let mut staging_rip = original_rip;
            let sentinel = 0xBADD_CAFE_DEAD_BEEF;
            let mut outputs = vec![sentinel; output_kinds.len()];
            let mut context = DataEffectContextV2 {
                staging_gprs: staging_gprs.as_mut_ptr(),
                output_values: outputs.as_mut_ptr(),
                staging_rflags: &mut staging_flags,
                staging_rip: &mut staging_rip,
            };
            // SAFETY: owner and all staging buffers remain live during call.
            let raw = unsafe {
                od_jit_allocator_call_with_gpr_canaries(
                    entry as usize,
                    (&mut context as *mut DataEffectContextV2) as usize,
                )
            };
            assert_eq!(raw, RAW_HELPER_CONTRACT_FAILURE_V1);
            assert_eq!(staging_gprs, original_gprs);
            assert_eq!(staging_flags, original_flags);
            assert_eq!(staging_rip, original_rip);
            assert_eq!(canonical_state, original_canonical);
            for (slot, kind) in output_kinds.iter().enumerate() {
                if matches!(kind, DataEffectOutputKind::GuestRegisterCandidate { .. }) {
                    assert_eq!(outputs[slot], sentinel);
                }
            }
        }
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

    fn compiled_arena_mov(base_rip: u64, value: u64) -> od_jit::block_compile::CompiledBlock {
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&value.to_le_bytes());
        let view = CodeView::new(base_rip, &code).unwrap();
        let BlockCompileOutcome::Compiled(block) = compile_block(view, base_rip).unwrap() else {
            panic!("MOV fixture should compile");
        };
        block
    }

    fn execute_arena_handle(
        backend: &mut PublishedArenaBackend,
        handle: &PublishedArenaBlock,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut od_isa::LongModeState,
        budget: u64,
    ) -> od_jit::dispatch::BlockExecutionReport {
        let stop = AtomicU8::new(0);
        backend
            .execute_cached_block(
                handle,
                block,
                state,
                budget,
                od_jit::dispatch::DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                },
            )
            .unwrap()
    }

    #[test]
    fn published_arena_executes_two_live_blocks_and_reuses_only_dropped_slot() {
        let _guard = jit_test_lock();
        let block_a = compiled_arena_mov(0x8000, 11);
        let block_b = compiled_arena_mov(0x8100, 22);
        let block_c = compiled_arena_mov(0x8200, 33);
        let mut backend = PublishedArenaBackend::new(2).unwrap();
        let handle_a = backend.publish_cached_block(&block_a).unwrap();
        let slot_a = handle_a.slot_index;
        let handle_b = backend.publish_cached_block(&block_b).unwrap();
        assert_eq!(backend.available_slots(), 0);
        assert!(matches!(
            backend.publish_cached_block(&block_c),
            Err(PublishedArenaError::CapacityFull)
        ));
        assert_eq!(
            backend.available_slots(),
            0,
            "full failure must not consume a slot"
        );

        let mut state_a = od_isa::LongModeState {
            rip: 0x8000,
            ..od_isa::LongModeState::default()
        };
        let report_a = execute_arena_handle(&mut backend, &handle_a, &block_a, &mut state_a, 2);
        assert_eq!(report_a.exit, RuntimeExitTag::BlockComplete);
        assert_eq!(report_a.retired_instructions, 1);
        assert_eq!(state_a.gpr[0], 11);

        let mut state_b = od_isa::LongModeState {
            rip: 0x8100,
            ..od_isa::LongModeState::default()
        };
        let report_b = execute_arena_handle(&mut backend, &handle_b, &block_b, &mut state_b, 2);
        assert_eq!(report_b.exit, RuntimeExitTag::BlockComplete);
        assert_eq!(state_b.gpr[0], 22);

        drop(handle_a);
        assert_eq!(backend.available_slots(), 1);
        let handle_c = backend.publish_cached_block(&block_c).unwrap();
        assert_eq!(handle_c.slot_index, slot_a, "free stack reuses A's slot");

        state_b.rip = 0x8100;
        let report_b_again =
            execute_arena_handle(&mut backend, &handle_b, &block_b, &mut state_b, 2);
        assert_eq!(report_b_again.exit, RuntimeExitTag::BlockComplete);
        assert_eq!(state_b.gpr[0], 22, "publishing C must not overwrite B");

        let mut state_c = od_isa::LongModeState {
            rip: 0x8200,
            ..od_isa::LongModeState::default()
        };
        let report_c = execute_arena_handle(&mut backend, &handle_c, &block_c, &mut state_c, 2);
        assert_eq!(report_c.exit, RuntimeExitTag::BlockComplete);
        assert_eq!(state_c.gpr[0], 33);
    }

    #[test]
    fn arena_lease_outlives_backend_until_last_handle_and_can_reopen() {
        let _guard = jit_test_lock();
        let block = compiled_arena_mov(0x8300, 44);
        let mut backend = PublishedArenaBackend::new(2).unwrap();
        let handle = backend.publish_cached_block(&block).unwrap();
        // This is an allocation-only probe; the bytes are never invoked.
        assert_eq!(
            PublishedCode::publish(&[0xC0, 0x03, 0x5F, 0xD6]).err(),
            Some(ExecutableMemoryError::MappingAlreadyLive)
        );
        assert_eq!(
            PublishedArenaBackend::new(1).err(),
            Some(PublishedArenaError::MappingAlreadyLive)
        );
        drop(backend);
        assert_eq!(
            PublishedCode::publish(&[0xC0, 0x03, 0x5F, 0xD6]).err(),
            Some(ExecutableMemoryError::MappingAlreadyLive),
            "the handle retains the arena after its backend drops"
        );
        assert_eq!(
            PublishedArenaBackend::new(1).err(),
            Some(PublishedArenaError::MappingAlreadyLive),
            "a slot handle also retains the global arena lease"
        );
        drop(handle);
        drop(PublishedCode::publish(&[0xC0, 0x03, 0x5F, 0xD6]).unwrap());
        let reopened = PublishedArenaBackend::new(1).unwrap();
        assert_eq!(reopened.available_slots(), 1);
    }

    #[test]
    fn arena_mapping_rounding_is_checked_and_capacity_is_bounded() {
        assert_eq!(arena_mapping_size(1, ARENA_SLOT_BYTES, 4096), Some(4096));
        assert_eq!(
            arena_mapping_size(2, ARENA_SLOT_BYTES, 16_384),
            Some(16_384)
        );
        assert_eq!(arena_mapping_size(0, ARENA_SLOT_BYTES, 4096), None);
        assert_eq!(arena_mapping_size(1, ARENA_SLOT_BYTES, 0), None);
        assert_eq!(arena_mapping_size(usize::MAX, ARENA_SLOT_BYTES, 4096), None);
        assert!(matches!(
            PublishedArenaBackend::new(0),
            Err(PublishedArenaError::InvalidCapacity { requested: 0, .. })
        ));
        assert!(matches!(
            PublishedArenaBackend::new(MAX_PUBLISHED_ARENA_BLOCKS + 1),
            Err(PublishedArenaError::InvalidCapacity { .. })
        ));
    }

    #[test]
    fn arena_rejects_foreign_handle_before_entry() {
        let _guard = jit_test_lock();
        let block = compiled_arena_mov(0x8400, 55);
        let mut backend = PublishedArenaBackend::new(1).unwrap();
        let handle = backend.publish_cached_block(&block).unwrap();
        // Fabricate a distinct owner token in the unit-test module to exercise
        // the same guard used for a handle passed from another arena backend.
        let mut foreign_backend = PublishedArenaBackend {
            arena: backend.arena.clone(),
            owner_id: Rc::new(()),
        };
        let mut state = od_isa::LongModeState {
            rip: 0x8400,
            ..od_isa::LongModeState::default()
        };
        let stop = AtomicU8::new(0);
        assert_eq!(
            foreign_backend.execute_cached_block(
                &handle,
                &block,
                &mut state,
                1,
                od_jit::dispatch::DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                },
            ),
            Err(PublishedArenaError::ForeignHandle)
        );
    }

    #[test]
    fn failed_slot_reservation_rolls_back_and_slot_remains_reusable() {
        let _guard = jit_test_lock();
        let block = compiled_arena_mov(0x8500, 66);
        let mut backend = PublishedArenaBackend::new(1).unwrap();
        backend.arena.borrow_mut().slots[0].generation = u64::MAX;
        assert!(matches!(
            backend.publish_cached_block(&block),
            Err(PublishedArenaError::GenerationExhausted)
        ));
        assert_eq!(backend.available_slots(), 1);
        backend.arena.borrow_mut().slots[0].generation = 0;
        let handle = backend.publish_cached_block(&block).unwrap();
        assert_eq!(backend.available_slots(), 0);
        drop(handle);
        assert_eq!(backend.available_slots(), 1);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn generated_checked_memory_probe_calls_fixed_width_helper() {
        use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
        use od_jit::memory_bridge::{
            CheckedMemoryAdapter, CheckedMemoryHelperV1, CheckedMemoryOutcomeV1,
            MEMORY_ERROR_BACKEND, MEMORY_HELPER_MEMORY_ERROR, MEMORY_HELPER_SUCCESS,
            MEMORY_OP_READ,
        };

        struct Memory {
            bytes: [u8; 16],
            reads: usize,
            writes: usize,
            fail: bool,
        }
        impl GuestMemory for Memory {
            fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
                self.bytes
                    .get(address as usize)
                    .copied()
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Fetch,
                    })
            }
            fn read(
                &mut self,
                address: u64,
                _: AccessContext,
                out: &mut [u8],
            ) -> Result<(), MemoryError> {
                self.reads += 1;
                if self.fail {
                    return Err(MemoryError::Backend("probe".into()));
                }
                let start = address as usize;
                let source =
                    self.bytes
                        .get(start..start + out.len())
                        .ok_or(MemoryError::Unmapped {
                            address,
                            access: AccessKind::Read,
                        })?;
                out.copy_from_slice(source);
                Ok(())
            }
            fn write(
                &mut self,
                address: u64,
                _: AccessContext,
                data: &[u8],
            ) -> Result<(), MemoryError> {
                self.writes += 1;
                if self.fail {
                    return Err(MemoryError::Backend("probe".into()));
                }
                let start = address as usize;
                let destination =
                    self.bytes
                        .get_mut(start..start + data.len())
                        .ok_or(MemoryError::Unmapped {
                            address,
                            access: AccessKind::Write,
                        })?;
                destination.copy_from_slice(data);
                Ok(())
            }
            fn atomic_exchange_integer(
                &mut self,
                _: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                unreachable!()
            }
            fn atomic_fetch_add_integer(
                &mut self,
                _: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
            ) -> Result<u64, MemoryError> {
                unreachable!()
            }
            fn atomic_compare_exchange_integer(
                &mut self,
                _: u64,
                _: AccessContext,
                _: od_isa::OperandSize,
                _: u64,
                _: u64,
            ) -> Result<u64, MemoryError> {
                unreachable!()
            }
            fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        fn emit_probe(helper: CheckedMemoryHelperV1) -> Vec<u8> {
            let address = helper as usize as u64;
            let mut encoder = Encoder::new();
            let x16 = XReg::new(16).unwrap();
            let x19 = XReg::new(19).unwrap();
            let x20 = XReg::new(20).unwrap();
            let x21 = XReg::new(21).unwrap();
            let x30 = XReg::new(30).unwrap();
            encoder
                .stp_x(x19, x20, AddressBase::Sp, -16, PairIndexMode::PreIndex)
                .unwrap();
            encoder
                .stp_x(x21, x30, AddressBase::Sp, -16, PairIndexMode::PreIndex)
                .unwrap();
            encoder.mov_x(x19, XRegOrZero::Reg(XReg::new(6).unwrap()));
            encoder.mov_x(x20, XRegOrZero::Reg(XReg::new(7).unwrap()));
            encoder.mov_x(x21, XRegOrZero::Reg(XReg::new(4).unwrap()));
            encoder.movz_x(x16, address as u16, 0).unwrap();
            encoder.movk_x(x16, (address >> 16) as u16, 1).unwrap();
            encoder.movk_x(x16, (address >> 32) as u16, 2).unwrap();
            encoder.movk_x(x16, (address >> 48) as u16, 3).unwrap();
            encoder.blr(x16);
            let failed = encoder.new_label().unwrap();
            encoder.cbnz_w(WReg::new(0).unwrap(), failed).unwrap();
            let x9 = XReg::new(9).unwrap();
            let x10 = XReg::new(10).unwrap();
            encoder
                .ldr_x(XRegOrZero::Reg(x9), AddressBase::Reg(x21), 0)
                .unwrap();
            encoder
                .str_x(XRegOrZero::Reg(x9), AddressBase::Reg(x19), 0)
                .unwrap();
            encoder
                .ldr_x(XRegOrZero::Reg(x10), AddressBase::Reg(x20), 0)
                .unwrap();
            encoder
                .adds_imm_x(XRegOrZero::Reg(x10), XRegOrSp::Reg(x10), 1)
                .unwrap();
            encoder
                .str_x(XRegOrZero::Reg(x10), AddressBase::Reg(x20), 0)
                .unwrap();
            encoder.bind(failed).unwrap();
            encoder
                .ldp_x(x21, x30, AddressBase::Sp, 16, PairIndexMode::PostIndex)
                .unwrap();
            encoder
                .ldp_x(x19, x20, AddressBase::Sp, 16, PairIndexMode::PostIndex)
                .unwrap();
            encoder.ret(XReg::new(30).unwrap());
            encoder.finish().unwrap().to_le_bytes().unwrap()
        }

        type ProbeEntry = unsafe extern "C" fn(
            *mut std::ffi::c_void,
            u32,
            u64,
            u32,
            *mut u8,
            *mut CheckedMemoryOutcomeV1,
            *mut u64,
            *mut u64,
        ) -> u32;

        let _guard = jit_test_lock();
        let code = emit_probe(od_jit::memory_bridge::checked_memory_helper_v1);
        let published = PublishedCode::publish(&code).unwrap();
        // SAFETY: `code` is a typed AAPCS64 wrapper with the helper's exact
        // fixed-width signature and its mapping remains owned below.
        let entry: ProbeEntry = unsafe { std::mem::transmute(published.as_ptr()) };
        let mut memory = Memory {
            bytes: [0; 16],
            reads: 0,
            writes: 0,
            fail: false,
        };
        memory.bytes[4..12].copy_from_slice(&0x8877_6655_4433_2211_u64.to_le_bytes());
        let mut staged = [0xAA; 8];
        let mut outcome = CheckedMemoryOutcomeV1::default();
        let mut state = probe_state();
        state.gpr[0] = 0xAAAA_BBBB_CCCC_DDDD;
        let mut retired = 41;
        let status = {
            let mut adapter = CheckedMemoryAdapter::new(&mut memory, AccessContext::default());
            // SAFETY: adapter and output buffers live across the synchronous call.
            unsafe {
                entry(
                    adapter.as_opaque_ptr(),
                    MEMORY_OP_READ,
                    4,
                    8,
                    staged.as_mut_ptr(),
                    &mut outcome,
                    state.gpr.as_mut_ptr(),
                    &mut retired,
                )
            }
        };
        assert_eq!(status, MEMORY_HELPER_SUCCESS);
        assert_eq!(staged, 0x8877_6655_4433_2211_u64.to_le_bytes());
        assert_eq!(state.gpr[0], 0x8877_6655_4433_2211);
        assert_eq!(retired, 42);
        assert_eq!(memory.reads, 1);

        memory.fail = true;
        staged = [0xAA; 8];
        state = probe_state();
        state.gpr[0] = 0xAAAA_BBBB_CCCC_DDDD;
        let state_before = state.clone();
        retired = 41;
        let status = {
            let mut adapter = CheckedMemoryAdapter::new(&mut memory, AccessContext::default());
            // SAFETY: failure path uses valid invocation-local pointers.
            unsafe {
                entry(
                    adapter.as_opaque_ptr(),
                    MEMORY_OP_READ,
                    4,
                    8,
                    staged.as_mut_ptr(),
                    &mut outcome,
                    state.gpr.as_mut_ptr(),
                    &mut retired,
                )
            }
        };
        assert_eq!(status, MEMORY_HELPER_MEMORY_ERROR);
        assert_eq!(outcome.error_kind, MEMORY_ERROR_BACKEND);
        assert_eq!(staged, [0xAA; 8]);
        assert_eq!(state, state_before);
        assert_eq!(retired, 41);
    }
}
