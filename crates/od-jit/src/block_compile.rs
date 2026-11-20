//! Compile one verified, bounded guest block into runtime-ready ARM64 code.

use od_isa::memory::AccessContext;

use crate::allocator::{Allocation, AllocatorConfig, AllocatorError, HelperBoundaryPosition};
use crate::block::{
    self, AdmittedInstructionBoundaries, BlockLimits, BlockStopReason, BuildError, BuildOutcome,
    CodeView, DecodeBoundary, NativeRegisterV1Admission, S0_09NopAddRaxImm8Admission,
    S0_09NopAdmission, S0_09NopCmpRaxImm8Admission, S0_09NopDecRaxAdmission,
    S0_09NopIncRaxAdmission, S0_09NopLeaRaxRbxAdmission, S0_09NopLeaRaxRbxDisp8Admission,
    S0_09NopLeaRaxRbxRcxAdmission, S0_09NopLeaRaxRipDisp32Admission, S0_09NopMovRaxImm32Admission,
    S0_09NopNegRaxAdmission, S0_09NopNotRaxAdmission, S0_09NopSubRaxImm8Admission,
    S0_09NopXchgRaxRbxAdmission,
};
use crate::block_emitter::{
    BlockEmitterError, EmittedCheckedLoadBlockV2, EmittedCheckedStoreBlockV2, EmittedRuntimeBlock,
    emit_checked_load_runtime_block_v2, emit_checked_store_runtime_block_v2, emit_runtime_block,
    emit_two_memory_runtime_block_v2,
};
use crate::ir::Operation;
use crate::lowering::{self, LoweredProgram, LoweringError, LoweringOutcome};
use crate::phase_observation::{DispatchPhase, NoopObserver, PhaseObserver, measure};

pub const MAX_EMITTED_BLOCK_BYTES: usize = 4096;
pub const RUNTIME_FRAME_PREFIX_BYTES: u32 = 176;
const DEMAND_FLAGS_RUNTIME_FRAME_PREFIX_BYTES: u32 = 192;
const CHECKED_LOAD_FRAME_PREFIX_BYTES_V2: u32 = 176;
const CHECKED_STORE_FRAME_PREFIX_BYTES_V2: u32 = 176;
const REGISTER_HOME_BUDGET: u8 = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledBlock {
    emitted: EmittedRuntimeBlock,
    code_end: u64,
    deferred_error: Option<DecodeBoundary>,
}

impl CompiledBlock {
    pub fn emitted(&self) -> &EmittedRuntimeBlock {
        &self.emitted
    }

    pub const fn code_end(&self) -> u64 {
        self.code_end
    }

    pub fn deferred_error(&self) -> Option<&DecodeBoundary> {
        self.deferred_error.as_ref()
    }

