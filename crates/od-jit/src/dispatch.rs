//! Iterative multi-block dispatch over a fully admitted immutable fixture.
//!
//! The executor boundary runs exactly one compiled block per call. A host
//! implementation can publish, execute, and drop that block before this loop
//! compiles or invokes the next one.

use od_isa::LongModeState;
use od_isa::execute::StepOutcome;
use od_isa::memory::{AccessContext, MemoryError};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::block::{
    AdmittedInstructionBoundaries, CodeMemory, CodeView, NativeRegisterV1Admission,
};
use crate::block_compile::{
    BlockCompileError, BlockCompileOutcome, build_dispatch_block, compile_block_for_dispatch,
    compile_block_for_native_add_rax_imm8,
    compile_block_for_native_add_rax_imm8_demand_flags_candidate,
    compile_block_for_native_cmp_rax_imm8,
    compile_block_for_native_cmp_rax_imm8_demand_flags_candidate, compile_block_for_native_dec_rax,
    compile_block_for_native_dec_rax_demand_flags_candidate, compile_block_for_native_inc_rax,
    compile_block_for_native_inc_rax_demand_flags_candidate, compile_block_for_native_lea_rax_rbx,
    compile_block_for_native_lea_rax_rbx_demand_flags_candidate,
    compile_block_for_native_lea_rax_rbx_disp8,
    compile_block_for_native_lea_rax_rbx_disp8_demand_flags_candidate,
    compile_block_for_native_lea_rax_rbx_rcx,
    compile_block_for_native_lea_rax_rbx_rcx_demand_flags_candidate,
    compile_block_for_native_lea_rax_rip_disp32,
    compile_block_for_native_lea_rax_rip_disp32_demand_flags_candidate,
    compile_block_for_native_mov_rax_imm32,
    compile_block_for_native_mov_rax_imm32_demand_flags_candidate,
    compile_block_for_native_neg_rax, compile_block_for_native_neg_rax_demand_flags_candidate,
    compile_block_for_native_not_rax, compile_block_for_native_not_rax_demand_flags_candidate,
    compile_block_for_native_register_v1,
    compile_block_for_native_register_v1_demand_flags_candidate,
    compile_block_for_native_sub_rax_imm8,
    compile_block_for_native_sub_rax_imm8_demand_flags_candidate,
    compile_block_for_native_xchg_rax_rbx,
    compile_block_for_native_xchg_rax_rbx_demand_flags_candidate, compile_block_for_nop_fallback,
    compile_block_for_nop_fallback_demand_flags_candidate, compile_block_for_nop_inc_rax_fallback,
    compile_block_for_nop_inc_rax_fallback_demand_flags_candidate,
};
#[cfg(feature = "phase-observation")]
use crate::phase_observation::PhaseRecorder;
use crate::phase_observation::{DispatchPhase, NoopObserver, PhaseObserver, measure};
use crate::runtime::RuntimeExitTag;
use crate::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CompileFlavor,
    ExecutionContextSnapshot, ExecutionProfile, MappingId, MappingVersion, TranslationKey,
    TranslationKeyError,
};

/// Owner notification that makes cached code unreachable before the next
/// synchronous dispatch invocation. Call only between dispatch calls, when
/// every cached entry is inactive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodeInvalidation {
    BackingContentChanged {
        backing: BackingId,
        current_version: BackingVersion,
    },
    MappingReplaced {
        address_space: AddressSpaceId,
        mapping: MappingId,
        current_version: MappingVersion,
    },
    MappingRemoved {
        address_space: AddressSpaceId,
        mapping: MappingId,
    },
    ExecutePermissionChanged {
        address_space: AddressSpaceId,
        mapping: MappingId,
        executable: bool,
    },
}

pub(crate) fn dependencies_match_invalidation(
    dependencies: &[crate::translation::CodeDependencySpan],
    change: CodeInvalidation,
) -> bool {
    dependencies.iter().any(|span| match change {
        CodeInvalidation::BackingContentChanged {
            backing,
            current_version,
        } => span.identity.backing == backing && span.identity.backing_version != current_version,
        CodeInvalidation::MappingReplaced {
            address_space,
            mapping,
            current_version,
        } => {
            span.identity.address_space == address_space
                && span.identity.mapping == mapping
                && span.identity.mapping_version != current_version
        }
        CodeInvalidation::MappingRemoved {
            address_space,
            mapping,
        } => span.identity.address_space == address_space && span.identity.mapping == mapping,
        CodeInvalidation::ExecutePermissionChanged {
            address_space,
            mapping,
            executable,
        } => {
            span.identity.address_space == address_space
                && span.identity.mapping == mapping
                && span.identity.executable != executable
        }
    })
}

fn entry_matches_invalidation<H>(entry: &CachedEntry<H>, change: CodeInvalidation) -> bool {
    dependencies_match_invalidation(entry.key.dependencies(), change)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockExecutionReport {
    pub exit: RuntimeExitTag,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
}

pub use crate::block::S0_09NopAddRaxImm8Admission as AddRaxImm8InterpreterFallbackAdmission;
pub use crate::block::S0_09NopAdmission as InterpreterFallbackAdmission;
pub use crate::block::S0_09NopCmpRaxImm8Admission as CmpRaxImm8InterpreterFallbackAdmission;
pub use crate::block::S0_09NopDecRaxAdmission as DecRaxInterpreterFallbackAdmission;
pub use crate::block::S0_09NopIncRaxAdmission as IncRaxInterpreterFallbackAdmission;
pub use crate::block::S0_09NopLeaRaxRbxAdmission as LeaRaxRbxInterpreterFallbackAdmission;
pub use crate::block::S0_09NopLeaRaxRbxDisp8Admission as LeaRaxRbxDisp8InterpreterFallbackAdmission;
pub use crate::block::S0_09NopLeaRaxRbxRcxAdmission as LeaRaxRbxRcxInterpreterFallbackAdmission;
pub use crate::block::S0_09NopLeaRaxRipDisp32Admission as LeaRaxRipDisp32InterpreterFallbackAdmission;
pub use crate::block::S0_09NopMovRaxImm32Admission as MovRaxImm32InterpreterFallbackAdmission;
pub use crate::block::S0_09NopNegRaxAdmission as NegRaxInterpreterFallbackAdmission;
pub use crate::block::S0_09NopNotRaxAdmission as NotRaxInterpreterFallbackAdmission;
pub use crate::block::S0_09NopSubRaxImm8Admission as SubRaxImm8InterpreterFallbackAdmission;
pub use crate::block::S0_09NopXchgRaxRbxAdmission as XchgRaxRbxInterpreterFallbackAdmission;

/// One cooperative control context is shared for the full dispatch invocation.
/// STOP fields are forwarded to generated guards; an event request is sampled
/// only by the host dispatcher between completed blocks or interpreter steps.
#[derive(Clone, Copy)]
pub struct DispatchControl<'a> {
    pub stop_rip: Option<u64>,
    pub stop_requested: &'a AtomicU8,
    /// Optional cooperative host event request. Dispatch observes it only
    /// between completed native blocks or interpreter fallback steps.
    pub event_requested: Option<&'a AtomicBool>,
}

/// Executor contract for fallback dispatch. Implementations must pass this
/// exact stop address and atomic to the generated block invocation and release
/// the executable mapping before returning. Event requests are host-side and
/// do not enter the generated runtime ABI.
pub trait ControlledBlockExecutor {
    type Error;

