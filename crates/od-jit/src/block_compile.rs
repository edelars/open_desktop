//! Compile one verified, bounded guest block into runtime-ready ARM64 code.

use od_isa::memory::AccessContext;

use crate::allocator::{Allocation, AllocatorConfig, AllocatorError, HelperBoundaryPosition};
use crate::block::{self, BlockLimits, BlockStopReason, BuildError, CodeView, DecodeBoundary};
use crate::block_emitter::{BlockEmitterError, EmittedRuntimeBlock, emit_runtime_block};
use crate::ir::Operation;
use crate::lowering::{self, LoweredProgram, LoweringError, LoweringOutcome};

pub const MAX_EMITTED_BLOCK_BYTES: usize = 4096;
const RUNTIME_FRAME_PREFIX_BYTES: u32 = 112;
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
}

/// Build and compile a single bounded block with the current runtime profile.
/// `NoNativePrefix` remains a distinct outcome; it is not converted into an
/// empty Program or a successful compiled block.
pub fn compile_block(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<BlockCompileOutcome, BlockCompileError> {
    let built = block::build_block(
        code,
        entry_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(BlockCompileError::Build)?;
    let lowered = match lowering::lower(&built).map_err(BlockCompileError::Lowering)? {
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
            register_homes: REGISTER_HOME_BUDGET,
            reserved_prefix_bytes: RUNTIME_FRAME_PREFIX_BYTES,
        },
        &helper_boundaries,
    )
    .map_err(BlockCompileError::Allocation)?;
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
}
