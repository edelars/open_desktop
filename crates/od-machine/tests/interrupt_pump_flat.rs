//! S4-27i1: flat event pump (`InterruptPump`) over the accepted S4-27c/d/e
//! primitives.
//!
//! The images are assembled here byte-for-byte (mirroring the Python builder's
//! validated encodings) so these tests need no toolchain; the builder-to-runner
//! test exercises the real clang/lld kernel. Expected values come from the Intel
//! SDM (see the builder's module documentation), not from interpreter output:
//!
//! * 64-bit interrupt gate frame: SS, RSP, RFLAGS, CS, RIP (five qwords); the
//!   stack top 0x6ff0 is 16-byte aligned, so RSP becomes 0x6ff0 - 40 = 0x6fc8;
//!   the pushed RFLAGS is 0x202 and IF is cleared in the handler (RFLAGS 0x2).
//! * STI delays acceptance by one instruction; a wake from HLT pushes the RIP
//!   after HLT; IF=0 HLT is not woken by a maskable vector; a request is
//!   accepted only above the in-service priority class until EOI.

use od_isa::Register;
use od_machine::{
    InterruptPump, PumpEntrySource, PumpExit, PumpInjection, PumpRejection, PumpTraceKind as K,
    STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES, STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_A as VEC_A,
    STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_B as VEC_B, Stage4InterruptPumpProbeLoadError,
    load_stage4_interrupt_pump_probe,
};

const BASE: usize = 0x1000;
const FRAME: u64 = 0x6fc8;
const STACK_TOP: u64 = 0x6ff0;
const LAPIC: u64 = 0xfee0_0000;

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    StiHlt,
    If1Hlt,
    CliHlt,
    Rep,
}

struct Built {
    image: Vec<u8>,
    hlt: u64,
    handler_a: u64,
    handler_b: u64,
}

fn mov_edi() -> Vec<u8> {
    vec![0xbf, 0x00, 0x00, 0xe0, 0xfe]
}

fn store_eax(disp: u32) -> Vec<u8> {
    let mut code = vec![0x89, 0x87];
    code.extend(disp.to_le_bytes());
    code
}

fn eoi(omit: bool) -> Vec<u8> {
    if omit {
        return vec![];
    }
    let mut code = vec![0xb8, 0, 0, 0, 0];
    code.extend(store_eax(0xb0));
    code
}

fn entry(scenario: Scenario) -> Vec<u8> {
    let mut code = mov_edi();
    code.extend([0xb8, 0xff, 0x01, 0, 0]);
    code.extend(store_eax(0xf0));
    match scenario {
        Scenario::StiHlt => code.extend([0xfb, 0xf4, 0xf4]),
        Scenario::If1Hlt => code.extend([0xfb, 0x90, 0xf4]),
        Scenario::CliHlt => code.extend([0xfa, 0xf4]),
        Scenario::Rep => {
            code.extend([0xfb, 0x90]);
            code.extend([0xb9, 0xd0, 0x07, 0, 0]);
            code.extend([0xbf, 0x00, 0x50, 0, 0]);
            code.extend([0xf3, 0xaa, 0xf4]);
        }
    }
    code.extend([0xeb, 0xfe]);
    code
}

fn handler_a(omit_eoi: bool) -> Vec<u8> {
    let mut code = mov_edi();
    code.extend([0xb8, 0x41, 0x40, 0x04, 0x00]); // ICR: fixed, assert, self, 0x41
    code.extend(store_eax(0x300));
    code.extend(eoi(omit_eoi));
    code.extend([0x48, 0xcf]);
    code
}

fn handler_b(omit_eoi: bool) -> Vec<u8> {
    let mut code = mov_edi();
    code.extend(eoi(omit_eoi));
    code.extend([0x48, 0xcf]);
    code
}

fn put(image: &mut [u8], gpa: usize, bytes: &[u8]) {
    image[gpa - BASE..gpa - BASE + bytes.len()].copy_from_slice(bytes);
}

