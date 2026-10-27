//! ARM64-only capture of real fixture dispatcher entry RIPs.
//!
//! Capture wraps the existing one-block executor and records `state.rip` before
//! each publish/execute/drop call. It never infers control flow from final
//! snapshots and refuses non-Apple-Silicon hosts at the CLI boundary.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use od_host::{PublishedBlockExecutionError, PublishedBlockExecutor};
use od_isa::execute::{RunConfig, RunExit, run as interpreter_run};
use od_isa::memory::{AccessContext, GuestMemory, MemoryError};
use od_isa::{LongModeState, Register};
use od_jit::block::{
    AdmittedInstructionBoundaries, BlockLimits, BuildOutcome, CodeView, build_block,
};
use od_jit::block_compile::CompiledBlock;
use od_jit::dispatch::{BlockExecutionReport, BlockExecutor, DispatchExit, run as dispatch_run};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity, CompileFlavor,
    MappingId, MappingVersion, TranslationKey,
};
use od_testkit::case::{ExpectedExit, Fixture};
use od_testkit::sha256_hex;

use crate::{
    BenchmarkError, HostMetadata, MAX_CODE_BYTES_PER_INPUT, MAX_INPUT_JSON_BYTES, MAX_INPUTS,
    MAX_TOTAL_TRACE_VISITS, MAX_VISITS_PER_INPUT, RecordedInput, RecordedTrace, RecordedVisit,
};

#[derive(Debug)]
pub enum RecordError {
    Io(String),
    Fixture(String),
    Limit(BenchmarkError),
    Decode(String),
    CompileOrExecute(String),
    InterpreterMismatch {
        case_id: String,
        detail: String,
    },
    ExpectedExitMismatch {
        case_id: String,
    },
    RetirementMismatch {
        case_id: String,
        interpreter: u64,
        jit: u64,
        expected_remaining: Option<u64>,
        jit_remaining: u64,
    },
    DuplicateCaseId(String),
    TimeLimitExceeded(Duration),
}

pub fn record_case_list(path: &Path) -> Result<RecordedTrace, RecordError> {
    let list = std::fs::read_to_string(path).map_err(|error| RecordError::Io(error.to_string()))?;
    let paths: Vec<_> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .collect();
    record_fixtures(&paths)
}

