//! Runtime entry emission for verified register-only guest blocks.

use crate::allocator::{Allocation, AllocatorError, HelperBoundaryPosition, ValueLocation};
use crate::arm64::Condition;
use crate::arm64::{
    AddressBase, EncodeError, EncodedCode, Encoder, PairIndexMode, XReg, XRegOrSp, XRegOrZero,
};
use crate::ir::{
    self, BranchCondition, ExitKind, FlagEffect, GuestGpr, IntWidth, IntegerKind, Operation,
    Program, TempId, VerifyError,
};
use crate::runtime::{
    ArithmeticFlagsHelper, HelperOperationTag, LONG_MODE_GPR_OFFSET, LONG_MODE_RFLAGS_OFFSET,
    LONG_MODE_RIP_OFFSET, RAW_HELPER_CONTRACT_FAILURE_V1, RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET,
    RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET, RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET,
    RUNTIME_CONTEXT_STATE_OFFSET, RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET,
    RUNTIME_CONTEXT_STOP_RIP_ENABLED_OFFSET, RUNTIME_CONTEXT_STOP_RIP_OFFSET,
    RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET, RuntimeExitTag, arithmetic_flags_helper_adapter,
};
use crate::statepoints::{
    GuestBoundaryPoint, GuestRipBoundary, GuestRipSource, HostCodeRange, PcMap, PcMapError,
    SnapshotAdmissibility, StatePhase, StatePointRange,
};
use od_isa::integer_flags;

const MAX_RUNTIME_FRAME_BYTES: u32 = 32_768;
const MIN_RUNTIME_PREFIX_BYTES: u32 = 104;
const FRAME_STAGING_GPRS_POINTER: u32 = 0;
const FRAME_STAGING_FLAGS_POINTER: u32 = 16;
const FRAME_STAGING_RIP_POINTER: u32 = 24;
const FRAME_HELPER_FLAGS_OUTPUT: u32 = 32;
const FRAME_CANONICAL_STATE_POINTER: u32 = 40;
const FRAME_RUNTIME_CONTEXT_POINTER: u32 = 48;
const FRAME_STAGING_GPRS: u32 = 56;
const FRAME_STAGING_FLAGS: u32 = 88;
const FRAME_STAGING_RIP: u32 = 96;
const MAX_BLOCK_INSTRUCTIONS: usize = 32;

/// An emitted block entry with private code/map provenance. Only blocks made
/// by `emit_runtime_block` may be invoked through the safe host wrapper.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmittedRuntimeBlock {
    code: EncodedCode,
    pc_map: PcMap,
    entry_rip: u64,
    safe_entry: bool,
}

/// Isolated V2 entry emitted from the shared typed IR and allocator path.
/// Its code and map are compiler-issued and cannot be called as a V1 block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmittedCheckedLoadBlockV2 {
    code: EncodedCode,
    pc_map: PcMap,
    entry_rip: u64,
}

/// Isolated V2 entry emitted from the shared typed IR for one checked store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmittedCheckedStoreBlockV2 {
    code: EncodedCode,
    pc_map: PcMap,
    entry_rip: u64,
}

/// Isolated entry for the exact S3-20j store-then-load profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmittedStoreThenLoadBlockV2 {
    code: EncodedCode,
    pc_map: PcMap,
    entry_rip: u64,
}

impl EmittedStoreThenLoadBlockV2 {
    pub fn code(&self) -> &EncodedCode {
        &self.code
    }
    pub fn pc_map(&self) -> &PcMap {
        &self.pc_map
    }
    pub const fn entry_rip(&self) -> u64 {
        self.entry_rip
    }
}

impl EmittedCheckedStoreBlockV2 {
    pub fn code(&self) -> &EncodedCode {
        &self.code
    }
    pub fn pc_map(&self) -> &PcMap {
        &self.pc_map
    }
    pub const fn entry_rip(&self) -> u64 {
        self.entry_rip
    }
}

impl EmittedCheckedLoadBlockV2 {
    pub fn code(&self) -> &EncodedCode {
        &self.code
    }
    pub fn pc_map(&self) -> &PcMap {
        &self.pc_map
    }
    pub const fn entry_rip(&self) -> u64 {
        self.entry_rip
    }
}

impl EmittedRuntimeBlock {
    pub fn code(&self) -> &EncodedCode {
        &self.code
    }

    pub fn to_le_bytes(&self) -> Result<Vec<u8>, EncodeError> {
        self.code.to_le_bytes()
    }

    pub fn pc_map(&self) -> &PcMap {
        &self.pc_map
    }

    pub const fn entry_rip(&self) -> u64 {
        self.entry_rip
    }

    pub const fn safe_entry_provenance(&self) -> bool {
        self.safe_entry
    }

    /// Logical bytes occupied by emitted words and state-map metadata,
    /// excluding allocator spare capacity.
    pub(crate) fn logical_storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                self.code
                    .words()
                    .len()
                    .checked_mul(std::mem::size_of::<u32>())?,
            )?
            .checked_add(self.pc_map.logical_storage_bytes()?)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockEmitterError {
    InvalidProgram(VerifyError),
    Allocation(AllocatorError),
    AllocationProgramMismatch,
    InsufficientFramePrefix {
        required: u32,
        actual: u32,
    },
    UnsupportedShape {
        instruction_index: usize,
    },
    UnsupportedWidth {
        operation: usize,
        width: IntWidth,
    },
    UnsupportedFlagEffect {
        operation: usize,
        effect: FlagEffect,
    },
    MissingHelperBoundary {
        operation: usize,
    },
    InvalidRegister(GuestGpr),
    CodeRangeOverflow,
    FrameTooLarge,
    PcMap(PcMapError),
    Encoding(EncodeError),
}

impl From<AllocatorError> for BlockEmitterError {
    fn from(error: AllocatorError) -> Self {
        Self::Allocation(error)
    }
}

impl From<EncodeError> for BlockEmitterError {
    fn from(error: EncodeError) -> Self {
        Self::Encoding(error)
    }
}

/// Emit an entry that commits supported guest instructions to `LongModeState`
/// and returns only the planned exits in `RuntimeContextV1`.
pub fn emit_runtime_block(
    program: &Program,
    allocation: &Allocation<'_>,
) -> Result<EmittedRuntimeBlock, BlockEmitterError> {
    emit_runtime_block_inner(program, allocation, arithmetic_flags_helper_adapter, true)
}

