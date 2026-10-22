//! Bounded instruction-aware reduction of valid CPU fixtures.

use crate::case::{Fixture, Program};
use crate::{FormatError, sha256_hex};
use od_isa::{DecodeError, Instruction, Operation, decode};
use std::collections::HashMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MinimizeResult {
    pub fixture: Fixture,
    pub mismatch_class: String,
    pub attempts: usize,
    pub original_instruction_count: usize,
    pub minimized_instruction_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MinimizeError {
    InvalidAttemptLimit,
    InvalidFixture(FormatError),
    InvalidProgram(DecodeError),
    NoInitialMismatch,
    FlakyInitialResult {
        first: Option<String>,
        second: Option<String>,
    },
}

/// Reduce a fixture while a caller-provided oracle keeps the original mismatch class.
///
/// The oracle is called twice before reduction. Candidates are always revalidated and
/// decoded, and attempts are bounded. This is a best-effort reducer, not a global
/// minimum proof.
pub fn minimize_fixture<F>(
    original: &Fixture,
    max_attempts: usize,
    mut oracle: F,
) -> Result<MinimizeResult, MinimizeError>
where
    F: FnMut(&Fixture) -> Option<String>,
{
    if max_attempts < 2 {
        return Err(MinimizeError::InvalidAttemptLimit);
    }
    original.validate().map_err(MinimizeError::InvalidFixture)?;
    let first = oracle(original);
    let second = oracle(original);
    if first != second {
        return Err(MinimizeError::FlakyInitialResult { first, second });
    }
    let Some(mismatch_class) = first else {
        return Err(MinimizeError::NoInitialMismatch);
    };
    let original_code = original
        .program
        .bytes()
        .map_err(MinimizeError::InvalidFixture)?;
    let original_instructions = decode(&original_code).map_err(MinimizeError::InvalidProgram)?;
    let mut current = original.clone();
    let mut attempts = 2;
    let mut current_instructions = original_instructions.len();

    while attempts < max_attempts && current_instructions > 1 {
        let code = current
            .program
            .bytes()
            .map_err(MinimizeError::InvalidFixture)?;
        let instructions = decode(&code).map_err(MinimizeError::InvalidProgram)?;
        let mut accepted = None;
        let mut chunk = (instructions.len() / 2).max(1);
        while chunk > 0 && accepted.is_none() && attempts < max_attempts {
            for start in (0..instructions.len()).step_by(chunk) {
                let end = (start + chunk).min(instructions.len());
                if end - start == instructions.len() {
                    continue;
                }
                let Some(candidate) =
                    remove_instruction_range(&current, &code, &instructions, start, end)
                else {
                    continue;
                };
                if candidate.validate().is_err() {
                    continue;
                }
                if stable_candidate(
                    &candidate,
                    &mismatch_class,
                    &mut oracle,
                    &mut attempts,
                    max_attempts,
                ) {
                    accepted = Some(candidate);
                    break;
                }
                if attempts >= max_attempts {
                    break;
                }
            }
            if accepted.is_none() {
                chunk /= 2;
            }
        }
        if let Some(candidate) = accepted {
            current = candidate;
            current_instructions = decode(
                &current
                    .program
                    .bytes()
                    .map_err(MinimizeError::InvalidFixture)?,
            )
            .map_err(MinimizeError::InvalidProgram)?
            .len();
        } else {
            break;
        }
    }

    if attempts < max_attempts {
        current = reduce_immediates_and_initial_values(
            &current,
            &mismatch_class,
            &mut oracle,
            &mut attempts,
            max_attempts,
        )?;
    }
    let minimized_instruction_count = decode(
        &current
            .program
            .bytes()
            .map_err(MinimizeError::InvalidFixture)?,
    )
    .map_err(MinimizeError::InvalidProgram)?
    .len();
    Ok(MinimizeResult {
        fixture: current,
        mismatch_class,
        attempts,
        original_instruction_count: original_instructions.len(),
        minimized_instruction_count,
    })
}

fn remove_instruction_range(
    fixture: &Fixture,
    code: &[u8],
    instructions: &[Instruction],
    remove_start: usize,
    remove_end: usize,
) -> Option<Fixture> {
    let kept: Vec<_> = instructions
        .iter()
        .enumerate()
        .filter(|(index, _)| *index < remove_start || *index >= remove_end)
        .map(|(_, instruction)| *instruction)
        .collect();
    if kept.is_empty() {
        return None;
    }
    let mut offsets = HashMap::new();
    let mut output = Vec::with_capacity(code.len());
    for instruction in &kept {
        offsets.insert(instruction.offset, output.len());
        output.extend_from_slice(&code[instruction.offset..instruction.next_offset()]);
    }
    let new_end = output.len();
    for instruction in &kept {
        let displacement = match instruction.operation {
            Operation::Jz { displacement } | Operation::Jnz { displacement } => displacement,
            _ => continue,
        };
        let old_target = (instruction.next_offset() as i64).checked_add(i64::from(displacement))?;
        if old_target < 0 || old_target as usize > code.len() {
            return None;
        }
        let mapped_target = if old_target as usize == code.len() {
            new_end
        } else {
            instructions
                .iter()
                .filter(|candidate| candidate.offset >= old_target as usize)
                .find_map(|candidate| offsets.get(&candidate.offset).copied())
                .unwrap_or(new_end)
        };
        let new_instruction_offset = offsets[&instruction.offset];
        let new_displacement =
            mapped_target as i64 - (new_instruction_offset + instruction.length) as i64;
        let new_displacement = i8::try_from(new_displacement).ok()?;
        output[new_instruction_offset + 1] = new_displacement as u8;
    }
    let old_entry = usize::try_from(fixture.initial_state.rip.get()).ok()?;
    let new_entry = instructions
        .iter()
        .filter(|instruction| instruction.offset >= old_entry)
        .find_map(|instruction| offsets.get(&instruction.offset).copied())?;
    let mut candidate = replace_program(fixture, output);
    candidate.initial_state.rip = crate::HexU64::new(new_entry as u64);
    Some(candidate)
}

fn reduce_immediates_and_initial_values<F>(
    fixture: &Fixture,
    mismatch_class: &str,
    oracle: &mut F,
    attempts: &mut usize,
    max_attempts: usize,
) -> Result<Fixture, MinimizeError>
where
    F: FnMut(&Fixture) -> Option<String>,
{
    let mut current = fixture.clone();
    let code = current
        .program
        .bytes()
        .map_err(MinimizeError::InvalidFixture)?;
    let instructions = decode(&code).map_err(MinimizeError::InvalidProgram)?;
    for instruction in instructions {
        let Operation::MovImm { value, .. } = instruction.operation else {
            continue;
        };
        for candidate_value in [0, 1, u64::MAX, 1 << 63, value & 0xffff_ffff] {
            if candidate_value == value || *attempts >= max_attempts {
                continue;
            }
            let mut candidate_code = current
                .program
                .bytes()
                .map_err(MinimizeError::InvalidFixture)?;
            let start = instruction.offset + 2;
            candidate_code[start..start + 8].copy_from_slice(&candidate_value.to_le_bytes());
            let candidate = replace_program(&current, candidate_code);
            if candidate.validate().is_err() {
                continue;
            }
            if stable_candidate(&candidate, mismatch_class, oracle, attempts, max_attempts) {
                current = candidate;
                break;
            }
        }
    }

    for register in 0..4 {
        let current_value = match register {
            0 => current.initial_state.rax.get(),
            1 => current.initial_state.rcx.get(),
            2 => current.initial_state.rdx.get(),
            _ => current.initial_state.rbx.get(),
        };
        for value in [0, 1, u64::MAX] {
            if value == current_value || *attempts >= max_attempts {
                continue;
            }
            let mut candidate = current.clone();
            match register {
                0 => candidate.initial_state.rax = crate::HexU64::new(value),
                1 => candidate.initial_state.rcx = crate::HexU64::new(value),
                2 => candidate.initial_state.rdx = crate::HexU64::new(value),
                _ => candidate.initial_state.rbx = crate::HexU64::new(value),
            }
            if candidate.validate().is_err() {
                continue;
            }
            if stable_candidate(&candidate, mismatch_class, oracle, attempts, max_attempts) {
                current = candidate;
                break;
            }
        }
    }
    Ok(current)
}

fn stable_candidate<F>(
    candidate: &Fixture,
    mismatch_class: &str,
    oracle: &mut F,
    attempts: &mut usize,
    max_attempts: usize,
) -> bool
where
    F: FnMut(&Fixture) -> Option<String>,
{
    // Reserve both runs before trying a candidate; an unconfirmed final run
    // must never replace the last reproducible fixture.
    if max_attempts.saturating_sub(*attempts) < 2 {
        return false;
    }
    *attempts += 1;
    if oracle(candidate).as_deref() != Some(mismatch_class) {
        return false;
    }
    *attempts += 1;
    oracle(candidate).as_deref() == Some(mismatch_class)
}

fn replace_program(fixture: &Fixture, bytes: Vec<u8>) -> Fixture {
    let mut updated = fixture.clone();
    updated.program = Program {
        bytes_hex: encode_hex(&bytes),
        sha256: sha256_hex(&bytes),
    };
    updated
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{MinimizeError, minimize_fixture};
    use crate::HexU64;
    use crate::case::{ExpectedExit, Fixture};
    use crate::compare::compare_results;
    use crate::generate::generate_fixture;
    use crate::result::{
        Backend, BuildMetadata, CompletionStatus, CpuSnapshot, ExitReason, GprSnapshot,
        MachineMetadata, Observation, RESULT_SCHEMA_VERSION, RunResult,
    };
    use od_isa::{CpuState, interpret};
    use std::cell::Cell;

    #[test]
    fn injected_mismatch_is_reproduced_and_instruction_stream_is_reduced() {
        let fixture = generate_fixture(0xabc, 9);
        let result = minimize_fixture(&fixture, 64, injected_snapshot_mismatch).unwrap();
        assert!(result.mismatch_class.contains("rax"));
        assert!(result.minimized_instruction_count < result.original_instruction_count);
        assert!(result.fixture.validate().is_ok());
    }

    fn injected_snapshot_mismatch(fixture: &Fixture) -> Option<String> {
        let bytes = fixture.to_json().ok()?;
        let loaded = Fixture::from_json(&bytes).ok()?;
        let code = fixture.program.bytes().ok()?;
        let initial = &fixture.initial_state;
        let mut state = CpuState {
            gpr: [
                initial.rax.get(),
                initial.rcx.get(),
                initial.rdx.get(),
                initial.rbx.get(),
            ],
            rip: initial.rip.get() as usize,
            rflags: initial.rflags.get(),
        };
        let execution = interpret(&code, &mut state, fixture.instruction_budget).ok()?;
        let expected_exit = match execution.reason {
            od_isa::ExitReason::ReachedCodeEnd => ExpectedExit::ReachedCodeEnd,
            od_isa::ExitReason::InstructionBudgetExhausted => {
                ExpectedExit::InstructionBudgetExhausted
            }
            od_isa::ExitReason::StopRequested => return None,
        };
        let baseline = RunResult {
            schema_version: RESULT_SCHEMA_VERSION,
            case_id: fixture.case_id.clone(),
            input_sha256: loaded.input_sha256.clone(),
            profile: fixture.profile,
            backend: Backend::Interpreter,
            status: CompletionStatus::Completed,
            expected_exit,
            exit_reason: Observation::Available {
                value: match execution.reason {
                    od_isa::ExitReason::ReachedCodeEnd => ExitReason::ReachedCodeEnd,
                    od_isa::ExitReason::InstructionBudgetExhausted => {
                        ExitReason::InstructionBudgetExhausted
                    }
                    od_isa::ExitReason::StopRequested => return None,
                },
            },
            instructions_executed: Observation::Available {
                value: HexU64::new(execution.instructions_executed),
            },
            snapshot: CpuSnapshot {
                gpr: Observation::Available {
                    value: GprSnapshot {
                        rax: HexU64::new(execution.state.gpr[0]),
                        rcx: HexU64::new(execution.state.gpr[1]),
                        rdx: HexU64::new(execution.state.gpr[2]),
                        rbx: HexU64::new(execution.state.gpr[3]),
                    },
                },
                rip: Observation::Available {
                    value: HexU64::new(execution.state.rip as u64),
                },
                rflags: Observation::Available {
                    value: HexU64::new(execution.state.rflags),
                },
                memory: Observation::Unavailable {
                    reason: "register-only injected test".to_string(),
                },
                exception: Observation::Unavailable {
                    reason: "register-only injected test".to_string(),
                },
            },
            build: BuildMetadata {
                revision: "test-only-injection".to_string(),
                target: "test-target".to_string(),
                profile: "test".to_string(),
                compiler: "test".to_string(),
            },
            machine: MachineMetadata {
                host_os: "test".to_string(),
                host_arch: "test".to_string(),
                cpu_model: None,
                kernel: None,
                page_size: None,
            },
        };
        let mut injected = baseline.clone();
        injected.backend = Backend::Jit;
        let Observation::Available { value } = &mut injected.snapshot.gpr else {
            return None;
        };
        value.rax = HexU64::new(value.rax.get() ^ 1);
        let report = compare_results(&loaded, &baseline, &injected).ok()?;
        report
            .differences
            .first()
            .map(|difference| difference.field.clone())
    }

    #[test]
    fn rejects_a_flaky_initial_mismatch_before_reduction() {
        let fixture = generate_fixture(0xabc, 10);
        let calls = Cell::new(0);
        let result = minimize_fixture(&fixture, 20, |_| {
            let call = calls.get();
            calls.set(call + 1);
            if call == 0 {
                Some("injected:gpr.rax".to_string())
            } else {
                None
            }
        });
        assert!(matches!(
            result,
            Err(MinimizeError::FlakyInitialResult { .. })
        ));
    }

    #[test]
    fn does_not_accept_a_candidate_that_only_fails_once() {
        let fixture = generate_fixture(0xabc, 9);
        let mut seen = std::collections::HashSet::new();
        let result = minimize_fixture(&fixture, 64, |candidate| {
            if candidate == &fixture || seen.insert(candidate.to_json().unwrap()) {
                Some("injected:gpr.rax".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(result.fixture, fixture);
        assert!(result.attempts <= 64);
    }

    #[test]
    fn too_small_attempt_limit_never_calls_the_oracle() {
        let fixture = generate_fixture(0xabc, 9);
        for limit in [0, 1] {
            let mut calls = 0;
            assert!(
                minimize_fixture(&fixture, limit, |_| {
                    calls += 1;
                    Some("injected:gpr.rax".to_string())
                })
                .is_err()
            );
            assert_eq!(calls, 0);
        }
    }

    #[test]
    fn removing_instructions_before_entry_remaps_initial_rip() {
        let mut fixture = generate_fixture(0xabc, 9);
        let mut code = Vec::new();
        for value in [1u64, 2, 3] {
            code.extend_from_slice(&[0x48, 0xb8]);
            code.extend_from_slice(&value.to_le_bytes());
        }
        fixture = super::replace_program(&fixture, code.clone());
        fixture.initial_state.rip = HexU64::new(10);
        let instructions = od_isa::decode(&code).unwrap();
        let reduced =
            super::remove_instruction_range(&fixture, &code, &instructions, 0, 1).unwrap();
        assert_eq!(reduced.initial_state.rip, HexU64::new(0));
        assert_eq!(reduced.program.bytes().unwrap(), code[10..]);
        reduced.validate().unwrap();
    }
}
