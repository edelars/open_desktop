//! ADR-0023 slice C2a-3 (plan `work/s7-32/c1/plan-c1-c2a.md`, C2a-3 item 1): the U/S and R/W
//! faults of the class `Neutral` at CPL3 on the one-level page rights of the sweep memory.
//!
//! Every `Neutral` encoding of the C2a-1 corpus whose first data access lands on the data page
//! (`cpl3_sweep::DATA`, where the GPRs of input A point) is run with that page made a supervisor
//! page and a read-only user page. The expectation comes from the same encoding at CPL0 (the
//! architectural rule is the same, only the U/S bit of the access differs), Vol. 3A §4.6 and
//! "Event 14": a user access to a supervisor page is `#PF` with P = 1 and U = 1 whatever the
//! instruction; a user write to a read-only page is `#PF` with P = 1, W = 1, U = 1 at CR0.WP = 0
//! and 1; a user read of a read-only page executes. A fault changes nothing.

mod cpl3_sweep;

use cpl3_sweep::*;
use od_isa::Cpl0ProfileSetExit;
use od_isa::cpl3_class::{Cpl3Class, cpl3_class};
use od_isa::execute::RunExit;

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/c2a_cpl3_legacy_corpus.txt"
);
/// CR0.WP.
const CR0_WP: u64 = 1 << 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DataPage {
    Absent,
    Supervisor,
    ReadOnly,
}

fn neutral_corpus() -> Vec<Vec<u8>> {
    std::fs::read_to_string(CORPUS)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| unhex(line.split_whitespace().next().unwrap()))
        .filter(|bytes| {
            decode(bytes)
                .is_ok_and(|instruction| cpl3_class(&instruction.operation) == Cpl3Class::Neutral)
        })
        .collect()
}

fn operation_name(bytes: &[u8]) -> String {
    format!("{:?}", decode(bytes).unwrap().operation)
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_owned()
}

/// One instruction of `bytes` at `cpl` on input A with the data page as `page` (CR0.WP as `wp`).
fn run(bytes: &[u8], cpl: u8, page: DataPage, wp: bool) -> (Run, od_isa::LongModeState) {
    let (mut state, mut memory) = input_machine(cpl, CR4, InputVariant::A);
    match page {
        DataPage::Absent => memory.absent(DATA),
        DataPage::Supervisor => memory.map(DATA, false, true),
        DataPage::ReadOnly => memory.map(DATA, true, false),
    }
    if !wp {
        state.cr0 = state.cr0.map(|cr0| cr0 & !CR0_WP);
        memory.write_protect_off = true;
    }
    let initial = state.clone();
    (run_on(state, memory, Cells::new(), bytes, 1), initial)
}

/// The `#PF` of a run: (error code, address).
fn page_fault(run: &Run) -> Option<(u32, u64)> {
    match &run.result {
        Ok(result) => match &result.exit {
            Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) if fault.vector == 14 => {
                Some((fault.error_code?, fault.linear_address?))
            }
            _ => None,
        },
        Err(error) => panic!("not admitted: {error:?}"),
    }
}

fn on_the_data_page(address: u64) -> bool {
    address & !0xfff == DATA
}

/// A fault that changed nothing: no retire, the initial state, no write.
fn unchanged(run: &Run, initial: &od_isa::LongModeState) -> Result<(), String> {
    let retired = run
        .result
        .as_ref()
        .map_or(0, |result| result.instructions_executed);
    if retired != 0 || run.state != *initial || !run.memory.writes.is_empty() {
        return Err(format!(
            "changed: retired {retired}, state equal {}, writes {:x?}",
            run.state == *initial,
            run.memory.writes
        ));
    }
    Ok(())
}

/// The result of one check: `None` — the encoding has no data access to the data page (skipped),
/// `Some(Ok)` — checked and right, `Some(Err)` — a failure.
type Checked = Option<Result<(), String>>;

/// Every `Neutral` encoding of the corpus through `check`: (encodings, checked, failures).
fn check_all(check: impl Fn(&[u8]) -> Checked + Sync) -> (usize, usize, Vec<String>) {
    let encodings = neutral_corpus();
    let results = parallel_map_with(&encodings, || (), |_, bytes| check(bytes));
    let checked = results.iter().filter(|result| result.is_some()).count();
    let failures = results
        .into_iter()
        .flatten()
        .filter_map(Result::err)
        .collect();
    (encodings.len(), checked, failures)
}

