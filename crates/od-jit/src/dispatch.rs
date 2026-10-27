//! Iterative multi-block dispatch over a fully admitted immutable fixture.
//!
//! The executor boundary runs exactly one compiled block per call. A host
//! implementation can publish, execute, and drop that block before this loop
//! compiles or invokes the next one.

use od_isa::LongModeState;
use od_isa::execute::StepOutcome;
use od_isa::memory::{AccessContext, MemoryError};
use std::sync::atomic::{AtomicU8, Ordering};

use crate::block::{AdmittedInstructionBoundaries, CodeMemory, CodeView};
use crate::block_compile::{
    BlockCompileError, BlockCompileOutcome, build_dispatch_block, compile_block_for_dispatch,
    compile_block_for_nop_fallback, compile_prepared_dispatch_block,
};
use crate::runtime::RuntimeExitTag;
use crate::translation::{CodeDependencySpan, CompileFlavor, TranslationKey, TranslationKeyError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockExecutionReport {
    pub exit: RuntimeExitTag,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
}

pub use crate::block::S0_09NopAdmission as InterpreterFallbackAdmission;

/// One cooperative control context is shared by generated guards and
/// interpreter pre-step guards for the full dispatch invocation.
#[derive(Clone, Copy)]
pub struct DispatchControl<'a> {
    pub stop_rip: Option<u64>,
    pub stop_requested: &'a AtomicU8,
}

/// Executor contract for fallback dispatch. Implementations must pass this
/// exact stop address and atomic to the generated block invocation and release
/// the executable mapping before returning.
pub trait ControlledBlockExecutor {
    type Error;

    fn execute_one_block_controlled(
        &mut self,
        block: &crate::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<BlockExecutionReport, Self::Error>;
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

pub const MAX_SINGLE_ENTRY_CACHE_BYTES: usize = 512 * 1024;

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
}

struct CachedEntry<H> {
    key: TranslationKey,
    block: crate::block_compile::CompiledBlock,
    published: H,
    logical_bytes: usize,
}

/// Explicitly bounded one-entry cache. Published host memory stays alive only
/// while this slot owns its opaque handle. The byte limit counts key contents,
/// compiled code/state-map contents, and one equivalent publication copy;
/// host allocator bookkeeping, page rounding, and opaque handle internals are
/// excluded and are not a process-RSS bound.
pub struct SingleEntryBlockCache<H> {
    max_bytes: usize,
    entry: Option<CachedEntry<H>>,
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

    pub fn clear(&mut self) {
        self.entry = None;
    }

    fn key(&self) -> Option<&TranslationKey> {
        self.entry.as_ref().map(|entry| &entry.key)
    }

    fn cached_block(&self) -> Option<&crate::block_compile::CompiledBlock> {
        self.entry.as_ref().map(|entry| &entry.block)
    }

    fn published(&self) -> Option<&H> {
        self.entry.as_ref().map(|entry| &entry.published)
    }

