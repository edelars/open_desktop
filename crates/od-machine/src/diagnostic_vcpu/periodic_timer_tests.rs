//! S4-28b: guest-visible periodic Local APIC timer on a shared virtual clock.
//!
//! The guest profile is built directly from CPU state, as in the S4-27/S4-28a3
//! tests: a flat identity profile with a 64-bit interrupt gate for vector 0x40
//! whose handler increments a RAM counter, writes the Local APIC EOI register
//! through MMIO and executes IRETQ. Every transition is a caller operation:
//! advance, dispatch, APIC quantum or HLT poll.
use super::*;
use crate::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, LOCAL_APIC_MMIO_BASE, LocalApic,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
};
use od_isa::memory::{AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{Register, StiShadow};

const VECTOR: u8 = 0x40;
const HANDLER: u64 = 0x4000;
const MAIN: u64 = 0x4100;
const COUNTER: u64 = 0x5000;
const STACK_TOP: u64 = 0x7000;
/// Timer frequency 1 GHz, divide-by-1 and Initial Count 100: one period is 100 ns.
const PERIOD: u64 = 100;
const LVT_PERIODIC: u64 = 0x2_0040;
const LVT_PERIODIC_MASKED: u64 = 0x3_0040;
const LVT_ONE_SHOT_MASKED: u64 = 0x1_0040;
/// `inc qword [r8]; nop; mov [rdi], esi; iretq` (R8 = counter, RDI = EOI, RSI = 0).
const HANDLER_CODE: [u8; 8] = [0x49, 0xff, 0x00, 0x90, 0x89, 0x37, 0x48, 0xcf];
/// `inc rax` repeated: the main thread's progress is RAX and RIP.
const INC_RAX: [u8; 3] = [0x48, 0xff, 0xc0];

struct Rig {
    cpu: DiagnosticVcpuSession,
    apic: LocalApicHandle,
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

fn put(cpu: &mut DiagnosticVcpuSession, address: u64, bytes: &[u8]) {
    for (offset, byte) in bytes.iter().enumerate() {
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(address + offset as u64),
                AccessWidth::Byte,
                u64::from(*byte),
                |_| {},
            )
            .unwrap();
    }
}

fn put_qword(cpu: &mut DiagnosticVcpuSession, address: u64, value: u64) {
    cpu.bus
        .write_physical(
            GuestPhysicalAddress::new(address),
            AccessWidth::Qword,
            value,
            |_| {},
        )
        .unwrap();
}

fn qword(cpu: &mut DiagnosticVcpuSession, address: u64) -> u64 {
    cpu.bus
        .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
        .unwrap()
}

fn mmio_write(
    cpu: &mut DiagnosticVcpuSession,
    offset: u64,
    value: u64,
) -> Result<(), crate::BusAccessError> {
    cpu.bus.write_physical(
        GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
        AccessWidth::Dword,
        value,
        |_| {},
    )
}

fn counter(cpu: &mut DiagnosticVcpuSession) -> u64 {
    qword(cpu, COUNTER)
}

fn rax(cpu: &DiagnosticVcpuSession) -> u64 {
    cpu.state.gpr[Register::Rax as usize]
}

/// Build the profile with `main` at `MAIN`, the timer programmed (LVT, divide
/// 1, Initial Count 100) at virtual time zero, and the shared clock claimed by
/// the session.
fn rig(main: &[u8], lvt: u64) -> Rig {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0x1000), 0x7000).unwrap();
    let apic_range = GuestPhysicalRange::new(
        GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE),
        crate::LOCAL_APIC_MMIO_SIZE,
    )
    .unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
            MemoryRegionSpec::new(apic_range, MemoryRegionKind::Mmio),
        ],
    )
    .unwrap();
    let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
    let device = LocalApic::with_shared_timer(clock.clone(), 1_000_000_000).unwrap();
    let apic = device.handle();
    let bus = MachineBus::new(
        map,
        RamLimits::new(0x7000),
        vec![Box::new(device)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    let mut state = LongModeState::default();
    state.rip = MAIN;
    state.rflags = 0x202;
    state.gpr[Register::Rsp as usize] = STACK_TOP;
    state.gpr[Register::R8 as usize] = COUNTER;
    state.gpr[Register::Rdi as usize] = LOCAL_APIC_MMIO_BASE + 0xb0;
    state.gpr[Register::Rsi as usize] = 0;
    state.cs_selector = Some(8);
    state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x9b,
        flags: 2,
    });
    state.ss_selector = Some(0x10);
    state.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x93,
        flags: 0,
    });
    state.gdtr = Some(od_isa::state::Gdtr {
        base: 0x2000,
        limit: 0x2f,
    });
    state.idtr = Some(od_isa::state::Idtr {
        base: 0x3000,
        limit: 0xfff,
    });
    state.activity = Some(od_isa::CpuActivity::Running);
    state.sti_shadow = Some(StiShadow::Clear);
    let mut cpu = DiagnosticVcpuSession::new_with_clock_handle(
        state,
        bus,
        context(),
        INTEGER_FLAGS,
        clock,
        RdtscControls { cr4_tsd: false },
    )
    .unwrap();
    // GDT: ring-0 code (0x08), data (0x10) and the gate's target code (0x20).
    put_qword(&mut cpu, 0x2008, 0x00af_9b00_0000_ffff);
    put_qword(&mut cpu, 0x2010, 0x00cf_9300_0000_ffff);
    put_qword(&mut cpu, 0x2020, 0x0020_9b00_0000_0000);
    // Present 64-bit interrupt gate for the timer vector.
    put_qword(&mut cpu, 0x3000 + u64::from(VECTOR) * 16, 0x8e00_0020_4000);
    put_qword(&mut cpu, 0x3008 + u64::from(VECTOR) * 16, 0);
    put(&mut cpu, HANDLER, &HANDLER_CODE);
    put(&mut cpu, MAIN, main);
    put_qword(&mut cpu, COUNTER, 0);
    mmio_write(&mut cpu, 0xf0, 0x1ff).unwrap();
    mmio_write(&mut cpu, 0x320, lvt).unwrap();
    mmio_write(&mut cpu, 0x3e0, 0xb).unwrap();
    mmio_write(&mut cpu, 0x380, PERIOD).unwrap();
    Rig { cpu, apic }
}