    pub(crate) fn logical_storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>().checked_add(self.emitted.logical_storage_bytes()?)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockCompileOutcome {
    Compiled(CompiledBlock),
    NoNativePrefix {
        rip: u64,
        reason: BlockStopReason,
        deferred_error: Option<DecodeBoundary>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockCompileError {
    Build(BuildError),
    Lowering(LoweringError),
    Allocation(AllocatorError),
    Emitter(BlockEmitterError),
    EmittedCodeTooLarge { actual: usize, max: usize },
    DispatchAdmissionDoesNotMatchCode,
    DispatchEntryNotAdmitted { entry_rip: u64 },
    DispatchBlockBoundaryNotAdmitted { rip: u64 },
    NopAdmissionDoesNotMatchCode,
    NopEntryNotAdmitted { entry_rip: u64 },
    NativeRegisterV1AdmissionDoesNotMatchCode,
    NativeRegisterV1EntryNotAdmitted { entry_rip: u64 },
    S3_32aAdmissionDoesNotMatchCode,
    S3_32aEntryNotAdmitted { entry_rip: u64 },
}

/// Build and compile a single bounded block with the current runtime profile.
/// `NoNativePrefix` remains a distinct outcome; it is not converted into an
/// empty Program or a successful compiled block.
pub fn compile_block(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_block_inner(
        code,
        entry_rip,
        None,
        None,
        None,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile one uncached block with an explicit register-home count for
/// allocator cost experiments. Production and cache paths continue to use the
/// default four-home profile.
pub fn compile_block_with_register_homes(
    code: CodeView<'_>,
    entry_rip: u64,
    register_homes: u8,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_block_inner_with_register_homes(
        code,
        entry_rip,
        None,
        None,
        None,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
        register_homes,
    )
}

/// Compile the exact S3-32a alias-read candidate through the ordinary
/// verifier, allocator, and emitter. This entry is uncached and opt-in.
pub fn compile_block_for_s3_32a_add_rax_rax_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &block::S3_32aAddRaxRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::S3_32aAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) || !admission.admits_add_rax_rax(entry_rip) {
        return Err(BlockCompileError::S3_32aEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_s3_32a_add_rax_rax_candidate(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(
        code.code_end(),
        lowered,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile an uncached, opt-in demand-driven arithmetic-flags candidate for
/// correctness comparison. Its result must not be inserted into the shared
/// translation cache; eager remains the production/reference strategy until
/// the final Stage 3 comparison is complete.
pub fn compile_block_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_block_inner(
        code,
        entry_rip,
        None,
        None,
        None,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

/// Compile only the exact checked qword-load profile through the shared block
/// builder, typed lowering, allocator, and runtime emitter. The result has a
/// distinct V2 entry type and cannot enter the V1 executor.
pub(crate) fn compile_checked_load_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<EmittedCheckedLoadBlockV2, BlockCompileError> {
    let built = block::build_checked_qword_load_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower(&built).map_err(BlockCompileError::Lowering)? {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix { .. } => {
            return Err(BlockCompileError::Emitter(
                BlockEmitterError::UnsupportedShape {
                    instruction_index: 0,
                },
            ));
        }
    };
    let allocation = allocate_block(&lowered, CHECKED_LOAD_FRAME_PREFIX_BYTES_V2)?;
    let emitted = emit_checked_load_runtime_block_v2(&lowered.program, &allocation)
        .map_err(BlockCompileError::Emitter)?;
    let actual = emitted
        .code()
        .to_le_bytes()
        .map_err(|error| BlockCompileError::Emitter(BlockEmitterError::Encoding(error)))?
        .len();
    if actual > MAX_EMITTED_BLOCK_BYTES {
        return Err(BlockCompileError::EmittedCodeTooLarge {
            actual,
            max: MAX_EMITTED_BLOCK_BYTES,
        });
    }
    Ok(emitted)
}

pub(crate) fn compile_checked_dword_load_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<EmittedCheckedLoadBlockV2, BlockCompileError> {
    let built = block::build_checked_dword_load_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower(&built).map_err(BlockCompileError::Lowering)? {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix { .. } => {
            return Err(BlockCompileError::Emitter(
                BlockEmitterError::UnsupportedShape {
                    instruction_index: 0,
                },
            ));
        }
    };
    let allocation = allocate_block(&lowered, CHECKED_LOAD_FRAME_PREFIX_BYTES_V2)?;
    let emitted = emit_checked_load_runtime_block_v2(&lowered.program, &allocation)
        .map_err(BlockCompileError::Emitter)?;
    let actual = emitted
        .code()
        .to_le_bytes()
        .map_err(|error| BlockCompileError::Emitter(BlockEmitterError::Encoding(error)))?
        .len();
    if actual > MAX_EMITTED_BLOCK_BYTES {
        return Err(BlockCompileError::EmittedCodeTooLarge {
            actual,
            max: MAX_EMITTED_BLOCK_BYTES,
        });
    }
    Ok(emitted)
}

pub(crate) fn compile_checked_dword_store_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<EmittedCheckedStoreBlockV2, BlockCompileError> {
    let built = block::build_checked_dword_store_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower(&built).map_err(BlockCompileError::Lowering)? {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix { .. } => {
            return Err(BlockCompileError::Emitter(
                BlockEmitterError::UnsupportedShape {
                    instruction_index: 0,
                },
            ));
        }
    };
    let allocation = allocate_block(&lowered, CHECKED_LOAD_FRAME_PREFIX_BYTES_V2)?;
    let emitted = emit_checked_store_runtime_block_v2(&lowered.program, &allocation)
        .map_err(BlockCompileError::Emitter)?;
    let actual = emitted
        .code()
        .to_le_bytes()
        .map_err(|error| BlockCompileError::Emitter(BlockEmitterError::Encoding(error)))?
        .len();
    if actual > MAX_EMITTED_BLOCK_BYTES {
        return Err(BlockCompileError::EmittedCodeTooLarge {
            actual,
            max: MAX_EMITTED_BLOCK_BYTES,
        });
    }
    Ok(emitted)
}

pub(crate) fn compile_checked_store_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<EmittedCheckedStoreBlockV2, BlockCompileError> {
    let built = block::build_checked_qword_store_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower(&built).map_err(BlockCompileError::Lowering)? {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix { .. } => {
            return Err(BlockCompileError::Emitter(
                BlockEmitterError::UnsupportedShape {
                    instruction_index: 0,
                },
            ));
        }
    };
    let allocation = allocate_block(&lowered, CHECKED_STORE_FRAME_PREFIX_BYTES_V2)?;
    let emitted = emit_checked_store_runtime_block_v2(&lowered.program, &allocation)
        .map_err(BlockCompileError::Emitter)?;
    let actual = emitted
        .code()
        .to_le_bytes()
        .map_err(|error| BlockCompileError::Emitter(BlockEmitterError::Encoding(error)))?
        .len();
    if actual > MAX_EMITTED_BLOCK_BYTES {
        return Err(BlockCompileError::EmittedCodeTooLarge {
            actual,
            max: MAX_EMITTED_BLOCK_BYTES,
        });
    }
    Ok(emitted)
}

pub(crate) fn compile_checked_two_memory_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<crate::block_emitter::EmittedTwoMemoryBlockV2, BlockCompileError> {
    let built = block::build_checked_qword_two_site_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower(&built).map_err(BlockCompileError::Lowering)? {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix { .. } => {
            return Err(BlockCompileError::Emitter(
                BlockEmitterError::UnsupportedShape {
                    instruction_index: 0,
                },
            ));
        }
    };
    if lowered.program.guest_instructions.len() != 2 {
        return Err(BlockCompileError::Emitter(
            BlockEmitterError::UnsupportedShape {
                instruction_index: lowered.program.guest_instructions.len(),
            },
        ));
    }
    let allocation = allocate_block(&lowered, CHECKED_STORE_FRAME_PREFIX_BYTES_V2)?;
    let emitted = emit_two_memory_runtime_block_v2(&lowered.program, &allocation)
        .map_err(BlockCompileError::Emitter)?;
    let actual = emitted
        .code()
        .to_le_bytes()
        .map_err(|error| BlockCompileError::Emitter(BlockEmitterError::Encoding(error)))?
        .len();
    if actual > MAX_EMITTED_BLOCK_BYTES {
        return Err(BlockCompileError::EmittedCodeTooLarge {
            actual,
            max: MAX_EMITTED_BLOCK_BYTES,
        });
    }
    Ok(emitted)
}

/// Compile one block with a typed Jcc dispatch handoff, using boundaries from
/// complete fixture-v1 decoder admission. This does not dispatch or retry the
/// selected target; it commits once and returns `DispatchCandidate`.
pub fn compile_block_for_dispatch(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &AdmittedInstructionBoundaries,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let built = build_dispatch_block(code, entry_rip, admission)?;
    compile_prepared_dispatch_block(code.code_end(), built, admission)
}

/// Lower one dispatch block through the exact production path for opt-in
/// allocator census tooling. This exposes no runtime policy and does not emit
/// or publish code; the caller can inspect the verified IR and allocator
/// mapping while checking its emission against [`compile_block_for_dispatch`].
#[cfg(feature = "phase-observation")]
#[doc(hidden)]
pub fn lower_dispatch_block_for_observation(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &AdmittedInstructionBoundaries,
) -> Result<LoweringOutcome, BlockCompileError> {
    let built = build_dispatch_block(code, entry_rip, admission)?;
    let lowered =
        lowering::lower_for_dispatch(&built, admission).map_err(BlockCompileError::Lowering)?;
    Ok(lowered)
}

/// Decode and validate the source block once so the cache path can construct
/// a full translation key before deciding whether native lowering is needed.
pub(crate) fn build_dispatch_block(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &AdmittedInstructionBoundaries,
) -> Result<BuildOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::DispatchAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::DispatchEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    Ok(built)
}

pub(crate) fn compile_prepared_dispatch_block(
    code_end: u64,
    built: BuildOutcome,
    admission: &AdmittedInstructionBoundaries,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let mut observer = NoopObserver;
    compile_prepared_dispatch_block_observed(code_end, built, admission, &mut observer)
}

pub(crate) fn compile_prepared_dispatch_block_observed<O: PhaseObserver>(
    code_end: u64,
    built: BuildOutcome,
    admission: &AdmittedInstructionBoundaries,
    observer: &mut O,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let lowered_outcome = measure(observer, DispatchPhase::IrLowering, || {
        lowering::lower_for_dispatch(&built, admission)
    })
    .map_err(BlockCompileError::Lowering)?;
    let lowered = match lowered_outcome {
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
        LoweringOutcome::Program(lowered) => lowered,
    };
    compile_lowered_observed(
        code_end,
        lowered,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
        observer,
    )
}

/// Compile one native prefix for the separately admitted S0-09-plus-NOP
/// interpreter fallback path. Fixture-v1 dispatcher callers keep their own
/// admission API and are not widened by this helper.
pub fn compile_block_for_nop_fallback(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    compile_block_inner(
        code,
        entry_rip,
        Some(admission.boundaries()),
        Some(admission),
        None,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile an uncached demand-flags candidate for the existing bounded
/// register-only prefix followed by admitted bare NOP bytes. The candidate
/// returns canonical state at the fallback boundary and is not wired to the
/// shared translation cache.
pub fn compile_block_for_nop_fallback_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    compile_block_inner(
        code,
        entry_rip,
        Some(admission.boundaries()),
        Some(admission),
        None,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

pub fn compile_block_for_nop_inc_rax_fallback(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopIncRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    compile_block_inner(
        code,
        entry_rip,
        Some(admission.boundaries()),
        None,
        Some(admission),
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

pub fn compile_block_for_nop_inc_rax_fallback_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopIncRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    compile_block_inner(
        code,
        entry_rip,
        Some(admission.boundaries()),
        None,
        Some(admission),
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

/// Compile an exact INC RAX native candidate. This API is opt-in and uncached;
/// the established interpreter-bridge path remains available independently.
pub fn compile_block_for_native_inc_rax(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopIncRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_inc_rax_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile an uncached demand-flags candidate for the exact INC RAX path.
pub fn compile_block_for_native_inc_rax_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopIncRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_inc_rax_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_inc_rax_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopIncRaxAdmission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_inc_rax(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_inc_rax(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare W64 DEC RAX as an opt-in uncached native candidate.
pub fn compile_block_for_native_dec_rax(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopDecRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_dec_rax_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile an uncached demand-flags candidate for exact bare W64 DEC RAX.
pub fn compile_block_for_native_dec_rax_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopDecRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_dec_rax_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

/// Compile exact bare W64 NEG RAX as a separate opt-in uncached candidate.
pub fn compile_block_for_native_neg_rax(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopNegRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_neg_rax_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags variant of the exact native NEG RAX candidate.
pub fn compile_block_for_native_neg_rax_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopNegRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_neg_rax_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_neg_rax_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopNegRaxAdmission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_neg_rax(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_neg_rax(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare W64 NOT RAX as a separate opt-in uncached candidate.
pub fn compile_block_for_native_not_rax(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopNotRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_not_rax_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare NOT RAX.
pub fn compile_block_for_native_not_rax_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopNotRaxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_not_rax_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_not_rax_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopNotRaxAdmission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_not_rax(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_not_rax(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare `LEA RAX,[RBX]` as a separate opt-in uncached path.
pub fn compile_block_for_native_lea_rax_rbx(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_lea_rax_rbx_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare `LEA RAX,[RBX]`.
pub fn compile_block_for_native_lea_rax_rbx_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_lea_rax_rbx_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_lea_rax_rbx_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxAdmission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_lea_rax_rbx(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_lea_rax_rbx(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare `LEA RAX,[RBX+disp8]` as a separate opt-in uncached path.
pub fn compile_block_for_native_lea_rax_rbx_disp8(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxDisp8Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_lea_rax_rbx_disp8_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare `LEA RAX,[RBX+disp8]`.
pub fn compile_block_for_native_lea_rax_rbx_disp8_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxDisp8Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_lea_rax_rbx_disp8_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_lea_rax_rbx_disp8_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxDisp8Admission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_lea_rax_rbx_disp8(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_lea_rax_rbx_disp8(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare `LEA RAX,[RBX+RCX]` as a separate opt-in uncached path.
pub fn compile_block_for_native_lea_rax_rbx_rcx(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxRcxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_lea_rax_rbx_rcx_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare `LEA RAX,[RBX+RCX]`.
pub fn compile_block_for_native_lea_rax_rbx_rcx_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxRcxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_lea_rax_rbx_rcx_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_lea_rax_rbx_rcx_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRbxRcxAdmission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_lea_rax_rbx_rcx(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_lea_rax_rbx_rcx(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare `LEA RAX,[RIP+disp32]` as a separate opt-in uncached path.
pub fn compile_block_for_native_lea_rax_rip_disp32(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRipDisp32Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_lea_rax_rip_disp32_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare `LEA RAX,[RIP+disp32]`.
pub fn compile_block_for_native_lea_rax_rip_disp32_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRipDisp32Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_lea_rax_rip_disp32_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_lea_rax_rip_disp32_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopLeaRaxRipDisp32Admission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_lea_rax_rip_disp32(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_lea_rax_rip_disp32(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare `XCHG RAX,RBX` as a separate opt-in uncached path.
pub fn compile_block_for_native_xchg_rax_rbx(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopXchgRaxRbxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_xchg_rax_rbx_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare `XCHG RAX,RBX`.
pub fn compile_block_for_native_xchg_rax_rbx_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopXchgRaxRbxAdmission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_xchg_rax_rbx_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_xchg_rax_rbx_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopXchgRaxRbxAdmission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_xchg_rax_rbx(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_xchg_rax_rbx(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare W64 `ADD RAX, sign-extended imm8` as an uncached opt-in block.
pub fn compile_block_for_native_add_rax_imm8(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopAddRaxImm8Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_add_rax_imm8_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare `ADD RAX, sign-extended imm8`.
pub fn compile_block_for_native_add_rax_imm8_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopAddRaxImm8Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_add_rax_imm8_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_add_rax_imm8_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopAddRaxImm8Admission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_add_rax_imm8(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_add_rax_imm8(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile the exact native `CMP RAX, sign-extended imm8` profile eagerly.
pub fn compile_block_for_native_cmp_rax_imm8(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopCmpRaxImm8Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_cmp_rax_imm8_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare `CMP RAX, sign-extended imm8`.
pub fn compile_block_for_native_cmp_rax_imm8_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopCmpRaxImm8Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_cmp_rax_imm8_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_cmp_rax_imm8_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopCmpRaxImm8Admission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_cmp_rax_imm8(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_cmp_rax_imm8(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare W64 `SUB RAX, sign-extended imm8` eagerly.
pub fn compile_block_for_native_sub_rax_imm8(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopSubRaxImm8Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_sub_rax_imm8_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare `SUB RAX, sign-extended imm8`.
pub fn compile_block_for_native_sub_rax_imm8_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopSubRaxImm8Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_sub_rax_imm8_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_sub_rax_imm8_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopSubRaxImm8Admission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_sub_rax_imm8(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_sub_rax_imm8(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile exact bare W64 `MOV RAX, sign-extended imm32` eagerly.
pub fn compile_block_for_native_mov_rax_imm32(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopMovRaxImm32Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_mov_rax_imm32_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile the demand-flags candidate for exact bare MOV RAX, sign-extended imm32.
pub fn compile_block_for_native_mov_rax_imm32_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopMovRaxImm32Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_mov_rax_imm32_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_mov_rax_imm32_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopMovRaxImm32Admission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_mov_rax_imm32(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_mov_rax_imm32(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

/// Compile the bounded S3-33n exact-form union eagerly. This API is uncached
/// and opt-in; it does not alter `compile_block` or existing admissions.
pub fn compile_block_for_native_register_v1(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &NativeRegisterV1Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_register_v1_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::Eager,
    )
}

/// Compile an uncached demand-flags candidate for the exact S3-33n union.
pub fn compile_block_for_native_register_v1_demand_flags_candidate(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &NativeRegisterV1Admission,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_native_register_v1_inner(
        code,
        entry_rip,
        admission,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate,
    )
}

fn compile_native_register_v1_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &NativeRegisterV1Admission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NativeRegisterV1AdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NativeRegisterV1EntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_register_v1(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_register_v1(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

fn compile_native_dec_rax_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: &S0_09NopDecRaxAdmission,
    strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    if !admission.matches_code(code) {
        return Err(BlockCompileError::NopAdmissionDoesNotMatchCode);
    }
    if !admission.contains(entry_rip) {
        return Err(BlockCompileError::NopEntryNotAdmitted { entry_rip });
    }
    let built = block::build_block_for_native_dec_rax(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower_for_native_dec_rax(&built, admission)
        .map_err(BlockCompileError::Lowering)?
    {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
    };
    if let BuildOutcome::Block(block) = &built {
        for instruction in &block.instructions {
            for rip in [instruction.guest_rip, instruction.next_rip] {
                if !admission.contains(rip) {
                    return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                }
            }
        }
    }
    compile_lowered(code.code_end(), lowered, strategy)
}

fn compile_block_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: Option<&AdmittedInstructionBoundaries>,
    nop_admission: Option<&S0_09NopAdmission>,
    nop_inc_rax_admission: Option<&S0_09NopIncRaxAdmission>,
    flag_strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_block_inner_with_register_homes(
        code,
        entry_rip,
        admission,
        nop_admission,
        nop_inc_rax_admission,
        flag_strategy,
        REGISTER_HOME_BUDGET,
    )
}

fn compile_block_inner_with_register_homes(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: Option<&AdmittedInstructionBoundaries>,
    nop_admission: Option<&S0_09NopAdmission>,
    nop_inc_rax_admission: Option<&S0_09NopIncRaxAdmission>,
    flag_strategy: crate::block_emitter::FlagMaterializationStrategy,
    register_homes: u8,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let built = block::build_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    if let Some(admission) = admission {
        if let BuildOutcome::Block(block) = &built {
            for instruction in &block.instructions {
                for rip in [instruction.guest_rip, instruction.next_rip] {
                    if !admission.contains(rip) {
                        return Err(BlockCompileError::DispatchBlockBoundaryNotAdmitted { rip });
                    }
                }
            }
        }
    }
    let lowered_outcome = if let Some(admission) = nop_admission {
        lowering::lower_for_nop_fallback(&built, admission)
    } else if let Some(admission) = nop_inc_rax_admission {
        lowering::lower_for_nop_inc_rax_fallback(&built, admission)
    } else if let Some(admission) = admission {
        lowering::lower_for_dispatch(&built, admission)
    } else {
        lowering::lower(&built)
    }
    .map_err(BlockCompileError::Lowering)?;
    let lowered = match lowered_outcome {
        LoweringOutcome::NoNativePrefix {
            rip,
            reason,
            deferred_error,
        } => {
            return Ok(BlockCompileOutcome::NoNativePrefix {
                rip,
                reason,
                deferred_error,
            });
        }
        LoweringOutcome::Program(lowered) => lowered,
    };
    compile_lowered_with_register_homes(code.code_end(), lowered, flag_strategy, register_homes)
}

fn compile_lowered(
    code_end: u64,
    lowered: LoweredProgram,
    flag_strategy: crate::block_emitter::FlagMaterializationStrategy,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let mut observer = NoopObserver;
    compile_lowered_observed_with_register_homes(
        code_end,
        lowered,
        flag_strategy,
        REGISTER_HOME_BUDGET,
        &mut observer,
    )
}

fn compile_lowered_with_register_homes(
    code_end: u64,
    lowered: LoweredProgram,
    flag_strategy: crate::block_emitter::FlagMaterializationStrategy,
    register_homes: u8,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let mut observer = NoopObserver;
    compile_lowered_observed_with_register_homes(
        code_end,
        lowered,
        flag_strategy,
        register_homes,
        &mut observer,
    )
}

fn compile_lowered_observed<O: PhaseObserver>(
    code_end: u64,
    lowered: LoweredProgram,
    flag_strategy: crate::block_emitter::FlagMaterializationStrategy,
    observer: &mut O,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_lowered_observed_with_register_homes(
        code_end,
        lowered,
        flag_strategy,
        REGISTER_HOME_BUDGET,
        observer,
    )
}

fn compile_lowered_observed_with_register_homes<O: PhaseObserver>(
    code_end: u64,
    lowered: LoweredProgram,
    flag_strategy: crate::block_emitter::FlagMaterializationStrategy,
    register_homes: u8,
    observer: &mut O,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let frame_prefix = match flag_strategy {
        crate::block_emitter::FlagMaterializationStrategy::Eager => RUNTIME_FRAME_PREFIX_BYTES,
        crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate => {
            DEMAND_FLAGS_RUNTIME_FRAME_PREFIX_BYTES
        }
    };
    let allocation = measure(observer, DispatchPhase::Allocation, || {
        allocate_lowered_block(
            &lowered,
            AllocatorConfig {
                register_homes,
                reserved_prefix_bytes: frame_prefix,
            },
        )
    })?;
    let (emitted, actual) = measure(observer, DispatchPhase::EmissionAndFinalization, || {
        let emitted = match flag_strategy {
            crate::block_emitter::FlagMaterializationStrategy::Eager => {
                emit_runtime_block(&lowered.program, &allocation)
            }
            crate::block_emitter::FlagMaterializationStrategy::DemandDrivenCandidate => {
                crate::block_emitter::emit_runtime_block_demand_flags_candidate(
                    &lowered.program,
                    &allocation,
                )
            }
        }
        .map_err(BlockCompileError::Emitter)?;
        let actual = emitted
            .to_le_bytes()
            .map_err(|error| BlockCompileError::Emitter(BlockEmitterError::Encoding(error)))?
            .len();
        Ok::<_, BlockCompileError>((emitted, actual))
    })?;
    if actual > MAX_EMITTED_BLOCK_BYTES {
        return Err(BlockCompileError::EmittedCodeTooLarge {
            actual,
            max: MAX_EMITTED_BLOCK_BYTES,
        });
    }
    Ok(BlockCompileOutcome::Compiled(CompiledBlock {
        emitted,
        code_end,
        deferred_error: lowered.deferred_error,
    }))
}

fn allocate_block(
    lowered: &LoweredProgram,
    reserved_prefix_bytes: u32,
) -> Result<Allocation<'_>, BlockCompileError> {
    allocate_lowered_block(
        lowered,
        AllocatorConfig {
            register_homes: REGISTER_HOME_BUDGET,
            reserved_prefix_bytes,
        },
    )
}

/// Build the production helper-boundary plan and allocation for a lowered
/// block. Exposed so the opt-in allocator cost probe can report the same
/// mapping and counters consumed by emission.
pub fn allocate_lowered_block(
    lowered: &LoweredProgram,
    config: AllocatorConfig,
) -> Result<Allocation<'_>, BlockCompileError> {
    let helper_boundaries: Vec<_> = lowered
        .program
        .operations
        .iter()
        .enumerate()
        .filter_map(|(index, operation)| {
            matches!(
                operation,
                Operation::Integer { .. }
                    | Operation::CheckedLoad { .. }
                    | Operation::CheckedStore { .. }
            )
            .then_some(HelperBoundaryPosition::BeforeOperation(index))
        })
        .collect();
    let allocation = Allocation::build(&lowered.program, config, &helper_boundaries)
        .map_err(BlockCompileError::Allocation)?;
    Ok(allocation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BuildOutcome, CodeView};

    #[test]
    fn preserves_no_native_prefix_and_compiles_one_bounded_block() {
        let code = [0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0];
        let view = CodeView::new(0, &code).unwrap();
        let no_prefix = compile_block(view, code.len() as u64).unwrap();
        assert!(matches!(
            no_prefix,
            BlockCompileOutcome::NoNativePrefix {
                reason: BlockStopReason::BufferEnd,
                deferred_error: None,
                ..
            }
        ));

        let compiled = compile_block(view, 0).unwrap();
        let BlockCompileOutcome::Compiled(compiled) = compiled else {
            panic!("MOV prefix should compile");
        };
        assert_eq!(compiled.code_end(), code.len() as u64);
        assert!(compiled.deferred_error().is_none());
        assert!(compiled.emitted().pc_map().code_len() as usize <= MAX_EMITTED_BLOCK_BYTES);

        let unsupported = [0x90];
        let no_prefix = compile_block(CodeView::new(0, &unsupported).unwrap(), 0).unwrap();
        assert!(matches!(
            no_prefix,
            BlockCompileOutcome::NoNativePrefix {
                reason: BlockStopReason::UnsupportedDecoded { .. },
                deferred_error: None,
                ..
            }
        ));
    }

    #[test]
    fn s3_32a_exact_candidate_uses_the_normal_verified_compile_pipeline() {
        let bytes = [0x48, 0x03, 0xc0];
        let view = CodeView::new(0x3200, &bytes).unwrap();
        let admission = block::S3_32aAddRaxRaxAdmission::from_code(view).unwrap();
        let BlockCompileOutcome::Compiled(compiled) =
            compile_block_for_s3_32a_add_rax_rax_candidate(view, 0x3200, &admission).unwrap()
        else {
            panic!("exact ADD candidate should compile");
        };
        assert!(compiled.emitted().safe_entry_provenance());
        assert_eq!(compiled.emitted().entry_rip(), 0x3200);
        let pc_map = compiled.emitted().pc_map();
        assert!(pc_map.code_len() > 0);
        assert_eq!(pc_map.instruction_count(), 1);
        assert!(pc_map.ranges().iter().any(|range| {
            range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0x3200)
                && range.instruction_index == Some(0)
                && range.phase != crate::statepoints::StatePhase::Exit
        }));
        assert!(pc_map.boundaries().iter().any(|boundary| {
            boundary.rip == crate::statepoints::GuestRipBoundary::Fixed(0x3203)
                && boundary.instruction_index == 0
                && boundary.committed_instructions == 1
        }));

        let built = block::build_block(
            view,
            0x3200,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let LoweringOutcome::Program(lowered) =
            lowering::lower_for_s3_32a_add_rax_rax_candidate(&built, &admission).unwrap()
        else {
            panic!("exact ADD candidate should lower");
        };
        let allocation = allocate_lowered_block(
            &lowered,
            AllocatorConfig {
                register_homes: 4,
                reserved_prefix_bytes: RUNTIME_FRAME_PREFIX_BYTES,
            },
        )
        .unwrap();
        assert_eq!(allocation.program(), &lowered.program);
        assert_eq!(
            allocation.frame().reserved_prefix_bytes,
            RUNTIME_FRAME_PREFIX_BYTES
        );
        assert!(allocation.frame().frame_bytes >= RUNTIME_FRAME_PREFIX_BYTES);
        let location_counts = allocation.location_counts();
        assert!(location_counts.register_homes > 0);
        assert!(location_counts.register_homes <= 4);
        assert!(allocation.location(crate::ir::TempId(0)).is_ok());
        assert!(allocation.location(crate::ir::TempId(1)).is_ok());

        let other_bytes = [0x48, 0x03, 0xc1];
        let other = CodeView::new(0x3200, &other_bytes).unwrap();
        assert!(matches!(
            compile_block_for_s3_32a_add_rax_rax_candidate(other, 0x3200, &admission),
            Err(BlockCompileError::S3_32aAdmissionDoesNotMatchCode)
        ));
    }

    #[test]
    fn demand_flags_candidate_is_opt_in_uncached_and_marks_pending_boundaries() {
        use crate::statepoints::SnapshotAdmissibility;

        let code = [0x48, 0x03, 0xC1]; // ADD RAX, RCX
        let view = CodeView::new(0x8000, &code).unwrap();
        let eager = compile_block(view, 0x8000).unwrap();
        let candidate = compile_block_demand_flags_candidate(view, 0x8000).unwrap();
        let BlockCompileOutcome::Compiled(eager) = eager else {
            panic!("eager ADD should compile");
        };
        let BlockCompileOutcome::Compiled(candidate) = candidate else {
            panic!("demand-flags ADD should compile");
        };
        assert!(eager.emitted().safe_entry_provenance());
        assert!(candidate.emitted().safe_entry_provenance());
        let map = candidate.emitted().pc_map();
        assert!(
            map.ranges()
                .iter()
                .any(|range| { range.snapshot == SnapshotAdmissibility::InadmissiblePendingFlags })
        );
        assert_eq!(map.boundaries().len(), 1);
        assert_eq!(
            map.boundaries()[0].snapshot,
            SnapshotAdmissibility::InadmissiblePendingFlags
        );
        assert!(
            map.boundaries()
                .iter()
                .all(|boundary| boundary.snapshot != SnapshotAdmissibility::CanonicalBoundary)
        );

        let add_mov = [0x48, 0x03, 0xC1, 0x48, 0x89, 0xC3]; // ADD RAX, RCX; MOV RBX, RAX
        let BlockCompileOutcome::Compiled(add_mov) =
            compile_block_demand_flags_candidate(CodeView::new(0x8800, &add_mov).unwrap(), 0x8800)
                .unwrap()
        else {
            panic!("ADD/MOV candidate should compile");
        };
        let mov_ranges = add_mov
            .emitted()
            .pc_map()
            .ranges()
            .iter()
            .filter(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0x8803)
                    && range.phase != crate::statepoints::StatePhase::Exit
            })
            .collect::<Vec<_>>();
        assert!(
            !mov_ranges.is_empty(),
            "MOV should have native statepoint ranges"
        );
        assert!(
            mov_ranges
                .iter()
                .all(|range| { range.snapshot == SnapshotAdmissibility::InadmissiblePendingFlags }),
            "unexpected MOV ranges: {mov_ranges:#?}"
        );
        assert!(add_mov.emitted().pc_map().ranges().iter().any(|range| {
            range.phase == crate::statepoints::StatePhase::Service
                && range.snapshot == SnapshotAdmissibility::InadmissiblePendingFlags
        }));

        let deferred_tail = [0x48, 0x03, 0xC1, 0x48];
        let BlockCompileOutcome::Compiled(deferred) = compile_block_demand_flags_candidate(
            CodeView::new(0x9000, &deferred_tail).unwrap(),
            0x9000,
        )
        .unwrap() else {
            panic!("valid prefix before a truncated tail should compile");
        };
        assert!(deferred.deferred_error().is_some());
        assert!(
            deferred
                .emitted()
                .pc_map()
                .ranges()
                .iter()
                .any(|range| { range.snapshot == SnapshotAdmissibility::InadmissiblePendingFlags })
        );

        let nop_tail = [0x48, 0x03, 0xC1, 0x90];
        let nop_view = CodeView::new(0xA000, &nop_tail).unwrap();
        let nop_admission = S0_09NopAdmission::from_code(nop_view).unwrap();
        let BlockCompileOutcome::Compiled(nop_candidate) =
            compile_block_for_nop_fallback_demand_flags_candidate(nop_view, 0xA000, &nop_admission)
                .unwrap()
        else {
            panic!("ADD prefix before admitted NOP should compile");
        };
        assert!(
            nop_candidate
                .emitted()
                .pc_map()
                .ranges()
                .iter()
                .any(|range| {
                    range.phase == crate::statepoints::StatePhase::Exit
                        && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
                })
        );
    }

    #[test]
    fn native_inc_rax_is_opt_in_and_compiles_following_branch_consumers() {
        let code = [
            0x48, 0xff, 0xc0, // INC RAX
            0x74, 0x00, // JZ to code end, consuming INC's ZF
        ];
        let view = CodeView::new(0xB000, &code).unwrap();
        let admission = S0_09NopIncRaxAdmission::from_code(view).unwrap();
        let bridge = compile_block_for_nop_inc_rax_fallback(view, 0xB000, &admission).unwrap();
        assert!(matches!(
            bridge,
            BlockCompileOutcome::NoNativePrefix {
                reason: BlockStopReason::UnsupportedDecoded {
                    guest_rip: 0xB000,
                    ..
                },
                ..
            }
        ));

        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_inc_rax_demand_flags_candidate(view, 0xB000, &admission)
            } else {
                compile_block_for_native_inc_rax(view, 0xB000, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native INC/JZ should compile");
            };
            assert_eq!(compiled.code_end(), 0xB005);
            let ranges = compiled.emitted().pc_map().ranges();
            assert!(
                ranges
                    .iter()
                    .any(|range| range.guest_rip
                        == crate::statepoints::GuestRipSource::Fixed(0xB000))
            );
            assert!(
                ranges
                    .iter()
                    .any(|range| range.guest_rip
                        == crate::statepoints::GuestRipSource::Fixed(0xB003))
            );
            assert!(compiled.emitted().safe_entry_provenance());
        }
    }

    #[test]
    fn native_dec_rax_is_opt_in_and_compiles_following_branch_consumers() {
        let code = [
            0x48, 0xff, 0xc8, // DEC RAX
            0x74, 0x00, // JZ to code end, consuming DEC's ZF
        ];
        let view = CodeView::new(0xB100, &code).unwrap();
        let admission = S0_09NopDecRaxAdmission::from_code(view).unwrap();
        let BlockCompileOutcome::Compiled(compiled) =
            compile_block_for_native_dec_rax_demand_flags_candidate(view, 0xB100, &admission)
                .unwrap()
        else {
            panic!("native DEC/JZ should compile");
        };
        assert_eq!(compiled.code_end(), 0xB105);
        let ranges = compiled.emitted().pc_map().ranges();
        assert!(
            ranges.iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB100)
            })
        );
        assert!(
            ranges.iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB103)
            })
        );
        assert!(compiled.emitted().safe_entry_provenance());
    }

    #[test]
    fn native_neg_rax_is_opt_in_and_compiles_following_branch_consumers() {
        let code = [0x48, 0xf7, 0xd8, 0x74, 0x00]; // NEG RAX; JZ to code end
        let view = CodeView::new(0xB200, &code).unwrap();
        let admission = S0_09NopNegRaxAdmission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_neg_rax_demand_flags_candidate(view, 0xB200, &admission)
            } else {
                compile_block_for_native_neg_rax(view, 0xB200, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native NEG/JZ should compile")
            };
            assert_eq!(compiled.code_end(), 0xB205);
            let ranges = compiled.emitted().pc_map().ranges();
            assert!(
                ranges
                    .iter()
                    .any(|r| r.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB200))
            );
            assert!(
                ranges
                    .iter()
                    .any(|r| r.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB203))
            );
            assert!(compiled.emitted().safe_entry_provenance());
        }
    }

    #[test]
    fn ten_mov_block_remains_inside_the_unchanged_code_size_limit() {
        let mut source = Vec::new();
        for immediate in 0..10_u64 {
            source.extend_from_slice(&[0x48, 0xb8]);
            source.extend_from_slice(&immediate.to_le_bytes());
        }
        let compiled = compile_block(CodeView::new(0, &source).unwrap(), 0).unwrap();
        let BlockCompileOutcome::Compiled(compiled) = compiled else {
            panic!("ten exact MOV instructions must compile within the existing cap");
        };
        assert_eq!(compiled.emitted().pc_map().instruction_count(), 10);
        let code_len = compiled.emitted().pc_map().code_len() as usize;
        assert_eq!(code_len, 3868);
        assert!(code_len <= MAX_EMITTED_BLOCK_BYTES);
        assert_eq!(MAX_EMITTED_BLOCK_BYTES, 4096);
    }

    #[test]
    fn dispatch_compile_requires_fixture_boundaries_and_legacy_path_stays_one_block() {
        let forward = [0x74, 0x02, 0x74, 0x00, 0x74, 0x00];
        let view = CodeView::new(0, &forward).unwrap();
        let decoded = od_isa::decode(&forward).unwrap();
        let admission =
            block::AdmittedInstructionBoundaries::from_fixture_decode(view, &decoded).unwrap();
        assert!(matches!(
            compile_block_for_dispatch(view, 0, &admission).unwrap(),
            BlockCompileOutcome::Compiled(_)
        ));

        let changed_bytes = [0x75, 0x02, 0x74, 0x00, 0x74, 0x00];
        assert_eq!(
            compile_block_for_dispatch(CodeView::new(0, &changed_bytes).unwrap(), 0, &admission),
            Err(BlockCompileError::DispatchAdmissionDoesNotMatchCode)
        );

        let mut invalid_target = vec![0x75, 0x02, 0x48, 0xB8];
        invalid_target.extend_from_slice(&0_u64.to_le_bytes()); // JNZ targets inside this MOV.
        let substitute_bytes = [
            0x74, 0x00, 0x74, 0x00, 0x74, 0x00, 0x74, 0x00, 0x74, 0x00, 0x74, 0x00,
        ];
        assert_eq!(invalid_target.len(), substitute_bytes.len());
        let substitute_decode = od_isa::decode(&substitute_bytes).unwrap();
        assert!(matches!(
            block::AdmittedInstructionBoundaries::from_fixture_decode(
                CodeView::new(0, &invalid_target).unwrap(),
                &substitute_decode,
            ),
            Err(block::AdmissionError::FixtureDecodeRejected(_))
        ));

        // A rel8 target into its own instruction is rejected by fixture-v1
        // admission and by the bounded one-block builder; it is never promoted
        // into a dispatch candidate from numeric buffer membership.
        let mid_instruction = [0x74, 0xFF];
        assert!(od_isa::decode(&mid_instruction).is_err());
        assert!(matches!(
            compile_block(CodeView::new(0, &mid_instruction).unwrap(), 0).unwrap(),
            BlockCompileOutcome::NoNativePrefix {
                reason: BlockStopReason::UnsupportedBranchTarget { .. },
                ..
            }
        ));

        // The old compile_block API still does not admit the forward target
        // without whole-fixture boundary evidence.
        assert!(matches!(
            compile_block(view, 0).unwrap(),
            BlockCompileOutcome::NoNativePrefix {
                reason: BlockStopReason::Branch { .. },
                ..
            }
        ));

        let overlapping_fixture = [0x48, 0xB8, 0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0x74, 0x00];
        let overlapping_view = CodeView::new(0, &overlapping_fixture).unwrap();
        let full_decode = od_isa::decode(&overlapping_fixture).unwrap();
        let full_admission = block::AdmittedInstructionBoundaries::from_fixture_decode(
            overlapping_view,
            &full_decode,
        )
        .unwrap();
        assert_eq!(
            compile_block_for_dispatch(overlapping_view, 2, &full_admission),
            Err(BlockCompileError::DispatchEntryNotAdmitted { entry_rip: 2 })
        );
        // The generic block builder still treats an explicit in-view entry as
        // a fresh boundary; only fixture-dispatch compilation enforces the
        // whole-fixture admission boundary.
        assert!(matches!(
            compile_block(overlapping_view, 2).unwrap(),
            BlockCompileOutcome::Compiled(_)
        ));
    }

    #[test]
    fn decoded_tail_errors_remain_deferred_after_a_native_prefix() {
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&1_u64.to_le_bytes());
        code.extend_from_slice(&[0x48, 0x03]);
        let view = CodeView::new(0, &code).unwrap();
        let built =
            block::build_block(view, 0, AccessContext::default(), BlockLimits::default()).unwrap();
        assert!(matches!(built, BuildOutcome::Block(_)));
        let result = compile_block(view, 0).unwrap();
        let BlockCompileOutcome::Compiled(compiled) = result else {
            panic!("valid MOV prefix should be retained");
        };
        assert!(compiled.deferred_error().is_some());
    }

    #[test]
    fn multi_helper_alias_fixture_uses_production_spills_and_live_through_captures() {
        let code = [
            0x48, 0x03, 0xC0, // ADD RAX, RAX
            0x48, 0x2B, 0xD2, // SUB RDX, RDX
            0x48, 0x03, 0xC3, // ADD RAX, RBX
            0x48, 0x2B, 0xC9, // SUB RCX, RCX
            0x48, 0x3B, 0xC3, // CMP RAX, RBX
            0x74, 0x00, // JZ code_end
        ];
        let view = CodeView::new(0, &code).unwrap();
        let built =
            block::build_block(view, 0, AccessContext::default(), BlockLimits::default()).unwrap();
        if !matches!(built, BuildOutcome::Block(_)) {
            panic!("the aliased helper sequence should form one block");
        }
        let lowered = match lowering::lower(&built).unwrap() {
            LoweringOutcome::Program(lowered) => lowered,
            LoweringOutcome::NoNativePrefix { .. } => panic!("fixture must have native prefix"),
        };
        let allocation = allocate_block(&lowered, RUNTIME_FRAME_PREFIX_BYTES).unwrap();
        assert!(allocation.counters().value_spill_slots > 0);
        assert!(allocation.counters().helper_capture_slots > 0);
        assert!(allocation.counters().helper_saves > 0);

        let compiled = compile_block(view, 0).unwrap();
        assert!(matches!(compiled, BlockCompileOutcome::Compiled(_)));
    }

    #[test]
    fn rbp_memory_data_spill_stays_after_eight_gpr_runtime_prefix() {
        let source = [0x48, 0x8b, 0x28]; // MOV RBP,[RAX]
        let code = CodeView::new(0x5a00, &source).unwrap();
        let built = block::build_checked_qword_load_block(
            code,
            0x5a00,
            AccessContext::default(),
            crate::block::BlockLimits::default(),
        )
        .unwrap();
        assert!(matches!(built, BuildOutcome::Block(_)), "built={built:?}");
        let LoweringOutcome::Program(lowered) = lowering::lower(&built).unwrap() else {
            panic!("RBP memory-data singleton should lower");
        };
        let loaded = lowered
            .program
            .operations
            .iter()
            .find_map(|operation| match operation {
                Operation::CheckedLoad { dst, .. } => Some(*dst),
                _ => None,
            })
            .unwrap();
        let helper_boundaries = lowered
            .program
            .operations
            .iter()
            .enumerate()
            .filter_map(|(index, operation)| {
                matches!(operation, Operation::CheckedLoad { .. })
                    .then_some(HelperBoundaryPosition::BeforeOperation(index))
            })
            .collect::<Vec<_>>();
        let allocation = Allocation::build(
            &lowered.program,
            AllocatorConfig {
                register_homes: 0,
                reserved_prefix_bytes: 176,
            },
            &helper_boundaries,
        )
        .unwrap();
        let crate::allocator::ValueLocation::Spill(slot) = allocation.location(loaded).unwrap()
        else {
            panic!("zero register homes must spill the checked-load value");
        };
        assert!(slot.byte_offset() >= 176);
        assert_eq!(allocation.frame().reserved_prefix_bytes, 176);
        assert_eq!(allocation.frame().frame_bytes % 16, 0);
        let emitted = emit_checked_load_runtime_block_v2(&lowered.program, &allocation).unwrap();
        assert_eq!(emitted.entry_rip(), 0x5a00);
        assert_eq!(emitted.pc_map().instruction_count(), 1);
        assert!(!emitted.code().words().is_empty());
    }

    #[test]
    fn nop_fallback_compiler_retains_only_native_prefix_and_legacy_stays_unsupported() {
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&7_u64.to_le_bytes());
        code.push(0x90);
        code.extend_from_slice(&[0x48, 0x03, 0xC1]);
        let view = CodeView::new(0x6000, &code).unwrap();
        let admission = block::S0_09NopAdmission::from_code(view).unwrap();

        let compiled = compile_block_for_nop_fallback(view, 0x6000, &admission).unwrap();
        let BlockCompileOutcome::Compiled(compiled) = compiled else {
            panic!("the supported MOV prefix should compile before NOP");
        };
        assert_eq!(compiled.emitted().pc_map().instruction_count(), 1);

        let nop_only = [0x90];
        let nop_view = CodeView::new(0x7000, &nop_only).unwrap();
        let nop_admission = block::S0_09NopAdmission::from_code(nop_view).unwrap();
        assert!(matches!(
            compile_block_for_nop_fallback(nop_view, 0x7000, &nop_admission).unwrap(),
            BlockCompileOutcome::NoNativePrefix {
                reason: BlockStopReason::UnsupportedDecoded {
                    guest_rip: 0x7000,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            compile_block(nop_view, 0x7000).unwrap(),
            BlockCompileOutcome::NoNativePrefix {
                reason: BlockStopReason::UnsupportedDecoded {
                    guest_rip: 0x7000,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn native_not_rax_is_opt_in_and_emits_no_flag_helper_for_not() {
        let code = [0x48, 0xf7, 0xd0];
        let view = CodeView::new(0xB280, &code).unwrap();
        let admission = S0_09NopNotRaxAdmission::from_code(view).unwrap();
        for candidate in [false, true] {
            let compiled = if candidate {
                compile_block_for_native_not_rax_demand_flags_candidate(view, 0xB280, &admission)
            } else {
                compile_block_for_native_not_rax(view, 0xB280, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(block) = compiled else {
                panic!("exact NOT RAX should compile: {compiled:?}");
            };
            assert_eq!(block.emitted().pc_map().instruction_count(), 1);
            assert_eq!(
                block
                    .emitted()
                    .code()
                    .words()
                    .iter()
                    .filter(|word| **word == 0xD63F_0200)
                    .count(),
                0,
                "NOT RAX must not call the arithmetic flags helper"
            );
            assert!(block.emitted().pc_map().ranges().iter().any(|range| {
                range.instruction_index == Some(0)
                    && range.phase == crate::statepoints::StatePhase::PendingEffects
                    && range.snapshot
                        == crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction
            }));
            assert!(!block.emitted().pc_map().ranges().iter().any(|range| {
                range.instruction_index == Some(0)
                    && range.phase == crate::statepoints::StatePhase::AfterHelper
            }));
        }
    }

    #[test]
    fn native_lea_rax_rbx_compiles_eager_and_candidate_without_memory_helper() {
        let code = [
            0x48, 0x8d, 0x03, // LEA RAX,[RBX]
            0x74, 0x00, // JZ to code end, consuming unchanged flags
        ];
        let view = CodeView::new(0xB2A0, &code).unwrap();
        let admission = S0_09NopLeaRaxRbxAdmission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_lea_rax_rbx_demand_flags_candidate(
                    view, 0xB2A0, &admission,
                )
            } else {
                compile_block_for_native_lea_rax_rbx(view, 0xB2A0, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native LEA/JZ should compile: {outcome:?}");
            };
            assert_eq!(compiled.code_end(), 0xB2A5);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 2);
            assert!(compiled.emitted().safe_entry_provenance());
            assert_eq!(
                compiled
                    .emitted()
                    .code()
                    .words()
                    .iter()
                    .filter(|word| **word == 0xD63F_0200)
                    .count(),
                0,
                "LEA RAX,[RBX] must not call a helper"
            );
            for instruction_rip in [0xB2A0, 0xB2A3] {
                assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                    range.guest_rip == crate::statepoints::GuestRipSource::Fixed(instruction_rip)
                }));
            }
        }
    }

    #[test]
    fn native_lea_rax_rbx_rcx_compiles_eager_and_candidate_without_memory_helper() {
        let code = [
            0x48, 0x8d, 0x04, 0x0b, // LEA RAX,[RBX+RCX]
            0x74, 0x00, // JZ to code end, consuming unchanged flags
        ];
        let view = CodeView::new(0xB2A0, &code).unwrap();
        let admission = S0_09NopLeaRaxRbxRcxAdmission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_lea_rax_rbx_rcx_demand_flags_candidate(
                    view, 0xB2A0, &admission,
                )
            } else {
                compile_block_for_native_lea_rax_rbx_rcx(view, 0xB2A0, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native LEA/JZ should compile: {outcome:?}");
            };
            assert_eq!(compiled.code_end(), 0xB2A6);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 2);
            assert!(compiled.emitted().safe_entry_provenance());
            assert_eq!(
                compiled
                    .emitted()
                    .code()
                    .words()
                    .iter()
                    .filter(|word| **word == 0xD63F_0200)
                    .count(),
                0,
                "LEA RAX,[RBX+RCX] must not call a helper"
            );
            for instruction_rip in [0xB2A0, 0xB2A4] {
                assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                    range.guest_rip == crate::statepoints::GuestRipSource::Fixed(instruction_rip)
                }));
            }
        }
    }

    #[test]
    fn native_lea_rax_rip_disp32_compiles_eager_and_candidate_from_guest_next_rip() {
        // Variable-length ADD establishes pending flags; LEA must use its own
        // next_rip (0xB2BA), not the block start or end.
        let code = [
            0x48, 0x03, 0xc1, // ADD RAX,RCX
            0x48, 0x8d, 0x05, 0xf9, 0xff, 0xff, 0xff, // LEA RAX,[RIP-7]
            0x74, 0x00, // JZ, consuming ADD flags
        ];
        let view = CodeView::new(0xB2B0, &code).unwrap();
        let admission = S0_09NopLeaRaxRipDisp32Admission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_lea_rax_rip_disp32_demand_flags_candidate(
                    view, 0xB2B0, &admission,
                )
            } else {
                compile_block_for_native_lea_rax_rip_disp32(view, 0xB2B0, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native ADD/LEA/JZ should compile: {outcome:?}");
            };
            assert_eq!(compiled.code_end(), 0xB2BC);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 3);
            assert!(compiled.emitted().safe_entry_provenance());
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB2B3)
            }));
        }
    }

    #[test]
    fn native_lea_rax_rbx_disp8_compiles_eager_and_candidate_without_memory_helper() {
        let code = [
            0x48, 0x03, 0xc1, // ADD RAX,RCX; pending flags
            0x48, 0x8d, 0x43, 0xff, // LEA RAX,[RBX-1]
            0x75, 0x00, // JNZ to code end, consuming the ADD flags
        ];
        let view = CodeView::new(0xB2B0, &code).unwrap();
        let admission = S0_09NopLeaRaxRbxDisp8Admission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_lea_rax_rbx_disp8_demand_flags_candidate(
                    view, 0xB2B0, &admission,
                )
            } else {
                compile_block_for_native_lea_rax_rbx_disp8(view, 0xB2B0, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native ADD/LEA/JNZ should compile: {outcome:?}");
            };
            assert_eq!(compiled.code_end(), 0xB2B9);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 3);
            assert!(compiled.emitted().safe_entry_provenance());
            for instruction_rip in [0xB2B0, 0xB2B3, 0xB2B7] {
                assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                    range.guest_rip == crate::statepoints::GuestRipSource::Fixed(instruction_rip)
                }));
            }
        }
        let simple_code = [
            0x48, 0x8d, 0x43, 0xff, // LEA RAX,[RBX-1]
            0x75, 0x00, // JNZ to code end, consuming unchanged flags
        ];
        let simple_view = CodeView::new(0xB2D0, &simple_code).unwrap();
        let simple_admission = S0_09NopLeaRaxRbxDisp8Admission::from_code(simple_view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_lea_rax_rbx_disp8_demand_flags_candidate(
                    simple_view,
                    0xB2D0,
                    &simple_admission,
                )
            } else {
                compile_block_for_native_lea_rax_rbx_disp8(simple_view, 0xB2D0, &simple_admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("bare disp8 LEA/JNZ should compile: {outcome:?}");
            };
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 2);
            assert_eq!(
                compiled
                    .emitted()
                    .code()
                    .words()
                    .iter()
                    .filter(|word| **word == 0xD63F_0200)
                    .count(),
                0,
                "bare disp8 LEA must not call a helper"
            );
        }
    }

    #[test]
    fn native_xchg_rax_rbx_compiles_with_atomic_pending_statepoints() {
        let code = [
            0x48, 0x3b, 0xca, // CMP RCX,RDX; candidate flags pending
            0x48, 0x87, 0xd8, // XCHG RAX,RBX
            0x74, 0x00, // JZ to code end, consuming the prior flags
        ];
        let view = CodeView::new(0xB600, &code).unwrap();
        let admission = S0_09NopXchgRaxRbxAdmission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_xchg_rax_rbx_demand_flags_candidate(
                    view, 0xB600, &admission,
                )
            } else {
                compile_block_for_native_xchg_rax_rbx(view, 0xB600, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native XCHG/JZ should compile: {outcome:?}");
            };
            assert_eq!(compiled.code_end(), 0xB608);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 3);
            let xchg_ranges: Vec<_> = compiled
                .emitted()
                .pc_map()
                .ranges()
                .iter()
                .filter(|range| {
                    range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB603)
                        && range.phase == crate::statepoints::StatePhase::PendingEffects
                })
                .collect();
            assert_eq!(xchg_ranges.len(), if candidate { 8 } else { 4 });
            let expected_snapshot = if candidate {
                crate::statepoints::SnapshotAdmissibility::InadmissiblePendingFlags
            } else {
                crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction
            };
            assert!(
                xchg_ranges
                    .iter()
                    .all(|range| range.snapshot == expected_snapshot)
            );
        }
    }

    #[test]
    fn native_add_rax_imm8_compiles_eager_and_candidate_branch_consumers() {
        let code = [
            0x48, 0x83, 0xc0, 0xff, // ADD RAX, -1
            0x74, 0x00, // JZ to code end, consuming ADD's ZF
        ];
        let view = CodeView::new(0xB300, &code).unwrap();
        let admission = S0_09NopAddRaxImm8Admission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_add_rax_imm8_demand_flags_candidate(
                    view, 0xB300, &admission,
                )
            } else {
                compile_block_for_native_add_rax_imm8(view, 0xB300, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native ADD-immediate/JZ should compile");
            };
            assert_eq!(compiled.code_end(), 0xB306);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 2);
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB300)
            }));
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB304)
            }));
            assert!(compiled.emitted().safe_entry_provenance());
        }
    }

    #[test]
    fn native_mov_rax_imm32_compiles_eager_and_candidate_without_flag_helpers() {
        let code = [
            0x48, 0xC7, 0xC0, 0x21, 0x43, 0x65, 0x87, // MOV RAX, sign-extended 0x87654321
            0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // MOV RAX, 1
            0x75, 0x00, // JNZ to code end, consuming incoming flags
        ];
        let view = CodeView::new(0xB700, &code).unwrap();
        let admission = S0_09NopMovRaxImm32Admission::from_code(view).unwrap();
        assert_eq!(
            admission.mov_rax_imm32_value(0xB700),
            Some(0xFFFF_FFFF_8765_4321)
        );
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_mov_rax_imm32_demand_flags_candidate(
                    view, 0xB700, &admission,
                )
            } else {
                compile_block_for_native_mov_rax_imm32(view, 0xB700, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native MOV-immediate/JNZ should compile");
            };
            assert_eq!(compiled.code_end(), 0xB710);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 3);
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB700)
            }));
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB707)
            }));
            assert!(compiled.emitted().safe_entry_provenance());
        }

