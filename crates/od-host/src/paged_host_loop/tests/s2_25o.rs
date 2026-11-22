//! S2-25o: the host loop runs a vCPU on PAE paging (CR0.PG = 1, CR4.PAE = 1, EFER.LME = 0) through the
//! 32-bit model over the PAE adapter instead of refusing it, and names the first instruction it does not
//! cover. The JIT and the Long64 interpreter are not entered for such a state (no block is admitted, no
//! retirement is theirs). The 32-bit model is checked against `ProtectedCpu` over physical memory in
//! `od-machine` (`pae_protected_step`); here the loop accounting, the exits and the exact refusals.

use super::*;
use od_isa::protected_exec::ProtectedInstruction;
use od_machine::{PaeProtectedStop, PaeStateRefusal};

const PAE_PDPT: u64 = 0x1000;
const PAE_PD: u64 = 0x2000;
const PAE_PT: u64 = 0x3000;
const PAE_CODE: u64 = 0x8000;
const PAE_DATA: u64 = 0x9000;
const PAE_EFER: u64 = 1 << 11;

fn pae_config() -> PagedHostLoopConfig {
    PagedHostLoopConfig {
        cr3: PAE_PDPT,
        efer: PAE_EFER,
        access_context: context(),
    }
}

fn le(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

/// `mov eax, 0x12345678`, `mov [0x9010], eax`, `mov edx, [0x9010]`, `cmp edx, [0x9014]` (not equal),
/// `jne +5` (taken, skips the next `mov`), `mov eax, 0xDEADBEEF`, `add esp, 0x10`, then `tail`.
fn pae_program(tail: &[u8]) -> Vec<u8> {
    [
        &[0xB8][..],
        &le(0x1234_5678),
        &[0xA3],
        &le(0x9010),
        &[0x8B, 0x15],
        &le(0x9010),
        &[0x3B, 0x15],
        &le(0x9014),
        &[0x75, 0x05],
        &[0xB8],
        &le(0xDEAD_BEEF),
        &[0x81, 0xC4],
        &le(0x10),
        tail,
    ]
    .concat()
}
/// Instructions of [`pae_program`] that retire before the tail (the skipped `mov` does not).
const PAE_RETIRING: u64 = 6;

/// PDPTE0 -> PD, PD[0] -> PT, PT[8] -> the code page, PT[9] -> the data page; identity-mapped.
fn pae_bus(code: &[u8]) -> MachineBus {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_BYTES).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(RAM_BYTES), vec![], vec![], vec![]).unwrap();
    for (address, value) in [
        (PAE_PDPT, PAE_PD | 1),
        (PAE_PD, PAE_PT | 7),
        (PAE_PT + 8 * 8, PAE_CODE | 7),
        (PAE_PT + 9 * 8, PAE_DATA | 7),
    ] {
        put_qword(&mut bus, address, value);
    }
    bus.patch_physical_ram(GuestPhysicalAddress::new(PAE_CODE), code)
        .unwrap();
    bus
}

fn pae_state() -> LongModeState {
    let mut state = make_state(0, 0, 0);
    state.rip = PAE_CODE;
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(CR4);
    state.rflags = 2;
    state.gpr[Register::Rsp as usize] = 0x7000;
    state
}

struct PaeRun {
    report: PagedHostLoopReport,
    writes: Vec<(u64, u64)>,
}

fn run_pae(
    session: &mut PagedFetchedLoadSession,
    bus: &mut MachineBus,
    state: &mut LongModeState,
    budget: u64,
) -> PaeRun {
    let control = PagedHostLoopControl::new();
    let mut rflags_defined = RFLAGS_DEFINED;
    let mut writes = Vec::new();
    let report = run_paged_host_loop(
        session,
        &control,
        pae_config(),
        state,
        bus,
        &mut rflags_defined,
        budget,
        |range| writes.push((range.start().get(), range.len())),
    );
    PaeRun { report, writes }
}