fn main_incs(count: usize) -> Vec<u8> {
    INC_RAX.repeat(count)
}

/// `hlt; inc rax` repeated.
fn main_hlts(count: usize) -> Vec<u8> {
    let mut code = Vec::new();
    for _ in 0..count {
        code.push(0xf4);
        code.extend_from_slice(&INC_RAX);
    }
    code
}

fn run(cpu: &mut DiagnosticVcpuSession, budget: u64) -> DiagnosticRunResult {
    cpu.run(budget, || false).unwrap()
}

fn quantum(rig: &mut Rig) -> DiagnosticApicQuantumResult {
    rig.cpu.run_apic_quantum(&rig.apic, || false).unwrap()
}

fn expect_delivered(rig: &mut Rig) {
    match quantum(rig) {
        DiagnosticApicQuantumResult::Delivered {
            vector: VECTOR,
            handler,
        } => assert_eq!(handler.instructions_executed, 1),
        other => panic!("expected delivery, got {other:?}"),
    }
}

fn expect_ordinary(rig: &mut Rig) {
    assert!(
        matches!(quantum(rig), DiagnosticApicQuantumResult::Ordinary(_)),
        "expected ordinary execution"
    );
}

/// After the delivery quantum has run `inc [r8]`: `nop`, guest EOI, IRETQ.
fn finish_handler(cpu: &mut DiagnosticVcpuSession) {
    assert_eq!(run(cpu, 2).instructions_executed, 2);
    assert_eq!(
        cpu.run_same_cpl0_iretq().unwrap(),
        DiagnosticIretqResult::Retired
    );
}

