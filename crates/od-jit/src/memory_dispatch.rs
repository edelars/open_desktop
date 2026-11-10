//! Uncached, bounded dispatch for the checked qword MOV memory slice.
//!
//! This API is owner-borrowed: executable bytes are an immutable `CodeView`
//! kept alive for the complete call, and the data-memory borrow is held until
//! the synchronous executor returns. It is not an MMU or production RAM owner.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use od_isa::{
    LongModeState, long_decode,
    long_decode::DecodeError,
    memory::{AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError},
};

use crate::{
    block::{CodeMemory, CodeView},
    block_compile::{
        BlockCompileError, BlockCompileOutcome, compile_block, compile_block_demand_flags_candidate,
    },
    dispatch::{
        BlockExecutionReport, ControlledBlockExecution, ControlledBlockExecutor, DispatchControl,
    },
    memory_load_v2::{
        IrMemoryLoadBlockV2, IrMemoryStoreBlockV2, IrMemoryTwoSiteBlockV2,
        compile_ir_memory_load_block_v2, compile_ir_memory_store_block_v2,
        compile_ir_memory_two_site_block_v2,
    },
};

/// Compiler strategy for the bounded arithmetic/load/Jcc handoff. The
/// demand-driven candidate is deliberately explicit and uncached; eager stays
/// the stable default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundedArithmeticLoadStrategy {
    Eager,
    DemandFlagsCandidate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundedArithmeticLoadExit {
    Complete,
    CodeInvalidated,
    BudgetExhausted,
    StopRequested,
    StopAddress,
    EventPending,
    GuestFault(ArchitecturalFault),
    Unsupported { rip: u64 },
    Invalid { rip: u64 },
    Runtime(crate::runtime::RuntimeExitTag),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedArithmeticLoadResult {
    pub exit: BoundedArithmeticLoadExit,
    pub remaining_budget: u64,
    pub retired_native: u64,
    pub retired_memory: u64,
    pub native_blocks: u64,
    pub memory_blocks: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub enum BoundedArithmeticLoadError<NE, ME> {
    UnsupportedCr3State {
        value: u64,
    },
    InvalidPlan {
        rip: u64,
    },
    InvalidEntryRip {
        rip: u64,
    },
    Compile(BlockCompileError),
    NoNativePrefix {
        rip: u64,
    },
    NativeExecutor(NE),
    NativeAccounting {
        incoming: u64,
        report: BlockExecutionReport,
    },
    InvalidBudgetExit {
        remaining: u64,
    },
    NativeProgress {
        rip: u64,
    },
    InvalidMemoryReport {
        report: BoundedMemoryResult,
    },
    InvalidNativeYield {
        rip: u64,
        exit: crate::dispatch::DispatchExit,
    },
    InvalidNativeControl {
        rip: u64,
        exit: crate::runtime::RuntimeExitTag,
    },
    Memory(BoundedMemoryError<ME>),
    InvalidPhaseRip {
        expected: u64,
        actual: u64,
    },
    CounterOverflow,
}

/// Executes the exact bounded S3 arithmetic -> checked-load -> JZ/JNZ profile
/// over one immutable full `CodeView`. The same view (and therefore original
/// guest RIPs) is retained for every phase. Calls beginning at any decoded
/// instruction boundary in the arithmetic prefix, at the load, Jcc, its
/// fallthrough suffix, or the global code end are accepted. This permits
/// exact retry after a budget/STOP handoff without replaying committed work.
///
/// # Safety
/// The memory executor must satisfy the ordinary synchronous private-RAM
/// preconditions required by [`MemoryBlockExecutor`].
pub unsafe fn run_bounded_arithmetic_load<E, M>(
    code: CodeView<'_>,
    state: &mut LongModeState,
    memory: &mut dyn GuestMemory,
    access_context: AccessContext,
    total_budget: u64,
    stop_rip: Option<u64>,
    stop_requested: &AtomicU8,
    event_requested: Option<&AtomicBool>,
    strategy: BoundedArithmeticLoadStrategy,
    native_executor: &mut E,
    memory_executor: &mut M,
) -> Result<BoundedArithmeticLoadResult, BoundedArithmeticLoadError<E::Error, M::Error>>
where
    E: ControlledBlockExecutor,
    M: MemoryBlockExecutor,
{
    if let Some(value) = state.cr3 {
        return Err(BoundedArithmeticLoadError::UnsupportedCr3State { value });
    }
    let plan = bounded_arithmetic_load_plan(code)
        .ok_or(BoundedArithmeticLoadError::InvalidPlan { rip: state.rip })?;
    if !plan.admits_entry(state.rip) {
        return Err(BoundedArithmeticLoadError::InvalidEntryRip { rip: state.rip });
    }

    if stop_rip == Some(state.rip) {
        return Ok(handoff_result(
            BoundedArithmeticLoadExit::StopAddress,
            total_budget,
            0,
            0,
            0,
            0,
        ));
    }
    if stop_requested.load(Ordering::Acquire) != 0 {
        return Ok(handoff_result(
            BoundedArithmeticLoadExit::StopRequested,
            total_budget,
            0,
            0,
            0,
            0,
        ));
    }
    if state.rip == plan.code_end {
        return Ok(handoff_result(
            BoundedArithmeticLoadExit::Complete,
            total_budget,
            0,
            0,
            0,
            0,
        ));
    }
    if total_budget == 0 {
        return Ok(handoff_result(
            BoundedArithmeticLoadExit::BudgetExhausted,
            0,
            0,
            0,
            0,
            0,
        ));
    }
    if event_requested.is_some_and(|event| event.load(Ordering::Acquire)) {
        return Ok(handoff_result(
            BoundedArithmeticLoadExit::EventPending,
            total_budget,
            0,
            0,
            0,
            0,
        ));
    }

    let mut remaining = total_budget;
    let mut retired_native = 0_u64;
    let mut retired_memory = 0_u64;
    let mut native_blocks = 0_u64;
    let mut memory_blocks = 0_u64;

    if state.rip < plan.load_rip {
        match bounded_native_phase(
            code,
            plan,
            state,
            remaining,
            plan.load_rip,
            stop_rip,
            stop_requested,
            event_requested,
            None,
            strategy,
            native_executor,
        )? {
            Phase::Continue(report) => {
                remaining = report.remaining_budget;
                retired_native = retired_native
                    .checked_add(report.retired_instructions)
                    .ok_or(BoundedArithmeticLoadError::CounterOverflow)?;
                native_blocks = native_blocks
                    .checked_add(1)
                    .ok_or(BoundedArithmeticLoadError::CounterOverflow)?;
            }
            Phase::Exit { exit, report } => {
                if let Some(report) = report {
                    record_native_report(
                        report,
                        &mut remaining,
                        &mut retired_native,
                        &mut native_blocks,
                    )?;
                }
                return Ok(handoff_result(
                    exit,
                    remaining,
                    retired_native,
                    retired_memory,
                    native_blocks,
                    memory_blocks,
                ));
            }
        }
        if state.rip != plan.load_rip {
            return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
                expected: plan.load_rip,
                actual: state.rip,
            });
        }
    }

    if state.rip == plan.load_rip {
        if let Some(exit) = pending_control(
            state.rip,
            plan.load_rip,
            remaining,
            stop_rip,
            stop_requested,
            event_requested,
        ) {
            return Ok(handoff_result(
                exit,
                remaining,
                retired_native,
                retired_memory,
                native_blocks,
                memory_blocks,
            ));
        }
        let boundary = FixtureExecutionBoundary::new(code, plan.suffix_rip)
            .map_err(|_| BoundedArithmeticLoadError::InvalidPlan { rip: state.rip })?;
        // SAFETY: forwarded from this function's caller contract.
        let report = unsafe {
            run_controlled(
                code,
                boundary,
                state,
                memory,
                access_context,
                remaining,
                stop_rip,
                stop_requested,
                event_requested,
                memory_executor,
            )
        };
        let report = match report {
            Ok(report) => report,
            Err(error) => {
                if state.rip != plan.load_rip {
                    return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
                        expected: plan.load_rip,
                        actual: state.rip,
                    });
                }
                return Err(BoundedArithmeticLoadError::Memory(error));
            }
        };
        let expected_rip = match report.retired_instructions {
            0 if report.remaining_budget == remaining => Some(plan.load_rip),
            1 if remaining.checked_sub(1) == Some(report.remaining_budget) => Some(plan.suffix_rip),
            _ => None,
        };
        let exit_matches_retirement = match report.exit {
            BoundedMemoryExit::GuestFault(_) => report.retired_instructions == 0,
            BoundedMemoryExit::BoundaryReached { rip } => {
                report.retired_instructions == 1 && rip == plan.suffix_rip
            }
            BoundedMemoryExit::Unsupported { rip } | BoundedMemoryExit::Invalid { rip } => {
                report.retired_instructions == 0 && rip == plan.load_rip
            }
            BoundedMemoryExit::CodeInvalidated
            | BoundedMemoryExit::BudgetExhausted
            | BoundedMemoryExit::StopRequested
            | BoundedMemoryExit::StopAddress
            | BoundedMemoryExit::EventPending => true,
        };
        let blocks_match_exit = match report.exit {
            BoundedMemoryExit::GuestFault(_)
            | BoundedMemoryExit::BoundaryReached { .. }
            | BoundedMemoryExit::CodeInvalidated => report.blocks_executed == 1,
            BoundedMemoryExit::Unsupported { .. } | BoundedMemoryExit::Invalid { .. } => {
                report.blocks_executed == 0
            }
            BoundedMemoryExit::EventPending => {
                report.blocks_executed == report.retired_instructions
            }
            BoundedMemoryExit::BudgetExhausted
            | BoundedMemoryExit::StopRequested
            | BoundedMemoryExit::StopAddress => {
                report.blocks_executed <= 1
                    && (report.retired_instructions == 0 || report.blocks_executed == 1)
            }
        };
        if state.rip != plan.load_rip && state.rip != plan.suffix_rip {
            return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
                expected: expected_rip.unwrap_or(plan.load_rip),
                actual: state.rip,
            });
        }
        if expected_rip != Some(state.rip) || !exit_matches_retirement || !blocks_match_exit {
            return Err(BoundedArithmeticLoadError::InvalidMemoryReport { report });
        }
        if report.exit
            == (BoundedMemoryExit::BoundaryReached {
                rip: plan.suffix_rip,
            })
        {
            if state.rip != plan.suffix_rip || report.retired_instructions != 1 {
                return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
                    expected: plan.suffix_rip,
                    actual: state.rip,
                });
            }
            remaining = report.remaining_budget;
            retired_memory = report.retired_instructions;
            memory_blocks = report.blocks_executed;
        } else {
            return Ok(handoff_memory_exit(
                report,
                retired_native,
                retired_memory,
                native_blocks,
                memory_blocks,
            ));
        }
    }

    let branch_fallthrough = plan.branch_fallthrough;
    if state.rip == plan.suffix_rip || state.rip == branch_fallthrough {
        while state.rip != plan.code_end {
            match bounded_native_phase(
                code,
                plan,
                state,
                remaining,
                plan.code_end,
                stop_rip,
                stop_requested,
                event_requested,
                Some(branch_fallthrough),
                strategy,
                native_executor,
            )? {
                Phase::Continue(report) => {
                    remaining = report.remaining_budget;
                    retired_native = retired_native
                        .checked_add(report.retired_instructions)
                        .ok_or(BoundedArithmeticLoadError::CounterOverflow)?;
                    native_blocks = native_blocks
                        .checked_add(1)
                        .ok_or(BoundedArithmeticLoadError::CounterOverflow)?;
                }
                Phase::Exit { exit, report } => {
                    if let Some(report) = report {
                        record_native_report(
                            report,
                            &mut remaining,
                            &mut retired_native,
                            &mut native_blocks,
                        )?;
                    }
                    return Ok(handoff_result(
                        exit,
                        remaining,
                        retired_native,
                        retired_memory,
                        native_blocks,
                        memory_blocks,
                    ));
                }
            }
            if state.rip != plan.code_end && state.rip != branch_fallthrough {
                return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
                    expected: plan.code_end,
                    actual: state.rip,
                });
            }
        }
    }

    if let Some(exit) = pending_control(
        state.rip,
        plan.code_end,
        remaining,
        stop_rip,
        stop_requested,
        event_requested,
    ) {
        return Ok(handoff_result(
            exit,
            remaining,
            retired_native,
            retired_memory,
            native_blocks,
            memory_blocks,
        ));
    }

    Ok(BoundedArithmeticLoadResult {
        exit: BoundedArithmeticLoadExit::Complete,
        remaining_budget: remaining,
        retired_native,
        retired_memory,
        native_blocks,
        memory_blocks,
    })
}

