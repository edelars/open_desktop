//! Uncached, bounded dispatch for the checked qword MOV memory slice.
//!
//! This API is owner-borrowed: executable bytes are an immutable `CodeView`
//! kept alive for the complete call, and the data-memory borrow is held until
//! the synchronous executor returns. It is not an MMU or production RAM owner.

use std::sync::atomic::{AtomicU8, Ordering};

use od_isa::{
    LongModeState, long_decode,
    long_decode::DecodeError,
    memory::{AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError},
};

use crate::{
    block::{CodeMemory, CodeView},
    memory_load_v2::{
        IrMemoryLoadBlockV2, IrMemoryStoreBlockV2, IrMemoryTwoSiteBlockV2,
        compile_ir_memory_load_block_v2, compile_ir_memory_store_block_v2,
        compile_ir_memory_two_site_block_v2,
    },
};

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
    GuestFault(ArchitecturalFault),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryBlockReport {
    pub exit: MemoryBlockExit,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
}

/// Executes one compiler-issued checked-memory artifact synchronously.
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundedMemoryExit {
    BoundaryReached { rip: u64 },
    CodeInvalidated,
    BudgetExhausted,
    StopRequested,
    StopAddress,
    GuestFault(ArchitecturalFault),
    Unsupported { rip: u64 },
    Invalid { rip: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedMemoryResult {
    pub exit: BoundedMemoryExit,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
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
    let mut remaining = total_budget;
    let mut retired = 0u64;
    let mut blocks = 0u64;
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
            executor.execute_one_memory_block(
                &block,
                state,
                memory,
                access_context,
                incoming_budget,
                stop_rip,
                stop_requested,
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
            [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b] | [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02]
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
    if instruction_len != 3 {
        return CompileAt::Unsupported;
    }
    let instruction = &bytes[..instruction_len];
    let Ok(view) = CodeView::new(rip, instruction) else {
        return CompileAt::Unsupported;
    };
    match instruction {
        [0x48, 0x8b, _] => compile_ir_memory_load_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Load)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
        [0x48, 0x89, _] => compile_ir_memory_store_block_v2(view, rip)
            .map(BoundedMemoryArtifact::Store)
            .map(CompileAt::Block)
            .unwrap_or(CompileAt::Unsupported),
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