#[test]
fn periodic_guest_handler_counts_every_period_with_eoi() {
    const N: u64 = 8;
    let mut rig = rig(&main_incs(64), LVT_PERIODIC);
    for step in 1..=N {
        // Quiet quantum: no expiry, so the main thread makes progress.
        expect_ordinary(&mut rig);
        let main_flags = rig.cpu.state.rflags;
        rig.cpu.advance_time_ns(PERIOD).unwrap();
        assert!(rig.apic.dispatch_timer().unwrap());
        assert_eq!(rig.apic.pending(), Some(VECTOR));
        // One due occurrence per dispatch: nothing more is due at this time.
        assert!(!rig.apic.dispatch_timer().unwrap());
        expect_delivered(&mut rig);
        assert_eq!(rig.apic.vector_state(VECTOR), (false, true));
        assert_eq!(counter(&mut rig.cpu), step);
        assert_eq!(rig.cpu.state.rflags & 0x200, 0);
        finish_handler(&mut rig.cpu);
        // Guest EOI cleared ISR; the stack and IF are back to the main thread.
        assert_eq!(rig.apic.vector_state(VECTOR), (false, false));
        assert_eq!(rig.cpu.state.gpr[Register::Rsp as usize], STACK_TOP);
        assert_eq!(rig.cpu.state.rflags, main_flags);
        assert_ne!(rig.cpu.state.rflags & 0x200, 0);
        assert_eq!(rig.cpu.state.rip, MAIN + 3 * step);
        assert_eq!(rax(&rig.cpu), step);
        assert_eq!(rig.cpu.virtual_clock().unwrap().now_ns(), step * PERIOD);
    }
    // Resume: the main thread continues exactly where it was interrupted.
    for extra in 0..3 {
        expect_ordinary(&mut rig);
        assert_eq!(rax(&rig.cpu), N + extra + 1);
    }
    assert_eq!(counter(&mut rig.cpu), N);
    assert_eq!(rig.cpu.state.rip, MAIN + 3 * (N + 3));
    assert_eq!(rax(&rig.cpu), N + 3);
    assert_eq!(rig.apic.pending(), None);
}

#[test]
fn periodic_timer_wakes_halt_wait_repeatedly() {
    const WAKES: u64 = 3;
    let mut rig = rig(&main_hlts(4), LVT_PERIODIC);
    for wake in 1..=WAKES {
        let mut wait = match rig.cpu.into_halt_wait() {
            Ok(wait) => wait,
            Err(failure) => panic!("HLT rejected: {:?}", failure.reason),
        };
        // HLT retired: RIP is after it, and the timer has rearmed by itself.
        assert_eq!(wait.state().rip, MAIN + 4 * (wake - 1) + 1);
        assert_eq!(wait.poll_apic(&rig.apic), DiagnosticHaltWakeResult::Waiting);
        // Not yet due half a period in.
        wait.advance_time_ns(PERIOD / 2).unwrap();
        assert!(!rig.apic.dispatch_timer().unwrap());
        assert_eq!(wait.poll_apic(&rig.apic), DiagnosticHaltWakeResult::Waiting);
        wait.advance_time_ns(PERIOD / 2).unwrap();
        assert!(rig.apic.dispatch_timer().unwrap());
        assert_eq!(
            wait.poll_apic(&rig.apic),
            DiagnosticHaltWakeResult::Entered { vector: VECTOR }
        );
        assert_eq!(rig.apic.vector_state(VECTOR), (false, true));
        assert_eq!(
            wait.advance_time_ns(1),
            Err(DiagnosticHaltWaitError::NotWaiting)
        );
        rig.cpu = match wait.into_running() {
            Ok(session) => session,
            Err(_) => panic!("entry did not wake"),
        };
        assert_eq!(rig.cpu.state.rip, HANDLER);
        assert_eq!(rig.cpu.state.activity, Some(od_isa::CpuActivity::Running));
        // Handler: inc [r8], nop, EOI, then IRETQ back after the HLT.
        assert_eq!(run(&mut rig.cpu, 3).instructions_executed, 3);
        assert_eq!(counter(&mut rig.cpu), wake);
        assert_eq!(
            rig.cpu.run_same_cpl0_iretq().unwrap(),
            DiagnosticIretqResult::Retired
        );
        assert_eq!(rig.apic.vector_state(VECTOR), (false, false));
        assert_eq!(rig.cpu.state.rip, MAIN + 4 * (wake - 1) + 1);
        // The instruction after HLT retires on the main thread.
        expect_ordinary(&mut rig);
        assert_eq!(rax(&rig.cpu), wake);
        assert_eq!(rig.cpu.virtual_clock().unwrap().now_ns(), wake * PERIOD);
    }
    assert_eq!(counter(&mut rig.cpu), WAKES);
    assert_eq!(rig.cpu.state.rip, MAIN + 4 * WAKES);
}