pub fn record_fixtures(paths: &[PathBuf]) -> Result<RecordedTrace, RecordError> {
    if paths.len() > MAX_INPUTS {
        return Err(RecordError::Limit(BenchmarkError::TooManyInputs {
            actual: paths.len(),
            max: MAX_INPUTS,
        }));
    }
    let started = Instant::now();
    let mut seen_ids = HashSet::new();
    let mut inputs = Vec::with_capacity(paths.len());
    let mut visits = Vec::new();
    let mut visit_counts = vec![0usize; paths.len()];
    let mut total_fixture_bytes = 0usize;

    for (input_index, path) in paths.iter().enumerate() {
        if started.elapsed() > crate::MAX_TRACE_TIME {
            return Err(RecordError::TimeLimitExceeded(started.elapsed()));
        }
        let raw = std::fs::read(path).map_err(|error| RecordError::Io(error.to_string()))?;
        if raw.len() > MAX_INPUT_JSON_BYTES {
            return Err(RecordError::Limit(BenchmarkError::InputJsonTooLarge {
                actual: raw.len(),
                max: MAX_INPUT_JSON_BYTES,
            }));
        }
        total_fixture_bytes = total_fixture_bytes
            .checked_add(raw.len())
            .ok_or(RecordError::Limit(BenchmarkError::CounterOverflow))?;
        let loaded = Fixture::from_json(&raw)
            .map_err(|error| RecordError::Fixture(format!("{}: {error:?}", path.display())))?;
        let fixture = loaded.fixture;
        if !seen_ids.insert(fixture.case_id.clone()) {
            return Err(RecordError::DuplicateCaseId(fixture.case_id));
        }
        let code_bytes = fixture
            .program
            .bytes()
            .map_err(|error| RecordError::Fixture(format!("{}: {error:?}", path.display())))?;
        if code_bytes.len() > MAX_CODE_BYTES_PER_INPUT {
            return Err(RecordError::Limit(BenchmarkError::CodeTooLarge {
                input_index,
                actual: code_bytes.len(),
                max: MAX_CODE_BYTES_PER_INPUT,
            }));
        }

        let initial_state = initial_state(&fixture);
        let code = CodeView::new(0, &code_bytes)
            .map_err(|error| RecordError::CompileOrExecute(format!("code view: {error:?}")))?;
        let fixture_decode = od_isa::decode(&code_bytes)
            .map_err(|error| RecordError::Decode(format!("{}: {error:?}", fixture.case_id)))?;
        let admission =
            AdmittedInstructionBoundaries::from_fixture_decode(code, &fixture_decode)
                .map_err(|error| RecordError::Decode(format!("{}: {error:?}", fixture.case_id)))?;

        let mut interpreter_state = initial_state.clone();
        let mut interpreter_memory = FetchMemory(&code_bytes);
        let interpreter_result = interpreter_run(
            &mut interpreter_state,
            &mut interpreter_memory,
            RunConfig {
                instruction_budget: fixture.instruction_budget,
                stop_rip: Some(code.code_end()),
                context: AccessContext::default(),
            },
            || false,
        );
        if started.elapsed() > crate::MAX_TRACE_TIME {
            return Err(RecordError::TimeLimitExceeded(started.elapsed()));
        }

        let identity_value = u64::try_from(input_index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or(RecordError::Limit(BenchmarkError::CounterOverflow))?;
        let identity = CodeIdentity {
            address_space: AddressSpaceId::new(std::num::NonZeroU64::new(1).unwrap()),
            mapping: MappingId::new(std::num::NonZeroU64::new(identity_value).unwrap()),
            mapping_version: MappingVersion::new(std::num::NonZeroU64::new(1).unwrap()),
            backing: BackingId::new(std::num::NonZeroU64::new(identity_value).unwrap()),
            backing_version: BackingVersion::new(std::num::NonZeroU64::new(1).unwrap()),
            executable: true,
        };
        let mut executor = RecordingExecutor {
            code,
            identity,
            input_index,
            total_visits_before: visits.len(),
            started,
            inner: PublishedBlockExecutor,
            visits: Vec::new(),
        };
        let mut jit_state = initial_state;
        let jit_result = dispatch_run(
            code,
            &admission,
            &mut jit_state,
            fixture.instruction_budget,
            &mut executor,
        )
        .map_err(|error| {
            RecordError::CompileOrExecute(format!("{}: {error:?}", fixture.case_id))
        })?;

        check_retirement_counts(
            &fixture.case_id,
            fixture.instruction_budget,
            interpreter_result.instructions_executed,
            jit_result.retired_instructions,
            jit_result.remaining_budget,
        )?;

        check_exit(
            &fixture.case_id,
            fixture.expected_exit,
            &interpreter_result.exit,
            jit_result.exit,
        )
        .map_err(|error| error)?;
        check_state(&fixture, &interpreter_state, &jit_state)?;
        visit_counts[input_index] = executor.visits.len();
        if visit_counts[input_index] > MAX_VISITS_PER_INPUT {
            return Err(RecordError::Limit(BenchmarkError::TooManyVisitsForInput {
                input_index,
                actual: visit_counts[input_index],
                max: MAX_VISITS_PER_INPUT,
            }));
        }
        if visits.len() + executor.visits.len() > MAX_TOTAL_TRACE_VISITS {
            return Err(RecordError::Limit(BenchmarkError::TooManyTraceVisits {
                actual: visits.len() + executor.visits.len(),
                max: MAX_TOTAL_TRACE_VISITS,
            }));
        }
        visits.extend(executor.visits);
        inputs.push(RecordedInput {
            case_id: fixture.case_id,
            fixture_sha256: loaded.input_sha256,
            fixture_json_bytes: raw.len(),
            code_hex: code_bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            program_sha256: sha256_hex(&code_bytes),
            base_rip: 0,
            address_space_id: 1,
            mapping_id: identity_value,
            mapping_version: 1,
            backing_id: identity_value,
            backing_version: 1,
        });
    }

    if total_fixture_bytes > crate::MAX_LOGICAL_BYTES {
        return Err(RecordError::Limit(BenchmarkError::LogicalMemoryTooLarge {
            actual: total_fixture_bytes,
            max: crate::MAX_LOGICAL_BYTES,
        }));
    }
    Ok(RecordedTrace {
        schema: 2,
        capture_host: HostMetadata::current(),
        trace_elapsed_ns: started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
        inputs,
        visits,
    })
}

struct RecordingExecutor<'a> {
    code: CodeView<'a>,
    identity: CodeIdentity,
    input_index: usize,
    total_visits_before: usize,
    started: Instant,
    inner: PublishedBlockExecutor,
    visits: Vec<RecordedVisit>,
}