    fn execute_one_block_controlled(
        &mut self,
        block: &crate::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<BlockExecutionReport, Self::Error>;

    /// Event-aware publication hook. Executors that publish before entering
    /// native code should override this method and sample `event_requested`
    /// after publication, dropping the mapping before native entry when an
    /// event is pending. The default preserves compatibility for executors
    /// whose publication and entry are inseparable.
    fn execute_one_block_controlled_with_event(
        &mut self,
        block: &crate::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<ControlledBlockExecution, Self::Error> {
        if control.stop_rip == Some(state.rip) {
            return Ok(ControlledBlockExecution::Yield(DispatchExit::Runtime(
                RuntimeExitTag::StopAddress,
            )));
        }
        if control.stop_requested.load(Ordering::Acquire) != 0 {
            return Ok(ControlledBlockExecution::Yield(DispatchExit::Runtime(
                RuntimeExitTag::StopRequested,
            )));
        }
        if incoming_budget == 0 {
            return Ok(ControlledBlockExecution::Yield(
                DispatchExit::BudgetExhausted,
            ));
        }
        if control
            .event_requested
            .is_some_and(|event| event.load(Ordering::Acquire))
        {
            return Ok(ControlledBlockExecution::Yield(DispatchExit::EventPending));
        }
        self.execute_one_block_controlled(block, state, incoming_budget, control)
            .map(ControlledBlockExecution::Executed)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlledBlockExecution {
    Executed(BlockExecutionReport),
    Yield(DispatchExit),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FallbackDispatchResult {
    pub exit: DispatchExit,
    pub remaining_budget: u64,
    pub retired_native: u64,
    pub retired_interpreter: u64,
    pub blocks_executed: u64,
}

/// Executes one block and returns only after all resources for it are released.
pub trait BlockExecutor {
    type Error;

    fn execute_one_block(
        &mut self,
        block: &crate::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
    ) -> Result<BlockExecutionReport, Self::Error>;
}

/// One-block publication backend for the opt-in one-entry cache. The
/// associated handle stays opaque to od-jit and remains owned by the cache
/// across calls. Existing release-on-return executor traits are unchanged.
pub trait CachedBlockBackend {
    type Error;
    type Published;

    fn publish_cached_block(
        &mut self,
        block: &crate::block_compile::CompiledBlock,
    ) -> Result<Self::Published, Self::Error>;

    fn execute_cached_block(
        &mut self,
        published: &Self::Published,
        block: &crate::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<BlockExecutionReport, Self::Error>;
}

/// Capacity contract for backends that can retain more than one published
/// block at once. This stays separate so existing single-block backends keep
/// their current implementation and release-on-return API.
pub trait MultiBlockCachedBackend: CachedBlockBackend {
    fn published_block_capacity(&self) -> usize;
}

pub const MAX_SINGLE_ENTRY_CACHE_BYTES: usize = 512 * 1024;
pub const MAX_MULTI_ENTRY_CACHE_BLOCKS: usize = 16;
pub const MAX_MULTI_ENTRY_CACHE_CODE_BYTES: usize = 65_536;
pub const MAX_MULTI_ENTRY_CACHE_METADATA_BYTES: usize = 512 * 1024;

/// Replacement policy for bounded multi-entry caches. FIFO remains the
/// default; LRU is opt-in for measurement and carries no performance claim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CacheEvictionPolicy {
    #[default]
    Fifo,
    Lru,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheConfigError {
    RequiresExactlyOneBlock { requested: usize },
    InvalidByteLimit { requested: usize, max: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheError {
    EntryTooLarge { actual: usize, max: usize },
    MinimumEntryTooLarge { minimum: usize, max: usize },
    SizeOverflow,
    CounterOverflow,
}

struct CachedEntry<H> {
    key: TranslationKey,
    block: crate::block_compile::CompiledBlock,
    published: H,
    logical_bytes: usize,
}

trait CachedEntryStore<H> {
    fn find(&self, key: &TranslationKey) -> Option<usize>;
    fn entry(&self, index: usize) -> Option<&CachedEntry<H>>;
    fn check_minimum(&self, minimum: usize) -> Result<(), CacheError>;
    fn size_for_candidate(
        &self,
        key: &TranslationKey,
        block: &crate::block_compile::CompiledBlock,
    ) -> Result<usize, CacheError>;
    fn prepare_install(
        &mut self,
        code_bytes: usize,
        logical_bytes: usize,
    ) -> Result<(), CacheError>;
    fn install(&mut self, entry: CachedEntry<H>) -> Result<usize, CacheError>;
    fn remove(&mut self, index: usize) -> Result<(), CacheError>;
    fn record_hit(&mut self, index: usize) -> Result<(), CacheError>;
    fn record_miss(&mut self) -> Result<(), CacheError>;
    fn record_compile(&mut self) -> Result<(), CacheError>;
    fn record_publish(&mut self, code_bytes: usize) -> Result<(), CacheError>;
    fn bind_execution_context(
        &mut self,
        context: ExecutionContextSnapshot,
    ) -> Result<(), ExecutionContextError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionContextError {
    UnsupportedProfile(ExecutionProfile),
    GenerationNotAdvanced { current: u64, requested: u64 },
    UnsupportedCr3State { value: u64 },
}

fn validate_execution_context(
    current: Option<ExecutionContextSnapshot>,
    requested: ExecutionContextSnapshot,
) -> Result<bool, ExecutionContextError> {
    if !requested.profile.is_supported_cached_fixture() {
        return Err(ExecutionContextError::UnsupportedProfile(requested.profile));
    }
    let Some(current) = current else {
        return Ok(true);
    };
    if current == requested {
        return Ok(false);
    }
    if requested.id <= current.id {
        return Err(ExecutionContextError::GenerationNotAdvanced {
            current: current.id.get(),
            requested: requested.id.get(),
        });
    }
    Ok(true)
}

/// Explicitly bounded one-entry cache. Published host memory stays alive only
/// while this slot owns its opaque handle. The byte limit counts key contents,
/// compiled code/state-map contents, and one equivalent publication copy;
/// host allocator bookkeeping, page rounding, and opaque handle internals are
/// excluded and are not a process-RSS bound.
pub struct SingleEntryBlockCache<H> {
    max_bytes: usize,
    entry: Option<CachedEntry<H>>,
    execution_context: Option<ExecutionContextSnapshot>,
    counters: SingleEntryCacheCounters,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SingleEntryCacheCounters {
    pub hits: u64,
    pub misses: u64,
    pub successful_compiles: u64,
    pub successful_publications: u64,
    pub replacements: u64,
    pub resident_entries_high_water: usize,
    pub resident_code_bytes_high_water: usize,
    pub resident_logical_metadata_bytes_high_water: usize,
    /// Total native bytes successfully published by this cache. Available to
    /// benchmark builds that enable phase observation; it is not runtime policy.
    #[cfg(feature = "phase-observation")]
    pub published_code_bytes: u64,
}

impl<H> SingleEntryBlockCache<H> {
    pub fn new(max_blocks: usize, max_bytes: usize) -> Result<Self, CacheConfigError> {
        if max_blocks != 1 {
            return Err(CacheConfigError::RequiresExactlyOneBlock {
                requested: max_blocks,
            });
        }
        if max_bytes == 0 || max_bytes > MAX_SINGLE_ENTRY_CACHE_BYTES {
            return Err(CacheConfigError::InvalidByteLimit {
                requested: max_bytes,
                max: MAX_SINGLE_ENTRY_CACHE_BYTES,
            });
        }
        Ok(Self {
            max_bytes,
            entry: None,
            execution_context: None,
            counters: SingleEntryCacheCounters::default(),
        })
    }

    pub const fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn resident_entries(&self) -> usize {
        usize::from(self.entry.is_some())
    }

    pub fn resident_logical_bytes(&self) -> usize {
        self.entry.as_ref().map_or(0, |entry| entry.logical_bytes)
    }

    pub fn resident_code_bytes(&self) -> usize {
        self.entry.as_ref().map_or(0, |entry| {
            entry.block.emitted().code().words().len() * std::mem::size_of::<u32>()
        })
    }

    pub const fn counters(&self) -> SingleEntryCacheCounters {
        self.counters
    }

    pub fn clear(&mut self) {
        self.entry = None;
    }

    /// Drop this entry when an owner reports a committed dependency change.
    /// Invoke only between dispatch calls; `&mut self` prevents a caller from
    /// invalidating while the cached entry is executing.
    pub fn invalidate(&mut self, change: CodeInvalidation) -> usize {
        if self
            .entry
            .as_ref()
            .is_some_and(|entry| entry_matches_invalidation(entry, change))
        {
            self.clear();
            1
        } else {
            0
        }
    }

    /// Flush entries and bind the cache to a new context generation. Reusing
    /// the current generation for a reset is rejected.
    pub fn reset_for_execution_context(
        &mut self,
        context: ExecutionContextSnapshot,
    ) -> Result<(), ExecutionContextError> {
        let Some(current) = self.execution_context else {
            if !context.profile.is_supported_cached_fixture() {
                return Err(ExecutionContextError::UnsupportedProfile(context.profile));
            }
            self.clear();
            self.execution_context = Some(context);
            return Ok(());
        };
        if !validate_execution_context(Some(current), context)? || context.id == current.id {
            return Err(ExecutionContextError::GenerationNotAdvanced {
                current: current.id.get(),
                requested: context.id.get(),
            });
        }
        self.clear();
        self.execution_context = Some(context);
        Ok(())
    }

    fn install(&mut self, entry: CachedEntry<H>) -> Result<(), CacheError> {
        let metadata_bytes = cached_entry_metadata_size::<H>(&entry.key, &entry.block)?;
        self.counters.resident_entries_high_water =
            self.counters.resident_entries_high_water.max(1);
        self.counters.resident_code_bytes_high_water = self
            .counters
            .resident_code_bytes_high_water
            .max(entry.block.emitted().code().words().len() * std::mem::size_of::<u32>());
        self.counters.resident_logical_metadata_bytes_high_water = self
            .counters
            .resident_logical_metadata_bytes_high_water
            .max(metadata_bytes);
        self.entry = Some(entry);
        Ok(())
    }
}

impl<H> CachedEntryStore<H> for SingleEntryBlockCache<H> {
    fn find(&self, key: &TranslationKey) -> Option<usize> {
        self.entry
            .as_ref()
            .filter(|entry| entry.key == *key)
            .map(|_| 0)
    }

    fn entry(&self, index: usize) -> Option<&CachedEntry<H>> {
        (index == 0).then(|| self.entry.as_ref()).flatten()
    }

    fn check_minimum(&self, minimum: usize) -> Result<(), CacheError> {
        if minimum > self.max_bytes {
            Err(CacheError::MinimumEntryTooLarge {
                minimum,
                max: self.max_bytes,
            })
        } else {
            Ok(())
        }
    }

    fn size_for_candidate(
        &self,
        key: &TranslationKey,
        block: &crate::block_compile::CompiledBlock,
    ) -> Result<usize, CacheError> {
        cached_entry_size::<H>(key, block)
    }

    fn prepare_install(
        &mut self,
        _code_bytes: usize,
        logical_bytes: usize,
    ) -> Result<(), CacheError> {
        if logical_bytes > self.max_bytes {
            return Err(CacheError::EntryTooLarge {
                actual: logical_bytes,
                max: self.max_bytes,
            });
        }
        if self.entry.is_some() {
            Self::add_counter(&mut self.counters.replacements)?;
        }
        self.clear();
        Ok(())
    }

    fn install(&mut self, entry: CachedEntry<H>) -> Result<usize, CacheError> {
        SingleEntryBlockCache::install(self, entry)?;
        Ok(0)
    }

    fn remove(&mut self, index: usize) -> Result<(), CacheError> {
        if index == 0 {
            self.clear();
        }
        Ok(())
    }

    fn record_hit(&mut self, _index: usize) -> Result<(), CacheError> {
        Self::add_counter(&mut self.counters.hits)
    }
    fn record_miss(&mut self) -> Result<(), CacheError> {
        Self::add_counter(&mut self.counters.misses)
    }
    fn record_compile(&mut self) -> Result<(), CacheError> {
        Self::add_counter(&mut self.counters.successful_compiles)
    }
    fn record_publish(&mut self, code_bytes: usize) -> Result<(), CacheError> {
        #[cfg(feature = "phase-observation")]
        let published_code_bytes =
            u64::try_from(code_bytes).map_err(|_| CacheError::SizeOverflow)?;
        #[cfg(feature = "phase-observation")]
        let total_published_code_bytes = self
            .counters
            .published_code_bytes
            .checked_add(published_code_bytes)
            .ok_or(CacheError::CounterOverflow)?;
        #[cfg(not(feature = "phase-observation"))]
        let _ = code_bytes;
        Self::add_counter(&mut self.counters.successful_publications)?;
        #[cfg(feature = "phase-observation")]
        {
            self.counters.published_code_bytes = total_published_code_bytes;
        }
        Ok(())
    }

    fn bind_execution_context(
        &mut self,
        context: ExecutionContextSnapshot,
    ) -> Result<(), ExecutionContextError> {
        if validate_execution_context(self.execution_context, context)? {
            self.clear();
            self.execution_context = Some(context);
        }
        Ok(())
    }
}

impl<H> SingleEntryBlockCache<H> {
    fn add_counter(counter: &mut u64) -> Result<(), CacheError> {
        *counter = counter.checked_add(1).ok_or(CacheError::CounterOverflow)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MultiEntryCacheCounters {
    pub hits: u64,
    pub misses: u64,
    pub successful_compiles: u64,
    pub successful_publications: u64,
    pub evictions: u64,
    pub resident_code_bytes: usize,
    pub resident_logical_metadata_bytes: usize,
    pub resident_entries_high_water: usize,
    pub resident_code_bytes_high_water: usize,
    pub resident_logical_metadata_bytes_high_water: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MultiEntryCacheConfigError {
    InvalidEntryLimit { requested: usize, max: usize },
    InvalidCodeLimit { requested: usize, max: usize },
    InvalidMetadataLimit { requested: usize, max: usize },
    BackendCapacityTooSmall { requested: usize, available: usize },
}

/// Bounded multi-entry cache for published blocks. The default policy is FIFO;
/// an opt-in policy may refresh hit order. Limits account for emitted bytes
/// and logical key/block/publication metadata, not allocator bookkeeping or RSS.
pub struct MultiEntryBlockCache<H> {
    max_entries: usize,
    max_code_bytes: usize,
    max_metadata_bytes: usize,
    eviction_policy: CacheEvictionPolicy,
    entries: Vec<CachedEntry<H>>,
    execution_context: Option<ExecutionContextSnapshot>,
    counters: MultiEntryCacheCounters,
}

impl<H> MultiEntryBlockCache<H> {
    pub fn new(
        max_entries: usize,
        max_code_bytes: usize,
        max_metadata_bytes: usize,
    ) -> Result<Self, MultiEntryCacheConfigError> {
        Self::with_policy(
            max_entries,
            max_code_bytes,
            max_metadata_bytes,
            CacheEvictionPolicy::Fifo,
        )
    }

    pub fn with_policy(
        max_entries: usize,
        max_code_bytes: usize,
        max_metadata_bytes: usize,
        eviction_policy: CacheEvictionPolicy,
    ) -> Result<Self, MultiEntryCacheConfigError> {
        if max_entries == 0 || max_entries > MAX_MULTI_ENTRY_CACHE_BLOCKS {
            return Err(MultiEntryCacheConfigError::InvalidEntryLimit {
                requested: max_entries,
                max: MAX_MULTI_ENTRY_CACHE_BLOCKS,
            });
        }
        if max_code_bytes == 0 || max_code_bytes > MAX_MULTI_ENTRY_CACHE_CODE_BYTES {
            return Err(MultiEntryCacheConfigError::InvalidCodeLimit {
                requested: max_code_bytes,
                max: MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            });
        }
        if max_metadata_bytes == 0 || max_metadata_bytes > MAX_MULTI_ENTRY_CACHE_METADATA_BYTES {
            return Err(MultiEntryCacheConfigError::InvalidMetadataLimit {
                requested: max_metadata_bytes,
                max: MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
            });
        }
        Ok(Self {
            max_entries,
            max_code_bytes,
            max_metadata_bytes,
            eviction_policy,
            entries: Vec::with_capacity(max_entries),
            execution_context: None,
            counters: MultiEntryCacheCounters::default(),
        })
    }

    pub const fn max_entries(&self) -> usize {
        self.max_entries
    }
    pub const fn max_code_bytes(&self) -> usize {
        self.max_code_bytes
    }
    pub const fn max_metadata_bytes(&self) -> usize {
        self.max_metadata_bytes
    }
    pub const fn eviction_policy(&self) -> CacheEvictionPolicy {
        self.eviction_policy
    }
    pub const fn counters(&self) -> MultiEntryCacheCounters {
        self.counters
    }
    pub fn resident_entries(&self) -> usize {
        self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.counters.resident_code_bytes = 0;
        self.counters.resident_logical_metadata_bytes = 0;
    }

    /// Remove every inactive entry depending on the changed backing or mapping.
    /// Call only between synchronous dispatch invocations. Removed published
    /// handles are dropped immediately and their accounted resident bytes are
    /// reclaimed.
    pub fn invalidate(&mut self, change: CodeInvalidation) -> usize {
        let mut removed = 0;
        let mut index = self.entries.len();
        while index != 0 {
            index -= 1;
            if self
                .entries
                .get(index)
                .is_some_and(|entry| entry_matches_invalidation(entry, change))
            {
                self.remove(index)
                    .expect("cache accounting remains consistent while invalidating entries");
                removed += 1;
            }
        }
        removed
    }

    /// Flush entries and bind the cache to a new context generation. Reusing
    /// the current generation for a reset is rejected.
    pub fn reset_for_execution_context(
        &mut self,
        context: ExecutionContextSnapshot,
    ) -> Result<(), ExecutionContextError> {
        if !context.profile.is_supported_cached_fixture() {
            return Err(ExecutionContextError::UnsupportedProfile(context.profile));
        }
        let Some(current) = self.execution_context else {
            self.clear();
            self.execution_context = Some(context);
            return Ok(());
        };
        if !validate_execution_context(Some(current), context)? || context.id == current.id {
            return Err(ExecutionContextError::GenerationNotAdvanced {
                current: current.id.get(),
                requested: context.id.get(),
            });
        }
        self.clear();
        self.execution_context = Some(context);
        Ok(())
    }

    fn add_counter(counter: &mut u64) -> Result<(), CacheError> {
        *counter = counter.checked_add(1).ok_or(CacheError::CounterOverflow)?;
        Ok(())
    }

    fn evict_oldest(&mut self) -> Result<(), CacheError> {
        let oldest = self.entries.first().ok_or(CacheError::SizeOverflow)?;
        let oldest_code_bytes = emitted_code_bytes(&oldest.block)?;
        let code_bytes = self
            .counters
            .resident_code_bytes
            .checked_sub(oldest_code_bytes)
            .ok_or(CacheError::SizeOverflow)?;
        let metadata_bytes = self
            .counters
            .resident_logical_metadata_bytes
            .checked_sub(oldest.logical_bytes)
            .ok_or(CacheError::SizeOverflow)?;
        let entry = self.entries.remove(0);
        self.counters.resident_code_bytes = code_bytes;
        self.counters.resident_logical_metadata_bytes = metadata_bytes;
        Self::add_counter(&mut self.counters.evictions)?;
        drop(entry);
        Ok(())
    }
}

impl<H> CachedEntryStore<H> for MultiEntryBlockCache<H> {
    fn find(&self, key: &TranslationKey) -> Option<usize> {
        self.entries.iter().position(|entry| entry.key == *key)
    }

    fn entry(&self, index: usize) -> Option<&CachedEntry<H>> {
        self.entries.get(index)
    }

    fn check_minimum(&self, minimum: usize) -> Result<(), CacheError> {
        if minimum > self.max_metadata_bytes {
            Err(CacheError::MinimumEntryTooLarge {
                minimum,
                max: self.max_metadata_bytes,
            })
        } else {
            Ok(())
        }
    }

    fn size_for_candidate(
        &self,
        key: &TranslationKey,
        block: &crate::block_compile::CompiledBlock,
    ) -> Result<usize, CacheError> {
        cached_entry_metadata_size::<H>(key, block)
    }

    fn prepare_install(
        &mut self,
        code_bytes: usize,
        logical_bytes: usize,
    ) -> Result<(), CacheError> {
        if code_bytes > self.max_code_bytes {
            return Err(CacheError::EntryTooLarge {
                actual: code_bytes,
                max: self.max_code_bytes,
            });
        }
        if logical_bytes > self.max_metadata_bytes {
            return Err(CacheError::EntryTooLarge {
                actual: logical_bytes,
                max: self.max_metadata_bytes,
            });
        }
        while self.entries.len() >= self.max_entries
            || self
                .counters
                .resident_code_bytes
                .checked_add(code_bytes)
                .ok_or(CacheError::SizeOverflow)?
                > self.max_code_bytes
            || self
                .counters
                .resident_logical_metadata_bytes
                .checked_add(logical_bytes)
                .ok_or(CacheError::SizeOverflow)?
                > self.max_metadata_bytes
        {
            self.evict_oldest()?;
        }
        Ok(())
    }

    fn install(&mut self, entry: CachedEntry<H>) -> Result<usize, CacheError> {
        let entry_code_bytes = emitted_code_bytes(&entry.block)?;
        let code_bytes = self
            .counters
            .resident_code_bytes
            .checked_add(entry_code_bytes)
            .ok_or(CacheError::SizeOverflow)?;
        let metadata_bytes = self
            .counters
            .resident_logical_metadata_bytes
            .checked_add(entry.logical_bytes)
            .ok_or(CacheError::SizeOverflow)?;
        self.counters.resident_code_bytes = code_bytes;
        self.counters.resident_logical_metadata_bytes = metadata_bytes;
        self.entries.push(entry);
        self.counters.resident_entries_high_water = self
            .counters
            .resident_entries_high_water
            .max(self.entries.len());
        self.counters.resident_code_bytes_high_water =
            self.counters.resident_code_bytes_high_water.max(code_bytes);
        self.counters.resident_logical_metadata_bytes_high_water = self
            .counters
            .resident_logical_metadata_bytes_high_water
            .max(metadata_bytes);
        Ok(self.entries.len() - 1)
    }

    fn remove(&mut self, index: usize) -> Result<(), CacheError> {
        if index < self.entries.len() {
            let current = &self.entries[index];
            let code_bytes = emitted_code_bytes(&current.block)?;
            let resident_code_bytes = self
                .counters
                .resident_code_bytes
                .checked_sub(code_bytes)
                .ok_or(CacheError::SizeOverflow)?;
            let resident_metadata_bytes = self
                .counters
                .resident_logical_metadata_bytes
                .checked_sub(current.logical_bytes)
                .ok_or(CacheError::SizeOverflow)?;
            let entry = self.entries.remove(index);
            self.counters.resident_code_bytes = resident_code_bytes;
            self.counters.resident_logical_metadata_bytes = resident_metadata_bytes;
            drop(entry);
        }
        Ok(())
    }

    fn record_hit(&mut self, index: usize) -> Result<(), CacheError> {
        Self::add_counter(&mut self.counters.hits)?;
        if self.eviction_policy == CacheEvictionPolicy::Lru && index < self.entries.len() {
            let touched = self.entries.remove(index);
            self.entries.push(touched);
        }
        Ok(())
    }
    fn record_miss(&mut self) -> Result<(), CacheError> {
        Self::add_counter(&mut self.counters.misses)
    }
    fn record_compile(&mut self) -> Result<(), CacheError> {
        Self::add_counter(&mut self.counters.successful_compiles)
    }
    fn record_publish(&mut self, _code_bytes: usize) -> Result<(), CacheError> {
        Self::add_counter(&mut self.counters.successful_publications)
    }

    fn bind_execution_context(
        &mut self,
        context: ExecutionContextSnapshot,
    ) -> Result<(), ExecutionContextError> {
        if validate_execution_context(self.execution_context, context)? {
            self.clear();
            self.execution_context = Some(context);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchExit {
    CodeEnd,
    BudgetExhausted,
    /// A host event was requested and the current guest work unit has ended.
    /// The request remains set; the caller handles it and resumes at `state.rip`.
    EventPending,
    Runtime(RuntimeExitTag),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchResult {
    pub exit: DispatchExit,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
    pub blocks_executed: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub enum DispatchError<E> {
    ExecutionContext(ExecutionContextError),
    AdmissionDoesNotMatchCode,
    EntryNotAdmitted { rip: u64 },
    Compile(BlockCompileError),
    NoNativePrefix { rip: u64 },
    Executor(E),
    RetiredExceedsBudget { retired: u64, incoming: u64 },
    RetiredExceedsBlock { retired: u64, emitted: u64 },
    InconsistentRemaining { expected: u64, actual: u64 },
    InvalidBudgetExit { remaining: u64 },
    AggregateOverflow,
    UnadmittedDispatchRip { rip: u64 },
    ZeroProgress { rip: u64 },
    TranslationKey(TranslationKeyError),
    StaleDependencySnapshot,
    Cache(CacheError),
    MultiCacheConfig(MultiEntryCacheConfigError),
    CachePublish(E),
    CacheExecutor(E),
    NopAdmissionDoesNotMatchCode,
    InterpreterInstructionNotAdmitted { rip: u64 },
    InterpreterStepFailure { rip: u64, outcome: StepOutcome },
    InterpreterMemoryError { rip: u64, error: MemoryError },
    InterpreterProgressMismatch { expected_rip: u64, actual_rip: u64 },
    RetirementInvariant { expected: u64, actual: u64 },
}

/// Run fixture-dispatch blocks through a one-entry resident cache. This is a
/// separate opt-in path; `run` retains its publish/execute/drop-per-block
/// contract. Dependency identities are caller-owned stable snapshots, and
/// must cover the entire immutable code view.
pub fn run_cached<B: CachedBlockBackend>(
    code: CodeView<'_>,
    admission: &AdmittedInstructionBoundaries,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    cache: &mut SingleEntryBlockCache<B::Published>,
    backend: &mut B,
) -> Result<DispatchResult, DispatchError<B::Error>> {
    let no_stop = AtomicU8::new(0);
    run_cached_controlled(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        DispatchControl {
            stop_rip: None,
            stop_requested: &no_stop,
            event_requested: None,
        },
        cache,
        backend,
    )
}

/// Controlled variant of [`run_cached`] with the same STOP/RIP guard order as
/// generated block entry. Cache lookup/compile/publication never bypass a
/// boundary guard and guest counters are sampled from the runtime report.
pub fn run_cached_controlled<B: CachedBlockBackend>(
    code: CodeView<'_>,
    admission: &AdmittedInstructionBoundaries,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut SingleEntryBlockCache<B::Published>,
    backend: &mut B,
) -> Result<DispatchResult, DispatchError<B::Error>> {
    let mut observer = NoopObserver;
    run_cached_store_controlled(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        control,
        cache,
        backend,
        &mut observer,
    )
}

/// Run the composed NativeRegisterV1 fixture through a one-entry eager cache.
/// Its cache key uses the dedicated NativeRegisterV1 semantic profile and an
/// eager-specific compile flavor. This is fixture-only: the caller supplies
/// the immutable code view, complete dependency snapshot and frozen
/// execution-context generation.
pub fn run_cached_native_register_v1_controlled<B: CachedBlockBackend>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut SingleEntryBlockCache<B::Published>,
    backend: &mut B,
) -> Result<FallbackDispatchResult, DispatchError<B::Error>> {
    let mut observer = NoopObserver;
    run_cached_native_register_v1_strategy_controlled::<false, false, _, _>(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        control,
        cache,
        backend,
        &mut observer,
        NativeRegisterV1CacheStrategy::Eager,
        None,
    )
}

/// Explicit benchmark-only eager NativeRegisterV1 path with dispatcher phase
/// timings. The default entrypoint retains its no-op observer path.
#[cfg(feature = "phase-observation")]
pub fn run_cached_native_register_v1_controlled_observed<B: CachedBlockBackend>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut SingleEntryBlockCache<B::Published>,
    backend: &mut B,
    observer: &mut crate::phase_observation::PhaseRecorder,
) -> Result<FallbackDispatchResult, DispatchError<B::Error>> {
    run_cached_native_register_v1_strategy_controlled::<false, false, _, _>(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        control,
        cache,
        backend,
        observer,
        NativeRegisterV1CacheStrategy::Eager,
        None,
    )
}

/// Run the opt-in demand-flags NativeRegisterV1 candidate through the same
/// bounded one-entry cache lifecycle as the eager path. Candidate artifacts
/// use a distinct compile flavor, so they cannot satisfy eager lookups.
pub fn run_cached_native_register_v1_demand_flags_candidate_controlled<B: CachedBlockBackend>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut SingleEntryBlockCache<B::Published>,
    backend: &mut B,
) -> Result<FallbackDispatchResult, DispatchError<B::Error>> {
    run_register_dispatch(
        code,
        admission,
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        SingleEntryRegisterPath {
            execution_context,
            dependencies,
            cache,
            backend,
        },
    )
}

/// Run the bounded NativeRegisterV1 fixture through the existing multi-entry
/// cache. The caller supplies an immutable code view, dependency snapshot and
/// execution context for each synchronous dispatch invocation.
pub fn run_multi_cached_native_register_v1_controlled<B: MultiBlockCachedBackend>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut MultiEntryBlockCache<B::Published>,
    backend: &mut B,
    flags: RegisterFlagsMode,
) -> Result<FallbackDispatchResult, DispatchError<B::Error>> {
    run_register_dispatch(
        code,
        admission,
        flags,
        state,
        total_budget,
        control,
        MultiEntryRegisterPath {
            execution_context,
            dependencies,
            cache,
            backend,
        },
    )
}

/// Run the eager NativeRegisterV1 cache path with a single last-RIP prepared
/// block/key memo scoped to this call. The cache lookup and all snapshot
/// validation still run on every iteration; this only skips rebuilding the
/// same native block and constructing its TranslationKey again.
pub fn run_cached_native_register_v1_prepared_key_candidate_controlled<B: CachedBlockBackend>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut SingleEntryBlockCache<B::Published>,
    backend: &mut B,
) -> Result<FallbackDispatchResult, DispatchError<B::Error>> {
    let mut observer = NoopObserver;
    run_cached_native_register_v1_strategy_controlled::<true, false, _, _>(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        control,
        cache,
        backend,
        &mut observer,
        NativeRegisterV1CacheStrategy::PreparedKeyReuseCandidate,
        None,
    )
}

/// Instrumented counterpart for untimed candidate preflight only. Do not use
/// this entrypoint for scored samples.
pub fn run_cached_native_register_v1_prepared_key_candidate_controlled_counted<
    B: CachedBlockBackend,
>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut SingleEntryBlockCache<B::Published>,
    backend: &mut B,
    counts: &mut PreparedKeyReuseCounts,
) -> Result<FallbackDispatchResult, DispatchError<B::Error>> {
    let mut observer = NoopObserver;
    run_cached_native_register_v1_strategy_controlled::<true, true, _, _>(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        control,
        cache,
        backend,
        &mut observer,
        NativeRegisterV1CacheStrategy::PreparedKeyReuseCandidate,
        Some(counts),
    )
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PreparedKeyReuseCounts {
    pub prepared_keys: u64,
    pub reused_keys: u64,
}

struct PreparedNativeRegisterKey {
    rip: u64,
    block: crate::block::BasicBlock,
    key: TranslationKey,
}

#[derive(Clone, Copy)]
enum NativeRegisterV1CacheStrategy {
    Eager,
    DemandFlagsCandidate,
    PreparedKeyReuseCandidate,
}

fn run_cached_native_register_v1_strategy_controlled<
    const REUSE_PREPARED_KEY: bool,
    const TRACK_REUSE_COUNTS: bool,
    B: CachedBlockBackend,
    O: PhaseObserver,
>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut impl CachedEntryStore<B::Published>,
    backend: &mut B,
    observer: &mut O,
    strategy: NativeRegisterV1CacheStrategy,
    mut reuse_counts: Option<&mut PreparedKeyReuseCounts>,
) -> Result<FallbackDispatchResult, DispatchError<B::Error>> {
    if let Some(value) = state.cr3 {
        return Err(DispatchError::ExecutionContext(
            ExecutionContextError::UnsupportedCr3State { value },
        ));
    }
    if execution_context.profile != ExecutionProfile::NATIVE_REGISTER_V1_LONG64_CANONICAL48 {
        return Err(DispatchError::ExecutionContext(
            ExecutionContextError::UnsupportedProfile(execution_context.profile),
        ));
    }
    if !admission.matches_code(code) {
        return Err(DispatchError::NopAdmissionDoesNotMatchCode);
    }
    if let Some(exit) = control_exit(state.rip, code.code_end(), total_budget, control) {
        return fallback_result(exit, total_budget, total_budget, 0, 0, 0);
    }
    if !admission.contains(state.rip) {
        return Err(DispatchError::EntryNotAdmitted { rip: state.rip });
    }
    if let Some(exit) = event_exit(control) {
        return fallback_result(exit, total_budget, total_budget, 0, 0, 0);
    }
    cache
        .bind_execution_context(execution_context)
        .map_err(DispatchError::ExecutionContext)?;

    let mut remaining = total_budget;
    let mut retired_native = 0u64;
    let mut retired_interpreter = 0u64;
    let mut blocks_executed = 0u64;
    let mut prepared_key: Option<PreparedNativeRegisterKey> = None;
    loop {
        if let Some(exit) = control_exit(state.rip, code.code_end(), remaining, control) {
            return fallback_result(
                exit,
                total_budget,
                remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            );
        }
        if !admission.contains(state.rip) {
            return Err(DispatchError::EntryNotAdmitted { rip: state.rip });
        }
        if let Some(exit) = event_exit(control) {
            return fallback_result(
                exit,
                total_budget,
                remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            );
        }
        if admission.admits_nop(state.rip) {
            prepared_key = None;
            if let Some(exit) = step_admitted_fallback::<B::Error>(
                code,
                FallbackAdmission::NativeRegisterV1(admission),
                state,
                &mut remaining,
                &mut retired_interpreter,
                total_budget,
                retired_native,
                control,
            )? {
                return fallback_result(
                    exit,
                    total_budget,
                    remaining,
                    retired_native,
                    retired_interpreter,
                    blocks_executed,
                );
            }
            validate_fallback_retirement_total(
                total_budget,
                remaining,
                retired_native,
                retired_interpreter,
            )?;
            continue;
        }

        let reuse_candidate = REUSE_PREPARED_KEY;
        let reuse_hit = reuse_candidate
            && prepared_key
                .as_ref()
                .is_some_and(|prepared| prepared.rip == state.rip);
        if !reuse_hit {
            prepared_key = None;
        }
        let guest_block = if reuse_hit {
            None
        } else {
            let built = measure(observer, DispatchPhase::BlockBuildAndAdmission, || {
                crate::block::build_block_for_native_register_v1(
                    code,
                    state.rip,
                    AccessContext::default(),
                    crate::block::BlockLimits::default(),
                )
            })
            .map_err(|error| DispatchError::Compile(BlockCompileError::Build(error)))?;
            let block = match built {
                crate::block::BuildOutcome::Block(block) => block,
                crate::block::BuildOutcome::NoBlock(_) => {
                    return Err(DispatchError::NoNativePrefix { rip: state.rip });
                }
            };
            Some(block)
        };
        if reuse_hit {
            if TRACK_REUSE_COUNTS {
                let counts = reuse_counts
                    .as_deref_mut()
                    .expect("counted candidate has a counter receiver");
                counts.reused_keys = counts
                    .reused_keys
                    .checked_add(1)
                    .ok_or(DispatchError::AggregateOverflow)?;
            }
        }
        let (key, hit_index) = measure(observer, DispatchPhase::KeySnapshotAndLookup, || {
            let block = guest_block
                .as_ref()
                .or_else(|| prepared_key.as_ref().map(|prepared| &prepared.block))
                .expect("block or prepared memo exists");
            let minimum = minimum_cache_key_size::<B::Published>(code, dependencies, block)
                .ok_or(DispatchError::Cache(CacheError::SizeOverflow))?;
            cache.check_minimum(minimum).map_err(DispatchError::Cache)?;
            if reuse_hit {
                let key = &prepared_key.as_ref().expect("memo exists").key;
                validate_cached_snapshot(key, code, dependencies)?;
                let hit_index = cache.find(key);
                if let Some(index) = hit_index {
                    cache.record_hit(index).map_err(DispatchError::Cache)?;
                } else {
                    cache.record_miss().map_err(DispatchError::Cache)?;
                }
                Ok::<_, DispatchError<B::Error>>((None, hit_index))
            } else {
                let block = guest_block.as_ref().expect("new block exists");
                let key = TranslationKey::for_block(
                    code,
                    block,
                    match strategy {
                        NativeRegisterV1CacheStrategy::Eager
                        | NativeRegisterV1CacheStrategy::PreparedKeyReuseCandidate => {
                            CompileFlavor::S3_38bNativeRegisterV1Cached
                        }
                        NativeRegisterV1CacheStrategy::DemandFlagsCandidate => {
                            CompileFlavor::S3_38cNativeRegisterV1DemandFlagsCachedCandidate
                        }
                    },
                    execution_context,
                    dependencies,
                )
                .map_err(DispatchError::TranslationKey)?;
                validate_cached_snapshot(&key, code, dependencies)?;
                let hit_index = cache.find(&key);
                if let Some(index) = hit_index {
                    cache.record_hit(index).map_err(DispatchError::Cache)?;
                } else {
                    cache.record_miss().map_err(DispatchError::Cache)?;
                }
                if reuse_candidate {
                    prepared_key = Some(PreparedNativeRegisterKey {
                        rip: state.rip,
                        block: block.clone(),
                        key: key.clone(),
                    });
                    if TRACK_REUSE_COUNTS {
                        let counts = reuse_counts
                            .as_deref_mut()
                            .expect("counted candidate has a counter receiver");
                        counts.prepared_keys = counts
                            .prepared_keys
                            .checked_add(1)
                            .ok_or(DispatchError::AggregateOverflow)?;
                    }
                }
                Ok((Some(key), hit_index))
            }
        })?;
        let key = key
            .as_ref()
            .or_else(|| prepared_key.as_ref().map(|prepared| &prepared.key))
            .expect("key or prepared memo exists");
        let cached_index = if hit_index.is_some() {
            // LRU hits can move the matched entry. Resolve its index again
            // after recording the hit before borrowing the published handle.
            cache.find(key).expect("hit remains resident")
        } else {
            let compilation = match strategy {
                NativeRegisterV1CacheStrategy::Eager => {
                    compile_block_for_native_register_v1(code, state.rip, admission)
                }
                NativeRegisterV1CacheStrategy::DemandFlagsCandidate => {
                    compile_block_for_native_register_v1_demand_flags_candidate(
                        code, state.rip, admission,
                    )
                }
                NativeRegisterV1CacheStrategy::PreparedKeyReuseCandidate => {
                    compile_block_for_native_register_v1(code, state.rip, admission)
                }
            };
            let compiled = match compilation.map_err(DispatchError::Compile)? {
                BlockCompileOutcome::Compiled(block) => block,
                BlockCompileOutcome::NoNativePrefix { rip, .. } => {
                    return Err(DispatchError::NoNativePrefix { rip });
                }
            };
            cache.record_compile().map_err(DispatchError::Cache)?;
            let logical_bytes = cache
                .size_for_candidate(key, &compiled)
                .map_err(DispatchError::Cache)?;
            let code_bytes = emitted_code_bytes(&compiled).map_err(DispatchError::Cache)?;
            cache
                .prepare_install(code_bytes, logical_bytes)
                .map_err(DispatchError::Cache)?;
            validate_cached_snapshot(key, code, dependencies)?;
            let published = measure(observer, DispatchPhase::Publication, || {
                backend.publish_cached_block(&compiled)
            })
            .map_err(DispatchError::CachePublish)?;
            cache
                .record_publish(code_bytes)
                .map_err(DispatchError::Cache)?;
            let published = validate_published_candidate(key, code, dependencies, published)?;
            CachedEntryStore::install(
                cache,
                CachedEntry {
                    key: key.clone(),
                    block: compiled,
                    published,
                    logical_bytes,
                },
            )
            .map_err(DispatchError::Cache)?
        };
        if reuse_candidate && hit_index.is_none() {
            // A cold install may replace the cache resident; never retain a
            // prepared memo across that replacement boundary.
            prepared_key = None;
        }
        let cached_key = &cache.entry(cached_index).expect("cache index valid").key;
        if let Err(error) = validate_cached_snapshot(cached_key, code, dependencies) {
            cache.remove(cached_index).map_err(DispatchError::Cache)?;
            drop(prepared_key.take());
            return Err(error);
        }
        if !cached_key.matches_code_view(code) {
            cache.remove(cached_index).map_err(DispatchError::Cache)?;
            drop(prepared_key.take());
            return Err(DispatchError::StaleDependencySnapshot);
        }
        let cached = cache.entry(cached_index).expect("cache index valid");
        if let Some(exit) = control_exit(state.rip, code.code_end(), remaining, control) {
            return fallback_result(
                exit,
                total_budget,
                remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            );
        }
        if let Some(exit) = event_exit(control) {
            return fallback_result(
                exit,
                total_budget,
                remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            );
        }
        let execution = measure(observer, DispatchPhase::HostExecutionBoundary, || {
            backend.execute_cached_block(
                &cached.published,
                &cached.block,
                state,
                remaining,
                control,
            )
        });
        let report = match execution {
            Ok(report) => report,
            Err(error) => {
                cache.remove(cached_index).map_err(DispatchError::Cache)?;
                drop(prepared_key.take());
                return Err(DispatchError::CacheExecutor(error));
            }
        };
        validate_counters(
            report,
            remaining,
            u64::from(cached.block.emitted().pc_map().instruction_count()),
        )?;
        remaining = report.remaining_budget;
        retired_native = retired_native
            .checked_add(report.retired_instructions)
            .ok_or(DispatchError::AggregateOverflow)?;
        blocks_executed = blocks_executed
            .checked_add(1)
            .ok_or(DispatchError::AggregateOverflow)?;
        validate_fallback_retirement_total(
            total_budget,
            remaining,
            retired_native,
            retired_interpreter,
        )?;
        match report.exit {
            RuntimeExitTag::NeedInterpreter => {
                prepared_key = None;
                let Some(exit) = step_admitted_fallback::<B::Error>(
                    code,
                    FallbackAdmission::NativeRegisterV1(admission),
                    state,
                    &mut remaining,
                    &mut retired_interpreter,
                    total_budget,
                    retired_native,
                    control,
                )?
                else {
                    validate_fallback_retirement_total(
                        total_budget,
                        remaining,
                        retired_native,
                        retired_interpreter,
                    )?;
                    continue;
                };
                return fallback_result(
                    exit,
                    total_budget,
                    remaining,
                    retired_native,
                    retired_interpreter,
                    blocks_executed,
                );
            }
            RuntimeExitTag::BlockComplete | RuntimeExitTag::DispatchCandidate => {
                if !admission.contains(state.rip) {
                    return Err(DispatchError::UnadmittedDispatchRip { rip: state.rip });
                }
                if let Some(exit) = control_exit(state.rip, code.code_end(), remaining, control) {
                    return fallback_result(
                        exit,
                        total_budget,
                        remaining,
                        retired_native,
                        retired_interpreter,
                        blocks_executed,
                    );
                }
                if report.retired_instructions == 0 {
                    return Err(DispatchError::ZeroProgress { rip: state.rip });
                }
                if let Some(exit) = event_exit(control) {
                    return fallback_result(
                        exit,
                        total_budget,
                        remaining,
                        retired_native,
                        retired_interpreter,
                        blocks_executed,
                    );
                }
            }
            RuntimeExitTag::BudgetExhausted => {
                if remaining != 0 {
                    return Err(DispatchError::InvalidBudgetExit { remaining });
                }
                let exit = control_exit(state.rip, code.code_end(), remaining, control)
                    .expect("zero remaining budget always produces a terminal control result");
                return fallback_result(
                    exit,
                    total_budget,
                    remaining,
                    retired_native,
                    retired_interpreter,
                    blocks_executed,
                );
            }
            exit => {
                return fallback_result(
                    DispatchExit::Runtime(exit),
                    total_budget,
                    remaining,
                    retired_native,
                    retired_interpreter,
                    blocks_executed,
                );
            }
        }
    }
}
/// Run through the bounded multi-entry cache. The configured entry count must
/// fit the backend's simultaneously published-block capacity.
pub fn run_multi_cached<B: MultiBlockCachedBackend>(
    code: CodeView<'_>,
    admission: &AdmittedInstructionBoundaries,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    cache: &mut MultiEntryBlockCache<B::Published>,
    backend: &mut B,
) -> Result<DispatchResult, DispatchError<B::Error>> {
    let no_stop = AtomicU8::new(0);
    run_multi_cached_controlled(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        DispatchControl {
            stop_rip: None,
            stop_requested: &no_stop,
            event_requested: None,
        },
        cache,
        backend,
    )
}

pub fn run_multi_cached_controlled<B: MultiBlockCachedBackend>(
    code: CodeView<'_>,
    admission: &AdmittedInstructionBoundaries,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut MultiEntryBlockCache<B::Published>,
    backend: &mut B,
) -> Result<DispatchResult, DispatchError<B::Error>> {
    let mut observer = NoopObserver;
    run_multi_cached_controlled_with_observer(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        control,
        cache,
        backend,
        &mut observer,
    )
}

/// Run through the same multi-entry dispatcher while collecting timings for
/// non-overlapping cache/compiler/host boundaries. Available only to opt-in
/// benchmark builds; ordinary callers use the no-op observer path.
#[cfg(feature = "phase-observation")]
pub fn run_multi_cached_controlled_observed<B: MultiBlockCachedBackend>(
    code: CodeView<'_>,
    admission: &AdmittedInstructionBoundaries,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut MultiEntryBlockCache<B::Published>,
    backend: &mut B,
    observer: &mut PhaseRecorder,
) -> Result<DispatchResult, DispatchError<B::Error>> {
    run_multi_cached_controlled_with_observer(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        control,
        cache,
        backend,
        observer,
    )
}

fn run_multi_cached_controlled_with_observer<B, O>(
    code: CodeView<'_>,
    admission: &AdmittedInstructionBoundaries,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut MultiEntryBlockCache<B::Published>,
    backend: &mut B,
    observer: &mut O,
) -> Result<DispatchResult, DispatchError<B::Error>>
where
    B: MultiBlockCachedBackend,
    O: PhaseObserver,
{
    if let Some(value) = state.cr3 {
        return Err(DispatchError::ExecutionContext(
            ExecutionContextError::UnsupportedCr3State { value },
        ));
    }
    if !execution_context.profile.is_supported_fixture() {
        return Err(DispatchError::ExecutionContext(
            ExecutionContextError::UnsupportedProfile(execution_context.profile),
        ));
    }
    let capacity = backend.published_block_capacity();
    if cache.max_entries() > capacity {
        return Err(DispatchError::MultiCacheConfig(
            MultiEntryCacheConfigError::BackendCapacityTooSmall {
                requested: cache.max_entries(),
                available: capacity,
            },
        ));
    }
    run_cached_store_controlled(
        code,
        admission,
        execution_context,
        dependencies,
        state,
        total_budget,
        control,
        cache,
        backend,
        observer,
    )
}

fn run_cached_store_controlled<B, C, O>(
    code: CodeView<'_>,
    admission: &AdmittedInstructionBoundaries,
    execution_context: ExecutionContextSnapshot,
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut C,
    backend: &mut B,
    observer: &mut O,
) -> Result<DispatchResult, DispatchError<B::Error>>
where
    B: CachedBlockBackend,
    C: CachedEntryStore<B::Published>,
    O: PhaseObserver,
{
    if let Some(value) = state.cr3 {
        return Err(DispatchError::ExecutionContext(
            ExecutionContextError::UnsupportedCr3State { value },
        ));
    }
    if !execution_context.profile.is_supported_fixture() {
        return Err(DispatchError::ExecutionContext(
            ExecutionContextError::UnsupportedProfile(execution_context.profile),
        ));
    }
    if !admission.matches_code(code) {
        return Err(DispatchError::AdmissionDoesNotMatchCode);
    }
    if control.stop_rip == Some(state.rip) {
        return Ok(result(
            DispatchExit::Runtime(RuntimeExitTag::StopAddress),
            total_budget,
            0,
            0,
        ));
    }
    if control.stop_requested.load(Ordering::Acquire) != 0 {
        return Ok(result(
            DispatchExit::Runtime(RuntimeExitTag::StopRequested),
            total_budget,
            0,
            0,
        ));
    }
    if state.rip == code.code_end() {
        return Ok(result(DispatchExit::CodeEnd, total_budget, 0, 0));
    }
    if !admission.contains(state.rip) {
        return Err(DispatchError::EntryNotAdmitted { rip: state.rip });
    }
    if total_budget == 0 {
        return Ok(result(DispatchExit::BudgetExhausted, 0, 0, 0));
    }
    if let Some(exit) = event_exit(control) {
        return Ok(result(exit, total_budget, 0, 0));
    }
    cache
        .bind_execution_context(execution_context)
        .map_err(DispatchError::ExecutionContext)?;

    let mut remaining = total_budget;
    let mut retired_total = 0u64;
    let mut blocks_executed = 0u64;

    loop {
        if let Some(exit) = control_exit(state.rip, code.code_end(), remaining, control) {
            return Ok(result(exit, remaining, retired_total, blocks_executed));
        }
        if !admission.contains(state.rip) {
            return Err(DispatchError::EntryNotAdmitted { rip: state.rip });
        }
        if let Some(exit) = event_exit(control) {
            return Ok(result(exit, remaining, retired_total, blocks_executed));
        }

        let built = measure(observer, DispatchPhase::BlockBuildAndAdmission, || {
            build_dispatch_block(code, state.rip, admission)
        })
        .map_err(DispatchError::Compile)?;
        let guest_block = match &built {
            crate::block::BuildOutcome::Block(block) => block,
            crate::block::BuildOutcome::NoBlock(no_block) => {
                return Err(DispatchError::NoNativePrefix {
                    rip: no_block.entry_rip,
                });
            }
        };
        let (key, hit) = measure(observer, DispatchPhase::KeySnapshotAndLookup, || {
            let minimum = minimum_cache_key_size::<B::Published>(code, dependencies, guest_block)
                .ok_or(DispatchError::Cache(CacheError::SizeOverflow))?;
            cache.check_minimum(minimum).map_err(DispatchError::Cache)?;
            let key = TranslationKey::for_block(
                code,
                guest_block,
                CompileFlavor::S0_09FixtureDispatch,
                execution_context,
                dependencies,
            )
            .map_err(DispatchError::TranslationKey)?;
            validate_cached_snapshot(&key, code, dependencies)?;

            let hit_index = cache.find(&key);
            let hit = hit_index.is_some();
            if let Some(index) = hit_index {
                cache.record_hit(index).map_err(DispatchError::Cache)?;
            } else {
                cache.record_miss().map_err(DispatchError::Cache)?;
            }
            Ok::<_, DispatchError<B::Error>>((key, hit))
        })?;

        let cached_index = if !hit {
            let compiled = match crate::block_compile::compile_prepared_dispatch_block_observed(
                code.code_end(),
                built,
                admission,
                observer,
            )
            .map_err(DispatchError::Compile)?
            {
                BlockCompileOutcome::Compiled(block) => block,
                BlockCompileOutcome::NoNativePrefix { rip, .. } => {
                    return Err(DispatchError::NoNativePrefix { rip });
                }
            };
            cache.record_compile().map_err(DispatchError::Cache)?;
            let logical_bytes = cache
                .size_for_candidate(&key, &compiled)
                .map_err(DispatchError::Cache)?;
            let code_bytes = emitted_code_bytes(&compiled).map_err(DispatchError::Cache)?;
            cache
                .prepare_install(code_bytes, logical_bytes)
                .map_err(DispatchError::Cache)?;

            validate_cached_snapshot(&key, code, dependencies)?;

            let published = measure(observer, DispatchPhase::Publication, || {
                backend.publish_cached_block(&compiled)
            })
            .map_err(DispatchError::CachePublish)?;
            cache
                .record_publish(code_bytes)
                .map_err(DispatchError::Cache)?;
            let published = validate_published_candidate(&key, code, dependencies, published)?;
            let index = cache
                .install(CachedEntry {
                    key,
                    block: compiled,
                    published,
                    logical_bytes,
                })
                .map_err(DispatchError::Cache)?;
            index
        } else {
            cache.find(&key).expect("hit index still resident")
        };

        let cached_key = &cache.entry(cached_index).expect("cache index valid").key;
        if let Err(error) = validate_cached_snapshot(cached_key, code, dependencies) {
            cache.remove(cached_index).map_err(DispatchError::Cache)?;
            return Err(error);
        }
        if !cached_key.matches_code_view(code) {
            cache.remove(cached_index).map_err(DispatchError::Cache)?;
            return Err(DispatchError::StaleDependencySnapshot);
        }
        let cached = cache.entry(cached_index).expect("cache index valid");
        let cached_block = &cached.block;
        let published = &cached.published;
        if let Some(exit) = control_exit(state.rip, code.code_end(), remaining, control) {
            return Ok(result(exit, remaining, retired_total, blocks_executed));
        }
        if let Some(exit) = event_exit(control) {
            return Ok(result(exit, remaining, retired_total, blocks_executed));
        }
        // Keep both the verified block metadata and published handle borrowed
        // across generated execution and host result validation. Cache clear,
        // eviction, or slot reuse requires `&mut cache` and therefore cannot
        // occur until this call has returned; an executor error is removed
        // only below, after its call has returned.
        let execution = measure(observer, DispatchPhase::HostExecutionBoundary, || {
            backend.execute_cached_block(published, cached_block, state, remaining, control)
        });
        let report = match execution {
            Ok(report) => report,
            Err(error) => {
                cache.remove(cached_index).map_err(DispatchError::Cache)?;
                return Err(DispatchError::CacheExecutor(error));
            }
        };
        validate_counters(
            report,
            remaining,
            u64::from(cached_block.emitted().pc_map().instruction_count()),
        )?;
        remaining = report.remaining_budget;
        retired_total = retired_total
            .checked_add(report.retired_instructions)
            .ok_or(DispatchError::AggregateOverflow)?;
        blocks_executed = blocks_executed
            .checked_add(1)
            .ok_or(DispatchError::AggregateOverflow)?;
        if retired_total > total_budget {
            return Err(DispatchError::AggregateOverflow);
        }

        match report.exit {
            RuntimeExitTag::BlockComplete | RuntimeExitTag::DispatchCandidate => {
                if !admission.contains(state.rip) {
                    return Err(DispatchError::UnadmittedDispatchRip { rip: state.rip });
                }
                if let Some(exit) = control_exit(state.rip, code.code_end(), remaining, control) {
                    return Ok(result(exit, remaining, retired_total, blocks_executed));
                }
                if report.retired_instructions == 0 {
                    return Err(DispatchError::ZeroProgress { rip: state.rip });
                }
                if let Some(exit) = event_exit(control) {
                    return Ok(result(exit, remaining, retired_total, blocks_executed));
                }
            }
            RuntimeExitTag::BudgetExhausted => {
                if remaining != 0 {
                    return Err(DispatchError::InvalidBudgetExit { remaining });
                }
                let exit = control_exit(state.rip, code.code_end(), remaining, control)
                    .expect("zero remaining budget always produces a terminal control result");
                return Ok(result(exit, remaining, retired_total, blocks_executed));
            }
            exit => {
                return Ok(result(
                    DispatchExit::Runtime(exit),
                    remaining,
                    retired_total,
                    blocks_executed,
                ));
            }
        }
    }
}

fn validate_cached_snapshot<E>(
    key: &TranslationKey,
    code: CodeView<'_>,
    dependencies: &[CodeDependencySpan],
) -> Result<(), DispatchError<E>> {
    if !key.matches_code_view(code)
        || !key
            .dependencies_match_snapshot(dependencies)
            .map_err(DispatchError::TranslationKey)?
    {
        return Err(DispatchError::StaleDependencySnapshot);
    }
    Ok(())
}

fn validate_published_candidate<E, H>(
    key: &TranslationKey,
    code: CodeView<'_>,
    dependencies: &[CodeDependencySpan],
    published: H,
) -> Result<H, DispatchError<E>> {
    if !key.matches_code_view(code) {
        return Err(DispatchError::StaleDependencySnapshot);
    }
    validate_cached_snapshot(key, code, dependencies)?;
    Ok(published)
}

fn cached_entry_size<H>(
    key: &TranslationKey,
    block: &crate::block_compile::CompiledBlock,
) -> Result<usize, CacheError> {
    let key_bytes = key
        .logical_storage_bytes()
        .ok_or(CacheError::SizeOverflow)?;
    let compiled_bytes = block
        .logical_storage_bytes()
        .ok_or(CacheError::SizeOverflow)?;
    // PublishedCode owns another code-byte and PcMap copy. Count that copy
    // even though the opaque backend handle's allocator bookkeeping is outside
    // this logical bound.
    let published_copy = block
        .emitted()
        .logical_storage_bytes()
        .ok_or(CacheError::SizeOverflow)?;
    key_bytes
        .checked_add(compiled_bytes)
        .and_then(|total| total.checked_add(published_copy))
        .and_then(|total| total.checked_add(std::mem::size_of::<CachedEntry<H>>()))
        .ok_or(CacheError::SizeOverflow)
}

fn cached_entry_metadata_size<H>(
    key: &TranslationKey,
    block: &crate::block_compile::CompiledBlock,
) -> Result<usize, CacheError> {
    let key_bytes = key
        .logical_storage_bytes()
        .ok_or(CacheError::SizeOverflow)?
        .checked_sub(std::mem::size_of::<TranslationKey>())
        .ok_or(CacheError::SizeOverflow)?;
    let map_bytes = block
        .emitted()
        .pc_map()
        .logical_storage_bytes()
        .ok_or(CacheError::SizeOverflow)?;
    std::mem::size_of::<CachedEntry<H>>()
        .checked_add(key_bytes)
        .and_then(|sum| {
            map_bytes
                .checked_mul(2)
                .and_then(|maps| sum.checked_add(maps))
        })
        .ok_or(CacheError::SizeOverflow)
}

fn emitted_code_bytes(block: &crate::block_compile::CompiledBlock) -> Result<usize, CacheError> {
    block
        .emitted()
        .code()
        .words()
        .len()
        .checked_mul(std::mem::size_of::<u32>())
        .ok_or(CacheError::SizeOverflow)
}

/// A lower bound available before TranslationKey clones source/dependency
/// buffers. It includes all key-owned payloads and the fixed cache wrapper; a
/// later exact logical check also accounts for compiled output and its
/// publication copy.
fn minimum_cache_key_size<H>(
    code: CodeView<'_>,
    dependencies: &[CodeDependencySpan],
    block: &crate::block::BasicBlock,
) -> Option<usize> {
    std::mem::size_of::<TranslationKey>()
        .checked_add(code.len())?
        .checked_add(
            dependencies
                .len()
                .checked_mul(std::mem::size_of::<CodeDependencySpan>())?,
        )?
        .checked_add(
            block
                .guest_ranges
                .len()
                .checked_mul(std::mem::size_of::<crate::block::GuestByteRange>())?,
        )?
        .checked_add(std::mem::size_of::<CachedEntry<H>>())
}

/// Run admitted blocks until the fixture ends, the exact guest budget is
/// consumed, or a typed runtime exit requires the caller to stop.
///
/// The entry-at-end check precedes the zero-budget check. No compile or
/// executor call occurs for a zero budget. Each successful continuation
/// consumes the counters reported by the generated block; it does not
/// precharge the block's maximum instruction count.
pub fn run<E: BlockExecutor>(
    code: CodeView<'_>,
    admission: &AdmittedInstructionBoundaries,
    state: &mut LongModeState,
    total_budget: u64,
    executor: &mut E,
) -> Result<DispatchResult, DispatchError<E::Error>> {
    if let Some(value) = state.cr3 {
        return Err(DispatchError::ExecutionContext(
            ExecutionContextError::UnsupportedCr3State { value },
        ));
    }
    if !admission.matches_code(code) {
        return Err(DispatchError::AdmissionDoesNotMatchCode);
    }
    if state.rip == code.code_end() {
        return Ok(DispatchResult {
            exit: DispatchExit::CodeEnd,
            remaining_budget: total_budget,
            retired_instructions: 0,
            blocks_executed: 0,
        });
    }
    if !admission.contains(state.rip) {
        return Err(DispatchError::EntryNotAdmitted { rip: state.rip });
    }
    if total_budget == 0 {
        return Ok(DispatchResult {
            exit: DispatchExit::BudgetExhausted,
            remaining_budget: 0,
            retired_instructions: 0,
            blocks_executed: 0,
        });
    }

    let mut remaining = total_budget;
    let mut retired_total = 0u64;
    let mut blocks_executed = 0u64;

    loop {
        let block = match compile_block_for_dispatch(code, state.rip, admission)
            .map_err(DispatchError::Compile)?
        {
            BlockCompileOutcome::Compiled(block) => block,
            BlockCompileOutcome::NoNativePrefix { rip, .. } => {
                return Err(DispatchError::NoNativePrefix { rip });
            }
        };

        let report = executor
            .execute_one_block(&block, state, remaining)
            .map_err(DispatchError::Executor)?;
        validate_counters(
            report,
            remaining,
            u64::from(block.emitted().pc_map().instruction_count()),
        )?;
        remaining = report.remaining_budget;
        retired_total = retired_total
            .checked_add(report.retired_instructions)
            .ok_or(DispatchError::AggregateOverflow)?;
        blocks_executed = blocks_executed
            .checked_add(1)
            .ok_or(DispatchError::AggregateOverflow)?;
        if retired_total > total_budget {
            return Err(DispatchError::AggregateOverflow);
        }

        match report.exit {
            RuntimeExitTag::BlockComplete | RuntimeExitTag::DispatchCandidate => {
                if !admission.contains(state.rip) {
                    return Err(DispatchError::UnadmittedDispatchRip { rip: state.rip });
                }
                if state.rip == code.code_end() {
                    return Ok(result(
                        DispatchExit::CodeEnd,
                        remaining,
                        retired_total,
                        blocks_executed,
                    ));
                }
                if remaining == 0 {
                    return Ok(result(
                        DispatchExit::BudgetExhausted,
                        remaining,
                        retired_total,
                        blocks_executed,
                    ));
                }
                if report.retired_instructions == 0 {
                    return Err(DispatchError::ZeroProgress { rip: state.rip });
                }
            }
            RuntimeExitTag::BudgetExhausted => {
                if remaining != 0 {
                    return Err(DispatchError::InvalidBudgetExit { remaining });
                }
                return Ok(result(
                    DispatchExit::BudgetExhausted,
                    remaining,
                    retired_total,
                    blocks_executed,
                ));
            }
            exit => {
                return Ok(result(
                    DispatchExit::Runtime(exit),
                    remaining,
                    retired_total,
                    blocks_executed,
                ));
            }
        }
    }
}

/// Run the S0-09 profile with a narrow interpreter bridge for exact bare NOP.
/// This API is intentionally separate from fixture-v1 `run` and its CLI
/// backend: the admission profile is S0-09 plus NOP, not arbitrary Stage 2.
pub fn run_with_nop_fallback<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_with_nop_fallback_strategy(
        code,
        FallbackAdmission::Nop(admission),
        state,
        total_budget,
        control,
        executor,
        NopFallbackCompilation::Eager,
    )
}

/// Opt-in uncached dispatcher for the demand-driven flags candidate. It uses
/// the same control, fallback, and accounting loop as [`run_with_nop_fallback`]
/// while compiling each reached native block with candidate flag materialization.
/// The shared translation cache and eager default API remain unchanged.
pub fn run_with_nop_fallback_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::Nop(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Run the opt-in S3-33n exact-form union without a shared translation cache.
/// Only admitted bare NOP instructions are handed to the interpreter.
pub fn run_with_native_register_v1<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeRegisterV1(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags correctness candidate for [`run_with_native_register_v1`].
/// It remains uncached; eager compilation is the reference path.
pub fn run_with_native_register_v1_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeRegisterV1(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher that admits exact `INC RAX` interpreter
/// handoffs between native S0-09 prefixes. Existing NOP-only APIs remain
/// bounded by their original admission type.
pub fn run_with_nop_inc_rax_fallback<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &IncRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_with_nop_fallback_strategy(
        code,
        FallbackAdmission::NopIncRax(admission),
        state,
        total_budget,
        control,
        executor,
        NopFallbackCompilation::Eager,
    )
}

pub fn run_with_nop_inc_rax_fallback_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &IncRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::InterpretedIncRax(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached native INC RAX dispatcher. The admission remains exact and
/// the existing interpreter-bridge API is unchanged.
pub fn run_with_native_inc_rax<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &IncRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeIncRax(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Uncached demand-flags candidate for the native INC RAX path. Any pending
/// full writer is materialized before INC; its partial write is then eager.
pub fn run_with_native_inc_rax_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &IncRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeIncRax(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached native DEC RAX dispatcher with its own exact admission.
pub fn run_with_native_dec_rax<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &DecRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeDecRax(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact DEC RAX. A pending full writer is
/// materialized before DEC, then DEC flags are merged eagerly.
pub fn run_with_native_dec_rax_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &DecRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeDecRax(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached native NEG RAX dispatcher with its own exact admission.
pub fn run_with_native_neg_rax<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &NegRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeNegRax(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact NEG RAX; it overwrites the full pending flags descriptor.
pub fn run_with_native_neg_rax_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &NegRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeNegRax(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached native dispatcher for exact `NOT RAX`; flags are preserved.
pub fn run_with_native_not_rax<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &NotRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeNotRax(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact NOT RAX, preserving any pending writer.
pub fn run_with_native_not_rax_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &NotRaxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeNotRax(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact native `LEA RAX,[RBX]`.
pub fn run_with_native_lea_rax_rbx<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &LeaRaxRbxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeLeaRaxRbx(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact native `LEA RAX,[RBX]`; flags are preserved.
pub fn run_with_native_lea_rax_rbx_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &LeaRaxRbxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeLeaRaxRbx(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact `LEA RAX,[RBX+sign-extended disp8]`.
pub fn run_with_native_lea_rax_rbx_disp8<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &LeaRaxRbxDisp8InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeLeaRaxRbxDisp8(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact LEA RAX,[RBX+disp8]; flags are preserved.
pub fn run_with_native_lea_rax_rbx_disp8_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &LeaRaxRbxDisp8InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeLeaRaxRbxDisp8(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact `LEA RAX,[RBX+RCX]`.
pub fn run_with_native_lea_rax_rbx_rcx<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &LeaRaxRbxRcxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeLeaRaxRbxRcx(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact LEA RAX,[RBX+RCX]; flags are preserved.
pub fn run_with_native_lea_rax_rbx_rcx_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &LeaRaxRbxRcxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeLeaRaxRbxRcx(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact `LEA RAX,[RIP+disp32]`.
pub fn run_with_native_lea_rax_rip_disp32<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &LeaRaxRipDisp32InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeLeaRaxRipDisp32(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

pub fn run_with_native_lea_rax_rip_disp32_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &LeaRaxRipDisp32InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeLeaRaxRipDisp32(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact native `XCHG RAX,RBX`.
pub fn run_with_native_xchg_rax_rbx<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &XchgRaxRbxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeXchgRaxRbx(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact native `XCHG RAX,RBX`; flags are preserved.
pub fn run_with_native_xchg_rax_rbx_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &XchgRaxRbxInterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeXchgRaxRbx(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact native `ADD RAX, sign-extended imm8`.
pub fn run_with_native_add_rax_imm8<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &AddRaxImm8InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeAddRaxImm8(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact native `ADD RAX, sign-extended imm8`.
pub fn run_with_native_add_rax_imm8_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &AddRaxImm8InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeAddRaxImm8(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact native `CMP RAX, sign-extended imm8`.
pub fn run_with_native_cmp_rax_imm8<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &CmpRaxImm8InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeCmpRaxImm8(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact native `CMP RAX, sign-extended imm8`.
pub fn run_with_native_cmp_rax_imm8_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &CmpRaxImm8InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeCmpRaxImm8(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact native `SUB RAX, sign-extended imm8`.
pub fn run_with_native_sub_rax_imm8<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &SubRaxImm8InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeSubRaxImm8(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact native `SUB RAX, sign-extended imm8`.
pub fn run_with_native_sub_rax_imm8_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &SubRaxImm8InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeSubRaxImm8(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Opt-in uncached dispatcher for exact bare `MOV RAX, sign-extended imm32`.
pub fn run_with_native_mov_rax_imm32<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &MovRaxImm32InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeMovRaxImm32(admission),
        RegisterFlagsMode::Eager,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Demand-flags candidate for exact bare `MOV RAX, sign-extended imm32`.
pub fn run_with_native_mov_rax_imm32_demand_flags_candidate<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: &MovRaxImm32InterpreterFallbackAdmission,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    run_register_dispatch(
        code,
        RegisterDispatchProfile::NativeMovRaxImm32(admission),
        RegisterFlagsMode::DemandFlagsCandidate,
        state,
        total_budget,
        control,
        UncachedRegisterPath(executor),
    )
}

/// Flag computation mode for the bounded register dispatcher.
// A profile identifies the admission and whether its extra form is interpreted
// or compiled natively. Flag policy is independent of that choice.
#[derive(Clone, Copy)]
pub enum RegisterFlagsMode {
    Eager,
    DemandFlagsCandidate,
}

#[derive(Clone, Copy)]
enum RegisterDispatchProfile<'a> {
    Nop(&'a InterpreterFallbackAdmission),
    InterpretedIncRax(&'a IncRaxInterpreterFallbackAdmission),
    NativeIncRax(&'a IncRaxInterpreterFallbackAdmission),
    NativeDecRax(&'a DecRaxInterpreterFallbackAdmission),
    NativeNegRax(&'a NegRaxInterpreterFallbackAdmission),
    NativeNotRax(&'a NotRaxInterpreterFallbackAdmission),
    NativeLeaRaxRbx(&'a LeaRaxRbxInterpreterFallbackAdmission),
    NativeLeaRaxRbxDisp8(&'a LeaRaxRbxDisp8InterpreterFallbackAdmission),
    NativeLeaRaxRbxRcx(&'a LeaRaxRbxRcxInterpreterFallbackAdmission),
    NativeLeaRaxRipDisp32(&'a LeaRaxRipDisp32InterpreterFallbackAdmission),
    NativeXchgRaxRbx(&'a XchgRaxRbxInterpreterFallbackAdmission),
    NativeAddRaxImm8(&'a AddRaxImm8InterpreterFallbackAdmission),
    NativeCmpRaxImm8(&'a CmpRaxImm8InterpreterFallbackAdmission),
    NativeSubRaxImm8(&'a SubRaxImm8InterpreterFallbackAdmission),
    NativeMovRaxImm32(&'a MovRaxImm32InterpreterFallbackAdmission),
    NativeRegisterV1(&'a NativeRegisterV1Admission),
}

impl<'a> RegisterDispatchProfile<'a> {
    fn into_strategy(
        self,
        flags: RegisterFlagsMode,
    ) -> (FallbackAdmission<'a>, NopFallbackCompilation) {
        match self {
            Self::Nop(admission) => (
                FallbackAdmission::Nop(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::Eager,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::DemandFlagsCandidate
                    }
                },
            ),
            Self::InterpretedIncRax(admission) => (
                FallbackAdmission::NopIncRax(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::Eager,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::DemandFlagsCandidate
                    }
                },
            ),
            Self::NativeIncRax(admission) => (
                FallbackAdmission::NopIncRax(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeIncRax,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeIncRaxDemandFlagsCandidate
                    }
                },
            ),
            Self::NativeDecRax(admission) => (
                FallbackAdmission::NopDecRax(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeDecRax,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeDecRaxDemandFlagsCandidate
                    }
                },
            ),
            Self::NativeNegRax(admission) => (
                FallbackAdmission::NopNegRax(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeNegRax,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeNegRaxDemandFlagsCandidate
                    }
                },
            ),
            Self::NativeNotRax(admission) => (
                FallbackAdmission::NopNotRax(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeNotRax,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeNotRaxDemandFlagsCandidate
                    }
                },
            ),
            Self::NativeLeaRaxRbx(admission) => (
                FallbackAdmission::NopLeaRaxRbx(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeLeaRaxRbx,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeLeaRaxRbxDemandFlagsCandidate
                    }
                },
            ),
            Self::NativeLeaRaxRbxDisp8(admission) => (
                FallbackAdmission::NopLeaRaxRbxDisp8(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeLeaRaxRbxDisp8,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeLeaRaxRbxDisp8DemandFlagsCandidate
                    }
                },
            ),
            Self::NativeLeaRaxRbxRcx(admission) => (
                FallbackAdmission::NopLeaRaxRbxRcx(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeLeaRaxRbxRcx,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeLeaRaxRbxRcxDemandFlagsCandidate
                    }
                },
            ),
            Self::NativeLeaRaxRipDisp32(admission) => (
                FallbackAdmission::NopLeaRaxRipDisp32(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeLeaRaxRipDisp32,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeLeaRaxRipDisp32DemandFlagsCandidate
                    }
                },
            ),
            Self::NativeXchgRaxRbx(admission) => (
                FallbackAdmission::NopXchgRaxRbx(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeXchgRaxRbx,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeXchgRaxRbxDemandFlagsCandidate
                    }
                },
            ),
            Self::NativeAddRaxImm8(admission) => (
                FallbackAdmission::NopAddRaxImm8(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeAddRaxImm8,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeAddRaxImm8DemandFlagsCandidate
                    }
                },
            ),
            Self::NativeCmpRaxImm8(admission) => (
                FallbackAdmission::NopCmpRaxImm8(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeCmpRaxImm8,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeCmpRaxImm8DemandFlagsCandidate
                    }
                },
            ),
            Self::NativeSubRaxImm8(admission) => (
                FallbackAdmission::NopSubRaxImm8(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeSubRaxImm8,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeSubRaxImm8DemandFlagsCandidate
                    }
                },
            ),
            Self::NativeMovRaxImm32(admission) => (
                FallbackAdmission::NopMovRaxImm32(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeMovRaxImm32,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeMovRaxImm32DemandFlagsCandidate
                    }
                },
            ),
            Self::NativeRegisterV1(admission) => (
                FallbackAdmission::NativeRegisterV1(admission),
                match flags {
                    RegisterFlagsMode::Eager => NopFallbackCompilation::NativeRegisterV1,
                    RegisterFlagsMode::DemandFlagsCandidate => {
                        NopFallbackCompilation::NativeRegisterV1DemandFlagsCandidate
                    }
                },
            ),
        }
    }
}

// The cache path has different dependencies and executor ownership from the
// uncached path. This private adapter keeps those contracts typed while both
// routes enter through the same profile/flags dispatcher.
trait RegisterDispatchPath<'admission> {
    type Error;
    type Profile;

    fn dispatch(
        self,
        code: CodeView<'_>,
        profile: Self::Profile,
        flags: RegisterFlagsMode,
        state: &mut LongModeState,
        total_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<FallbackDispatchResult, DispatchError<Self::Error>>;
}

struct UncachedRegisterPath<'a, E>(&'a mut E);

impl<'admission, E: ControlledBlockExecutor> RegisterDispatchPath<'admission>
    for UncachedRegisterPath<'_, E>
{
    type Error = E::Error;
    type Profile = RegisterDispatchProfile<'admission>;

    fn dispatch(
        self,
        code: CodeView<'_>,
        profile: Self::Profile,
        flags: RegisterFlagsMode,
        state: &mut LongModeState,
        total_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<FallbackDispatchResult, DispatchError<Self::Error>> {
        let (admission, compilation) = profile.into_strategy(flags);
        run_with_nop_fallback_strategy(
            code,
            admission,
            state,
            total_budget,
            control,
            self.0,
            compilation,
        )
    }
}

struct SingleEntryRegisterPath<'a, B: CachedBlockBackend> {
    execution_context: ExecutionContextSnapshot,
    dependencies: &'a [CodeDependencySpan],
    cache: &'a mut SingleEntryBlockCache<B::Published>,
    backend: &'a mut B,
}

impl<'admission, B: CachedBlockBackend> RegisterDispatchPath<'admission>
    for SingleEntryRegisterPath<'_, B>
{
    type Error = B::Error;
    type Profile = &'admission NativeRegisterV1Admission;

    fn dispatch(
        self,
        code: CodeView<'_>,
        admission: Self::Profile,
        flags: RegisterFlagsMode,
        state: &mut LongModeState,
        total_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<FallbackDispatchResult, DispatchError<Self::Error>> {
        let mut observer = NoopObserver;
        let strategy = match flags {
            RegisterFlagsMode::Eager => NativeRegisterV1CacheStrategy::Eager,
            RegisterFlagsMode::DemandFlagsCandidate => {
                NativeRegisterV1CacheStrategy::DemandFlagsCandidate
            }
        };
        run_cached_native_register_v1_strategy_controlled::<false, false, _, _>(
            code,
            admission,
            self.execution_context,
            self.dependencies,
            state,
            total_budget,
            control,
            self.cache,
            self.backend,
            &mut observer,
            strategy,
            None,
        )
    }
}

struct MultiEntryRegisterPath<'a, B: MultiBlockCachedBackend> {
    execution_context: ExecutionContextSnapshot,
    dependencies: &'a [CodeDependencySpan],
    cache: &'a mut MultiEntryBlockCache<B::Published>,
    backend: &'a mut B,
}

impl<'admission, B: MultiBlockCachedBackend> RegisterDispatchPath<'admission>
    for MultiEntryRegisterPath<'_, B>
{
    type Error = B::Error;
    type Profile = &'admission NativeRegisterV1Admission;

    fn dispatch(
        self,
        code: CodeView<'_>,
        admission: Self::Profile,
        flags: RegisterFlagsMode,
        state: &mut LongModeState,
        total_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<FallbackDispatchResult, DispatchError<Self::Error>> {
        let available = self.backend.published_block_capacity();
        if self.cache.max_entries() > available {
            return Err(DispatchError::MultiCacheConfig(
                MultiEntryCacheConfigError::BackendCapacityTooSmall {
                    requested: self.cache.max_entries(),
                    available,
                },
            ));
        }
        let strategy = match flags {
            RegisterFlagsMode::Eager => NativeRegisterV1CacheStrategy::Eager,
            RegisterFlagsMode::DemandFlagsCandidate => {
                NativeRegisterV1CacheStrategy::DemandFlagsCandidate
            }
        };
        let mut observer = NoopObserver;
        run_cached_native_register_v1_strategy_controlled::<false, false, _, _>(
            code,
            admission,
            self.execution_context,
            self.dependencies,
            state,
            total_budget,
            control,
            self.cache,
            self.backend,
            &mut observer,
            strategy,
            None,
        )
    }
}

fn run_register_dispatch<'admission, P: RegisterDispatchPath<'admission>>(
    code: CodeView<'_>,
    profile: P::Profile,
    flags: RegisterFlagsMode,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    path: P,
) -> Result<FallbackDispatchResult, DispatchError<P::Error>> {
    path.dispatch(code, profile, flags, state, total_budget, control)
}

#[derive(Clone, Copy)]
enum NopFallbackCompilation {
    Eager,
    DemandFlagsCandidate,
    NativeIncRax,
    NativeIncRaxDemandFlagsCandidate,
    NativeDecRax,
    NativeDecRaxDemandFlagsCandidate,
    NativeNegRax,
    NativeNegRaxDemandFlagsCandidate,
    NativeNotRax,
    NativeNotRaxDemandFlagsCandidate,
    NativeLeaRaxRbx,
    NativeLeaRaxRbxDemandFlagsCandidate,
    NativeLeaRaxRbxDisp8,
    NativeLeaRaxRbxDisp8DemandFlagsCandidate,
    NativeLeaRaxRbxRcx,
    NativeLeaRaxRbxRcxDemandFlagsCandidate,
    NativeLeaRaxRipDisp32,
    NativeLeaRaxRipDisp32DemandFlagsCandidate,
    NativeXchgRaxRbx,
    NativeXchgRaxRbxDemandFlagsCandidate,
    NativeAddRaxImm8,
    NativeAddRaxImm8DemandFlagsCandidate,
    NativeCmpRaxImm8,
    NativeCmpRaxImm8DemandFlagsCandidate,
    NativeSubRaxImm8,
    NativeSubRaxImm8DemandFlagsCandidate,
    NativeMovRaxImm32,
    NativeMovRaxImm32DemandFlagsCandidate,
    NativeRegisterV1,
    NativeRegisterV1DemandFlagsCandidate,
}

#[derive(Clone, Copy)]
enum FallbackAdmission<'a> {
    Nop(&'a InterpreterFallbackAdmission),
    NopIncRax(&'a IncRaxInterpreterFallbackAdmission),
    NopDecRax(&'a DecRaxInterpreterFallbackAdmission),
    NopNegRax(&'a NegRaxInterpreterFallbackAdmission),
    NopNotRax(&'a NotRaxInterpreterFallbackAdmission),
    NopLeaRaxRbx(&'a LeaRaxRbxInterpreterFallbackAdmission),
    NopLeaRaxRbxDisp8(&'a LeaRaxRbxDisp8InterpreterFallbackAdmission),
    NopLeaRaxRbxRcx(&'a LeaRaxRbxRcxInterpreterFallbackAdmission),
    NopLeaRaxRipDisp32(&'a LeaRaxRipDisp32InterpreterFallbackAdmission),
    NopXchgRaxRbx(&'a XchgRaxRbxInterpreterFallbackAdmission),
    NopAddRaxImm8(&'a AddRaxImm8InterpreterFallbackAdmission),
    NopCmpRaxImm8(&'a CmpRaxImm8InterpreterFallbackAdmission),
    NopSubRaxImm8(&'a SubRaxImm8InterpreterFallbackAdmission),
    NopMovRaxImm32(&'a MovRaxImm32InterpreterFallbackAdmission),
    NativeRegisterV1(&'a NativeRegisterV1Admission),
}

impl FallbackAdmission<'_> {
    fn matches_code(self, code: CodeView<'_>) -> bool {
        match self {
            Self::Nop(admission) => admission.matches_code(code),
            Self::NopIncRax(admission) => admission.matches_code(code),
            Self::NopDecRax(admission) => admission.matches_code(code),
            Self::NopNegRax(admission) => admission.matches_code(code),
            Self::NopNotRax(admission) => admission.matches_code(code),
            Self::NopLeaRaxRbx(admission) => admission.matches_code(code),
            Self::NopLeaRaxRbxDisp8(admission) => admission.matches_code(code),
            Self::NopLeaRaxRbxRcx(admission) => admission.matches_code(code),
            Self::NopLeaRaxRipDisp32(admission) => admission.matches_code(code),
            Self::NopXchgRaxRbx(admission) => admission.matches_code(code),
            Self::NopAddRaxImm8(admission) => admission.matches_code(code),
            Self::NopCmpRaxImm8(admission) => admission.matches_code(code),
            Self::NopSubRaxImm8(admission) => admission.matches_code(code),
            Self::NopMovRaxImm32(admission) => admission.matches_code(code),
            Self::NativeRegisterV1(admission) => admission.matches_code(code),
        }
    }
    fn contains(self, rip: u64) -> bool {
        match self {
            Self::Nop(admission) => admission.contains(rip),
            Self::NopIncRax(admission) => admission.contains(rip),
            Self::NopDecRax(admission) => admission.contains(rip),
            Self::NopNegRax(admission) => admission.contains(rip),
            Self::NopNotRax(admission) => admission.contains(rip),
            Self::NopLeaRaxRbx(admission) => admission.contains(rip),
            Self::NopLeaRaxRbxDisp8(admission) => admission.contains(rip),
            Self::NopLeaRaxRbxRcx(admission) => admission.contains(rip),
            Self::NopLeaRaxRipDisp32(admission) => admission.contains(rip),
            Self::NopXchgRaxRbx(admission) => admission.contains(rip),
            Self::NopAddRaxImm8(admission) => admission.contains(rip),
            Self::NopCmpRaxImm8(admission) => admission.contains(rip),
            Self::NopSubRaxImm8(admission) => admission.contains(rip),
            Self::NopMovRaxImm32(admission) => admission.contains(rip),
            Self::NativeRegisterV1(admission) => admission.contains(rip),
        }
    }
    fn admits_nop(self, rip: u64) -> bool {
        match self {
            Self::Nop(admission) => admission.admits_nop(rip),
            Self::NopIncRax(admission) => admission.admits_nop(rip),
            Self::NopDecRax(admission) => admission.admits_nop(rip),
            Self::NopNegRax(admission) => admission.admits_nop(rip),
            Self::NopNotRax(admission) => admission.admits_nop(rip),
            Self::NopLeaRaxRbx(admission) => admission.admits_nop(rip),
            Self::NopLeaRaxRbxDisp8(admission) => admission.admits_nop(rip),
            Self::NopLeaRaxRbxRcx(admission) => admission.admits_nop(rip),
            Self::NopLeaRaxRipDisp32(admission) => admission.admits_nop(rip),
            Self::NopXchgRaxRbx(admission) => admission.admits_nop(rip),
            Self::NopAddRaxImm8(admission) => admission.admits_nop(rip),
            Self::NopCmpRaxImm8(admission) => admission.admits_nop(rip),
            Self::NopSubRaxImm8(admission) => admission.admits_nop(rip),
            Self::NopMovRaxImm32(admission) => admission.admits_nop(rip),
            Self::NativeRegisterV1(admission) => admission.admits_nop(rip),
        }
    }
    fn admits_inc_rax(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::IncRax)
            ),
            Self::Nop(_)
            | Self::NopDecRax(_)
            | Self::NopNegRax(_)
            | Self::NopNotRax(_)
            | Self::NopLeaRaxRbx(_)
            | Self::NopLeaRaxRbxDisp8(_)
            | Self::NopLeaRaxRbxRcx(_)
            | Self::NopLeaRaxRipDisp32(_)
            | Self::NopXchgRaxRbx(_)
            | Self::NopAddRaxImm8(_)
            | Self::NopCmpRaxImm8(_)
            | Self::NopMovRaxImm32(_) => false,
            Self::NopSubRaxImm8(_) => false,
            Self::NopIncRax(admission) => admission.admits_inc_rax(rip),
        }
    }
    fn admits_dec_rax(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::DecRax)
            ),
            Self::Nop(_)
            | Self::NopIncRax(_)
            | Self::NopNegRax(_)
            | Self::NopNotRax(_)
            | Self::NopLeaRaxRbx(_)
            | Self::NopLeaRaxRbxDisp8(_)
            | Self::NopLeaRaxRbxRcx(_)
            | Self::NopLeaRaxRipDisp32(_)
            | Self::NopXchgRaxRbx(_)
            | Self::NopAddRaxImm8(_)
            | Self::NopCmpRaxImm8(_)
            | Self::NopMovRaxImm32(_) => false,
            Self::NopSubRaxImm8(_) => false,
            Self::NopDecRax(admission) => admission.admits_dec_rax(rip),
        }
    }
    fn admits_neg_rax(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::NegRax)
            ),
            Self::NopNegRax(admission) => admission.admits_neg_rax(rip),
            _ => false,
        }
    }
    fn admits_not_rax(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::NotRax)
            ),
            Self::NopNotRax(admission) => admission.admits_not_rax(rip),
            _ => false,
        }
    }
    fn admits_lea_rax_rbx(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::LeaRaxRbx)
            ),
            Self::NopLeaRaxRbx(admission) => admission.admits_lea_rax_rbx(rip),
            _ => false,
        }
    }
    fn admits_lea_rax_rbx_disp8(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::LeaRaxRbxDisp8(_))
            ),
            Self::NopLeaRaxRbxDisp8(admission) => admission.admits_lea_rax_rbx_disp8(rip),
            _ => false,
        }
    }
    fn admits_lea_rax_rbx_rcx(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::LeaRaxRbxRcx)
            ),
            Self::NopLeaRaxRbxRcx(admission) => admission.admits_lea_rax_rbx_rcx(rip),
            _ => false,
        }
    }
    fn admits_lea_rax_rip_disp32(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::LeaRaxRipDisp32(
                    _
                ))
            ),
            Self::NopLeaRaxRipDisp32(admission) => admission.admits_lea_rax_rip_disp32(rip),
            _ => false,
        }
    }
    fn admits_xchg_rax_rbx(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::XchgRaxRbx)
            ),
            Self::NopXchgRaxRbx(admission) => admission.admits_xchg_rax_rbx(rip),
            _ => false,
        }
    }
    fn admits_add_rax_imm8(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::AddRaxImm8(_))
            ),
            Self::NopAddRaxImm8(admission) => admission.admits_add_rax_imm8(rip),
            _ => false,
        }
    }
    fn admits_cmp_rax_imm8(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::CmpRaxImm8(_))
            ),
            Self::NopCmpRaxImm8(admission) => admission.admits_cmp_rax_imm8(rip),
            _ => false,
        }
    }
    fn admits_sub_rax_imm8(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::SubRaxImm8(_))
            ),
            Self::NopSubRaxImm8(admission) => admission.admits_sub_rax_imm8(rip),
            _ => false,
        }
    }
    fn admits_mov_rax_imm32(self, rip: u64) -> bool {
        match self {
            Self::NativeRegisterV1(admission) => matches!(
                admission.instruction(rip),
                Some(crate::block::NativeRegisterV1Instruction::MovRaxImm32(_))
            ),
            Self::NopMovRaxImm32(admission) => admission.admits_mov_rax_imm32(rip),
            _ => false,
        }
    }
}

fn run_with_nop_fallback_strategy<E: ControlledBlockExecutor>(
    code: CodeView<'_>,
    admission: FallbackAdmission<'_>,
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    executor: &mut E,
    compilation: NopFallbackCompilation,
) -> Result<FallbackDispatchResult, DispatchError<E::Error>> {
    if let Some(value) = state.cr3 {
        return Err(DispatchError::ExecutionContext(
            ExecutionContextError::UnsupportedCr3State { value },
        ));
    }
    if !admission.matches_code(code) {
        return Err(DispatchError::NopAdmissionDoesNotMatchCode);
    }

    let mut remaining = total_budget;
    let mut retired_native = 0u64;
    let mut retired_interpreter = 0u64;
    let mut blocks_executed = 0u64;

    loop {
        if let Some(exit) = control_exit(state.rip, code.code_end(), remaining, control) {
            return fallback_result(
                exit,
                total_budget,
                remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            );
        }
        if !admission.contains(state.rip) {
            return Err(DispatchError::EntryNotAdmitted { rip: state.rip });
        }
        if let Some(exit) = event_exit(control) {
            return fallback_result(
                exit,
                total_budget,
                remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            );
        }

        let compiled =
            match (admission, compilation) {
                (
                    FallbackAdmission::NativeRegisterV1(admission),
                    NopFallbackCompilation::NativeRegisterV1,
                ) => compile_block_for_native_register_v1(code, state.rip, admission),
                (
                    FallbackAdmission::NativeRegisterV1(admission),
                    NopFallbackCompilation::NativeRegisterV1DemandFlagsCandidate,
                ) => compile_block_for_native_register_v1_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::Nop(admission), NopFallbackCompilation::Eager) => {
                    compile_block_for_nop_fallback(code, state.rip, admission)
                }
                (
                    FallbackAdmission::Nop(admission),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => compile_block_for_nop_fallback_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopIncRax(admission), NopFallbackCompilation::Eager) => {
                    compile_block_for_nop_inc_rax_fallback(code, state.rip, admission)
                }
                (
                    FallbackAdmission::NopIncRax(admission),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => compile_block_for_nop_inc_rax_fallback_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopIncRax(admission), NopFallbackCompilation::NativeIncRax) => {
                    compile_block_for_native_inc_rax(code, state.rip, admission)
                }
                (
                    FallbackAdmission::NopIncRax(admission),
                    NopFallbackCompilation::NativeIncRaxDemandFlagsCandidate,
                ) => compile_block_for_native_inc_rax_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopDecRax(_), NopFallbackCompilation::Eager)
                | (FallbackAdmission::NopDecRax(_), NopFallbackCompilation::DemandFlagsCandidate) =>
                {
                    unreachable!("DEC admission is native-only")
                }
                (FallbackAdmission::NopDecRax(admission), NopFallbackCompilation::NativeDecRax) => {
                    compile_block_for_native_dec_rax(code, state.rip, admission)
                }
                (
                    FallbackAdmission::NopDecRax(admission),
                    NopFallbackCompilation::NativeDecRaxDemandFlagsCandidate,
                ) => compile_block_for_native_dec_rax_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopNegRax(_), NopFallbackCompilation::Eager)
                | (FallbackAdmission::NopNegRax(_), NopFallbackCompilation::DemandFlagsCandidate) =>
                {
                    unreachable!("NEG admission is native-only")
                }
                (FallbackAdmission::NopNegRax(admission), NopFallbackCompilation::NativeNegRax) => {
                    compile_block_for_native_neg_rax(code, state.rip, admission)
                }
                (
                    FallbackAdmission::NopNegRax(admission),
                    NopFallbackCompilation::NativeNegRaxDemandFlagsCandidate,
                ) => compile_block_for_native_neg_rax_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopNotRax(_), NopFallbackCompilation::Eager)
                | (
                    FallbackAdmission::NopNotRax(_),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("NOT admission is native-only"),
                (
                    FallbackAdmission::NopNotRax(admission),
                    NopFallbackCompilation::NativeNotRax,
                ) => compile_block_for_native_not_rax(code, state.rip, admission),
                (
                    FallbackAdmission::NopNotRax(admission),
                    NopFallbackCompilation::NativeNotRaxDemandFlagsCandidate,
                ) => compile_block_for_native_not_rax_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopLeaRaxRbx(_), NopFallbackCompilation::Eager)
                | (
                    FallbackAdmission::NopLeaRaxRbx(_),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("LEA admission is native-only"),
                (
                    FallbackAdmission::NopLeaRaxRbx(admission),
                    NopFallbackCompilation::NativeLeaRaxRbx,
                ) => compile_block_for_native_lea_rax_rbx(code, state.rip, admission),
                (
                    FallbackAdmission::NopLeaRaxRbx(admission),
                    NopFallbackCompilation::NativeLeaRaxRbxDemandFlagsCandidate,
                ) => compile_block_for_native_lea_rax_rbx_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopLeaRaxRbx(_), _) => {
                    unreachable!("native LEA RAX,RBX requires its dedicated compiler")
                }
                (
                    FallbackAdmission::NopLeaRaxRbxDisp8(_),
                    NopFallbackCompilation::Eager | NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("disp8 LEA admission is native-only"),
                (
                    FallbackAdmission::NopLeaRaxRbxDisp8(admission),
                    NopFallbackCompilation::NativeLeaRaxRbxDisp8,
                ) => compile_block_for_native_lea_rax_rbx_disp8(code, state.rip, admission),
                (
                    FallbackAdmission::NopLeaRaxRbxDisp8(admission),
                    NopFallbackCompilation::NativeLeaRaxRbxDisp8DemandFlagsCandidate,
                ) => compile_block_for_native_lea_rax_rbx_disp8_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopLeaRaxRbxDisp8(_), _) => {
                    unreachable!("disp8 LEA requires its dedicated compiler")
                }
                (
                    FallbackAdmission::NopLeaRaxRbxRcx(_),
                    NopFallbackCompilation::Eager | NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("SIB LEA admission is native-only"),
                (
                    FallbackAdmission::NopLeaRaxRbxRcx(admission),
                    NopFallbackCompilation::NativeLeaRaxRbxRcx,
                ) => compile_block_for_native_lea_rax_rbx_rcx(code, state.rip, admission),
                (
                    FallbackAdmission::NopLeaRaxRbxRcx(admission),
                    NopFallbackCompilation::NativeLeaRaxRbxRcxDemandFlagsCandidate,
                ) => compile_block_for_native_lea_rax_rbx_rcx_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopLeaRaxRbxRcx(_), _) => {
                    unreachable!("SIB LEA requires its dedicated compiler")
                }
                (
                    FallbackAdmission::NopLeaRaxRipDisp32(_),
                    NopFallbackCompilation::Eager | NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("RIP-relative LEA admission is native-only"),
                (FallbackAdmission::NopLeaRaxRipDisp32(admission), NopFallbackCompilation::NativeLeaRaxRipDisp32) =>
                    compile_block_for_native_lea_rax_rip_disp32(code, state.rip, admission),
                (FallbackAdmission::NopLeaRaxRipDisp32(admission), NopFallbackCompilation::NativeLeaRaxRipDisp32DemandFlagsCandidate) =>
                    compile_block_for_native_lea_rax_rip_disp32_demand_flags_candidate(code, state.rip, admission),
                (FallbackAdmission::NopLeaRaxRipDisp32(_), _) => unreachable!("RIP-relative LEA requires its dedicated compiler"),
                (FallbackAdmission::NopXchgRaxRbx(_), NopFallbackCompilation::Eager)
                | (
                    FallbackAdmission::NopXchgRaxRbx(_),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("XCHG admission is native-only"),
                (
                    FallbackAdmission::NopXchgRaxRbx(admission),
                    NopFallbackCompilation::NativeXchgRaxRbx,
                ) => compile_block_for_native_xchg_rax_rbx(code, state.rip, admission),
                (
                    FallbackAdmission::NopXchgRaxRbx(admission),
                    NopFallbackCompilation::NativeXchgRaxRbxDemandFlagsCandidate,
                ) => compile_block_for_native_xchg_rax_rbx_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopXchgRaxRbx(_), _) => {
                    unreachable!("native XCHG RAX,RBX requires its dedicated compiler")
                }
                (FallbackAdmission::NopAddRaxImm8(_), NopFallbackCompilation::Eager)
                | (
                    FallbackAdmission::NopAddRaxImm8(_),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("ADD-immediate admission is native-only"),
                (
                    FallbackAdmission::NopAddRaxImm8(admission),
                    NopFallbackCompilation::NativeAddRaxImm8,
                ) => compile_block_for_native_add_rax_imm8(code, state.rip, admission),
                (
                    FallbackAdmission::NopAddRaxImm8(admission),
                    NopFallbackCompilation::NativeAddRaxImm8DemandFlagsCandidate,
                ) => compile_block_for_native_add_rax_imm8_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopCmpRaxImm8(_), NopFallbackCompilation::Eager)
                | (
                    FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("CMP-immediate admission is native-only"),
                (
                    FallbackAdmission::NopCmpRaxImm8(admission),
                    NopFallbackCompilation::NativeCmpRaxImm8,
                ) => compile_block_for_native_cmp_rax_imm8(code, state.rip, admission),
                (
                    FallbackAdmission::NopCmpRaxImm8(admission),
                    NopFallbackCompilation::NativeCmpRaxImm8DemandFlagsCandidate,
                ) => compile_block_for_native_cmp_rax_imm8_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::Nop(_), NopFallbackCompilation::NativeIncRax)
                | (FallbackAdmission::NopNegRax(_), NopFallbackCompilation::NativeIncRax)
                | (FallbackAdmission::NopAddRaxImm8(_), NopFallbackCompilation::NativeIncRax)
                | (FallbackAdmission::NopCmpRaxImm8(_), NopFallbackCompilation::NativeIncRax)
                | (
                    FallbackAdmission::Nop(_),
                    NopFallbackCompilation::NativeIncRaxDemandFlagsCandidate,
                ) => unreachable!("native INC RAX requires its dedicated admission"),
                (
                    FallbackAdmission::NopNegRax(_),
                    NopFallbackCompilation::NativeIncRaxDemandFlagsCandidate,
                ) => unreachable!("native INC RAX requires its dedicated admission"),
                (
                    FallbackAdmission::NopAddRaxImm8(_),
                    NopFallbackCompilation::NativeIncRaxDemandFlagsCandidate,
                ) => unreachable!("native INC RAX requires its dedicated admission"),
                (
                    FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeIncRaxDemandFlagsCandidate,
                ) => unreachable!("native INC RAX requires its dedicated admission"),
                (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopNegRax(_)
                    | FallbackAdmission::NopAddRaxImm8(_)
                    | FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeDecRax,
                )
                | (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopNegRax(_)
                    | FallbackAdmission::NopAddRaxImm8(_)
                    | FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeDecRaxDemandFlagsCandidate,
                ) => unreachable!("native DEC RAX requires its dedicated admission"),
                (FallbackAdmission::NopDecRax(_), NopFallbackCompilation::NativeIncRax)
                | (
                    FallbackAdmission::NopDecRax(_),
                    NopFallbackCompilation::NativeIncRaxDemandFlagsCandidate,
                ) => unreachable!("native INC RAX requires its dedicated admission"),
                (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopDecRax(_)
                    | FallbackAdmission::NopAddRaxImm8(_)
                    | FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeNegRax,
                )
                | (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopDecRax(_)
                    | FallbackAdmission::NopAddRaxImm8(_)
                    | FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeNegRaxDemandFlagsCandidate,
                ) => unreachable!("native NEG RAX requires its dedicated admission"),
                (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopDecRax(_)
                    | FallbackAdmission::NopNegRax(_)
                    | FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeAddRaxImm8,
                )
                | (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopDecRax(_)
                    | FallbackAdmission::NopNegRax(_)
                    | FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeAddRaxImm8DemandFlagsCandidate,
                ) => unreachable!("native ADD immediate requires its dedicated admission"),
                (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopDecRax(_)
                    | FallbackAdmission::NopNegRax(_)
                    | FallbackAdmission::NopAddRaxImm8(_),
                    NopFallbackCompilation::NativeCmpRaxImm8,
                )
                | (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopDecRax(_)
                    | FallbackAdmission::NopNegRax(_)
                    | FallbackAdmission::NopAddRaxImm8(_),
                    NopFallbackCompilation::NativeCmpRaxImm8DemandFlagsCandidate,
                ) => unreachable!("native CMP immediate requires its dedicated admission"),
                (FallbackAdmission::NopSubRaxImm8(_), NopFallbackCompilation::Eager)
                | (
                    FallbackAdmission::NopSubRaxImm8(_),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("SUB-immediate admission is native-only"),
                (
                    FallbackAdmission::NopSubRaxImm8(admission),
                    NopFallbackCompilation::NativeSubRaxImm8,
                ) => compile_block_for_native_sub_rax_imm8(code, state.rip, admission),
                (
                    FallbackAdmission::NopSubRaxImm8(admission),
                    NopFallbackCompilation::NativeSubRaxImm8DemandFlagsCandidate,
                ) => compile_block_for_native_sub_rax_imm8_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (FallbackAdmission::NopMovRaxImm32(_), NopFallbackCompilation::Eager)
                | (
                    FallbackAdmission::NopMovRaxImm32(_),
                    NopFallbackCompilation::DemandFlagsCandidate,
                ) => unreachable!("MOV imm32 admission is native-only"),
                (
                    FallbackAdmission::NopMovRaxImm32(admission),
                    NopFallbackCompilation::NativeMovRaxImm32,
                ) => compile_block_for_native_mov_rax_imm32(code, state.rip, admission),
                (
                    FallbackAdmission::NopMovRaxImm32(admission),
                    NopFallbackCompilation::NativeMovRaxImm32DemandFlagsCandidate,
                ) => compile_block_for_native_mov_rax_imm32_demand_flags_candidate(
                    code, state.rip, admission,
                ),
                (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopDecRax(_)
                    | FallbackAdmission::NopNegRax(_)
                    | FallbackAdmission::NopAddRaxImm8(_)
                    | FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeSubRaxImm8
                    | NopFallbackCompilation::NativeSubRaxImm8DemandFlagsCandidate,
                ) => unreachable!("native SUB immediate requires its dedicated admission"),
                (FallbackAdmission::NopSubRaxImm8(_), _) => {
                    unreachable!("native SUB immediate admission requires its dedicated compiler")
                }
                (FallbackAdmission::NopNotRax(_), _) => {
                    unreachable!("native NOT RAX requires its dedicated admission")
                }
                (
                    FallbackAdmission::Nop(_)
                    | FallbackAdmission::NopIncRax(_)
                    | FallbackAdmission::NopDecRax(_)
                    | FallbackAdmission::NopNegRax(_)
                    | FallbackAdmission::NopAddRaxImm8(_)
                    | FallbackAdmission::NopCmpRaxImm8(_),
                    NopFallbackCompilation::NativeNotRax
                    | NopFallbackCompilation::NativeNotRaxDemandFlagsCandidate,
                ) => unreachable!("native NOT RAX requires its dedicated admission"),
                _ => unreachable!("fallback compilation strategy and admission must match"),
            }
            .map_err(DispatchError::Compile)?;
        let block = match compiled {
            BlockCompileOutcome::Compiled(block) => block,
            BlockCompileOutcome::NoNativePrefix { rip, .. } => {
                if matches!(admission, FallbackAdmission::NativeRegisterV1(_))
                    && !admission.admits_nop(rip)
                {
                    return Err(DispatchError::NoNativePrefix { rip });
                }
                if !(admission.admits_nop(rip)
                    || admission.admits_inc_rax(rip)
                    || admission.admits_dec_rax(rip)
                    || admission.admits_neg_rax(rip)
                    || admission.admits_not_rax(rip)
                    || admission.admits_lea_rax_rbx(rip)
                    || admission.admits_lea_rax_rbx_disp8(rip)
                    || admission.admits_lea_rax_rbx_rcx(rip)
                    || admission.admits_lea_rax_rip_disp32(rip)
                    || admission.admits_xchg_rax_rbx(rip)
                    || admission.admits_add_rax_imm8(rip)
                    || admission.admits_cmp_rax_imm8(rip)
                    || admission.admits_sub_rax_imm8(rip)
                    || admission.admits_mov_rax_imm32(rip))
                    || rip != state.rip
                {
                    return Err(DispatchError::NoNativePrefix { rip });
                }
                if let Some(exit) = step_admitted_fallback(
                    code,
                    admission,
                    state,
                    &mut remaining,
                    &mut retired_interpreter,
                    total_budget,
                    retired_native,
                    control,
                )? {
                    return fallback_result(
                        exit,
                        total_budget,
                        remaining,
                        retired_native,
                        retired_interpreter,
                        blocks_executed,
                    );
                }
                continue;
            }
        };

        let entry_state = state.clone();
        let report = match executor
            .execute_one_block_controlled_with_event(&block, state, remaining, control)
            .map_err(DispatchError::Executor)?
        {
            ControlledBlockExecution::Executed(report) => report,
            ControlledBlockExecution::Yield(exit) => {
                if *state != entry_state
                    || control_exit(state.rip, code.code_end(), remaining, control)
                        .or_else(|| event_exit(control))
                        != Some(exit)
                {
                    return Err(DispatchError::ZeroProgress { rip: state.rip });
                }
                return fallback_result(
                    exit,
                    total_budget,
                    remaining,
                    retired_native,
                    retired_interpreter,
                    blocks_executed,
                );
            }
        };
        validate_counters(
            report,
            remaining,
            u64::from(block.emitted().pc_map().instruction_count()),
        )?;
        remaining = report.remaining_budget;
        retired_native = retired_native
            .checked_add(report.retired_instructions)
            .ok_or(DispatchError::AggregateOverflow)?;
        blocks_executed = blocks_executed
            .checked_add(1)
            .ok_or(DispatchError::AggregateOverflow)?;
        validate_fallback_retirement_total(
            total_budget,
            remaining,
            retired_native,
            retired_interpreter,
        )?;

        match report.exit {
            RuntimeExitTag::NeedInterpreter => {
                let rip = state.rip;
                if let Some(exit) = control_exit(rip, code.code_end(), remaining, control) {
                    return fallback_result(
                        exit,
                        total_budget,
                        remaining,
                        retired_native,
                        retired_interpreter,
                        blocks_executed,
                    );
                }
                let fallback_admitted =
                    if matches!(admission, FallbackAdmission::NativeRegisterV1(_)) {
                        admission.admits_nop(rip)
                    } else {
                        admission.admits_nop(rip)
                            || admission.admits_inc_rax(rip)
                            || admission.admits_dec_rax(rip)
                            || admission.admits_neg_rax(rip)
                            || admission.admits_lea_rax_rbx(rip)
                            || admission.admits_lea_rax_rbx_disp8(rip)
                            || admission.admits_lea_rax_rbx_rcx(rip)
                            || admission.admits_lea_rax_rip_disp32(rip)
                            || admission.admits_add_rax_imm8(rip)
                    };
                if !fallback_admitted {
                    return Err(DispatchError::InterpreterInstructionNotAdmitted { rip });
                }
                if let Some(exit) = step_admitted_fallback(
                    code,
                    admission,
                    state,
                    &mut remaining,
                    &mut retired_interpreter,
                    total_budget,
                    retired_native,
                    control,
                )? {
                    return fallback_result(
                        exit,
                        total_budget,
                        remaining,
                        retired_native,
                        retired_interpreter,
                        blocks_executed,
                    );
                }
            }
            RuntimeExitTag::BlockComplete | RuntimeExitTag::DispatchCandidate => {
                if !admission.contains(state.rip) {
                    return Err(DispatchError::UnadmittedDispatchRip { rip: state.rip });
                }
                if let Some(exit) = control_exit(state.rip, code.code_end(), remaining, control) {
                    return fallback_result(
                        exit,
                        total_budget,
                        remaining,
                        retired_native,
                        retired_interpreter,
                        blocks_executed,
                    );
                }
                if report.retired_instructions == 0 {
                    return Err(DispatchError::ZeroProgress { rip: state.rip });
                }
                if let Some(exit) = event_exit(control) {
                    return fallback_result(
                        exit,
                        total_budget,
                        remaining,
                        retired_native,
                        retired_interpreter,
                        blocks_executed,
                    );
                }
            }
            RuntimeExitTag::BudgetExhausted => {
                if remaining != 0 {
                    return Err(DispatchError::InvalidBudgetExit { remaining });
                }
                let exit = control_exit(state.rip, code.code_end(), remaining, control)
                    .expect("zero remaining budget always produces a terminal control result");
                return fallback_result(
                    exit,
                    total_budget,
                    remaining,
                    retired_native,
                    retired_interpreter,
                    blocks_executed,
                );
            }
            exit => {
                return fallback_result(
                    DispatchExit::Runtime(exit),
                    total_budget,
                    remaining,
                    retired_native,
                    retired_interpreter,
                    blocks_executed,
                );
            }
        }
    }
}

fn control_exit(
    rip: u64,
    code_end: u64,
    remaining: u64,
    control: DispatchControl<'_>,
) -> Option<DispatchExit> {
    if control.stop_rip == Some(rip) {
        return Some(DispatchExit::Runtime(RuntimeExitTag::StopAddress));
    }
    if control.stop_requested.load(Ordering::Acquire) != 0 {
        return Some(DispatchExit::Runtime(RuntimeExitTag::StopRequested));
    }
    if rip == code_end {
        return Some(DispatchExit::CodeEnd);
    }
    if remaining == 0 {
        return Some(DispatchExit::BudgetExhausted);
    }
    None
}

fn event_exit(control: DispatchControl<'_>) -> Option<DispatchExit> {
    control
        .event_requested
        .filter(|request| request.load(Ordering::Acquire))
        .map(|_| DispatchExit::EventPending)
}

#[allow(clippy::too_many_arguments)]
fn step_admitted_fallback<E>(
    code: CodeView<'_>,
    admission: FallbackAdmission<'_>,
    state: &mut LongModeState,
    remaining: &mut u64,
    retired_interpreter: &mut u64,
    total_budget: u64,
    retired_native: u64,
    control: DispatchControl<'_>,
) -> Result<Option<DispatchExit>, DispatchError<E>> {
    let rip = state.rip;
    let is_nop = admission.admits_nop(rip);
    let is_inc_rax = admission.admits_inc_rax(rip);
    let is_dec_rax = admission.admits_dec_rax(rip);
    let is_neg_rax = admission.admits_neg_rax(rip);
    let is_not_rax = admission.admits_not_rax(rip);
    let is_lea_rax_rbx = admission.admits_lea_rax_rbx(rip);
    let is_lea_rax_rbx_disp8 = admission.admits_lea_rax_rbx_disp8(rip);
    let is_lea_rax_rbx_rcx = admission.admits_lea_rax_rbx_rcx(rip);
    let is_lea_rax_rip_disp32 = admission.admits_lea_rax_rip_disp32(rip);
    let is_add_rax_imm8 = admission.admits_add_rax_imm8(rip);
    let is_cmp_rax_imm8 = admission.admits_cmp_rax_imm8(rip);
    let is_mov_rax_imm32 = admission.admits_mov_rax_imm32(rip);
    if matches!(admission, FallbackAdmission::NativeRegisterV1(_)) && !is_nop {
        return Err(DispatchError::InterpreterInstructionNotAdmitted { rip });
    }
    if !admission.matches_code(code)
        || !(is_nop
            || is_inc_rax
            || is_dec_rax
            || is_neg_rax
            || is_not_rax
            || is_lea_rax_rbx
            || is_lea_rax_rbx_disp8
            || is_lea_rax_rbx_rcx
            || is_lea_rax_rip_disp32
            || is_add_rax_imm8
            || is_cmp_rax_imm8
            || is_mov_rax_imm32)
    {
        return Err(DispatchError::InterpreterInstructionNotAdmitted { rip });
    }
    if let Some(exit) = control_exit(rip, code.code_end(), *remaining, control) {
        return Ok(Some(exit));
    }
    if let Some(exit) = event_exit(control) {
        return Ok(Some(exit));
    }

    let length = if is_mov_rax_imm32 || is_lea_rax_rip_disp32 {
        7
    } else if is_add_rax_imm8 || is_cmp_rax_imm8 || is_lea_rax_rbx_disp8 || is_lea_rax_rbx_rcx {
        4
    } else if is_inc_rax || is_dec_rax || is_neg_rax || is_not_rax || is_lea_rax_rbx {
        3
    } else {
        1
    };
    let end_rip = rip
        .checked_add(length)
        .ok_or(DispatchError::InterpreterInstructionNotAdmitted { rip })?;
    let mut memory = CodeMemory::bounded_range(code, rip, end_rip).ok_or(
        DispatchError::InterpreterMemoryError {
            rip,
            error: MemoryError::Unmapped {
                address: rip,
                access: od_isa::memory::AccessKind::Fetch,
            },
        },
    )?;
    match od_isa::execute::step(state, &mut memory, AccessContext::default()) {
        Ok(StepOutcome::Advanced) => {
            let expected_rip = end_rip;
            if state.rip != expected_rip {
                return Err(DispatchError::InterpreterProgressMismatch {
                    expected_rip,
                    actual_rip: state.rip,
                });
            }
            *remaining = remaining
                .checked_sub(1)
                .ok_or(DispatchError::InterpreterInstructionNotAdmitted { rip })?;
            *retired_interpreter = retired_interpreter
                .checked_add(1)
                .ok_or(DispatchError::AggregateOverflow)?;
            validate_fallback_retirement_total(
                total_budget,
                *remaining,
                retired_native,
                *retired_interpreter,
            )?;
            Ok(None)
        }
        Ok(outcome) => Err(DispatchError::InterpreterStepFailure { rip, outcome }),
        Err(error) => Err(DispatchError::InterpreterMemoryError { rip, error }),
    }
}

fn validate_fallback_retirement_total<E>(
    total_budget: u64,
    remaining: u64,
    retired_native: u64,
    retired_interpreter: u64,
) -> Result<(), DispatchError<E>> {
    let actual = retired_native
        .checked_add(retired_interpreter)
        .ok_or(DispatchError::AggregateOverflow)?;
    let expected =
        total_budget
            .checked_sub(remaining)
            .ok_or(DispatchError::RetirementInvariant {
                expected: 0,
                actual,
            })?;
    if expected != actual {
        return Err(DispatchError::RetirementInvariant { expected, actual });
    }
    Ok(())
}

fn fallback_result<E>(
    exit: DispatchExit,
    total_budget: u64,
    remaining_budget: u64,
    retired_native: u64,
    retired_interpreter: u64,
    blocks_executed: u64,
) -> Result<FallbackDispatchResult, DispatchError<E>> {
    validate_fallback_retirement_total::<E>(
        total_budget,
        remaining_budget,
        retired_native,
        retired_interpreter,
    )?;
    Ok(FallbackDispatchResult {
        exit,
        remaining_budget,
        retired_native,
        retired_interpreter,
        blocks_executed,
    })
}

fn validate_counters<E>(
    report: BlockExecutionReport,
    incoming: u64,
    emitted_instructions: u64,
) -> Result<(), DispatchError<E>> {
    if report.retired_instructions > incoming {
        return Err(DispatchError::RetiredExceedsBudget {
            retired: report.retired_instructions,
            incoming,
        });
    }
    if report.retired_instructions > emitted_instructions {
        return Err(DispatchError::RetiredExceedsBlock {
            retired: report.retired_instructions,
            emitted: emitted_instructions,
        });
    }
    let expected = incoming - report.retired_instructions;
    if report.remaining_budget != expected {
        return Err(DispatchError::InconsistentRemaining {
            expected,
            actual: report.remaining_budget,
        });
    }
    Ok(())
}

fn result(
    exit: DispatchExit,
    remaining_budget: u64,
    retired_instructions: u64,
    blocks_executed: u64,
) -> DispatchResult {
    DispatchResult {
        exit,
        remaining_budget,
        retired_instructions,
        blocks_executed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translation::{
        AddressSpaceId, BackingId, BackingVersion, CodeIdentity, ExecutionContextId, MappingId,
        MappingVersion,
    };
    use std::cell::RefCell;
    use std::num::NonZeroU64;
    use std::rc::Rc;

    fn test_context() -> ExecutionContextSnapshot {
        test_context_with_generation(1)
    }

    fn test_context_with_generation(generation: u64) -> ExecutionContextSnapshot {
        ExecutionContextSnapshot::s0_09_fixture(
            ExecutionContextId::from_u64(generation).expect("test context ID is nonzero"),
        )
    }

    fn native_test_context_with_generation(generation: u64) -> ExecutionContextSnapshot {
        ExecutionContextSnapshot::native_register_v1_fixture(
            ExecutionContextId::from_u64(generation).expect("test context ID is nonzero"),
        )
    }

    #[test]
    fn nop_fallback_accepts_event_yield_after_publication() {
        struct PublicationEvent<'a>(&'a AtomicBool);
        impl ControlledBlockExecutor for PublicationEvent<'_> {
            type Error = ();
            fn execute_one_block_controlled(
                &mut self,
                _: &crate::block_compile::CompiledBlock,
                _: &mut LongModeState,
                _: u64,
                _: DispatchControl<'_>,
            ) -> Result<BlockExecutionReport, ()> {
                panic!("unexpected native entry")
            }
            fn execute_one_block_controlled_with_event(
                &mut self,
                _: &crate::block_compile::CompiledBlock,
                _: &mut LongModeState,
                _: u64,
                _: DispatchControl<'_>,
            ) -> Result<ControlledBlockExecution, ()> {
                self.0.store(true, Ordering::Release);
                Ok(ControlledBlockExecution::Yield(DispatchExit::EventPending))
            }
        }
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let code = CodeView::new(0x1000, &bytes).unwrap();
        let admission = InterpreterFallbackAdmission::from_code(code).unwrap();
        let mut state = LongModeState {
            rip: 0x1000,
            ..LongModeState::default()
        };
        let before = state.clone();
        let event = AtomicBool::new(false);
        let stop = AtomicU8::new(0);
        let result = run_with_nop_fallback(
            code,
            &admission,
            &mut state,
            4,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut PublicationEvent(&event),
        )
        .unwrap();
        assert_eq!(result.exit, DispatchExit::EventPending);
        assert_eq!(result.remaining_budget, 4);
        assert_eq!(result.retired_native, 0);
        assert_eq!(result.retired_interpreter, 0);
        assert_eq!(result.blocks_executed, 0);
        assert_eq!(state, before);
        assert!(event.load(Ordering::Acquire));
    }

    #[derive(Default)]
    struct ScriptedExecutor {
        reports: Vec<(BlockExecutionReport, Option<u64>)>,
        incoming_budgets: Vec<u64>,
        calls: usize,
    }

    impl BlockExecutor for ScriptedExecutor {
        type Error = ();

        fn execute_one_block(
            &mut self,
            _block: &crate::block_compile::CompiledBlock,
            state: &mut LongModeState,
            incoming_budget: u64,
        ) -> Result<BlockExecutionReport, Self::Error> {
            self.incoming_budgets.push(incoming_budget);
            self.calls += 1;
            let (report, next_rip) = self.reports.remove(0);
            if let Some(rip) = next_rip {
                state.rip = rip;
            }
            Ok(report)
        }
    }

    #[derive(Clone, Copy)]
    struct ControlledReply {
        report: BlockExecutionReport,
        next_rip: Option<u64>,
        rax: Option<u64>,
        rflags: Option<u64>,
        request_stop: bool,
        request_event: bool,
    }

    #[derive(Default)]
    struct ScriptedControlledExecutor {
        replies: Vec<ControlledReply>,
        incoming_budgets: Vec<u64>,
        control_addresses: Vec<(Option<u64>, usize)>,
        calls: usize,
    }

    #[derive(Default)]
    struct FailingControlledExecutor {
        calls: usize,
    }

    impl ControlledBlockExecutor for FailingControlledExecutor {
        type Error = ();

        fn execute_one_block_controlled(
            &mut self,
            _block: &crate::block_compile::CompiledBlock,
            _state: &mut LongModeState,
            _incoming_budget: u64,
            _control: DispatchControl<'_>,
        ) -> Result<BlockExecutionReport, Self::Error> {
            self.calls += 1;
            Err(())
        }
    }

    impl ControlledBlockExecutor for ScriptedControlledExecutor {
        type Error = ();

        fn execute_one_block_controlled(
            &mut self,
            _block: &crate::block_compile::CompiledBlock,
            state: &mut LongModeState,
            incoming_budget: u64,
            control: DispatchControl<'_>,
        ) -> Result<BlockExecutionReport, Self::Error> {
            self.calls += 1;
            self.incoming_budgets.push(incoming_budget);
            self.control_addresses.push((
                control.stop_rip,
                control.stop_requested as *const AtomicU8 as usize,
            ));
            let reply = self.replies.remove(0);
            if let Some(rip) = reply.next_rip {
                state.rip = rip;
            }
            if let Some(rax) = reply.rax {
                state.gpr[0] = rax;
            }
            if let Some(rflags) = reply.rflags {
                state.rflags = rflags;
            }
            if reply.request_stop {
                control.stop_requested.store(1, Ordering::Release);
            }
            if reply.request_event {
                control
                    .event_requested
                    .expect("scripted event request must be configured")
                    .store(true, Ordering::Release);
            }
            Ok(reply.report)
        }
    }

    fn admitted(code: &[u8], base: u64) -> (CodeView<'_>, AdmittedInstructionBoundaries) {
        let view = CodeView::new(base, code).unwrap();
        let decoded = od_isa::decode(code).unwrap();
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(view, &decoded).unwrap();
        (view, admission)
    }

    #[test]
    fn rip_disp32_fallback_decodes_the_full_seven_byte_instruction() {
        let base = 0x7100;
        let code = [0x48, 0x8d, 0x05, 0xf9, 0xff, 0xff, 0xff];
        let view = CodeView::new(base, &code).unwrap();
        let admission = LeaRaxRipDisp32InterpreterFallbackAdmission::from_code(view).unwrap();
        let mut executor = ScriptedControlledExecutor {
            replies: vec![ControlledReply {
                report: report(RuntimeExitTag::NeedInterpreter, 1, 0),
                next_rip: None,
                rax: None,
                rflags: None,
                request_stop: false,
                request_event: false,
            }],
            ..ScriptedControlledExecutor::default()
        };
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut state = LongModeState::default();
        state.rip = base;
        let outcome = run_with_native_lea_rax_rip_disp32(
            view,
            &admission,
            &mut state,
            1,
            control,
            &mut executor,
        )
        .unwrap();
        assert_eq!(state.gpr[0], base);
        assert_eq!(state.rip, base + 7);
        assert_eq!(outcome.retired_native, 0);
        assert_eq!(outcome.retired_interpreter, 1);
    }

    fn report(
        exit: RuntimeExitTag,
        remaining_budget: u64,
        retired_instructions: u64,
    ) -> BlockExecutionReport {
        BlockExecutionReport {
            exit,
            remaining_budget,
            retired_instructions,
        }
    }

    #[derive(Default)]
    struct PublishedTracker {
        publishes: usize,
        executions: usize,
        live_during_execute: Vec<usize>,
        published_ids: Vec<usize>,
        executed_ids: Vec<usize>,
        live: usize,
        max_live: usize,
        drops: usize,
        fail_publish: bool,
        fail_execute_once: bool,
        loop_to_entry: bool,
        request_stop_once: bool,
        request_event_once: bool,
        scripted_replies: Vec<(u64, RuntimeExitTag, u64)>,
        stop_on_publish: Option<Rc<AtomicU8>>,
        event_on_publish: Option<Rc<AtomicBool>>,
        allow_multiple_live: bool,
        capacity: usize,
    }

    struct MockPublished(Rc<RefCell<PublishedTracker>>, usize);

    impl Drop for MockPublished {
        fn drop(&mut self) {
            let mut tracker = self.0.borrow_mut();
            tracker.live -= 1;
            tracker.drops += 1;
        }
    }

    #[derive(Clone)]
    struct MockCachedBackend(Rc<RefCell<PublishedTracker>>);

    impl CachedBlockBackend for MockCachedBackend {
        type Error = &'static str;
        type Published = MockPublished;

        fn publish_cached_block(
            &mut self,
            _block: &crate::block_compile::CompiledBlock,
        ) -> Result<Self::Published, Self::Error> {
            let mut tracker = self.0.borrow_mut();
            if tracker.fail_publish {
                return Err("publish failed");
            }
            if !tracker.allow_multiple_live {
                assert_eq!(tracker.live, 0, "old mapping must drop before publish");
            }
            tracker.publishes += 1;
            let published_id = tracker.publishes;
            tracker.published_ids.push(published_id);
            tracker.live += 1;
            tracker.max_live = tracker.max_live.max(tracker.live);
            if let Some(stop) = tracker.stop_on_publish.take() {
                stop.store(1, Ordering::Release);
            }
            if let Some(event) = tracker.event_on_publish.take() {
                event.store(true, Ordering::Release);
            }
            drop(tracker);
            Ok(MockPublished(self.0.clone(), published_id))
        }

        fn execute_cached_block(
            &mut self,
            published: &Self::Published,
            block: &crate::block_compile::CompiledBlock,
            state: &mut LongModeState,
            incoming_budget: u64,
            control: DispatchControl<'_>,
        ) -> Result<BlockExecutionReport, Self::Error> {
            let (
                loop_to_entry,
                request_stop_once,
                request_event_once,
                fail_execute_once,
                scripted_reply,
            ) = {
                let mut tracker = self.0.borrow_mut();
                tracker.executions += 1;
                tracker.executed_ids.push(published.1);
                let live_during_execute = tracker.live;
                tracker.live_during_execute.push(live_during_execute);
                let request_stop = tracker.request_stop_once;
                tracker.request_stop_once = false;
                let request_event = tracker.request_event_once;
                tracker.request_event_once = false;
                let fail_execute = tracker.fail_execute_once;
                tracker.fail_execute_once = false;
                let scripted_reply = if tracker.scripted_replies.is_empty() {
                    None
                } else {
                    Some(tracker.scripted_replies.remove(0))
                };
                (
                    tracker.loop_to_entry,
                    request_stop,
                    request_event,
                    fail_execute,
                    scripted_reply,
                )
            };
            if request_stop_once {
                control.stop_requested.store(1, Ordering::Release);
            }
            if request_event_once {
                control
                    .event_requested
                    .expect("mock backend event request must be configured")
                    .store(true, Ordering::Release);
            }
            if fail_execute_once {
                return Err("execution failed");
            }
            if let Some((next_rip, exit, retired)) = scripted_reply {
                state.rip = next_rip;
                return Ok(report(exit, incoming_budget - retired, retired));
            }
            state.rip = if loop_to_entry {
                block.emitted().entry_rip()
            } else {
                block.code_end()
            };
            let retired = 1;
            Ok(report(
                if loop_to_entry {
                    RuntimeExitTag::DispatchCandidate
                } else {
                    RuntimeExitTag::BlockComplete
                },
                incoming_budget - retired,
                retired,
            ))
        }
    }

    impl MultiBlockCachedBackend for MockCachedBackend {
        fn published_block_capacity(&self) -> usize {
            let capacity = self.0.borrow().capacity;
            if capacity == 0 {
                MAX_MULTI_ENTRY_CACHE_BLOCKS
            } else {
                capacity
            }
        }
    }

    fn dependency(code: CodeView<'_>, version: u64) -> [CodeDependencySpan; 1] {
        let nz = |value| NonZeroU64::new(value).unwrap();
        [CodeDependencySpan {
            start_rip: code.base_rip(),
            end_rip: code.code_end(),
            identity: CodeIdentity {
                address_space: AddressSpaceId::new(nz(1)),
                mapping: MappingId::new(nz(2)),
                mapping_version: MappingVersion::new(nz(version)),
                backing: BackingId::new(nz(3)),
                backing_version: BackingVersion::new(nz(version)),
                executable: true,
            },
        }]
    }

    fn native_admission(code: CodeView<'_>) -> NativeRegisterV1Admission {
        NativeRegisterV1Admission::from_code(code).expect("fixture code is admitted")
    }

    fn s3_54_run(
        bytes: &[u8],
        base: u64,
        dependency_version: u64,
        generation: u64,
        flags: RegisterFlagsMode,
        cache: &mut MultiEntryBlockCache<MockPublished>,
        backend: &mut MockCachedBackend,
    ) -> Result<(FallbackDispatchResult, LongModeState), DispatchError<&'static str>> {
        let view = CodeView::new(base, bytes).unwrap();
        let admission = native_admission(view);
        let dependencies = dependency(view, dependency_version);
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        let stop = AtomicU8::new(0);
        let result = run_multi_cached_native_register_v1_controlled(
            view,
            &admission,
            native_test_context_with_generation(generation),
            &dependencies,
            &mut state,
            1,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: None,
            },
            cache,
            backend,
            flags,
        )?;
        Ok((result, state))
    }

    fn s3_54_backend(capacity: usize) -> (Rc<RefCell<PublishedTracker>>, MockCachedBackend) {
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            capacity,
            ..PublishedTracker::default()
        }));
        (tracker.clone(), MockCachedBackend(tracker))
    }

    #[test]
    fn s3_54_register_corpus_cold_warm_counters_and_flag_isolation() {
        let corpus: &[&[u8]] = &[
            &[0x48, 0xb8, 0x2a, 0, 0, 0, 0, 0, 0, 0],
            &[0x48, 0x03, 0xc0],
            &[0x48, 0xff, 0xc0],
            &[0x48, 0xff, 0xc8],
            &[0x48, 0xf7, 0xd8],
            &[0x48, 0xf7, 0xd0],
            &[0x48, 0x8d, 0x03],
            &[0x48, 0x8d, 0x43, 0x7f],
            &[0x48, 0x8d, 0x04, 0x0b],
            &[0x48, 0x8d, 0x05, 0, 0, 0, 0],
            &[0x48, 0x87, 0xd8],
            &[0x48, 0x83, 0xc0, 0x01],
            &[0x48, 0x83, 0xf8, 0x01],
            &[0x48, 0x83, 0xe8, 0x01],
            &[0x48, 0xc7, 0xc0, 1, 0, 0, 0],
        ];
        for flags in [
            RegisterFlagsMode::Eager,
            RegisterFlagsMode::DemandFlagsCandidate,
        ] {
            let (tracker, mut backend) = s3_54_backend(16);
            let mut cache = MultiEntryBlockCache::new(
                16,
                MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
                MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
            )
            .unwrap();
            for pass in 0..2 {
                for (index, bytes) in corpus.iter().enumerate() {
                    let base = 0x9000 + (index as u64) * 0x100;
                    let (result, state) =
                        s3_54_run(bytes, base, 1, 1, flags, &mut cache, &mut backend).unwrap();
                    assert_eq!(result.retired_native, 1);
                    assert_eq!(result.retired_interpreter, 0);
                    assert_eq!(result.blocks_executed, 1);
                    assert_eq!(state.rip, base + bytes.len() as u64);
                }
                let c = cache.counters();
                assert_eq!(
                    (
                        c.misses,
                        c.successful_compiles,
                        c.successful_publications,
                        c.evictions
                    ),
                    (15, 15, 15, 0)
                );
                assert_eq!(c.hits, if pass == 0 { 0 } else { 15 });
            }
            assert_eq!(tracker.borrow().publishes, 15);
            assert_eq!(tracker.borrow().executions, 30);
        }

        let (tracker, mut backend) = s3_54_backend(2);
        let mut cache = MultiEntryBlockCache::new(
            2,
            MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap();
        let code = [0x48, 0xff, 0xc0];
        for flags in [
            RegisterFlagsMode::Eager,
            RegisterFlagsMode::DemandFlagsCandidate,
            RegisterFlagsMode::Eager,
            RegisterFlagsMode::DemandFlagsCandidate,
        ] {
            s3_54_run(&code, 0xb000, 1, 1, flags, &mut cache, &mut backend).unwrap();
        }
        let c = cache.counters();
        assert_eq!(
            (
                c.misses,
                c.hits,
                c.successful_compiles,
                c.successful_publications
            ),
            (2, 2, 2, 2)
        );
        assert_eq!(tracker.borrow().executed_ids, [1, 2, 1, 2]);
    }

    #[test]
    fn s3_54_fifo_lru_hit_executes_the_matched_published_handle() {
        let code = [0x48, 0xff, 0xc0];
        for policy in [CacheEvictionPolicy::Fifo, CacheEvictionPolicy::Lru] {
            let (tracker, mut backend) = s3_54_backend(2);
            let mut cache = MultiEntryBlockCache::with_policy(
                2,
                MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
                MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
                policy,
            )
            .unwrap();
            for base in [0xc000, 0xc100, 0xc000, 0xc200] {
                s3_54_run(
                    &code,
                    base,
                    1,
                    1,
                    RegisterFlagsMode::Eager,
                    &mut cache,
                    &mut backend,
                )
                .unwrap();
            }
            assert_eq!(
                tracker.borrow().executed_ids,
                [1, 2, 1, 3],
                "LRU hit must not use the old index"
            );
            assert_eq!(
                (
                    cache.counters().hits,
                    cache.counters().misses,
                    cache.counters().evictions
                ),
                (1, 3, 1)
            );
            let (resident, evicted) = if policy == CacheEvictionPolicy::Fifo {
                (0xc100, 0xc000)
            } else {
                (0xc000, 0xc100)
            };
            s3_54_run(
                &code,
                resident,
                1,
                1,
                RegisterFlagsMode::Eager,
                &mut cache,
                &mut backend,
            )
            .unwrap();
            assert_eq!(cache.counters().hits, 2);
            s3_54_run(
                &code,
                evicted,
                1,
                1,
                RegisterFlagsMode::Eager,
                &mut cache,
                &mut backend,
            )
            .unwrap();
            assert_eq!(
                (cache.counters().misses, cache.counters().evictions),
                (4, 2)
            );
        }
    }

    #[test]
    fn s3_54_capacity_and_publication_failures_are_explicit() {
        let code = [0x48, 0xff, 0xc0];
        let (_, mut backend) = s3_54_backend(2);
        let mut probe = MultiEntryBlockCache::new(
            2,
            MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap();
        s3_54_run(
            &code,
            0xd000,
            1,
            1,
            RegisterFlagsMode::Eager,
            &mut probe,
            &mut backend,
        )
        .unwrap();
        let one_code = probe.counters().resident_code_bytes;
        let one_metadata = probe.counters().resident_logical_metadata_bytes;
        assert!(one_code > 1 && one_metadata > 1);
        drop(probe);

        for (entries, code_limit, metadata_limit) in [
            (
                1,
                MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
                MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
            ),
            (2, one_code, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES),
            (2, MAX_MULTI_ENTRY_CACHE_CODE_BYTES, one_metadata),
        ] {
            let (tracker, mut backend) = s3_54_backend(2);
            let mut cache = MultiEntryBlockCache::new(entries, code_limit, metadata_limit).unwrap();
            for base in [0xd100, 0xd200] {
                s3_54_run(
                    &code,
                    base,
                    1,
                    1,
                    RegisterFlagsMode::Eager,
                    &mut cache,
                    &mut backend,
                )
                .unwrap();
            }
            assert_eq!(cache.resident_entries(), 1);
            assert_eq!(cache.counters().evictions, 1);
            assert!(cache.counters().resident_code_bytes <= code_limit);
            assert!(cache.counters().resident_logical_metadata_bytes <= metadata_limit);
            assert_eq!(tracker.borrow().drops, 1);
        }

        let (tracker, mut backend) = s3_54_backend(1);
        let mut too_many = MultiEntryBlockCache::new(
            2,
            MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap();
        assert!(matches!(
            s3_54_run(
                &code,
                0xd300,
                1,
                1,
                RegisterFlagsMode::Eager,
                &mut too_many,
                &mut backend
            ),
            Err(DispatchError::MultiCacheConfig(
                MultiEntryCacheConfigError::BackendCapacityTooSmall {
                    requested: 2,
                    available: 1
                }
            ))
        ));
        assert_eq!(tracker.borrow().publishes, 0);

        let (tracker, mut backend) = s3_54_backend(1);
        let mut short_code =
            MultiEntryBlockCache::new(1, 1, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        assert!(matches!(
            s3_54_run(
                &code,
                0xd400,
                1,
                1,
                RegisterFlagsMode::Eager,
                &mut short_code,
                &mut backend
            ),
            Err(DispatchError::Cache(CacheError::EntryTooLarge { .. }))
        ));
        assert_eq!(tracker.borrow().publishes, 0);
        let mut short_metadata =
            MultiEntryBlockCache::new(1, MAX_MULTI_ENTRY_CACHE_CODE_BYTES, 1).unwrap();
        assert!(matches!(
            s3_54_run(
                &code,
                0xd400,
                1,
                1,
                RegisterFlagsMode::Eager,
                &mut short_metadata,
                &mut backend
            ),
            Err(DispatchError::Cache(
                CacheError::MinimumEntryTooLarge { .. } | CacheError::EntryTooLarge { .. }
            ))
        ));
        assert_eq!(tracker.borrow().publishes, 0);

        tracker.borrow_mut().fail_publish = true;
        let mut cache = MultiEntryBlockCache::new(
            1,
            MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap();
        assert!(matches!(
            s3_54_run(
                &code,
                0xd400,
                1,
                1,
                RegisterFlagsMode::Eager,
                &mut cache,
                &mut backend
            ),
            Err(DispatchError::CachePublish("publish failed"))
        ));
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(cache.counters().successful_publications, 0);
    }

    #[test]
    fn s3_54_dependency_invalidation_and_context_reset_prevent_stale_hits() {
        let code = [0x48, 0xff, 0xc0];
        let base = 0xe000;
        let view = CodeView::new(base, &code).unwrap();
        let old = dependency(view, 1);
        let current = dependency(view, 2);
        let (tracker, mut backend) = s3_54_backend(2);
        let mut cache = MultiEntryBlockCache::new(
            2,
            MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap();
        for version in [1, 1, 2] {
            s3_54_run(
                &code,
                base,
                version,
                1,
                RegisterFlagsMode::Eager,
                &mut cache,
                &mut backend,
            )
            .unwrap();
        }
        assert_eq!(
            (
                cache.counters().hits,
                cache.counters().misses,
                cache.resident_entries()
            ),
            (1, 2, 2)
        );
        assert_eq!(tracker.borrow().executed_ids, [1, 1, 2]);
        assert_eq!(
            cache.invalidate(CodeInvalidation::BackingContentChanged {
                backing: old[0].identity.backing,
                current_version: current[0].identity.backing_version,
            }),
            1
        );
        s3_54_run(
            &code,
            base,
            2,
            1,
            RegisterFlagsMode::Eager,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().executed_ids, [1, 1, 2, 2]);
        assert!(matches!(
            cache.reset_for_execution_context(native_test_context_with_generation(1)),
            Err(ExecutionContextError::GenerationNotAdvanced { .. })
        ));
        cache
            .reset_for_execution_context(native_test_context_with_generation(2))
            .unwrap();
        assert_eq!(cache.resident_entries(), 0);
        s3_54_run(
            &code,
            base,
            2,
            2,
            RegisterFlagsMode::Eager,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().executed_ids, [1, 1, 2, 2, 3]);
        assert_eq!(
            (
                cache.counters().hits,
                cache.counters().misses,
                cache.counters().successful_publications
            ),
            (2, 3, 3)
        );
        assert_eq!(tracker.borrow().drops, 2);
    }

    #[test]
    fn native_cached_demand_candidate_strategy_key_isolated() {
        let code = [0x48, 0x83, 0xC0, 0x01];
        let base = 0x7A40;
        let view = CodeView::new(base, &code).unwrap();
        let admission = native_admission(view);
        let dependencies = dependency(view, 1);
        let context = native_test_context_with_generation(1);
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = mock_cached_backend(tracker.clone());
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };

        #[cfg(feature = "phase-observation")]
        let mut expected_published_code_bytes = 0u64;
        for (candidate, expected) in [(false, 1), (true, 2), (true, 2), (false, 3)] {
            state.rip = base;
            #[cfg(feature = "phase-observation")]
            let publications_before = cache.counters().successful_publications;
            if candidate {
                run_cached_native_register_v1_demand_flags_candidate_controlled(
                    view,
                    &admission,
                    context,
                    &dependencies,
                    &mut state,
                    4,
                    control,
                    &mut cache,
                    &mut backend,
                )
                .unwrap();
                let entry = cache.entry(0).expect("candidate entry is resident");
                assert!(
                    entry.block.emitted().pc_map().ranges().iter().any(|range| {
                        range.snapshot
                            == crate::statepoints::SnapshotAdmissibility::InadmissiblePendingFlags
                    }),
                    "candidate compiler must retain pending-flags PC-map evidence"
                );
            } else {
                run_cached_native_register_v1_controlled(
                    view,
                    &admission,
                    context,
                    &dependencies,
                    &mut state,
                    4,
                    control,
                    &mut cache,
                    &mut backend,
                )
                .unwrap();
            }
            assert_eq!(cache.counters().successful_compiles, expected);
            assert_eq!(cache.counters().successful_publications, expected);
            #[cfg(feature = "phase-observation")]
            if cache.counters().successful_publications != publications_before {
                expected_published_code_bytes += cache.resident_code_bytes() as u64;
                assert_eq!(
                    cache.counters().published_code_bytes,
                    expected_published_code_bytes
                );
            }
        }
        assert_eq!((cache.counters().misses, cache.counters().hits), (3, 1));
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(tracker.borrow().publishes, 3);
    }

    #[test]
    fn native_cached_prepared_key_candidate_reuses_only_within_each_run() {
        let code = [0x48, 0x83, 0xC0, 0x01];
        let base = 0x7A60;
        let view = CodeView::new(base, &code).unwrap();
        let admission = native_admission(view);
        let dependencies = dependency(view, 1);
        let context = native_test_context_with_generation(1);
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = mock_cached_backend(tracker.clone());
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };

        run_cached_native_register_v1_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut state,
            1,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();

        tracker.borrow_mut().loop_to_entry = true;
        let initial = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        let mut baseline_state = initial.clone();
        let baseline = run_cached_native_register_v1_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut baseline_state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        let mut candidate_state = initial;
        let mut first_counts = PreparedKeyReuseCounts::default();
        let candidate = run_cached_native_register_v1_prepared_key_candidate_controlled_counted(
            view,
            &admission,
            context,
            &dependencies,
            &mut candidate_state,
            4,
            control,
            &mut cache,
            &mut backend,
            &mut first_counts,
        )
        .unwrap();
        assert_eq!(candidate_state, baseline_state);
        assert_eq!(candidate, baseline);
        assert_eq!(
            (first_counts.prepared_keys, first_counts.reused_keys),
            (1, 3)
        );
        assert_eq!(candidate.retired_native, 4);
        assert_eq!(candidate.remaining_budget, 0);

        let mut second_state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        let mut second_counts = PreparedKeyReuseCounts::default();
        run_cached_native_register_v1_prepared_key_candidate_controlled_counted(
            view,
            &admission,
            context,
            &dependencies,
            &mut second_state,
            4,
            control,
            &mut cache,
            &mut backend,
            &mut second_counts,
        )
        .unwrap();
        assert_eq!(
            (second_counts.prepared_keys, second_counts.reused_keys),
            (1, 3)
        );
        assert_eq!((cache.counters().misses, cache.counters().hits), (1, 12));

        tracker.borrow_mut().fail_execute_once = true;
        let mut error_state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        let mut error_counts = PreparedKeyReuseCounts::default();
        assert!(matches!(
            run_cached_native_register_v1_prepared_key_candidate_controlled_counted(
                view,
                &admission,
                context,
                &dependencies,
                &mut error_state,
                4,
                control,
                &mut cache,
                &mut backend,
                &mut error_counts,
            ),
            Err(DispatchError::CacheExecutor("execution failed"))
        ));
        assert_eq!(
            cache.resident_entries(),
            0,
            "executor error removes resident"
        );
        assert_eq!(
            (error_counts.prepared_keys, error_counts.reused_keys),
            (1, 0)
        );

        // A new call has no memo from the failed invocation. Warm its cache
        // first, then verify the candidate prepares once for this RUN.
        state.rip = base;
        run_cached_native_register_v1_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut state,
            1,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        let mut post_error_state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        let mut post_error_counts = PreparedKeyReuseCounts::default();
        run_cached_native_register_v1_prepared_key_candidate_controlled_counted(
            view,
            &admission,
            context,
            &dependencies,
            &mut post_error_state,
            4,
            control,
            &mut cache,
            &mut backend,
            &mut post_error_counts,
        )
        .unwrap();
        assert_eq!(
            (
                post_error_counts.prepared_keys,
                post_error_counts.reused_keys
            ),
            (1, 3)
        );
    }

    #[test]
    fn native_cached_prepared_key_candidate_rebuilds_after_need_interpreter_nop_and_new_rip() {
        let code = [
            0x48, 0x83, 0xC0, 0x01, // ADD RAX, 1
            0x90, // admitted NOP fallback
            0x48, 0x83, 0xC0, 0x01, // second native entry
        ];
        let base = 0x7A90;
        let view = CodeView::new(base, &code).unwrap();
        let admission = native_admission(view);
        let dependencies = dependency(view, 1);
        let context = native_test_context_with_generation(1);
        let script = || {
            vec![
                (base, RuntimeExitTag::DispatchCandidate, 1),
                (base + 4, RuntimeExitTag::NeedInterpreter, 0),
                (view.code_end(), RuntimeExitTag::BlockComplete, 1),
            ]
        };
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let initial = LongModeState {
            cr3: None,
            rip: base,
            rflags: 0x202,
            ..LongModeState::default()
        };

        let baseline_tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut baseline_backend = mock_cached_backend(baseline_tracker.clone());
        let mut baseline_cache =
            SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut baseline_warm_state = initial.clone();
        run_cached_native_register_v1_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut baseline_warm_state,
            1,
            control,
            &mut baseline_cache,
            &mut baseline_backend,
        )
        .unwrap();
        baseline_tracker.borrow_mut().scripted_replies = script();
        let mut baseline_state = initial.clone();
        let baseline = run_cached_native_register_v1_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut baseline_state,
            6,
            control,
            &mut baseline_cache,
            &mut baseline_backend,
        )
        .unwrap();

        let candidate_tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut candidate_backend = mock_cached_backend(candidate_tracker.clone());
        let mut candidate_cache =
            SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut candidate_warm_state = initial.clone();
        run_cached_native_register_v1_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut candidate_warm_state,
            1,
            control,
            &mut candidate_cache,
            &mut candidate_backend,
        )
        .unwrap();
        candidate_tracker.borrow_mut().scripted_replies = script();
        let mut candidate_state = initial;
        let mut counts = PreparedKeyReuseCounts::default();
        let candidate = run_cached_native_register_v1_prepared_key_candidate_controlled_counted(
            view,
            &admission,
            context,
            &dependencies,
            &mut candidate_state,
            6,
            control,
            &mut candidate_cache,
            &mut candidate_backend,
            &mut counts,
        )
        .unwrap();

        assert_eq!(candidate_state, baseline_state);
        assert_eq!(candidate, baseline);
        assert_eq!(candidate.exit, DispatchExit::CodeEnd);
        assert_eq!(candidate.retired_native, 2);
        assert_eq!(candidate.retired_interpreter, 1);
        assert_eq!(candidate.remaining_budget, 3);
        assert_eq!(candidate.blocks_executed, 3);
        assert_eq!((counts.prepared_keys, counts.reused_keys), (2, 1));
        assert_eq!(
            (
                candidate_cache.counters().misses,
                candidate_cache.counters().hits,
                candidate_cache.counters().successful_compiles,
                candidate_cache.counters().successful_publications,
                candidate_cache.counters().replacements,
            ),
            (2, 2, 2, 2, 1)
        );
        assert_eq!(
            (
                baseline_cache.counters().misses,
                baseline_cache.counters().hits,
                baseline_cache.counters().successful_compiles,
                baseline_cache.counters().successful_publications,
                baseline_cache.counters().replacements,
            ),
            (2, 2, 2, 2, 1)
        );
        assert_eq!(candidate_tracker.borrow().executions, 4);
    }

    #[test]
    fn native_cached_demand_candidate_guards_and_lifetime() {
        let code = [0x90, 0x48, 0x83, 0xC0, 0x01];
        let base = 0x7A80;
        let view = CodeView::new(base, &code).unwrap();
        let admission = native_admission(view);
        let dependencies = dependency(view, 1);
        let context = native_test_context_with_generation(1);
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = mock_cached_backend(tracker.clone());
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        stop.store(1, Ordering::Release);
        let stopped = run_cached_native_register_v1_demand_flags_candidate_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            stopped.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(cache.counters().misses, 0);
        stop.store(0, Ordering::Release);
        run_cached_native_register_v1_demand_flags_candidate_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(cache.counters().successful_compiles, 1);
        assert_eq!(cache.counters().successful_publications, 1);
        assert_eq!(cache.resident_entries(), 1);
        tracker.borrow_mut().fail_execute_once = true;
        state.rip = base;
        assert!(matches!(
            run_cached_native_register_v1_demand_flags_candidate_controlled(
                view,
                &admission,
                context,
                &dependencies,
                &mut state,
                4,
                control,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::CacheExecutor("execution failed"))
        ));
        assert_eq!(cache.resident_entries(), 0);
        tracker.borrow_mut().fail_publish = true;
        state.rip = base;
        assert!(matches!(
            run_cached_native_register_v1_demand_flags_candidate_controlled(
                view,
                &admission,
                context,
                &dependencies,
                &mut state,
                4,
                control,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::CachePublish("publish failed"))
        ));
        assert_eq!(cache.resident_entries(), 0);
        cache.clear();
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(tracker.borrow().live, 0);
    }

    fn mock_cached_backend(tracker: Rc<RefCell<PublishedTracker>>) -> MockCachedBackend {
        MockCachedBackend(tracker)
    }

    #[test]
    fn native_cached_profile_isolated_and_dependency_context_changes_miss() {
        let code = [0x48, 0xB8, 0x2A, 0, 0, 0, 0, 0, 0, 0];
        let base = 0x7A00;
        let view = CodeView::new(base, &code).unwrap();
        let admission = native_admission(view);
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = mock_cached_backend(tracker.clone());
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        let deps_v1 = dependency(view, 1);
        run_cached_native_register_v1_controlled(
            view,
            &admission,
            native_test_context_with_generation(1),
            &deps_v1,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        let first = cache.counters();
        assert_eq!((first.misses, first.hits), (1, 0));

        state.rip = base;
        run_cached_native_register_v1_controlled(
            view,
            &admission,
            native_test_context_with_generation(1),
            &deps_v1,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(cache.counters().hits, 1);

        state.rip = base;
        let deps_v2 = dependency(view, 2);
        run_cached_native_register_v1_controlled(
            view,
            &admission,
            native_test_context_with_generation(1),
            &deps_v2,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(cache.counters().misses, 2, "mapping version is in the key");
        assert_eq!(cache.counters().replacements, 1);

        let mut deps_new_mapping = deps_v2;
        deps_new_mapping[0].identity.mapping = MappingId::new(NonZeroU64::new(22).unwrap());
        state.rip = base;
        run_cached_native_register_v1_controlled(
            view,
            &admission,
            native_test_context_with_generation(1),
            &deps_new_mapping,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(cache.counters().misses, 3, "mapping identity is in the key");

        let changed_code = [0x48, 0xB8, 0x2B, 0, 0, 0, 0, 0, 0, 0];
        let changed_view = CodeView::new(base, &changed_code).unwrap();
        let changed_admission = native_admission(changed_view);
        let changed_deps = dependency(changed_view, 2);
        state.rip = base;
        run_cached_native_register_v1_controlled(
            changed_view,
            &changed_admission,
            native_test_context_with_generation(1),
            &changed_deps,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            cache.counters().misses,
            4,
            "source bytes are part of the key"
        );

        state.rip = base;
        run_cached_native_register_v1_controlled(
            changed_view,
            &changed_admission,
            native_test_context_with_generation(2),
            &changed_deps,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            cache.counters().misses,
            5,
            "context generation flushes the key"
        );

        let fixture_admission = AdmittedInstructionBoundaries::from_fixture_decode(
            changed_view,
            &od_isa::decode(&changed_code).unwrap(),
        )
        .unwrap();
        state.rip = base;
        run_cached_controlled(
            changed_view,
            &fixture_admission,
            test_context_with_generation(3),
            &changed_deps,
            &mut state,
            4,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(cache.counters().misses, 6);
        assert_eq!(
            cache.counters().hits,
            1,
            "S0-09 cannot hit NativeRegisterV1"
        );
        assert_eq!(cache.counters().successful_compiles, 6);
        assert_eq!(cache.counters().successful_publications, 6);
        assert_eq!(cache.resident_entries(), 1);
        assert!(cache.resident_code_bytes() <= cache.counters().resident_code_bytes_high_water);
        assert!(cache.resident_logical_bytes() <= MAX_SINGLE_ENTRY_CACHE_BYTES);
    }

    #[test]
    fn native_cached_guards_and_failures_preserve_publication_and_entry_counts() {
        let code = [0x48, 0x83, 0xc0, 0x01];
        let base = 0x7B00;
        let view = CodeView::new(base, &code).unwrap();
        let admission = native_admission(view);
        let deps = dependency(view, 1);

        let stop = AtomicU8::new(1);
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = mock_cached_backend(tracker.clone());
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        let result = run_cached_native_register_v1_controlled(
            view,
            &admission,
            native_test_context_with_generation(1),
            &deps,
            &mut state,
            4,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: None,
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            result.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(cache.counters().misses, 0, "pre-lookup STOP wins");
        assert_eq!(tracker.borrow().executions, 0);

        for event_after_publish in [false, true] {
            let stop = Rc::new(AtomicU8::new(0));
            let event = Rc::new(AtomicBool::new(false));
            let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
            if event_after_publish {
                tracker.borrow_mut().event_on_publish = Some(event.clone());
            } else {
                tracker.borrow_mut().stop_on_publish = Some(stop.clone());
            }
            let mut backend = mock_cached_backend(tracker.clone());
            let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
            let mut state = LongModeState {
                cr3: None,
                rip: base,
                ..LongModeState::default()
            };
            let result = run_cached_native_register_v1_controlled(
                view,
                &admission,
                native_test_context_with_generation(1),
                &deps,
                &mut state,
                4,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: Some(&event),
                },
                &mut cache,
                &mut backend,
            )
            .unwrap();
            assert_eq!(
                result.exit,
                if event_after_publish {
                    DispatchExit::EventPending
                } else {
                    DispatchExit::Runtime(RuntimeExitTag::StopRequested)
                }
            );
            assert_eq!(
                tracker.borrow().executions,
                0,
                "guard wins before native entry"
            );
            assert_eq!(
                cache.resident_entries(),
                1,
                "successful publication is retained"
            );
            assert_eq!(cache.counters().successful_publications, 1);
        }

        let tracker = Rc::new(RefCell::new(PublishedTracker {
            fail_publish: true,
            ..PublishedTracker::default()
        }));
        let mut backend = mock_cached_backend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        assert!(matches!(
            run_cached_native_register_v1_controlled(
                view,
                &admission,
                native_test_context_with_generation(1),
                &deps,
                &mut state,
                4,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &AtomicU8::new(0),
                    event_requested: None
                },
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::CachePublish("publish failed"))
        ));
        assert_eq!(cache.counters().successful_publications, 0);
        assert_eq!(cache.resident_entries(), 0);

        let tracker = Rc::new(RefCell::new(PublishedTracker {
            fail_execute_once: true,
            ..PublishedTracker::default()
        }));
        let mut backend = mock_cached_backend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        assert!(matches!(
            run_cached_native_register_v1_controlled(
                view,
                &admission,
                native_test_context_with_generation(1),
                &deps,
                &mut state,
                4,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &AtomicU8::new(0),
                    event_requested: None
                },
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::CacheExecutor("execution failed"))
        ));
        assert_eq!(cache.counters().successful_publications, 1);
        assert_eq!(
            cache.resident_entries(),
            0,
            "failed entry is safely released after return"
        );
        assert_eq!(tracker.borrow().live, 0);
        multi_entry_fixture_dispatch_rejects_native_context_after_binding();
    }

    fn multi_entry_fixture_dispatch_rejects_native_context_after_binding() {
        let mut cache = MultiEntryBlockCache::<MockPublished>::new(
            1,
            4096,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap();
        let native = native_test_context_with_generation(2);
        let code = [0x48, 0xB8, 0x2A, 0, 0, 0, 0, 0, 0, 0];
        let view = CodeView::new(0x7C00, &code).unwrap();
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(
            view,
            &od_isa::decode(&code).unwrap(),
        )
        .unwrap();
        let dependencies = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = mock_cached_backend(tracker);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        assert!(matches!(
            run_multi_cached(
                view,
                &admission,
                native,
                &dependencies,
                &mut state,
                1,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::ExecutionContext(
                ExecutionContextError::UnsupportedProfile(_)
            ))
        ));
        assert_eq!(cache.counters().misses, 0);
        assert_eq!(cache.counters().successful_compiles, 0);
        cache.reset_for_execution_context(native).unwrap();
        assert!(matches!(
            run_multi_cached(
                view,
                &admission,
                native,
                &dependencies,
                &mut state,
                1,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::ExecutionContext(
                ExecutionContextError::UnsupportedProfile(_)
            ))
        ));
        cache
            .reset_for_execution_context(test_context_with_generation(3))
            .unwrap();
        assert_eq!(cache.resident_entries(), 0);
    }

    struct FixtureCodeOwner {
        bytes: Vec<u8>,
        base: u64,
        address_space: AddressSpaceId,
        mapping: MappingId,
        mapping_version: MappingVersion,
        backing: BackingId,
        backing_version: BackingVersion,
        executable: bool,
        mapped: bool,
        fail_next_write: bool,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FixtureOwnerError {
        InjectedWriteFailure,
        OutOfRange,
        VersionExhausted,
    }

    impl FixtureCodeOwner {
        fn new(bytes: &[u8], base: u64, mapping: u64, backing: u64) -> Self {
            let nz = |value| NonZeroU64::new(value).unwrap();
            Self {
                bytes: bytes.to_vec(),
                base,
                address_space: AddressSpaceId::new(nz(1)),
                mapping: MappingId::new(nz(mapping)),
                mapping_version: MappingVersion::new(nz(1)),
                backing: BackingId::new(nz(backing)),
                backing_version: BackingVersion::new(nz(1)),
                executable: true,
                mapped: true,
                fail_next_write: false,
            }
        }

        fn snapshot(&self) -> Option<(CodeView<'_>, [CodeDependencySpan; 1])> {
            if !self.mapped {
                return None;
            }
            let view = CodeView::new(self.base, &self.bytes).ok()?;
            let dependency = [CodeDependencySpan {
                start_rip: view.base_rip(),
                end_rip: view.code_end(),
                identity: CodeIdentity {
                    address_space: self.address_space,
                    mapping: self.mapping,
                    mapping_version: self.mapping_version,
                    backing: self.backing,
                    backing_version: self.backing_version,
                    executable: self.executable,
                },
            }];
            Some((view, dependency))
        }

        fn commit_write<H>(
            &mut self,
            offset: usize,
            bytes: &[u8],
            cache: &mut MultiEntryBlockCache<H>,
        ) -> Result<(), FixtureOwnerError> {
            let end = offset
                .checked_add(bytes.len())
                .filter(|end| *end <= self.bytes.len())
                .ok_or(FixtureOwnerError::OutOfRange)?;
            let next_version = self
                .backing_version
                .checked_next()
                .ok_or(FixtureOwnerError::VersionExhausted)?;
            if std::mem::take(&mut self.fail_next_write) {
                return Err(FixtureOwnerError::InjectedWriteFailure);
            }
            self.bytes[offset..end].copy_from_slice(bytes);
            self.backing_version = next_version;
            cache.invalidate(CodeInvalidation::BackingContentChanged {
                backing: self.backing,
                current_version: next_version,
            });
            Ok(())
        }

        fn replace_mapping<H>(
            &mut self,
            new_mapping: MappingId,
            new_backing: BackingId,
            cache: &mut MultiEntryBlockCache<H>,
        ) -> Result<(), FixtureOwnerError> {
            let old_mapping = self.mapping;
            let next_version = self
                .mapping_version
                .checked_next()
                .ok_or(FixtureOwnerError::VersionExhausted)?;
            self.mapping = new_mapping;
            self.backing = new_backing;
            self.mapping_version = next_version;
            self.backing_version = BackingVersion::new(NonZeroU64::new(1).unwrap());
            self.mapped = true;
            cache.invalidate(CodeInvalidation::MappingReplaced {
                address_space: self.address_space,
                mapping: old_mapping,
                current_version: next_version,
            });
            Ok(())
        }

        fn remove_mapping<H>(&mut self, cache: &mut MultiEntryBlockCache<H>) {
            self.mapped = false;
            cache.invalidate(CodeInvalidation::MappingRemoved {
                address_space: self.address_space,
                mapping: self.mapping,
            });
        }

        fn set_executable<H>(&mut self, executable: bool, cache: &mut MultiEntryBlockCache<H>) {
            self.executable = executable;
            cache.invalidate(CodeInvalidation::ExecutePermissionChanged {
                address_space: self.address_space,
                mapping: self.mapping,
                executable,
            });
        }
    }

    fn run_owner_fixture(
        owner: &FixtureCodeOwner,
        cache: &mut MultiEntryBlockCache<MockPublished>,
        backend: &mut MockCachedBackend,
    ) -> Result<DispatchResult, DispatchError<&'static str>> {
        let (view, dependencies) = owner.snapshot().expect("fixture mapping exists");
        let (_, admission) = admitted(view.bytes(), view.base_rip());
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        run_multi_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            cache,
            backend,
        )
    }

    fn cached_fixture<'a>(
        bytes: &'a [u8],
        base: u64,
    ) -> (CodeView<'a>, AdmittedInstructionBoundaries) {
        admitted(bytes, base)
    }

    fn run_multi_fixture(
        bytes: &[u8],
        base: u64,
        cache: &mut MultiEntryBlockCache<MockPublished>,
        backend: &mut MockCachedBackend,
    ) -> Result<DispatchResult, DispatchError<&'static str>> {
        let (view, admission) = cached_fixture(bytes, base);
        let dependencies = dependency(view, 1);
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        run_multi_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            cache,
            backend,
        )
    }

    fn new_fixture_multi_cache() -> MultiEntryBlockCache<MockPublished> {
        MultiEntryBlockCache::new(
            4,
            MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap()
    }

    fn new_fixture_backend() -> (MockCachedBackend, Rc<RefCell<PublishedTracker>>) {
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            capacity: MAX_MULTI_ENTRY_CACHE_BLOCKS,
            ..PublishedTracker::default()
        }));
        (MockCachedBackend(tracker.clone()), tracker)
    }

    #[test]
    fn committed_fixture_write_invalidates_old_code_before_next_snapshot() {
        let initial = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let mut owner = FixtureCodeOwner::new(&initial, 0x9100, 2, 3);
        let mut cache = new_fixture_multi_cache();
        let (mut backend, tracker) = new_fixture_backend();
        run_owner_fixture(&owner, &mut cache, &mut backend).unwrap();
        let original_emitted = cache.entries[0].block.emitted().code().words().to_vec();
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(tracker.borrow().live, 1);

        owner
            .commit_write(2, &[2], &mut cache)
            .expect("fixture write commits and notifies cache");
        assert_eq!(owner.backing_version.get(), 2);
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(tracker.borrow().live, 0);
        assert_eq!(tracker.borrow().drops, 1);

        run_owner_fixture(&owner, &mut cache, &mut backend).unwrap();
        assert_eq!(cache.counters().successful_compiles, 2);
        assert_eq!(cache.counters().successful_publications, 2);
        assert_ne!(
            cache.entries[0].block.emitted().code().words(),
            original_emitted
        );
        assert_eq!(tracker.borrow().executed_ids, [1, 2]);
    }

    #[test]
    fn backing_write_notification_invalidates_aliases_but_not_unrelated_backings() {
        let code = [0x48, 0xb8, 7, 0, 0, 0, 0, 0, 0, 0];
        let mut owner_a = FixtureCodeOwner::new(&code, 0x9200, 2, 3);
        let owner_b = FixtureCodeOwner::new(&code, 0x9300, 4, 3);
        let mut cache = new_fixture_multi_cache();
        let (mut backend, tracker) = new_fixture_backend();
        run_owner_fixture(&owner_a, &mut cache, &mut backend).unwrap();
        run_owner_fixture(&owner_b, &mut cache, &mut backend).unwrap();
        assert_eq!(cache.resident_entries(), 2);
        assert_eq!(tracker.borrow().live, 2);

        owner_a
            .commit_write(2, &[8], &mut cache)
            .expect("backing change notifies every alias dependency");
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(tracker.borrow().live, 0);
        assert_eq!(tracker.borrow().drops, 2);

        run_owner_fixture(&owner_a, &mut cache, &mut backend).unwrap();
        let unrelated = CodeInvalidation::BackingContentChanged {
            backing: BackingId::new(NonZeroU64::new(99).unwrap()),
            current_version: BackingVersion::new(NonZeroU64::new(2).unwrap()),
        };
        assert_eq!(cache.invalidate(unrelated), 0);
        assert_eq!(cache.resident_entries(), 1);
        let publishes = tracker.borrow().publishes;
        run_owner_fixture(&owner_a, &mut cache, &mut backend).unwrap();
        assert_eq!(tracker.borrow().publishes, publishes);
        assert_eq!(cache.counters().hits, 1);
    }

    #[test]
    fn failed_fixture_write_preserves_bytes_version_and_cached_handle() {
        let code = [0x48, 0xb8, 9, 0, 0, 0, 0, 0, 0, 0];
        let mut owner = FixtureCodeOwner::new(&code, 0x9400, 2, 3);
        let mut cache = new_fixture_multi_cache();
        let (mut backend, tracker) = new_fixture_backend();
        run_owner_fixture(&owner, &mut cache, &mut backend).unwrap();
        let bytes_before = owner.bytes.clone();
        owner.fail_next_write = true;
        assert_eq!(
            owner.commit_write(2, &[10], &mut cache),
            Err(FixtureOwnerError::InjectedWriteFailure)
        );
        assert_eq!(owner.bytes, bytes_before);
        assert_eq!(owner.backing_version.get(), 1);
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(tracker.borrow().live, 1);
        let publications = tracker.borrow().publishes;
        run_owner_fixture(&owner, &mut cache, &mut backend).unwrap();
        assert_eq!(tracker.borrow().publishes, publications);
        assert_eq!(cache.counters().hits, 1);
    }

    #[test]
    fn remap_and_execute_revocation_drop_cached_entries_before_reentry() {
        let old_code = [0x48, 0xb8, 11, 0, 0, 0, 0, 0, 0, 0];
        let new_code = [0x48, 0xb8, 12, 0, 0, 0, 0, 0, 0, 0];
        let mut owner = FixtureCodeOwner::new(&old_code, 0x9500, 2, 3);
        let mut cache = new_fixture_multi_cache();
        let (mut backend, tracker) = new_fixture_backend();
        run_owner_fixture(&owner, &mut cache, &mut backend).unwrap();
        owner.bytes.copy_from_slice(&new_code);
        owner
            .replace_mapping(
                MappingId::new(NonZeroU64::new(5).unwrap()),
                BackingId::new(NonZeroU64::new(6).unwrap()),
                &mut cache,
            )
            .unwrap();
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(tracker.borrow().live, 0);
        run_owner_fixture(&owner, &mut cache, &mut backend).unwrap();
        assert_eq!(tracker.borrow().publishes, 2);
        assert_eq!(cache.counters().successful_compiles, 2);

        owner.set_executable(false, &mut cache);
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(tracker.borrow().live, 0);
        let executions = tracker.borrow().executions;
        assert!(matches!(
            run_owner_fixture(&owner, &mut cache, &mut backend),
            Err(DispatchError::TranslationKey(
                TranslationKeyError::NonExecutableDependency { .. }
            ))
        ));
        assert_eq!(tracker.borrow().executions, executions);
        assert_eq!(cache.resident_entries(), 0);
    }

    #[test]
    fn mapping_removal_notification_reclaims_multi_entry_resources() {
        let code = [0x48, 0xb8, 13, 0, 0, 0, 0, 0, 0, 0];
        let mut owner = FixtureCodeOwner::new(&code, 0x9600, 2, 3);
        let mut cache = new_fixture_multi_cache();
        let (mut backend, tracker) = new_fixture_backend();
        run_owner_fixture(&owner, &mut cache, &mut backend).unwrap();
        assert!(cache.counters().resident_code_bytes > 0);
        assert!(cache.counters().resident_logical_metadata_bytes > 0);
        owner.remove_mapping(&mut cache);
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(cache.counters().resident_code_bytes, 0);
        assert_eq!(cache.counters().resident_logical_metadata_bytes, 0);
        assert_eq!(tracker.borrow().live, 0);
        assert_eq!(tracker.borrow().drops, 1);
        assert!(owner.snapshot().is_none());
    }

    #[test]
    fn code_end_precedes_zero_budget_and_zero_budget_does_not_execute() {
        let code = [0x74, 0x00];
        let (view, admission) = admitted(&code, 0x1000);
        let mut state = LongModeState {
            cr3: None,
            rip: view.code_end(),
            ..LongModeState::default()
        };
        let before = state.clone();
        let mut executor = ScriptedExecutor::default();
        let result = run(view, &admission, &mut state, 0, &mut executor).unwrap();
        assert_eq!(result.exit, DispatchExit::CodeEnd);
        assert_eq!(result.remaining_budget, 0);
        assert_eq!(result.retired_instructions, 0);
        assert_eq!(executor.calls, 0);
        assert_eq!(state, before);

        state.rip = view.base_rip();
        let before = state.clone();
        let result = run(view, &admission, &mut state, 0, &mut executor).unwrap();
        assert_eq!(result.exit, DispatchExit::BudgetExhausted);
        assert_eq!(result.blocks_executed, 0);
        assert_eq!(executor.calls, 0);
        assert_eq!(state, before);
    }

    #[test]
    fn forwards_remaining_budget_and_dispatches_iteratively_to_code_end() {
        let code = [0x74, 0x00, 0x48, 0xB8, 7, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = admitted(&code, 0x2000);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let mut executor = ScriptedExecutor {
            reports: vec![
                (
                    report(RuntimeExitTag::DispatchCandidate, 4, 1),
                    Some(0x2002),
                ),
                (
                    report(RuntimeExitTag::BlockComplete, 3, 1),
                    Some(view.code_end()),
                ),
            ],
            ..ScriptedExecutor::default()
        };

        let result = run(view, &admission, &mut state, 5, &mut executor).unwrap();
        assert_eq!(result.exit, DispatchExit::CodeEnd);
        assert_eq!(result.remaining_budget, 3);
        assert_eq!(result.retired_instructions, 2);
        assert_eq!(result.blocks_executed, 2);
        assert_eq!(executor.incoming_budgets, [5, 4]);
        assert_eq!(state.rip, view.code_end());
    }

    #[test]
    fn rejects_invalid_counters_and_zero_progress_continuation() {
        let code = [0x74, 0x00];
        let (view, admission) = admitted(&code, 0x3000);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let mut inconsistent = ScriptedExecutor {
            reports: vec![(report(RuntimeExitTag::BlockComplete, 8, 0), None)],
            ..ScriptedExecutor::default()
        };
        assert_eq!(
            run(view, &admission, &mut state, 8, &mut inconsistent),
            Err(DispatchError::ZeroProgress {
                rip: view.base_rip()
            })
        );

        state.rip = view.base_rip();
        let mut bad_counts = ScriptedExecutor {
            reports: vec![(
                report(RuntimeExitTag::DispatchCandidate, 6, 1),
                Some(view.code_end()),
            )],
            ..ScriptedExecutor::default()
        };
        assert_eq!(
            run(view, &admission, &mut state, 8, &mut bad_counts),
            Err(DispatchError::InconsistentRemaining {
                expected: 7,
                actual: 6
            })
        );

        state.rip = view.base_rip();
        let mut over_budget = ScriptedExecutor {
            reports: vec![(
                report(RuntimeExitTag::DispatchCandidate, 0, 9),
                Some(view.code_end()),
            )],
            ..ScriptedExecutor::default()
        };
        assert_eq!(
            run(view, &admission, &mut state, 8, &mut over_budget),
            Err(DispatchError::RetiredExceedsBudget {
                retired: 9,
                incoming: 8
            })
        );

        state.rip = view.base_rip();
        let mut over_block = ScriptedExecutor {
            reports: vec![(
                report(RuntimeExitTag::DispatchCandidate, 6, 2),
                Some(view.code_end()),
            )],
            ..ScriptedExecutor::default()
        };
        assert_eq!(
            run(view, &admission, &mut state, 8, &mut over_block),
            Err(DispatchError::RetiredExceedsBlock {
                retired: 2,
                emitted: 1
            })
        );

        state.rip = view.base_rip();
        let mut invalid_budget_exit = ScriptedExecutor {
            reports: vec![(report(RuntimeExitTag::BudgetExhausted, 7, 1), None)],
            ..ScriptedExecutor::default()
        };
        assert_eq!(
            run(view, &admission, &mut state, 8, &mut invalid_budget_exit),
            Err(DispatchError::InvalidBudgetExit { remaining: 7 })
        );

        state.rip = view.base_rip();
        let mut unadmitted = ScriptedExecutor {
            reports: vec![(
                report(RuntimeExitTag::DispatchCandidate, 7, 1),
                Some(0xDEAD),
            )],
            ..ScriptedExecutor::default()
        };
        assert_eq!(
            run(view, &admission, &mut state, 8, &mut unadmitted),
            Err(DispatchError::UnadmittedDispatchRip { rip: 0xDEAD })
        );
    }

    #[test]
    fn returns_typed_runtime_exit_without_fallback() {
        let code = [0x74, 0x00];
        let (view, admission) = admitted(&code, 0x4000);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let mut executor = ScriptedExecutor {
            reports: vec![(report(RuntimeExitTag::Unsupported, 7, 1), None)],
            ..ScriptedExecutor::default()
        };
        let result = run(view, &admission, &mut state, 8, &mut executor).unwrap();
        assert_eq!(
            result.exit,
            DispatchExit::Runtime(RuntimeExitTag::Unsupported)
        );
        assert_eq!(executor.calls, 1);
    }

    fn nop_admission(code: &[u8], base: u64) -> (CodeView<'_>, InterpreterFallbackAdmission) {
        let view = CodeView::new(base, code).unwrap();
        let admission = InterpreterFallbackAdmission::from_code(view).unwrap();
        (view, admission)
    }

    #[test]
    fn nop_admission_is_s0_09_plus_only_exact_bare_90_and_valid_boundaries() {
        let code = [
            0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0,    // MOV RAX,1
            0x90, // exact bare NOP
            0x48, 0x03, 0xC1, // ADD RAX,RCX
            0x74, 0x00, // JZ code_end
        ];
        let (view, admission) = nop_admission(&code, 0x5100);
        assert!(admission.matches_code(view));
        assert!(admission.admits_nop(0x510A));
        assert!(admission.contains(view.code_end()));
        assert!(!admission.admits_nop(0x5100));

        let changed_code = [0x90];
        let changed_view = CodeView::new(view.base_rip(), &changed_code).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: changed_view.base_rip(),
            ..LongModeState::default()
        };
        let stop = AtomicU8::new(0);
        let mut executor = ScriptedControlledExecutor::default();
        assert_eq!(
            run_with_nop_fallback(
                changed_view,
                &admission,
                &mut state,
                1,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: None,
                },
                &mut executor,
            ),
            Err(DispatchError::NopAdmissionDoesNotMatchCode)
        );
        assert_eq!(executor.calls, 0);

        for denied in [
            &[0x66, 0x90][..], // prefixed NOP is not the exact admitted form
            &[0xF4][..],       // HLT is outside S0-09 plus NOP
            &[0x74, 0x01, 0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0][..], // target inside MOV
        ] {
            assert!(
                InterpreterFallbackAdmission::from_code(CodeView::new(0x5200, denied).unwrap())
                    .is_err()
            );
        }
    }

    #[test]
    fn all_nop_fallback_preserves_flags_and_counts_and_obeys_budget_guards() {
        let code = [0x90, 0x90];
        let (view, admission) = nop_admission(&code, 0x5300);
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            rflags: 0xAD0F,
            ..LongModeState::default()
        };
        let mut executor = ScriptedControlledExecutor::default();

        let budget_zero =
            run_with_nop_fallback(view, &admission, &mut state, 0, control, &mut executor).unwrap();
        assert_eq!(budget_zero.exit, DispatchExit::BudgetExhausted);
        assert_eq!(budget_zero.retired_interpreter, 0);
        assert_eq!(state.rip, view.base_rip());
        assert_eq!(executor.calls, 0);

        let one =
            run_with_nop_fallback(view, &admission, &mut state, 1, control, &mut executor).unwrap();
        assert_eq!(one.exit, DispatchExit::BudgetExhausted);
        assert_eq!(one.remaining_budget, 0);
        assert_eq!(one.retired_native, 0);
        assert_eq!(one.retired_interpreter, 1);
        assert_eq!(state.rip, view.base_rip() + 1);
        assert_eq!(state.rflags, 0xAD0F);
        assert_eq!(executor.calls, 0);

        state.rip = view.code_end();
        let end_before_budget =
            run_with_nop_fallback(view, &admission, &mut state, 0, control, &mut executor).unwrap();
        assert_eq!(end_before_budget.exit, DispatchExit::CodeEnd);
        assert_eq!(executor.calls, 0);

        state.rip = view.base_rip();
        let two =
            run_with_nop_fallback(view, &admission, &mut state, 2, control, &mut executor).unwrap();
        assert_eq!(two.exit, DispatchExit::CodeEnd);
        assert_eq!(two.remaining_budget, 0);
        assert_eq!(two.retired_native, 0);
        assert_eq!(two.retired_interpreter, 2);
        assert_eq!(state.rip, view.code_end());
        assert_eq!(state.rflags, 0xAD0F);
        assert_eq!(executor.calls, 0);
    }

    #[test]
    fn shared_control_stops_before_entry_or_before_interpreter_fallback() {
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&7_u64.to_le_bytes());
        code.push(0x90);
        let (view, admission) = nop_admission(&code, 0x5400);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let stop = AtomicU8::new(0);
        let mut executor = ScriptedControlledExecutor::default();

        let stopped_at_entry = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            5,
            DispatchControl {
                stop_rip: Some(view.base_rip()),
                stop_requested: &stop,
                event_requested: None,
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            stopped_at_entry.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopAddress)
        );
        assert_eq!(executor.calls, 0);

        stop.store(1, Ordering::Release);
        let stopped_before_compile = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            5,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: None,
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            stopped_before_compile.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(executor.calls, 0);
        stop.store(0, Ordering::Release);

        executor.replies.push(ControlledReply {
            report: report(RuntimeExitTag::NeedInterpreter, 4, 1),
            next_rip: Some(view.base_rip() + 10),
            rax: Some(7),
            rflags: None,
            request_stop: false,
            request_event: false,
        });
        let stopped_before_nop = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            5,
            DispatchControl {
                stop_rip: Some(view.base_rip() + 10),
                stop_requested: &stop,
                event_requested: None,
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            stopped_before_nop.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopAddress)
        );
        assert_eq!(stopped_before_nop.retired_native, 1);
        assert_eq!(stopped_before_nop.retired_interpreter, 0);
        assert_eq!(state.rip, view.base_rip() + 10);
        assert_eq!(executor.calls, 1);

        state.rip = view.base_rip();
        stop.store(0, Ordering::Release);
        executor.replies.push(ControlledReply {
            report: report(RuntimeExitTag::NeedInterpreter, 4, 1),
            next_rip: Some(view.base_rip() + 10),
            rax: Some(7),
            rflags: None,
            request_stop: true,
            request_event: false,
        });
        let stopped_by_shared_atomic = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            5,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: None,
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            stopped_by_shared_atomic.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(stopped_by_shared_atomic.retired_native, 1);
        assert_eq!(stopped_by_shared_atomic.retired_interpreter, 0);
        assert_eq!(stop.load(Ordering::Acquire), 1);
    }