#[test]
fn expiry_while_in_service_is_one_edge_delivered_only_after_guest_eoi() {
    let mut rig = rig(&main_incs(16), LVT_PERIODIC);
    rig.cpu.advance_time_ns(PERIOD).unwrap();
    assert!(rig.apic.dispatch_timer().unwrap());
    expect_delivered(&mut rig);
    assert_eq!(rig.apic.vector_state(VECTOR), (false, true));
    assert_eq!(counter(&mut rig.cpu), 1);
    // Model a handler that re-enabled interrupts: with IF=1 only the ISR
    // priority holds the next expiry back (the shared-clock session cannot
    // execute STI through the STI profile).
    rig.cpu.state.rflags |= 0x200;

    // Two more periods pass while the vector is still in service; the timer
    // keeps running (EOI does not control its phase) and the IRR bit is single.
    for _ in 0..2 {
        rig.cpu.advance_time_ns(PERIOD).unwrap();
        assert!(rig.apic.dispatch_timer().unwrap());
        assert_eq!(rig.apic.vector_state(VECTOR), (true, true));
    }
    assert!(!rig.apic.dispatch_timer().unwrap());
    // ISR blocks delivery before EOI: the handler's `nop` runs, nothing nests.
    assert_eq!(rig.apic.eligible(), None);
    expect_ordinary(&mut rig);
    assert_eq!(rig.cpu.state.rip, HANDLER + 4);
    assert_eq!(counter(&mut rig.cpu), 1);
    // The guest EOI instruction retires: ISR clears, the pending edge remains.
    expect_ordinary(&mut rig);
    assert_eq!(rig.apic.vector_state(VECTOR), (true, false));
    assert_eq!(rig.cpu.state.rip, HANDLER + 6);
    // The EOI released the priority block: the coalesced edge is now eligible.
    assert_eq!(rig.apic.eligible(), Some(VECTOR));
    // Return to the main thread (IF=1 from the frame), then deliver it once.
    assert_eq!(
        rig.cpu.run_same_cpl0_iretq().unwrap(),
        DiagnosticIretqResult::Retired
    );
    assert_eq!(rig.cpu.state.rip, MAIN);
    expect_delivered(&mut rig);
    assert_eq!(counter(&mut rig.cpu), 2);
    assert_eq!(rig.apic.vector_state(VECTOR), (false, true));
    finish_handler(&mut rig.cpu);
    assert_eq!(rig.apic.vector_state(VECTOR), (false, false));
    // Three dispatches produced exactly two deliveries; no third edge exists.
    assert_eq!(rig.apic.pending(), None);
    assert_eq!(rig.cpu.state.rip, MAIN);
    assert_eq!(rig.cpu.state.gpr[Register::Rsp as usize], STACK_TOP);
    expect_ordinary(&mut rig);
    assert_eq!(counter(&mut rig.cpu), 2);
    assert_eq!(rax(&rig.cpu), 1);
}

