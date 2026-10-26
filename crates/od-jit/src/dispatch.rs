//! Iterative multi-block dispatch over a fully admitted immutable fixture.
//!
//! The executor boundary runs exactly one compiled block per call. A host
//! implementation can publish, execute, and drop that block before this loop
//! compiles or invokes the next one.

use od_isa::LongModeState;

use crate::block::{AdmittedInstructionBoundaries, CodeView};
use crate::block_compile::{BlockCompileError, BlockCompileOutcome, compile_block_for_dispatch};
use crate::runtime::RuntimeExitTag;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockExecutionReport {
    pub exit: RuntimeExitTag,
    pub remaining_budget: u64,
    pub retired_instructions: u64,
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
}