fn qword(bus: &mut MachineBus, address: u64) -> u64 {
    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
        .unwrap()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn a_pae_vcpu_runs_to_the_first_instruction_outside_the_covered_set_and_names_it() {
    let _guard = jit_guard();
    let mut session = new_session();
    let mut bus = pae_bus(&pae_program(&[0x0F, 0x22, 0xD8])); // mov cr3, eax
    let mut state = pae_state();
    let run = run_pae(&mut session, &mut bus, &mut state, 100);
    assert_eq!(
        run.report.exit,
        PagedHostLoopExit::PaeProtected(PaeProtectedStop::Uncovered {
            eip: (PAE_CODE + 35) as u32,
            instruction: ProtectedInstruction::MovCr3Eax,
            length: 3,
        })
    );
    // The six instructions retired as the interpreter's; no JIT, nothing admitted.
    assert_eq!(run.report.retired_interpreter, PAE_RETIRING);
    assert_eq!(run.report.steps_interpreter, PAE_RETIRING);
    assert_eq!((run.report.retired_jit, run.report.steps_jit), (0, 0));
    assert_eq!(session.admissions(), 0);
    assert_eq!(run.report.remaining_budget, 100 - PAE_RETIRING);
    assert!(run.report.budget_is_consistent());
    assert!(run.report.page_fault.is_none());
    // The state is that of the program up to the uncovered instruction.
    assert_eq!(state.rip, PAE_CODE + 35);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1234_5678);
    assert_eq!(state.gpr[Register::Rdx as usize], 0x1234_5678);
    assert_eq!(state.gpr[Register::Rsp as usize], 0x7010);
    assert_eq!(
        bus.read_physical(
            GuestPhysicalAddress::new(PAE_DATA + 0x10),
            AccessWidth::Dword
        )
        .unwrap(),
        0x1234_5678
    );
    // The walks: the code leaf was accessed, the data leaf accessed and (by the store) dirty; the
    // write list holds those entry writes and the operand bytes.
    assert_eq!(qword(&mut bus, PAE_PT + 8 * 8) & 0x60, 0x20);
    assert_eq!(qword(&mut bus, PAE_PT + 9 * 8) & 0x60, 0x60);
    assert!(
        run.writes.contains(&(PAE_DATA + 0x10, 4)),
        "{:?}",
        run.writes
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn a_pae_vcpu_stops_on_its_budget_and_resumes_exactly() {
    let _guard = jit_guard();
    let mut session = new_session();
    let mut bus = pae_bus(&pae_program(&[0x0F, 0x22, 0xD8]));
    let mut state = pae_state();
    let first = run_pae(&mut session, &mut bus, &mut state, 3);
    assert_eq!(first.report.exit, PagedHostLoopExit::BudgetExhausted);
    assert_eq!(first.report.retired_interpreter, 3);
    assert!(first.report.budget_is_consistent());
    assert_eq!(state.rip, PAE_CODE + 16, "mov, mov, mov edx");
    let second = run_pae(&mut session, &mut bus, &mut state, 100);
    assert_eq!(second.report.retired_interpreter, PAE_RETIRING - 3);
    assert!(matches!(
        second.report.exit,
        PagedHostLoopExit::PaeProtected(PaeProtectedStop::Uncovered { .. })
    ));
    assert_eq!(state.rip, PAE_CODE + 35);
    // A zero budget runs nothing and fetches nothing.
    let before = state.clone();
    let none = run_pae(&mut session, &mut bus, &mut state, 0);
    assert_eq!(none.report.exit, PagedHostLoopExit::BudgetExhausted);
    assert_eq!(state, before);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn a_page_fault_of_a_pae_vcpu_is_a_guest_fault_with_the_record_and_the_state_of_the_instruction() {
    let _guard = jit_guard();
    let mut session = new_session();
    // mov eax, [0xA000]: the page table has no entry for linear page 10.
    let mut bus = pae_bus(&[&[0xB9][..], &le(7), &[0xA1], &le(0xA000)].concat());
    let mut state = pae_state();
    let run = run_pae(&mut session, &mut bus, &mut state, 10);
    let PagedHostLoopExit::GuestFault(fault) = &run.report.exit else {
        panic!("a guest fault, got {:?}", run.report.exit);
    };
    assert_eq!(
        (fault.vector, fault.error_code, fault.linear_address),
        (14, Some(0), Some(0xA000))
    );
    let record = run.report.page_fault.as_ref().expect("the record");
    assert_eq!(
        (record.saved_rip(), record.cr2_value()),
        (PAE_CODE + 5, 0xA000)
    );
    assert_eq!(run.report.retired_interpreter, 1, "the mov ecx retired");
    assert_eq!(
        state.rip,
        PAE_CODE + 5,
        "the faulting instruction did not retire"
    );
    assert_eq!(state.gpr[Register::Rcx as usize], 7);
    assert!(run.report.budget_is_consistent());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn wrmsr_and_a_refused_form_are_named_and_a_state_that_is_not_the_model_is_refused_with_its_reason()
{
    let _guard = jit_guard();
    let mut session = new_session();
    // WRMSR: EFER is the loop configuration's, so a written value could not be handed back.
    let mut bus = pae_bus(&[0x0F, 0x30]);
    let mut state = pae_state();
    let run = run_pae(&mut session, &mut bus, &mut state, 10);
    assert_eq!(
        run.report.exit,
        PagedHostLoopExit::PaeProtected(PaeProtectedStop::Uncovered {
            eip: PAE_CODE as u32,
            instruction: ProtectedInstruction::Wrmsr,
            length: 2,
        })
    );
    // An opcode of no covered form: the model's own refusal, with its bytes.
    let mut bus = pae_bus(&[0x90]);
    let mut state = pae_state();
    let run = run_pae(&mut session, &mut bus, &mut state, 10);
    assert!(
        matches!(
            run.report.exit,
            PagedHostLoopExit::PaeProtected(PaeProtectedStop::Refused(
                od_isa::protected_exec::ProtectedRefusal::Unsupported { eip: 0x8000, .. }
            ))
        ),
        "{:?}",
        run.report.exit
    );
    assert_eq!(run.report.retired_total(), 0);
    // CPL 3 (a CS selector with RPL 3): the model is CPL 0.
    let mut bus = pae_bus(&[0x0F, 0x09]);
    let mut state = pae_state();
    state.cs_selector = Some(0x1B);
    let before = state.clone();
    let run = run_pae(&mut session, &mut bus, &mut state, 10);
    assert_eq!(
        run.report.exit,
        PagedHostLoopExit::PaeProtected(PaeProtectedStop::State(PaeStateRefusal::NotCpl0 {
            cs_selector: 0x1B
        }))
    );
    assert_eq!(state, before);
    assert!(run.writes.is_empty(), "nothing was walked");
}

/// The parallel mode has only `&MachineBus`: it keeps the S2-25n refusal, named exactly, before any
/// fetch or walk.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn a_shared_bus_keeps_the_exact_pae_refusal() {
    let _guard = jit_guard();
    let mut session = new_session();
    let control = PagedHostLoopControl::new();
    let bus = pae_bus(&pae_program(&[]));
    let mut state = pae_state();
    let before = state.clone();
    let mut rflags_defined = RFLAGS_DEFINED;
    let mut writes = Vec::new();
    let report = run_paged_host_loop_shared(
        &mut session,
        &control,
        pae_config(),
        &mut state,
        &bus,
        0,
        &mut rflags_defined,
        10,
        |range| writes.push(range),
        || false,
    );
    assert_eq!(
        report.exit,
        PagedHostLoopExit::Interpreter(od_machine::PagedProbeExit::Rejected(
            od_machine::PagedProbeRejection::Configuration(
                od_machine::ProbeConfigError::PaeProtectedMode {
                    cr0: 0x8005_0033,
                    cr4: CR4,
                    efer: PAE_EFER,
                }
            )
        ))
    );
    assert_eq!(report.retired_total(), 0);
    assert!(writes.is_empty());
    assert_eq!(state, before);
}

/// A shared bus names the first failed register check in the order of the four-level configuration
/// (`CR0.ET` clear is `Cr0EtRequired`), and `PaeProtectedMode` only for registers that pass them; the
/// exclusive bus gives the same reason through the PAE snapshot. Before any fetch or walk in both.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn a_pae_state_with_cr0_et_clear_is_refused_with_that_reason_on_either_bus() {
    let _guard = jit_guard();
    let mut session = new_session();
    let control = PagedHostLoopControl::new();
    let cr0_without_et = 0x8005_0023u64;
    let mut shared_state = pae_state();
    shared_state.cr0 = Some(cr0_without_et);
    let before = shared_state.clone();
    let shared_bus = pae_bus(&pae_program(&[]));
    let mut rflags_defined = RFLAGS_DEFINED;
    let mut writes = Vec::new();
    let report = run_paged_host_loop_shared(
        &mut session,
        &control,
        pae_config(),
        &mut shared_state,
        &shared_bus,
        0,
        &mut rflags_defined,
        10,
        |range| writes.push(range),
        || false,
    );
    assert_eq!(
        report.exit,
        PagedHostLoopExit::Interpreter(od_machine::PagedProbeExit::Rejected(
            od_machine::PagedProbeRejection::Configuration(
                od_machine::ProbeConfigError::Cr0EtRequired {
                    value: cr0_without_et
                }
            )
        ))
    );
    assert_eq!(report.retired_total(), 0);
    assert!(writes.is_empty());
    assert_eq!(shared_state, before);

    let mut bus = pae_bus(&pae_program(&[]));
    let mut state = pae_state();
    state.cr0 = Some(cr0_without_et);
    let run = run_pae(&mut session, &mut bus, &mut state, 10);
    assert_eq!(
        run.report.exit,
        PagedHostLoopExit::PaeProtected(PaeProtectedStop::State(PaeStateRefusal::Paging(
            od_machine::PaeConfigError::Config(od_machine::ProbeConfigError::Cr0EtRequired {
                value: cr0_without_et
            })
        )))
    );
    assert_eq!(run.report.retired_total(), 0);
    assert!(run.writes.is_empty(), "nothing was walked");
}

/// `EFLAGS.VM` = 1 is a virtual-8086 vCPU, not the flat CPL 0 model: the loop stops with that reason
/// before any fetch.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn a_virtual_8086_pae_state_is_refused_before_any_fetch() {
    let _guard = jit_guard();
    let mut session = new_session();
    let mut bus = pae_bus(&pae_program(&[]));
    let mut state = pae_state();
    state.rflags = 2 | (1 << 17);
    let before = state.clone();
    let run = run_pae(&mut session, &mut bus, &mut state, 10);
    assert_eq!(
        run.report.exit,
        PagedHostLoopExit::PaeProtected(PaeProtectedStop::Virtual8086 {
            eflags: 2 | (1 << 17)
        })
    );
    assert_eq!(run.report.retired_total(), 0);
    assert!(run.writes.is_empty(), "nothing was walked");
    assert_eq!(state, before);
}