    fn install(&mut self, entry: CachedEntry<H>) {
        self.entry = Some(entry);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchExit {
    CodeEnd,
    BudgetExhausted,
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
        dependencies,
        state,
        total_budget,
        DispatchControl {
            stop_rip: None,
            stop_requested: &no_stop,
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
    dependencies: &[CodeDependencySpan],
    state: &mut LongModeState,
    total_budget: u64,
    control: DispatchControl<'_>,
    cache: &mut SingleEntryBlockCache<B::Published>,
    backend: &mut B,
) -> Result<DispatchResult, DispatchError<B::Error>> {
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

        let built =
            build_dispatch_block(code, state.rip, admission).map_err(DispatchError::Compile)?;
        let guest_block = match &built {
            crate::block::BuildOutcome::Block(block) => block,
            crate::block::BuildOutcome::NoBlock(no_block) => {
                return Err(DispatchError::NoNativePrefix {
                    rip: no_block.entry_rip,
                });
            }
        };
        let minimum = minimum_cache_key_size::<B::Published>(code, dependencies, guest_block)
            .ok_or(DispatchError::Cache(CacheError::SizeOverflow))?;
        if minimum > cache.max_bytes() {
            return Err(DispatchError::Cache(CacheError::MinimumEntryTooLarge {
                minimum,
                max: cache.max_bytes(),
            }));
        }
        let key = TranslationKey::for_block(
            code,
            guest_block,
            CompileFlavor::S0_09FixtureDispatch,
            dependencies,
        )
        .map_err(DispatchError::TranslationKey)?;
        validate_cached_snapshot(&key, code, dependencies)?;

        let hit = cache.key().is_some_and(|resident| resident == &key)
            && cache
                .key()
                .is_some_and(|resident| resident.matches_code_view(code));

        if !hit {
            let compiled = match compile_prepared_dispatch_block(code.code_end(), built, admission)
                .map_err(DispatchError::Compile)?
            {
                BlockCompileOutcome::Compiled(block) => block,
                BlockCompileOutcome::NoNativePrefix { rip, .. } => {
                    return Err(DispatchError::NoNativePrefix { rip });
                }
            };
            let logical_bytes =
                cached_entry_size::<B::Published>(&key, &compiled).map_err(DispatchError::Cache)?;
            if logical_bytes > cache.max_bytes() {
                return Err(DispatchError::Cache(CacheError::EntryTooLarge {
                    actual: logical_bytes,
                    max: cache.max_bytes(),
                }));
            }

            validate_cached_snapshot(&key, code, dependencies)?;

            // MAP_JIT is a process-wide singleton lease on the supported host.
            // Release the previous owner before requesting another publication.
            cache.clear();
            let published = backend
                .publish_cached_block(&compiled)
                .map_err(DispatchError::CachePublish)?;
            if !key.matches_code_view(code) {
                drop(published);
                return Err(DispatchError::StaleDependencySnapshot);
            }
            if let Err(error) = validate_cached_snapshot(&key, code, dependencies) {
                drop(published);
                return Err(error);
            }
            cache.install(CachedEntry {
                key,
                block: compiled,
                published,
                logical_bytes,
            });
        }

        let cached_key = cache.key().expect("cache entry installed or hit");
        if let Err(error) = validate_cached_snapshot(cached_key, code, dependencies) {
            cache.clear();
            return Err(error);
        }
        if !cached_key.matches_code_view(code) {
            cache.clear();
            return Err(DispatchError::StaleDependencySnapshot);
        }
        let cached_block = cache.cached_block().expect("cache entry installed or hit");
        let published = cache.published().expect("cache entry installed or hit");
        let execution =
            backend.execute_cached_block(published, cached_block, state, remaining, control);
        let report = match execution {
            Ok(report) => report,
            Err(error) => {
                cache.clear();
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

        let block = match compile_block_for_nop_fallback(code, state.rip, admission)
            .map_err(DispatchError::Compile)?
        {
            BlockCompileOutcome::Compiled(block) => block,
            BlockCompileOutcome::NoNativePrefix { rip, .. } => {
                if !admission.admits_nop(rip) || rip != state.rip {
                    return Err(DispatchError::NoNativePrefix { rip });
                }
                if let Some(exit) = step_admitted_nop(
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

        let report = executor
            .execute_one_block_controlled(&block, state, remaining, control)
            .map_err(DispatchError::Executor)?;
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
                if !admission.admits_nop(rip) {
                    return Err(DispatchError::InterpreterInstructionNotAdmitted { rip });
                }
                if let Some(exit) = step_admitted_nop(
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

#[allow(clippy::too_many_arguments)]
fn step_admitted_nop<E>(
    code: CodeView<'_>,
    admission: &InterpreterFallbackAdmission,
    state: &mut LongModeState,
    remaining: &mut u64,
    retired_interpreter: &mut u64,
    total_budget: u64,
    retired_native: u64,
    control: DispatchControl<'_>,
) -> Result<Option<DispatchExit>, DispatchError<E>> {
    let rip = state.rip;
    if !admission.matches_code(code) || !admission.admits_nop(rip) {
        return Err(DispatchError::InterpreterInstructionNotAdmitted { rip });
    }
    if let Some(exit) = control_exit(rip, code.code_end(), *remaining, control) {
        return Ok(Some(exit));
    }

    let mut memory =
        CodeMemory::one_instruction(code, rip).ok_or(DispatchError::InterpreterMemoryError {
            rip,
            error: MemoryError::Unmapped {
                address: rip,
                access: od_isa::memory::AccessKind::Fetch,
            },
        })?;
    match od_isa::execute::step(state, &mut memory, AccessContext::default()) {
        Ok(StepOutcome::Advanced) => {
            let expected_rip = rip.wrapping_add(1);
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
        AddressSpaceId, BackingId, BackingVersion, CodeIdentity, MappingId, MappingVersion,
    };
    use std::cell::RefCell;
    use std::num::NonZeroU64;
    use std::rc::Rc;

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
            Ok(reply.report)
        }
    }

    fn admitted(code: &[u8], base: u64) -> (CodeView<'_>, AdmittedInstructionBoundaries) {
        let view = CodeView::new(base, code).unwrap();
        let decoded = od_isa::decode(code).unwrap();
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(view, &decoded).unwrap();
        (view, admission)
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
        live: usize,
        max_live: usize,
        drops: usize,
        fail_publish: bool,
        loop_to_entry: bool,
        request_stop_once: bool,
    }

    struct MockPublished(Rc<RefCell<PublishedTracker>>);

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
            assert_eq!(tracker.live, 0, "old mapping must drop before publish");
            tracker.publishes += 1;
            tracker.live += 1;
            tracker.max_live = tracker.max_live.max(tracker.live);
            drop(tracker);
            Ok(MockPublished(self.0.clone()))
        }

        fn execute_cached_block(
            &mut self,
            _published: &Self::Published,
            block: &crate::block_compile::CompiledBlock,
            state: &mut LongModeState,
            incoming_budget: u64,
            control: DispatchControl<'_>,
        ) -> Result<BlockExecutionReport, Self::Error> {
            let (loop_to_entry, request_stop_once) = {
                let mut tracker = self.0.borrow_mut();
                let request_stop = tracker.request_stop_once;
                tracker.request_stop_once = false;
                (tracker.loop_to_entry, request_stop)
            };
            if request_stop_once {
                control.stop_requested.store(1, Ordering::Release);
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

    fn cached_fixture<'a>(
        bytes: &'a [u8],
        base: u64,
    ) -> (CodeView<'a>, AdmittedInstructionBoundaries) {
        admitted(bytes, base)
    }

    #[test]
    fn code_end_precedes_zero_budget_and_zero_budget_does_not_execute() {
        let code = [0x74, 0x00];
        let (view, admission) = admitted(&code, 0x1000);
        let mut state = LongModeState {
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
        };
        let mut state = LongModeState {
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
        });
        let stopped_before_nop = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            5,
            DispatchControl {
                stop_rip: Some(view.base_rip() + 10),
                stop_requested: &stop,
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
        });
        let stopped_by_shared_atomic = run_with_nop_fallback(
            view,
            &admission,
            &mut state,
            5,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
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
    fn executor_error_is_terminal_and_never_retries_or_falls_back() {
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&3_u64.to_le_bytes());
        code.push(0x90);
        let (view, admission) = nop_admission(&code, 0x5480);
        let mut state = LongModeState {
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
                },
                ControlledReply {
                    report: report(RuntimeExitTag::DispatchCandidate, 5, 2),
                    next_rip: Some(view.code_end()),
                    rax: None,
                    rflags: Some(0x242),
                    request_stop: false,
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
            rip: view.base_rip(),
            ..LongModeState::default()
        };

        let result = run_cached(
            view,
            &admission,
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
            rip: view_a.base_rip(),
            ..LongModeState::default()
        };
        run_cached(
            view_a,
            &admission_a,
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
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        run_cached(
            view,
            &admission,
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
            &deps_v2,
            &mut state,
            1,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(tracker.borrow().publishes, 2);

        state.rip = view.base_rip();
        assert!(matches!(
            run_cached(
                view,
                &admission,
                &[],
                &mut state,
                1,
                &mut cache,
                &mut backend,
            ),
            Err(DispatchError::TranslationKey(_))
        ));
        assert_eq!(tracker.borrow().publishes, 2);
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
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        assert!(matches!(
            run_cached(
                view,
                &admission,
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
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        assert!(matches!(
            run_cached(
                view,
                &admission,
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
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let result = run_cached(
            view,
            &admission,
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
            rip: view.base_rip(),
            ..LongModeState::default()
        };
        let tracker = Rc::new(RefCell::new(PublishedTracker::default()));
        let mut backend = MockCachedBackend(tracker.clone());
        let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let stopped = run_cached_controlled(
            view,
            &admission,
            &dependencies,
            &mut state,
            1,
            DispatchControl {
                stop_rip: Some(view.code_end()),
                stop_requested: &stop_requested,
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
}