/// Emit the one-instruction checked-load IR profile through the common IR,
/// allocator helper boundary, commit, and PC-map machinery. Context layout
/// starts with V1, but this entry is explicitly typed V2 by its publisher.
pub fn emit_checked_load_runtime_block_v2(
    program: &Program,
    allocation: &Allocation<'_>,
) -> Result<EmittedCheckedLoadBlockV2, BlockEmitterError> {
    ir::verify(program).map_err(BlockEmitterError::InvalidProgram)?;
    if !std::ptr::eq(program, allocation.program()) {
        return Err(BlockEmitterError::AllocationProgramMismatch);
    }
    let (load_operation, loaded_temp) = program
        .operations
        .iter()
        .enumerate()
        .find_map(|(i, op)| {
            if let Operation::CheckedLoad { dst, .. } = op {
                Some((i, *dst))
            } else {
                None
            }
        })
        .ok_or(BlockEmitterError::UnsupportedShape {
            instruction_index: 0,
        })?;
    let instruction =
        program
            .guest_instructions
            .first()
            .ok_or(BlockEmitterError::UnsupportedShape {
                instruction_index: 0,
            })?;
    if program.guest_instructions.len() != 1
        || program.operations.len() != 5
        || load_operation != 1
        || !matches!(
            program.operations.first(),
            Some(Operation::BeginInstruction { index: 0 })
        )
        || !matches!(program.operations.get(load_operation + 1), Some(Operation::WriteGpr { src, width: IntWidth::W64, .. }) if *src == loaded_temp)
        || !matches!(
            program.operations.get(load_operation + 2),
            Some(Operation::CommitInstruction { index: 0 })
        )
        || !matches!(program.operations.last(), Some(Operation::Exit { kind: ExitKind::BlockComplete, target_rip }) if *target_rip == instruction.next_rip)
    {
        return Err(BlockEmitterError::UnsupportedShape {
            instruction_index: 0,
        });
    }
    if allocation.frame().reserved_prefix_bytes < 144 {
        return Err(BlockEmitterError::InsufficientFramePrefix {
            required: 144,
            actual: allocation.frame().reserved_prefix_bytes,
        });
    }

    let mut encoder = Encoder::new();
    let frame = allocation.frame();
    if frame.alignment != 16
        || frame.frame_bytes % frame.alignment != 0
        || frame.frame_bytes > MAX_RUNTIME_FRAME_BYTES
    {
        return Err(BlockEmitterError::FrameTooLarge);
    }
    let pair_count =
        usize::try_from(frame.frame_bytes / 16).map_err(|_| BlockEmitterError::FrameTooLarge)?;
    let epilogue = encoder.new_label()?;
    let helper_failure = encoder.new_label()?;
    let mut returns = Vec::<ReturnStub>::new();
    let mut ranges = Vec::<StatePointRange>::new();
    let mut boundaries = Vec::<GuestBoundaryPoint>::new();

    let service_start = code_offset(&encoder)?;
    emit_prologue(&mut encoder, pair_count)?;
    // Preserve the additional callee-saved registers used for the V2 helper context.
    for (a, b, offset) in [(19, 20, 104), (21, 22, 120)] {
        encoder.stp_x(
            xreg(a)?,
            xreg(b)?,
            AddressBase::Sp,
            offset,
            PairIndexMode::Offset,
        )?;
    }
    emit_runtime_frame_setup(&mut encoder)?;
    push_range(
        &mut ranges,
        service_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        None,
        StatePhase::Service,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    let guard_start = code_offset(&encoder)?;
    emit_instruction_guards(&mut encoder, &mut returns, instruction.guest_rip, 0)?;
    push_range(
        &mut ranges,
        guard_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::BeforeInstruction,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    let boundary = HelperBoundaryPosition::BeforeOperation(load_operation);
    allocation
        .helper_boundary(boundary)
        .map_err(|_| BlockEmitterError::MissingHelperBoundary {
            operation: load_operation,
        })?;
    let pending_start = code_offset(&encoder)?;
    allocation.emit_save_before_helper(&mut encoder, boundary)?;
    emit_saved_pointer_load(&mut encoder, FRAME_RUNTIME_CONTEXT_POINTER, xreg(19)?)?;
    encoder.mov_x(xreg(0)?, XRegOrZero::Reg(xreg(19)?));
    encoder.movz_x(xreg(1)?, 0, 0)?;
    emit_u64(
        &mut encoder,
        xreg(16)?,
        crate::runtime::memory_site_helper_v2 as *const () as usize as u64,
    )?;
    let call_start = code_offset(&encoder)?;
    push_range(
        &mut ranges,
        pending_start,
        call_start,
        instruction.guest_rip,
        Some(0),
        StatePhase::PendingEffects,
        SnapshotAdmissibility::ReplayCurrentInstruction,
    );
    push_range(
        &mut ranges,
        call_start,
        call_start
            .checked_add(4)
            .ok_or(BlockEmitterError::CodeRangeOverflow)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::HelperCall,
        SnapshotAdmissibility::ReplayCurrentInstruction,
    );
    encoder.blr(xreg(16)?);
    encoder.cbnz_w(crate::arm64::WReg::new(0)?, helper_failure)?;
    allocation.emit_restore_after_helper(&mut encoder, boundary)?;
    emit_saved_pointer_load(&mut encoder, FRAME_RUNTIME_CONTEXT_POINTER, xreg(19)?)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(19)?),
        crate::runtime::RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET as u32,
    )?;
    allocation.emit_store_scratch(&mut encoder, loaded_temp, xreg(13)?)?;
    push_range(
        &mut ranges,
        call_start + 4,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::AfterHelper,
        SnapshotAdmissibility::ReplayCurrentInstruction,
    );

    let write = program
        .operations
        .iter()
        .find_map(|op| {
            if let Operation::WriteGpr { register, src, .. } = op {
                Some((*register, *src))
            } else {
                None
            }
        })
        .ok_or(BlockEmitterError::UnsupportedShape {
            instruction_index: 0,
        })?;
    let write_start = code_offset(&encoder)?;
    allocation.emit_load_scratch(&mut encoder, write.1, xreg(13)?)?;
    emit_saved_pointer_load(&mut encoder, FRAME_STAGING_GPRS_POINTER, xreg(16)?)?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        guest_gpr_offset(write.0)?,
    )?;
    push_range(
        &mut ranges,
        write_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::PendingEffects,
        SnapshotAdmissibility::ReplayCurrentInstruction,
    );

    let commit_start = code_offset(&encoder)?;
    emit_u64(&mut encoder, xreg(13)?, instruction.next_rip)?;
    emit_saved_pointer_load(&mut encoder, FRAME_STAGING_RIP_POINTER, xreg(16)?)?;
    encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Reg(xreg(16)?), 0)?;
    emit_commit(&mut encoder, 0, pair_count)?;
    push_range(
        &mut ranges,
        commit_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::CommitInProgress,
        SnapshotAdmissibility::InadmissibleDuringCommit,
    );
    boundaries.push(GuestBoundaryPoint {
        host_offset: code_offset(&encoder)?,
        rip: GuestRipBoundary::Fixed(instruction.next_rip),
        instruction_index: 0,
        committed_instructions: 1,
        phase: StatePhase::Committed,
    });

    let terminal_start = code_offset(&encoder)?;
    let complete = encoder.new_label()?;
    encoder.b(complete)?;
    returns.push(ReturnStub {
        label: complete,
        raw_tag: RuntimeExitTag::BlockComplete as u32,
        guest_rip: GuestRipSource::CanonicalState,
        instruction_index: Some(0),
        snapshot: SnapshotAdmissibility::CanonicalBoundary,
    });
    push_range(
        &mut ranges,
        terminal_start,
        code_offset(&encoder)?,
        instruction.next_rip,
        Some(0),
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    let mut failures = Vec::new();
    failures.push(ReturnStub {
        label: helper_failure,
        raw_tag: crate::runtime::RUNTIME_EXIT_V2_MEMORY_OPERATION,
        guest_rip: GuestRipSource::Fixed(instruction.guest_rip),
        instruction_index: Some(0),
        snapshot: SnapshotAdmissibility::ReplayCurrentInstruction,
    });
    for stub in returns.iter().chain(failures.iter()) {
        let start = code_offset(&encoder)?;
        encoder.bind(stub.label)?;
        emit_u64(&mut encoder, xreg(0)?, u64::from(stub.raw_tag))?;
        encoder.b(epilogue)?;
        push_range_with_source(
            &mut ranges,
            start,
            code_offset(&encoder)?,
            stub.guest_rip,
            stub.instruction_index,
            StatePhase::Exit,
            stub.snapshot,
        );
    }
    let epilogue_start = code_offset(&encoder)?;
    encoder.bind(epilogue)?;
    for (a, b, offset) in [(19, 20, 104), (21, 22, 120)] {
        encoder.ldp_x(
            xreg(a)?,
            xreg(b)?,
            AddressBase::Sp,
            offset,
            PairIndexMode::Offset,
        )?;
    }
    emit_epilogue(&mut encoder, pair_count)?;
    push_range_with_source(
        &mut ranges,
        epilogue_start,
        code_offset(&encoder)?,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::Service,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    let code = encoder.finish()?;
    let bytes = code.to_le_bytes()?;
    let pc_map =
        PcMap::new(bytes.len(), 1, ranges, boundaries).map_err(BlockEmitterError::PcMap)?;
    Ok(EmittedCheckedLoadBlockV2 {
        code,
        pc_map,
        entry_rip: instruction.guest_rip,
    })
}

/// Emit a single checked store from the typed IR. The helper/write return to
/// the guest-state commit is explicitly non-replay-admissible in the PC map.
pub fn emit_checked_store_runtime_block_v2(
    program: &Program,
    allocation: &Allocation<'_>,
) -> Result<EmittedCheckedStoreBlockV2, BlockEmitterError> {
    ir::verify(program).map_err(BlockEmitterError::InvalidProgram)?;
    if !std::ptr::eq(program, allocation.program()) {
        return Err(BlockEmitterError::AllocationProgramMismatch);
    }
    let instruction =
        program
            .guest_instructions
            .first()
            .ok_or(BlockEmitterError::UnsupportedShape {
                instruction_index: 0,
            })?;
    let valid_shape = program.guest_instructions.len() == 1
        && program.operations.len() == 4
        && matches!(program.operations.as_slice(), [Operation::BeginInstruction { index: 0 }, Operation::CheckedStore { .. }, Operation::CommitInstruction { index: 0 }, Operation::Exit { kind: ExitKind::BlockComplete, target_rip }] if *target_rip == instruction.next_rip);
    if !valid_shape {
        return Err(BlockEmitterError::UnsupportedShape {
            instruction_index: 0,
        });
    }
    let store_operation = program
        .operations
        .iter()
        .position(|operation| matches!(operation, Operation::CheckedStore { .. }))
        .ok_or(BlockEmitterError::UnsupportedShape {
            instruction_index: 0,
        })?;
    let store_boundary = HelperBoundaryPosition::BeforeOperation(store_operation);
    allocation.helper_boundary(store_boundary).map_err(|_| {
        BlockEmitterError::MissingHelperBoundary {
            operation: store_operation,
        }
    })?;
    if allocation.frame().reserved_prefix_bytes < 144 {
        return Err(BlockEmitterError::InsufficientFramePrefix {
            required: 144,
            actual: allocation.frame().reserved_prefix_bytes,
        });
    }
    let mut encoder = Encoder::new();
    let frame = allocation.frame();
    if frame.alignment != 16
        || frame.frame_bytes % 16 != 0
        || frame.frame_bytes > MAX_RUNTIME_FRAME_BYTES
    {
        return Err(BlockEmitterError::FrameTooLarge);
    }
    let pair_count =
        usize::try_from(frame.frame_bytes / 16).map_err(|_| BlockEmitterError::FrameTooLarge)?;
    let epilogue = encoder.new_label()?;
    let helper_failure = encoder.new_label()?;
    let stop_after_commit = encoder.new_label()?;
    let success = encoder.new_label()?;
    let mut ranges = Vec::new();
    let mut boundaries = Vec::new();
    let mut guard_returns = Vec::new();
    let service_start = code_offset(&encoder)?;
    emit_prologue(&mut encoder, pair_count)?;
    for (a, b, offset) in [(19, 20, 104), (21, 22, 120)] {
        encoder.stp_x(
            xreg(a)?,
            xreg(b)?,
            AddressBase::Sp,
            offset,
            PairIndexMode::Offset,
        )?;
    }
    emit_runtime_frame_setup(&mut encoder)?;
    push_range(
        &mut ranges,
        service_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        None,
        StatePhase::Service,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    let guard_start = code_offset(&encoder)?;
    emit_instruction_guards(&mut encoder, &mut guard_returns, instruction.guest_rip, 0)?;
    push_range(
        &mut ranges,
        guard_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::BeforeInstruction,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    let setup_start = code_offset(&encoder)?;
    emit_saved_pointer_load(&mut encoder, FRAME_RUNTIME_CONTEXT_POINTER, xreg(19)?)?;
    encoder.mov_x(xreg(0)?, XRegOrZero::Reg(xreg(19)?));
    encoder.movz_x(xreg(1)?, 0, 0)?;
    emit_u64(
        &mut encoder,
        xreg(16)?,
        crate::runtime::memory_site_helper_v2 as *const () as usize as u64,
    )?;
    push_range(
        &mut ranges,
        setup_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::PendingEffects,
        SnapshotAdmissibility::ReplayCurrentInstruction,
    );
    let call_start = code_offset(&encoder)?;
    encoder.blr(xreg(16)?);
    push_range(
        &mut ranges,
        call_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::HelperCall,
        SnapshotAdmissibility::InadmissibleAfterEffect,
    );
    let check_start = code_offset(&encoder)?;
    encoder.cbnz_w(crate::arm64::WReg::new(0)?, helper_failure)?;
    push_range(
        &mut ranges,
        check_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::AfterHelper,
        SnapshotAdmissibility::InadmissibleAfterEffect,
    );

    let commit_start = code_offset(&encoder)?;
    emit_u64(&mut encoder, xreg(13)?, instruction.next_rip)?;
    emit_saved_pointer_load(&mut encoder, FRAME_STAGING_RIP_POINTER, xreg(16)?)?;
    encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Reg(xreg(16)?), 0)?;
    emit_commit(&mut encoder, 0, pair_count)?;
    push_range(
        &mut ranges,
        commit_start,
        code_offset(&encoder)?,
        instruction.guest_rip,
        Some(0),
        StatePhase::CommitInProgress,
        SnapshotAdmissibility::InadmissibleDuringCommit,
    );
    boundaries.push(GuestBoundaryPoint {
        host_offset: code_offset(&encoder)?,
        rip: GuestRipBoundary::Fixed(instruction.next_rip),
        instruction_index: 0,
        committed_instructions: 1,
        phase: StatePhase::Committed,
    });
    let post_commit_start = code_offset(&encoder)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(9)?),
        AddressBase::Reg(xreg(19)?),
        RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET as u32,
    )?;
    let no_stop = encoder.new_label()?;
    encoder.cbz_x(XRegOrZero::Reg(xreg(9)?), no_stop)?;
    encoder.ldarb_w(crate::arm64::WReg::new(9)?, AddressBase::Reg(xreg(9)?));
    encoder.cbnz_w(crate::arm64::WReg::new(9)?, stop_after_commit)?;
    encoder.bind(no_stop)?;
    encoder.b(success)?;
    push_range(
        &mut ranges,
        post_commit_start,
        code_offset(&encoder)?,
        instruction.next_rip,
        Some(0),
        StatePhase::Committed,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    for (label, tag, rip, admissibility) in [
        (
            success,
            RuntimeExitTag::BlockComplete as u32,
            GuestRipSource::Fixed(instruction.next_rip),
            SnapshotAdmissibility::CanonicalBoundary,
        ),
        (
            stop_after_commit,
            RuntimeExitTag::StopRequested as u32,
            GuestRipSource::Fixed(instruction.next_rip),
            SnapshotAdmissibility::CanonicalBoundary,
        ),
        (
            helper_failure,
            crate::runtime::RUNTIME_EXIT_V2_MEMORY_OPERATION,
            GuestRipSource::Fixed(instruction.guest_rip),
            SnapshotAdmissibility::ReplayCurrentInstruction,
        ),
    ] {
        let start = code_offset(&encoder)?;
        encoder.bind(label)?;
        emit_u64(&mut encoder, xreg(0)?, u64::from(tag))?;
        encoder.b(epilogue)?;
        push_range_with_source(
            &mut ranges,
            start,
            code_offset(&encoder)?,
            rip,
            Some(0),
            StatePhase::Exit,
            admissibility,
        );
    }
    for stub in &guard_returns {
        let start = code_offset(&encoder)?;
        encoder.bind(stub.label)?;
        emit_u64(&mut encoder, xreg(0)?, u64::from(stub.raw_tag))?;
        encoder.b(epilogue)?;
        push_range_with_source(
            &mut ranges,
            start,
            code_offset(&encoder)?,
            stub.guest_rip,
            stub.instruction_index,
            StatePhase::Exit,
            stub.snapshot,
        );
    }
    let epilogue_start = code_offset(&encoder)?;
    encoder.bind(epilogue)?;
    for (a, b, offset) in [(19, 20, 104), (21, 22, 120)] {
        encoder.ldp_x(
            xreg(a)?,
            xreg(b)?,
            AddressBase::Sp,
            offset,
            PairIndexMode::Offset,
        )?;
    }
    emit_epilogue(&mut encoder, pair_count)?;
    push_range_with_source(
        &mut ranges,
        epilogue_start,
        code_offset(&encoder)?,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::Service,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    let code = encoder.finish()?;
    let pc_map = PcMap::new(code.to_le_bytes()?.len(), 1, ranges, boundaries)
        .map_err(BlockEmitterError::PcMap)?;
    Ok(EmittedCheckedStoreBlockV2 {
        code,
        pc_map,
        entry_rip: instruction.guest_rip,
    })
}

/// Emit one entry for exactly `MOV [RBX], RAX; MOV RCX, [RBX]`. The two
/// helpers and commits remain in guest order; the second instruction is also
/// a legal continuation entry when the canonical RIP already names it.
pub fn emit_store_then_load_runtime_block_v2(
    program: &Program,
    allocation: &Allocation<'_>,
) -> Result<EmittedStoreThenLoadBlockV2, BlockEmitterError> {
    ir::verify(program).map_err(BlockEmitterError::InvalidProgram)?;
    if !std::ptr::eq(program, allocation.program()) {
        return Err(BlockEmitterError::AllocationProgramMismatch);
    }
    if program.guest_instructions.len() != 2
        || !matches!(program.operations.as_slice(), [
            Operation::BeginInstruction { index: 0 },
            Operation::CheckedStore { src: GuestGpr(0), operand: store_operand },
            Operation::CommitInstruction { index: 0 },
            Operation::BeginInstruction { index: 1 },
            Operation::CheckedLoad { dst: load_temp, operand: load_operand },
            Operation::WriteGpr { register: GuestGpr(1), src, width: IntWidth::W64 },
            Operation::CommitInstruction { index: 1 },
            Operation::Exit { kind: ExitKind::BlockComplete, target_rip },
        ] if src == load_temp
            && *target_rip == program.guest_instructions[1].next_rip
            && supported_store_then_load_operands(*store_operand, *load_operand)
        )
    {
        return Err(BlockEmitterError::UnsupportedShape {
            instruction_index: 0,
        });
    }
    if allocation.frame().reserved_prefix_bytes < 144 {
        return Err(BlockEmitterError::InsufficientFramePrefix {
            required: 144,
            actual: allocation.frame().reserved_prefix_bytes,
        });
    }
    let frame = allocation.frame();
    if frame.alignment != 16
        || frame.frame_bytes % frame.alignment != 0
        || frame.frame_bytes > MAX_RUNTIME_FRAME_BYTES
    {
        return Err(BlockEmitterError::FrameTooLarge);
    }
    let pair_count =
        usize::try_from(frame.frame_bytes / 16).map_err(|_| BlockEmitterError::FrameTooLarge)?;
    let [store, load] = program.guest_instructions.as_slice() else {
        return Err(BlockEmitterError::UnsupportedShape {
            instruction_index: 0,
        });
    };
    let mut encoder = Encoder::new();
    let epilogue = encoder.new_label()?;
    let run_store = encoder.new_label()?;
    let run_load = encoder.new_label()?;
    let store_failed = encoder.new_label()?;
    let load_failed = encoder.new_label()?;
    let success = encoder.new_label()?;
    let mut returns = Vec::<ReturnStub>::new();
    let mut ranges = Vec::<StatePointRange>::new();
    let mut boundaries = Vec::<GuestBoundaryPoint>::new();

    let service_start = code_offset(&encoder)?;
    emit_prologue(&mut encoder, pair_count)?;
    for (a, b, offset) in [(19, 20, 104), (21, 22, 120)] {
        encoder.stp_x(
            xreg(a)?,
            xreg(b)?,
            AddressBase::Sp,
            offset,
            PairIndexMode::Offset,
        )?;
    }
    emit_runtime_frame_setup(&mut encoder)?;
    push_range_with_source(
        &mut ranges,
        service_start,
        code_offset(&encoder)?,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::Service,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    // The same generated entry supports initial execution and continuation at
    // the load boundary, but rejects every other canonical RIP.
    let entry_check = code_offset(&encoder)?;
    emit_state_load(&mut encoder, LONG_MODE_RIP_OFFSET as u32, xreg(15)?)?;
    emit_u64(&mut encoder, xreg(14)?, store.guest_rip)?;
    encoder.subs_x(XRegOrZero::Zero, xreg(15)?, XRegOrZero::Reg(xreg(14)?));
    encoder.b_cond(Condition::Equal, run_store)?;
    emit_u64(&mut encoder, xreg(14)?, load.guest_rip)?;
    encoder.subs_x(XRegOrZero::Zero, xreg(15)?, XRegOrZero::Reg(xreg(14)?));
    encoder.b_cond(Condition::Equal, run_load)?;
    let bad_entry = new_exit_stub(
        &mut encoder,
        &mut returns,
        RuntimeExitTag::NeedInterpreter as u32,
        store.guest_rip,
        None,
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    encoder.b(bad_entry)?;
    push_range_with_source(
        &mut ranges,
        entry_check,
        code_offset(&encoder)?,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::BeforeInstruction,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    encoder.bind(run_store)?;
    let store_guard = code_offset(&encoder)?;
    emit_instruction_guards(&mut encoder, &mut returns, store.guest_rip, 0)?;
    push_range(
        &mut ranges,
        store_guard,
        code_offset(&encoder)?,
        store.guest_rip,
        Some(0),
        StatePhase::BeforeInstruction,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    emit_checked_memory_site_call(
        &mut encoder,
        &mut ranges,
        allocation,
        1,
        0,
        store.guest_rip,
        store_failed,
    )?;
    let store_commit = code_offset(&encoder)?;
    emit_u64(&mut encoder, xreg(13)?, store.next_rip)?;
    emit_saved_pointer_load(&mut encoder, FRAME_STAGING_RIP_POINTER, xreg(16)?)?;
    encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Reg(xreg(16)?), 0)?;
    emit_commit(&mut encoder, 0, pair_count)?;
    push_range(
        &mut ranges,
        store_commit,
        code_offset(&encoder)?,
        store.guest_rip,
        Some(0),
        StatePhase::CommitInProgress,
        SnapshotAdmissibility::InadmissibleDuringCommit,
    );
    boundaries.push(GuestBoundaryPoint {
        host_offset: code_offset(&encoder)?,
        rip: GuestRipBoundary::Fixed(store.next_rip),
        instruction_index: 0,
        committed_instructions: 1,
        phase: StatePhase::Committed,
    });
    let after_store = code_offset(&encoder)?;
    encoder.b(run_load)?;
    push_range(
        &mut ranges,
        after_store,
        code_offset(&encoder)?,
        store.next_rip,
        Some(0),
        StatePhase::Committed,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    encoder.bind(run_load)?;
    let load_guard = code_offset(&encoder)?;
    emit_instruction_guards(&mut encoder, &mut returns, load.guest_rip, 1)?;
    push_range(
        &mut ranges,
        load_guard,
        code_offset(&encoder)?,
        load.guest_rip,
        Some(1),
        StatePhase::BeforeInstruction,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    emit_checked_memory_site_call(
        &mut encoder,
        &mut ranges,
        allocation,
        4,
        1,
        load.guest_rip,
        load_failed,
    )?;
    let load_commit = code_offset(&encoder)?;
    emit_saved_pointer_load(&mut encoder, FRAME_RUNTIME_CONTEXT_POINTER, xreg(16)?)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Sp,
        FRAME_STAGING_GPRS + 8,
    )?;
    emit_u64(&mut encoder, xreg(13)?, load.next_rip)?;
    emit_saved_pointer_load(&mut encoder, FRAME_STAGING_RIP_POINTER, xreg(16)?)?;
    encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Reg(xreg(16)?), 0)?;
    emit_commit(&mut encoder, 1, pair_count)?;
    push_range(
        &mut ranges,
        load_commit,
        code_offset(&encoder)?,
        load.guest_rip,
        Some(1),
        StatePhase::CommitInProgress,
        SnapshotAdmissibility::InadmissibleDuringCommit,
    );
    boundaries.push(GuestBoundaryPoint {
        host_offset: code_offset(&encoder)?,
        rip: GuestRipBoundary::Fixed(load.next_rip),
        instruction_index: 1,
        committed_instructions: 2,
        phase: StatePhase::Committed,
    });
    let skip_stubs = code_offset(&encoder)?;
    encoder.b(success)?;
    push_range(
        &mut ranges,
        skip_stubs,
        code_offset(&encoder)?,
        load.next_rip,
        Some(1),
        StatePhase::Committed,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    for (label, tag, rip, index, snapshot) in [
        (
            store_failed,
            crate::runtime::RUNTIME_EXIT_V2_MEMORY_OPERATION,
            GuestRipSource::Fixed(store.guest_rip),
            Some(0),
            SnapshotAdmissibility::ReplayCurrentInstruction,
        ),
        (
            load_failed,
            crate::runtime::RUNTIME_EXIT_V2_MEMORY_OPERATION,
            GuestRipSource::Fixed(load.guest_rip),
            Some(1),
            SnapshotAdmissibility::ReplayCurrentInstruction,
        ),
    ] {
        let start = code_offset(&encoder)?;
        encoder.bind(label)?;
        emit_u64(&mut encoder, xreg(0)?, u64::from(tag))?;
        encoder.b(epilogue)?;
        push_range_with_source(
            &mut ranges,
            start,
            code_offset(&encoder)?,
            rip,
            index,
            StatePhase::Exit,
            snapshot,
        );
    }
    let success_start = code_offset(&encoder)?;
    encoder.bind(success)?;
    emit_u64(&mut encoder, xreg(0)?, RuntimeExitTag::BlockComplete as u64)?;
    encoder.b(epilogue)?;
    push_range(
        &mut ranges,
        success_start,
        code_offset(&encoder)?,
        load.next_rip,
        Some(1),
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    for stub in &returns {
        let start = code_offset(&encoder)?;
        encoder.bind(stub.label)?;
        emit_u64(&mut encoder, xreg(0)?, u64::from(stub.raw_tag))?;
        encoder.b(epilogue)?;
        push_range_with_source(
            &mut ranges,
            start,
            code_offset(&encoder)?,
            stub.guest_rip,
            stub.instruction_index,
            StatePhase::Exit,
            stub.snapshot,
        );
    }
    let epilogue_start = code_offset(&encoder)?;
    encoder.bind(epilogue)?;
    for (a, b, offset) in [(19, 20, 104), (21, 22, 120)] {
        encoder.ldp_x(
            xreg(a)?,
            xreg(b)?,
            AddressBase::Sp,
            offset,
            PairIndexMode::Offset,
        )?;
    }
    emit_epilogue(&mut encoder, pair_count)?;
    push_range_with_source(
        &mut ranges,
        epilogue_start,
        code_offset(&encoder)?,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::Service,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    let code = encoder.finish()?;
    let pc_map = PcMap::new(code.to_le_bytes()?.len(), 2, ranges, boundaries)
        .map_err(BlockEmitterError::PcMap)?;
    Ok(EmittedStoreThenLoadBlockV2 {
        code,
        pc_map,
        entry_rip: store.guest_rip,
    })
}

fn supported_store_then_load_operands(
    store: od_isa::long_decode::MemoryOperand,
    load: od_isa::long_decode::MemoryOperand,
) -> bool {
    use od_isa::OperandSize;
    use od_isa::long_decode::AddressBase;
    [store, load].into_iter().all(|operand| {
        operand.base == AddressBase::Register(od_isa::Register::Rbx)
            && operand.index.is_none()
            && operand.scale == 1
            && operand.displacement == 0
            && operand.address_size == OperandSize::Qword
            && operand.size == OperandSize::Qword
    })
}

fn emit_checked_memory_site_call(
    encoder: &mut Encoder,
    ranges: &mut Vec<StatePointRange>,
    allocation: &Allocation<'_>,
    operation: usize,
    site: u16,
    rip: u64,
    failure: crate::arm64::Label,
) -> Result<(), BlockEmitterError> {
    let boundary = HelperBoundaryPosition::BeforeOperation(operation);
    allocation
        .helper_boundary(boundary)
        .map_err(|_| BlockEmitterError::MissingHelperBoundary { operation })?;
    let pending = code_offset(encoder)?;
    allocation.emit_save_before_helper(encoder, boundary)?;
    emit_saved_pointer_load(encoder, FRAME_RUNTIME_CONTEXT_POINTER, xreg(19)?)?;
    encoder.mov_x(xreg(0)?, XRegOrZero::Reg(xreg(19)?));
    emit_u64(encoder, xreg(1)?, u64::from(site))?;
    emit_u64(
        encoder,
        xreg(16)?,
        crate::runtime::memory_site_helper_store_load_v2 as *const () as usize as u64,
    )?;
    let call = code_offset(encoder)?;
    encoder.blr(xreg(16)?);
    push_range(
        ranges,
        pending,
        call,
        rip,
        Some(u32::from(site)),
        StatePhase::PendingEffects,
        SnapshotAdmissibility::ReplayCurrentInstruction,
    );
    push_range(
        ranges,
        call,
        code_offset(encoder)?,
        rip,
        Some(u32::from(site)),
        StatePhase::HelperCall,
        SnapshotAdmissibility::InadmissibleAfterEffect,
    );
    let check = code_offset(encoder)?;
    encoder.cbnz_w(crate::arm64::WReg::new(0)?, failure)?;
    push_range(
        ranges,
        check,
        code_offset(encoder)?,
        rip,
        Some(u32::from(site)),
        StatePhase::AfterHelper,
        SnapshotAdmissibility::InadmissibleAfterEffect,
    );
    Ok(())
}

/// Emit a test-only/custom-helper block. Such a block cannot be called by the
/// safe `PublishedCode` block executor; callers must invoke it unsafely and
/// uphold the same AAPCS64/no-unwind/pure-helper contract as the production
/// adapter.
///
/// # Safety
/// `helper` must implement the scalar helper ABI, return status without unwind,
/// preserve AAPCS64 nonvolatile registers and SP, and avoid mutating canonical
/// guest state. It may modify only its documented output on success.
#[doc(hidden)]
pub unsafe fn emit_runtime_block_with_helper(
    program: &Program,
    allocation: &Allocation<'_>,
    helper: ArithmeticFlagsHelper,
) -> Result<EmittedRuntimeBlock, BlockEmitterError> {
    emit_runtime_block_inner(program, allocation, helper, false)
}

#[derive(Clone, Copy)]
struct ReturnStub {
    label: crate::arm64::Label,
    raw_tag: u32,
    guest_rip: GuestRipSource,
    instruction_index: Option<u32>,
    snapshot: SnapshotAdmissibility,
}

fn emit_runtime_block_inner(
    program: &Program,
    allocation: &Allocation<'_>,
    helper: ArithmeticFlagsHelper,
    safe_entry: bool,
) -> Result<EmittedRuntimeBlock, BlockEmitterError> {
    ir::verify(program).map_err(BlockEmitterError::InvalidProgram)?;
    if !std::ptr::eq(program, allocation.program()) {
        return Err(BlockEmitterError::AllocationProgramMismatch);
    }
    if allocation.frame().reserved_prefix_bytes < MIN_RUNTIME_PREFIX_BYTES {
        return Err(BlockEmitterError::InsufficientFramePrefix {
            required: MIN_RUNTIME_PREFIX_BYTES,
            actual: allocation.frame().reserved_prefix_bytes,
        });
    }
    let terminal_index = program
        .operations
        .iter()
        .position(|operation| {
            matches!(
                operation,
                Operation::Branch { .. }
                    | Operation::DispatchBranch { .. }
                    | Operation::Exit { .. }
            )
        })
        .ok_or(BlockEmitterError::InvalidProgram(
            VerifyError::MissingTerminal,
        ))?;
    validate_runtime_shape(program, terminal_index)?;

    let entry_rip = program.guest_instructions[0].guest_rip;
    let mut encoder = Encoder::new();
    let frame = allocation.frame();
    if frame.alignment != 16
        || frame.frame_bytes % frame.alignment != 0
        || frame.frame_bytes > MAX_RUNTIME_FRAME_BYTES
    {
        return Err(BlockEmitterError::FrameTooLarge);
    }
    let pair_count =
        usize::try_from(frame.frame_bytes / 16).map_err(|_| BlockEmitterError::FrameTooLarge)?;
    let epilogue = encoder.new_label()?;
    let mut return_stubs = Vec::<ReturnStub>::new();
    let mut helper_failures = Vec::<ReturnStub>::new();
    let mut ranges = Vec::<StatePointRange>::new();
    let mut boundaries = Vec::<GuestBoundaryPoint>::new();

    // The prologue is service code: it preserves frame state and initializes
    // private pending copies from the canonical guest state.
    let service_start = code_offset(&encoder)?;
    emit_prologue(&mut encoder, pair_count)?;
    emit_runtime_frame_setup(&mut encoder)?;
    push_range(
        &mut ranges,
        service_start,
        code_offset(&encoder)?,
        entry_rip,
        None,
        StatePhase::Service,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    let mut active_instruction = None;
    for (operation_index, operation) in program.operations.iter().enumerate() {
        match operation {
            Operation::BeginInstruction { index } => {
                active_instruction = Some(*index);
                let instruction = program.guest_instructions[*index];
                let start = code_offset(&encoder)?;
                emit_instruction_guards(
                    &mut encoder,
                    &mut return_stubs,
                    instruction.guest_rip,
                    *index,
                )?;
                push_range(
                    &mut ranges,
                    start,
                    code_offset(&encoder)?,
                    instruction.guest_rip,
                    Some(to_u32(*index)?),
                    StatePhase::BeforeInstruction,
                    SnapshotAdmissibility::CanonicalBoundary,
                );
            }
            Operation::Constant { dst, width, value } => {
                require_w64(*width, operation_index)?;
                let (rip, index) = active_guest(program, active_instruction)?;
                let start = code_offset(&encoder)?;
                emit_constant(&mut encoder, allocation, *dst, *value)?;
                push_range(
                    &mut ranges,
                    start,
                    code_offset(&encoder)?,
                    rip,
                    Some(to_u32(index)?),
                    StatePhase::PendingEffects,
                    SnapshotAdmissibility::ReplayCurrentInstruction,
                );
            }
            Operation::ReadGpr {
                dst,
                register,
                width,
            } => {
                require_w64(*width, operation_index)?;
                let (rip, index) = active_guest(program, active_instruction)?;
                let start = code_offset(&encoder)?;
                let byte_offset = guest_gpr_offset(*register)?;
                emit_saved_pointer_load(&mut encoder, FRAME_STAGING_GPRS_POINTER, xreg(16)?)?;
                match allocation.location(*dst)? {
                    ValueLocation::Register(home) => encoder.ldr_x(
                        XRegOrZero::Reg(home),
                        AddressBase::Reg(xreg(16)?),
                        byte_offset,
                    )?,
                    ValueLocation::Spill(_) => {
                        encoder.ldr_x(
                            XRegOrZero::Reg(xreg(13)?),
                            AddressBase::Reg(xreg(16)?),
                            byte_offset,
                        )?;
                        allocation.emit_store_scratch(&mut encoder, *dst, xreg(13)?)?;
                    }
                }
                push_range(
                    &mut ranges,
                    start,
                    code_offset(&encoder)?,
                    rip,
                    Some(to_u32(index)?),
                    StatePhase::PendingEffects,
                    SnapshotAdmissibility::ReplayCurrentInstruction,
                );
            }
            Operation::CheckedLoad { .. } => {
                return Err(BlockEmitterError::UnsupportedShape {
                    instruction_index: active_instruction.unwrap_or(0),
                });
            }
            Operation::CheckedStore { .. } => {
                return Err(BlockEmitterError::UnsupportedShape {
                    instruction_index: active_instruction.unwrap_or(0),
                });
            }
            Operation::Integer {
                kind,
                lhs,
                rhs,
                result,
                width,
                flags,
            } => {
                require_w64(*width, operation_index)?;
                if *flags != FlagEffect::Arithmetic {
                    return Err(BlockEmitterError::UnsupportedFlagEffect {
                        operation: operation_index,
                        effect: *flags,
                    });
                }
                let (rip, index) = active_guest(program, active_instruction)?;
                let boundary = HelperBoundaryPosition::BeforeOperation(operation_index);
                allocation.helper_boundary(boundary).map_err(|_| {
                    BlockEmitterError::MissingHelperBoundary {
                        operation: operation_index,
                    }
                })?;
                let pending_start = code_offset(&encoder)?;
                allocation.emit_save_before_helper(&mut encoder, boundary)?;
                allocation.emit_load_scratch(&mut encoder, *lhs, xreg(13)?)?;
                allocation.emit_load_scratch(&mut encoder, *rhs, xreg(14)?)?;
                encoder.mov_x(xreg(1)?, XRegOrZero::Reg(xreg(13)?));
                encoder.mov_x(xreg(2)?, XRegOrZero::Reg(xreg(14)?));
                encoder.movz_x(xreg(0)?, helper_operation_tag(*kind) as u16, 0)?;
                emit_saved_pointer_load(&mut encoder, FRAME_STAGING_FLAGS_POINTER, xreg(16)?)?;
                encoder.ldr_x(XRegOrZero::Reg(xreg(3)?), AddressBase::Reg(xreg(16)?), 0)?;
                encoder.adds_imm_x(
                    XRegOrZero::Reg(xreg(4)?),
                    XRegOrSp::Sp,
                    FRAME_HELPER_FLAGS_OUTPUT as u16,
                )?;
                emit_u64(&mut encoder, xreg(16)?, helper as usize as u64)?;
                let helper_start = code_offset(&encoder)?;
                push_range(
                    &mut ranges,
                    pending_start,
                    helper_start,
                    rip,
                    Some(to_u32(index)?),
                    StatePhase::PendingEffects,
                    SnapshotAdmissibility::ReplayCurrentInstruction,
                );
                push_range(
                    &mut ranges,
                    helper_start,
                    helper_start
                        .checked_add(4)
                        .ok_or(BlockEmitterError::CodeRangeOverflow)?,
                    rip,
                    Some(to_u32(index)?),
                    StatePhase::HelperCall,
                    SnapshotAdmissibility::ReplayCurrentInstruction,
                );
                encoder.blr(xreg(16)?);
                let failure = encoder.new_label()?;
                let success = encoder.new_label()?;
                encoder.cbz_w(crate::arm64::WReg::new(0)?, success)?;
                encoder.b(failure)?;
                let before_success = code_offset(&encoder)?;
                push_range(
                    &mut ranges,
                    before_success
                        .checked_sub(8)
                        .ok_or(BlockEmitterError::CodeRangeOverflow)?,
                    before_success,
                    rip,
                    Some(to_u32(index)?),
                    StatePhase::AfterHelper,
                    SnapshotAdmissibility::ReplayCurrentInstruction,
                );
                encoder.bind(success)?;
                allocation.emit_restore_after_helper(&mut encoder, boundary)?;
                encoder.ldr_x(
                    XRegOrZero::Reg(xreg(13)?),
                    AddressBase::Sp,
                    FRAME_HELPER_FLAGS_OUTPUT,
                )?;
                emit_saved_pointer_load(&mut encoder, FRAME_STAGING_FLAGS_POINTER, xreg(16)?)?;
                encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Reg(xreg(16)?), 0)?;
                allocation.emit_load_scratch(&mut encoder, *lhs, xreg(13)?)?;
                allocation.emit_load_scratch(&mut encoder, *rhs, xreg(14)?)?;
                match (kind, result) {
                    (IntegerKind::Add, Some(dst)) => {
                        encoder.add_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                        allocation.emit_store_scratch(&mut encoder, *dst, xreg(15)?)?;
                    }
                    (IntegerKind::Subtract, Some(dst)) => {
                        encoder.sub_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                        allocation.emit_store_scratch(&mut encoder, *dst, xreg(15)?)?;
                    }
                    (IntegerKind::Compare, None) => {
                        encoder.sub_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                    }
                    _ => {
                        return Err(BlockEmitterError::UnsupportedShape {
                            instruction_index: index,
                        });
                    }
                }
                push_range(
                    &mut ranges,
                    before_success,
                    code_offset(&encoder)?,
                    rip,
                    Some(to_u32(index)?),
                    StatePhase::AfterHelper,
                    SnapshotAdmissibility::ReplayCurrentInstruction,
                );
                helper_failures.push(ReturnStub {
                    label: failure,
                    raw_tag: RAW_HELPER_CONTRACT_FAILURE_V1,
                    guest_rip: GuestRipSource::Fixed(rip),
                    instruction_index: Some(to_u32(index)?),
                    snapshot: SnapshotAdmissibility::ReplayCurrentInstruction,
                });
            }
            Operation::WriteGpr {
                register,
                src,
                width,
            } => {
                require_w64(*width, operation_index)?;
                let (rip, index) = active_guest(program, active_instruction)?;
                let start = code_offset(&encoder)?;
                allocation.emit_load_scratch(&mut encoder, *src, xreg(13)?)?;
                let byte_offset = guest_gpr_offset(*register)?;
                emit_saved_pointer_load(&mut encoder, FRAME_STAGING_GPRS_POINTER, xreg(16)?)?;
                encoder.str_x(
                    XRegOrZero::Reg(xreg(13)?),
                    AddressBase::Reg(xreg(16)?),
                    byte_offset,
                )?;
                push_range(
                    &mut ranges,
                    start,
                    code_offset(&encoder)?,
                    rip,
                    Some(to_u32(index)?),
                    StatePhase::PendingEffects,
                    SnapshotAdmissibility::ReplayCurrentInstruction,
                );
            }
            Operation::CommitInstruction { index } => {
                let instruction = program.guest_instructions[*index];
                let branch = match program.operations.get(operation_index + 1) {
                    Some(Operation::Branch {
                        condition,
                        target_rip,
                        fallthrough_rip,
                    })
                    | Some(Operation::DispatchBranch {
                        condition,
                        target_rip,
                        fallthrough_rip,
                        ..
                    }) => Some((*condition, *target_rip, *fallthrough_rip)),
                    _ => None,
                };
                let start = code_offset(&encoder)?;
                if let Some((condition, target, fallthrough)) = branch {
                    emit_branch_rip_candidate(&mut encoder, condition, target, fallthrough)?;
                } else {
                    emit_u64(&mut encoder, xreg(13)?, instruction.next_rip)?;
                    emit_saved_pointer_load(&mut encoder, FRAME_STAGING_RIP_POINTER, xreg(16)?)?;
                    encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Reg(xreg(16)?), 0)?;
                }
                emit_commit(&mut encoder, *index, pair_count)?;
                push_range(
                    &mut ranges,
                    start,
                    code_offset(&encoder)?,
                    instruction.guest_rip,
                    Some(to_u32(*index)?),
                    StatePhase::CommitInProgress,
                    SnapshotAdmissibility::InadmissibleDuringCommit,
                );
                boundaries.push(GuestBoundaryPoint {
                    host_offset: code_offset(&encoder)?,
                    rip: if let Some((_, target_rip, fallthrough_rip)) = branch {
                        GuestRipBoundary::BranchSelection {
                            target_rip,
                            fallthrough_rip,
                        }
                    } else {
                        GuestRipBoundary::Fixed(instruction.next_rip)
                    },
                    instruction_index: to_u32(*index)?,
                    committed_instructions: to_u32(index + 1)?,
                    phase: StatePhase::Committed,
                });
                active_instruction = None;
            }
            operation @ (Operation::Branch { .. } | Operation::DispatchBranch { .. }) => {
                let start = code_offset(&encoder)?;
                let label = encoder.new_label()?;
                encoder.b(label)?;
                let index = program.guest_instructions.len() - 1;
                push_range_with_source(
                    &mut ranges,
                    start,
                    code_offset(&encoder)?,
                    GuestRipSource::CanonicalState,
                    Some(to_u32(index)?),
                    StatePhase::Exit,
                    SnapshotAdmissibility::CanonicalBoundary,
                );
                return_stubs.push(ReturnStub {
                    label,
                    raw_tag: if matches!(operation, Operation::DispatchBranch { .. }) {
                        RuntimeExitTag::DispatchCandidate as u32
                    } else {
                        RuntimeExitTag::BlockComplete as u32
                    },
                    guest_rip: GuestRipSource::CanonicalState,
                    instruction_index: Some(to_u32(index)?),
                    snapshot: SnapshotAdmissibility::CanonicalBoundary,
                });
            }
            Operation::Exit { kind, target_rip } => {
                if *kind != ExitKind::BlockComplete {
                    let start = code_offset(&encoder)?;
                    emit_exit_guards(&mut encoder, &mut return_stubs, *target_rip)?;
                    push_range(
                        &mut ranges,
                        start,
                        code_offset(&encoder)?,
                        *target_rip,
                        None,
                        StatePhase::BeforeInstruction,
                        SnapshotAdmissibility::CanonicalBoundary,
                    );
                }
                let start = code_offset(&encoder)?;
                let label = encoder.new_label()?;
                encoder.b(label)?;
                let index = program.guest_instructions.len() - 1;
                push_range(
                    &mut ranges,
                    start,
                    code_offset(&encoder)?,
                    *target_rip,
                    Some(to_u32(index)?),
                    StatePhase::Exit,
                    SnapshotAdmissibility::CanonicalBoundary,
                );
                return_stubs.push(ReturnStub {
                    label,
                    raw_tag: exit_tag(*kind),
                    guest_rip: GuestRipSource::Fixed(*target_rip),
                    instruction_index: Some(to_u32(index)?),
                    snapshot: SnapshotAdmissibility::CanonicalBoundary,
                });
            }
        }
    }

    // Stubs are out of line and individually attributed. Canonical RIP still
    // identifies the first uncommitted instruction on helper failure.
    for stub in return_stubs.iter().chain(helper_failures.iter()) {
        let start = code_offset(&encoder)?;
        encoder.bind(stub.label)?;
        emit_u64(&mut encoder, xreg(0)?, u64::from(stub.raw_tag))?;
        encoder.b(epilogue)?;
        push_range_with_source(
            &mut ranges,
            start,
            code_offset(&encoder)?,
            stub.guest_rip,
            stub.instruction_index,
            StatePhase::Exit,
            stub.snapshot,
        );
    }
    let epilogue_start = code_offset(&encoder)?;
    encoder.bind(epilogue)?;
    emit_epilogue(&mut encoder, pair_count)?;
    push_range_with_source(
        &mut ranges,
        epilogue_start,
        code_offset(&encoder)?,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::Service,
        SnapshotAdmissibility::CanonicalBoundary,
    );

    let code = encoder.finish()?;
    let bytes = code.to_le_bytes()?;
    let pc_map = PcMap::new(
        bytes.len(),
        program.guest_instructions.len(),
        ranges,
        boundaries,
    )
    .map_err(BlockEmitterError::PcMap)?;
    Ok(EmittedRuntimeBlock {
        code,
        pc_map,
        entry_rip,
        safe_entry,
    })
}

fn validate_runtime_shape(
    program: &Program,
    terminal_index: usize,
) -> Result<(), BlockEmitterError> {
    if program.guest_instructions.len() > MAX_BLOCK_INSTRUCTIONS {
        return Err(BlockEmitterError::UnsupportedShape {
            instruction_index: MAX_BLOCK_INSTRUCTIONS,
        });
    }

    let mut active = None;
    let mut integer_count = 0usize;
    let mut write_count = 0usize;
    let mut integer_kind = None;
    for (index, operation) in program.operations.iter().enumerate() {
        match operation {
            Operation::BeginInstruction { index } => {
                active = Some(*index);
                integer_count = 0;
                write_count = 0;
                integer_kind = None;
            }
            Operation::Integer {
                kind, width, flags, ..
            } => {
                let guest_index = active.ok_or(BlockEmitterError::UnsupportedShape {
                    instruction_index: 0,
                })?;
                require_w64(*width, index)?;
                if *flags != FlagEffect::Arithmetic {
                    return Err(BlockEmitterError::UnsupportedFlagEffect {
                        operation: index,
                        effect: *flags,
                    });
                }
                integer_count += 1;
                integer_kind = Some(*kind);
                if integer_count > 1 {
                    return Err(BlockEmitterError::UnsupportedShape {
                        instruction_index: guest_index,
                    });
                }
            }
            Operation::Constant { width, .. } | Operation::ReadGpr { width, .. } => {
                require_w64(*width, index)?;
            }
            Operation::CheckedLoad { .. } => {
                return Err(BlockEmitterError::UnsupportedShape {
                    instruction_index: active.unwrap_or(0),
                });
            }
            Operation::CheckedStore { .. } => {
                return Err(BlockEmitterError::UnsupportedShape {
                    instruction_index: active.unwrap_or(0),
                });
            }
            Operation::WriteGpr { width, .. } => {
                require_w64(*width, index)?;
                write_count += 1;
                if write_count > 1 {
                    return Err(BlockEmitterError::UnsupportedShape {
                        instruction_index: active.unwrap_or(0),
                    });
                }
            }
            Operation::CommitInstruction { index } => {
                let guest_index = active.ok_or(BlockEmitterError::UnsupportedShape {
                    instruction_index: *index,
                })?;
                let is_branch_record = matches!(
                    program.operations.get(terminal_index),
                    Some(Operation::Branch { .. } | Operation::DispatchBranch { .. })
                ) && guest_index + 1 == program.guest_instructions.len();
                if is_branch_record && (integer_count != 0 || write_count != 0) {
                    return Err(BlockEmitterError::UnsupportedShape {
                        instruction_index: guest_index,
                    });
                }
                if integer_kind == Some(IntegerKind::Compare) && write_count != 0 {
                    return Err(BlockEmitterError::UnsupportedShape {
                        instruction_index: guest_index,
                    });
                }
                active = None;
            }
            Operation::Branch { .. }
            | Operation::DispatchBranch { .. }
            | Operation::Exit { .. } => {}
        }
    }
    Ok(())
}

fn emit_prologue(encoder: &mut Encoder, pair_count: usize) -> Result<(), BlockEmitterError> {
    encoder.stp_x(
        xreg(29)?,
        xreg(30)?,
        AddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.adds_imm_x(XRegOrZero::Reg(xreg(29)?), XRegOrSp::Sp, 0)?;
    for _ in 0..pair_count {
        encoder.stp_x(
            xreg(13)?,
            xreg(14)?,
            AddressBase::Sp,
            -16,
            PairIndexMode::PreIndex,
        )?;
    }
    Ok(())
}

fn emit_runtime_frame_setup(encoder: &mut Encoder) -> Result<(), BlockEmitterError> {
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(16)?),
        AddressBase::Reg(xreg(0)?),
        RUNTIME_CONTEXT_STATE_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(16)?),
        AddressBase::Sp,
        FRAME_CANONICAL_STATE_POINTER,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(0)?),
        AddressBase::Sp,
        FRAME_RUNTIME_CONTEXT_POINTER,
    )?;

    for (field_offset, stack_slot) in [
        (FRAME_STAGING_GPRS_POINTER, FRAME_STAGING_GPRS),
        (FRAME_STAGING_FLAGS_POINTER, FRAME_STAGING_FLAGS),
        (FRAME_STAGING_RIP_POINTER, FRAME_STAGING_RIP),
    ] {
        encoder.adds_imm_x(XRegOrZero::Reg(xreg(17)?), XRegOrSp::Sp, stack_slot as u16)?;
        encoder.str_x(XRegOrZero::Reg(xreg(17)?), AddressBase::Sp, field_offset)?;
    }

    for register in 0..4u32 {
        let guest_offset = checked_offset(LONG_MODE_GPR_OFFSET, register * 8)?;
        let staged_offset = FRAME_STAGING_GPRS + register * 8;
        encoder.ldr_x(
            XRegOrZero::Reg(xreg(13)?),
            AddressBase::Reg(xreg(16)?),
            guest_offset,
        )?;
        encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Sp, staged_offset)?;
    }
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        LONG_MODE_RFLAGS_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Sp,
        FRAME_STAGING_FLAGS,
    )?;
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Sp,
        FRAME_STAGING_RIP,
    )?;

    encoder.ldr_x(
        XRegOrZero::Reg(xreg(16)?),
        AddressBase::Reg(xreg(0)?),
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(16)?),
        AddressBase::Reg(xreg(0)?),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.movz_x(xreg(16)?, 0, 0)?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(16)?),
        AddressBase::Reg(xreg(0)?),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    Ok(())
}

