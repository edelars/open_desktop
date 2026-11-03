//! Capacity-one retained cache for the exact bounded memory-pair fixtures.
//!
//! This is separate from `MemoryBlockExecutor`: its opaque backend handle
//! remains owned between synchronous calls and must retain code plus immutable
//! artifact metadata only.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::{collections::HashMap, num::NonZeroU64};

use od_isa::{
    LongModeState,
    memory::{AccessContext, GuestMemory},
};

use crate::{
    block::CodeView,
    memory_dispatch::{BoundedMemoryExit, BoundedMemoryResult, MemoryBlockExit, MemoryBlockReport},
    memory_load_v2::{
        IrMemoryTwoSiteBlockV2, MemoryLoadCompileErrorV2, compile_ir_memory_two_site_block_v2,
    },
    translation::CodeDependencySpan,
};

/// Logical per-entry ceiling, matching the S3-18 one-entry fixture bound.
/// Allocator/page overhead and opaque host-handle internals are excluded.
pub const MAX_MEMORY_PAIR_CACHE_BYTES: usize = 512 * 1024;
const MAX_MEMORY_PAIR_DEPENDENCIES: usize = 6;

/// The exact memory profiles admitted by this fixture cache.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MemoryPairProfile {
    StoreThenLoad,
    LoadThenStore,
    LoadThenLoad,
}

impl MemoryPairProfile {
    pub const fn source_bytes(self) -> [u8; 6] {
        match self {
            Self::StoreThenLoad => [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b],
            Self::LoadThenStore => [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02],
            Self::LoadThenLoad => [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b],
        }
    }
}

/// Nonzero caller-owned generation for a frozen fixture execution context.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MemoryExecutionContextGeneration(NonZeroU64);