#[test]
fn large_time_jump_coalesces_to_one_interrupt_and_keeps_phase() {
    let mut rig = rig(&main_incs(16), LVT_PERIODIC);
    // 10.5 periods in one advance.
    rig.cpu.advance_time_ns(PERIOD * 10 + PERIOD / 2).unwrap();
    assert!(rig.apic.dispatch_timer().unwrap());
    assert!(!rig.apic.dispatch_timer().unwrap());
    assert_eq!(rig.apic.vector_state(VECTOR), (true, false));
    expect_delivered(&mut rig);
    finish_handler(&mut rig.cpu);
    assert_eq!(counter(&mut rig.cpu), 1);
    expect_ordinary(&mut rig);
    assert_eq!(rig.apic.pending(), None);
    assert!(!rig.apic.dispatch_timer().unwrap());
    // The phase is the original arm time, not the dispatch time: the next
    // boundary is 11 periods, half a period away.
    rig.cpu.advance_time_ns(PERIOD / 2 - 1).unwrap();
    assert!(!rig.apic.dispatch_timer().unwrap());
    rig.cpu.advance_time_ns(1).unwrap();
    assert!(rig.apic.dispatch_timer().unwrap());
    expect_delivered(&mut rig);
    finish_handler(&mut rig.cpu);
    assert_eq!(counter(&mut rig.cpu), 2);
    assert!(!rig.apic.dispatch_timer().unwrap());
}

fn enter(cpu: DiagnosticVcpuSession) -> DiagnosticHaltWait {
    match cpu.into_halt_wait() {
        Ok(wait) => wait,
        Err(failure) => panic!("HLT rejected: {:?}", failure.reason),
    }
}

fn halt_rig(lvt: u64) -> (LocalApicHandle, DiagnosticHaltWait) {
    let rig = rig(&main_hlts(2), lvt);
    (rig.apic, enter(rig.cpu))
}

#[test]
fn halt_gate_if_clear_keeps_irr_waiting_and_gate_open() {
    let mut rig = rig(&main_hlts(2), LVT_PERIODIC);
    rig.cpu.state.rflags = 0x002;
    let apic = rig.apic.clone();
    let mut wait = enter(rig.cpu);
    wait.advance_time_ns(PERIOD).unwrap();
    assert!(apic.dispatch_timer().unwrap());
    assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
    assert_eq!(apic.vector_state(VECTOR), (true, false));
    // Gate reopened: advancing and dispatching is allowed, never Busy.
    wait.advance_time_ns(PERIOD).unwrap();
    assert_eq!(apic.dispatch_timer(), Ok(true));
    assert_eq!(wait.state().activity, Some(od_isa::CpuActivity::Halted));
    assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
    assert_eq!(apic.vector_state(VECTOR), (true, false));
}

#[test]
fn halt_gate_tpr_blocked_vector_keeps_waiting() {
    let (apic, mut wait) = halt_rig(LVT_PERIODIC);
    // TPR class above the timer vector's class: pending but not eligible.
    wait.session
        .bus
        .write_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0x80),
            AccessWidth::Dword,
            0x50,
            |_| {},
        )
        .unwrap();
    wait.advance_time_ns(PERIOD).unwrap();
    assert!(apic.dispatch_timer().unwrap());
    assert_eq!(apic.pending(), Some(VECTOR));
    assert_eq!(apic.eligible(), None);
    assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
    wait.advance_time_ns(PERIOD).unwrap();
    assert_eq!(apic.dispatch_timer(), Ok(true));
    // Lowering TPR makes the single pending edge deliverable.
    wait.session
        .bus
        .write_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0x80),
            AccessWidth::Dword,
            0,
            |_| {},
        )
        .unwrap();
    assert_eq!(
        wait.poll_apic(&apic),
        DiagnosticHaltWakeResult::Entered { vector: VECTOR }
    );
    let session = wait.into_running().ok().expect("entry wakes");
    assert_eq!(session.state.rip, HANDLER);
}