fn emit_instruction_guards(
    encoder: &mut Encoder,
    stubs: &mut Vec<ReturnStub>,
    guest_rip: u64,
    instruction_index: usize,
) -> Result<(), BlockEmitterError> {
    emit_boundary_guards(encoder, stubs, guest_rip, Some(instruction_index), true)
}

fn emit_exit_guards(
    encoder: &mut Encoder,
    stubs: &mut Vec<ReturnStub>,
    guest_rip: u64,
) -> Result<(), BlockEmitterError> {
    emit_boundary_guards(encoder, stubs, guest_rip, None, false)
}

fn emit_boundary_guards(
    encoder: &mut Encoder,
    stubs: &mut Vec<ReturnStub>,
    guest_rip: u64,
    instruction_index: Option<usize>,
    check_rip: bool,
) -> Result<(), BlockEmitterError> {
    let stop_disabled = encoder.new_label()?;
    let stop_address = new_exit_stub(
        encoder,
        stubs,
        RuntimeExitTag::StopAddress as u32,
        guest_rip,
        instruction_index,
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    emit_context_load(
        encoder,
        RUNTIME_CONTEXT_STOP_RIP_ENABLED_OFFSET as u32,
        xreg(16)?,
    )?;
    encoder.cbz_x(XRegOrZero::Reg(xreg(16)?), stop_disabled)?;
    emit_context_load(encoder, RUNTIME_CONTEXT_STOP_RIP_OFFSET as u32, xreg(14)?)?;
    emit_state_load(encoder, LONG_MODE_RIP_OFFSET as u32, xreg(15)?)?;
    encoder.subs_x(XRegOrZero::Zero, xreg(15)?, XRegOrZero::Reg(xreg(14)?));
    encoder.b_cond(Condition::Equal, stop_address)?;
    encoder.bind(stop_disabled)?;

    let stop_clear = encoder.new_label()?;
    let stop_requested = new_exit_stub(
        encoder,
        stubs,
        RuntimeExitTag::StopRequested as u32,
        guest_rip,
        instruction_index,
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    emit_context_load(
        encoder,
        RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET as u32,
        xreg(16)?,
    )?;
    encoder.ldarb_w(crate::arm64::WReg::new(17)?, AddressBase::Reg(xreg(16)?));
    encoder.cbz_w(crate::arm64::WReg::new(17)?, stop_clear)?;
    encoder.b(stop_requested)?;
    encoder.bind(stop_clear)?;

    let budget_exhausted = new_exit_stub(
        encoder,
        stubs,
        RuntimeExitTag::BudgetExhausted as u32,
        guest_rip,
        instruction_index,
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    emit_context_load(
        encoder,
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
        xreg(16)?,
    )?;
    encoder.cbz_x(XRegOrZero::Reg(xreg(16)?), budget_exhausted)?;

    if check_rip {
        let rip_matches = encoder.new_label()?;
        let wrong_rip = new_exit_stub(
            encoder,
            stubs,
            RuntimeExitTag::NeedInterpreter as u32,
            guest_rip,
            instruction_index,
            SnapshotAdmissibility::CanonicalBoundary,
        )?;
        emit_state_load(encoder, LONG_MODE_RIP_OFFSET as u32, xreg(15)?)?;
        emit_u64(encoder, xreg(14)?, guest_rip)?;
        encoder.subs_x(XRegOrZero::Zero, xreg(15)?, XRegOrZero::Reg(xreg(14)?));
        encoder.b_cond(Condition::Equal, rip_matches)?;
        encoder.b(wrong_rip)?;
        encoder.bind(rip_matches)?;
    }
    Ok(())
}

fn new_exit_stub(
    encoder: &mut Encoder,
    stubs: &mut Vec<ReturnStub>,
    raw_tag: u32,
    guest_rip: u64,
    instruction_index: Option<usize>,
    snapshot: SnapshotAdmissibility,
) -> Result<crate::arm64::Label, BlockEmitterError> {
    let label = encoder.new_label()?;
    stubs.push(ReturnStub {
        label,
        raw_tag,
        guest_rip: GuestRipSource::Fixed(guest_rip),
        instruction_index: instruction_index.map(to_u32).transpose()?,
        snapshot,
    });
    Ok(label)
}

fn emit_commit(
    encoder: &mut Encoder,
    instruction_index: usize,
    _pair_count: usize,
) -> Result<(), BlockEmitterError> {
    for register in 0..4u32 {
        encoder.ldr_x(
            XRegOrZero::Reg(xreg(13)?),
            AddressBase::Sp,
            FRAME_STAGING_GPRS + register * 8,
        )?;
        emit_saved_pointer_load(encoder, FRAME_CANONICAL_STATE_POINTER, xreg(16)?)?;
        encoder.str_x(
            XRegOrZero::Reg(xreg(13)?),
            AddressBase::Reg(xreg(16)?),
            checked_offset(LONG_MODE_GPR_OFFSET, register * 8)?,
        )?;
    }
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Sp,
        FRAME_STAGING_FLAGS,
    )?;
    emit_saved_pointer_load(encoder, FRAME_CANONICAL_STATE_POINTER, xreg(16)?)?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        LONG_MODE_RFLAGS_OFFSET as u32,
    )?;
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Sp,
        FRAME_STAGING_RIP,
    )?;
    emit_saved_pointer_load(encoder, FRAME_CANONICAL_STATE_POINTER, xreg(16)?)?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;

    emit_saved_pointer_load(encoder, FRAME_RUNTIME_CONTEXT_POINTER, xreg(16)?)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.subs_imm_x(XRegOrZero::Reg(xreg(13)?), XRegOrSp::Reg(xreg(13)?), 1)?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    encoder.adds_imm_x(XRegOrZero::Reg(xreg(13)?), XRegOrSp::Reg(xreg(13)?), 1)?;
    encoder.str_x(
        XRegOrZero::Reg(xreg(13)?),
        AddressBase::Reg(xreg(16)?),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    let _ = instruction_index;
    Ok(())
}

fn emit_branch_rip_candidate(
    encoder: &mut Encoder,
    condition: BranchCondition,
    target_rip: u64,
    fallthrough_rip: u64,
) -> Result<(), BlockEmitterError> {
    let selected = encoder.new_label()?;
    emit_saved_pointer_load(encoder, FRAME_STAGING_FLAGS_POINTER, xreg(16)?)?;
    encoder.ldr_x(XRegOrZero::Reg(xreg(17)?), AddressBase::Reg(xreg(16)?), 0)?;
    emit_u64(encoder, xreg(14)?, integer_flags::ZERO_FLAG)?;
    encoder.and_x(xreg(15)?, xreg(17)?, XRegOrZero::Reg(xreg(14)?));
    match condition {
        BranchCondition::Zero => {
            let not_taken = encoder.new_label()?;
            encoder.cbz_x(XRegOrZero::Reg(xreg(15)?), not_taken)?;
            emit_u64(encoder, xreg(16)?, target_rip)?;
            encoder.b(selected)?;
            encoder.bind(not_taken)?;
            emit_u64(encoder, xreg(16)?, fallthrough_rip)?;
        }
        BranchCondition::NotZero => {
            let taken = encoder.new_label()?;
            encoder.cbz_x(XRegOrZero::Reg(xreg(15)?), taken)?;
            emit_u64(encoder, xreg(16)?, fallthrough_rip)?;
            encoder.b(selected)?;
            encoder.bind(taken)?;
            emit_u64(encoder, xreg(16)?, target_rip)?;
        }
    }
    encoder.bind(selected)?;
    emit_saved_pointer_load(encoder, FRAME_STAGING_RIP_POINTER, xreg(17)?)?;
    encoder.str_x(XRegOrZero::Reg(xreg(16)?), AddressBase::Reg(xreg(17)?), 0)?;
    Ok(())
}

fn emit_epilogue(encoder: &mut Encoder, pair_count: usize) -> Result<(), BlockEmitterError> {
    for _ in 0..pair_count {
        encoder.ldp_x(
            xreg(13)?,
            xreg(14)?,
            AddressBase::Sp,
            16,
            PairIndexMode::PostIndex,
        )?;
    }
    encoder.ldp_x(
        xreg(29)?,
        xreg(30)?,
        AddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ret(xreg(30)?);
    Ok(())
}

fn emit_context_load(
    encoder: &mut Encoder,
    offset: u32,
    dst: XReg,
) -> Result<(), BlockEmitterError> {
    emit_saved_pointer_load(encoder, FRAME_RUNTIME_CONTEXT_POINTER, xreg(16)?)?;
    encoder.ldr_x(XRegOrZero::Reg(dst), AddressBase::Reg(xreg(16)?), offset)?;
    Ok(())
}

fn emit_state_load(encoder: &mut Encoder, offset: u32, dst: XReg) -> Result<(), BlockEmitterError> {
    emit_saved_pointer_load(encoder, FRAME_CANONICAL_STATE_POINTER, xreg(16)?)?;
    encoder.ldr_x(XRegOrZero::Reg(dst), AddressBase::Reg(xreg(16)?), offset)?;
    Ok(())
}

fn emit_saved_pointer_load(
    encoder: &mut Encoder,
    offset: u32,
    dst: XReg,
) -> Result<(), BlockEmitterError> {
    encoder.ldr_x(XRegOrZero::Reg(dst), AddressBase::Sp, offset)?;
    Ok(())
}

fn emit_constant(
    encoder: &mut Encoder,
    allocation: &Allocation<'_>,
    temp: TempId,
    value: u64,
) -> Result<(), BlockEmitterError> {
    let destination = match allocation.location(temp)? {
        ValueLocation::Register(register) => register,
        ValueLocation::Spill(_) => xreg(13)?,
    };
    emit_u64(encoder, destination, value)?;
    if matches!(allocation.location(temp)?, ValueLocation::Spill(_)) {
        allocation.emit_store_scratch(encoder, temp, destination)?;
    }
    Ok(())
}

fn emit_u64(encoder: &mut Encoder, destination: XReg, value: u64) -> Result<(), BlockEmitterError> {
    encoder.movz_x(destination, value as u16, 0)?;
    for halfword in 1..4 {
        encoder.movk_x(
            destination,
            (value >> (halfword * 16)) as u16,
            halfword as u8,
        )?;
    }
    Ok(())
}

fn guest_gpr_offset(register: GuestGpr) -> Result<u32, BlockEmitterError> {
    if register.0 >= 4 {
        return Err(BlockEmitterError::InvalidRegister(register));
    }
    Ok(u32::from(register.0) * 8)
}

fn active_guest(
    program: &Program,
    active: Option<usize>,
) -> Result<(u64, usize), BlockEmitterError> {
    let index = active.ok_or(BlockEmitterError::UnsupportedShape {
        instruction_index: 0,
    })?;
    Ok((program.guest_instructions[index].guest_rip, index))
}

fn helper_operation_tag(kind: IntegerKind) -> HelperOperationTag {
    match kind {
        IntegerKind::Add => HelperOperationTag::Add,
        IntegerKind::Subtract => HelperOperationTag::Subtract,
        IntegerKind::Compare => HelperOperationTag::Compare,
    }
}

fn exit_tag(kind: ExitKind) -> u32 {
    match kind {
        ExitKind::BlockComplete => RuntimeExitTag::BlockComplete as u32,
        ExitKind::NeedInterpreter => RuntimeExitTag::NeedInterpreter as u32,
        ExitKind::Unsupported => RuntimeExitTag::Unsupported as u32,
    }
}

fn require_w64(width: IntWidth, operation: usize) -> Result<(), BlockEmitterError> {
    if width == IntWidth::W64 {
        Ok(())
    } else {
        Err(BlockEmitterError::UnsupportedWidth { operation, width })
    }
}

fn push_range(
    ranges: &mut Vec<StatePointRange>,
    start: u32,
    end: u32,
    guest_rip: u64,
    instruction_index: Option<u32>,
    phase: StatePhase,
    snapshot: SnapshotAdmissibility,
) {
    push_range_with_source(
        ranges,
        start,
        end,
        GuestRipSource::Fixed(guest_rip),
        instruction_index,
        phase,
        snapshot,
    );
}

fn push_range_with_source(
    ranges: &mut Vec<StatePointRange>,
    start: u32,
    end: u32,
    guest_rip: GuestRipSource,
    instruction_index: Option<u32>,
    phase: StatePhase,
    snapshot: SnapshotAdmissibility,
) {
    if start != end {
        ranges.push(StatePointRange {
            range: HostCodeRange::new(start, end),
            guest_rip,
            instruction_index,
            phase,
            snapshot,
        });
    }
}

fn code_offset(encoder: &Encoder) -> Result<u32, BlockEmitterError> {
    encoder
        .position()
        .checked_mul(4)
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or(BlockEmitterError::CodeRangeOverflow)
}

fn to_u32(value: usize) -> Result<u32, BlockEmitterError> {
    u32::try_from(value).map_err(|_| BlockEmitterError::CodeRangeOverflow)
}

fn checked_offset(base: usize, add: u32) -> Result<u32, BlockEmitterError> {
    base.checked_add(add as usize)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(BlockEmitterError::CodeRangeOverflow)
}

fn xreg(number: u8) -> Result<XReg, BlockEmitterError> {
    Ok(XReg::new(number)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocator::AllocatorConfig;
    use crate::ir::{GuestInstruction, Operation};

    fn mov_program() -> Program {
        Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: 0x1000,
                next_rip: 0x100A,
            }],
            code_end: 0x100A,
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::Constant {
                    dst: TempId(1),
                    width: IntWidth::W64,
                    value: 0xFEDC_BA98_7654_3210,
                },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(1),
                    width: IntWidth::W64,
                },
                Operation::CommitInstruction { index: 0 },
                Operation::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: 0x100A,
                },
            ],
        }
    }

    #[test]
    fn emits_runtime_entry_and_complete_pc_map_for_one_committed_instruction() {
        let program = mov_program();
        let allocation = Allocation::build(
            &program,
            AllocatorConfig {
                register_homes: 1,
                reserved_prefix_bytes: 112,
            },
            &[],
        )
        .unwrap();
        let block = emit_runtime_block(&program, &allocation).unwrap();
        assert!(block.safe_entry_provenance());
        assert_eq!(block.entry_rip(), 0x1000);
        assert_eq!(block.pc_map().instruction_count(), 1);
        assert_eq!(
            block.pc_map().ranges().first().unwrap().phase,
            StatePhase::Service
        );
        assert!(
            block
                .pc_map()
                .ranges()
                .iter()
                .any(|entry| entry.phase == StatePhase::CommitInProgress
                    && entry.snapshot == SnapshotAdmissibility::InadmissibleDuringCommit)
        );
        assert_eq!(block.pc_map().boundaries().len(), 1);
        assert_eq!(
            block.pc_map().boundaries()[0].rip,
            GuestRipBoundary::Fixed(0x100A)
        );
    }
}