impl BlockExecutor for RecordingExecutor<'_> {
    type Error = RecordExecutorError;

    fn execute_one_block(
        &mut self,
        block: &CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
    ) -> Result<BlockExecutionReport, Self::Error> {
        if self.started.elapsed() > crate::MAX_TRACE_TIME {
            return Err(RecordExecutorError::TimeLimitExceeded);
        }
        if self.visits.len() >= MAX_VISITS_PER_INPUT
            || self
                .total_visits_before
                .checked_add(self.visits.len())
                .is_none_or(|count| count >= MAX_TOTAL_TRACE_VISITS)
        {
            return Err(RecordExecutorError::VisitLimitExceeded);
        }
        let entry_rip = state.rip;
        if block.emitted().entry_rip() != entry_rip {
            return Err(RecordExecutorError::Source(format!(
                "compiled entry {:#x} differs from dispatcher RIP {entry_rip:#x}",
                block.emitted().entry_rip()
            )));
        }
        let BuildOutcome::Block(source) = build_block(
            self.code,
            entry_rip,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .map_err(|error| RecordExecutorError::Source(format!("{error:?}")))?
        else {
            return Err(RecordExecutorError::Source(
                "no block at dispatch entry".into(),
            ));
        };
        let dependency = CodeDependencySpan {
            start_rip: self.code.base_rip(),
            end_rip: self.code.code_end(),
            identity: self.identity,
        };
        TranslationKey::for_block(
            self.code,
            &source,
            CompileFlavor::S0_09FixtureDispatch,
            crate::benchmark_execution_context(),
            &[dependency],
        )
        .map_err(|error| RecordExecutorError::Source(format!("translation key: {error:?}")))?;
        self.visits.push(RecordedVisit {
            input_index: self.input_index,
            entry_rip,
        });
        self.inner
            .execute_one_block(block, state, incoming_budget)
            .map_err(RecordExecutorError::Host)
    }
}

#[derive(Debug)]
enum RecordExecutorError {
    Source(String),
    Host(PublishedBlockExecutionError),
    TimeLimitExceeded,
    VisitLimitExceeded,
}

impl std::fmt::Display for RecordExecutorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(message) => write!(formatter, "trace source validation failed: {message}"),
            Self::Host(error) => write!(formatter, "native block execution failed: {error:?}"),
            Self::TimeLimitExceeded => write!(formatter, "trace capture time limit exceeded"),
            Self::VisitLimitExceeded => write!(formatter, "trace block-visit limit exceeded"),
        }
    }
}

fn initial_state(fixture: &Fixture) -> LongModeState {
    let mut state = LongModeState::default();
    state.gpr[..4].copy_from_slice(&[
        fixture.initial_state.rax.get(),
        fixture.initial_state.rcx.get(),
        fixture.initial_state.rdx.get(),
        fixture.initial_state.rbx.get(),
    ]);
    state.rip = fixture.initial_state.rip.get();
    state.rflags = fixture.initial_state.rflags.get();
    state
}