#[test]
fn halt_gate_masked_lvt_consumes_event_without_irr_or_wake() {
    // Periodic masked: occurrences are consumed and the timer keeps rearming.
    let (apic, mut wait) = halt_rig(LVT_PERIODIC_MASKED);
    for _ in 0..3 {
        wait.advance_time_ns(PERIOD).unwrap();
        assert_eq!(apic.dispatch_timer(), Ok(true));
        assert_eq!(apic.pending(), None);
        assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
        assert_eq!(apic.dispatch_timer(), Ok(false));
    }
    // Unmasking does not replay the masked occurrences.
    wait.session
        .bus
        .write_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0x320),
            AccessWidth::Dword,
            LVT_PERIODIC,
            |_| {},
        )
        .unwrap();
    assert_eq!(apic.pending(), None);
    assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
    wait.advance_time_ns(PERIOD).unwrap();
    assert_eq!(apic.dispatch_timer(), Ok(true));
    assert_eq!(
        wait.poll_apic(&apic),
        DiagnosticHaltWakeResult::Entered { vector: VECTOR }
    );

    // One-shot masked: the event is consumed once and never reappears.
    let (apic, mut wait) = halt_rig(LVT_ONE_SHOT_MASKED);
    wait.advance_time_ns(PERIOD).unwrap();
    assert_eq!(apic.dispatch_timer(), Ok(true));
    assert_eq!(apic.dispatch_timer(), Ok(false));
    assert_eq!(apic.pending(), None);
    wait.advance_time_ns(PERIOD * 5).unwrap();
    assert_eq!(apic.dispatch_timer(), Ok(false));
    assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
}

#[test]
fn halt_stop_in_waiting_closes_gate_and_keeps_event_pending() {
    let (apic, mut wait) = halt_rig(LVT_PERIODIC);
    wait.advance_time_ns(PERIOD).unwrap();
    // Due but not yet dispatched when STOP arrives.
    wait.stop();
    assert_eq!(apic.dispatch_timer(), Err(crate::DeviceError::Busy));
    assert_eq!(apic.pending(), None);
    assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Stopped);
    assert_eq!(
        wait.advance_time_ns(1),
        Err(DiagnosticHaltWaitError::NotWaiting)
    );
    assert_eq!(apic.dispatch_timer(), Err(crate::DeviceError::Busy));
    assert!(wait.into_running().is_err());
}

#[test]
fn halt_gate_reopens_after_outside_profile_and_wakes_after_fix() {
    let (apic, mut wait) = halt_rig(LVT_PERIODIC);
    let idtr = wait.session.state.idtr.take();
    wait.advance_time_ns(PERIOD).unwrap();
    assert!(apic.dispatch_timer().unwrap());
    assert!(matches!(
        wait.poll_apic(&apic),
        DiagnosticHaltWakeResult::OutsideProfile(od_isa::InterruptOutsideProfile::MissingIdtr)
    ));
    // The attempt did not acknowledge the vector and reopened the gate.
    assert_eq!(apic.vector_state(VECTOR), (true, false));
    wait.advance_time_ns(PERIOD).unwrap();
    assert_eq!(apic.dispatch_timer(), Ok(true));
    wait.session.state.idtr = idtr;
    assert_eq!(
        wait.poll_apic(&apic),
        DiagnosticHaltWakeResult::Entered { vector: VECTOR }
    );
    let mut session = wait.into_running().ok().expect("entry wakes");
    assert_eq!(session.state.rip, HANDLER);
    session.advance_time_ns(PERIOD).unwrap();
    assert_eq!(apic.dispatch_timer(), Ok(true));
}