        let mov_then_nop = [0x48, 0xC7, 0xC0, 1, 0, 0, 0, 0x90];
        let fallback_view = CodeView::new(0xB800, &mov_then_nop).unwrap();
        let fallback_admission = S0_09NopMovRaxImm32Admission::from_code(fallback_view).unwrap();
        let fallback =
            compile_block_for_native_mov_rax_imm32(fallback_view, 0xB800, &fallback_admission)
                .unwrap();
        let BlockCompileOutcome::Compiled(fallback) = fallback else {
            panic!("an admitted MOV prefix should compile before NOP fallback");
        };
        assert_eq!(fallback.emitted().pc_map().instruction_count(), 1);
        let built = block::build_block_for_native_mov_rax_imm32(
            fallback_view,
            0xB800,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let LoweringOutcome::Program(lowered) =
            lowering::lower_for_native_mov_rax_imm32(&built, &fallback_admission).unwrap()
        else {
            panic!("MOV prefix should lower before the NOP fallback");
        };
        assert!(matches!(
            lowered.program.operations.last(),
            Some(Operation::Exit {
                kind: crate::ir::ExitKind::NeedInterpreter,
                target_rip: 0xB807,
            })
        ));
        assert!(matches!(
            compile_block(fallback_view, 0xB800).unwrap(),
            BlockCompileOutcome::NoNativePrefix { .. }
        ));
    }