    #[test]
    fn event_pending_before_entry_preserves_state_budget_cache_and_nop_fallback() {
        let code = [0x48, 0xB8, 7, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x5488);
        let dependencies = dependency(view, 1);
        let stop = AtomicU8::new(0);
        let event = AtomicBool::new(true);

        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            rflags: 0xA55,
            ..LongModeState::default()
        };
        state.gpr[3] = 0x1234;
        let before = state.clone();
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();

        let result = run_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            4,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(result.exit, DispatchExit::EventPending);
        assert_eq!(result.remaining_budget, 4);
        assert_eq!(result.retired_instructions, 0);
        assert_eq!(result.blocks_executed, 0);
        assert_eq!(state, before);
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(tracker.borrow().publishes, 0);
        assert_eq!(tracker.borrow().executions, 0);
        assert!(event.load(Ordering::Acquire));

        let mut multi_backend =
            MockCachedBackend(Rc::new(RefCell::new(PublishedTracker::default())));
        for capacity in [1, 2] {
            let mut multi_cache = MultiEntryBlockCache::new(
                capacity,
                MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
                MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
            )
            .unwrap();
            let before = state.clone();
            let result = run_multi_cached_controlled(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                3,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: Some(&event),
                },
                &mut multi_cache,
                &mut multi_backend,
            )
            .unwrap();
            assert_eq!(result.exit, DispatchExit::EventPending);
            assert_eq!(result.remaining_budget, 3);
            assert_eq!(result.retired_instructions, 0);
            assert_eq!(result.blocks_executed, 0);
            assert_eq!(state, before);
            assert_eq!(multi_cache.resident_entries(), 0);
        }
        assert_eq!(multi_backend.0.borrow().publishes, 0);
        assert_eq!(multi_backend.0.borrow().executions, 0);

        let (nop_view, nop_admission) = nop_admission(&[0x90], 0x5500);
        let mut nop_state = LongModeState {
            cr3: None,
            rip: nop_view.base_rip(),
            rflags: 0xAD0F,
            ..LongModeState::default()
        };
        let nop_before = nop_state.clone();
        let mut executor = ScriptedControlledExecutor::default();
        let result = run_with_nop_fallback(
            nop_view,
            &nop_admission,
            &mut nop_state,
            2,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(result.exit, DispatchExit::EventPending);
        assert_eq!(result.remaining_budget, 2);
        assert_eq!(result.retired_interpreter, 0);
        assert_eq!(result.retired_native, 0);
        assert_eq!(result.blocks_executed, 0);
        assert_eq!(nop_state, nop_before);
        assert_eq!(executor.calls, 0);
        assert!(event.load(Ordering::Acquire));
    }

    #[test]
    fn event_pending_after_native_cache_miss_and_hit_preserves_request_until_cleared() {
        let code = [0x48, 0xB8, 7, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x5580);
        let dependencies = dependency(view, 1);
        let stop = AtomicU8::new(0);
        let event = AtomicBool::new(false);
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            loop_to_entry: true,
            request_event_once: true,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };

        let first = run_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            5,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(first.exit, DispatchExit::EventPending);
        assert_eq!(first.remaining_budget, 4);
        assert_eq!(first.retired_instructions, 1);
        assert_eq!(first.blocks_executed, 1);
        assert_eq!(state.rip, view.base_rip());
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(tracker.borrow().publishes, 1);
        assert_eq!(tracker.borrow().executions, 1);
        assert!(event.load(Ordering::Acquire));

        let stopped_state = state.clone();
        let still_pending = run_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            4,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(still_pending.exit, DispatchExit::EventPending);
        assert_eq!(still_pending.remaining_budget, 4);
        assert_eq!(still_pending.retired_instructions, 0);
        assert_eq!(still_pending.blocks_executed, 0);
        assert_eq!(state, stopped_state);
        assert_eq!(tracker.borrow().executions, 1);

        event.store(false, Ordering::Release);
        tracker.borrow_mut().request_event_once = true;
        let hit = run_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            4,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(hit.exit, DispatchExit::EventPending);
        assert_eq!(hit.remaining_budget, 3);
        assert_eq!(hit.retired_instructions, 1);
        assert_eq!(hit.blocks_executed, 1);
        assert_eq!(cache.counters().hits, 1);
        assert_eq!(tracker.borrow().publishes, 1);
        assert_eq!(tracker.borrow().executions, 2);
        assert!(event.load(Ordering::Acquire));
    }

    #[test]
    fn event_set_during_publication_prevents_native_entry() {
        let code = [0x48, 0xB8, 3, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x55A0);
        let dependencies = dependency(view, 1);
        let stop = AtomicU8::new(0);
        let event = Rc::new(AtomicBool::new(false));
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            event_on_publish: Some(event.clone()),
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            rflags: 0xA55,
            ..LongModeState::default()
        };
        let before = state.clone();

        let result = run_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            4,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();

        assert_eq!(result.exit, DispatchExit::EventPending);
        assert_eq!(result.remaining_budget, 4);
        assert_eq!(result.retired_instructions, 0);
        assert_eq!(result.blocks_executed, 0);
        assert_eq!(state, before);
        assert!(event.load(Ordering::Acquire));
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(tracker.borrow().publishes, 1);
        assert_eq!(tracker.borrow().executions, 0);
    }

    #[test]
    fn cached_publication_boundary_rechecks_stop_priority_before_event() {
        let code = [0x48, 0xB8, 5, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x55B0);
        let dependencies = dependency(view, 1);

        for (raise_stop, raise_event, expected_exit) in [
            (false, true, DispatchExit::EventPending),
            (
                true,
                false,
                DispatchExit::Runtime(RuntimeExitTag::StopRequested),
            ),
            (
                true,
                true,
                DispatchExit::Runtime(RuntimeExitTag::StopRequested),
            ),
        ] {
            let stop = Rc::new(AtomicU8::new(0));
            let event = Rc::new(AtomicBool::new(false));
            let tracker = Rc::new(RefCell::new(PublishedTracker {
                stop_on_publish: raise_stop.then(|| stop.clone()),
                event_on_publish: raise_event.then(|| event.clone()),
                ..PublishedTracker::default()
            }));
            let mut backend = MockCachedBackend(tracker.clone());
            let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
            let mut state = LongModeState {
                cr3: None,
                rip: view.base_rip(),
                rflags: 0xA55,
                ..LongModeState::default()
            };
            let before = state.clone();

            let result = run_cached_controlled(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                4,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: Some(&event),
                },
                &mut cache,
                &mut backend,
            )
            .unwrap();

            assert_eq!(result.exit, expected_exit);
            assert_eq!(result.remaining_budget, 4);
            assert_eq!(result.retired_instructions, 0);
            assert_eq!(result.blocks_executed, 0);
            assert_eq!(state, before);
            assert_eq!(stop.load(Ordering::Acquire), u8::from(raise_stop));
            assert_eq!(event.load(Ordering::Acquire), raise_event);
            assert_eq!(cache.resident_entries(), 1);
            assert_eq!(tracker.borrow().publishes, 1);
            assert_eq!(tracker.borrow().executions, 0);
        }
    }

    #[test]
    fn event_pending_on_multi_cache_hit_and_miss_obeys_capacity() {
        let code = [0x48, 0xB8, 9, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x55C0);
        let dependencies = dependency(view, 1);
        let stop = AtomicU8::new(0);

        for capacity in [1, 2] {
            let event = AtomicBool::new(false);
            let tracker = Rc::new(RefCell::new(PublishedTracker {
                loop_to_entry: true,
                request_event_once: true,
                capacity: 16,
                ..PublishedTracker::default()
            }));
            let mut backend = MockCachedBackend(tracker.clone());
            let mut cache = MultiEntryBlockCache::new(
                capacity,
                MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
                MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
            )
            .unwrap();
            let mut state = LongModeState {
                cr3: None,
                rip: view.base_rip(),
                ..LongModeState::default()
            };

            let miss = run_multi_cached_controlled(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                4,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: Some(&event),
                },
                &mut cache,
                &mut backend,
            )
            .unwrap();
            assert_eq!(miss.exit, DispatchExit::EventPending);
            assert_eq!(miss.remaining_budget, 3);
            assert_eq!(miss.retired_instructions, 1);
            assert_eq!(miss.blocks_executed, 1);
            assert_eq!(tracker.borrow().publishes, 1);
            assert_eq!(tracker.borrow().executions, 1);
            assert!(event.load(Ordering::Acquire));

            event.store(false, Ordering::Release);
            tracker.borrow_mut().request_event_once = true;
            let hit = run_multi_cached_controlled(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                3,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: Some(&event),
                },
                &mut cache,
                &mut backend,
            )
            .unwrap();
            assert_eq!(hit.exit, DispatchExit::EventPending);
            assert_eq!(hit.remaining_budget, 2);
            assert_eq!(hit.retired_instructions, 1);
            assert_eq!(hit.blocks_executed, 1);
            assert_eq!(cache.counters().hits, 1);
            assert_eq!(tracker.borrow().publishes, 1);
            assert_eq!(tracker.borrow().executions, 2);
        }
    }

    #[test]
    fn event_after_jcc_yields_before_nop_and_resumes_without_replaying_prefix() {
        let mut code = vec![0x48, 0xB8]; // MOV RAX,7
        code.extend_from_slice(&7_u64.to_le_bytes());
        code.extend_from_slice(&[0x74, 0x00, 0x90, 0x48, 0xB8]);
        code.extend_from_slice(&9_u64.to_le_bytes()); // MOV RAX,9 after NOP
        let (view, admission) = nop_admission(&code, 0x5600);
        let initial = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            rflags: 0x242, // ZF is set, so the native JZ is taken.
            ..LongModeState::default()
        };
        let mut expected_after_prefix = initial.clone();
        let mut prefix_memory = CodeMemory::full_code(view);
        for _ in 0..2 {
            od_isa::execute::step(
                &mut expected_after_prefix,
                &mut prefix_memory,
                AccessContext::default(),
            )
            .unwrap();
        }
        let mut expected_complete = expected_after_prefix.clone();
        let mut suffix_memory = CodeMemory::full_code(view);
        for _ in 0..2 {
            od_isa::execute::step(
                &mut expected_complete,
                &mut suffix_memory,
                AccessContext::default(),
            )
            .unwrap();
        }
        assert_eq!(expected_after_prefix.rip, view.base_rip() + 12);
        assert_eq!(expected_complete.rip, view.code_end());
        assert_eq!(expected_complete.gpr[0], 9);

        let stop = AtomicU8::new(0);
        let event = AtomicBool::new(false);
        let mut state = initial;
        let mut executor = ScriptedControlledExecutor {
            replies: vec![ControlledReply {
                report: report(RuntimeExitTag::DispatchCandidate, 8, 2),
                next_rip: Some(view.base_rip() + 12),
                rax: Some(7),
                rflags: Some(expected_after_prefix.rflags),
                request_stop: false,
                request_event: true,
            }],
            ..ScriptedControlledExecutor::default()
        };

        let yielded = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            10,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(yielded.exit, DispatchExit::EventPending);
        assert_eq!(yielded.remaining_budget, 8);
        assert_eq!(yielded.retired_native, 2);
        assert_eq!(yielded.retired_interpreter, 0);
        assert_eq!(yielded.blocks_executed, 1);
        assert_eq!(state, expected_after_prefix);
        assert_eq!(executor.calls, 1);
        assert!(event.load(Ordering::Acquire));

        let held_state = state.clone();
        let held = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            8,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(held.exit, DispatchExit::EventPending);
        assert_eq!(held.retired_native + held.retired_interpreter, 0);
        assert_eq!(state, held_state);
        assert_eq!(executor.calls, 1);

        event.store(false, Ordering::Release);
        executor.replies.push(ControlledReply {
            report: report(RuntimeExitTag::BlockComplete, 6, 1),
            next_rip: Some(view.code_end()),
            rax: Some(9),
            rflags: None,
            request_stop: false,
            request_event: false,
        });
        let resumed = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            8,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: Some(&event),
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(resumed.exit, DispatchExit::CodeEnd);
        assert_eq!(resumed.remaining_budget, 6);
        assert_eq!(resumed.retired_native, 1);
        assert_eq!(resumed.retired_interpreter, 1);
        assert_eq!(resumed.blocks_executed, 1);
        assert_eq!(state, expected_complete);
        assert_eq!(executor.calls, 2);
        assert_eq!(executor.incoming_budgets, [10, 7]);
        assert!(!event.load(Ordering::Acquire));
    }

    #[test]
    fn event_request_obeys_stop_code_end_budget_and_error_priority() {
        let code = [0x48, 0xB8, 7, 0, 0, 0, 0, 0, 0, 0, 0x90];
        let (view, admission) = nop_admission(&code, 0x5680);
        let event = AtomicBool::new(true);
        let mut executor = ScriptedControlledExecutor::default();

        let stop_rip = AtomicU8::new(0);
        let mut at_entry = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let stopped_by_rip = run_with_nop_fallback(
            view,
            &admission,
            &mut at_entry,
            3,
            DispatchControl {
                stop_rip: Some(view.base_rip()),
                stop_requested: &stop_rip,
                event_requested: Some(&event),
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            stopped_by_rip.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopAddress)
        );

        let stop_requested = AtomicU8::new(1);
        let stopped_by_flag = run_with_nop_fallback(
            view,
            &admission,
            &mut at_entry,
            3,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop_requested,
                event_requested: Some(&event),
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(
            stopped_by_flag.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );

        let no_stop = AtomicU8::new(0);
        at_entry.rip = view.code_end();
        let at_end = run_with_nop_fallback(
            view,
            &admission,
            &mut at_entry,
            0,
            DispatchControl {
                stop_rip: None,
                stop_requested: &no_stop,
                event_requested: Some(&event),
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(at_end.exit, DispatchExit::CodeEnd);

        at_entry.rip = view.base_rip();
        let budget_first = run_with_nop_fallback(
            view,
            &admission,
            &mut at_entry,
            0,
            DispatchControl {
                stop_rip: None,
                stop_requested: &no_stop,
                event_requested: Some(&event),
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(budget_first.exit, DispatchExit::BudgetExhausted);
        assert_eq!(executor.calls, 0);

        event.store(false, Ordering::Release);
        let mut terminal_executor = ScriptedControlledExecutor {
            replies: vec![ControlledReply {
                report: report(RuntimeExitTag::StopRequested, 4, 1),
                next_rip: Some(view.base_rip() + 10),
                rax: Some(7),
                rflags: None,
                request_stop: false,
                request_event: true,
            }],
            ..ScriptedControlledExecutor::default()
        };
        let terminal = run_with_nop_fallback(
            view,
            &admission,
            &mut at_entry,
            5,
            DispatchControl {
                stop_rip: None,
                stop_requested: &no_stop,
                event_requested: Some(&event),
            },
            &mut terminal_executor,
        )
        .unwrap();
        assert_eq!(
            terminal.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(terminal.retired_native, 1);
        assert!(event.load(Ordering::Acquire));

        let mut malformed_executor = ScriptedControlledExecutor {
            replies: vec![ControlledReply {
                report: report(RuntimeExitTag::BudgetExhausted, 4, 1),
                next_rip: Some(view.base_rip() + 10),
                rax: Some(7),
                rflags: None,
                request_stop: false,
                request_event: true,
            }],
            ..ScriptedControlledExecutor::default()
        };
        event.store(false, Ordering::Release);
        at_entry.rip = view.base_rip();
        let malformed = run_with_nop_fallback(
            view,
            &admission,
            &mut at_entry,
            5,
            DispatchControl {
                stop_rip: None,
                stop_requested: &no_stop,
                event_requested: Some(&event),
            },
            &mut malformed_executor,
        );
        assert_eq!(
            malformed,
            Err(DispatchError::InvalidBudgetExit { remaining: 4 })
        );
        assert!(event.load(Ordering::Acquire));

        event.store(false, Ordering::Release);
        let failing_tracker = Rc::new(RefCell::new(PublishedTracker {
            request_event_once: true,
            fail_execute_once: true,
            ..PublishedTracker::default()
        }));
        let mut failing_backend = MockCachedBackend(failing_tracker);
        let (cached_view, cached_admission) = cached_fixture(&code[..10], 0x56C0);
        let dependencies = dependency(cached_view, 1);
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut cached_state = LongModeState {
            cr3: None,
            rip: cached_view.base_rip(),
            ..LongModeState::default()
        };
        let failure = run_cached_controlled(
            cached_view,
            &cached_admission,
            test_context(),
            &dependencies,
            &mut cached_state,
            3,
            DispatchControl {
                stop_rip: None,
                stop_requested: &no_stop,
                event_requested: Some(&event),
            },
            &mut cache,
            &mut failing_backend,
        );
        assert!(matches!(
            failure,
            Err(DispatchError::CacheExecutor("execution failed"))
        ));
    }

    #[test]
    fn executor_error_is_terminal_and_never_retries_or_falls_back() {
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&3_u64.to_le_bytes());
        code.push(0x90);
        let (view, admission) = nop_admission(&code, 0x5480);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let stop = AtomicU8::new(0);
        let mut executor = FailingControlledExecutor::default();
        assert_eq!(
            run_with_nop_fallback(
                view,
                &admission,
                &mut state,
                4,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: None,
                },
                &mut executor,
            ),
            Err(DispatchError::Executor(()))
        );
        assert_eq!(executor.calls, 1);
        assert_eq!(state.rip, view.base_rip());
        assert_eq!(state.gpr[0], 0);
    }

    #[test]
    fn native_prefix_nop_native_suffix_accounts_each_retirement_once() {
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&7_u64.to_le_bytes());
        code.extend_from_slice(&[0x48, 0x03, 0xC1, 0x90, 0x48, 0x3B, 0xC3, 0x74, 0x00]);
        let (view, admission) = nop_admission(&code, 0x5500);
        let stop = AtomicU8::new(0);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            rflags: 0x202,
            ..LongModeState::default()
        };
        state.gpr[1] = 5;
        state.gpr[3] = 12;
        let mut executor = ScriptedControlledExecutor {
            replies: vec![
                ControlledReply {
                    report: report(RuntimeExitTag::NeedInterpreter, 8, 2),
                    next_rip: Some(view.base_rip() + 13),
                    rax: Some(12),
                    rflags: Some(0x202),
                    request_stop: false,
                    request_event: false,
                },
                ControlledReply {
                    report: report(RuntimeExitTag::DispatchCandidate, 5, 2),
                    next_rip: Some(view.code_end()),
                    rax: None,
                    rflags: Some(0x242),
                    request_stop: false,
                    request_event: false,
                },
            ],
            ..ScriptedControlledExecutor::default()
        };

        let result = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            10,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: None,
            },
            &mut executor,
        )
        .unwrap();
        assert_eq!(result.exit, DispatchExit::CodeEnd);
        assert_eq!(result.retired_native, 4);
        assert_eq!(result.retired_interpreter, 1);
        assert_eq!(result.remaining_budget, 5);
        assert_eq!(result.blocks_executed, 2);
        assert_eq!(executor.incoming_budgets, [10, 7]);
        assert_eq!(
            executor.control_addresses[0],
            (None, &stop as *const AtomicU8 as usize)
        );
        assert_eq!(
            executor.control_addresses[1],
            (None, &stop as *const AtomicU8 as usize)
        );
        assert_eq!(state.rip, view.code_end());
        assert_eq!(state.gpr[0], 12);
        assert_eq!(state.rflags, 0x242);
    }

    #[test]
    fn one_entry_cache_reuses_same_key_and_self_loop_consumes_budget() {
        let code = [0x75, 0xFE]; // JNZ to this instruction.
        let (view, admission) = cached_fixture(&code, 0x6000);
        let dependencies = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            loop_to_entry: true,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };

        let result = run_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            3,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(result.exit, DispatchExit::BudgetExhausted);
        assert_eq!(result.retired_instructions, 3);
        assert_eq!(result.remaining_budget, 0);
        assert_eq!(tracker.borrow().publishes, 1);
        assert_eq!(cache.resident_entries(), 1);

        state.rip = view.base_rip();
        let second = run_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(second.retired_instructions, 1);
        assert_eq!(
            tracker.borrow().publishes,
            1,
            "cache hit must not republish"
        );
        let built = build_dispatch_block(view, view.base_rip(), &admission).unwrap();
        let source = match &built {
            crate::block::BuildOutcome::Block(block) => block,
            _ => panic!("self-loop fixture should produce a block"),
        };
        let deps = dependency(view, 1);
        let key = TranslationKey::for_block(
            view,
            source,
            CompileFlavor::S0_09FixtureDispatch,
            test_context(),
            &deps,
        )
        .unwrap();
        let compiled = match crate::block_compile::compile_prepared_dispatch_block(
            view.code_end(),
            built,
            &admission,
        )
        .unwrap()
        {
            BlockCompileOutcome::Compiled(block) => block,
            _ => panic!("self-loop fixture should compile"),
        };
        let expected_legacy_bytes = key
            .logical_storage_bytes()
            .unwrap()
            .checked_add(compiled.logical_storage_bytes().unwrap())
            .and_then(|size| size.checked_add(compiled.emitted().logical_storage_bytes().unwrap()))
            .and_then(|size| size.checked_add(std::mem::size_of::<CachedEntry<MockPublished>>()))
            .unwrap();
        assert_eq!(cache.resident_logical_bytes(), expected_legacy_bytes);

        let mut exact_limit_cache = SingleEntryBlockCache::new(1, expected_legacy_bytes).unwrap();
        let exact_tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut exact_backend = MockCachedBackend(exact_tracker.clone());
        let mut exact_state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        run_cached(
            view,
            &admission,
            test_context(),
            &deps,
            &mut exact_state,
            1,
            &mut exact_limit_cache,
            &mut exact_backend,
        )
        .unwrap();
        assert_eq!(
            exact_limit_cache.resident_logical_bytes(),
            expected_legacy_bytes
        );

        let mut short_limit_cache =
            SingleEntryBlockCache::new(1, expected_legacy_bytes - 1).unwrap();
        let short_tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut short_backend = MockCachedBackend(short_tracker.clone());
        let mut short_state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        assert!(matches!(
            run_cached(view, &admission,
            test_context(), &deps, &mut short_state, 1, &mut short_limit_cache, &mut short_backend),
            Err(DispatchError::Cache(CacheError::EntryTooLarge { actual, max }))
                if actual == expected_legacy_bytes && max == expected_legacy_bytes - 1
        ));
        assert_eq!(short_tracker.borrow().publishes, 0);
    }

    #[test]
    fn execution_context_generation_separates_same_rip_cache_entries() {
        let code = [0x48, 0xB8, 0x2A, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x7B40);
        let dependencies = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let first_stop = AtomicU8::new(0);

        run_cached_controlled(
            view,
            &admission,
            test_context_with_generation(10),
            &dependencies,
            &mut state,
            3,
            DispatchControl {
                stop_rip: None,
                stop_requested: &first_stop,
                event_requested: None,
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().executed_ids, [1]);

        // Architectural state, budget, STOP-RIP and STOP byte are invocation
        // inputs and must not create another translation.
        state.rip = view.base_rip();
        state.gpr[3] = 0xCAFE;
        state.rflags = 0xA55;
        let second_stop = AtomicU8::new(0);
        run_cached_controlled(
            view,
            &admission,
            test_context_with_generation(10),
            &dependencies,
            &mut state,
            1,
            DispatchControl {
                stop_rip: Some(view.base_rip() + 1),
                stop_requested: &second_stop,
                event_requested: None,
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(cache.counters().hits, 1);
        assert_eq!(tracker.borrow().publishes, 1);
        assert_eq!(tracker.borrow().executed_ids, [1, 1]);

        // Same RIP/code/dependencies with a fresh context generation flushes
        // and drops the old handle before publication; the old handle is never
        // used for this invocation.
        state.rip = view.base_rip();
        run_cached(
            view,
            &admission,
            test_context_with_generation(11),
            &dependencies,
            &mut state,
            3,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        let tracked = tracker.borrow();
        assert_eq!(tracked.published_ids, [1, 2]);
        assert_eq!(tracked.executed_ids, [1, 1, 2]);
        assert_eq!(tracked.drops, 1);
        assert_eq!(tracked.live, 1);
        drop(tracked);

        assert!(matches!(
            cache.reset_for_execution_context(test_context_with_generation(11)),
            Err(ExecutionContextError::GenerationNotAdvanced { .. })
        ));
        assert_eq!(cache.resident_entries(), 1);
        cache
            .reset_for_execution_context(test_context_with_generation(12))
            .unwrap();
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(tracker.borrow().live, 0);

        state.rip = view.base_rip();
        run_cached(
            view,
            &admission,
            test_context_with_generation(12),
            &dependencies,
            &mut state,
            3,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        let tracked = tracker.borrow();
        assert_eq!(tracked.published_ids, [1, 2, 3]);
        assert_eq!(tracked.executed_ids, [1, 1, 2, 3]);
        assert_eq!(tracked.drops, 2);
        assert_eq!(tracked.live, 1);
    }

    #[test]
    fn unsupported_execution_profiles_fail_before_cache_or_backend_work() {
        let code = [0x48, 0xB8, 0x2A, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x7B60);
        let dependencies = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let unsupported_profiles = [
            ExecutionProfile {
                mode: crate::translation::GuestExecutionMode::Compatibility,
                canonical_address_bits: 48,
                jit_profile: crate::translation::JitSemanticProfile::S0_09Fixture,
            },
            ExecutionProfile {
                mode: crate::translation::GuestExecutionMode::Long64,
                canonical_address_bits: 57,
                jit_profile: crate::translation::JitSemanticProfile::S0_09Fixture,
            },
            ExecutionProfile {
                mode: crate::translation::GuestExecutionMode::Long64,
                canonical_address_bits: 48,
                jit_profile: crate::translation::JitSemanticProfile::Other,
            },
        ];
        for profile in unsupported_profiles {
            assert!(matches!(
                run_cached(
                    view,
                    &admission,
                    ExecutionContextSnapshot::new(test_context().id, profile),
                    &dependencies,
                    &mut state,
                    1,
                    &mut cache,
                    &mut backend,
                ),
                Err(DispatchError::ExecutionContext(
                    ExecutionContextError::UnsupportedProfile(_)
                ))
            ));
        }
        assert_eq!(cache.counters().successful_compiles, 0);
        assert_eq!(cache.counters().successful_publications, 0);
        assert_eq!(cache.resident_entries(), 0);
        let tracked = tracker.borrow();
        assert_eq!(tracked.publishes, 0);
        assert_eq!(tracked.executions, 0);
        assert_eq!(tracked.live, 0);
    }

    #[test]
    fn cache_replaces_changed_key_only_after_dropping_old_handle() {
        let code_a = [0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0];
        let code_b = [0x48, 0xB8, 2, 0, 0, 0, 0, 0, 0, 0];
        let (view_a, admission_a) = cached_fixture(&code_a, 0x6100);
        let (view_b, admission_b) = cached_fixture(&code_b, 0x6100);
        let deps_a = dependency(view_a, 1);
        let deps_b = dependency(view_b, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view_a.base_rip(),
            ..LongModeState::default()
        };
        run_cached(
            view_a,
            &admission_a,
            test_context(),
            &deps_a,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().live, 1);
        state.rip = view_b.base_rip();
        run_cached(
            view_b,
            &admission_b,
            test_context(),
            &deps_b,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        let status = tracker.borrow();
        assert_eq!(status.publishes, 2);
        assert_eq!(status.drops, 1);
        assert_eq!(status.live, 1);
        assert_eq!(status.max_live, 1);
        drop(status);
        cache.clear();
        assert_eq!(tracker.borrow().live, 0);
        assert_eq!(tracker.borrow().drops, 2);

        state.rip = view_b.base_rip();
        run_cached(
            view_b,
            &admission_b,
            test_context(),
            &deps_b,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().live, 1);
        drop(cache);
        assert_eq!(tracker.borrow().live, 0);
        assert_eq!(tracker.borrow().drops, 3);
    }

    #[test]
    fn dependency_change_replaces_entry_and_invalid_snapshot_fails_closed() {
        let code = [0x48, 0xB8, 3, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x6200);
        let deps_v1 = dependency(view, 1);
        let deps_v2 = dependency(view, 2);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        run_cached(
            view,
            &admission,
            test_context(),
            &deps_v1,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        state.rip = view.base_rip();
        run_cached(
            view,
            &admission,
            test_context(),
            &deps_v2,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().publishes, 2);

        let nz = |value| NonZeroU64::new(value).unwrap();
        let mut deps_new_mapping = deps_v2;
        deps_new_mapping[0].identity.mapping = MappingId::new(nz(77));
        deps_new_mapping[0].identity.backing = BackingId::new(nz(88));
        state.rip = view.base_rip();
        run_cached(
            view,
            &admission,
            test_context(),
            &deps_new_mapping,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().publishes, 3);

        state.rip = view.base_rip();
        let mut non_executable = deps_new_mapping;
        non_executable[0].identity.executable = false;
        assert!(matches!(
            run_cached(
                view,
                &admission,
                test_context(),
                &non_executable,
                &mut state,
                1,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::TranslationKey(
                TranslationKeyError::NonExecutableDependency { .. }
            ))
        ));
        assert_eq!(tracker.borrow().publishes, 3);
    }

    #[test]
    fn cache_rejects_oversize_before_publish_and_surfaces_publish_errors() {
        let code = [0x48, 0xB8, 4, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x6300);
        let dependencies = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut tiny_cache = SingleEntryBlockCache::new(1, 1).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        assert!(matches!(
            run_cached(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                1,
                &mut tiny_cache,
                &mut backend,
            ),
            Err(DispatchError::Cache(
                CacheError::MinimumEntryTooLarge { .. }
            ))
        ));
        assert_eq!(tracker.borrow().publishes, 0);

        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        tracker.borrow_mut().fail_publish = true;
        assert_eq!(
            run_cached(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                1,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::CachePublish("publish failed"))
        );
        assert_eq!(cache.resident_entries(), 0);
    }

    #[test]
    fn oversized_code_view_is_rejected_before_translation_key_copy() {
        let mut code = Vec::with_capacity(2_000);
        for _ in 0..200 {
            code.extend_from_slice(&[0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0]);
        }
        let (view, admission) = cached_fixture(&code, 0x6350);
        let dependencies = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, 1_000).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        assert!(matches!(
            run_cached(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                1,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::Cache(CacheError::MinimumEntryTooLarge {
                minimum: 2_000..,
                max: 1_000,
            }))
        ));
        assert_eq!(tracker.borrow().publishes, 0);
        assert_eq!(cache.resident_entries(), 0);
    }

    #[test]
    fn cache_config_and_zero_budget_do_not_publish() {
        assert_eq!(
            SingleEntryBlockCache::<()>::new(2, 1024).err().unwrap(),
            CacheConfigError::RequiresExactlyOneBlock { requested: 2 }
        );
        assert_eq!(
            SingleEntryBlockCache::<()>::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES + 1)
                .err()
                .unwrap(),
            CacheConfigError::InvalidByteLimit {
                requested: MAX_SINGLE_ENTRY_CACHE_BYTES + 1,
                max: MAX_SINGLE_ENTRY_CACHE_BYTES,
            }
        );
        let code = [0x74, 0];
        let (view, admission) = cached_fixture(&code, 0x6400);
        let dependencies = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let result = run_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            0,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(result.exit, DispatchExit::BudgetExhausted);
        assert_eq!(tracker.borrow().publishes, 0);
        state.rip = view.code_end();
        let result = run_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            0,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(result.exit, DispatchExit::CodeEnd);
        assert_eq!(tracker.borrow().publishes, 0);
        state.rip = view.base_rip() + 1;
        assert_eq!(
            run_cached(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                0,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::EntryNotAdmitted {
                rip: view.base_rip() + 1
            })
        );
        assert_eq!(tracker.borrow().publishes, 0);
    }

    #[test]
    fn cached_dispatch_samples_stop_at_guest_boundary_and_does_not_leak_default_stop() {
        let code = [0x48, 0xB8, 5, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x6500);
        let dependencies = dependency(view, 1);
        let stop_requested = AtomicU8::new(0);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let stopped = run_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            DispatchControl {
                stop_rip: Some(view.code_end()),
                stop_requested: &stop_requested,
                event_requested: None,
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            stopped.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopAddress)
        );
        assert_eq!(stopped.retired_instructions, 1);

        // The default API's cooperative flag belongs to this invocation. A
        // backend that requests STOP once cannot poison a later call.
        tracker.borrow_mut().request_stop_once = true;
        state.rip = view.base_rip();
        let stop_once = run_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            stop_once.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        state.rip = view.base_rip();
        let next_call = run_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(next_call.exit, DispatchExit::CodeEnd);
        assert_eq!(tracker.borrow().publishes, 1);
    }

    #[test]
    fn multi_entry_cache_reuses_a_b_a_and_self_loop_without_refreshing_fifo_order() {
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache =
            MultiEntryBlockCache::new(2, 4096, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        let a = [0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0];
        let b = [0x48, 0xB8, 2, 0, 0, 0, 0, 0, 0, 0];
        run_multi_fixture(&a, 0x7000, &mut cache, &mut backend).unwrap();
        run_multi_fixture(&b, 0x7000, &mut cache, &mut backend).unwrap();
        run_multi_fixture(&a, 0x7000, &mut cache, &mut backend).unwrap();
        assert_eq!(cache.counters().successful_compiles, 2);
        assert_eq!(cache.counters().successful_publications, 2);
        assert_eq!(cache.counters().hits, 1);
        assert_eq!(tracker.borrow().live, 2);

        let loop_code = [0x75, 0xFE];
        let (view, admission) = cached_fixture(&loop_code, 0x7100);
        let deps = dependency(view, 1);
        let mut looping = MockCachedBackend(Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            loop_to_entry: true,
            ..PublishedTracker::default()
        })));
        let mut loop_cache =
            MultiEntryBlockCache::new(2, 4096, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let result = run_multi_cached(
            view,
            &admission,
            test_context(),
            &deps,
            &mut state,
            3,
            &mut loop_cache,
            &mut looping,
        )
        .unwrap();
        assert_eq!(result.exit, DispatchExit::BudgetExhausted);
        assert_eq!(result.retired_instructions, 3);
        assert_eq!(loop_cache.counters().successful_publications, 1);
        assert_eq!(loop_cache.counters().hits, 2);
    }

    #[test]
    fn multi_cache_a_b_a_churn_reclaims_handles_at_capacity_one_and_two() {
        let a = [0x48, 0xb8, 0x41, 0, 0, 0, 0, 0, 0, 0];
        let b = [0x48, 0xb8, 0x42, 0, 0, 0, 0, 0, 0, 0];
        for capacity in [1, 2] {
            let tracker = Rc::new(RefCell::new(PublishedTracker {
                allow_multiple_live: true,
                capacity,
                ..PublishedTracker::default()
            }));
            let mut backend = MockCachedBackend(tracker.clone());
            let mut cache =
                MultiEntryBlockCache::new(capacity, 4096, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES)
                    .unwrap();
            run_multi_fixture(&a, 0x7250, &mut cache, &mut backend).unwrap();
            assert_eq!(cache.resident_entries(), 1);
            assert_eq!(tracker.borrow().live, 1);
            run_multi_fixture(&b, 0x7250, &mut cache, &mut backend).unwrap();
            assert_eq!(cache.resident_entries(), capacity.min(2));
            assert_eq!(tracker.borrow().live, capacity.min(2));
            run_multi_fixture(&a, 0x7250, &mut cache, &mut backend).unwrap();
            let expected_publications = if capacity == 1 { 3 } else { 2 };
            assert_eq!(tracker.borrow().publishes, expected_publications);
            assert_eq!(tracker.borrow().live, capacity.min(2));
            if capacity == 1 {
                assert_eq!(cache.counters().hits, 0);
                assert_eq!(tracker.borrow().drops, 2);
            } else {
                assert_eq!(cache.counters().hits, 1);
                assert_eq!(tracker.borrow().drops, 0);
            }
            cache.clear();
            assert_eq!(cache.resident_entries(), 0);
            assert_eq!(cache.counters().resident_code_bytes, 0);
            assert_eq!(cache.counters().resident_logical_metadata_bytes, 0);
            assert_eq!(tracker.borrow().live, 0);
            assert_eq!(tracker.borrow().drops, expected_publications);
        }
    }

    #[test]
    fn cached_stop_and_executor_error_release_only_at_safe_return_boundaries() {
        let code = [0x48, 0xb8, 0x51, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x7260);
        let dependencies = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let stop = AtomicU8::new(0);

        // STOP before dispatch entry makes no backend call and retains any
        // already published entry. First compile it, then stop a hit.
        run_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        let executions_before_stop = tracker.borrow().executions;
        stop.store(1, Ordering::Release);
        state.rip = view.base_rip();
        let stopped_before = run_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: None,
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            stopped_before.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(tracker.borrow().executions, executions_before_stop);
        assert_eq!(tracker.borrow().live, 1);
        assert_eq!(cache.resident_entries(), 1);

        // STOP raised by the backend after successful generated execution is
        // observed after result decode; the resident handle remains reusable.
        stop.store(0, Ordering::Release);
        tracker.borrow_mut().request_stop_once = true;
        state.rip = view.base_rip();
        let stopped_after = run_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: None,
            },
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            stopped_after.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(tracker.borrow().live_during_execute, [1, 1]);
        assert_eq!(tracker.borrow().live, 1);
        assert_eq!(tracker.borrow().drops, 0);
        assert_eq!(cache.resident_entries(), 1);

        // Injected backend/executor failure drops the entry only after the
        // executor has returned its error to dispatch, then a later call may
        // republish. This test does not inject a native helper failure.
        stop.store(0, Ordering::Release);
        tracker.borrow_mut().fail_execute_once = true;
        state.rip = view.base_rip();
        assert_eq!(
            run_cached(
                view,
                &admission,
                test_context(),
                &dependencies,
                &mut state,
                1,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::CacheExecutor("execution failed"))
        );
        assert_eq!(tracker.borrow().live_during_execute, [1, 1, 1]);
        assert_eq!(tracker.borrow().live, 0);
        assert_eq!(tracker.borrow().drops, 1);
        assert_eq!(cache.resident_entries(), 0);
        state.rip = view.base_rip();
        run_cached(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().publishes, 2);
        cache.clear();
        assert_eq!(tracker.borrow().live, 0);
    }

    #[test]
    fn opt_in_lru_refreshes_hit_order_while_new_constructor_remains_fifo() {
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker);
        let mut lru = MultiEntryBlockCache::with_policy(
            2,
            4096,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
            CacheEvictionPolicy::Lru,
        )
        .unwrap();
        let a = [0x48, 0xB8, 0x11, 0, 0, 0, 0, 0, 0, 0];
        let b = [0x48, 0xB8, 0x22, 0, 0, 0, 0, 0, 0, 0];
        let c = [0x48, 0xB8, 0x33, 0, 0, 0, 0, 0, 0, 0];
        run_multi_fixture(&a, 0x7200, &mut lru, &mut backend).unwrap();
        run_multi_fixture(&b, 0x7200, &mut lru, &mut backend).unwrap();
        run_multi_fixture(&a, 0x7200, &mut lru, &mut backend).unwrap();
        run_multi_fixture(&c, 0x7200, &mut lru, &mut backend).unwrap();
        run_multi_fixture(&a, 0x7200, &mut lru, &mut backend).unwrap();
        assert_eq!(lru.eviction_policy(), CacheEvictionPolicy::Lru);
        assert_eq!(lru.counters().successful_compiles, 3);
        assert_eq!(lru.counters().successful_publications, 3);

        let fifo_tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            ..PublishedTracker::default()
        }));
        let mut fifo_backend = MockCachedBackend(fifo_tracker);
        let mut fifo =
            MultiEntryBlockCache::new(2, 4096, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        assert_eq!(fifo.eviction_policy(), CacheEvictionPolicy::Fifo);
        run_multi_fixture(&a, 0x7200, &mut fifo, &mut fifo_backend).unwrap();
        run_multi_fixture(&b, 0x7200, &mut fifo, &mut fifo_backend).unwrap();
        run_multi_fixture(&a, 0x7200, &mut fifo, &mut fifo_backend).unwrap();
        run_multi_fixture(&c, 0x7200, &mut fifo, &mut fifo_backend).unwrap();
        run_multi_fixture(&a, 0x7200, &mut fifo, &mut fifo_backend).unwrap();
        assert_eq!(fifo.counters().successful_compiles, 4);
    }

    #[test]
    fn multi_entry_cache_reuses_sixteen_keys_and_evicts_exact_fifo_victim() {
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache =
            MultiEntryBlockCache::new(16, 65_536, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        let fixtures: Vec<[u8; 10]> = (0..17)
            .map(|value| {
                let mut bytes = [0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0];
                bytes[2] = value;
                bytes
            })
            .collect();
        for bytes in &fixtures[..16] {
            run_multi_fixture(bytes, 0x7200, &mut cache, &mut backend).unwrap();
        }
        for bytes in &fixtures[..16] {
            run_multi_fixture(bytes, 0x7200, &mut cache, &mut backend).unwrap();
        }
        assert_eq!(cache.counters().successful_compiles, 16);
        assert_eq!(cache.counters().successful_publications, 16);
        assert_eq!(cache.counters().hits, 16);
        assert_eq!(cache.resident_entries(), 16);

        run_multi_fixture(&fixtures[16], 0x7200, &mut cache, &mut backend).unwrap();
        assert_eq!(cache.counters().evictions, 1);
        // The newest insertion did not refresh FIFO order: key 1 still hits,
        // and key 0 was the exact victim and must compile again.
        run_multi_fixture(&fixtures[1], 0x7200, &mut cache, &mut backend).unwrap();
        run_multi_fixture(&fixtures[0], 0x7200, &mut cache, &mut backend).unwrap();
        assert_eq!(cache.counters().successful_compiles, 18);
        assert_eq!(cache.counters().hits, 17);
        assert_eq!(cache.counters().evictions, 2);
        assert!(cache.counters().resident_code_bytes <= cache.max_code_bytes());
        assert!(cache.counters().resident_logical_metadata_bytes <= cache.max_metadata_bytes());
    }

    fn metadata_size_for_fixture(bytes: &[u8], base: u64) -> usize {
        let (view, admission) = cached_fixture(bytes, base);
        let built = build_dispatch_block(view, base, &admission).unwrap();
        let guest = match &built {
            crate::block::BuildOutcome::Block(block) => block,
            _ => panic!(),
        };
        let deps = dependency(view, 1);
        let key = TranslationKey::for_block(
            view,
            guest,
            CompileFlavor::S0_09FixtureDispatch,
            test_context(),
            &deps,
        )
        .unwrap();
        let compiled = match crate::block_compile::compile_prepared_dispatch_block(
            view.code_end(),
            built,
            &admission,
        )
        .unwrap()
        {
            BlockCompileOutcome::Compiled(block) => block,
            _ => panic!(),
        };
        cached_entry_metadata_size::<MockPublished>(&key, &compiled).unwrap()
    }

    #[test]
    fn metadata_pressure_can_evict_multiple_entries_before_publish() {
        let short_a = [0x48, 0xB8, 11, 0, 0, 0, 0, 0, 0, 0];
        let short_b = [0x48, 0xB8, 12, 0, 0, 0, 0, 0, 0, 0];
        // The first Jcc ends the compiled block. The complete admitted tail
        // still increases key metadata without increasing emitted code.
        let mut long = vec![0x74, 0x00];
        for value in 0..20 {
            long.extend_from_slice(&[0x48, 0xB8, value, 0, 0, 0, 0, 0, 0, 0]);
        }
        let small = metadata_size_for_fixture(&short_a, 0x7300);
        let large = metadata_size_for_fixture(&long, 0x7300);
        assert!(large > small);
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker);
        let mut cache =
            MultiEntryBlockCache::new(4, 65_536, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        run_multi_fixture(&short_a, 0x7300, &mut cache, &mut backend).unwrap();
        run_multi_fixture(&short_b, 0x7300, &mut cache, &mut backend).unwrap();
        cache.max_metadata_bytes = large + small - 1;
        run_multi_fixture(&long, 0x7300, &mut cache, &mut backend).unwrap();
        assert_eq!(cache.counters().evictions, 2);
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(cache.counters().resident_logical_metadata_bytes, large);
    }

    #[test]
    fn multi_cache_rejects_oversize_without_eviction_and_malformed_dependencies_without_execution()
    {
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache =
            MultiEntryBlockCache::new(4, 4096, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        let a = [0x48, 0xB8, 21, 0, 0, 0, 0, 0, 0, 0];
        run_multi_fixture(&a, 0x7400, &mut cache, &mut backend).unwrap();
        let existing_code = cache.counters().resident_code_bytes;
        cache.max_code_bytes = existing_code;
        let two = [
            0x48, 0xB8, 22, 0, 0, 0, 0, 0, 0, 0, 0x48, 0xB8, 23, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert!(matches!(
            run_multi_fixture(&two, 0x7400, &mut cache, &mut backend),
            Err(DispatchError::Cache(CacheError::EntryTooLarge { .. }))
        ));
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(cache.counters().evictions, 0);

        let before_exec = tracker.borrow().executions;
        let (view, admission) = cached_fixture(&a, 0x7400);
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        assert!(matches!(
            run_multi_cached(
                view,
                &admission,
                test_context(),
                &[],
                &mut state,
                1,
                &mut cache,
                &mut backend
            ),
            Err(DispatchError::TranslationKey(_))
        ));
        assert_eq!(tracker.borrow().executions, before_exec);
        assert_eq!(cache.resident_entries(), 1);
    }

    #[test]
    fn post_publish_stale_candidate_is_dropped_before_entry_or_execution() {
        let code = [0x48, 0xB8, 24, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x7450);
        let built = build_dispatch_block(view, view.base_rip(), &admission).unwrap();
        let source = match &built {
            crate::block::BuildOutcome::Block(block) => block,
            _ => panic!("MOV fixture should produce a block"),
        };
        let dependencies_v1 = dependency(view, 1);
        let dependencies_v2 = dependency(view, 2);
        let key = TranslationKey::for_block(
            view,
            source,
            CompileFlavor::S0_09FixtureDispatch,
            test_context(),
            &dependencies_v1,
        )
        .unwrap();
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            live: 1,
            ..PublishedTracker::default()
        }));
        let candidate = MockPublished(tracker.clone(), 1);
        let checked = validate_published_candidate::<&'static str, _>(
            &key,
            view,
            &dependencies_v2,
            candidate,
        );
        assert!(matches!(
            checked,
            Err(DispatchError::StaleDependencySnapshot)
        ));
        assert_eq!(
            tracker.borrow().live,
            0,
            "stale candidate handle is dropped"
        );
        assert_eq!(tracker.borrow().drops, 1);
        assert_eq!(tracker.borrow().executions, 0);
        let cache = MultiEntryBlockCache::<MockPublished>::new(
            1,
            4096,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .unwrap();
        assert_eq!(cache.resident_entries(), 0);
    }

    #[test]
    fn multi_cache_checks_backend_capacity_and_config_limits() {
        assert!(matches!(
            MultiEntryBlockCache::<()>::new(17, 1, 1),
            Err(MultiEntryCacheConfigError::InvalidEntryLimit { .. })
        ));
        assert!(matches!(
            MultiEntryBlockCache::<()>::new(1, 65_537, 1),
            Err(MultiEntryCacheConfigError::InvalidCodeLimit { .. })
        ));
        assert!(matches!(
            MultiEntryBlockCache::<()>::new(1, 1, 512 * 1024 + 1),
            Err(MultiEntryCacheConfigError::InvalidMetadataLimit { .. })
        ));
        let code = [0x48, 0xB8, 31, 0, 0, 0, 0, 0, 0, 0];
        let (view, admission) = cached_fixture(&code, 0x7500);
        let deps = dependency(view, 1);
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            capacity: 1,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache =
            MultiEntryBlockCache::new(2, 4096, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        assert!(matches!(
            run_multi_cached(
                view,
                &admission,
                test_context(),
                &deps,
                &mut state,
                1,
                &mut cache,
                &mut backend
            ),
            Err(DispatchError::MultiCacheConfig(
                MultiEntryCacheConfigError::BackendCapacityTooSmall {
                    requested: 2,
                    available: 1
                }
            ))
        ));
        assert_eq!(tracker.borrow().publishes, 0);
    }

    #[test]
    fn publish_failure_keeps_prior_fifo_evictions_but_does_not_install_candidate() {
        let tracker = Rc::new(RefCell::new(PublishedTracker {
            allow_multiple_live: true,
            ..PublishedTracker::default()
        }));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache =
            MultiEntryBlockCache::new(2, 4096, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES).unwrap();
        let a = [0x48, 0xB8, 51, 0, 0, 0, 0, 0, 0, 0];
        let b = [0x48, 0xB8, 52, 0, 0, 0, 0, 0, 0, 0];
        let c = [0x48, 0xB8, 53, 0, 0, 0, 0, 0, 0, 0];
        run_multi_fixture(&a, 0x7600, &mut cache, &mut backend).unwrap();
        run_multi_fixture(&b, 0x7600, &mut cache, &mut backend).unwrap();
        tracker.borrow_mut().fail_publish = true;
        assert_eq!(
            run_multi_fixture(&c, 0x7600, &mut cache, &mut backend),
            Err(DispatchError::CachePublish("publish failed"))
        );
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(cache.counters().evictions, 1);
        assert_eq!(
            cache.counters().resident_code_bytes,
            emitted_code_bytes(&cache.entries[0].block).unwrap()
        );
        assert_eq!(tracker.borrow().live, 1);
        assert_eq!(tracker.borrow().publishes, 2);
    }

    #[cfg(feature = "phase-observation")]
    #[test]
    fn observed_multi_cache_matches_default_and_counts_cold_and_hit_phases() {
        let code = [0x48, 0xB8, 9, 0, 0, 0, 0, 0, 0, 0];
        let base = 0x7A80;
        let (view, admission) = cached_fixture(&code, base);
        let dependencies = dependency(view, 1);
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut plain_state = LongModeState {
            cr3: None,
            rip: base,
            ..LongModeState::default()
        };
        let mut observed_state = plain_state.clone();
        let mut plain_cache = new_fixture_multi_cache();
        let mut observed_cache = new_fixture_multi_cache();
        let mut plain_backend = new_fixture_backend().0;
        let mut observed_backend = new_fixture_backend().0;

        let plain = run_multi_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut plain_state,
            4,
            control,
            &mut plain_cache,
            &mut plain_backend,
        )
        .unwrap();
        let mut recorder = PhaseRecorder::default();
        let observed = run_multi_cached_controlled_observed(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut observed_state,
            4,
            control,
            &mut observed_cache,
            &mut observed_backend,
            &mut recorder,
        )
        .unwrap();
        assert_eq!(observed, plain);
        assert_eq!(observed_state, plain_state);
        assert_eq!(observed_cache.counters(), plain_cache.counters());
        let cold_totals = recorder.totals();
        assert_eq!(
            cold_totals
                .iter()
                .map(|total| total.calls)
                .collect::<Vec<_>>(),
            vec![1; 7]
        );

        observed_state.rip = base;
        plain_state.rip = base;
        let plain_hit = run_multi_cached_controlled(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut plain_state,
            4,
            control,
            &mut plain_cache,
            &mut plain_backend,
        )
        .unwrap();
        let observed_hit = run_multi_cached_controlled_observed(
            view,
            &admission,
            test_context(),
            &dependencies,
            &mut observed_state,
            4,
            control,
            &mut observed_cache,
            &mut observed_backend,
            &mut recorder,
        )
        .unwrap();
        assert_eq!(observed_hit, plain_hit);
        assert_eq!(observed_state, plain_state);
        assert_eq!(observed_cache.counters(), plain_cache.counters());
        let totals = recorder.totals();
        assert_eq!(totals[0].calls, 2);
        assert_eq!(totals[1].calls, 2);
        assert_eq!(totals[2].calls, 1);
        assert_eq!(totals[3].calls, 1);
        assert_eq!(totals[4].calls, 1);
        assert_eq!(totals[5].calls, 1);
        assert_eq!(totals[6].calls, 2);
        assert_eq!(observed_cache.counters().hits, 1);
        assert_eq!(observed_cache.counters().successful_compiles, 1);
        assert_eq!(observed_cache.counters().successful_publications, 1);
    }
    // S3-53 tests compare each public wrapper with the new entry and the
    // pre-existing strategy, including the emitted native code.
    #[derive(Default)]
    struct S3_53RecordingExecutor {
        next_rip: u64,
        retired: u64,
        branch_exit: bool,
        emissions: Vec<(Vec<u8>, u64)>,
    }

    impl ControlledBlockExecutor for S3_53RecordingExecutor {
        type Error = ();

        fn execute_one_block_controlled(
            &mut self,
            block: &crate::block_compile::CompiledBlock,
            state: &mut LongModeState,
            incoming_budget: u64,
            _control: DispatchControl<'_>,
        ) -> Result<BlockExecutionReport, Self::Error> {
            self.emissions
                .push((block.emitted().to_le_bytes().unwrap(), incoming_budget));
            state.rip = self.next_rip;
            state.gpr[0] = 0x1234;
            state.rflags = 0x202;
            Ok(report(
                if self.branch_exit {
                    RuntimeExitTag::DispatchCandidate
                } else {
                    RuntimeExitTag::NeedInterpreter
                },
                incoming_budget - self.retired,
                self.retired,
            ))
        }
    }

    macro_rules! s3_53_uncached_wrapper_test {
        ($test:ident, $wrapper:ident, $admission_ty:ty, $profile:ident, $flags:ident,
         $legacy_admission:ident, $legacy_compilation:ident, $bytes:expr, $native_retired:expr) => {
            #[test]
            fn $test() {
                let base = 0x9300;
                let mut bytes = Vec::from($bytes.as_slice());
                // A second baseline flag writer makes eager/demand compilation
                // observable in the emitted block where the profile allows it.
                bytes.extend_from_slice(&[0x48, 0x03, 0xc0, 0x90]);
                let view = CodeView::new(base, &bytes).unwrap();
                let admission = <$admission_ty>::from_code(view).unwrap();
                let initial = LongModeState {
                    rip: base,
                    ..LongModeState::default()
                };
                let mut wrapper_state = initial.clone();
                let mut direct_state = initial.clone();
                let mut legacy_state = initial;
                let native_end = view.code_end() - 1;
                let mut wrapper_executor = S3_53RecordingExecutor {
                    next_rip: native_end,
                    retired: $native_retired,
                    ..Default::default()
                };
                let mut direct_executor = S3_53RecordingExecutor {
                    next_rip: native_end,
                    retired: $native_retired,
                    ..Default::default()
                };
                let mut legacy_executor = S3_53RecordingExecutor {
                    next_rip: native_end,
                    retired: $native_retired,
                    ..Default::default()
                };
                let stop = AtomicU8::new(0);
                let control = DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: None,
                };
                let wrapper = $wrapper(
                    view,
                    &admission,
                    &mut wrapper_state,
                    6,
                    control,
                    &mut wrapper_executor,
                )
                .unwrap();
                let direct = run_register_dispatch(
                    view,
                    RegisterDispatchProfile::$profile(&admission),
                    RegisterFlagsMode::$flags,
                    &mut direct_state,
                    6,
                    control,
                    UncachedRegisterPath(&mut direct_executor),
                )
                .unwrap();
                let legacy = run_with_nop_fallback_strategy(
                    view,
                    FallbackAdmission::$legacy_admission(&admission),
                    &mut legacy_state,
                    6,
                    control,
                    &mut legacy_executor,
                    NopFallbackCompilation::$legacy_compilation,
                )
                .unwrap();
                assert_eq!(wrapper, direct);
                assert_eq!(wrapper, legacy);
                assert_eq!(wrapper_state, direct_state);
                assert_eq!(wrapper_state, legacy_state);
                assert_eq!(wrapper_executor.emissions, direct_executor.emissions);
                assert_eq!(wrapper_executor.emissions, legacy_executor.emissions);
                assert_eq!(wrapper.exit, DispatchExit::CodeEnd);
                assert_eq!(wrapper.remaining_budget, 3);
                assert_eq!(wrapper.retired_native, $native_retired);
                assert_eq!(wrapper.retired_interpreter, 3 - $native_retired);
                assert_eq!(wrapper.blocks_executed, 1);
                assert_eq!(wrapper_executor.emissions.len(), 1);
            }
        };
    }
    s3_53_uncached_wrapper_test!(
        s3_53_native_register_v1,
        run_with_native_register_v1,
        NativeRegisterV1Admission,
        NativeRegisterV1,
        Eager,
        NativeRegisterV1,
        NativeRegisterV1,
        [0x48, 0xff, 0xc0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_register_v1_demand_flags_candidate,
        run_with_native_register_v1_demand_flags_candidate,
        NativeRegisterV1Admission,
        NativeRegisterV1,
        DemandFlagsCandidate,
        NativeRegisterV1,
        NativeRegisterV1DemandFlagsCandidate,
        [0x48, 0xff, 0xc0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_inc_rax,
        run_with_native_inc_rax,
        IncRaxInterpreterFallbackAdmission,
        NativeIncRax,
        Eager,
        NopIncRax,
        NativeIncRax,
        [0x48, 0xff, 0xc0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_inc_rax_demand_flags_candidate,
        run_with_native_inc_rax_demand_flags_candidate,
        IncRaxInterpreterFallbackAdmission,
        NativeIncRax,
        DemandFlagsCandidate,
        NopIncRax,
        NativeIncRaxDemandFlagsCandidate,
        [0x48, 0xff, 0xc0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_dec_rax,
        run_with_native_dec_rax,
        DecRaxInterpreterFallbackAdmission,
        NativeDecRax,
        Eager,
        NopDecRax,
        NativeDecRax,
        [0x48, 0xff, 0xc8],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_dec_rax_demand_flags_candidate,
        run_with_native_dec_rax_demand_flags_candidate,
        DecRaxInterpreterFallbackAdmission,
        NativeDecRax,
        DemandFlagsCandidate,
        NopDecRax,
        NativeDecRaxDemandFlagsCandidate,
        [0x48, 0xff, 0xc8],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_neg_rax,
        run_with_native_neg_rax,
        NegRaxInterpreterFallbackAdmission,
        NativeNegRax,
        Eager,
        NopNegRax,
        NativeNegRax,
        [0x48, 0xf7, 0xd8],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_neg_rax_demand_flags_candidate,
        run_with_native_neg_rax_demand_flags_candidate,
        NegRaxInterpreterFallbackAdmission,
        NativeNegRax,
        DemandFlagsCandidate,
        NopNegRax,
        NativeNegRaxDemandFlagsCandidate,
        [0x48, 0xf7, 0xd8],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_not_rax,
        run_with_native_not_rax,
        NotRaxInterpreterFallbackAdmission,
        NativeNotRax,
        Eager,
        NopNotRax,
        NativeNotRax,
        [0x48, 0xf7, 0xd0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_not_rax_demand_flags_candidate,
        run_with_native_not_rax_demand_flags_candidate,
        NotRaxInterpreterFallbackAdmission,
        NativeNotRax,
        DemandFlagsCandidate,
        NopNotRax,
        NativeNotRaxDemandFlagsCandidate,
        [0x48, 0xf7, 0xd0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_lea_rax_rbx,
        run_with_native_lea_rax_rbx,
        LeaRaxRbxInterpreterFallbackAdmission,
        NativeLeaRaxRbx,
        Eager,
        NopLeaRaxRbx,
        NativeLeaRaxRbx,
        [0x48, 0x8d, 0x03],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_lea_rax_rbx_demand_flags_candidate,
        run_with_native_lea_rax_rbx_demand_flags_candidate,
        LeaRaxRbxInterpreterFallbackAdmission,
        NativeLeaRaxRbx,
        DemandFlagsCandidate,
        NopLeaRaxRbx,
        NativeLeaRaxRbxDemandFlagsCandidate,
        [0x48, 0x8d, 0x03],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_lea_rax_rbx_disp8,
        run_with_native_lea_rax_rbx_disp8,
        LeaRaxRbxDisp8InterpreterFallbackAdmission,
        NativeLeaRaxRbxDisp8,
        Eager,
        NopLeaRaxRbxDisp8,
        NativeLeaRaxRbxDisp8,
        [0x48, 0x8d, 0x43, 0x01],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_lea_rax_rbx_disp8_demand_flags_candidate,
        run_with_native_lea_rax_rbx_disp8_demand_flags_candidate,
        LeaRaxRbxDisp8InterpreterFallbackAdmission,
        NativeLeaRaxRbxDisp8,
        DemandFlagsCandidate,
        NopLeaRaxRbxDisp8,
        NativeLeaRaxRbxDisp8DemandFlagsCandidate,
        [0x48, 0x8d, 0x43, 0x01],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_lea_rax_rbx_rcx,
        run_with_native_lea_rax_rbx_rcx,
        LeaRaxRbxRcxInterpreterFallbackAdmission,
        NativeLeaRaxRbxRcx,
        Eager,
        NopLeaRaxRbxRcx,
        NativeLeaRaxRbxRcx,
        [0x48, 0x8d, 0x04, 0x0b],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_lea_rax_rbx_rcx_demand_flags_candidate,
        run_with_native_lea_rax_rbx_rcx_demand_flags_candidate,
        LeaRaxRbxRcxInterpreterFallbackAdmission,
        NativeLeaRaxRbxRcx,
        DemandFlagsCandidate,
        NopLeaRaxRbxRcx,
        NativeLeaRaxRbxRcxDemandFlagsCandidate,
        [0x48, 0x8d, 0x04, 0x0b],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_lea_rax_rip_disp32,
        run_with_native_lea_rax_rip_disp32,
        LeaRaxRipDisp32InterpreterFallbackAdmission,
        NativeLeaRaxRipDisp32,
        Eager,
        NopLeaRaxRipDisp32,
        NativeLeaRaxRipDisp32,
        [0x48, 0x8d, 0x05, 0x01, 0, 0, 0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_lea_rax_rip_disp32_demand_flags_candidate,
        run_with_native_lea_rax_rip_disp32_demand_flags_candidate,
        LeaRaxRipDisp32InterpreterFallbackAdmission,
        NativeLeaRaxRipDisp32,
        DemandFlagsCandidate,
        NopLeaRaxRipDisp32,
        NativeLeaRaxRipDisp32DemandFlagsCandidate,
        [0x48, 0x8d, 0x05, 0x01, 0, 0, 0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_xchg_rax_rbx,
        run_with_native_xchg_rax_rbx,
        XchgRaxRbxInterpreterFallbackAdmission,
        NativeXchgRaxRbx,
        Eager,
        NopXchgRaxRbx,
        NativeXchgRaxRbx,
        [0x48, 0x87, 0xd8],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_xchg_rax_rbx_demand_flags_candidate,
        run_with_native_xchg_rax_rbx_demand_flags_candidate,
        XchgRaxRbxInterpreterFallbackAdmission,
        NativeXchgRaxRbx,
        DemandFlagsCandidate,
        NopXchgRaxRbx,
        NativeXchgRaxRbxDemandFlagsCandidate,
        [0x48, 0x87, 0xd8],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_add_rax_imm8,
        run_with_native_add_rax_imm8,
        AddRaxImm8InterpreterFallbackAdmission,
        NativeAddRaxImm8,
        Eager,
        NopAddRaxImm8,
        NativeAddRaxImm8,
        [0x48, 0x83, 0xc0, 0x01],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_add_rax_imm8_demand_flags_candidate,
        run_with_native_add_rax_imm8_demand_flags_candidate,
        AddRaxImm8InterpreterFallbackAdmission,
        NativeAddRaxImm8,
        DemandFlagsCandidate,
        NopAddRaxImm8,
        NativeAddRaxImm8DemandFlagsCandidate,
        [0x48, 0x83, 0xc0, 0x01],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_cmp_rax_imm8,
        run_with_native_cmp_rax_imm8,
        CmpRaxImm8InterpreterFallbackAdmission,
        NativeCmpRaxImm8,
        Eager,
        NopCmpRaxImm8,
        NativeCmpRaxImm8,
        [0x48, 0x83, 0xf8, 0x01],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_cmp_rax_imm8_demand_flags_candidate,
        run_with_native_cmp_rax_imm8_demand_flags_candidate,
        CmpRaxImm8InterpreterFallbackAdmission,
        NativeCmpRaxImm8,
        DemandFlagsCandidate,
        NopCmpRaxImm8,
        NativeCmpRaxImm8DemandFlagsCandidate,
        [0x48, 0x83, 0xf8, 0x01],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_sub_rax_imm8,
        run_with_native_sub_rax_imm8,
        SubRaxImm8InterpreterFallbackAdmission,
        NativeSubRaxImm8,
        Eager,
        NopSubRaxImm8,
        NativeSubRaxImm8,
        [0x48, 0x83, 0xe8, 0x01],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_sub_rax_imm8_demand_flags_candidate,
        run_with_native_sub_rax_imm8_demand_flags_candidate,
        SubRaxImm8InterpreterFallbackAdmission,
        NativeSubRaxImm8,
        DemandFlagsCandidate,
        NopSubRaxImm8,
        NativeSubRaxImm8DemandFlagsCandidate,
        [0x48, 0x83, 0xe8, 0x01],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_mov_rax_imm32,
        run_with_native_mov_rax_imm32,
        MovRaxImm32InterpreterFallbackAdmission,
        NativeMovRaxImm32,
        Eager,
        NopMovRaxImm32,
        NativeMovRaxImm32,
        [0x48, 0xc7, 0xc0, 0x01, 0, 0, 0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_native_mov_rax_imm32_demand_flags_candidate,
        run_with_native_mov_rax_imm32_demand_flags_candidate,
        MovRaxImm32InterpreterFallbackAdmission,
        NativeMovRaxImm32,
        DemandFlagsCandidate,
        NopMovRaxImm32,
        NativeMovRaxImm32DemandFlagsCandidate,
        [0x48, 0xc7, 0xc0, 0x01, 0, 0, 0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_nop_fallback_demand,
        run_with_nop_fallback_demand_flags_candidate,
        InterpreterFallbackAdmission,
        Nop,
        DemandFlagsCandidate,
        Nop,
        DemandFlagsCandidate,
        [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0],
        2
    );
    s3_53_uncached_wrapper_test!(
        s3_53_nop_inc_fallback_demand,
        run_with_nop_inc_rax_fallback_demand_flags_candidate,
        IncRaxInterpreterFallbackAdmission,
        InterpretedIncRax,
        DemandFlagsCandidate,
        NopIncRax,
        DemandFlagsCandidate,
        [0x48, 0xff, 0xc0],
        1
    );
    #[test]
    fn s3_53_cached_native_register_v1_demand_wrapper() {
        let bytes = [0x48, 0x83, 0xc0, 1];
        let base = 0x9500;
        let view = CodeView::new(base, &bytes).unwrap();
        let admission = native_admission(view);
        let dependencies = dependency(view, 1);
        let context = native_test_context_with_generation(1);
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut wrapper_cache =
            SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut direct_cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut legacy_cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let wrapper_tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let direct_tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let legacy_tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut wrapper_backend = mock_cached_backend(wrapper_tracker.clone());
        let mut direct_backend = mock_cached_backend(direct_tracker.clone());
        let mut legacy_backend = mock_cached_backend(legacy_tracker.clone());

        // Cold demand, warm demand, eager replacement, demand replacement.
        for demand in [true, true, false, true] {
            let initial = LongModeState {
                rip: base,
                ..LongModeState::default()
            };
            let mut wrapper_state = initial.clone();
            let mut direct_state = initial.clone();
            let mut legacy_state = initial;
            let wrapper = if demand {
                run_cached_native_register_v1_demand_flags_candidate_controlled(
                    view,
                    &admission,
                    context,
                    &dependencies,
                    &mut wrapper_state,
                    4,
                    control,
                    &mut wrapper_cache,
                    &mut wrapper_backend,
                )
                .unwrap()
            } else {
                run_cached_native_register_v1_controlled(
                    view,
                    &admission,
                    context,
                    &dependencies,
                    &mut wrapper_state,
                    4,
                    control,
                    &mut wrapper_cache,
                    &mut wrapper_backend,
                )
                .unwrap()
            };
            let direct = run_register_dispatch(
                view,
                &admission,
                if demand {
                    RegisterFlagsMode::DemandFlagsCandidate
                } else {
                    RegisterFlagsMode::Eager
                },
                &mut direct_state,
                4,
                control,
                SingleEntryRegisterPath {
                    execution_context: context,
                    dependencies: &dependencies,
                    cache: &mut direct_cache,
                    backend: &mut direct_backend,
                },
            )
            .unwrap();
            let mut observer = NoopObserver;
            let legacy = run_cached_native_register_v1_strategy_controlled::<false, false, _, _>(
                view,
                &admission,
                context,
                &dependencies,
                &mut legacy_state,
                4,
                control,
                &mut legacy_cache,
                &mut legacy_backend,
                &mut observer,
                if demand {
                    NativeRegisterV1CacheStrategy::DemandFlagsCandidate
                } else {
                    NativeRegisterV1CacheStrategy::Eager
                },
                None,
            )
            .unwrap();
            assert_eq!(wrapper, direct);
            assert_eq!(wrapper, legacy);
            assert_eq!(wrapper_state, direct_state);
            assert_eq!(wrapper_state, legacy_state);
            assert_eq!(wrapper.exit, DispatchExit::CodeEnd);
            assert_eq!(wrapper.retired_native, 1);
            assert_eq!(wrapper.retired_interpreter, 0);
            assert_eq!(wrapper.blocks_executed, 1);
            assert_eq!(wrapper_cache.counters(), direct_cache.counters());
            assert_eq!(wrapper_cache.counters(), legacy_cache.counters());
        }
        let counts = wrapper_cache.counters();
        assert_eq!((counts.misses, counts.hits), (3, 1));
        assert_eq!(
            (counts.successful_compiles, counts.successful_publications),
            (3, 3)
        );
        for tracker in [&wrapper_tracker, &direct_tracker, &legacy_tracker] {
            let tracker = tracker.borrow();
            assert_eq!(
                (
                    tracker.publishes,
                    tracker.executions,
                    tracker.live,
                    tracker.drops
                ),
                (3, 4, 1, 2)
            );
            assert_eq!(tracker.executed_ids, [1, 1, 2, 3]);
        }
        drop(wrapper_cache);
        drop(direct_cache);
        drop(legacy_cache);
        for tracker in [&wrapper_tracker, &direct_tracker, &legacy_tracker] {
            let tracker = tracker.borrow();
            assert_eq!((tracker.live, tracker.drops), (0, 3));
        }
    }

    #[test]
    fn s3_53_direct_entry_preserves_control_and_source_binding() {
        let code = [0x90];
        let other = [0xcc];
        let base = 0x9600;
        let view = CodeView::new(base, &code).unwrap();
        let admission = InterpreterFallbackAdmission::from_code(view).unwrap();
        for (stop_rip, stop_value, event_value, budget, expected) in [
            (
                Some(base),
                0,
                false,
                3,
                DispatchExit::Runtime(RuntimeExitTag::StopAddress),
            ),
            (
                None,
                1,
                false,
                3,
                DispatchExit::Runtime(RuntimeExitTag::StopRequested),
            ),
            (None, 0, false, 0, DispatchExit::BudgetExhausted),
            (None, 0, true, 3, DispatchExit::EventPending),
        ] {
            let stop = AtomicU8::new(stop_value);
            let event = AtomicBool::new(event_value);
            let control = DispatchControl {
                stop_rip,
                stop_requested: &stop,
                event_requested: Some(&event),
            };
            let initial = LongModeState {
                rip: base,
                ..LongModeState::default()
            };
            let mut wrapper_state = initial.clone();
            let mut direct_state = initial.clone();
            let mut wrapper_executor = S3_53RecordingExecutor::default();
            let mut direct_executor = S3_53RecordingExecutor::default();
            let wrapper = run_with_nop_fallback_demand_flags_candidate(
                view,
                &admission,
                &mut wrapper_state,
                budget,
                control,
                &mut wrapper_executor,
            )
            .unwrap();
            let direct = run_register_dispatch(
                view,
                RegisterDispatchProfile::Nop(&admission),
                RegisterFlagsMode::DemandFlagsCandidate,
                &mut direct_state,
                budget,
                control,
                UncachedRegisterPath(&mut direct_executor),
            )
            .unwrap();
            assert_eq!(wrapper, direct);
            assert_eq!(wrapper.exit, expected);
            assert_eq!(wrapper_state, initial);
            assert_eq!(direct_state, initial);
            assert!(wrapper_executor.emissions.is_empty());
            assert!(direct_executor.emissions.is_empty());
        }
        let wrong_view = CodeView::new(base, &other).unwrap();
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let initial = LongModeState {
            rip: base,
            ..LongModeState::default()
        };
        let mut wrapper_state = initial.clone();
        let mut direct_state = initial.clone();
        let mut wrapper_executor = S3_53RecordingExecutor::default();
        let mut direct_executor = S3_53RecordingExecutor::default();
        let wrapper = run_with_nop_fallback_demand_flags_candidate(
            wrong_view,
            &admission,
            &mut wrapper_state,
            3,
            control,
            &mut wrapper_executor,
        );
        let direct = run_register_dispatch(
            wrong_view,
            RegisterDispatchProfile::Nop(&admission),
            RegisterFlagsMode::DemandFlagsCandidate,
            &mut direct_state,
            3,
            control,
            UncachedRegisterPath(&mut direct_executor),
        );
        assert_eq!(wrapper, direct);
        assert_eq!(wrapper, Err(DispatchError::NopAdmissionDoesNotMatchCode));
        assert_eq!(wrapper_state, initial);
        assert_eq!(direct_state, initial);
        assert!(wrapper_executor.emissions.is_empty());
        assert!(direct_executor.emissions.is_empty());
    }

    #[test]
    fn s3_53_direct_entry_preserves_branch_and_nop_handoff() {
        // CMP RAX,RAX; JE next; NOP. The scripted native block commits Jcc.
        let bytes = [0x48, 0x3b, 0xc0, 0x74, 0, 0x90];
        let base = 0x9700;
        let view = CodeView::new(base, &bytes).unwrap();
        let admission = InterpreterFallbackAdmission::from_code(view).unwrap();
        let initial = LongModeState {
            rip: base,
            ..LongModeState::default()
        };
        let mut wrapper_state = initial.clone();
        let mut direct_state = initial;
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut wrapper_executor = S3_53RecordingExecutor {
            next_rip: base + 5,
            retired: 2,
            branch_exit: true,
            ..Default::default()
        };
        let mut direct_executor = S3_53RecordingExecutor {
            next_rip: base + 5,
            retired: 2,
            branch_exit: true,
            ..Default::default()
        };
        let wrapper = run_with_nop_fallback_demand_flags_candidate(
            view,
            &admission,
            &mut wrapper_state,
            5,
            control,
            &mut wrapper_executor,
        )
        .unwrap();
        let direct = run_register_dispatch(
            view,
            RegisterDispatchProfile::Nop(&admission),
            RegisterFlagsMode::DemandFlagsCandidate,
            &mut direct_state,
            5,
            control,
            UncachedRegisterPath(&mut direct_executor),
        )
        .unwrap();
        assert_eq!(wrapper, direct);
        assert_eq!(wrapper_state, direct_state);
        assert_eq!(wrapper_executor.emissions, direct_executor.emissions);
        assert_eq!(
            (
                wrapper.exit,
                wrapper.retired_native,
                wrapper.retired_interpreter,
                wrapper.remaining_budget
            ),
            (DispatchExit::CodeEnd, 2, 1, 2)
        );
        assert_eq!(wrapper_state.rip, view.code_end());
    }
    // Isolated dispatcher timing with the same admitted bytes and mock
    // executor/backend on both routes. This is not native guest execution.
    #[test]
    #[ignore = "S3-53 dispatcher overhead measurement"]
    fn s3_53_dispatch_entry_microbench() {
        use std::hint::black_box;
        use std::time::Instant;

        struct BenchBackend;
        impl ControlledBlockExecutor for BenchBackend {
            type Error = ();
            fn execute_one_block_controlled(
                &mut self,
                block: &crate::block_compile::CompiledBlock,
                state: &mut LongModeState,
                incoming_budget: u64,
                _: DispatchControl<'_>,
            ) -> Result<BlockExecutionReport, ()> {
                state.rip = block.code_end();
                Ok(report(
                    RuntimeExitTag::BlockComplete,
                    incoming_budget - 1,
                    1,
                ))
            }
        }
        impl CachedBlockBackend for BenchBackend {
            type Error = ();
            type Published = ();
            fn publish_cached_block(
                &mut self,
                _: &crate::block_compile::CompiledBlock,
            ) -> Result<(), ()> {
                Ok(())
            }
            fn execute_cached_block(
                &mut self,
                _: &(),
                block: &crate::block_compile::CompiledBlock,
                state: &mut LongModeState,
                incoming_budget: u64,
                _: DispatchControl<'_>,
            ) -> Result<BlockExecutionReport, ()> {
                state.rip = block.code_end();
                Ok(report(
                    RuntimeExitTag::BlockComplete,
                    incoming_budget - 1,
                    1,
                ))
            }
        }
        fn measure(iterations: usize, action: &mut impl FnMut()) -> u128 {
            let start = Instant::now();
            for _ in 0..iterations {
                action();
            }
            start.elapsed().as_nanos()
        }
        macro_rules! paired {
            ($name:literal, $round:expr, $iterations:expr, $old:expr, $new:expr) => {{
                let (old_ns, new_ns) = if $round % 2 == 0 {
                    let old = measure($iterations, &mut $old);
                    let new = measure($iterations, &mut $new);
                    (old, new)
                } else {
                    let new = measure($iterations, &mut $new);
                    let old = measure($iterations, &mut $old);
                    (old, new)
                };
                println!(
                    "S3_53_BENCH,{},{},{},{},{}",
                    $name, $round, $iterations, old_ns, new_ns
                );
            }};
        }
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let nop_bytes = [0x90];
        let nop_view = CodeView::new(0x9800, &nop_bytes).unwrap();
        let nop_admission = InterpreterFallbackAdmission::from_code(nop_view).unwrap();
        let inc_bytes = [0x48, 0xff, 0xc0];
        let inc_view = CodeView::new(0x9900, &inc_bytes).unwrap();
        let inc_admission = IncRaxInterpreterFallbackAdmission::from_code(inc_view).unwrap();
        let union_admission = NativeRegisterV1Admission::from_code(inc_view).unwrap();
        let dependencies = dependency(inc_view, 1);
        let context = native_test_context_with_generation(1);
        let mut old_nop_backend = BenchBackend;
        let mut new_nop_backend = BenchBackend;
        let mut old_inc_backend = BenchBackend;
        let mut new_inc_backend = BenchBackend;
        let mut old_cache_backend = BenchBackend;
        let mut new_cache_backend = BenchBackend;
        let mut old_cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut new_cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let mut old_nop = || {
            let mut state = LongModeState {
                rip: nop_view.base_rip(),
                ..LongModeState::default()
            };
            black_box(
                run_with_nop_fallback_strategy(
                    nop_view,
                    FallbackAdmission::Nop(&nop_admission),
                    &mut state,
                    2,
                    control,
                    &mut old_nop_backend,
                    NopFallbackCompilation::DemandFlagsCandidate,
                )
                .unwrap(),
            );
        };
        let mut new_nop = || {
            let mut state = LongModeState {
                rip: nop_view.base_rip(),
                ..LongModeState::default()
            };
            black_box(
                run_register_dispatch(
                    nop_view,
                    RegisterDispatchProfile::Nop(&nop_admission),
                    RegisterFlagsMode::DemandFlagsCandidate,
                    &mut state,
                    2,
                    control,
                    UncachedRegisterPath(&mut new_nop_backend),
                )
                .unwrap(),
            );
        };
        let mut old_inc = || {
            let mut state = LongModeState {
                rip: inc_view.base_rip(),
                ..LongModeState::default()
            };
            black_box(
                run_with_nop_fallback_strategy(
                    inc_view,
                    FallbackAdmission::NopIncRax(&inc_admission),
                    &mut state,
                    2,
                    control,
                    &mut old_inc_backend,
                    NopFallbackCompilation::NativeIncRax,
                )
                .unwrap(),
            );
        };
        let mut new_inc = || {
            let mut state = LongModeState {
                rip: inc_view.base_rip(),
                ..LongModeState::default()
            };
            black_box(
                run_register_dispatch(
                    inc_view,
                    RegisterDispatchProfile::NativeIncRax(&inc_admission),
                    RegisterFlagsMode::Eager,
                    &mut state,
                    2,
                    control,
                    UncachedRegisterPath(&mut new_inc_backend),
                )
                .unwrap(),
            );
        };
        let mut old_cached = || {
            let mut state = LongModeState {
                rip: inc_view.base_rip(),
                ..LongModeState::default()
            };
            let mut observer = NoopObserver;
            black_box(
                run_cached_native_register_v1_strategy_controlled::<false, false, _, _>(
                    inc_view,
                    &union_admission,
                    context,
                    &dependencies,
                    &mut state,
                    2,
                    control,
                    &mut old_cache,
                    &mut old_cache_backend,
                    &mut observer,
                    NativeRegisterV1CacheStrategy::DemandFlagsCandidate,
                    None,
                )
                .unwrap(),
            );
        };
        let mut new_cached = || {
            let mut state = LongModeState {
                rip: inc_view.base_rip(),
                ..LongModeState::default()
            };
            black_box(
                run_register_dispatch(
                    inc_view,
                    &union_admission,
                    RegisterFlagsMode::DemandFlagsCandidate,
                    &mut state,
                    2,
                    control,
                    SingleEntryRegisterPath {
                        execution_context: context,
                        dependencies: &dependencies,
                        cache: &mut new_cache,
                        backend: &mut new_cache_backend,
                    },
                )
                .unwrap(),
            );
        };
        // Warm the binary, code paths, and both single-entry caches before timing.
        old_nop();
        new_nop();
        old_inc();
        new_inc();
        old_cached();
        new_cached();
        for round in 0..9 {
            paired!("nop", round, 10_000, old_nop, new_nop);
            paired!("uncached_inc", round, 250, old_inc, new_inc);
            paired!("cached_demand", round, 1_000, old_cached, new_cached);
        }
    }
}