impl MemoryExecutionContextGeneration {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn from_u64(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Full identity of one admitted checked-memory fixture artifact.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MemoryPairCacheKey {
    profile: MemoryPairProfile,
    context_generation: MemoryExecutionContextGeneration,
    code_base: u64,
    code_end: u64,
    entry_rip: u64,
    source: [u8; 6],
    dependencies: Vec<CodeDependencySpan>,
}

impl MemoryPairCacheKey {
    pub fn new(
        profile: MemoryPairProfile,
        context_generation: MemoryExecutionContextGeneration,
        code: CodeView<'_>,
        dependencies: &[CodeDependencySpan],
    ) -> Result<Self, MemoryCacheKeyError> {
        let source: [u8; 6] = code
            .bytes()
            .try_into()
            .map_err(|_| MemoryCacheKeyError::InvalidSource)?;
        if source != profile.source_bytes() {
            return Err(MemoryCacheKeyError::ProfileSourceMismatch);
        }
        if code.code_end().checked_sub(code.base_rip()) != Some(6) {
            return Err(MemoryCacheKeyError::InvalidCodeView);
        }
        let dependencies = validate_dependencies(code, dependencies)?;
        Ok(Self {
            profile,
            context_generation,
            code_base: code.base_rip(),
            code_end: code.code_end(),
            entry_rip: code.base_rip(),
            source,
            dependencies,
        })
    }

    pub const fn profile(&self) -> MemoryPairProfile {
        self.profile
    }

    pub const fn context_generation(&self) -> MemoryExecutionContextGeneration {
        self.context_generation
    }

    fn logical_bytes(&self, artifact: &IrMemoryTwoSiteBlockV2) -> Option<usize> {
        let code_bytes = artifact
            .code()
            .words()
            .len()
            .checked_mul(size_of::<u32>())?;
        let map_bytes = artifact
            .pc_map()
            .ranges()
            .len()
            .checked_mul(size_of::<crate::statepoints::StatePointRange>())?
            .checked_add(
                artifact
                    .pc_map()
                    .boundaries()
                    .len()
                    .checked_mul(size_of::<crate::statepoints::GuestBoundaryPoint>())?,
            )?;
        let key_bytes = self
            .source
            .len()
            .checked_add(
                self.dependencies
                    .len()
                    .checked_mul(size_of::<CodeDependencySpan>())?,
            )?
            .checked_add(size_of::<Self>())?;
        let artifact_bytes = code_bytes.checked_add(map_bytes)?;
        key_bytes.checked_add(artifact_bytes.checked_mul(2)?)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryCacheKeyError {
    InvalidSource,
    ProfileSourceMismatch,
    InvalidCodeView,
    NoDependencies,
    TooManyDependencies {
        actual: usize,
        max: usize,
    },
    DependencyOutsideCode {
        index: usize,
    },
    DependencyOverlap {
        previous_end: u64,
        next_start: u64,
    },
    NonExecutableDependency {
        index: usize,
    },
    DependencyGap {
        index: usize,
        expected: u64,
        actual: u64,
    },
    InconsistentMappingSnapshot {
        index: usize,
    },
    InconsistentBackingVersion {
        index: usize,
    },
    DependencyCoverageIncomplete {
        expected: u64,
        actual: u64,
    },
}

fn validate_dependencies(
    code: CodeView<'_>,
    dependencies: &[CodeDependencySpan],
) -> Result<Vec<CodeDependencySpan>, MemoryCacheKeyError> {
    if dependencies.is_empty() {
        return Err(MemoryCacheKeyError::NoDependencies);
    }
    if dependencies.len() > MAX_MEMORY_PAIR_DEPENDENCIES {
        return Err(MemoryCacheKeyError::TooManyDependencies {
            actual: dependencies.len(),
            max: MAX_MEMORY_PAIR_DEPENDENCIES,
        });
    }
    let mut ordered = dependencies.to_vec();
    ordered.sort_by_key(|span| (span.start_rip, span.end_rip));
    let mut mapping_snapshots = HashMap::new();
    let mut backing_versions = HashMap::new();
    let mut next = code.base_rip();
    for (index, span) in ordered.iter().enumerate() {
        if span.start_rip < code.base_rip()
            || span.end_rip > code.code_end()
            || span.start_rip >= span.end_rip
        {
            return Err(MemoryCacheKeyError::DependencyOutsideCode { index });
        }
        if !span.identity.executable {
            return Err(MemoryCacheKeyError::NonExecutableDependency { index });
        }
        if span.start_rip < next {
            return Err(MemoryCacheKeyError::DependencyOverlap {
                previous_end: next,
                next_start: span.start_rip,
            });
        }
        if span.start_rip > next {
            return Err(MemoryCacheKeyError::DependencyGap {
                index,
                expected: next,
                actual: span.start_rip,
            });
        }
        let mapping_key = (span.identity.address_space, span.identity.mapping);
        let mapping_snapshot = (
            span.identity.mapping_version,
            span.identity.backing,
            span.identity.backing_version,
            span.identity.executable,
        );
        if mapping_snapshots
            .insert(mapping_key, mapping_snapshot)
            .is_some_and(|previous| previous != mapping_snapshot)
        {
            return Err(MemoryCacheKeyError::InconsistentMappingSnapshot { index });
        }
        if backing_versions
            .insert(span.identity.backing, span.identity.backing_version)
            .is_some_and(|previous| previous != span.identity.backing_version)
        {
            return Err(MemoryCacheKeyError::InconsistentBackingVersion { index });
        }
        next = span.end_rip;
    }
    if next != code.code_end() {
        return Err(MemoryCacheKeyError::DependencyCoverageIncomplete {
            expected: code.code_end(),
            actual: next,
        });
    }
    Ok(ordered)
}

/// Typed boundary for retained memory-entry publication and execution.
/// `Published` is the opaque result of publishing the exact artifact and is
/// the only compiled identity accepted by execution; this makes pairing that
/// handle with a different artifact unrepresentable. Implementations must not
/// retain invocation-local pointers in `Published`.
pub trait CachedMemoryPairBackend {
    type Error;
    type Published;

    fn publish_memory_pair(
        &mut self,
        block: &IrMemoryTwoSiteBlockV2,
    ) -> Result<Self::Published, Self::Error>;

    /// # Safety
    /// The same immutable non-aliasing code and private synchronous ordinary
    /// RAM requirements as `memory_dispatch::run_controlled` apply. In
    /// particular, no MMIO, DMA, concurrency, re-entry or unwind is allowed,
    /// and writes must be all-or-nothing. The published handle contains no
    /// per-invocation state, RAM or control pointers.
    unsafe fn execute_memory_pair(
        &mut self,
        published: &Self::Published,
        state: &mut LongModeState,
        memory: &mut dyn GuestMemory,
        access_context: AccessContext,
        incoming_budget: u64,
        stop_rip: Option<u64>,
        stop_requested: &AtomicU8,
        event_requested: Option<&AtomicBool>,
    ) -> Result<MemoryBlockReport, Self::Error>;
}

struct MemoryPairCacheEntry<H> {
    key: MemoryPairCacheKey,
    // Retain compiler metadata with its published handle, but never pass it
    // separately to execution where a different handle could be paired.
    _artifact: IrMemoryTwoSiteBlockV2,
    published: H,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryPairCacheCounters {
    pub hits: u64,
    pub misses: u64,
    pub successful_compiles: u64,
    pub successful_publications: u64,
    pub replacements: u64,
    pub resident_entries_high_water: usize,
    pub resident_logical_bytes_high_water: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryCacheError {
    Key(MemoryCacheKeyError),
    Compile(MemoryLoadCompileErrorV2),
    EntryTooLarge {
        actual: usize,
        max: usize,
    },
    SizeOverflow,
    ContextGenerationNotAdvanced {
        current: u64,
        requested: u64,
    },
    ProfileChangedWithoutGeneration,
    CounterOverflow,
    InvalidAccounting {
        incoming: u64,
        report: MemoryBlockReport,
    },
    InvalidProgress {
        rip: u64,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum MemoryPairCacheRunError<E> {
    Cache(MemoryCacheError),
    Backend(E),
}

/// One resident compiled/published two-site artifact. `&mut self` ensures
/// replacement and invalidation happen only between synchronous calls.
pub struct MemoryPairCache<H> {
    entry: Option<MemoryPairCacheEntry<H>>,
    context: Option<(MemoryPairProfile, MemoryExecutionContextGeneration)>,
    counters: MemoryPairCacheCounters,
}

impl<H> Default for MemoryPairCache<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> MemoryPairCache<H> {
    pub const fn new() -> Self {
        Self {
            entry: None,
            context: None,
            counters: MemoryPairCacheCounters {
                hits: 0,
                misses: 0,
                successful_compiles: 0,
                successful_publications: 0,
                replacements: 0,
                resident_entries_high_water: 0,
                resident_logical_bytes_high_water: 0,
            },
        }
    }

    pub const fn counters(&self) -> MemoryPairCacheCounters {
        self.counters
    }

    pub const fn resident_entries(&self) -> usize {
        if self.entry.is_some() { 1 } else { 0 }
    }

    pub fn clear(&mut self) {
        self.entry = None;
    }

    fn add_counter(counter: &mut u64) -> Result<(), MemoryCacheError> {
        *counter = counter
            .checked_add(1)
            .ok_or(MemoryCacheError::CounterOverflow)?;
        Ok(())
    }

    fn bind_context(
        &mut self,
        profile: MemoryPairProfile,
        generation: MemoryExecutionContextGeneration,
    ) -> Result<(), MemoryCacheError> {
        let Some((current_profile, current_generation)) = self.context else {
            self.context = Some((profile, generation));
            return Ok(());
        };
        if (current_profile, current_generation) == (profile, generation) {
            return Ok(());
        }
        if generation <= current_generation {
            return Err(if generation == current_generation {
                MemoryCacheError::ProfileChangedWithoutGeneration
            } else {
                MemoryCacheError::ContextGenerationNotAdvanced {
                    current: current_generation.get(),
                    requested: generation.get(),
                }
            });
        }
        self.clear();
        self.context = Some((profile, generation));
        Ok(())
    }

    fn ensure_entry<B>(
        &mut self,
        key: MemoryPairCacheKey,
        code: CodeView<'_>,
        backend: &mut B,
    ) -> Result<(), MemoryPairCacheRunError<B::Error>>
    where
        B: CachedMemoryPairBackend<Published = H>,
    {
        if self.entry.as_ref().is_some_and(|entry| entry.key == key) {
            Self::add_counter(&mut self.counters.hits).map_err(MemoryPairCacheRunError::Cache)?;
            return Ok(());
        }
        Self::add_counter(&mut self.counters.misses).map_err(MemoryPairCacheRunError::Cache)?;
        let artifact = compile_ir_memory_two_site_block_v2(code, key.entry_rip)
            .map_err(MemoryCacheError::Compile)
            .map_err(MemoryPairCacheRunError::Cache)?;
        if artifact.sequence()
            != match key.profile {
                MemoryPairProfile::StoreThenLoad => {
                    crate::runtime::MemoryPairSequenceV2::StoreThenLoad
                }
                MemoryPairProfile::LoadThenStore => {
                    crate::runtime::MemoryPairSequenceV2::LoadThenStore
                }
                MemoryPairProfile::LoadThenLoad => {
                    crate::runtime::MemoryPairSequenceV2::LoadThenLoad
                }
            }
        {
            return Err(MemoryPairCacheRunError::Cache(MemoryCacheError::Key(
                MemoryCacheKeyError::ProfileSourceMismatch,
            )));
        }
        Self::add_counter(&mut self.counters.successful_compiles)
            .map_err(MemoryPairCacheRunError::Cache)?;
        let logical_bytes = key
            .logical_bytes(&artifact)
            .ok_or(MemoryPairCacheRunError::Cache(
                MemoryCacheError::SizeOverflow,
            ))?;
        if logical_bytes > MAX_MEMORY_PAIR_CACHE_BYTES {
            return Err(MemoryPairCacheRunError::Cache(
                MemoryCacheError::EntryTooLarge {
                    actual: logical_bytes,
                    max: MAX_MEMORY_PAIR_CACHE_BYTES,
                },
            ));
        }
        if self.entry.is_some() {
            Self::add_counter(&mut self.counters.replacements)
                .map_err(MemoryPairCacheRunError::Cache)?;
        }
        // The host uses a single-live-MAP_JIT lease: release the prior mapping
        // before requesting publication of its replacement.
        self.clear();
        let published = backend
            .publish_memory_pair(&artifact)
            .map_err(MemoryPairCacheRunError::Backend)?;
        Self::add_counter(&mut self.counters.successful_publications)
            .map_err(MemoryPairCacheRunError::Cache)?;
        self.counters.resident_entries_high_water = 1;
        self.counters.resident_logical_bytes_high_water = self
            .counters
            .resident_logical_bytes_high_water
            .max(logical_bytes);
        self.entry = Some(MemoryPairCacheEntry {
            key,
            _artifact: artifact,
            published,
        });
        Ok(())
    }

    /// Run one exact admitted two-site profile through the resident capacity-one
    /// mapping. Other entry RIPs/forms are outside this opt-in API.
    ///
    /// # Safety
    /// The immutable source and private synchronous ordinary-RAM preconditions
    /// documented by `memory_dispatch::run_controlled` apply to this call.
    pub unsafe fn run<B>(
        &mut self,
        profile: MemoryPairProfile,
        context_generation: MemoryExecutionContextGeneration,
        code: CodeView<'_>,
        dependencies: &[CodeDependencySpan],
        state: &mut LongModeState,
        memory: &mut dyn GuestMemory,
        access_context: AccessContext,
        total_budget: u64,
        stop_rip: Option<u64>,
        stop_requested: &AtomicU8,
        event_requested: Option<&AtomicBool>,
        backend: &mut B,
    ) -> Result<BoundedMemoryResult, MemoryPairCacheRunError<B::Error>>
    where
        B: CachedMemoryPairBackend<Published = H>,
    {
        let code_end = code.code_end();
        if state.rip < code.base_rip() || state.rip > code_end {
            return Err(MemoryPairCacheRunError::Cache(MemoryCacheError::Key(
                MemoryCacheKeyError::InvalidCodeView,
            )));
        }
        if stop_rip == Some(state.rip) {
            return Ok(memory_result(
                BoundedMemoryExit::StopAddress,
                total_budget,
                0,
                0,
            ));
        }
        if stop_requested.load(Ordering::Acquire) != 0 {
            return Ok(memory_result(
                BoundedMemoryExit::StopRequested,
                total_budget,
                0,
                0,
            ));
        }
        if state.rip == code_end {
            return Ok(memory_result(
                BoundedMemoryExit::BoundaryReached { rip: state.rip },
                total_budget,
                0,
                0,
            ));
        }
        if total_budget == 0 {
            return Ok(memory_result(BoundedMemoryExit::BudgetExhausted, 0, 0, 0));
        }
        if event_requested.is_some_and(|event| event.load(Ordering::Acquire)) {
            return Ok(memory_result(
                BoundedMemoryExit::EventPending,
                total_budget,
                0,
                0,
            ));
        }
        if state.rip != code.base_rip() {
            return Err(MemoryPairCacheRunError::Cache(MemoryCacheError::Key(
                MemoryCacheKeyError::InvalidCodeView,
            )));
        }
        self.bind_context(profile, context_generation)
            .map_err(MemoryPairCacheRunError::Cache)?;
        let key = MemoryPairCacheKey::new(profile, context_generation, code, dependencies)
            .map_err(MemoryCacheError::Key)
            .map_err(MemoryPairCacheRunError::Cache)?;
        self.ensure_entry(key, code, backend)?;
        let entry = self.entry.as_ref().expect("entry installed");
        // Re-sample invocation-local guards immediately before entry. The
        // backend repeats this check after publication on a cache miss.
        if stop_rip == Some(state.rip) {
            return Ok(memory_result(
                BoundedMemoryExit::StopAddress,
                total_budget,
                0,
                0,
            ));
        }
        if stop_requested.load(Ordering::Acquire) != 0 {
            return Ok(memory_result(
                BoundedMemoryExit::StopRequested,
                total_budget,
                0,
                0,
            ));
        }
        if total_budget == 0 {
            return Ok(memory_result(BoundedMemoryExit::BudgetExhausted, 0, 0, 0));
        }
        if event_requested.is_some_and(|event| event.load(Ordering::Acquire)) {
            return Ok(memory_result(
                BoundedMemoryExit::EventPending,
                total_budget,
                0,
                0,
            ));
        }
        let prior_rip = state.rip;
        // SAFETY: caller guarantees the private synchronous ordinary-RAM
        // contract and this retained handle contains no invocation pointers.
        let execution = unsafe {
            backend.execute_memory_pair(
                &entry.published,
                state,
                memory,
                access_context,
                total_budget,
                stop_rip,
                stop_requested,
                event_requested,
            )
        };
        let report = match execution {
            Ok(report) => report,
            Err(error) => {
                // The backend has returned to a synchronous safe point; release
                // the suspect published mapping before surfacing the error.
                self.clear();
                return Err(MemoryPairCacheRunError::Backend(error));
            }
        };
        if let Err(error) = validate_report(
            &report,
            prior_rip,
            code.base_rip(),
            code_end,
            total_budget,
            state,
            stop_rip,
            stop_requested,
            event_requested,
        ) {
            self.clear();
            return Err(MemoryPairCacheRunError::Cache(error));
        }
        let mut exit = map_block_exit(report.exit.clone(), state.rip, code_end, event_requested);
        if report.exit == MemoryBlockExit::EventPending {
            exit = if stop_rip == Some(state.rip) {
                BoundedMemoryExit::StopAddress
            } else if stop_requested.load(Ordering::Acquire) != 0 {
                BoundedMemoryExit::StopRequested
            } else if state.rip == code_end {
                BoundedMemoryExit::BoundaryReached { rip: state.rip }
            } else if report.remaining_budget == 0 {
                BoundedMemoryExit::BudgetExhausted
            } else {
                BoundedMemoryExit::EventPending
            };
        }
        let mut result = memory_result(
            exit,
            report.remaining_budget,
            report.retired_instructions,
            u64::from(report.exit != MemoryBlockExit::EventPending),
        );
        if report.exit == MemoryBlockExit::Complete {
            if stop_rip == Some(state.rip) {
                result.exit = BoundedMemoryExit::StopAddress;
            } else if stop_requested.load(Ordering::Acquire) != 0 {
                result.exit = BoundedMemoryExit::StopRequested;
            } else if state.rip == code_end {
                result.exit = BoundedMemoryExit::BoundaryReached { rip: state.rip };
            } else if report.remaining_budget == 0 {
                result.exit = BoundedMemoryExit::BudgetExhausted;
            } else if event_requested.is_some_and(|event| event.load(Ordering::Acquire)) {
                result.exit = BoundedMemoryExit::EventPending;
            }
        }
        Ok(result)
    }
}

fn memory_result(
    exit: BoundedMemoryExit,
    remaining_budget: u64,
    retired_instructions: u64,
    blocks_executed: u64,
) -> BoundedMemoryResult {
    BoundedMemoryResult {
        exit,
        remaining_budget,
        retired_instructions,
        blocks_executed,
    }
}

fn map_block_exit(
    exit: MemoryBlockExit,
    rip: u64,
    code_end: u64,
    event_requested: Option<&AtomicBool>,
) -> BoundedMemoryExit {
    match exit {
        MemoryBlockExit::Complete => BoundedMemoryExit::BoundaryReached { rip: code_end },
        MemoryBlockExit::CodeInvalidated => BoundedMemoryExit::CodeInvalidated,
        MemoryBlockExit::BudgetExhausted => BoundedMemoryExit::BudgetExhausted,
        MemoryBlockExit::StopRequested => BoundedMemoryExit::StopRequested,
        MemoryBlockExit::StopAddress => BoundedMemoryExit::StopAddress,
        MemoryBlockExit::EventPending => {
            if event_requested.is_some_and(|event| event.load(Ordering::Acquire)) {
                BoundedMemoryExit::EventPending
            } else {
                BoundedMemoryExit::Invalid { rip }
            }
        }
        MemoryBlockExit::GuestFault(fault) => BoundedMemoryExit::GuestFault(fault),
    }
}

fn validate_report(
    report: &MemoryBlockReport,
    prior_rip: u64,
    code_base: u64,
    code_end: u64,
    incoming_budget: u64,
    state: &LongModeState,
    stop_rip: Option<u64>,
    stop_requested: &AtomicU8,
    event_requested: Option<&AtomicBool>,
) -> Result<(), MemoryCacheError> {
    if report.retired_instructions > 2
        || report.retired_instructions > incoming_budget
        || incoming_budget - report.retired_instructions != report.remaining_budget
    {
        return Err(MemoryCacheError::InvalidAccounting {
            incoming: incoming_budget,
            report: report.clone(),
        });
    }
    if state.rip < prior_rip || state.rip > code_end {
        return Err(MemoryCacheError::InvalidProgress { rip: state.rip });
    }
    if report.exit == MemoryBlockExit::EventPending
        && (report.retired_instructions != 0
            || report.remaining_budget != incoming_budget
            || state.rip != prior_rip
            || !event_requested.is_some_and(|event| event.load(Ordering::Acquire)))
    {
        return Err(MemoryCacheError::InvalidAccounting {
            incoming: incoming_budget,
            report: report.clone(),
        });
    }
    if report.exit == MemoryBlockExit::Complete
        && (report.retired_instructions != 2 || state.rip != code_end)
    {
        return Err(MemoryCacheError::InvalidProgress { rip: state.rip });
    }
    if report.exit == MemoryBlockExit::BudgetExhausted && report.remaining_budget != 0 {
        return Err(MemoryCacheError::InvalidAccounting {
            incoming: incoming_budget,
            report: report.clone(),
        });
    }
    let expected_rip = code_base
        .checked_add(report.retired_instructions * 3)
        .ok_or(MemoryCacheError::InvalidProgress { rip: state.rip })?;
    if state.rip != expected_rip {
        return Err(MemoryCacheError::InvalidProgress { rip: state.rip });
    }
    match &report.exit {
        MemoryBlockExit::Complete => {}
        MemoryBlockExit::CodeInvalidated => {}
        MemoryBlockExit::BudgetExhausted if report.remaining_budget == 0 => {}
        MemoryBlockExit::StopAddress
            if stop_rip == Some(state.rip)
                || (report.retired_instructions == 0 && stop_rip == Some(prior_rip)) => {}
        MemoryBlockExit::StopRequested if stop_requested.load(Ordering::Acquire) != 0 => {}
        MemoryBlockExit::EventPending => {}
        MemoryBlockExit::GuestFault(_) => {}
        _ => {
            return Err(MemoryCacheError::InvalidProgress { rip: state.rip });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, num::NonZeroU64, rc::Rc};

    use od_isa::memory::{AccessKind, MemoryError};

    use crate::translation::{
        AddressSpaceId, BackingId, BackingVersion, CodeIdentity, MappingId, MappingVersion,
    };

    const BASE: u64 = 0x1000;

    fn context(value: u64) -> MemoryExecutionContextGeneration {
        MemoryExecutionContextGeneration::from_u64(value).unwrap()
    }

    fn identity(mapping: u64, backing_version: u64) -> CodeIdentity {
        CodeIdentity {
            address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
            mapping: MappingId::new(NonZeroU64::new(mapping).unwrap()),
            mapping_version: MappingVersion::new(NonZeroU64::new(1).unwrap()),
            backing: BackingId::new(NonZeroU64::new(1).unwrap()),
            backing_version: BackingVersion::new(NonZeroU64::new(backing_version).unwrap()),
            executable: true,
        }
    }

    fn key(
        profile: MemoryPairProfile,
        generation: u64,
        base: u64,
        mapping: u64,
        backing_version: u64,
    ) -> (MemoryPairCacheKey, CodeView<'static>) {
        key_with_identity(
            profile,
            generation,
            base,
            identity(mapping, backing_version),
        )
    }

    fn key_with_identity(
        profile: MemoryPairProfile,
        generation: u64,
        base: u64,
        source_identity: CodeIdentity,
    ) -> (MemoryPairCacheKey, CodeView<'static>) {
        let bytes: &'static [u8; 6] = match profile {
            MemoryPairProfile::StoreThenLoad => &[0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b],
            MemoryPairProfile::LoadThenStore => &[0x48, 0x8b, 0x03, 0x48, 0x89, 0x02],
            MemoryPairProfile::LoadThenLoad => &[0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b],
        };
        let code = CodeView::new(base, bytes).unwrap();
        let dependencies = [crate::translation::CodeDependencySpan {
            start_rip: base,
            end_rip: base + 6,
            identity: source_identity,
        }];
        (
            MemoryPairCacheKey::new(profile, context(generation), code, &dependencies).unwrap(),
            code,
        )
    }

    struct MockPublished(Rc<Cell<usize>>);

    impl Drop for MockPublished {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    #[derive(Default)]
    struct MockBackend {
        live: Rc<Cell<usize>>,
        publishes: usize,
        fail_publish: bool,
        event_on_publish: Option<Rc<AtomicBool>>,
        executes: usize,
    }

    impl CachedMemoryPairBackend for MockBackend {
        type Error = &'static str;
        type Published = MockPublished;

        fn publish_memory_pair(
            &mut self,
            _: &IrMemoryTwoSiteBlockV2,
        ) -> Result<Self::Published, Self::Error> {
            assert_eq!(
                self.live.get(),
                0,
                "old mapping must drop before replacement"
            );
            self.publishes += 1;
            if let Some(event) = &self.event_on_publish {
                event.store(true, Ordering::Release);
            }
            if self.fail_publish {
                return Err("publish failed");
            }
            self.live.set(1);
            Ok(MockPublished(self.live.clone()))
        }

        unsafe fn execute_memory_pair(
            &mut self,
            _: &Self::Published,
            _: &mut LongModeState,
            _: &mut dyn GuestMemory,
            _: AccessContext,
            _: u64,
            _: Option<u64>,
            _: &AtomicU8,
            _: Option<&AtomicBool>,
        ) -> Result<MemoryBlockReport, Self::Error> {
            self.executes += 1;
            Err("unexpected mock execution")
        }
    }

    struct UnusedMemory;

    impl GuestMemory for UnusedMemory {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
        }
        fn read(
            &mut self,
            address: u64,
            _: AccessContext,
            _: &mut [u8],
        ) -> Result<(), MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })
        }
        fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
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

    #[test]
    fn profiles_admit_only_the_exact_load_store_sequences() {
        assert_eq!(
            MemoryPairProfile::StoreThenLoad.source_bytes(),
            [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b]
        );
        assert_eq!(
            MemoryPairProfile::LoadThenStore.source_bytes(),
            [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02]
        );
        assert_eq!(
            MemoryPairProfile::LoadThenLoad.source_bytes(),
            [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b]
        );
        let code = [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b];
        assert_eq!(
            MemoryPairCacheKey::new(
                MemoryPairProfile::LoadThenStore,
                MemoryExecutionContextGeneration::from_u64(1).unwrap(),
                CodeView::new(0x1000, &code).unwrap(),
                &[],
            ),
            Err(MemoryCacheKeyError::ProfileSourceMismatch)
        );
        let store_then_store = [0x48, 0x89, 0x03, 0x48, 0x89, 0x02];
        assert_eq!(
            MemoryPairCacheKey::new(
                MemoryPairProfile::LoadThenLoad,
                context(1),
                CodeView::new(0x1000, &store_then_store).unwrap(),
                &[],
            ),
            Err(MemoryCacheKeyError::ProfileSourceMismatch)
        );
    }

    #[test]
    fn internal_second_site_rip_remains_rejected_for_load_pair() {
        let profile = MemoryPairProfile::LoadThenLoad;
        let (cache_key, code) = key(profile, 1, BASE, 1, 1);
        let dependencies = cache_key.dependencies.clone();
        let mut cache = MemoryPairCache::<MockPublished>::new();
        let mut backend = MockBackend::default();
        let mut memory = UnusedMemory;
        let mut state = LongModeState {
            rip: BASE + 3,
            ..Default::default()
        };
        let before = state.clone();
        let stop = AtomicU8::new(0);
        // SAFETY: the interior RIP is rejected before publication or execution.
        let result = unsafe {
            cache.run(
                profile,
                context(1),
                code,
                &dependencies,
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop,
                None,
                &mut backend,
            )
        };
        assert_eq!(
            result,
            Err(MemoryPairCacheRunError::Cache(MemoryCacheError::Key(
                MemoryCacheKeyError::InvalidCodeView
            )))
        );
        assert_eq!(state, before);
        assert_eq!(backend.publishes, 0);
        assert_eq!(backend.executes, 0);
        assert_eq!(cache.resident_entries(), 0);
    }

    #[test]
    fn same_key_hits_and_replacement_drops_before_publish() {
        let (first, code) = key(MemoryPairProfile::StoreThenLoad, 1, BASE, 1, 1);
        let (changed, changed_code) = key(MemoryPairProfile::StoreThenLoad, 1, BASE + 0x100, 1, 1);
        let mut cache = MemoryPairCache::new();
        let mut backend = MockBackend::default();

        cache
            .ensure_entry(first.clone(), code, &mut backend)
            .unwrap();
        cache.ensure_entry(first, code, &mut backend).unwrap();
        assert_eq!(cache.counters().hits, 1);
        assert_eq!(cache.counters().misses, 1);
        assert_eq!(backend.publishes, 1);
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(backend.live.get(), 1);

        cache
            .ensure_entry(changed, changed_code, &mut backend)
            .unwrap();
        assert_eq!(cache.counters().replacements, 1);
        assert_eq!(backend.publishes, 2);
        assert_eq!(backend.live.get(), 1);
        cache.clear();
        assert_eq!(backend.live.get(), 0);
    }

    #[test]
    fn dependency_and_context_changes_do_not_reuse_an_entry() {
        let (first, code) = key(MemoryPairProfile::StoreThenLoad, 1, BASE, 1, 1);
        let (dependency_changed, _) = key(MemoryPairProfile::StoreThenLoad, 1, BASE, 2, 1);
        let (context_changed, _) = key(MemoryPairProfile::StoreThenLoad, 2, BASE, 2, 1);
        let mut cache = MemoryPairCache::new();
        let mut backend = MockBackend::default();
        cache.ensure_entry(first, code, &mut backend).unwrap();
        cache
            .ensure_entry(dependency_changed.clone(), code, &mut backend)
            .unwrap();
        cache
            .bind_context(
                MemoryPairProfile::StoreThenLoad,
                context_changed.context_generation(),
            )
            .unwrap();
        cache
            .ensure_entry(context_changed, code, &mut backend)
            .unwrap();
        assert_eq!(backend.publishes, 3);
        assert_eq!(cache.counters().misses, 3);
        assert_eq!(backend.live.get(), 1);
    }

    #[test]
    fn key_covers_each_dependency_identity_generation_and_source_boundary() {
        let profile = MemoryPairProfile::StoreThenLoad;
        let (original, code) = key(profile, 1, BASE, 1, 1);
        assert_eq!(original.entry_rip, BASE);
        assert_eq!(original.code_base, BASE);
        assert_eq!(original.code_end, BASE + 6);
        assert_eq!(original.source, profile.source_bytes());

        let variants = [
            CodeIdentity {
                address_space: AddressSpaceId::new(NonZeroU64::new(2).unwrap()),
                ..identity(1, 1)
            },
            CodeIdentity {
                mapping: MappingId::new(NonZeroU64::new(2).unwrap()),
                ..identity(1, 1)
            },
            CodeIdentity {
                mapping_version: MappingVersion::new(NonZeroU64::new(2).unwrap()),
                ..identity(1, 1)
            },
            CodeIdentity {
                backing: BackingId::new(NonZeroU64::new(2).unwrap()),
                ..identity(1, 1)
            },
            CodeIdentity {
                backing_version: BackingVersion::new(NonZeroU64::new(2).unwrap()),
                ..identity(1, 1)
            },
        ];
        for variant in variants {
            let (changed, _) = key_with_identity(profile, 1, BASE, variant);
            assert_ne!(changed, original);
        }

        let (changed_base, _) = key(profile, 1, BASE + 0x100, 1, 1);
        assert_ne!(changed_base, original);
        assert_eq!(changed_base.code_end, BASE + 0x106);
        assert_eq!(changed_base.entry_rip, BASE + 0x100);

        let non_executable = CodeIdentity {
            executable: false,
            ..identity(1, 1)
        };
        let bytes = profile.source_bytes();
        let non_executable_span = [crate::translation::CodeDependencySpan {
            start_rip: BASE,
            end_rip: BASE + 6,
            identity: non_executable,
        }];
        assert_eq!(
            MemoryPairCacheKey::new(profile, context(1), code, &non_executable_span),
            Err(MemoryCacheKeyError::NonExecutableDependency { index: 0 })
        );
        let unsupported_source = [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b];
        let unsupported_view = CodeView::new(BASE, &unsupported_source).unwrap();
        let valid_span = [crate::translation::CodeDependencySpan {
            start_rip: BASE,
            end_rip: BASE + 6,
            identity: identity(1, 1),
        }];
        assert_eq!(
            MemoryPairCacheKey::new(profile, context(1), unsupported_view, &valid_span),
            Err(MemoryCacheKeyError::ProfileSourceMismatch)
        );
        assert_eq!(bytes, profile.source_bytes());

        let split_spans = |first: CodeIdentity, second: CodeIdentity| {
            [
                crate::translation::CodeDependencySpan {
                    start_rip: BASE,
                    end_rip: BASE + 3,
                    identity: first,
                },
                crate::translation::CodeDependencySpan {
                    start_rip: BASE + 3,
                    end_rip: BASE + 6,
                    identity: second,
                },
            ]
        };
        let inconsistent_mapping = split_spans(
            identity(1, 1),
            CodeIdentity {
                mapping_version: MappingVersion::new(NonZeroU64::new(2).unwrap()),
                ..identity(1, 1)
            },
        );
        assert_eq!(
            MemoryPairCacheKey::new(profile, context(1), code, &inconsistent_mapping),
            Err(MemoryCacheKeyError::InconsistentMappingSnapshot { index: 1 })
        );
        let inconsistent_backing = split_spans(
            identity(1, 1),
            CodeIdentity {
                mapping: MappingId::new(NonZeroU64::new(2).unwrap()),
                backing_version: BackingVersion::new(NonZeroU64::new(2).unwrap()),
                ..identity(1, 1)
            },
        );
        assert_eq!(
            MemoryPairCacheKey::new(profile, context(1), code, &inconsistent_backing),
            Err(MemoryCacheKeyError::InconsistentBackingVersion { index: 1 })
        );
    }

    #[test]
    fn failed_replacement_publication_leaves_cache_empty() {
        let (first, code) = key(MemoryPairProfile::StoreThenLoad, 1, BASE, 1, 1);
        let (second, second_code) = key(MemoryPairProfile::StoreThenLoad, 1, BASE + 0x100, 1, 1);
        let mut cache = MemoryPairCache::new();
        let mut backend = MockBackend::default();
        cache.ensure_entry(first, code, &mut backend).unwrap();
        backend.fail_publish = true;
        assert_eq!(
            cache.ensure_entry(second, second_code, &mut backend),
            Err(MemoryPairCacheRunError::Backend("publish failed"))
        );
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(backend.live.get(), 0);
    }

    #[test]
    fn oversized_dependency_list_rejects_before_replacing_resident_entry() {
        let profile = MemoryPairProfile::StoreThenLoad;
        let (existing, code) = key(profile, 1, BASE, 1, 1);
        let mut cache = MemoryPairCache::<MockPublished>::new();
        let mut backend = MockBackend::default();
        cache
            .ensure_entry(existing.clone(), code, &mut backend)
            .unwrap();
        let oversized = vec![existing.dependencies[0]; MAX_MEMORY_PAIR_DEPENDENCIES + 1];
        let mut memory = UnusedMemory;
        let mut state = LongModeState {
            rip: BASE,
            ..Default::default()
        };
        let stop = AtomicU8::new(0);
        // SAFETY: invalid dependency count is rejected before backend execution.
        let result = unsafe {
            cache.run(
                profile,
                context(1),
                code,
                &oversized,
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop,
                None,
                &mut backend,
            )
        };
        assert_eq!(
            result,
            Err(MemoryPairCacheRunError::Cache(MemoryCacheError::Key(
                MemoryCacheKeyError::TooManyDependencies {
                    actual: MAX_MEMORY_PAIR_DEPENDENCIES + 1,
                    max: MAX_MEMORY_PAIR_DEPENDENCIES,
                }
            )))
        );
        assert_eq!(cache.resident_entries(), 1);
        assert_eq!(backend.live.get(), 1);
        assert_eq!(backend.publishes, 1);
    }

    #[test]
    fn entry_guards_do_not_publish_and_event_during_publish_does_not_execute() {
        let profile = MemoryPairProfile::StoreThenLoad;
        let (cache_key, code) = key(profile, 1, BASE, 1, 1);
        let dependencies = cache_key.dependencies.clone();
        let mut cache = MemoryPairCache::<MockPublished>::new();
        let mut backend = MockBackend::default();
        let mut memory = UnusedMemory;
        let mut state = LongModeState {
            rip: BASE,
            ..Default::default()
        };
        let stop = AtomicU8::new(1);
        // SAFETY: all calls return at pre-entry guards and never touch memory.
        let stopped = unsafe {
            cache.run(
                profile,
                context(1),
                code,
                &dependencies,
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop,
                None,
                &mut backend,
            )
        }
        .unwrap();
        assert_eq!(stopped.exit, BoundedMemoryExit::StopRequested);
        assert_eq!(backend.publishes, 0);
        stop.store(0, Ordering::Release);
        let no_budget = unsafe {
            cache.run(
                profile,
                context(1),
                code,
                &dependencies,
                &mut state,
                &mut memory,
                AccessContext::default(),
                0,
                None,
                &stop,
                None,
                &mut backend,
            )
        }
        .unwrap();
        assert_eq!(no_budget.exit, BoundedMemoryExit::BudgetExhausted);
        let stop_rip = unsafe {
            cache.run(
                profile,
                context(1),
                code,
                &dependencies,
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                Some(BASE),
                &stop,
                None,
                &mut backend,
            )
        }
        .unwrap();
        assert_eq!(stop_rip.exit, BoundedMemoryExit::StopAddress);
        let event = AtomicBool::new(true);
        let pending = unsafe {
            cache.run(
                profile,
                context(1),
                code,
                &dependencies,
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop,
                Some(&event),
                &mut backend,
            )
        }
        .unwrap();
        assert_eq!(pending.exit, BoundedMemoryExit::EventPending);
        assert_eq!(backend.publishes, 0);

        event.store(false, Ordering::Release);
        backend.event_on_publish = Some(Rc::new(AtomicBool::new(false)));
        let event_ref = backend.event_on_publish.as_ref().unwrap().clone();
        let pending = unsafe {
            cache.run(
                profile,
                context(1),
                code,
                &dependencies,
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop,
                Some(&event_ref),
                &mut backend,
            )
        }
        .unwrap();
        assert_eq!(pending.exit, BoundedMemoryExit::EventPending);
        assert_eq!(backend.publishes, 1);
        assert_eq!(backend.executes, 0);
        assert_eq!(backend.live.get(), 1);
    }

    #[test]
    fn executor_error_drops_entry_at_the_safe_return_boundary() {
        let profile = MemoryPairProfile::StoreThenLoad;
        let (cache_key, code) = key(profile, 1, BASE, 1, 1);
        let dependencies = cache_key.dependencies.clone();
        let mut cache = MemoryPairCache::<MockPublished>::new();
        let mut backend = MockBackend::default();
        let mut memory = UnusedMemory;
        let mut state = LongModeState {
            rip: BASE,
            ..Default::default()
        };
        let stop = AtomicU8::new(0);
        // SAFETY: the mock backend returns an error before touching state or
        // memory, and the fixture provides no actual memory access.
        let result = unsafe {
            cache.run(
                profile,
                context(1),
                code,
                &dependencies,
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop,
                None,
                &mut backend,
            )
        };
        assert_eq!(
            result,
            Err(MemoryPairCacheRunError::Backend(
                "unexpected mock execution"
            ))
        );
        assert_eq!(cache.resident_entries(), 0);
        assert_eq!(backend.live.get(), 0);
    }
}