fn check_exit(
    case_id: &str,
    expected: ExpectedExit,
    interpreter: &RunExit,
    jit: DispatchExit,
) -> Result<(), RecordError> {
    let expected_match = match expected {
        ExpectedExit::ReachedCodeEnd => {
            *interpreter == RunExit::StopAddress && jit == DispatchExit::CodeEnd
        }
        ExpectedExit::InstructionBudgetExhausted => {
            *interpreter == RunExit::InstructionBudgetExhausted
                && jit == DispatchExit::BudgetExhausted
        }
    };
    if expected_match {
        Ok(())
    } else {
        Err(RecordError::ExpectedExitMismatch {
            case_id: case_id.to_string(),
        })
    }
}

fn check_retirement_counts(
    case_id: &str,
    incoming_budget: u64,
    interpreter_retired: u64,
    jit_retired: u64,
    jit_remaining: u64,
) -> Result<(), RecordError> {
    let expected_remaining = incoming_budget.checked_sub(interpreter_retired);
    if interpreter_retired == jit_retired && expected_remaining == Some(jit_remaining) {
        Ok(())
    } else {
        Err(RecordError::RetirementMismatch {
            case_id: case_id.to_owned(),
            interpreter: interpreter_retired,
            jit: jit_retired,
            expected_remaining,
            jit_remaining,
        })
    }
}

fn check_state(
    fixture: &Fixture,
    interpreter: &LongModeState,
    jit: &LongModeState,
) -> Result<(), RecordError> {
    for register in &fixture.comparison.registers {
        let index = match register {
            od_testkit::case::Register::Rax => Register::Rax as usize,
            od_testkit::case::Register::Rcx => Register::Rcx as usize,
            od_testkit::case::Register::Rdx => Register::Rdx as usize,
            od_testkit::case::Register::Rbx => Register::Rbx as usize,
        };
        if interpreter.gpr[index] != jit.gpr[index] {
            return Err(RecordError::InterpreterMismatch {
                case_id: fixture.case_id.clone(),
                detail: format!("GPR {:?} differs", register),
            });
        }
    }
    if interpreter.rip != jit.rip {
        return Err(RecordError::InterpreterMismatch {
            case_id: fixture.case_id.clone(),
            detail: "RIP differs".into(),
        });
    }
    let flags_mask = fixture.comparison.rflags_mask.get();
    if interpreter.rflags & flags_mask != jit.rflags & flags_mask {
        return Err(RecordError::InterpreterMismatch {
            case_id: fixture.case_id.clone(),
            detail: "compared RFLAGS bits differ".into(),
        });
    }
    Ok(())
}

struct FetchMemory<'a>(&'a [u8]);

impl GuestMemory for FetchMemory<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        usize::try_from(address)
            .ok()
            .and_then(|offset| self.0.get(offset).copied())
            .ok_or_else(|| MemoryError::Unmapped {
                address,
                access: od_isa::memory::AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!(
            "benchmark fetch-only memory read at {address:#x}"
        )))
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!(
            "benchmark fetch-only memory write at {address:#x}"
        )))
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!(
            "benchmark fetch-only atomic at {address:#x}"
        )))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!(
            "benchmark fetch-only atomic at {address:#x}"
        )))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!(
            "benchmark fetch-only atomic at {address:#x}"
        )))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::check_retirement_counts;

    #[test]
    fn independent_interpreter_count_checks_dispatch_retirement_and_remaining() {
        assert!(check_retirement_counts("ok", 8, 5, 5, 3).is_ok());
        assert!(check_retirement_counts("underreported", 8, 5, 4, 4).is_err());
        assert!(check_retirement_counts("bad-budget", 8, 5, 5, 2).is_err());
        assert!(check_retirement_counts("over-budget", 4, 5, 5, 0).is_err());
    }
}