fn gate(handler: u64) -> Vec<u8> {
    let mut gate = Vec::new();
    gate.extend((handler as u16).to_le_bytes());
    gate.extend(8_u16.to_le_bytes());
    gate.extend([0, 0x8e, 0, 0]);
    gate.extend([0; 8]);
    gate
}

fn build(scenario: Scenario, omit_eoi: bool) -> Built {
    let mut image = vec![0; STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES];
    for (i, d) in [0_u64, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff]
        .iter()
        .enumerate()
    {
        put(&mut image, 0x2000 + 8 * i, &d.to_le_bytes());
    }
    let entry = entry(scenario);
    let a = handler_a(omit_eoi);
    let handler_a_gpa = 0x4000 + entry.len() as u64;
    let handler_b_gpa = handler_a_gpa + a.len() as u64;
    put(&mut image, 0x3000 + 0x41 * 16, &gate(handler_b_gpa));
    put(&mut image, 0x3000 + 0x51 * 16, &gate(handler_a_gpa));
    put(&mut image, 0x4000, &entry);
    put(&mut image, handler_a_gpa as usize, &a);
    put(&mut image, handler_b_gpa as usize, &handler_b(omit_eoi));
    // SDM-derived: the prologue is 16 bytes, so the first HLT follows it plus
    // the bytes before HLT in each scenario.
    let hlt = 0x4000
        + 16
        + match scenario {
            Scenario::StiHlt | Scenario::CliHlt => 1,
            Scenario::If1Hlt => 2,
            Scenario::Rep => 14,
        };
    Built {
        image,
        hlt,
        handler_a: handler_a_gpa,
        handler_b: handler_b_gpa,
    }
}

fn pump(scenario: Scenario, omit_eoi: bool) -> (InterruptPump, Built) {
    let built = build(scenario, omit_eoi);
    let pump = load_stage4_interrupt_pump_probe(&built.image).unwrap();
    (pump, built)
}

fn kinds(pump: &InterruptPump) -> Vec<K> {
    pump.trace()
        .iter()
        .map(|event| event.kind.clone())
        .collect()
}

fn entry_of(vector: u8, source: PumpEntrySource) -> K {
    K::Entry { vector, source }
}

fn frame(pump: &mut InterruptPump) -> [u64; 5] {
    let mut words = [0; 5];
    for (index, word) in words.iter_mut().enumerate() {
        *word = pump.peek_ram_qword(FRAME + 8 * index as u64).unwrap();
    }
    words
}

/// SS, RSP, RFLAGS, CS, RIP from high to low address; returned low to high.
fn expected_frame(rip: u64) -> [u64; 5] {
    [rip, 8, 0x202, STACK_TOP, 0x10]
}

fn rsp(pump: &InterruptPump) -> u64 {
    pump.state().gpr[Register::Rsp as usize]
}

/// Run the three prologue instructions (Local APIC software enable).
fn prologue(pump: &mut InterruptPump) {
    let result = pump.run(3, || false);
    assert_eq!(result.exit, PumpExit::Budget);
    assert_eq!((result.quanta, result.retired), (3, 3));
    assert_eq!(pump.state().rip, 0x4010);
}

fn request(pump: &InterruptPump, vector: u8) {
    pump.apic().request_fixed_edge(vector).unwrap();
}