#[derive(Clone, Copy, Debug)]
struct BoundedArithmeticLoadPlan {
    entry_rip: u64,
    load_rip: u64,
    suffix_rip: u64,
    branch_fallthrough: u64,
    code_end: u64,
}

impl BoundedArithmeticLoadPlan {
    fn admits_entry(self, rip: u64) -> bool {
        ((self.entry_rip..=self.load_rip).contains(&rip)
            && (rip - self.entry_rip).is_multiple_of(3))
            || rip == self.suffix_rip
            || rip == self.branch_fallthrough
            || rip == self.code_end
    }

    fn admits_native_phase_rip(
        self,
        phase_start: u64,
        phase_end: u64,
        continuation_rip: Option<u64>,
        rip: u64,
    ) -> bool {
        if phase_end == self.load_rip && phase_start < self.load_rip {
            return (phase_start..=self.load_rip).contains(&rip)
                && (rip - self.entry_rip).is_multiple_of(3);
        }
        phase_end == self.code_end
            && (phase_start == self.suffix_rip || phase_start == self.branch_fallthrough)
            && (rip == phase_start || continuation_rip == Some(rip) || rip == self.code_end)
    }
}

fn bounded_arithmetic_load_plan(code: CodeView<'_>) -> Option<BoundedArithmeticLoadPlan> {
    let bytes = code.bytes();
    if !(11..=14).contains(&bytes.len()) {
        return None;
    }
    let arithmetic_count = match bytes.len() {
        11 => 1,
        14 => 2,
        _ => return None,
    };
    for chunk in bytes[..arithmetic_count * 3].chunks_exact(3) {
        if !matches!(
            chunk,
            [0x48, 0x03, 0xc3] | [0x48, 0x2b, 0xc3] | [0x48, 0x3b, 0xc3]
        ) {
            return None;
        }
    }
    let load_offset = arithmetic_count * 3;
    if bytes.get(load_offset..load_offset + 3)? != [0x48, 0x8b, 0x03] {
        return None;
    }
    let branch_offset = load_offset + 3;
    if !matches!(
        bytes.get(branch_offset..branch_offset + 2)?,
        [0x74, 0x03] | [0x75, 0x03]
    ) {
        return None;
    }
    if bytes.get(branch_offset + 2..)? != [0x48, 0x03, 0xc3] {
        return None;
    }
    let mut source = CodeMemory::full_code(code);
    let mut rip = code.base_rip();
    while rip < code.code_end() {
        let instruction = long_decode::decode(&mut source, rip, AccessContext::default()).ok()?;
        if instruction.length == 0 {
            return None;
        }
        rip = rip.checked_add(u64::from(instruction.length))?;
    }
    if rip != code.code_end() {
        return None;
    }
    let entry_rip = code.base_rip();
    let load_rip = entry_rip.checked_add(load_offset as u64)?;
    let suffix_rip = load_rip.checked_add(3)?;
    let branch_fallthrough = suffix_rip.checked_add(2)?;
    Some(BoundedArithmeticLoadPlan {
        entry_rip,
        load_rip,
        suffix_rip,
        branch_fallthrough,
        code_end: code.code_end(),
    })
}

enum Phase {
    Continue(BlockExecutionReport),
    Exit {
        exit: BoundedArithmeticLoadExit,
        report: Option<BlockExecutionReport>,
    },
}

fn record_native_report<NE, ME>(
    report: BlockExecutionReport,
    remaining: &mut u64,
    retired_native: &mut u64,
    native_blocks: &mut u64,
) -> Result<(), BoundedArithmeticLoadError<NE, ME>> {
    *remaining = report.remaining_budget;
    *retired_native = retired_native
        .checked_add(report.retired_instructions)
        .ok_or(BoundedArithmeticLoadError::CounterOverflow)?;
    *native_blocks = native_blocks
        .checked_add(1)
        .ok_or(BoundedArithmeticLoadError::CounterOverflow)?;
    Ok(())
}

fn pending_control(
    rip: u64,
    end_rip: u64,
    remaining: u64,
    stop_rip: Option<u64>,
    stop: &AtomicU8,
    event: Option<&AtomicBool>,
) -> Option<BoundedArithmeticLoadExit> {
    if stop_rip == Some(rip) {
        Some(BoundedArithmeticLoadExit::StopAddress)
    } else if stop.load(Ordering::Acquire) != 0 {
        Some(BoundedArithmeticLoadExit::StopRequested)
    } else if rip == end_rip {
        None
    } else if remaining == 0 {
        Some(BoundedArithmeticLoadExit::BudgetExhausted)
    } else if event.is_some_and(|event| event.load(Ordering::Acquire)) {
        Some(BoundedArithmeticLoadExit::EventPending)
    } else {
        None
    }
}

fn prioritized_native_control(
    plan: BoundedArithmeticLoadPlan,
    rip: u64,
    remaining: u64,
    stop_rip: Option<u64>,
    stop: &AtomicU8,
    event: Option<&AtomicBool>,
) -> Option<BoundedArithmeticLoadExit> {
    if stop_rip == Some(rip) {
        Some(BoundedArithmeticLoadExit::StopAddress)
    } else if stop.load(Ordering::Acquire) != 0 {
        Some(BoundedArithmeticLoadExit::StopRequested)
    } else if rip == plan.code_end {
        Some(BoundedArithmeticLoadExit::Complete)
    } else if remaining == 0 {
        Some(BoundedArithmeticLoadExit::BudgetExhausted)
    } else if event.is_some_and(|request| request.load(Ordering::Acquire)) {
        Some(BoundedArithmeticLoadExit::EventPending)
    } else {
        None
    }
}

fn native_retirement_matches_phase(
    plan: BoundedArithmeticLoadPlan,
    start: u64,
    retired: u64,
    actual: u64,
) -> bool {
    if retired == 0 {
        return actual == start;
    }
    if start < plan.load_rip {
        let Some(delta) = retired.checked_mul(3) else {
            return false;
        };
        return start
            .checked_add(delta)
            .is_some_and(|expected| expected == actual && expected <= plan.load_rip);
    }
    if start == plan.suffix_rip {
        return match retired {
            1 => actual == plan.branch_fallthrough || actual == plan.code_end,
            2 => actual == plan.code_end,
            _ => false,
        };
    }
    start == plan.branch_fallthrough && retired == 1 && actual == plan.code_end
}

fn bounded_native_phase<E: ControlledBlockExecutor, ME>(
    code: CodeView<'_>,
    plan: BoundedArithmeticLoadPlan,
    state: &mut LongModeState,
    budget: u64,
    end_rip: u64,
    stop_rip: Option<u64>,
    stop: &AtomicU8,
    event: Option<&AtomicBool>,
    continuation_rip: Option<u64>,
    strategy: BoundedArithmeticLoadStrategy,
    executor: &mut E,
) -> Result<Phase, BoundedArithmeticLoadError<E::Error, ME>> {
    let rip = state.rip;
    if let Some(exit) = pending_control(rip, end_rip, budget, stop_rip, stop, event) {
        return Ok(Phase::Exit { exit, report: None });
    }
    let compiled = match strategy {
        BoundedArithmeticLoadStrategy::Eager => compile_block(code, rip),
        BoundedArithmeticLoadStrategy::DemandFlagsCandidate => {
            compile_block_demand_flags_candidate(code, rip)
        }
    }
    .map_err(BoundedArithmeticLoadError::Compile)?;
    let BlockCompileOutcome::Compiled(block) = compiled else {
        return Err(BoundedArithmeticLoadError::NoNativePrefix { rip });
    };
    if let Some(exit) = pending_control(rip, end_rip, budget, stop_rip, stop, event) {
        return Ok(Phase::Exit { exit, report: None });
    }
    let execution = executor
        .execute_one_block_controlled_with_event(
            &block,
            state,
            budget,
            DispatchControl {
                stop_rip,
                stop_requested: stop,
                event_requested: event,
            },
        )
        .map_err(BoundedArithmeticLoadError::NativeExecutor)?;
    let report = match execution {
        ControlledBlockExecution::Yield(exit) => {
            if state.rip != rip {
                return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
                    expected: rip,
                    actual: state.rip,
                });
            }
            if !plan.admits_native_phase_rip(rip, end_rip, continuation_rip, state.rip) {
                return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
                    expected: end_rip,
                    actual: state.rip,
                });
            }
            let control_is_pending = match exit {
                crate::dispatch::DispatchExit::CodeEnd => state.rip == plan.code_end,
                crate::dispatch::DispatchExit::BudgetExhausted => budget == 0,
                crate::dispatch::DispatchExit::EventPending => {
                    event.is_some_and(|request| request.load(Ordering::Acquire))
                }
                crate::dispatch::DispatchExit::Runtime(
                    crate::runtime::RuntimeExitTag::StopAddress,
                ) => stop_rip == Some(state.rip),
                crate::dispatch::DispatchExit::Runtime(
                    crate::runtime::RuntimeExitTag::StopRequested,
                ) => stop.load(Ordering::Acquire) != 0,
                crate::dispatch::DispatchExit::Runtime(_) => false,
            };
            if !control_is_pending {
                return Err(BoundedArithmeticLoadError::InvalidNativeYield {
                    rip: state.rip,
                    exit,
                });
            }
            let exit = match exit {
                crate::dispatch::DispatchExit::BudgetExhausted => {
                    BoundedArithmeticLoadExit::BudgetExhausted
                }
                crate::dispatch::DispatchExit::EventPending => {
                    BoundedArithmeticLoadExit::EventPending
                }
                crate::dispatch::DispatchExit::Runtime(
                    crate::runtime::RuntimeExitTag::StopAddress,
                ) => BoundedArithmeticLoadExit::StopAddress,
                crate::dispatch::DispatchExit::Runtime(
                    crate::runtime::RuntimeExitTag::StopRequested,
                ) => BoundedArithmeticLoadExit::StopRequested,
                crate::dispatch::DispatchExit::Runtime(other) => {
                    BoundedArithmeticLoadExit::Runtime(other)
                }
                crate::dispatch::DispatchExit::CodeEnd => BoundedArithmeticLoadExit::Complete,
            };
            let exit = prioritized_native_control(plan, state.rip, budget, stop_rip, stop, event)
                .unwrap_or(exit);
            return Ok(Phase::Exit { exit, report: None });
        }
        ControlledBlockExecution::Executed(report) => report,
    };
    if report.retired_instructions > budget
        || report.retired_instructions > u64::from(block.emitted().pc_map().instruction_count())
        || budget - report.retired_instructions != report.remaining_budget
    {
        return Err(BoundedArithmeticLoadError::NativeAccounting {
            incoming: budget,
            report,
        });
    }
    if !plan.admits_native_phase_rip(rip, end_rip, continuation_rip, state.rip) {
        return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
            expected: end_rip,
            actual: state.rip,
        });
    }
    if !native_retirement_matches_phase(plan, rip, report.retired_instructions, state.rip) {
        return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
            expected: rip,
            actual: state.rip,
        });
    }
    if report.exit == crate::runtime::RuntimeExitTag::BudgetExhausted
        && (report.remaining_budget != 0 || report.retired_instructions == 0)
    {
        return Err(BoundedArithmeticLoadError::InvalidBudgetExit {
            remaining: report.remaining_budget,
        });
    }
    let at_expected_boundary = state.rip == end_rip || continuation_rip == Some(state.rip);
    let terminal_exit = match report.exit {
        crate::runtime::RuntimeExitTag::BudgetExhausted => {
            Some(BoundedArithmeticLoadExit::BudgetExhausted)
        }
        crate::runtime::RuntimeExitTag::StopRequested => {
            Some(BoundedArithmeticLoadExit::StopRequested)
        }
        crate::runtime::RuntimeExitTag::StopAddress => Some(BoundedArithmeticLoadExit::StopAddress),
        _ => None,
    };
    if let Some(exit) = terminal_exit {
        let control_is_pending = match report.exit {
            crate::runtime::RuntimeExitTag::StopRequested => stop.load(Ordering::Acquire) != 0,
            crate::runtime::RuntimeExitTag::StopAddress => stop_rip == Some(state.rip),
            crate::runtime::RuntimeExitTag::BudgetExhausted => true,
            _ => true,
        };
        if !control_is_pending {
            return Err(BoundedArithmeticLoadError::InvalidNativeControl {
                rip: state.rip,
                exit: report.exit,
            });
        }
        let exit = prioritized_native_control(
            plan,
            state.rip,
            report.remaining_budget,
            stop_rip,
            stop,
            event,
        )
        .unwrap_or(exit);
        return Ok(Phase::Exit {
            exit,
            report: Some(report),
        });
    }
    if report.retired_instructions == 0 || state.rip == rip {
        return Err(BoundedArithmeticLoadError::NativeProgress { rip });
    }
    if at_expected_boundary
        && matches!(
            report.exit,
            crate::runtime::RuntimeExitTag::NeedInterpreter
                | crate::runtime::RuntimeExitTag::Unsupported
                | crate::runtime::RuntimeExitTag::DispatchCandidate
        )
    {
        return Ok(Phase::Continue(report));
    }
    if !matches!(
        report.exit,
        crate::runtime::RuntimeExitTag::BlockComplete
            | crate::runtime::RuntimeExitTag::DispatchCandidate
    ) {
        return Ok(Phase::Exit {
            exit: BoundedArithmeticLoadExit::Runtime(report.exit),
            report: Some(report),
        });
    }
    if !at_expected_boundary {
        return Err(BoundedArithmeticLoadError::InvalidPhaseRip {
            expected: end_rip,
            actual: state.rip,
        });
    }
    Ok(Phase::Continue(report))
}