    #[test]
    fn native_cmp_rax_imm8_compiles_eager_and_candidate_branch_consumers() {
        let code = [
            0x48, 0x83, 0xf8, 0xff, // CMP RAX, -1
            0x74, 0x00, // JZ to code end, consuming CMP flags
        ];
        let view = CodeView::new(0xB400, &code).unwrap();
        let admission = S0_09NopCmpRaxImm8Admission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_cmp_rax_imm8_demand_flags_candidate(
                    view, 0xB400, &admission,
                )
            } else {
                compile_block_for_native_cmp_rax_imm8(view, 0xB400, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native CMP-immediate/JZ should compile");
            };
            assert_eq!(compiled.code_end(), 0xB406);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 2);
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB400)
            }));
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB404)
            }));
            assert!(compiled.emitted().safe_entry_provenance());
        }
    }

    #[test]
    fn native_sub_rax_imm8_compiles_eager_and_candidate_branch_consumers() {
        let code = [
            0x48, 0x83, 0xe8, 0xff, // SUB RAX, -1
            0x74, 0x00, // JZ to code end, consuming SUB's flags
        ];
        let view = CodeView::new(0xB500, &code).unwrap();
        let admission = S0_09NopSubRaxImm8Admission::from_code(view).unwrap();
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_sub_rax_imm8_demand_flags_candidate(
                    view, 0xB500, &admission,
                )
            } else {
                compile_block_for_native_sub_rax_imm8(view, 0xB500, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("native SUB-immediate/JZ should compile");
            };
            assert_eq!(compiled.code_end(), 0xB506);
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 2);
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB500)
            }));
            assert!(compiled.emitted().pc_map().ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0xB504)
            }));
            assert!(compiled.emitted().safe_entry_provenance());
        }
    }

    #[test]
    fn native_register_v1_mixed_compiler_preserves_statepoints_and_boundaries() {
        let code = [
            0x48, 0x83, 0xc0, 0x80, // ADD RAX, -128 (full flag writer)
            0x48, 0x87, 0xd8, // XCHG RAX, RBX (preserving op)
            0x48, 0x8d, 0x43, 0x80, // LEA RAX, [RBX-128]
            0x48, 0x8d, 0x05, 0x01, 0x00, 0x00, 0x00, // RIP-relative LEA
            0x74, 0x00, // branch consumes flags and targets code end
        ];
        let view = CodeView::new(0xC000, &code).unwrap();
        let admission = NativeRegisterV1Admission::from_code(view).unwrap();
        let mut changed = code;
        changed[3] ^= 1;
        assert!(matches!(
            compile_block_for_native_register_v1(
                CodeView::new(0xC000, &changed).unwrap(),
                0xC000,
                &admission,
            ),
            Err(BlockCompileError::NativeRegisterV1AdmissionDoesNotMatchCode)
        ));
        assert!(matches!(
            compile_block_for_native_register_v1(
                CodeView::new(0xC000, &code[..code.len() - 1]).unwrap(),
                0xC000,
                &admission,
            ),
            Err(BlockCompileError::NativeRegisterV1AdmissionDoesNotMatchCode)
        ));
        assert!(matches!(
            compile_block_for_native_register_v1(view, 0xC001, &admission),
            Err(BlockCompileError::NativeRegisterV1EntryNotAdmitted { entry_rip: 0xC001 })
        ));
        for candidate in [false, true] {
            let outcome = if candidate {
                compile_block_for_native_register_v1_demand_flags_candidate(
                    view, 0xC000, &admission,
                )
            } else {
                compile_block_for_native_register_v1(view, 0xC000, &admission)
            }
            .unwrap();
            let BlockCompileOutcome::Compiled(compiled) = outcome else {
                panic!("mixed NativeRegisterV1 block should compile");
            };
            assert_eq!(compiled.code_end(), view.code_end());
            assert_eq!(compiled.emitted().pc_map().instruction_count(), 5);
            let map = compiled.emitted().pc_map();
            let rip_lea = 0xC000 + 11;
            assert!(map.ranges().iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(rip_lea)
                    && range.phase != crate::statepoints::StatePhase::Exit
            }));
            assert_eq!(map.boundaries().len(), 5);
            assert!(map.boundaries().iter().any(|boundary| {
                matches!(boundary.rip,
                    crate::statepoints::GuestRipBoundary::BranchSelection { target_rip, fallthrough_rip }
                    if target_rip == view.code_end() && fallthrough_rip == view.code_end())
                    && boundary.snapshot == crate::statepoints::SnapshotAdmissibility::CanonicalBoundary
            }));
            assert!(compiled.emitted().safe_entry_provenance());
        }
    }
}