#[test]
fn pending_before_sti_hlt_is_delivered_after_hlt_with_rip_after_hlt() {
    let (mut pump, built) = pump(Scenario::StiHlt, false);
    prologue(&mut pump);
    request(&pump, VEC_B);
    assert_eq!(pump.apic().eligible(), Some(VEC_B));
    assert_eq!(pump.state().rflags & 0x200, 0, "IF is still clear");

    // STI: the request is eligible but the next boundary is in the STI shadow.
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    assert_eq!((pump.state().rip, pump.state().rflags), (built.hlt, 0x202));
    assert_eq!(pump.apic().eligible(), Some(VEC_B));
    assert!(!kinds(&pump).iter().any(|k| matches!(k, K::Entry { .. })));

    // HLT retires first (RIP after HLT) and the CPU waits.
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    assert!(pump.is_waiting());
    assert_eq!(pump.state().rip, built.hlt + 1);
    assert_eq!(pump.apic().vector_state(VEC_B), (true, false));

    // Wake: the frame carries the RIP after HLT; the handler has not started.
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    assert!(!pump.is_waiting());
    assert_eq!(pump.state().rip, built.handler_b);
    assert_eq!((rsp(&pump), pump.state().rflags), (FRAME, 0x2));
    assert_eq!(pump.apic().vector_state(VEC_B), (false, true));
    assert_eq!(frame(&mut pump), expected_frame(built.hlt + 1));

    let result = pump.run(100, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    // The handler retires mov edi, mov eax, mov [EOI], IRETQ; then the second
    // HLT retires.
    assert_eq!(result.retired, 5);
    assert_eq!(pump.apic().vector_state(VEC_B), (false, false));
    assert!(pump.is_waiting());
    assert_eq!(
        (pump.state().rip, rsp(&pump), pump.state().rflags),
        (built.hlt + 2, STACK_TOP, 0x202)
    );
    assert_eq!(
        kinds(&pump),
        vec![
            K::Irr { vector: VEC_B },
            K::Sti,
            K::Hlt,
            entry_of(VEC_B, PumpEntrySource::HaltWake),
            K::Eoi { vector: VEC_B },
            K::Iretq,
            K::Hlt,
        ]
    );
}

#[test]
fn pending_with_if_set_before_hlt_is_accepted_at_the_boundary_with_rip_at_hlt() {
    let (mut pump, built) = pump(Scenario::If1Hlt, false);
    prologue(&mut pump);
    request(&pump, VEC_B);
    // STI, then NOP in the shadow.
    assert_eq!(pump.run(2, || false).exit, PumpExit::Budget);
    assert!(!kinds(&pump).iter().any(|k| matches!(k, K::Entry { .. })));
    assert_eq!((pump.state().rip, pump.state().rflags), (built.hlt, 0x202));

    // The boundary before HLT accepts the vector; HLT has not executed.
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    assert!(!pump.is_waiting());
    assert_eq!(frame(&mut pump), expected_frame(built.hlt));
    // The first handler instruction (mov edi) ran inside the delivery quantum.
    assert_eq!(pump.state().rip, built.handler_b + 5);
    assert_eq!(pump.state().gpr[Register::Rdi as usize], LAPIC);
    assert_eq!((rsp(&pump), pump.state().rflags), (FRAME, 0x2));
    assert_eq!(pump.apic().vector_state(VEC_B), (false, true));

    let result = pump.run(100, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    // After IRETQ the interrupted HLT executes with IF=1 and waits.
    assert_eq!(pump.state().rip, built.hlt + 1);
    assert_eq!((rsp(&pump), pump.state().rflags), (STACK_TOP, 0x202));
    assert_eq!(
        kinds(&pump),
        vec![
            K::Irr { vector: VEC_B },
            K::Sti,
            entry_of(VEC_B, PumpEntrySource::Boundary),
            K::Eoi { vector: VEC_B },
            K::Iretq,
            K::Hlt,
        ]
    );
}

#[test]
fn cli_hlt_is_idle_halt_and_a_pending_vector_stays_pending() {
    let (mut pump, built) = pump(Scenario::CliHlt, false);
    prologue(&mut pump);
    request(&pump, VEC_B);
    let result = pump.run(100, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert_eq!(result.retired, 2);
    assert!(pump.is_waiting());
    assert_eq!(
        (pump.state().rip, pump.state().rflags),
        (built.hlt + 1, 0x2)
    );
    assert_eq!(pump.apic().vector_state(VEC_B), (true, false));
    assert_eq!(
        pump.apic().eligible(),
        Some(VEC_B),
        "pending, gated by IF=0"
    );
    assert_eq!(kinds(&pump), vec![K::Irr { vector: VEC_B }, K::Cli, K::Hlt]);
    // IdleHalt is not terminal: asking again is idle again, without delivery.
    assert_eq!(pump.run(10, || false).exit, PumpExit::IdleHalt);
    assert_eq!(pump.trace().len(), 3);
}

#[test]
fn self_ipi_b_during_handler_a_is_delivered_after_eoi_and_iretq_in_order() {
    let (mut pump, built) = pump(Scenario::StiHlt, false);
    prologue(&mut pump);
    request(&pump, VEC_A);
    // STI, HLT, wake into A.
    assert_eq!(pump.run(3, || false).exit, PumpExit::Budget);
    assert_eq!(pump.state().rip, built.handler_a);
    assert_eq!(frame(&mut pump), expected_frame(built.hlt + 1));
    assert_eq!(pump.apic().vector_state(VEC_A), (false, true));

    // Handler A: mov edi, mov eax, mov [ICR],eax raises the self-IPI B.
    assert_eq!(pump.run(3, || false).exit, PumpExit::Budget);
    assert_eq!(pump.apic().vector_state(VEC_B), (true, false));
    assert_eq!(pump.apic().vector_state(VEC_A), (false, true));
    assert_eq!(
        pump.apic().eligible(),
        None,
        "B has a lower priority class than A in service"
    );

    // mov eax, mov [EOI],eax: ISR of A clears, B becomes eligible but IF=0.
    assert_eq!(pump.run(2, || false).exit, PumpExit::Budget);
    assert_eq!(pump.apic().vector_state(VEC_A), (false, false));
    assert_eq!(pump.apic().eligible(), Some(VEC_B));
    assert_eq!(pump.state().rflags, 0x2);
    assert!(!kinds(&pump).contains(&entry_of(VEC_B, PumpEntrySource::Boundary)));

    // IRETQ restores IF=1 and the RSP; the next boundary accepts B at the HLT
    // that A interrupted (RIP in B's frame is that HLT).
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    assert_eq!(
        (pump.state().rip, rsp(&pump), pump.state().rflags),
        (built.hlt + 1, STACK_TOP, 0x202)
    );
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    assert_eq!(frame(&mut pump), expected_frame(built.hlt + 1));
    assert_eq!(pump.apic().vector_state(VEC_B), (false, true));
    assert_eq!((rsp(&pump), pump.state().rflags), (FRAME, 0x2));

    assert_eq!(pump.run(100, || false).exit, PumpExit::IdleHalt);
    assert_eq!(
        (pump.state().rip, rsp(&pump), pump.state().rflags),
        (built.hlt + 2, STACK_TOP, 0x202)
    );
    for vector in [VEC_A, VEC_B] {
        assert_eq!(pump.apic().vector_state(vector), (false, false));
    }
    assert_eq!(
        kinds(&pump),
        vec![
            K::Irr { vector: VEC_A },
            K::Sti,
            K::Hlt,
            entry_of(VEC_A, PumpEntrySource::HaltWake),
            K::Irr { vector: VEC_B },
            K::Eoi { vector: VEC_A },
            K::Iretq,
            entry_of(VEC_B, PumpEntrySource::Boundary),
            K::Eoi { vector: VEC_B },
            K::Iretq,
            K::Hlt,
        ]
    );
}

#[test]
fn without_eoi_the_second_vector_is_never_delivered() {
    let (mut pump, built) = pump(Scenario::StiHlt, true);
    prologue(&mut pump);
    request(&pump, VEC_A);
    let result = pump.run(100, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    // A stays in service, B stays pending and is not eligible.
    assert_eq!(pump.apic().vector_state(VEC_A), (false, true));
    assert_eq!(pump.apic().vector_state(VEC_B), (true, false));
    assert_eq!(pump.apic().eligible(), None);
    assert!(pump.is_waiting());
    assert_eq!(pump.state().rip, built.hlt + 2);
    assert_eq!(
        kinds(&pump),
        vec![
            K::Irr { vector: VEC_A },
            K::Sti,
            K::Hlt,
            entry_of(VEC_A, PumpEntrySource::HaltWake),
            K::Irr { vector: VEC_B },
            K::Iretq,
            K::Hlt,
        ]
    );
}

#[test]
fn without_eoi_a_repeated_request_of_the_same_vector_is_not_accepted() {
    let (mut pump, _) = pump(Scenario::StiHlt, true);
    prologue(&mut pump);
    request(&pump, VEC_B);
    assert_eq!(pump.run(100, || false).exit, PumpExit::IdleHalt);
    assert_eq!(pump.apic().vector_state(VEC_B), (false, true));
    let entries_before = pump.trace().len();
    request(&pump, VEC_B);
    assert_eq!(
        pump.apic().eligible(),
        None,
        "same class as the in-service B"
    );
    assert_eq!(pump.run(100, || false).exit, PumpExit::IdleHalt);
    assert_eq!(pump.apic().vector_state(VEC_B), (true, true));
    let entries = kinds(&pump)
        .iter()
        .filter(|k| matches!(k, K::Entry { .. }))
        .count();
    assert_eq!(entries, 1);
    assert_eq!(
        pump.trace().len(),
        entries_before + 1,
        "only the new IRR event"
    );
}

#[test]
fn injection_skip_shadow_check_breaks_the_sti_hlt_scenario() {
    let (pump, built) = pump(Scenario::StiHlt, false);
    let mut pump = pump.with_injection(PumpInjection::SkipShadowCheck);
    prologue(&mut pump);
    request(&pump, VEC_B);
    let result = pump.run(100, || false);
    // The pump tries to accept the vector in the STI shadow; the primitive
    // refuses, so nothing is delivered and the scenario never reaches HLT.
    assert_eq!(
        result.exit,
        PumpExit::Rejected(PumpRejection::ApicQuantumProfile)
    );
    assert_eq!(pump.state().rip, built.hlt);
    assert!(!pump.is_waiting());
    assert_eq!(pump.apic().vector_state(VEC_B), (true, false));
    assert_eq!(kinds(&pump), vec![K::Irr { vector: VEC_B }, K::Sti]);
    // Final: the rejection is sticky.
    assert_eq!(
        pump.run(1, || false).exit,
        PumpExit::Rejected(PumpRejection::ApicQuantumProfile)
    );
}

#[test]
fn injection_hlt_as_ordinary_instruction_is_terminal_not_a_wait() {
    let (pump, built) = pump(Scenario::StiHlt, false);
    let mut pump = pump.with_injection(PumpInjection::HltAsOrdinary);
    prologue(&mut pump);
    request(&pump, VEC_B);
    let result = pump.run(100, || false);
    // The ordinary interpreter has no HLT: it is a terminal result, never a wait.
    assert!(
        matches!(
            result.exit,
            PumpExit::Terminal(od_machine::DiagnosticTerminalExit::Unsupported { .. })
        ),
        "{:?}",
        result.exit
    );
    assert!(!pump.is_waiting());
    assert_eq!(pump.apic().vector_state(VEC_B), (true, false));
    assert!(
        !kinds(&pump)
            .iter()
            .any(|k| matches!(k, K::Entry { .. } | K::Hlt))
    );
    let _ = built;
}

#[test]
fn partial_rep_delays_the_interrupt_until_the_instruction_completes() {
    let (mut pump, built) = pump(Scenario::Rep, false);
    prologue(&mut pump);
    // STI, NOP, MOV ECX, MOV EDI, then the first REP STOSB chunk of 1024 iterations.
    assert_eq!(pump.run(5, || false).exit, PumpExit::Budget);
    let rep = built.hlt - 2;
    let state = pump.state();
    assert_eq!(state.rip, rep, "boundary is inside the REP");
    assert_eq!(state.gpr[Register::Rcx as usize], 2000 - 1024);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x5000 + 1024);
    request(&pump, VEC_B);
    assert_eq!(pump.apic().eligible(), Some(VEC_B));
    assert_eq!(state_if(&pump), 0x200);

    // Deviation from hardware: the pump finishes the REP first (no APIC check
    // on an incomplete boundary), so there is no entry with RIP at the REP.
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    assert_eq!(pump.state().rip, built.hlt);
    assert_eq!(pump.state().gpr[Register::Rcx as usize], 0);
    assert!(!kinds(&pump).iter().any(|k| matches!(k, K::Entry { .. })));
    assert_eq!(pump.peek_ram_qword(0x57c8), Some(u64::MAX));
    assert_eq!(pump.peek_ram_qword(0x57d0), Some(0));

    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    assert_eq!(frame(&mut pump), expected_frame(built.hlt));
    assert_eq!(pump.run(100, || false).exit, PumpExit::IdleHalt);
    assert_eq!(
        kinds(&pump),
        vec![
            K::Sti,
            K::Irr { vector: VEC_B },
            entry_of(VEC_B, PumpEntrySource::Boundary),
            K::Eoi { vector: VEC_B },
            K::Iretq,
            K::Hlt,
        ]
    );
}

fn state_if(pump: &InterruptPump) -> u64 {
    pump.state().rflags & 0x200
}

#[test]
fn stop_is_checked_at_the_boundary_and_is_final() {
    let (mut pump, _) = pump(Scenario::StiHlt, false);
    let result = pump.run(100, || true);
    assert_eq!(result.exit, PumpExit::Stopped);
    assert_eq!((result.quanta, result.retired), (0, 0));
    assert_eq!(pump.state().rip, 0x4000);
    assert_eq!(pump.run(100, || false).exit, PumpExit::Stopped);

    let (mut pump, _) = pump_in_wait();
    assert!(pump.is_waiting());
    assert_eq!(pump.run(100, || true).exit, PumpExit::Stopped);
    assert_eq!(pump.run(100, || false).exit, PumpExit::Stopped);
}

fn pump_in_wait() -> (InterruptPump, Built) {
    let (mut pump, built) = pump(Scenario::CliHlt, false);
    assert_eq!(pump.run(100, || false).exit, PumpExit::IdleHalt);
    (pump, built)
}

#[test]
fn budget_zero_is_budget_and_a_resumed_run_continues() {
    let (mut pump, _) = pump(Scenario::CliHlt, false);
    let result = pump.run(0, || false);
    assert_eq!(result.exit, PumpExit::Budget);
    assert_eq!(result.quanta, 0);
    assert_eq!(pump.state().rip, 0x4000);
    assert_eq!(pump.run(100, || false).exit, PumpExit::IdleHalt);
}

#[test]
fn loader_rejects_a_wrong_size_and_a_stray_gate() {
    assert_eq!(
        load_stage4_interrupt_pump_probe(&[0; 16]).err(),
        Some(Stage4InterruptPumpProbeLoadError::ImageSize {
            expected: STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES,
            actual: 16
        })
    );
    let mut image = build(Scenario::StiHlt, false).image;
    image[0x3000 + 0x20 * 16 - BASE] = 1;
    assert!(matches!(
        load_stage4_interrupt_pump_probe(&image),
        Err(Stage4InterruptPumpProbeLoadError::Layout(_))
    ));
    let mut image = build(Scenario::StiHlt, false).image;
    image[0x5000 - BASE - 1] = 0; // inside the code page: allowed
    image[BASE - BASE] = 1; // below the GDT: rejected
    assert!(matches!(
        load_stage4_interrupt_pump_probe(&image),
        Err(Stage4InterruptPumpProbeLoadError::Layout(_))
    ));
}