fn handoff_result(
    exit: BoundedArithmeticLoadExit,
    remaining_budget: u64,
    retired_native: u64,
    retired_memory: u64,
    native_blocks: u64,
    memory_blocks: u64,
) -> BoundedArithmeticLoadResult {
    BoundedArithmeticLoadResult {
        exit,
        remaining_budget,
        retired_native,
        retired_memory,
        native_blocks,
        memory_blocks,
    }
}

fn handoff_memory_exit(
    report: BoundedMemoryResult,
    retired_native: u64,
    retired_memory: u64,
    native_blocks: u64,
    memory_blocks: u64,
) -> BoundedArithmeticLoadResult {
    let exit = match report.exit {
        BoundedMemoryExit::CodeInvalidated => BoundedArithmeticLoadExit::CodeInvalidated,
        BoundedMemoryExit::BudgetExhausted => BoundedArithmeticLoadExit::BudgetExhausted,
        BoundedMemoryExit::StopRequested => BoundedArithmeticLoadExit::StopRequested,
        BoundedMemoryExit::StopAddress => BoundedArithmeticLoadExit::StopAddress,
        BoundedMemoryExit::EventPending => BoundedArithmeticLoadExit::EventPending,
        BoundedMemoryExit::GuestFault(fault) => BoundedArithmeticLoadExit::GuestFault(fault),
        BoundedMemoryExit::BoundaryReached { .. } => BoundedArithmeticLoadExit::Complete,
        BoundedMemoryExit::Unsupported { rip } => BoundedArithmeticLoadExit::Unsupported { rip },
        BoundedMemoryExit::Invalid { rip } => BoundedArithmeticLoadExit::Invalid { rip },
    };
    BoundedArithmeticLoadResult {
        exit,
        remaining_budget: report.remaining_budget,
        retired_native,
        retired_memory: retired_memory + report.retired_instructions,
        native_blocks,
        memory_blocks: memory_blocks + report.blocks_executed,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundedMemoryArtifact {
    Pair(IrMemoryTwoSiteBlockV2),
    Load(IrMemoryLoadBlockV2),
    Store(IrMemoryStoreBlockV2),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryBlockExit {
    Complete,
    CodeInvalidated,
    BudgetExhausted,
    StopRequested,
    StopAddress,
    /// The host executor published the artifact, then observed a pending
    /// cooperative event before native entry. No guest work may be committed.
    EventPending,
    GuestFault(ArchitecturalFault),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryBlockReport {
    pub exit: MemoryBlockExit,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
}

/// Executes one compiler-issued checked-memory artifact synchronously.
/// Implementations release the artifact's executable mapping before returning
/// so opt-in interpreter continuation runs only after native publication ends.
///
/// # Safety
/// `memory` must satisfy the private synchronous ordinary RAM preconditions
/// documented by `PublishedCode::execute_ir_memory_two_site_v2`: no MMIO,
/// executable/code aliases, concurrent observers, delayed writes, re-entry,
/// or unwind through the helper. Writes must be all-or-nothing. The caller
/// keeps the owner, code bytes, state, and memory alive and exclusively
/// borrowed until this call returns.
pub unsafe trait MemoryBlockExecutor {
    type Error;

    unsafe fn execute_one_memory_block(
        &mut self,
        block: &BoundedMemoryArtifact,
        state: &mut LongModeState,
        memory: &mut dyn GuestMemory,
        access_context: AccessContext,
        incoming_budget: u64,
        stop_rip: Option<u64>,
        stop_requested: &AtomicU8,
    ) -> Result<MemoryBlockReport, Self::Error>;

    /// Event-aware counterpart for controlled dispatch. Existing executors
    /// retain their event-disabled implementation; the dispatcher still
    /// samples the request after the synchronous artifact returns. Executors
    /// that publish before entry should override this method to sample between
    /// publication and native execution, returning `MemoryBlockExit::EventPending`
    /// without changing `state`, `memory`, or accounting.
    unsafe fn execute_one_memory_block_controlled(
        &mut self,
        block: &BoundedMemoryArtifact,
        state: &mut LongModeState,
        memory: &mut dyn GuestMemory,
        access_context: AccessContext,
        incoming_budget: u64,
        stop_rip: Option<u64>,
        stop_requested: &AtomicU8,
        event_requested: Option<&AtomicBool>,
    ) -> Result<MemoryBlockReport, Self::Error> {
        if stop_rip == Some(state.rip) {
            return Ok(MemoryBlockReport {
                exit: MemoryBlockExit::StopAddress,
                remaining_budget: incoming_budget,
                retired_instructions: 0,
            });
        }
        if stop_requested.load(Ordering::Acquire) != 0 {
            return Ok(MemoryBlockReport {
                exit: MemoryBlockExit::StopRequested,
                remaining_budget: incoming_budget,
                retired_instructions: 0,
            });
        }
        if incoming_budget == 0 {
            return Ok(MemoryBlockReport {
                exit: MemoryBlockExit::BudgetExhausted,
                remaining_budget: 0,
                retired_instructions: 0,
            });
        }
        if event_requested.is_some_and(|request| request.load(Ordering::Acquire)) {
            return Ok(MemoryBlockReport {
                exit: MemoryBlockExit::EventPending,
                remaining_budget: incoming_budget,
                retired_instructions: 0,
            });
        }
        // SAFETY: this default preserves the original executor contract. The
        // dispatcher samples the event after the completed block returns.
        unsafe {
            self.execute_one_memory_block(
                block,
                state,
                memory,
                access_context,
                incoming_budget,
                stop_rip,
                stop_requested,
            )
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundedMemoryExit {
    BoundaryReached {
        rip: u64,
    },
    CodeInvalidated,
    BudgetExhausted,
    StopRequested,
    StopAddress,
    /// A cooperative host event is pending at a safe dispatcher boundary.
    EventPending,
    GuestFault(ArchitecturalFault),
    Unsupported {
        rip: u64,
    },
    Invalid {
        rip: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedMemoryResult {
    pub exit: BoundedMemoryExit,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
    pub blocks_executed: u64,
}

/// Accounting for the opt-in exact bare-NOP interpreter bridge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedMemoryFallbackResult {
    pub exit: BoundedMemoryExit,
    pub remaining_budget: u64,
    pub retired_native: u64,
    pub retired_interpreter: u64,
    pub blocks_executed: u64,
}

/// A caller-selected byte-window end for fixture execution. It must lie within
/// `code`; it may split an instruction and is never treated as a guest halt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureExecutionBoundary {
    end_rip: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureBoundaryError {
    OutsideCode { end_rip: u64 },
}

impl FixtureExecutionBoundary {
    pub fn new(code: CodeView<'_>, end_rip: u64) -> Result<Self, FixtureBoundaryError> {
        if end_rip < code.base_rip() || end_rip > code.code_end() {
            return Err(FixtureBoundaryError::OutsideCode { end_rip });
        }
        Ok(Self { end_rip })
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum BoundedMemoryError<E> {
    Executor(E),
    UnsupportedCr3State {
        value: u64,
    },
    InvalidExecutionBoundary {
        end_rip: u64,
    },
    InvalidEntryRip {
        rip: u64,
        code_start: u64,
        execution_end: u64,
    },
    InvalidAccounting {
        incoming: u64,
        report: MemoryBlockReport,
    },
    InvalidBlockProgress {
        rip: u64,
    },
    InterpreterMemory {
        rip: u64,
        error: MemoryError,
    },
    InterpreterStep {
        rip: u64,
        outcome: od_isa::execute::StepOutcome,
    },
    InterpreterRipMismatch {
        expected: u64,
        actual: u64,
    },
    InvalidTotalAccounting {
        total_budget: u64,
        remaining_budget: u64,
        retired_native: u64,
        retired_interpreter: u64,
    },
    CounterOverflow,
}

/// Continue through the explicit immutable code window until its byte end,
/// budget, STOP, stop-RIP, a guest data fault, or unsupported instruction.
///
/// At most one generated artifact is handed to the executor per iteration.
/// Exact two-site sequences compile as one artifact; otherwise the dispatcher
/// compiles only the current singleton load/store. An unsupported lookahead
/// therefore becomes visible only after the supported prefix has committed.
/// The byte end need not be an instruction boundary. A partial instruction at
/// the limit returns `BoundaryReached` without compiling or executing it. This
/// is not a guest fetch fault or guest halt.
///
/// # Safety
/// `memory` must be private, synchronous ordinary RAM with all-or-nothing
/// qword writes. No access may reach MMIO, executable code/backing or any code
/// alias. There may be no concurrent thread, vCPU, DMA engine, or external
/// observer. The backend must not re-enter or unwind through the helper.
/// `code` must remain immutable and must not alias writable data memory for the
/// duration of the call. The owner, state, memory, and code must remain alive
/// for the synchronous invocation. This is a caller contract, not a property
/// inferred from `GuestMemory`; production owner/MMU integration is separate.
pub unsafe fn run<E: MemoryBlockExecutor>(
    code: CodeView<'_>,
    boundary: FixtureExecutionBoundary,
    state: &mut LongModeState,
    memory: &mut dyn GuestMemory,
    access_context: AccessContext,
    total_budget: u64,
    stop_rip: Option<u64>,
    stop_requested: &AtomicU8,
    executor: &mut E,
) -> Result<BoundedMemoryResult, BoundedMemoryError<E::Error>> {
    // SAFETY: forwards the caller contract with event polling disabled.
    unsafe {
        run_controlled(
            code,
            boundary,
            state,
            memory,
            access_context,
            total_budget,
            stop_rip,
            stop_requested,
            None,
            executor,
        )
    }
}

/// Event-aware bounded checked-memory dispatch. The event is sampled only
/// after stop-RIP, STOP, byte-boundary, and budget guards, and is never cleared here.
/// Executors which publish artifacts before entry may return
/// `MemoryBlockExit::EventPending` to yield between publication and execution.
///
/// # Safety
/// Has the same synchronous private-memory and immutable-code requirements as
/// [`run`]. The request owner must not clear the event while this call runs.
pub unsafe fn run_controlled<E: MemoryBlockExecutor>(
    code: CodeView<'_>,
    boundary: FixtureExecutionBoundary,
    state: &mut LongModeState,
    memory: &mut dyn GuestMemory,
    access_context: AccessContext,
    total_budget: u64,
    stop_rip: Option<u64>,
    stop_requested: &AtomicU8,
    event_requested: Option<&AtomicBool>,
    executor: &mut E,
) -> Result<BoundedMemoryResult, BoundedMemoryError<E::Error>> {
    let mut remaining = total_budget;
    let mut retired = 0u64;
    let mut blocks = 0u64;
    if let Some(value) = state.cr3 {
        return Err(BoundedMemoryError::UnsupportedCr3State { value });
    }
    if boundary.end_rip < code.base_rip() || boundary.end_rip > code.code_end() {
        return Err(BoundedMemoryError::InvalidExecutionBoundary {
            end_rip: boundary.end_rip,
        });
    }
    if state.rip < code.base_rip() || state.rip > boundary.end_rip {
        return Err(BoundedMemoryError::InvalidEntryRip {
            rip: state.rip,
            code_start: code.base_rip(),
            execution_end: boundary.end_rip,
        });
    }

    loop {
        if state.rip > boundary.end_rip {
            return Err(BoundedMemoryError::InvalidEntryRip {
                rip: state.rip,
                code_start: code.base_rip(),
                execution_end: boundary.end_rip,
            });
        }
        if stop_rip == Some(state.rip) {
            return Ok(result(
                BoundedMemoryExit::StopAddress,
                remaining,
                retired,
                blocks,
            ));
        }
        if stop_requested.load(Ordering::Acquire) != 0 {
            return Ok(result(
                BoundedMemoryExit::StopRequested,
                remaining,
                retired,
                blocks,
            ));
        }
        if state.rip == boundary.end_rip {
            return Ok(result(
                BoundedMemoryExit::BoundaryReached { rip: state.rip },
                remaining,
                retired,
                blocks,
            ));
        }
        if remaining == 0 {
            return Ok(result(
                BoundedMemoryExit::BudgetExhausted,
                0,
                retired,
                blocks,
            ));
        }
        if event_requested.is_some_and(|request| request.load(Ordering::Acquire)) {
            return Ok(result(
                BoundedMemoryExit::EventPending,
                remaining,
                retired,
                blocks,
            ));
        }

        let block = match compile_at(code, state.rip, boundary.end_rip) {
            CompileAt::Block(block) => block,
            CompileAt::BoundaryReached => {
                return Ok(result(
                    BoundedMemoryExit::BoundaryReached { rip: state.rip },
                    remaining,
                    retired,
                    blocks,
                ));
            }
            CompileAt::Unsupported => {
                return Ok(result(
                    BoundedMemoryExit::Unsupported { rip: state.rip },
                    remaining,
                    retired,
                    blocks,
                ));
            }
            CompileAt::Invalid => {
                return Ok(result(
                    BoundedMemoryExit::Invalid { rip: state.rip },
                    remaining,
                    retired,
                    blocks,
                ));
            }
        };
        let instruction_count = match &block {
            BoundedMemoryArtifact::Pair(_) => 2,
            BoundedMemoryArtifact::Load(_) | BoundedMemoryArtifact::Store(_) => 1,
        };
        let prior_rip = state.rip;
        // SAFETY: forwarded unchanged from this function's documented caller contract.
        let incoming_budget = remaining;
        let report = unsafe {
            executor.execute_one_memory_block_controlled(
                &block,
                state,
                memory,
                access_context,
                incoming_budget,
                stop_rip,
                stop_requested,
                event_requested,
            )
        }
        .map_err(BoundedMemoryError::Executor)?;
        if state.rip > boundary.end_rip {
            return Err(BoundedMemoryError::InvalidEntryRip {
                rip: state.rip,
                code_start: code.base_rip(),
                execution_end: boundary.end_rip,
            });
        }
        if report.retired_instructions > remaining
            || report.retired_instructions > instruction_count
            || remaining - report.retired_instructions != report.remaining_budget
        {
            return Err(BoundedMemoryError::InvalidAccounting {
                incoming: incoming_budget,
                report,
            });
        }

        if report.exit == MemoryBlockExit::EventPending {
            if report.retired_instructions != 0
                || report.remaining_budget != incoming_budget
                || state.rip != prior_rip
                || !event_requested.is_some_and(|request| request.load(Ordering::Acquire))
            {
                return Err(BoundedMemoryError::InvalidAccounting {
                    incoming: incoming_budget,
                    report,
                });
            }
            if stop_rip == Some(state.rip) {
                return Ok(result(
                    BoundedMemoryExit::StopAddress,
                    remaining,
                    retired,
                    blocks,
                ));
            }
            if stop_requested.load(Ordering::Acquire) != 0 {
                return Ok(result(
                    BoundedMemoryExit::StopRequested,
                    remaining,
                    retired,
                    blocks,
                ));
            }
            if state.rip == boundary.end_rip {
                return Ok(result(
                    BoundedMemoryExit::BoundaryReached { rip: state.rip },
                    remaining,
                    retired,
                    blocks,
                ));
            }
            if remaining == 0 {
                return Ok(result(
                    BoundedMemoryExit::BudgetExhausted,
                    remaining,
                    retired,
                    blocks,
                ));
            }
            return Ok(result(
                BoundedMemoryExit::EventPending,
                remaining,
                retired,
                blocks,
            ));
        }
        remaining = report.remaining_budget;
        retired = retired
            .checked_add(report.retired_instructions)
            .ok_or(BoundedMemoryError::CounterOverflow)?;
        blocks = blocks
            .checked_add(1)
            .ok_or(BoundedMemoryError::CounterOverflow)?;

        match report.exit {
            MemoryBlockExit::Complete => {
                if report.retired_instructions == 0 || state.rip == prior_rip {
                    return Err(BoundedMemoryError::InvalidBlockProgress { rip: state.rip });
                }
            }
            MemoryBlockExit::CodeInvalidated => {
                return Ok(result(
                    BoundedMemoryExit::CodeInvalidated,
                    remaining,
                    retired,
                    blocks,
                ));
            }
            MemoryBlockExit::BudgetExhausted => {
                if remaining != 0 {
                    return Err(BoundedMemoryError::InvalidAccounting {
                        incoming: incoming_budget,
                        report,
                    });
                }
                return Ok(result(
                    BoundedMemoryExit::BudgetExhausted,
                    remaining,
                    retired,
                    blocks,
                ));
            }
            MemoryBlockExit::StopRequested => {
                return Ok(result(
                    BoundedMemoryExit::StopRequested,
                    remaining,
                    retired,
                    blocks,
                ));
            }
            MemoryBlockExit::StopAddress => {
                return Ok(result(
                    BoundedMemoryExit::StopAddress,
                    remaining,
                    retired,
                    blocks,
                ));
            }
            MemoryBlockExit::EventPending => unreachable!("handled before accounting commit"),
            MemoryBlockExit::GuestFault(fault) => {
                return Ok(result(
                    BoundedMemoryExit::GuestFault(fault),
                    remaining,
                    retired,
                    blocks,
                ));
            }
        }
    }
}

/// Run the checked-memory fixture path with opt-in interpreter steps for the
/// decoder-confirmed bare `0x90` and exact bare `48 F7 D1` (`NOT RCX`). The
/// strict `run` entry remains unchanged and still returns `Unsupported` at
/// either instruction.
///
/// Each strict native run stops at its first unsupported instruction. When
/// that RIP decodes as one of the admitted exact instructions, the native executor has already
/// returned (and its host adapter must have dropped the published mapping)
/// before this function steps the immutable instruction bytes with the
/// existing interpreter. It then resumes native dispatch at the resulting
/// actual RIP with the remaining total budget. Every other unsupported form
/// remains unsupported.
///
/// # Safety
/// The same private synchronous ordinary RAM, immutable non-aliasing code,
/// all-or-nothing write, no-MMIO/no-concurrency/no-re-entry/no-unwind
/// preconditions as [`run`] apply for the entire call.
pub unsafe fn run_with_nop_fallback<E: MemoryBlockExecutor>(
    code: CodeView<'_>,
    boundary: FixtureExecutionBoundary,
    state: &mut LongModeState,
    memory: &mut dyn GuestMemory,
    access_context: AccessContext,
    total_budget: u64,
    stop_rip: Option<u64>,
    stop_requested: &AtomicU8,
    executor: &mut E,
) -> Result<BoundedMemoryFallbackResult, BoundedMemoryError<E::Error>> {
    // SAFETY: forwards the caller contract with event polling disabled.
    unsafe {
        run_with_nop_fallback_controlled(
            code,
            boundary,
            state,
            memory,
            access_context,
            total_budget,
            stop_rip,
            stop_requested,
            None,
            executor,
        )
    }
}

/// Event-aware version of [`run_with_nop_fallback`]. An event is checked
/// before each native artifact and before each admitted interpreter step.
/// The caller owns clearing the request and resuming at the actual guest RIP.
///
/// # Safety
/// Has the same requirements as [`run_with_nop_fallback`]. The request owner
/// must not clear the event while this call runs.
pub unsafe fn run_with_nop_fallback_controlled<E: MemoryBlockExecutor>(
    code: CodeView<'_>,
    boundary: FixtureExecutionBoundary,
    state: &mut LongModeState,
    memory: &mut dyn GuestMemory,
    access_context: AccessContext,
    total_budget: u64,
    stop_rip: Option<u64>,
    stop_requested: &AtomicU8,
    event_requested: Option<&AtomicBool>,
    executor: &mut E,
) -> Result<BoundedMemoryFallbackResult, BoundedMemoryError<E::Error>> {
    let mut remaining = total_budget;
    let mut retired_native = 0u64;
    let mut retired_interpreter = 0u64;
    let mut blocks_executed = 0u64;

    loop {
        // SAFETY: forwarded unchanged from this function's caller contract.
        let native = unsafe {
            run_controlled(
                code,
                boundary.clone(),
                state,
                memory,
                access_context,
                remaining,
                stop_rip,
                stop_requested,
                event_requested,
                executor,
            )
        }?;
        remaining = native.remaining_budget;
        retired_native = retired_native
            .checked_add(native.retired_instructions)
            .ok_or(BoundedMemoryError::CounterOverflow)?;
        blocks_executed = blocks_executed
            .checked_add(native.blocks_executed)
            .ok_or(BoundedMemoryError::CounterOverflow)?;
        validate_fallback_accounting(total_budget, remaining, retired_native, retired_interpreter)?;

        let BoundedMemoryExit::Unsupported { rip } = native.exit else {
            return Ok(BoundedMemoryFallbackResult {
                exit: native.exit,
                remaining_budget: remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            });
        };
        if rip != state.rip {
            return Ok(BoundedMemoryFallbackResult {
                exit: BoundedMemoryExit::Unsupported { rip },
                remaining_budget: remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            });
        }
        let Some(instruction_length) =
            exact_fallback_instruction_length(code, boundary.end_rip, rip)
        else {
            return Ok(BoundedMemoryFallbackResult {
                exit: BoundedMemoryExit::Unsupported { rip },
                remaining_budget: remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            });
        };

        // `run` has already sampled stop-RIP, STOP, byte boundary and budget
        // before reporting Unsupported. Keep explicit guards here as well so
        // this bridge remains safe if its entry is refactored independently.
        if stop_rip == Some(state.rip) {
            return Ok(BoundedMemoryFallbackResult {
                exit: BoundedMemoryExit::StopAddress,
                remaining_budget: remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            });
        }
        if stop_requested.load(Ordering::Acquire) != 0 {
            return Ok(BoundedMemoryFallbackResult {
                exit: BoundedMemoryExit::StopRequested,
                remaining_budget: remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            });
        }
        if state.rip == boundary.end_rip {
            return Ok(BoundedMemoryFallbackResult {
                exit: BoundedMemoryExit::BoundaryReached { rip: state.rip },
                remaining_budget: remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            });
        }
        if remaining == 0 {
            return Ok(BoundedMemoryFallbackResult {
                exit: BoundedMemoryExit::BudgetExhausted,
                remaining_budget: remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            });
        }
        if event_requested.is_some_and(|request| request.load(Ordering::Acquire)) {
            return Ok(BoundedMemoryFallbackResult {
                exit: BoundedMemoryExit::EventPending,
                remaining_budget: remaining,
                retired_native,
                retired_interpreter,
                blocks_executed,
            });
        }

        let instruction_end = rip
            .checked_add(u64::from(instruction_length))
            .ok_or(BoundedMemoryError::CounterOverflow)?;
        let instruction_memory = CodeMemory::bounded_range(code, rip, instruction_end).ok_or(
            BoundedMemoryError::InterpreterMemory {
                rip,
                error: MemoryError::Unmapped {
                    address: rip,
                    access: AccessKind::Fetch,
                },
            },
        )?;
        // SAFETY: this operation is the same borrowed private-RAM invocation
        // guarded by this function's caller contract; admitted NOP and NOT
        // RCX forms access registers only and use the immutable code view.
        let mut fetch = instruction_memory;
        match od_isa::execute::step(state, &mut fetch, access_context) {
            Ok(od_isa::execute::StepOutcome::Advanced) => {
                let expected = rip
                    .checked_add(u64::from(instruction_length))
                    .ok_or(BoundedMemoryError::CounterOverflow)?;
                if state.rip != expected {
                    return Err(BoundedMemoryError::InterpreterRipMismatch {
                        expected,
                        actual: state.rip,
                    });
                }
            }
            Ok(outcome) => {
                return Err(BoundedMemoryError::InterpreterStep { rip, outcome });
            }
            Err(error) => {
                return Err(BoundedMemoryError::InterpreterMemory { rip, error });
            }
        }
        remaining -= 1;
        retired_interpreter = retired_interpreter
            .checked_add(1)
            .ok_or(BoundedMemoryError::CounterOverflow)?;
        validate_fallback_accounting(total_budget, remaining, retired_native, retired_interpreter)?;
    }
}

fn exact_fallback_instruction_length(
    code: CodeView<'_>,
    execution_end: u64,
    rip: u64,
) -> Option<u8> {
    if rip >= execution_end {
        return None;
    }
    let mut fetch = CodeMemory::bounded_range(code, rip, rip.saturating_add(3).min(execution_end))?;
    let decoded = long_decode::decode(&mut fetch, rip, AccessContext::default()).ok()?;
    let Some(offset) = rip
        .checked_sub(code.base_rip())
        .and_then(|offset| usize::try_from(offset).ok())
    else {
        return None;
    };
    let bytes = code.bytes().get(offset..)?;
    if crate::block::is_exact_bare_nop(decoded, *bytes.first()?) {
        Some(1)
    } else if crate::block::is_exact_not_rcx(decoded, bytes) {
        Some(3)
    } else {
        None
    }
}

fn validate_fallback_accounting<E>(
    total_budget: u64,
    remaining_budget: u64,
    retired_native: u64,
    retired_interpreter: u64,
) -> Result<(), BoundedMemoryError<E>> {
    let retired = retired_native
        .checked_add(retired_interpreter)
        .ok_or(BoundedMemoryError::CounterOverflow)?;
    if total_budget.checked_sub(remaining_budget) != Some(retired) {
        return Err(BoundedMemoryError::InvalidTotalAccounting {
            total_budget,
            remaining_budget,
            retired_native,
            retired_interpreter,
        });
    }
    Ok(())
}

enum CompileAt {
    Block(BoundedMemoryArtifact),
    BoundaryReached,
    Unsupported,
    Invalid,
}

fn compile_at(code: CodeView<'_>, rip: u64, execution_end: u64) -> CompileAt {
    let Some(offset) = rip
        .checked_sub(code.base_rip())
        .and_then(|offset| usize::try_from(offset).ok())
    else {
        return CompileAt::Unsupported;
    };
    let Some(bytes) = code.bytes().get(offset..) else {
        return CompileAt::BoundaryReached;
    };
    let available = usize::try_from(execution_end.saturating_sub(rip))
        .unwrap_or(usize::MAX)
        .min(bytes.len());
    let bytes = &bytes[..available];
    let Ok(view) = CodeView::new(rip, bytes) else {
        return CompileAt::Unsupported;
    };
    let mut source = CodeMemory::full_code(view);
    let decoded = match long_decode::decode(&mut source, rip, AccessContext::default()) {
        Ok(decoded) => decoded,
        Err(DecodeError::Unsupported { .. }) => return CompileAt::Unsupported,
        Err(DecodeError::InvalidMovCr3Lock { .. }) => return CompileAt::Unsupported,
        Err(DecodeError::InvalidMovCr0Lock { .. }) => return CompileAt::Unsupported,
        Err(DecodeError::InvalidLgdtLock { .. }) => return CompileAt::Unsupported,
        Err(DecodeError::InvalidLidtLock { .. }) => return CompileAt::Unsupported,
        Err(DecodeError::Invalid { .. }) => return CompileAt::Invalid,
        Err(DecodeError::Memory(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })) if address >= execution_end => return CompileAt::BoundaryReached,
        Err(DecodeError::Memory(_)) => return CompileAt::Unsupported,
    };
    if decoded.length == 0 {
        return CompileAt::Invalid;
    }
    if rip
        .checked_add(6)
        .is_some_and(|pair_end| pair_end <= execution_end)
        && bytes.len() >= 6
        && matches!(
            bytes.get(..6).unwrap(),
            [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b]
                | [0x48, 0x89, 0x03, 0x48, 0x89, 0x02]
                | [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02]
                | [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b]
        )
    {
        let Ok(view) = CodeView::new(rip, &bytes[..6]) else {
            return CompileAt::Unsupported;
        };
        return compile_ir_memory_two_site_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Pair)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported);
    }
    let instruction_len = usize::from(decoded.length);
    if instruction_len > bytes.len() {
        return CompileAt::BoundaryReached;
    }
    if !matches!(instruction_len, 3 | 4 | 5 | 6 | 7 | 8 | 9) {
        return CompileAt::Unsupported;
    }
    let instruction = &bytes[..instruction_len];
    let Ok(view) = CodeView::new(rip, instruction) else {
        return CompileAt::Unsupported;
    };
    // The bounded S3-29 profile admits this exact register-indirect qword
    // load. Keep the form exact; the generic memory profile stays unchanged.
    if instruction == [0x48, 0x8b, 0x03] {
        return compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported);
    }
    if instruction_len == 7
        && instruction[0] == 0x48
        && matches!(instruction[1], 0x8b | 0x89)
        && instruction[2] >> 6 == 0
        && instruction[2] & 7 == 5
        && matches!((instruction[2] >> 3) & 7, 6 | 7)
    {
        return if instruction[1] == 0x8b {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        } else {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        };
    }
    if matches!(instruction_len, 4 | 5 | 8)
        && instruction.starts_with(&[0x48])
        && matches!(instruction[1], 0x8b | 0x89)
    {
        let modrm = instruction[2];
        let expected_mode = match instruction_len {
            4 => 0,
            5 => 1,
            8 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && matches!((modrm >> 3) & 7, 6 | 7)
            && instruction[3] == 0x24
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction_len, 4 | 7)
        && instruction[0] == 0x48
        && matches!(instruction[1], 0x8b | 0x89)
    {
        let modrm = instruction[2];
        let expected_mode = if instruction_len == 4 { 1 } else { 2 };
        if modrm >> 6 == expected_mode
            && (modrm & 7 < 4 || modrm & 7 == 5)
            && matches!((modrm >> 3) & 7, 6 | 7)
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction_len, 4 | 5 | 8)
        && instruction.starts_with(&[0x67, 0x48])
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let expected_mode = match instruction.len() {
            4 => 0,
            5 => 1,
            8 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode && modrm & 7 < 4 && matches!((modrm >> 3) & 7, 6 | 7) {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 5 | 6 | 9)
        && instruction.starts_with(&[0x67, 0x48])
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let expected_mode = match instruction.len() {
            5 => 0,
            6 => 1,
            9 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && matches!(instruction[4], 0x20..=0x23)
        {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 6 | 9)
        && instruction.starts_with(&[0x67, 0x48])
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let expected_mode = if instruction.len() == 6 { 1 } else { 2 };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && instruction[4] == 0x25
        {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 4 | 5 | 8)
        && instruction[0] == 0x48
        && matches!(instruction[1], 0x8b | 0x89)
    {
        let modrm = instruction[2];
        let expected_mode = match instruction.len() {
            4 => 0,
            5 => 1,
            8 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && matches!(instruction[3], 0x20..=0x23)
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 4 | 5 | 8)
        && instruction[0] == 0x48
        && matches!(instruction[1], 0x8b | 0x89)
    {
        let modrm = instruction[2];
        let expected_mode = match instruction.len() {
            4 => 0,
            5 => 1,
            8 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && matches!(instruction[3], 0x26 | 0x27)
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 5 | 6 | 9)
        && instruction[0] == 0x67
        && instruction[1] == 0x48
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let expected_mode = match instruction.len() {
            5 => 0,
            6 => 1,
            9 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && matches!(instruction[4], 0x26 | 0x27)
        {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 5 | 8)
        && instruction[0] == 0x48
        && matches!(instruction[1], 0x8b | 0x89)
    {
        let modrm = instruction[2];
        let expected_mode = if instruction.len() == 5 { 1 } else { 2 };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && instruction[3] == 0x25
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 4 | 5 | 8)
        && instruction[0] == 0x48
        && matches!(instruction[1], 0x8b | 0x89)
    {
        let modrm = instruction[2];
        let sib = instruction[3];
        let expected_mode = match instruction.len() {
            4 => 0,
            5 => 1,
            8 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && sib & 7 == 4
            && matches!((sib >> 3) & 7, 6 | 7)
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 4 | 5 | 8)
        && instruction[0] == 0x48
        && matches!(instruction[1], 0x8b | 0x89)
    {
        let modrm = instruction[2];
        let sib = instruction[3];
        let expected_mode = match instruction.len() {
            4 => 0,
            5 => 1,
            8 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && matches!(sib & 7, 6 | 7)
            && matches!((sib >> 3) & 7, 6 | 7)
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 5 | 8)
        && instruction[0] == 0x48
        && matches!(instruction[1], 0x8b | 0x89)
    {
        let modrm = instruction[2];
        let sib = instruction[3];
        let expected_mode = if instruction.len() == 5 { 1 } else { 2 };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && sib & 7 == 5
            && matches!((sib >> 3) & 7, 6 | 7)
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if instruction.len() == 8 && instruction[0] == 0x48 && matches!(instruction[1], 0x8b | 0x89) {
        let modrm = instruction[2];
        let sib = instruction[3];
        if modrm >> 6 == 0
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && sib & 7 == 5
            && matches!((sib >> 3) & 7, 6 | 7)
        {
            return if instruction[1] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 5 | 6 | 9)
        && instruction[0] == 0x67
        && instruction[1] == 0x48
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let sib = instruction[4];
        let expected_mode = match instruction.len() {
            5 => 0,
            6 => 1,
            9 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && matches!(sib & 7, 6 | 7)
            && (sib >> 3) & 7 == 5
        {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 6 | 9)
        && instruction[0] == 0x67
        && instruction[1] == 0x48
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let sib = instruction[4];
        let expected_mode = if instruction.len() == 6 { 1 } else { 2 };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && sib & 7 == 5
            && matches!((sib >> 3) & 7, 6 | 7)
        {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 5 | 6 | 9)
        && instruction[0] == 0x67
        && instruction[1] == 0x48
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let sib = instruction[4];
        let expected_mode = match instruction.len() {
            5 => 0,
            6 => 1,
            9 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && sib & 7 == 4
            && matches!((sib >> 3) & 7, 6 | 7)
        {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 5 | 6 | 9)
        && instruction[0] == 0x67
        && instruction[1] == 0x48
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let sib = instruction[4];
        let expected_mode = match instruction.len() {
            5 => 0,
            6 => 1,
            9 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && sib & 7 < 4
            && matches!((sib >> 3) & 7, 6 | 7)
        {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    if matches!(instruction.len(), 5 | 6 | 9)
        && instruction[0] == 0x67
        && instruction[1] == 0x48
        && matches!(instruction[2], 0x8b | 0x89)
    {
        let modrm = instruction[3];
        let sib = instruction[4];
        let expected_mode = match instruction.len() {
            5 => 0,
            6 => 1,
            9 => 2,
            _ => unreachable!(),
        };
        if modrm >> 6 == expected_mode
            && modrm & 7 == 4
            && (modrm >> 3) & 7 <= 5
            && matches!(sib & 7, 6 | 7)
            && matches!((sib >> 3) & 7, 6 | 7)
        {
            return if instruction[2] == 0x8b {
                compile_ir_memory_load_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Load)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            } else {
                compile_ir_memory_store_block_v2(view, rip)
                    .map(BoundedMemoryArtifact::Store)
                    .map(CompileAt::Block)
                    .unwrap_or(CompileAt::Unsupported)
            };
        }
    }
    match instruction {
        [0x67, 0x48, 0x8b, modrm]
            if modrm >> 6 == 0
                && (modrm & 7 < 4 || matches!(modrm & 7, 6 | 7))
                && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm]
            if modrm >> 6 == 0
                && (modrm & 7 < 4 || matches!(modrm & 7, 6 | 7))
                && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (*sib == 0x24
                    || (sib & 7 < 4 || sib & 7 == 4) && (sib >> 3) & 7 < 4
                    || matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 < 4
                    || sib & 7 < 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 5 && (sib >> 3) & 7 == 5) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (*sib == 0x24
                    || (sib & 7 < 4 || sib & 7 == 4) && (sib >> 3) & 7 < 4
                    || matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 < 4
                    || sib & 7 < 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 5 && (sib >> 3) & 7 == 5) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, sib, _, _, _, _]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 == 5
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, sib, _, _, _, _]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 == 5
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (*sib == 0x24
                    || (sib & 7 < 4 || sib & 7 == 4 || sib & 7 == 5) && (sib >> 3) & 7 < 4
                    || matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 < 4
                    || sib & 7 < 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 5 && (sib >> 3) & 7 == 5) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (*sib == 0x24
                    || (sib & 7 < 4 || sib & 7 == 4 || sib & 7 == 5) && (sib >> 3) & 7 < 4
                    || matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 < 4
                    || sib & 7 < 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 5 && (sib >> 3) & 7 == 5) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (*sib == 0x24
                    || (sib & 7 < 4 || sib & 7 == 4 || sib & 7 == 5) && (sib >> 3) & 7 < 4
                    || matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 < 4
                    || sib & 7 < 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 5 && (sib >> 3) & 7 == 5) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (*sib == 0x24
                    || (sib & 7 < 4 || sib & 7 == 4 || sib & 7 == 5) && (sib >> 3) & 7 < 4
                    || matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 < 4
                    || sib & 7 < 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 4 && (sib >> 3) & 7 == 5
                    || sib & 7 == 5 && (sib >> 3) & 7 == 5) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, _disp]
            if modrm >> 6 == 1
                && (modrm & 7 < 4 || matches!(modrm & 7, 5 | 6 | 7))
                && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, _disp]
            if modrm >> 6 == 1
                && (modrm & 7 < 4 || matches!(modrm & 7, 5 | 6 | 7))
                && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, _, _, _, _]
            if modrm >> 6 == 2
                && (modrm & 7 < 4 || matches!(modrm & 7, 5 | 6 | 7))
                && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, _, _, _, _]
            if modrm >> 6 == 2
                && (modrm & 7 < 4 || matches!(modrm & 7, 5 | 6 | 7))
                && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, 0x25, _, _, _, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0x25, _, _, _, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0x2d, _, _, _, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0x2d, _, _, _, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, modrm, 0x25, _, _, _, _disp]
            if modrm >> 6 == 0 && modrm & 7 == 4 && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, 0x25, _, _, _, _disp]
            if modrm >> 6 == 0 && modrm & 7 == 4 && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 < 4
                && matches!((sib >> 3) & 7, 6 | 7) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 < 4
                && matches!((sib >> 3) & 7, 6 | 7) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 < 4
                && matches!((sib >> 3) & 7, 6 | 7) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 < 4
                && matches!((sib >> 3) & 7, 6 | 7) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 < 4
                && matches!((sib >> 3) & 7, 6 | 7) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 < 4
                && matches!((sib >> 3) & 7, 6 | 7) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (sib & 7 < 4 || sib & 7 == 4)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (sib & 7 < 4 || sib & 7 == 4)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (sib & 7 < 4 || sib & 7 == 4 || sib & 7 == 5)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (sib & 7 < 4 || sib & 7 == 4 || sib & 7 == 5)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (sib & 7 < 4 || sib & 7 == 4 || sib & 7 == 5)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (sib & 7 < 4 || sib & 7 == 4 || sib & 7 == 5)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 == 5
                && ((sib >> 3) & 7 < 4 || (sib >> 3) & 7 == 5) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 == 5
                && ((sib >> 3) & 7 < 4 || (sib >> 3) & 7 == 5) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, 0x25, _, _, _, _disp]
            if modrm >> 6 == 0 && modrm & 7 == 4 && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, 0x25, _, _, _, _disp]
            if modrm >> 6 == 0 && modrm & 7 == 4 && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 == 5
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 == 5
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x8b, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 == 5
                && matches!((sib >> 3) & 7, 6 | 7) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x67, 0x48, 0x89, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 == 5
                && matches!((sib >> 3) & 7, 6 | 7) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, _] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, _] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0x24, 0x24] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0x24, 0x24] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0x2c, 0x24] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0x2c, 0x24] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0x6c, 0x24, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0x6c, 0x24, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0xac, 0x24, _, _, _, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0xac, 0x24, _, _, _, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, modrm, _disp]
            if modrm >> 6 == 1 && modrm & 7 < 4 && matches!((modrm >> 3) & 7, 4 | 5) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, _disp]
            if modrm >> 6 == 1 && modrm & 7 < 4 && matches!((modrm >> 3) & 7, 4 | 5) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, _, _, _, _disp]
            if modrm >> 6 == 2 && modrm & 7 < 4 && matches!((modrm >> 3) & 7, 4 | 5) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, _, _, _, _disp]
            if modrm >> 6 == 2 && modrm & 7 < 4 && matches!((modrm >> 3) & 7, 4 | 5) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, 0x65, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0x65, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0xa5, _, _, _, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0xa5, _, _, _, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0x64, 0x24, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0x64, 0x24, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0xa4, 0x24, _, _, _, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0xa4, 0x24, _, _, _, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (sib & 7 <= 4 || matches!(sib & 7, 6 | 7))
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib]
            if modrm >> 6 == 0
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && (sib & 7 <= 4 || matches!(sib & 7, 6 | 7))
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, 0x24] if modrm >> 6 == 0 && modrm & 7 == 4 && (modrm >> 3) & 7 < 4 => {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, 0x24] if modrm >> 6 == 0 && modrm & 7 == 4 && (modrm >> 3) & 7 < 4 => {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, 0x24, _disp]
            if modrm >> 6 == 1 && modrm & 7 == 4 && (modrm >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, 0x24, _disp]
            if modrm >> 6 == 1 && modrm & 7 == 4 && (modrm >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 <= 5
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _disp]
            if modrm >> 6 == 1
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 <= 5
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, 0x24, _, _, _, _]
            if modrm >> 6 == 2 && modrm & 7 == 4 && (modrm >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, 0x24, _, _, _, _]
            if modrm >> 6 == 2 && modrm & 7 == 4 && (modrm >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _, _, _, _]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 <= 5
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _, _, _, _]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && sib & 7 <= 5
                && (sib >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, sib, _, _, _, _disp]
            if modrm >> 6 == 2
                && modrm & 7 == 4
                && (modrm >> 3) & 7 <= 5
                && matches!(sib & 7, 6 | 7)
                && (sib >> 3) & 7 == 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, _] if modrm >> 6 == 1 && modrm & 7 <= 3 && (modrm >> 3) & 7 <= 3 => {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, _] if modrm >> 6 == 1 && modrm & 7 <= 3 && (modrm >> 3) & 7 <= 3 => {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, _] if modrm >> 6 == 1 && modrm & 7 == 5 && (modrm >> 3) & 7 < 4 => {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, _] if modrm >> 6 == 1 && modrm & 7 == 5 && (modrm >> 3) & 7 < 4 => {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, _disp]
            if modrm >> 6 == 1 && matches!(modrm & 7, 6 | 7) && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, _disp]
            if modrm >> 6 == 1 && matches!(modrm & 7, 6 | 7) && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, 0x6d, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0x6d, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, 0xad, _, _, _, _disp] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, 0xad, _, _, _, _disp] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x8b, modrm, _, _, _, _]
            if modrm >> 6 == 2 && modrm & 7 == 5 && (modrm >> 3) & 7 < 4 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, _, _, _, _]
            if modrm >> 6 == 2 && modrm & 7 == 5 && (modrm >> 3) & 7 < 4 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, _, _, _, _]
            if (modrm >> 6 == 2 && modrm & 7 <= 3 && (modrm >> 3) & 7 <= 3)
                || (modrm >> 6 == 0 && modrm & 7 == 5 && (modrm >> 3) & 7 <= 3) =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, _, _, _, _]
            if (modrm >> 6 == 2 && modrm & 7 <= 3 && (modrm >> 3) & 7 <= 3)
                || (modrm >> 6 == 0 && modrm & 7 == 5 && (modrm >> 3) & 7 <= 3) =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x8b, modrm, _, _, _, _]
            if modrm >> 6 == 2 && matches!(modrm & 7, 6 | 7) && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_load_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Load)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        [0x48, 0x89, modrm, _, _, _, _]
            if modrm >> 6 == 2 && matches!(modrm & 7, 6 | 7) && (modrm >> 3) & 7 <= 5 =>
        {
            compile_ir_memory_store_block_v2(view, rip)
                .map(BoundedMemoryArtifact::Store)
                .map(CompileAt::Block)
                .unwrap_or(CompileAt::Unsupported)
        }
        _ => CompileAt::Unsupported,
    }
}

