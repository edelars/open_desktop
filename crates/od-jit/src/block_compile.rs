//! Compile one verified, bounded guest block into runtime-ready ARM64 code.

use od_isa::memory::AccessContext;

use crate::allocator::{Allocation, AllocatorConfig, AllocatorError, HelperBoundaryPosition};
use crate::block::{
    self, AdmittedInstructionBoundaries, BlockLimits, BlockStopReason, BuildError, BuildOutcome,
    CodeView, DecodeBoundary, S0_09NopAdmission,
};
use crate::block_emitter::{
    BlockEmitterError, EmittedCheckedLoadBlockV2, EmittedCheckedStoreBlockV2, EmittedRuntimeBlock,
    emit_checked_load_runtime_block_v2, emit_checked_store_runtime_block_v2, emit_runtime_block,
    emit_two_memory_runtime_block_v2,
};
use crate::ir::Operation;
use crate::lowering::{self, LoweredProgram, LoweringError, LoweringOutcome};

pub const MAX_EMITTED_BLOCK_BYTES: usize = 4096;
const RUNTIME_FRAME_PREFIX_BYTES: u32 = 160;
const CHECKED_LOAD_FRAME_PREFIX_BYTES_V2: u32 = 160;
const CHECKED_STORE_FRAME_PREFIX_BYTES_V2: u32 = 160;
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
}

/// Build and compile a single bounded block with the current runtime profile.
/// `NoNativePrefix` remains a distinct outcome; it is not converted into an
/// empty Program or a successful compiled block.
pub fn compile_block(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    compile_block_inner(code, entry_rip, None, None)
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
    let lowered_outcome =
        lowering::lower_for_dispatch(&built, admission).map_err(BlockCompileError::Lowering)?;
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
    compile_lowered(code_end, lowered)
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
    )
}

fn compile_block_inner(
    code: CodeView<'_>,
    entry_rip: u64,
    admission: Option<&AdmittedInstructionBoundaries>,
    nop_admission: Option<&S0_09NopAdmission>,
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
    let lowered_outcome = match nop_admission {
        Some(admission) => lowering::lower_for_nop_fallback(&built, admission),
        None => match admission {
            Some(admission) => lowering::lower_for_dispatch(&built, admission),
            None => lowering::lower(&built),
        },
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
    compile_lowered(code.code_end(), lowered)
}

fn compile_lowered(
    code_end: u64,
    lowered: LoweredProgram,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let allocation = allocate_block(&lowered, RUNTIME_FRAME_PREFIX_BYTES)?;
    let emitted =
        emit_runtime_block(&lowered.program, &allocation).map_err(BlockCompileError::Emitter)?;
    let actual = emitted
        .to_le_bytes()
        .map_err(|error| BlockCompileError::Emitter(BlockEmitterError::Encoding(error)))?
        .len();
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
    let allocation = Allocation::build(
        &lowered.program,
        AllocatorConfig {
            register_homes: REGISTER_HOME_BUDGET,
            reserved_prefix_bytes,
        },
        &helper_boundaries,
    )
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
    fn rbp_memory_data_spill_stays_after_six_gpr_runtime_prefix() {
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
                reserved_prefix_bytes: 160,
            },
            &helper_boundaries,
        )
        .unwrap();
        let crate::allocator::ValueLocation::Spill(slot) = allocation.location(loaded).unwrap()
        else {
            panic!("zero register homes must spill the checked-load value");
        };
        assert!(slot.byte_offset() >= 160);
        assert_eq!(allocation.frame().reserved_prefix_bytes, 160);
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
}