#[test]
fn pause_due_resume_delivers_exactly_once() {
    let mut rig = rig(&main_incs(16), LVT_PERIODIC);
    expect_ordinary(&mut rig);
    // Three periods pass, then the session pauses with the event undispatched.
    rig.cpu.advance_time_ns(PERIOD * 3).unwrap();
    rig.cpu.pause().unwrap();
    assert_eq!(rig.apic.dispatch_timer(), Err(crate::DeviceError::Busy));
    assert_eq!(rig.apic.pending(), None);
    assert!(matches!(
        rig.cpu.run_apic_quantum(&rig.apic, || false),
        Ok(DiagnosticApicQuantumResult::ProfileRejected)
    ));
    assert_eq!(
        rig.cpu.advance_time_ns(1),
        Err(DiagnosticVcpuControlError::NotRunnable(
            DiagnosticVcpuState::Paused
        ))
    );
    // Guest state did not move while paused (timer registers are not checked here).
    assert_eq!(rig.cpu.state.rip, MAIN + 3);
    assert_eq!(counter(&mut rig.cpu), 0);
    rig.cpu.resume().unwrap();
    assert!(rig.apic.dispatch_timer().unwrap());
    assert!(!rig.apic.dispatch_timer().unwrap());
    expect_delivered(&mut rig);
    finish_handler(&mut rig.cpu);
    assert_eq!(counter(&mut rig.cpu), 1);
    for _ in 0..4 {
        expect_ordinary(&mut rig);
    }
    assert_eq!(rig.apic.pending(), None);
    assert_eq!(counter(&mut rig.cpu), 1);
    // The next period boundary is the 4th, relative to arm time.
    rig.cpu.advance_time_ns(PERIOD).unwrap();
    assert!(rig.apic.dispatch_timer().unwrap());
    expect_delivered(&mut rig);
    finish_handler(&mut rig.cpu);
    assert_eq!(counter(&mut rig.cpu), 2);
    assert_eq!(rax(&rig.cpu), 5);
}

#[test]
fn tsc_deadline_mode_is_not_available_to_the_guest() {
    let mut rig = rig(&main_incs(4), LVT_PERIODIC);
    let tsc_deadline_lvt = 0x4_0040; // LVT bit 18 selects TSC-deadline mode.
    // Mode bits change only while idle: cancel the countdown first.
    mmio_write(&mut rig.cpu, 0x380, 0).unwrap();
    assert!(mmio_write(&mut rig.cpu, 0x320, tsc_deadline_lvt).is_err());
    assert!(mmio_write(&mut rig.cpu, 0x320, tsc_deadline_lvt | 0x2_0000).is_err());
    // CPUID leaf 1 ECX[24] (TSC-Deadline) is not declared by the profile.
    let leaf1 = od_isa::cpuid::CpuidProfile::DiagnosticV1.query(1, 0);
    assert_eq!(leaf1.ecx & (1 << 24), 0);
}

#[test]
fn clock_operations_do_not_take_the_apic_pair_lock() {
    // Lock order is pair -> clock (dispatch_timer holds both in that order).
    // That order is deadlock-free only if no clock operation ever waits for
    // the pair lock. Hold the pair lock here and require every clock-only
    // operation to complete on another thread.
    let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
    let device = LocalApic::with_shared_timer(clock.clone(), 1_000_000_000).unwrap();
    let apic = device.handle();
    let advancer = clock.claim_advancer().unwrap();
    let pair_guard = apic.0.lock().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        advancer.advance_time_ns(5).unwrap();
        advancer.set_dispatch_allowed(false);
        advancer.set_dispatch_allowed(true);
        assert_eq!(advancer.snapshot().now_ns(), 5);
        assert_eq!(clock.snapshot().now_ns(), 5);
        clock.with_dispatch_guard(|snapshot, allowed| {
            assert_eq!((snapshot.now_ns(), allowed), (5, true));
        });
        drop(advancer);
        sender.send(()).unwrap();
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("clock operation waited for the APIC pair lock");
    drop(pair_guard);
    worker.join().unwrap();
}