fn result(
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

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyMemory;
    impl GuestMemory for EmptyMemory {
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

    #[derive(Default, Clone, Debug, Eq, PartialEq)]
    struct MemoryCallCounts {
        fetch: usize,
        read: usize,
        write: usize,
        atomic_exchange: usize,
        atomic_fetch_add: usize,
        atomic_compare_exchange: usize,
        mfence: usize,
    }

    #[derive(Default)]
    struct MemorySpy {
        calls: MemoryCallCounts,
    }

    impl GuestMemory for MemorySpy {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.calls.fetch += 1;
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
            self.calls.read += 1;
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })
        }

        fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            self.calls.write += 1;
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
            self.calls.atomic_exchange += 1;
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
            self.calls.atomic_fetch_add += 1;
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
            self.calls.atomic_compare_exchange += 1;
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            })
        }

        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            self.calls.mfence += 1;
            Ok(())
        }
    }

    #[derive(Default)]
    struct PairContinuationExecutor {
        entries: Vec<(u64, bool)>,
    }
    unsafe impl MemoryBlockExecutor for PairContinuationExecutor {
        type Error = ();

        unsafe fn execute_one_memory_block(
            &mut self,
            block: &BoundedMemoryArtifact,
            state: &mut LongModeState,
            _: &mut dyn GuestMemory,
            _: AccessContext,
            incoming_budget: u64,
            _: Option<u64>,
            _: &AtomicU8,
        ) -> Result<MemoryBlockReport, Self::Error> {
            let pair = matches!(block, BoundedMemoryArtifact::Pair(_));
            self.entries.push((state.rip, pair));
            let retired = if pair {
                incoming_budget.min(2)
            } else {
                incoming_budget.min(1)
            };
            state.rip += retired * 3;
            Ok(MemoryBlockReport {
                exit: if pair && incoming_budget == retired {
                    MemoryBlockExit::BudgetExhausted
                } else {
                    MemoryBlockExit::Complete
                },
                remaining_budget: incoming_budget - retired,
                retired_instructions: retired,
            })
        }
    }

    #[test]
    fn uncached_dispatch_selects_store_store_pair_and_resumes_at_second_site() {
        const ENTRY: u64 = 0x8000;
        let source = [0x48, 0x89, 0x03, 0x48, 0x89, 0x02];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: ENTRY,
            ..Default::default()
        };
        let mut memory = EmptyMemory;
        let stop = AtomicU8::new(0);
        let mut executor = PairContinuationExecutor::default();

        let first = unsafe {
            run(
                code,
                boundary.clone(),
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(first.exit, BoundedMemoryExit::BudgetExhausted);
        assert_eq!(first.retired_instructions, 1);
        assert_eq!(state.rip, ENTRY + 3);
        assert_eq!(executor.entries, [(ENTRY, true)]);

        let resumed = unsafe {
            run(
                code,
                boundary,
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(
            resumed.exit,
            BoundedMemoryExit::BoundaryReached { rip: ENTRY + 6 }
        );
        assert_eq!(resumed.retired_instructions, 1);
        assert_eq!(state.rip, ENTRY + 6);
        assert_eq!(executor.entries, [(ENTRY, true), (ENTRY + 3, false)]);
    }

    #[derive(Default)]
    struct AdvancingExecutor {
        entries: Vec<(u64, usize)>,
        states_after_entry: Vec<LongModeState>,
        request_event_after_entry: bool,
        request_stop_after_entry: bool,
        invalidate_after_entry: bool,
    }
    unsafe impl MemoryBlockExecutor for AdvancingExecutor {
        type Error = ();

        unsafe fn execute_one_memory_block(
            &mut self,
            block: &BoundedMemoryArtifact,
            state: &mut LongModeState,
            _: &mut dyn GuestMemory,
            _: AccessContext,
            incoming_budget: u64,
            _: Option<u64>,
            _: &AtomicU8,
        ) -> Result<MemoryBlockReport, Self::Error> {
            let (site, source) = match block {
                BoundedMemoryArtifact::Load(block) => (block.site(), block.source_bytes()),
                BoundedMemoryArtifact::Store(block) => (block.site(), block.source_bytes()),
                BoundedMemoryArtifact::Pair(_) => return Err(()),
            };
            self.entries.push((state.rip, source.len()));
            state.rip = site.next_rip();
            self.states_after_entry.push(state.clone());
            Ok(MemoryBlockReport {
                exit: if self.invalidate_after_entry {
                    MemoryBlockExit::CodeInvalidated
                } else {
                    MemoryBlockExit::Complete
                },
                remaining_budget: incoming_budget - 1,
                retired_instructions: 1,
            })
        }

        unsafe fn execute_one_memory_block_controlled(
            &mut self,
            block: &BoundedMemoryArtifact,
            state: &mut LongModeState,
            memory: &mut dyn GuestMemory,
            access_context: AccessContext,
            incoming_budget: u64,
            stop_rip: Option<u64>,
            stop_requested: &AtomicU8,
            event_requested: Option<&AtomicBool>,
        ) -> Result<MemoryBlockReport, Self::Error> {
            // SAFETY: this mock performs exactly one bounded artifact and only
            // raises the request after its guest state/accounting has committed.
            let report = unsafe {
                self.execute_one_memory_block(
                    block,
                    state,
                    memory,
                    access_context,
                    incoming_budget,
                    stop_rip,
                    stop_requested,
                )?
            };
            if self.request_event_after_entry {
                if let Some(event_requested) = event_requested {
                    event_requested.store(true, Ordering::Release);
                }
            }
            if self.request_stop_after_entry {
                stop_requested.store(1, Ordering::Release);
            }
            Ok(report)
        }
    }

    #[derive(Default)]
    struct PublishBoundaryExecutor {
        publications: usize,
        entries: usize,
        request_stop_after_publish: bool,
    }
    unsafe impl MemoryBlockExecutor for PublishBoundaryExecutor {
        type Error = ();

        unsafe fn execute_one_memory_block(
            &mut self,
            _: &BoundedMemoryArtifact,
            _: &mut LongModeState,
            _: &mut dyn GuestMemory,
            _: AccessContext,
            _: u64,
            _: Option<u64>,
            _: &AtomicU8,
        ) -> Result<MemoryBlockReport, Self::Error> {
            self.entries += 1;
            Err(())
        }

        unsafe fn execute_one_memory_block_controlled(
            &mut self,
            _: &BoundedMemoryArtifact,
            _: &mut LongModeState,
            _: &mut dyn GuestMemory,
            _: AccessContext,
            incoming_budget: u64,
            _: Option<u64>,
            stop_requested: &AtomicU8,
            event_requested: Option<&AtomicBool>,
        ) -> Result<MemoryBlockReport, Self::Error> {
            // Represents publication completing before the event sample. The
            // old entrypoint is deliberately not called on this path.
            self.publications += 1;
            if self.request_stop_after_publish {
                stop_requested.store(1, Ordering::Release);
            }
            if let Some(event) = event_requested {
                event.store(true, Ordering::Release);
            }
            Ok(MemoryBlockReport {
                // The dispatcher applies its stop-before-event priority after
                // this simulated post-publication event sample.
                exit: MemoryBlockExit::EventPending,
                remaining_budget: incoming_budget,
                retired_instructions: 0,
            })
        }
    }

    struct MalformedEventExecutor;
    unsafe impl MemoryBlockExecutor for MalformedEventExecutor {
        type Error = ();

        unsafe fn execute_one_memory_block(
            &mut self,
            _: &BoundedMemoryArtifact,
            _: &mut LongModeState,
            _: &mut dyn GuestMemory,
            _: AccessContext,
            incoming_budget: u64,
            _: Option<u64>,
            _: &AtomicU8,
        ) -> Result<MemoryBlockReport, Self::Error> {
            Ok(MemoryBlockReport {
                exit: MemoryBlockExit::EventPending,
                remaining_budget: incoming_budget - 1,
                retired_instructions: 1,
            })
        }

        unsafe fn execute_one_memory_block_controlled(
            &mut self,
            _: &BoundedMemoryArtifact,
            _: &mut LongModeState,
            _: &mut dyn GuestMemory,
            _: AccessContext,
            incoming_budget: u64,
            _: Option<u64>,
            _: &AtomicU8,
            event_requested: Option<&AtomicBool>,
        ) -> Result<MemoryBlockReport, Self::Error> {
            if let Some(event) = event_requested {
                event.store(true, Ordering::Release);
            }
            Ok(MemoryBlockReport {
                exit: MemoryBlockExit::EventPending,
                remaining_budget: incoming_budget - 1,
                retired_instructions: 1,
            })
        }
    }

    #[test]
    fn cpu_owned_cr3_is_rejected_before_memory_block_execution() {
        const ENTRY: u64 = 0x99b0;
        let source = [0x48, 0x8b, 0x30];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let mut state = LongModeState {
            cr3: Some(0x2000),
            rip: ENTRY,
            ..LongModeState::default()
        };
        let mut memory = EmptyMemory;
        let stop = AtomicU8::new(0);
        let mut executor = AdvancingExecutor::default();
        let result = unsafe {
            run_controlled(
                code,
                boundary,
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
                None,
                &mut executor,
            )
        };
        assert_eq!(
            result,
            Err(BoundedMemoryError::UnsupportedCr3State { value: 0x2000 })
        );
        assert!(executor.entries.is_empty());
        assert_eq!(state.rip, ENTRY);
    }

    #[test]
    fn event_after_publication_skips_entry_and_stop_wins() {
        const ENTRY: u64 = 0x99c0;
        let source = [0x48, 0x8b, 0x30];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let mut memory = EmptyMemory;
        let initial = LongModeState {
            cr3: None,
            rip: ENTRY,
            ..Default::default()
        };

        for (stop_after_publish, expected) in [
            (false, BoundedMemoryExit::EventPending),
            (true, BoundedMemoryExit::StopRequested),
        ] {
            let stop = AtomicU8::new(0);
            let event = AtomicBool::new(false);
            let mut state = initial.clone();
            let mut executor = PublishBoundaryExecutor {
                request_stop_after_publish: stop_after_publish,
                ..Default::default()
            };
            let result = unsafe {
                run_controlled(
                    code,
                    boundary.clone(),
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    1,
                    None,
                    &stop,
                    Some(&event),
                    &mut executor,
                )
            }
            .unwrap();
            assert_eq!(result.exit, expected);
            assert_eq!(state, initial);
            assert_eq!(
                (result.remaining_budget, result.retired_instructions),
                (1, 0)
            );
            assert_eq!(result.blocks_executed, 0);
            assert_eq!((executor.publications, executor.entries), (1, 0));
            assert!(event.load(Ordering::Acquire));
        }
    }

    #[test]
    fn malformed_event_pending_executor_accounting_is_rejected() {
        const ENTRY: u64 = 0x99d0;
        let source = [0x48, 0x8b, 0x30];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let mut state = LongModeState {
            cr3: None,
            rip: ENTRY,
            ..Default::default()
        };
        let mut memory = EmptyMemory;
        let event = AtomicBool::new(false);
        let result = unsafe {
            run_controlled(
                code,
                boundary,
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &AtomicU8::new(0),
                Some(&event),
                &mut MalformedEventExecutor,
            )
        };
        assert!(matches!(
            result,
            Err(BoundedMemoryError::InvalidAccounting { .. })
        ));
        assert_eq!(state.rip, ENTRY);
    }

    #[test]
    fn controlled_event_yields_before_work_and_after_one_committed_artifact() {
        const ENTRY: u64 = 0x9a00;
        let source = [0x48, 0x8b, 0x30, 0x48, 0x8b, 0x30];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let stop_requested = AtomicU8::new(0);
        let event_requested = AtomicBool::new(true);
        let initial = LongModeState {
            cr3: None,
            rip: ENTRY,
            ..Default::default()
        };
        let mut state = initial.clone();
        let mut memory = EmptyMemory;
        let mut executor = AdvancingExecutor::default();

        let early = unsafe {
            run_controlled(
                code,
                boundary.clone(),
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop_requested,
                Some(&event_requested),
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(early.exit, BoundedMemoryExit::EventPending);
        assert_eq!(early.remaining_budget, 2);
        assert_eq!((early.retired_instructions, early.blocks_executed), (0, 0));
        assert_eq!(state, initial);
        assert!(executor.entries.is_empty());

        event_requested.store(false, Ordering::Release);
        executor.request_event_after_entry = true;
        let yielded = unsafe {
            run_controlled(
                code,
                boundary.clone(),
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop_requested,
                Some(&event_requested),
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(yielded.exit, BoundedMemoryExit::EventPending);
        assert_eq!(state.rip, ENTRY + 3);
        assert_eq!(yielded.remaining_budget, 1);
        assert_eq!(
            (yielded.retired_instructions, yielded.blocks_executed),
            (1, 1)
        );
        assert!(event_requested.load(Ordering::Acquire));
        assert_eq!(executor.entries, [(ENTRY, 3)]);

        event_requested.store(false, Ordering::Release);
        executor.request_event_after_entry = false;
        let resumed = unsafe {
            run_controlled(
                code,
                boundary.clone(),
                &mut state,
                &mut memory,
                AccessContext::default(),
                yielded.remaining_budget,
                None,
                &stop_requested,
                Some(&event_requested),
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(
            resumed.exit,
            BoundedMemoryExit::BoundaryReached {
                rip: code.code_end()
            }
        );
        assert_eq!(state.rip, code.code_end());
        assert_eq!(
            (resumed.remaining_budget, resumed.retired_instructions),
            (0, 1)
        );
        assert_eq!(executor.entries, [(ENTRY, 3), (ENTRY + 3, 3)]);

        let mut uninterrupted_state = initial;
        let mut uninterrupted_executor = AdvancingExecutor::default();
        let uninterrupted = unsafe {
            run(
                code,
                boundary,
                &mut uninterrupted_state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop_requested,
                &mut uninterrupted_executor,
            )
        }
        .unwrap();
        assert_eq!(uninterrupted.exit, resumed.exit);
        assert_eq!(uninterrupted_state, state);
        assert_eq!(uninterrupted_executor.entries, [(ENTRY, 3), (ENTRY + 3, 3)]);
    }

    #[test]
    fn completed_artifact_terminal_exit_and_stop_are_not_masked_by_event() {
        const ENTRY: u64 = 0x9a80;
        let source = [0x48, 0x8b, 0x30, 0x48, 0x8b, 0x30];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let mut memory = EmptyMemory;

        for (invalidate, stop_after, expected) in [
            (true, false, BoundedMemoryExit::CodeInvalidated),
            (false, true, BoundedMemoryExit::StopRequested),
        ] {
            let stop = AtomicU8::new(0);
            let event = AtomicBool::new(false);
            let mut state = LongModeState {
                cr3: None,
                rip: ENTRY,
                ..Default::default()
            };
            let mut executor = AdvancingExecutor {
                request_event_after_entry: true,
                request_stop_after_entry: stop_after,
                invalidate_after_entry: invalidate,
                ..Default::default()
            };
            let result = unsafe {
                run_controlled(
                    code,
                    boundary.clone(),
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    2,
                    None,
                    &stop,
                    Some(&event),
                    &mut executor,
                )
            }
            .unwrap();
            assert_eq!(result.exit, expected);
            assert_eq!(state.rip, ENTRY + 3);
            assert_eq!(
                (result.remaining_budget, result.retired_instructions),
                (1, 1)
            );
            assert_eq!(result.blocks_executed, 1);
            assert!(event.load(Ordering::Acquire));
            assert_eq!(executor.entries, [(ENTRY, 3)]);
        }
    }

    #[test]
    fn controlled_event_respects_stop_boundary_and_budget_priority() {
        const ENTRY: u64 = 0x9b00;
        let source = [0x48, 0x8b, 0x30];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let stop_requested = AtomicU8::new(0);
        let event_requested = AtomicBool::new(true);
        let mut memory = EmptyMemory;

        for (stop_rip, stop, end, budget, expected) in [
            (Some(ENTRY), 0, ENTRY + 3, 1, BoundedMemoryExit::StopAddress),
            (None, 1, ENTRY + 3, 1, BoundedMemoryExit::StopRequested),
            (
                None,
                0,
                ENTRY,
                1,
                BoundedMemoryExit::BoundaryReached { rip: ENTRY },
            ),
            (None, 0, ENTRY + 3, 0, BoundedMemoryExit::BudgetExhausted),
            (None, 0, ENTRY + 3, 1, BoundedMemoryExit::EventPending),
        ] {
            stop_requested.store(stop, Ordering::Release);
            let boundary = FixtureExecutionBoundary::new(code, end).unwrap();
            let mut state = LongModeState {
                cr3: None,
                rip: ENTRY,
                ..Default::default()
            };
            let mut executor = AdvancingExecutor::default();
            let result = unsafe {
                run_controlled(
                    code,
                    boundary,
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    budget,
                    stop_rip,
                    &stop_requested,
                    Some(&event_requested),
                    &mut executor,
                )
            }
            .unwrap();
            assert_eq!(result.exit, expected);
            assert_eq!(state.rip, ENTRY);
            assert!(executor.entries.is_empty());
        }
    }

    #[test]
    fn nop_fallback_event_yields_before_nop_and_resumes_without_replay() {
        const ENTRY: u64 = 0x9c00;
        let source = [0x48, 0x8b, 0x30, 0x90, 0x48, 0x8b, 0x30];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let stop_requested = AtomicU8::new(0);
        let event_requested = AtomicBool::new(false);
        let mut state = LongModeState {
            cr3: None,
            rip: ENTRY,
            ..Default::default()
        };
        let mut memory = EmptyMemory;
        let mut executor = AdvancingExecutor {
            request_event_after_entry: true,
            ..Default::default()
        };

        let yielded = unsafe {
            run_with_nop_fallback_controlled(
                code,
                boundary.clone(),
                &mut state,
                &mut memory,
                AccessContext::default(),
                3,
                None,
                &stop_requested,
                Some(&event_requested),
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(yielded.exit, BoundedMemoryExit::EventPending);
        assert_eq!(state.rip, ENTRY + 3);
        assert_eq!(
            (yielded.retired_native, yielded.retired_interpreter),
            (1, 0)
        );
        assert_eq!(yielded.remaining_budget, 2);
        assert_eq!(executor.entries, [(ENTRY, 3)]);

        event_requested.store(false, Ordering::Release);
        executor.request_event_after_entry = false;
        let resumed = unsafe {
            run_with_nop_fallback_controlled(
                code,
                boundary,
                &mut state,
                &mut memory,
                AccessContext::default(),
                yielded.remaining_budget,
                None,
                &stop_requested,
                Some(&event_requested),
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(
            resumed.exit,
            BoundedMemoryExit::BoundaryReached {
                rip: code.code_end()
            }
        );
        assert_eq!(state.rip, code.code_end());
        assert_eq!(
            (
                resumed.remaining_budget,
                resumed.retired_native,
                resumed.retired_interpreter
            ),
            (0, 1, 1)
        );
        assert_eq!(executor.entries, [(ENTRY, 3), (ENTRY + 4, 3)]);
    }

    #[test]
    fn not_rcx_fallback_is_exact_and_strict_run_stays_strict() {
        const ENTRY: u64 = 0x9d00;
        let source = [0x48, 0xf7, 0xd1];
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let initial = LongModeState {
            cr3: None,
            gpr: [
                0x1234_5678_9abc_def0,
                0x0123_4567_89ab_cdef,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
            rip: ENTRY,
            rflags: 0x0ed7,
            ..Default::default()
        };
        let stop = AtomicU8::new(0);
        let mut memory = EmptyMemory;

        let mut strict_state = initial.clone();
        let mut strict_executor = AdvancingExecutor::default();
        let strict = unsafe {
            run(
                code,
                boundary.clone(),
                &mut strict_state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
                &mut strict_executor,
            )
        }
        .unwrap();
        assert_eq!(strict.exit, BoundedMemoryExit::Unsupported { rip: ENTRY });
        assert_eq!(strict_state, initial);
        assert!(strict_executor.entries.is_empty());

        let mut expected = initial.clone();
        let mut interpreter_fetch = CodeMemory::full_code(code);
        assert_eq!(
            od_isa::execute::step(
                &mut expected,
                &mut interpreter_fetch,
                AccessContext::default()
            ),
            Ok(od_isa::execute::StepOutcome::Advanced)
        );

        let mut fallback_state = initial;
        let mut fallback_executor = AdvancingExecutor::default();
        let fallback = unsafe {
            run_with_nop_fallback(
                code,
                boundary,
                &mut fallback_state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
                &mut fallback_executor,
            )
        }
        .unwrap();
        assert_eq!(
            fallback.exit,
            BoundedMemoryExit::BoundaryReached { rip: ENTRY + 3 }
        );
        assert_eq!(fallback_state, expected);
        assert_eq!(
            (
                fallback.remaining_budget,
                fallback.retired_native,
                fallback.retired_interpreter
            ),
            (0, 0, 1)
        );
        assert!(fallback_executor.entries.is_empty());
    }

    #[test]
    fn lock_mov_cr0_stops_strict_dispatch_without_retirement_or_tail_execution() {
        const ENTRY: u64 = 0x9e00;
        const LOCK_MOV_CR0: [u8; 4] = [0xf0, 0x0f, 0x22, 0xc0];
        let stop = AtomicU8::new(0);
        let mut memory = MemorySpy::default();

        let mut at_start = LOCK_MOV_CR0.to_vec();
        at_start.extend([0x48, 0x89, 0x03]); // The supported store remains unreachable.
        let code = CodeView::new(ENTRY, &at_start).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let initial = LongModeState {
            rip: ENTRY,
            gpr: [0x5a5a; 16],
            rflags: 0x8d7,
            ..Default::default()
        };
        let mut state = initial.clone();
        let mut executor = AdvancingExecutor::default();
        let result = unsafe {
            run(
                code,
                boundary,
                &mut state,
                &mut memory,
                AccessContext::default(),
                2,
                None,
                &stop,
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(result.exit, BoundedMemoryExit::Unsupported { rip: ENTRY });
        assert_eq!(result.retired_instructions, 0);
        assert_eq!(result.remaining_budget, 2);
        assert_eq!(result.blocks_executed, 0);
        assert_eq!(state, initial);
        assert!(executor.entries.is_empty());
        assert_eq!(memory.calls, MemoryCallCounts::default());

        let mut after_prefix = vec![0x48, 0x8b, 0x03]; // Supported bounded load.
        after_prefix.extend(LOCK_MOV_CR0);
        after_prefix.extend([0x48, 0x89, 0x03]); // This store must not execute.
        let code = CodeView::new(ENTRY, &after_prefix).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let mut state = initial.clone();
        let mut executor = AdvancingExecutor::default();
        let result = unsafe {
            run(
                code,
                boundary,
                &mut state,
                &mut memory,
                AccessContext::default(),
                3,
                None,
                &stop,
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(
            result.exit,
            BoundedMemoryExit::Unsupported { rip: ENTRY + 3 }
        );
        assert_eq!(result.retired_instructions, 1);
        assert_eq!(result.remaining_budget, 2);
        assert_eq!(result.blocks_executed, 1);
        let mut expected_after_prefix = initial;
        expected_after_prefix.rip = ENTRY + 3;
        assert_eq!(executor.states_after_entry, [expected_after_prefix.clone()]);
        assert_eq!(state, expected_after_prefix);
        assert_eq!(executor.entries, [(ENTRY, 3)]);
        assert_eq!(memory.calls, MemoryCallCounts::default());
    }

    #[test]
    fn lock_mov_cr3_remains_unsupported_before_execution() {
        const ENTRY: u64 = 0x9f00;
        let mut source = vec![0xf0, 0x0f, 0x22, 0xd8];
        source.extend([0x48, 0x89, 0x03]);
        let code = CodeView::new(ENTRY, &source).unwrap();
        let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
        let initial = LongModeState {
            rip: ENTRY,
            ..Default::default()
        };
        let mut state = initial.clone();
        let mut memory = EmptyMemory;
        let mut executor = AdvancingExecutor::default();
        let result = unsafe {
            run(
                code,
                boundary,
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &AtomicU8::new(0),
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(result.exit, BoundedMemoryExit::Unsupported { rip: ENTRY });
        assert_eq!(result.retired_instructions, 0);
        assert_eq!(state, initial);
        assert!(executor.entries.is_empty());
    }

    #[test]
    fn invalid_unsupported_truncated_and_zero_budget_boundaries_remain_distinct() {
        const ENTRY: u64 = 0xa000;
        let cases: &[(&[u8], u64, BoundedMemoryExit)] = &[
            (
                &[0xf0, 0x0f, 0x20, 0xc0],
                1,
                BoundedMemoryExit::Invalid { rip: ENTRY },
            ),
            (
                &[0xf0, 0xf0, 0x0f, 0x22, 0xc0],
                1,
                BoundedMemoryExit::Unsupported { rip: ENTRY },
            ),
            (
                &[0x48],
                1,
                BoundedMemoryExit::BoundaryReached { rip: ENTRY },
            ),
            (&[0x48, 0x8b, 0x03], 0, BoundedMemoryExit::BudgetExhausted),
        ];

        for (source, budget, expected_exit) in cases {
            let code = CodeView::new(ENTRY, source).unwrap();
            let boundary = FixtureExecutionBoundary::new(code, code.code_end()).unwrap();
            let initial = LongModeState {
                rip: ENTRY,
                ..Default::default()
            };
            let mut state = initial.clone();
            let mut memory = EmptyMemory;
            let mut executor = AdvancingExecutor::default();
            let result = unsafe {
                run(
                    code,
                    boundary,
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    *budget,
                    None,
                    &AtomicU8::new(0),
                    &mut executor,
                )
            }
            .unwrap();
            assert_eq!(&result.exit, expected_exit, "source={source:02x?}");
            assert_eq!(result.retired_instructions, 0);
            assert_eq!(state, initial);
            assert!(executor.entries.is_empty());
        }
    }

    #[test]
    fn nop_fallback_handles_code_end_at_u64_max() {
        let base = u64::MAX - 1;
        let source = [0x90];
        let code = CodeView::new(base, &source).unwrap();
        assert_eq!(code.code_end(), u64::MAX);
        let boundary = FixtureExecutionBoundary::new(code, u64::MAX).unwrap();
        let stop = AtomicU8::new(0);
        let mut state = LongModeState {
            cr3: None,
            rip: base,
            ..Default::default()
        };
        let mut memory = EmptyMemory;
        let mut executor = AdvancingExecutor::default();

        let result = unsafe {
            run_with_nop_fallback(
                code,
                boundary,
                &mut state,
                &mut memory,
                AccessContext::default(),
                1,
                None,
                &stop,
                &mut executor,
            )
        }
        .unwrap();
        assert_eq!(
            result.exit,
            BoundedMemoryExit::BoundaryReached { rip: u64::MAX }
        );
        assert_eq!(state.rip, u64::MAX);
        assert_eq!(
            (
                result.remaining_budget,
                result.retired_native,
                result.retired_interpreter
            ),
            (0, 0, 1)
        );
    }

    #[test]
    fn rsp_sib_rsi_rdi_dispatch_respects_partial_windows_and_actual_rip() {
        const ENTRY: u64 = 0x9000;
        for opcode in [0x8b_u8, 0x89] {
            for data_register in [6_u8, 7] {
                for (mode, displacement) in
                    [(0_u8, vec![]), (1, vec![0x80]), (2, vec![0, 0, 0, 0x80])]
                {
                    let mut source =
                        vec![0x48, opcode, (mode << 6) | (data_register << 3) | 4, 0x24];
                    source.extend_from_slice(&displacement);
                    for window in 1..source.len() {
                        let code = CodeView::new(ENTRY, &source).unwrap();
                        let boundary =
                            FixtureExecutionBoundary::new(code, ENTRY + window as u64).unwrap();
                        let mut state = LongModeState {
                            cr3: None,
                            rip: ENTRY,
                            ..Default::default()
                        };
                        let before = state.clone();
                        let mut memory = EmptyMemory;
                        let mut executor = AdvancingExecutor::default();
                        let result = unsafe {
                            run(
                                code,
                                boundary,
                                &mut state,
                                &mut memory,
                                AccessContext::default(),
                                2,
                                None,
                                &AtomicU8::new(0),
                                &mut executor,
                            )
                        }
                        .unwrap();
                        assert_eq!(
                            result.exit,
                            BoundedMemoryExit::BoundaryReached { rip: ENTRY }
                        );
                        assert_eq!(result.blocks_executed, 0);
                        assert_eq!(state, before);
                        assert!(executor.entries.is_empty());
                    }

                    let code = CodeView::new(ENTRY, &source).unwrap();
                    let boundary =
                        FixtureExecutionBoundary::new(code, ENTRY + source.len() as u64).unwrap();
                    let mut state = LongModeState {
                        cr3: None,
                        rip: ENTRY,
                        ..Default::default()
                    };
                    let mut memory = EmptyMemory;
                    let mut executor = AdvancingExecutor::default();
                    let result = unsafe {
                        run(
                            code,
                            boundary.clone(),
                            &mut state,
                            &mut memory,
                            AccessContext::default(),
                            2,
                            None,
                            &AtomicU8::new(0),
                            &mut executor,
                        )
                    }
                    .unwrap();
                    assert_eq!(
                        result.exit,
                        BoundedMemoryExit::BoundaryReached {
                            rip: ENTRY + source.len() as u64
                        }
                    );
                    assert_eq!(
                        (
                            result.blocks_executed,
                            result.retired_instructions,
                            result.remaining_budget
                        ),
                        (1, 1, 1)
                    );
                    assert_eq!(executor.entries, [(ENTRY, source.len())]);
                    assert_eq!(state.rip, ENTRY + source.len() as u64);

                    let mut guarded_state = LongModeState {
                        cr3: None,
                        rip: ENTRY,
                        ..Default::default()
                    };
                    let guarded_before = guarded_state.clone();
                    let mut guarded_memory = EmptyMemory;
                    let mut guarded_executor = AdvancingExecutor::default();
                    let zero_budget = unsafe {
                        run(
                            code,
                            boundary.clone(),
                            &mut guarded_state,
                            &mut guarded_memory,
                            AccessContext::default(),
                            0,
                            None,
                            &AtomicU8::new(0),
                            &mut guarded_executor,
                        )
                    }
                    .unwrap();
                    assert_eq!(zero_budget.exit, BoundedMemoryExit::BudgetExhausted);
                    assert_eq!(guarded_state, guarded_before);
                    assert!(guarded_executor.entries.is_empty());

                    let stop = AtomicU8::new(1);
                    let stopped = unsafe {
                        run(
                            code,
                            boundary,
                            &mut guarded_state,
                            &mut guarded_memory,
                            AccessContext::default(),
                            1,
                            None,
                            &stop,
                            &mut guarded_executor,
                        )
                    }
                    .unwrap();
                    assert_eq!(stopped.exit, BoundedMemoryExit::StopRequested);
                    assert_eq!(guarded_state, guarded_before);
                    assert!(guarded_executor.entries.is_empty());
                }
            }
        }
    }
}