fn report(count: usize, checked: usize, failures: &[String]) {
    // Review F9: no empty pass.
    assert!(checked > 5_000, "{checked} of {count} encodings checked");
    assert!(
        failures.is_empty(),
        "{} of {count} Neutral encodings:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn neutral_memory_forms_on_a_supervisor_page_fault_with_the_user_bit_at_cpl3() {
    let (count, checked, failures) = check_all(|bytes| {
        let (absent, _) = run(bytes, 0, DataPage::Absent, true);
        let (code0, address) =
            page_fault(&absent).filter(|(_, address)| on_the_data_page(*address))?;
        // W of the user fault: that of the same access at CPL0, except the forms of the literal list
        // (review F2), whose destination the CPL3 entry reads with write intent (W = 1).
        let mut expected = code0 | 1 | 4;
        if cpl3_write_intent_form(bytes) {
            expected |= 2;
        }
        // Review F9: at CR0.WP = 1 and 0 (WP does not matter at CPL3).
        let mut errors = Vec::new();
        for wp in [true, false] {
            let (cpl3, initial) = run(bytes, 3, DataPage::Supervisor, wp);
            let verdict = match page_fault(&cpl3) {
                Some((code, at)) if code == expected && at == address => unchanged(&cpl3, &initial),
                _ => Err(format!(
                    "expected #PF({expected:#x}) at {address:#x}, got {:?}",
                    cpl3.result
                )),
            };
            if let Err(error) = verdict {
                errors.push(format!("WP {wp}: {error}"));
            }
        }
        Some(if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{} {}: {}",
                hex(bytes),
                operation_name(bytes),
                errors.join("; ")
            ))
        })
    });
    report(count, checked, &failures);
}

#[test]
fn neutral_writes_to_a_read_only_user_page_fault_at_cpl3() {
    let (count, checked, failures) = check_all(|bytes| {
        let name = operation_name(bytes);
        let (absent, _) = run(bytes, 0, DataPage::Absent, true);
        page_fault(&absent).filter(|(_, address)| on_the_data_page(*address))?;
        let (cpl0, _) = run(bytes, 0, DataPage::ReadOnly, true);
        let mut errors = Vec::new();
        for wp in [true, false] {
            let (cpl3, initial) = run(bytes, 3, DataPage::ReadOnly, wp);
            match page_fault(&cpl0)
                .filter(|(code, address)| code & 2 != 0 && on_the_data_page(*address))
            {
                // A write or a read-modify-write: the user write faults with P, W and U at both WP.
                Some((code0, address)) => match page_fault(&cpl3) {
                    Some((code, at)) if code == code0 | 4 && at == address => {
                        if let Err(error) = unchanged(&cpl3, &initial) {
                            errors.push(format!("WP {wp}: {error}"));
                        }
                    }
                    _ => errors.push(format!(
                        "WP {wp}: expected #PF({:#x}) at {address:#x}, got {:?}",
                        code0 | 4,
                        cpl3.result
                    )),
                },
                // A read (or no access to the page): the user read of a read-only page executes
                // like the supervisor one (the exit kind, RIP, RFLAGS and the GPRs; `MovSregStore`
                // of CS/SS stores the own selector and the x87 forms the own FCS — the
                // differential test masks those).
                None => {
                    let comparable = |run: &Run| {
                        let exit = run.result.as_ref().map(|result| {
                            let mut exit = result.exit.clone();
                            if let Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) = &mut exit
                                && fault.vector == 14
                            {
                                fault.error_code = fault.error_code.map(|code| code & !4);
                            }
                            (exit, result.instructions_executed)
                        });
                        let gpr = if name == "MovSregStore" {
                            [0; 16]
                        } else {
                            run.state.gpr
                        };
                        format!(
                            "{exit:?} {:#x} {:#x} {gpr:x?}",
                            run.state.rip, run.state.rflags
                        )
                    };
                    if comparable(&cpl3) != comparable(&cpl0) {
                        errors.push(format!(
                            "WP {wp}: cpl0 {} | cpl3 {}",
                            comparable(&cpl0),
                            comparable(&cpl3)
                        ));
                    }
                }
            }
        }
        Some(if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("{} {name}: {}", hex(bytes), errors.join("; ")))
        })
    });
    report(count, checked, &failures);
}
