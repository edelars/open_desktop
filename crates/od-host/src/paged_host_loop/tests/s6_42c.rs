//! S6-42c: the guest side of the boundary in the JIT host quantum on the vCPU threads of
//! `ParallelControl`: interrupts of the vCPU's own Local APIC delivered through the guest IDT, guest
//! faults delivered, the split `LOCK` behind the gate of the run, the NMI through the root named for
//! the vCPU.
//!
//! Every guest runs a loop that is native in part (`mov rax,[rbx]` is an admitted form of the
//! session) and interpreted in part (`jmp`, the handlers, the MMIO accesses of the Local APIC), so a
//! delivery lands in a vCPU that executes both. The boards are the boards of `host_parallel_board`
//! (Linux profile, Local APIC windows, clock owner), the tables are 4 KiB pages of one set for all
//! vCPUs. The initial state uses the configured root unless a test supplies its own CR3.

use super::*;
use crate::host_threads::JitHostQuantum;
use crate::jit_region::SharedJitRegion;
use od_isa::integer_flags::{AUXILIARY_CARRY_FLAG, CARRY_FLAG};
use od_isa::state::{Gdtr, Idtr, StiShadow};
use od_isa::{CodeSegmentDescriptorCache, CpuActivity, StackSegmentDescriptorCache};
use od_machine::{
    HostBoardIrqSource, HostParallelConfig, HostParallelFinish, HostQuantumCall, HostSplitGate,
    LoadedFirmwareBoard, host_install_vcpu, host_parallel_board, run_host_threads_with_nmi_paging,
};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

const LOCAL_APIC: u64 = 0xFEE0_0000;
const PML4: u64 = 0x1_0000;
const PDPT: u64 = 0x1_1000;
const PD: u64 = 0x1_2000;
const PT: u64 = 0x1_3000;
const PD_APIC: u64 = 0x1_4000;
const PT_APIC: u64 = 0x1_5000;
const PT_SPLIT: u64 = 0x1_6000;
const CODE_0: u64 = 0x2_0000;
const CODE_1: u64 = 0x2_1000;
const HANDLER_IRQ: u64 = 0x2_3000;
const HANDLER_NMI: u64 = 0x2_4000;
const HANDLER_PF: u64 = 0x2_5000;
const HANDLER_TIMER: u64 = 0x2_6000;
const IDT_AT: u64 = 0x3_0000;
const GDT_AT: u64 = 0x3_1000;
const COUNTER_IRQ: u64 = 0x4_0000;
const COUNTER_NMI: u64 = 0x4_0008;
const COUNTER_PF: u64 = 0x4_0010;
const COUNTER_TIMER: u64 = 0x4_0018;
const DATA: u64 = 0x4_1000;
const STACK_TOP_0: u64 = 0x6_0800;
const STACK_TOP_1: u64 = 0x7_0800;
const MARKER: u64 = 0x0BAD_CAFE_0123_4567;
/// A linear address no table maps (PDPT entry 1 is empty).
const UNMAPPED: u64 = 0x4000_0000;
/// The split operand: a dword that starts two bytes before the end of the page at linear 0x40_0000
/// (two frames far apart), and a cell in the first frame the program raises when it is done.
const SPLIT_LINEAR: u64 = 0x40_0FFE;
const DONE_LINEAR: u64 = 0x40_0100;
const SPLIT_FRAME_0: u64 = 0x10_0000;
const SPLIT_FRAME_1: u64 = 0x30_0000;

const VECTOR_IRQ: u8 = 0x40;
const VECTOR_TIMER: u8 = 0x41;

fn config() -> PagedHostLoopConfig {
    PagedHostLoopConfig {
        cr3: PML4,
        efer: EFER,
        access_context: context(),
    }
}

fn put(board: &mut LoadedFirmwareBoard, at: u64, bytes: &[u8]) {
    board
        .bus_mut()
        .patch_physical_ram(GuestPhysicalAddress::new(at), bytes)
        .expect("the board RAM covers the address");
}

fn put_u64(board: &mut LoadedFirmwareBoard, at: u64, value: u64) {
    put(board, at, &value.to_le_bytes());
}

fn queue_nmi_from_ap(board: &od_machine::MachineBus) {
    for (offset, value) in [(0x310, 0), (0x300, 0x400)] {
        board
            .write_mmio_shared(
                1,
                GuestPhysicalAddress::new(LOCAL_APIC + offset),
                AccessWidth::Dword,
                value,
            )
            .unwrap();
    }
}

// ---- the guest ------------------------------------------------------------------------------

fn imm(opcode: &[u8], value: u32) -> Vec<u8> {
    let mut code = opcode.to_vec();
    code.extend(value.to_le_bytes());
    code
}

fn mov_eax(value: u32) -> Vec<u8> {
    imm(&[0xB8], value)
}

fn mov_ebx(value: u32) -> Vec<u8> {
    imm(&[0xBB], value)
}

/// `MOV [RBX + disp32], EAX`.
fn store_eax(offset: u32) -> Vec<u8> {
    imm(&[0x89, 0x83], offset)
}

/// Software-enable the Local APIC of the running processor (SVR bit 8, spurious vector 0xFF);
/// leaves RBX = the Local APIC base.
fn enable_lapic() -> Vec<u8> {
    [mov_ebx(LOCAL_APIC as u32), mov_eax(0x1FF), store_eax(0xF0)].concat()
}

/// `mov ebx, DATA; again: mov rax,[rbx]; jmp again`: a native load and an interpreted jump.
fn native_loop() -> Vec<u8> {
    [mov_ebx(DATA as u32), vec![0x48, 0x8B, 0x03, 0xEB, 0xFB]].concat()
}

/// `counter += 1` through R8/R9 (a dword read-modify-write).
fn count(counter: u64) -> Vec<u8> {
    [
        imm(&[0x41, 0xB8], counter as u32), // mov r8d, counter
        vec![0x45, 0x8B, 0x08],             // mov r9d, [r8]
        vec![0x41, 0xFF, 0xC1],             // inc r9d
        vec![0x45, 0x89, 0x08],             // mov [r8], r9d
    ]
    .concat()
}

/// The handler of a vector of the Local APIC: count, EOI (R8 the base, R10 zero), `IRETQ`.
fn irq_handler(counter: u64) -> Vec<u8> {
    [
        count(counter),
        imm(&[0x41, 0xB8], LOCAL_APIC as u32),
        vec![0x45, 0x31, 0xD2],         // xor r10d, r10d
        imm(&[0x45, 0x89, 0x90], 0xB0), // mov [r8+0xB0], r10d  (EOI)
        vec![0x48, 0xCF],               // iretq
    ]
    .concat()
}

/// The handler of the NMI: count and `IRETQ`.
fn nmi_handler(counter: u64) -> Vec<u8> {
    [count(counter), vec![0x48, 0xCF]].concat()
}

/// The handler of `#PF`: drop the error code, repair RBX, count, `IRETQ` (the instruction restarts).
fn pf_handler(counter: u64) -> Vec<u8> {
    [
        vec![0x48, 0x83, 0xC4, 0x08], // add rsp, 8
        mov_ebx(DATA as u32),
        count(counter),
        vec![0x48, 0xCF],
    ]
    .concat()
}

/// ICR_HIGH = `destination << 24`, then ICR_LOW = `low` (RBX holds the Local APIC base).
fn write_icr(destination: u32, low: u32) -> Vec<u8> {
    [
        mov_eax(destination << 24),
        store_eax(0x310),
        mov_eax(low),
        store_eax(0x300),
    ]
    .concat()
}

/// The sleeper: `hlt; jmp hlt`.
const SLEEP: [u8; 3] = [0xF4, 0xEB, 0xFD];

// ---- the board --------------------------------------------------------------------------------

fn gate(handler: u64) -> [u8; 16] {
    let mut gate = [0u8; 16];
    gate[0..2].copy_from_slice(&(handler as u16).to_le_bytes());
    gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
    gate[5] = 0x8E; // present, DPL 0, 64-bit interrupt gate
    gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
    gate
}

/// A board of `vcpus` vCPUs with the tables, the GDT, the IDT (vectors 2, 14, 0x40, 0x41) and the
/// handlers; no code yet.
fn board(vcpus: usize) -> LoadedFirmwareBoard {
    with_paged_fixture(host_parallel_board(vcpus).unwrap())
}

fn with_paged_fixture(mut board: LoadedFirmwareBoard) -> LoadedFirmwareBoard {
    let flags = 0x63u64; // present, writable, accessed, dirty
    put_u64(&mut board, PML4, PDPT | flags);
    put_u64(&mut board, PDPT, PD | flags);
    put_u64(&mut board, PD, PT | flags);
    // 0..2 MiB identity, 4 KiB pages.
    for index in 0..512u64 {
        put_u64(&mut board, PT + index * 8, (index << 12) | flags);
    }
    // The Local APIC window: a page of its own (uncached).
    put_u64(&mut board, PDPT + 3 * 8, PD_APIC | flags);
    put_u64(&mut board, PD_APIC + 0x1F7 * 8, PT_APIC | flags);
    put_u64(&mut board, PT_APIC, LOCAL_APIC | flags | 0x10);
    // The split operand: linear 0x40_0000 and 0x40_1000 are two distant frames.
    put_u64(&mut board, PD + 2 * 8, PT_SPLIT | flags);
    put_u64(&mut board, PT_SPLIT, SPLIT_FRAME_0 | flags);
    put_u64(&mut board, PT_SPLIT + 8, SPLIT_FRAME_1 | flags);
    // The GDT: null, 64-bit ring-0 code (0x08), ring-0 data (0x10).
    let mut gdt = [0u8; 24];
    gdt[8..16].copy_from_slice(&0x00AF_9B00_0000_FFFFu64.to_le_bytes());
    gdt[16..24].copy_from_slice(&0x00CF_9300_0000_FFFFu64.to_le_bytes());
    put(&mut board, GDT_AT, &gdt);
    for (vector, handler) in [
        (2u64, HANDLER_NMI),
        (14, HANDLER_PF),
        (u64::from(VECTOR_IRQ), HANDLER_IRQ),
        (u64::from(VECTOR_TIMER), HANDLER_TIMER),
    ] {
        put(&mut board, IDT_AT + vector * 16, &gate(handler));
    }
    put(&mut board, HANDLER_IRQ, &irq_handler(COUNTER_IRQ));
    put(&mut board, HANDLER_TIMER, &irq_handler(COUNTER_TIMER));
    put(&mut board, HANDLER_NMI, &nmi_handler(COUNTER_NMI));
    put(&mut board, HANDLER_PF, &pf_handler(COUNTER_PF));
    put_u64(&mut board, DATA, MARKER);
    board
}

/// A processor in 64-bit mode at CPL0 with IF = 1 on the board's tables: no CR3 of its own (the
/// loop's configuration has the root), CR2 modelled (a `#PF` writes it), the IDT loaded.
fn state(rip: u64, rsp: u64) -> LongModeState {
    let mut state = LongModeState {
        rip,
        rflags: 2 | (1 << 9),
        cr0: Some(CR0),
        cr2: Some(0),
        cr3: None,
        cr4: Some(CR4),
        cs_selector: Some(0x08),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9B,
            flags: 2,
        }),
        ss_selector: Some(0x10),
        ss_cache: Some(StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        }),
        gdtr: Some(Gdtr {
            base: GDT_AT,
            limit: 0x17,
        }),
        idtr: Some(Idtr {
            base: IDT_AT,
            limit: 0xFFF,
        }),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = rsp;
    state
}

fn install(board: &mut LoadedFirmwareBoard, cpu: usize, code: &[u8], at: u64, rsp: u64) {
    put(board, at, code);
    assert!(host_install_vcpu(board, cpu, state(at, rsp)));
}

fn quantum_of(region: &SharedJitRegion, vcpus: usize) -> JitHostQuantum {
    let parts = (0..vcpus)
        .map(|_| Some(region.take_part().unwrap()))
        .collect();
    JitHostQuantum::new(parts, vec![config(); vcpus], RFLAGS_DEFINED).with_cpl0_set()
}

fn host_config(end_when_idle: bool) -> HostParallelConfig {
    HostParallelConfig {
        quantum: 64,
        budget: u64::MAX,
        end_when_idle,
    }
}

fn cell(control: &od_machine::HostParallelControl<'_, '_>, at: u64) -> u64 {
    control
        .bus()
        .atomic_inline_memory()
        .load_u64(GuestPhysicalAddress::new(at))
        .unwrap()
}

// ---- A: interrupts ----------------------------------------------------------------------------

/// The IPI of a Local APIC reaches the guest in a vCPU that runs native and interpreted steps: N
/// vectors posted one after another, each handler runs exactly once (the counter, the ISR and IRR
/// bits, the stack of the interrupted loop are exactly what they were before).
#[test]
fn s6_42c_a_fixed_vector_is_delivered_to_a_guest_that_runs_native_steps() {
    const VECTORS: u64 = 12;
    let _guard = jit_guard();
    let region = SharedJitRegion::new(1, 4).unwrap();
    let quantum = quantum_of(&region, 1);
    let mut board = board(1);
    install(
        &mut board,
        0,
        &[enable_lapic(), native_loop()].concat(),
        CODE_0,
        STACK_TOP_0,
    );
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(false),
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| {
            let lapic = control.lapic(0).unwrap();
            assert!(
                control.wait_until(WAIT, || quantum.stats(0).steps_jit >= 50),
                "the loop does not run natively: {:?} {:?}",
                quantum.stats(0),
                quantum.last_failure()
            );
            for k in 1..=VECTORS {
                lapic.request_fixed_edge(VECTOR_IRQ).unwrap();
                assert!(
                    control.wait_until(WAIT, || cell(control, COUNTER_IRQ) >= k),
                    "vector {k} is not delivered: {:?} {:?}",
                    quantum.stats(0),
                    quantum.last_failure()
                );
                // The EOI is part of the handler: the vector leaves the ISR before the next one.
                assert!(
                    control.wait_until(WAIT, || lapic.vector_state(VECTOR_IRQ) == (false, false)),
                    "vector {k} stays in service"
                );
            }
            // Nothing more arrives: let the loop run on and look again.
            let at = control.retired()[0];
            assert!(control.wait_until(WAIT, || control.retired()[0] >= at + 3000));
            control.stop_all(WAIT).expect("the vCPU acknowledges");
            assert_eq!(cell(control, COUNTER_IRQ), VECTORS, "each handler ran once");
            assert_eq!(lapic.pending(), None);
            let state = control.with_state(0, |state| state.clone()).unwrap();
            assert_eq!(
                state.gpr[Register::Rsp as usize],
                STACK_TOP_0,
                "the stack of the interrupted loop is balanced"
            );
            assert_eq!(state.gpr[Register::Rax as usize], MARKER);
            assert!(state.rflags & (1 << 9) != 0, "IRETQ restored IF");
            control.resume();
        },
    )
    .expect("the board runs");
    assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
    let stats = quantum.stats(0);
    assert_eq!(stats.irqs_delivered, VECTORS, "{stats:?}");
    assert!(
        stats.steps_jit > 0 && stats.steps_interpreter > 0,
        "{stats:?}"
    );
    assert_eq!(stats.exceptions_delivered, 0, "{stats:?}");
    assert_eq!(quantum.last_failure(), None);
    eprintln!(
        "S6-42c IPI: {VECTORS} vectors, native steps {}, interpreter steps {}, calls {}",
        stats.steps_jit, stats.steps_interpreter, stats.calls
    );
    drop(quantum);
    drop(region);
}

#[test]
fn s6_42d_a_pending_irq_waits_for_partial_rep_across_a_stop() {
    let _guard = jit_guard();
    const SOURCE: u64 = 0x5_0000;
    const DEST: u64 = 0x5_1000;
    const WORK_LIMIT: u64 = od_isa::execute::REP_ITERATION_WORK_LIMIT;

    let mut board = board(1);
    put(&mut board, CODE_0, &[0xF3, 0xA4, 0xF4]); // REP MOVSB; HLT
    put(&mut board, SOURCE, &vec![0xA5; (WORK_LIMIT + 1) as usize]);
    let lapic = board.local_apic_of(0).unwrap();
    let bus = board.bus_mut();
    bus.write_mmio_shared(
        0,
        GuestPhysicalAddress::new(LOCAL_APIC + 0xF0),
        AccessWidth::Dword,
        0x1FF,
    )
    .unwrap();
    let mut state = state(CODE_0, STACK_TOP_0);
    state.gpr[Register::Rsi as usize] = SOURCE;
    state.gpr[Register::Rdi as usize] = DEST;
    state.gpr[Register::Rcx as usize] = WORK_LIMIT + 1;
    let control = PagedHostLoopControl::new();
    let set = Cpl0ProfileSet::new(od_isa::execute::ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpu_owned_cr3(od_isa::MovCr3Long64V1);
    let mut session = PagedFetchedLoadSession::new();
    let mut defined = RFLAGS_DEFINED;
    let mut skip_boundary = false;
    let mut run = |state: &mut LongModeState,
                   write: &mut dyn FnMut(od_machine::GuestPhysicalRange)| {
        run_paged_host_loop_shared_set(
            &mut session,
            &control,
            config(),
            state,
            bus,
            0,
            PagedHostSetEnv {
                lapic: &lapic,
                board_irq: HostBoardIrqSource::none(),
                devices: od_machine::HostDeviceServices::none(),
                gate: HostSplitGate::none(),
                set: &set,
                skip_first_boundary: &mut skip_boundary,
            },
            &mut defined,
            1,
            write,
            || false,
        )
    };

    let partial = run(&mut state, &mut |range| {
        if range.start().get() == DEST {
            lapic.request_fixed_edge(VECTOR_IRQ).unwrap();
            control.request_stop();
        }
    });
    assert_eq!(
        partial.exit,
        PagedHostLoopExit::StopRequested,
        "{partial:?}"
    );
    assert_eq!(partial.retired_total(), 0);
    assert_eq!(partial.irqs_delivered, 0);
    assert_eq!(state.rip, CODE_0);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.gpr[Register::Rdi as usize], DEST + WORK_LIMIT);
    assert_eq!(lapic.vector_state(VECTOR_IRQ), (true, false));

    control.clear_stop_request();
    let complete = run(&mut state, &mut |_| {});
    assert_eq!(complete.exit, PagedHostLoopExit::BudgetExhausted);
    assert_eq!(complete.retired_interpreter, 1);
    assert_eq!(complete.irqs_delivered, 0);
    assert_eq!(state.rip, CODE_0 + 2);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(lapic.vector_state(VECTOR_IRQ), (true, false));
    assert_eq!(
        bus.atomic_inline_memory()
            .load_u64(GuestPhysicalAddress::new(DEST + WORK_LIMIT))
            .unwrap(),
        0xA5,
    );

    let handler = run(&mut state, &mut |_| {});
    assert_eq!(handler.exit, PagedHostLoopExit::BudgetExhausted);
    assert_eq!(handler.irqs_delivered, 1);
    assert_eq!(state.rip, HANDLER_IRQ + 6);
    assert_eq!(lapic.vector_state(VECTOR_IRQ), (false, true));
}

#[test]
fn s6_42d_the_pit_pic_source_wakes_the_bsp_and_cuts_guest_pit_writes_short() {
    let _guard = jit_guard();
    const VECTOR_PIT: u8 = 0x30;
    const COUNTER_PIT: u64 = COUNTER_TIMER + 8;
    const BUDGET: u64 = 200;
    let mut board = board(2);
    assert!(board.enable_pit_pic_irq());
    let mut program = mov_ebx(DATA as u32);
    program.extend([0x48, 0x8B, 0x03]); // native MOV RAX, [RBX]
    for (port, value) in [
        (0x20, 0x11),
        (0x21, VECTOR_PIT),
        (0x21, 0x04),
        (0x21, 0x01),
        (0x21, 0xFE),
        (0x43, 0x34),
        (0x40, 10),
        (0x40, 0),
    ] {
        program.extend([0xB0, value, 0xE6, port]); // MOV AL, value; OUT port, AL
    }
    program.push(0xFB); // STI
    program.extend(SLEEP);
    put(&mut board, CODE_0, &program);
    let handler = [count(COUNTER_PIT), vec![0xB0, 0x20, 0xE6, 0x20, 0x48, 0xCF]].concat();
    put(&mut board, HANDLER_TIMER, &handler);
    put(
        &mut board,
        IDT_AT + u64::from(VECTOR_PIT) * 16,
        &gate(HANDLER_TIMER),
    );
    let mut bsp = state(CODE_0, STACK_TOP_0);
    bsp.rflags = 2;
    assert!(host_install_vcpu(&mut board, 0, bsp));
    let mut ap = state(CODE_1, STACK_TOP_1);
    ap.rflags = 2;
    ap.activity = Some(CpuActivity::Halted);
    assert!(host_install_vcpu(&mut board, 1, ap));
    let region = SharedJitRegion::new(2, 4).unwrap();
    let quantum = quantum_of(&region, 2);
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        HostParallelConfig {
            budget: BUDGET,
            ..host_config(true)
        },
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| assert!(control.wait_finished(WAIT)),
    )
    .unwrap();
    assert_eq!(
        report.finished,
        HostParallelFinish::BudgetExhausted,
        "{report:?}, {:?}",
        quantum.last_failure()
    );
    assert_eq!(report.retired, [BUDGET, 0]);
    let stats = quantum.stats(0);
    assert_eq!(stats.retired_jit, 1);
    assert_eq!(stats.device_boundaries, 3, "one boundary per PIT write");
    let handled = board
        .bus_mut()
        .atomic_inline_memory()
        .load_u64(GuestPhysicalAddress::new(COUNTER_PIT))
        .unwrap();
    assert!(handled >= 5, "only {handled} IRQ0 handler entries");
    assert!((handled..=handled + 1).contains(&stats.irqs_delivered));
    assert!(
        board.clock().now_ns() >= (handled - 1) * 8381,
        "successive PIT periods"
    );
}

#[test]
fn s6_42d_cpl0_host_services_com1_and_charges_pit_and_pm_port_ticks() {
    let _guard = jit_guard();
    let mut board = board(1);
    let start_ns = board.clock().now_ns();
    board
        .bus_mut()
        .read_port(od_machine::PortAddress::new(0x61), AccessWidth::Byte)
        .unwrap();
    let program = [
        0x66, 0xBA, 0xF8, 0x03, // MOV DX, COM1
        0xB0, 0x00, 0xEE, // MOV AL, 0; OUT DX, AL
        0xB0, 0x0A, 0xEE, // MOV AL, newline; OUT DX, AL
        0xB0, 0xFF, 0xEE, // MOV AL, 0xff; OUT DX, AL
        0xE4, 0x61, // IN AL, PIT port B
        0x66, 0xBA, 0x08, 0x06, // MOV DX, PM timer
        0xEC, 0xEC, 0xF4, // IN AL, DX twice; HLT
    ];
    install(&mut board, 0, &program, CODE_0, STACK_TOP_0);
    let region = SharedJitRegion::new(1, 4).unwrap();
    let quantum = quantum_of(&region, 1);
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(true),
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| assert!(control.wait_finished(WAIT)),
    )
    .unwrap();
    assert_eq!(
        report.finished,
        HostParallelFinish::AllIdle,
        "{report:?}, {:?}",
        quantum.last_failure()
    );
    assert_eq!(board.com1_transcript(), [0x00, 0x0A, 0xFF]);
    assert_eq!(quantum.stats(0).device_boundaries, 7);
    assert_eq!(board.clock().now_ns() - start_ns, report.retired[0] + 4_000);
}

#[test]
fn s6_42d_port_ticks_from_two_host_workers_are_charged_once() {
    let _guard = jit_guard();
    let mut board = board(2);
    let start_ns = board.clock().now_ns();
    let program = [0xE4, 0x61, 0x66, 0xBA, 0x08, 0x06, 0xEC, 0xF4];
    install(&mut board, 0, &program, CODE_0, STACK_TOP_0);
    install(&mut board, 1, &program, CODE_1, STACK_TOP_1);
    let region = SharedJitRegion::new(2, 4).unwrap();
    let quantum = quantum_of(&region, 2);
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(true),
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| assert!(control.wait_finished(WAIT)),
    )
    .unwrap();
    assert_eq!(
        report.finished,
        HostParallelFinish::AllIdle,
        "{report:?}, {:?}",
        quantum.last_failure()
    );
    assert_eq!(report.retired, [4, 4]);
    assert_eq!(board.clock().now_ns() - start_ns, 4_008);
}

#[test]
fn s6_42d_acpi_stop_retires_the_out_and_preserves_the_board_stop_reason() {
    use od_machine::{
        ACPI_PM_TMR_BLK, ACPI_PM1_CNT_SLP_EN, ACPI_PM1_CNT_SLP_TYP_SHIFT, ACPI_PM1A_CNT_BLK,
        ACPI_RESET_REG, ACPI_RESET_VALUE, ACPI_SLP_TYP_S5, MachineStopReason,
    };

    let _guard = jit_guard();
    let sleep = ACPI_PM1_CNT_SLP_EN | (u16::from(ACPI_SLP_TYP_S5) << ACPI_PM1_CNT_SLP_TYP_SHIFT);
    for (cpu, reason, port, command, budget) in [
        (
            0,
            MachineStopReason::Shutdown,
            ACPI_PM1A_CNT_BLK + 1,
            sleep.to_le_bytes()[1],
            5,
        ),
        (
            1,
            MachineStopReason::Reset,
            ACPI_RESET_REG,
            ACPI_RESET_VALUE,
            100,
        ),
    ] {
        let mut board = board(2);
        let start_ns = board.clock().now_ns();
        let [timer_lo, timer_hi] = ACPI_PM_TMR_BLK.to_le_bytes();
        let [port_lo, port_hi] = port.to_le_bytes();
        let code = [
            0x66, 0xBA, timer_lo, timer_hi, 0xEC, // MOV DX, PM timer; IN AL, DX
            0x66, 0xBA, port_lo, port_hi, // MOV DX, PM stop register
            0xB0, command, 0xEE, // MOV AL, command; OUT DX, AL
            0xCC, // must not be fetched by a subsequent step
        ];
        let code_at = [CODE_0, CODE_1];
        let stack = [STACK_TOP_0, STACK_TOP_1];
        let other = 1 - cpu;
        install(&mut board, cpu, &code, code_at[cpu], stack[cpu]);
        let mut halted = state(code_at[other], stack[other]);
        halted.activity = Some(CpuActivity::Halted);
        halted.rflags = 2;
        assert!(host_install_vcpu(&mut board, other, halted));

        for first_run in [true, false] {
            let region = SharedJitRegion::new(2, 4).unwrap();
            let quantum = quantum_of(&region, 2);
            let (report, ()) = run_host_threads_with_nmi_paging(
                &mut board,
                HostParallelConfig {
                    budget,
                    ..host_config(true)
                },
                &quantum.nmi_paging(),
                &|call| quantum.run(call),
                &|_| quantum.thread_exit(),
                |control| assert!(control.wait_finished(WAIT)),
            )
            .unwrap();
            assert_eq!(
                report.finished,
                HostParallelFinish::GuestStop(reason),
                "{report:?}"
            );
            let mut retired = [0, 0];
            retired[cpu] = if first_run { 5 } else { 0 };
            assert_eq!(report.retired, retired);
            assert_eq!(report.threads_exited, 2);
            assert_eq!(board.acpi_pm().unwrap().stop_request(), Some(reason));
            assert_eq!(board.clock().now_ns() - start_ns, 1_005);
        }
    }
}

const NVME_ASQ: u64 = 0x8_0000;
const NVME_ACQ: u64 = 0x8_1000;
const NVME_IDENTIFY: u64 = 0x8_2000;
const NVME_CID: u16 = 0x42;

fn nvme_host_fixture() -> (LoadedFirmwareBoard, u64) {
    const PT_NVME: u64 = 0x1_C000;
    const PCI_FUNCTION: u64 = od_machine::PCI_ECAM_DESCRIPTOR.base + (4 << 15);
    let mut board = with_paged_fixture(
        od_machine::host_parallel_board_with_nvme(1, od_machine::NvmeConfig::default()).unwrap(),
    );
    let bus = board.bus_mut();
    let read_bar = |bus: &mut od_machine::MachineBus, register| {
        bus.read_physical(
            GuestPhysicalAddress::new(PCI_FUNCTION + register),
            AccessWidth::Dword,
        )
        .unwrap()
    };
    let bar = (read_bar(bus, 0x14) << 32) | (read_bar(bus, 0x10) & !0xF);
    for (addr, width, value) in [
        (PCI_FUNCTION + 4, AccessWidth::Word, 6), // memory decode and bus mastering
        (bar + 0x24, AccessWidth::Dword, 0x000F_000F),
        (bar + 0x28, AccessWidth::Qword, NVME_ASQ),
        (bar + 0x30, AccessWidth::Qword, NVME_ACQ),
        (bar + 0x14, AccessWidth::Dword, 0x0046_0001),
    ] {
        bus.write_physical(GuestPhysicalAddress::new(addr), width, value, |_| {})
            .unwrap();
    }
    let nvme = board.nvme().unwrap();
    assert!(nvme.is_ready());
    let doorbell = bar + 0x1000;
    put_u64(
        &mut board,
        PD_APIC + ((doorbell >> 21) & 0x1ff) * 8,
        PT_NVME | 0x63,
    );
    put_u64(
        &mut board,
        PT_NVME + ((doorbell >> 12) & 0x1ff) * 8,
        doorbell | 0x73,
    );
    let mut sqe = [0; 64];
    sqe[0..4].copy_from_slice(&(0x06u32 | (u32::from(NVME_CID) << 16)).to_le_bytes());
    sqe[24..32].copy_from_slice(&NVME_IDENTIFY.to_le_bytes());
    sqe[40..44].copy_from_slice(&1u32.to_le_bytes()); // Identify Controller
    put(&mut board, NVME_ASQ, &sqe);
    (board, doorbell)
}

#[test]
fn s6_42d_the_host_services_real_nvme_dma_before_the_next_guest_step() {
    let _guard = jit_guard();

    for (budget, stop_at_doorbell) in [(3, false), (u64::MAX, false), (u64::MAX, true)] {
        let (mut board, doorbell) = nvme_host_fixture();
        let nvme = board.nvme().unwrap();
        let generation = board
            .bus_mut()
            .atomic_inline_memory()
            .code_page_generation(GuestPhysicalAddress::new(NVME_IDENTIFY))
            .unwrap();
        let program = [
            mov_eax(1),
            mov_ebx(u32::try_from(doorbell).unwrap()),
            store_eax(0),
            mov_ebx((NVME_IDENTIFY + 4) as u32),
            vec![0x48, 0x8B, 0x03], // native load of DMA bytes
            mov_ebx(DATA as u32),
            vec![0x48, 0x89, 0x03, 0xF4], // publish the observed serial; HLT
        ]
        .concat();
        install(&mut board, 0, &program, CODE_0, STACK_TOP_0);
        let region = SharedJitRegion::new(1, 4).unwrap();
        let quantum = quantum_of(&region, 1).with_write_log();
        let (report, ()) = run_host_threads_with_nmi_paging(
            &mut board,
            HostParallelConfig {
                budget,
                ..host_config(true)
            },
            &quantum.nmi_paging(),
            &|mut call| {
                if !stop_at_doorbell {
                    return quantum.run(call);
                }
                let bus = call.bus;
                let poll = &mut *call.stop;
                let mut poll_with_reset = || {
                    let stopped = poll();
                    if !stopped && nvme.doorbell_pending() {
                        bus.write_port_shared(
                            od_machine::PortAddress::new(od_machine::ACPI_RESET_REG),
                            AccessWidth::Byte,
                            u32::from(od_machine::ACPI_RESET_VALUE),
                        )
                        .unwrap();
                    }
                    stopped
                };
                quantum.run(HostQuantumCall {
                    stop: &mut poll_with_reset,
                    board_irq: call.board_irq.reborrow(),
                    ..call
                })
            },
            &|_| quantum.thread_exit(),
            |control| assert!(control.wait_finished(WAIT)),
        )
        .unwrap();
        let (expected_exit, expected_retired, expected_data) = if stop_at_doorbell {
            (
                HostParallelFinish::GuestStop(od_machine::MachineStopReason::Reset),
                3,
                MARKER,
            )
        } else if budget == 3 {
            (HostParallelFinish::BudgetExhausted, 3, MARKER)
        } else {
            (
                HostParallelFinish::AllIdle,
                8,
                u64::from_le_bytes(*b"OD-NVME-"),
            )
        };
        assert_eq!(
            report.finished,
            expected_exit,
            "{report:?}, {:?}",
            quantum.last_failure()
        );
        assert_eq!(report.retired, [expected_retired]);
        let stats = quantum.stats(0);
        assert_eq!(stats.device_boundaries, 1);
        assert_eq!(stats.retired_jit, if expected_retired == 8 { 2 } else { 0 });
        assert_eq!(nvme.doorbell_pending(), stop_at_doorbell);
        let ram = board.bus_mut().atomic_inline_memory();
        assert_eq!(
            ram.load_u32(GuestPhysicalAddress::new(NVME_ACQ + 12))
                .unwrap(),
            if stop_at_doorbell {
                0
            } else {
                u32::from(NVME_CID) | (1 << 16)
            }
        );
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(NVME_ACQ + 16))
                .unwrap(),
            0
        );
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(DATA)).unwrap(),
            expected_data
        );
        let generation_after = ram
            .code_page_generation(GuestPhysicalAddress::new(NVME_IDENTIFY))
            .unwrap();
        assert_eq!(generation_after > generation, !stop_at_doorbell);
        let writes = quantum.committed_writes(0);
        assert_eq!(writes.contains(&(NVME_IDENTIFY, 4096)), !stop_at_doorbell);
        assert_eq!(writes.contains(&(NVME_ACQ, 16)), !stop_at_doorbell);
        assert_eq!(board.clock().now_ns(), expected_retired);
    }
}

#[test]
fn s6_42d_nvme_intx_reaches_a_guest_handler_and_clears_after_cq_head_and_eoi() {
    let _guard = jit_guard();
    const VECTOR: u8 = 0x51;
    const NVME_PIN: u64 = 16;
    const ACTIVE_LOW_LEVEL: u64 = (1 << 13) | (1 << 15);
    let (mut board, doorbell) = nvme_host_fixture();
    assert!(board.enable_pit_ioapic_irq());
    assert!(board.enable_uart_ioapic_irq4());
    let bus = board.bus_mut();
    let ioapic = od_machine::IO_APIC_MMIO_BASE;
    for (address, value) in [
        (LOCAL_APIC + 0xF0, 0x1FF),
        (ioapic, 0x11 + NVME_PIN * 2),
        (ioapic + 0x10, 0),
        (ioapic, 0x10 + NVME_PIN * 2),
        (ioapic + 0x10, ACTIVE_LOW_LEVEL | u64::from(VECTOR)),
    ] {
        bus.write_mmio_shared(
            0,
            GuestPhysicalAddress::new(address),
            AccessWidth::Dword,
            value,
        )
        .unwrap();
    }
    put(
        &mut board,
        IDT_AT + u64::from(VECTOR) * 16,
        &gate(HANDLER_TIMER),
    );
    let handler = [
        count(COUNTER_IRQ),
        mov_ebx((doorbell + 4) as u32),
        mov_eax(1),
        store_eax(0), // advance the admin completion queue head and deassert INTx
        mov_ebx(LOCAL_APIC as u32),
        mov_eax(0),
        store_eax(0xB0), // LAPIC EOI, which also clears IOAPIC remote IRR
        vec![0x48, 0xCF],
    ]
    .concat();
    put(&mut board, HANDLER_TIMER, &handler);
    let program = [
        mov_eax(1),
        mov_ebx(doorbell as u32),
        store_eax(0),
        vec![0xF4],
    ]
    .concat();
    install(&mut board, 0, &program, CODE_0, STACK_TOP_0);
    let region = SharedJitRegion::new(1, 4).unwrap();
    let quantum = quantum_of(&region, 1);
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        HostParallelConfig {
            budget: 100,
            ..host_config(true)
        },
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| assert!(control.wait_finished(WAIT)),
    )
    .unwrap();
    assert_eq!(
        report.finished,
        HostParallelFinish::AllIdle,
        "{report:?}, {:?}",
        quantum.last_failure()
    );
    assert_eq!(quantum.stats(0).irqs_delivered, 1);
    assert_eq!(quantum.stats(0).device_boundaries, 2);
    assert_eq!(
        board.local_apic_of(0).unwrap().vector_state(VECTOR),
        (false, false)
    );
    let nvme = board.nvme().unwrap();
    assert!(!nvme.doorbell_pending());
    let bus = board.bus_mut();
    let ram = bus.atomic_inline_memory();
    assert_eq!(
        ram.load_u64(GuestPhysicalAddress::new(COUNTER_IRQ))
            .unwrap(),
        1
    );
    assert_eq!(
        ram.load_u32(GuestPhysicalAddress::new(NVME_ACQ + 12))
            .unwrap(),
        u32::from(NVME_CID) | (1 << 16)
    );
    let route = bus
        .read_mmio_shared(
            0,
            GuestPhysicalAddress::new(ioapic + 0x10),
            AccessWidth::Dword,
        )
        .unwrap();
    assert_eq!(route & (1 << 14), 0, "remote IRR");
}

/// The one-shot timer of the vCPU's own Local APIC (the clock owner dispatches it on a charge)
/// fires exactly once and is delivered: the guest programs the timer registers by MMIO writes
/// (each ends the call, `DeviceBoundary`), runs on, and its handler counts the one expiry.
#[test]
fn s6_42c_a_one_shot_timer_is_delivered_once() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(1, 4).unwrap();
    let quantum = quantum_of(&region, 1);
    let mut board = board(1);
    let program = [
        enable_lapic(),
        mov_eax(u32::from(VECTOR_TIMER)), // LVT Timer: unmasked, one-shot
        store_eax(0x320),
        mov_eax(0xB), // divide by 1
        store_eax(0x3E0),
        mov_eax(1000), // initial count: arms the timer
        store_eax(0x380),
        native_loop(),
    ]
    .concat();
    install(&mut board, 0, &program, CODE_0, STACK_TOP_0);
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(false),
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| {
            assert!(
                control.wait_until(WAIT, || cell(control, COUNTER_TIMER) >= 1),
                "the timer is not delivered: {:?} {:?}",
                quantum.stats(0),
                quantum.last_failure()
            );
            // A one-shot timer fires once: run well past the expiry and look again.
            let at = control.retired()[0];
            assert!(control.wait_until(WAIT, || control.retired()[0] >= at + 20_000));
            control.stop_all(WAIT).expect("the vCPU acknowledges");
            assert_eq!(cell(control, COUNTER_TIMER), 1, "one expiry, one handler");
            let lapic = control.lapic(0).unwrap();
            assert_eq!(lapic.vector_state(VECTOR_TIMER), (false, false));
            assert_eq!(
                lapic.next_timer_deadline(),
                None,
                "a one-shot is not re-armed"
            );
            let state = control.with_state(0, |state| state.clone()).unwrap();
            assert_eq!(state.gpr[Register::Rsp as usize], STACK_TOP_0);
            control.resume();
        },
    )
    .expect("the board runs");
    assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
    let stats = quantum.stats(0);
    assert_eq!(stats.irqs_delivered, 1, "{stats:?}");
    assert_eq!(
        stats.device_boundaries, 3,
        "the three timer-register writes each end a call: {stats:?}"
    );
    assert!(stats.steps_jit > 0, "{stats:?}");
    assert_eq!(quantum.last_failure(), None);
    drop(quantum);
    drop(region);
}

/// A guest `#PF` of the native path (the walk of a `mov rax,[rbx]` the tables do not map) is
/// delivered through the guest IDT: the native step retires nothing, the interpreter step takes the
/// instruction again, raises the fault and delivers it; the handler repairs RBX and `IRETQ`
/// restarts the load, which then runs natively. Without the set (the base quantum) the same fault
/// is an error of the run.
#[test]
fn s6_42c_a_guest_page_fault_is_delivered_through_the_guest_idt() {
    let _guard = jit_guard();
    let program = [mov_ebx(UNMAPPED as u32), vec![0x48, 0x8B, 0x03, 0xEB, 0xFB]].concat();
    {
        let region = SharedJitRegion::new(1, 4).unwrap();
        let quantum = quantum_of(&region, 1);
        let mut board = board(1);
        install(&mut board, 0, &program, CODE_0, STACK_TOP_0);
        let (report, ()) = run_host_threads_with_nmi_paging(
            &mut board,
            host_config(false),
            &quantum.nmi_paging(),
            &|call| quantum.run(call),
            &|_| quantum.thread_exit(),
            |control| {
                assert!(
                    control.wait_until(WAIT, || cell(control, COUNTER_PF) >= 1
                        && quantum.stats(0).steps_jit >= 100),
                    "the fault is not delivered: {:?} {:?}",
                    quantum.stats(0),
                    quantum.last_failure()
                );
                control.stop_all(WAIT).expect("the vCPU acknowledges");
                assert_eq!(cell(control, COUNTER_PF), 1);
                let state = control.with_state(0, |state| state.clone()).unwrap();
                assert_eq!(
                    state.gpr[Register::Rsp as usize],
                    STACK_TOP_0,
                    "error code dropped"
                );
                assert_eq!(
                    state.gpr[Register::Rax as usize],
                    MARKER,
                    "the load was restarted"
                );
                assert_eq!(state.cr2, Some(UNMAPPED), "CR2 holds the faulting address");
                control.resume();
            },
        )
        .expect("the board runs");
        assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
        let stats = quantum.stats(0);
        assert_eq!(stats.exceptions_delivered, 1, "{stats:?}");
        assert_eq!(stats.jit_faults_delivered, 1, "{stats:?}");
        assert_eq!(quantum.last_failure(), None);
        drop(quantum);
        drop(region);
    }
    // The base quantum: the guest fault ends the run (no delivery in that profile).
    {
        let region = SharedJitRegion::new(1, 4).unwrap();
        let parts = vec![Some(region.take_part().unwrap())];
        let quantum = JitHostQuantum::new(parts, vec![config()], RFLAGS_DEFINED);
        let mut board = board(1);
        let mut base_state = state(CODE_0, STACK_TOP_0);
        // The base profile admits no IDT/GDT/CR2 modelling states of the set; keep what it needs.
        base_state.idtr = None;
        base_state.gdtr = None;
        base_state.cr2 = None;
        base_state.cs_selector = None;
        base_state.cs_cache = None;
        base_state.ss_selector = None;
        base_state.ss_cache = None;
        // The base profile has no `mov ebx, imm32`: RBX is set in the state, the program is the
        // faulting load alone.
        base_state.gpr[Register::Rbx as usize] = UNMAPPED;
        put(&mut board, CODE_0, &[0x48, 0x8B, 0x03]);
        assert!(host_install_vcpu(&mut board, 0, base_state));
        let (report, ()) = run_host_threads_with_nmi_paging(
            &mut board,
            host_config(false),
            &quantum.nmi_paging(),
            &|call| quantum.run(call),
            &|_| quantum.thread_exit(),
            |control| {
                assert!(
                    control.wait_finished(WAIT),
                    "the run does not end on the fault"
                );
            },
        )
        .expect("the board runs");
        assert!(
            matches!(report.finished, HostParallelFinish::Error { cpu: 0, .. }),
            "{report:?}"
        );
        assert!(
            quantum
                .last_failure()
                .is_some_and(|text| text.contains("14")),
            "{:?}",
            quantum.last_failure()
        );
        drop(quantum);
        drop(region);
    }
}

// ---- B: the split LOCK ------------------------------------------------------------------------

fn atomic_host_step(
    session: &mut PagedFetchedLoadSession,
    board: &mut LoadedFirmwareBoard,
    state: &mut LongModeState,
    defined: &mut u64,
    before_step: impl FnMut(&PagedHostLoopControl, u64),
) -> PagedHostLoopReport {
    let lapic = board.local_apic_of(0).unwrap();
    let set = Cpl0ProfileSet::new(od_isa::execute::ControlRegisterPolicy::Cpl0BaseLong64V1);
    run_paged_host_loop_core(
        session,
        &PagedHostLoopControl::new(),
        config(),
        state,
        LoopBusRef::SharedSet(
            board.bus_mut(),
            0,
            PagedHostSetEnv {
                lapic: &lapic,
                board_irq: HostBoardIrqSource::none(),
                devices: od_machine::HostDeviceServices::none(),
                gate: HostSplitGate::none(),
                set: &set,
                skip_first_boundary: &mut false,
            },
        ),
        defined,
        1,
        |_| {},
        before_step,
        || false,
        None,
    )
}

#[test]
fn s6_42d_parallel_host_quantum_without_a_part_executes_in_the_interpreter() {
    let _guard = jit_guard();
    let mut board = board(1);
    put(
        &mut board,
        CODE_0,
        &[0x48, 0x8B, 0x03, 0xF0, 0xFF, 0x03, 0xF4],
    );
    put_u64(&mut board, DATA, 0);
    let mut state = state(CODE_0, STACK_TOP_0);
    state.gpr[Register::Rbx as usize] = DATA;
    assert!(host_install_vcpu(&mut board, 0, state));
    let quantum = JitHostQuantum::new(vec![None], vec![config()], RFLAGS_DEFINED).with_cpl0_set();
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(true),
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| assert!(control.wait_finished(WAIT)),
    )
    .unwrap();
    assert_eq!(report.finished, HostParallelFinish::AllIdle, "{report:?}");
    assert_eq!(report.retired, [3]);
    let stats = quantum.stats(0);
    assert_eq!(
        (
            stats.admissions,
            stats.retired_jit,
            stats.retired_interpreter
        ),
        (0, 0, 3)
    );
    assert_eq!(
        board
            .bus_mut()
            .atomic_inline_memory()
            .load_u64(GuestPhysicalAddress::new(DATA))
            .unwrap(),
        1
    );
}

#[test]
fn s6_42d_parallel_host_loop_releases_owned_blocks_after_the_mode_changes() {
    let _guard = jit_guard();
    for (code, atomic) in [([0x48, 0x8B, 0x03], false), ([0xF0, 0xFF, 0x03], true)] {
        let mut board = board(1);
        put(&mut board, CODE_0, &code);
        put_u64(&mut board, DATA, 0);
        let mut state = state(CODE_0, STACK_TOP_0);
        state.gpr[Register::Rbx as usize] = DATA;
        let mut session = PagedFetchedLoadSession::new();
        let warm = atomic_host_step(&mut session, &mut board, &mut state, &mut 0, |_, _| {});
        assert_eq!(warm.retired_jit, 1, "{warm:?}");
        assert_eq!(warm.atomic_jit_steps, u64::from(atomic));
        assert_eq!(session.admissions(), 1);

        board.set_execution_mode(od_machine::ExecutionMode::Parallel);
        state.rip = CODE_0;
        let parallel = atomic_host_step(&mut session, &mut board, &mut state, &mut 0, |_, _| {});
        assert_eq!(
            parallel.exit,
            PagedHostLoopExit::BudgetExhausted,
            "{parallel:?}"
        );
        assert_eq!((parallel.retired_jit, parallel.retired_interpreter), (0, 1));
        assert_eq!(session.admissions(), 1);
        assert_eq!(session.resets(), 1);
        assert_eq!(
            board
                .bus_mut()
                .atomic_inline_memory()
                .load_u64(GuestPhysicalAddress::new(DATA))
                .unwrap(),
            if atomic { 2 } else { 0 }
        );
    }
}

#[test]
fn s6_42d_atomic_steps_share_the_budget_and_definedness_with_the_interpreter() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(2, 4).unwrap();
    for native in [true, false] {
        let mut board = board(1);
        put(
            &mut board,
            CODE_0,
            &[
                0xf0, 0xff, 0x03, // LOCK INC dword [RBX]
                0xf0, 0x83, 0x0b, 2, // LOCK OR dword [RBX],2
                0x0f, 0xae, 0xf0, // MFENCE
            ],
        );
        put_u64(&mut board, DATA, 0);
        let mut state = state(CODE_0, STACK_TOP_0);
        state.gpr[Register::Rbx as usize] = DATA;
        state.rflags |= RFLAGS_RF;
        let mut defined = 0;
        let mut session = PagedFetchedLoadSession::with_region_part(region.take_part().unwrap());
        if !native {
            session.set_atomic_mode_policy(Some(|_| false));
        }
        for (offset, value, cf, af) in [
            (3, 1, false, true),
            (7, 3, true, false),
            (10, 3, true, false),
        ] {
            let report = atomic_host_step(
                &mut session,
                &mut board,
                &mut state,
                &mut defined,
                |_, _| {},
            );
            assert_eq!(
                report.exit,
                PagedHostLoopExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.atomic_jit_steps, u64::from(native), "{report:?}");
            assert_eq!(report.retired_interpreter, u64::from(!native), "{report:?}");
            assert_eq!(state.rip, CODE_0 + offset);
            assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
            assert_eq!(state.rflags & RFLAGS_RF, 0);
            assert_eq!(defined & CARRY_FLAG != 0, cf);
            assert_eq!(defined & AUXILIARY_CARRY_FLAG != 0, af);
            let actual = board
                .bus_mut()
                .atomic_inline_memory()
                .load_u64(GuestPhysicalAddress::new(DATA))
                .unwrap();
            assert_eq!(actual, value);
        }
    }
}

#[test]
fn s6_42d_atomic_step_preserves_the_cpl0_sti_shadow_refusal() {
    let _guard = jit_guard();
    let mut board = board(1);
    put(&mut board, CODE_0, &[0xf0, 0xff, 0x03]);
    let mut state = state(CODE_0, STACK_TOP_0);
    state.gpr[Register::Rbx as usize] = DATA;
    state.sti_shadow = Some(StiShadow::Blocking);
    let before = state.clone();
    let mut session = PagedFetchedLoadSession::new();
    let report = atomic_host_step(&mut session, &mut board, &mut state, &mut 0, |_, _| {});
    assert!(matches!(report.exit, PagedHostLoopExit::Interpreter(_)));
    assert_eq!(report.remaining_budget, 1);
    assert_eq!(report.atomic_jit_steps, 0);
    assert_eq!(state, before);
    assert_eq!(
        board
            .bus_mut()
            .atomic_inline_memory()
            .load_u64(GuestPhysicalAddress::new(DATA))
            .unwrap(),
        MARKER,
    );
}

#[test]
fn s6_42d_atomic_stop_before_entry_keeps_the_instruction_unretired() {
    let _guard = jit_guard();
    let mut board = board(1);
    put(&mut board, CODE_0, &[0xf0, 0xff, 0x03]);
    let mut state = state(CODE_0, STACK_TOP_0);
    state.gpr[Register::Rbx as usize] = DATA;
    let before = state.clone();
    let mut session = PagedFetchedLoadSession::new();
    let report = atomic_host_step(
        &mut session,
        &mut board,
        &mut state,
        &mut 0,
        |control, _| {
            control.request_stop();
        },
    );
    assert_eq!(report.exit, PagedHostLoopExit::StopRequested, "{report:?}");
    assert_eq!(report.native_stop_exits, 1);
    assert_eq!(report.retired_total(), 0);
    assert_eq!(state, before);
    assert_eq!(
        board
            .bus_mut()
            .atomic_inline_memory()
            .load_u64(GuestPhysicalAddress::new(DATA))
            .unwrap(),
        MARKER,
    );
}

#[test]
fn s6_42d_native_atomic_fault_reaches_the_guest_idt_and_restarts_once() {
    let _guard = jit_guard();
    for (vector, address) in [(14, UNMAPPED), (13, 0x0000_8000_0000_0000)] {
        let region = SharedJitRegion::new(1, 4).unwrap();
        let quantum = quantum_of(&region, 1);
        let mut board = board(1);
        put_u64(&mut board, DATA, 0);
        if vector == 13 {
            put(&mut board, IDT_AT + 14 * 16, &[0; 16]);
            put(&mut board, IDT_AT + 13 * 16, &gate(HANDLER_PF));
        }
        put(&mut board, CODE_0, &[0xf0, 0xff, 0x03, 0xf4]);
        let mut cpu = state(CODE_0, STACK_TOP_0);
        cpu.gpr[Register::Rbx as usize] = address;
        assert!(host_install_vcpu(&mut board, 0, cpu));
        let (report, ()) = run_host_threads_with_nmi_paging(
            &mut board,
            host_config(true),
            &quantum.nmi_paging(),
            &|call| quantum.run(call),
            &|_| quantum.thread_exit(),
            |control| {
                assert!(control.wait_finished(WAIT));
                assert_eq!(cell(control, COUNTER_PF), 1, "vector {vector}");
                assert_eq!(cell(control, DATA), 1, "vector {vector}");
            },
        )
        .unwrap();
        assert_eq!(report.finished, HostParallelFinish::AllIdle, "{report:?}");
        let stats = quantum.stats(0);
        assert_eq!(stats.jit_faults_delivered, 1, "{stats:?}");
        assert_eq!(stats.atomic_jit_steps, 1, "{stats:?}");
    }
}

/// `mov ebx, SPLIT; mov ecx, repeat; again: lock inc dword [rbx]; dec ecx; jnz again;
/// mov ebx, DONE; lock inc dword [rbx]; hlt`.
fn lock_inc_program(repeat: u32) -> Vec<u8> {
    [
        mov_ebx(SPLIT_LINEAR as u32),
        imm(&[0xB9], repeat),
        vec![0xF0, 0xFF, 0x03], // again: lock inc dword [rbx]
        vec![0xFF, 0xC9],       // dec ecx
        vec![0x75, 0xF9],       // jnz again
        mov_ebx(DONE_LINEAR as u32),
        vec![0xF0, 0xFF, 0x03], // lock inc dword [rbx]
        vec![0xF4],             // hlt
    ]
    .concat()
}

fn word(board: &mut LoadedFirmwareBoard, at: u64) -> u16 {
    board
        .bus_mut()
        .read_physical(GuestPhysicalAddress::new(at), AccessWidth::Word)
        .unwrap() as u16
}

fn split_operand(board: &mut LoadedFirmwareBoard) -> u32 {
    let low = word(board, SPLIT_FRAME_0 + 0xFFE);
    let high = word(board, SPLIT_FRAME_1);
    u32::from(low) | (u32::from(high) << 16)
}

/// Two vCPU threads of the host quantum `LOCK INC` a dword that crosses a page 400 times each: the
/// count is exact, every operation went through the gate (`split_locks` counts them), the
/// interpreter step took the split operand instead of refusing it. Control: the same run with the
/// gate withheld (`HostSplitGate::none()`, what the host run did before S6-42c) is an error of the
/// run (the refusal of a locked operand that is not naturally aligned, `MisalignedAtomic`).
#[test]
fn s6_42c_a_guest_split_lock_passes_the_gate_of_the_host_quantum() {
    const REPEAT: u32 = 400;
    let _guard = jit_guard();
    // Through the gate.
    for round in 0..3 {
        let region = SharedJitRegion::new(2, 4).unwrap();
        let quantum = quantum_of(&region, 2);
        let mut board = board(2);
        put(&mut board, CODE_0, &lock_inc_program(REPEAT));
        for (cpu, top) in [(0, STACK_TOP_0), (1, STACK_TOP_1)] {
            assert!(host_install_vcpu(&mut board, cpu, state(CODE_0, top)));
        }
        let (report, ()) = run_host_threads_with_nmi_paging(
            &mut board,
            host_config(true),
            &quantum.nmi_paging(),
            &|call| quantum.run(call),
            &|_| quantum.thread_exit(),
            |control| {
                assert!(control.wait_finished(WAIT), "the counters do not finish");
            },
        )
        .expect("the board runs");
        assert_eq!(report.finished, HostParallelFinish::AllIdle, "{report:?}");
        assert_eq!(report.threads_exited, 2);
        assert_eq!(
            split_operand(&mut board),
            2 * REPEAT,
            "round {round}: the split counter lost an update ({report:?}, {:?})",
            quantum.last_failure()
        );
        assert_eq!(report.split_locks, vec![u64::from(REPEAT); 2], "{report:?}");
        let done = board
            .bus_mut()
            .read_physical(
                GuestPhysicalAddress::new(SPLIT_FRAME_0 + 0x100),
                AccessWidth::Dword,
            )
            .unwrap();
        assert_eq!(done, 2);
        for cpu in 0..2 {
            assert_eq!(quantum.stats(cpu).atomic_jit_steps, 1);
        }
        assert_eq!(quantum.last_failure(), None);
        drop(quantum);
        drop(region);
    }
    // Control: no gate, the split operand is refused and the run fails.
    {
        let region = SharedJitRegion::new(2, 4).unwrap();
        let quantum = quantum_of(&region, 2);
        let mut board = board(2);
        put(&mut board, CODE_0, &lock_inc_program(REPEAT));
        for (cpu, top) in [(0, STACK_TOP_0), (1, STACK_TOP_1)] {
            assert!(host_install_vcpu(&mut board, cpu, state(CODE_0, top)));
        }
        let (report, ()) = run_host_threads_with_nmi_paging(
            &mut board,
            host_config(true),
            &quantum.nmi_paging(),
            &|call| {
                quantum.run(HostQuantumCall {
                    gate: HostSplitGate::none(),
                    ..call
                })
            },
            &|_| quantum.thread_exit(),
            |control| {
                assert!(control.wait_finished(WAIT), "the run does not end");
            },
        )
        .expect("the board runs");
        assert!(
            matches!(report.finished, HostParallelFinish::Error { .. }),
            "{report:?}"
        );
        assert!(
            quantum
                .last_failure()
                .is_some_and(|text| text.contains("not naturally aligned")),
            "{:?}",
            quantum.last_failure()
        );
        assert_eq!(report.split_locks, vec![0, 0]);
        drop(quantum);
        drop(region);
    }
}

// ---- C: the NMI -------------------------------------------------------------------------------

#[test]
fn s6_42d_mov_cr3_with_no_flush_bit_faults_without_changing_the_root() {
    let _guard = jit_guard();
    let mut board = board(1);
    put(&mut board, CODE_0, &[0x0F, 0x22, 0xD8]);
    let lapic = board.local_apic_of(0).unwrap();
    let set = Cpl0ProfileSet::new(od_isa::execute::ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpu_owned_cr3(od_isa::MovCr3Long64V1);
    let old_cr3 = PML4 | 0x08;
    let mut state = state(CODE_0, STACK_TOP_0);
    state.cr3 = Some(old_cr3);
    state.idtr = None;
    state.gpr[Register::Rax as usize] = old_cr3 | (1 << 63);
    let before = state.clone();
    let report = run_paged_host_loop_shared_set(
        &mut PagedFetchedLoadSession::new(),
        &PagedHostLoopControl::new(),
        config(),
        &mut state,
        board.bus_mut(),
        0,
        PagedHostSetEnv {
            lapic: &lapic,
            board_irq: HostBoardIrqSource::none(),
            devices: od_machine::HostDeviceServices::none(),
            gate: HostSplitGate::none(),
            set: &set,
            skip_first_boundary: &mut false,
        },
        &mut 0,
        1,
        |_| {},
        || false,
    );
    assert_eq!(
        report.exit,
        PagedHostLoopExit::GuestFault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        }),
    );
    assert_eq!(report.remaining_budget, 1);
    assert_eq!(state, before);
}

#[test]
fn s6_42d_mov_cr3_changes_native_code_data_and_the_worker_nmi_root() {
    let _guard = jit_guard();
    const NEW_PML4: u64 = 0x1_7000;
    const NEW_PDPT: u64 = 0x1_8000;
    const NEW_PD: u64 = 0x1_9000;
    const NEW_PT: u64 = 0x1_A000;
    const NEW_CODE: u64 = 0x2_7000;
    const NEW_IDT: u64 = 0x3_2000;
    const NEW_DATA: u64 = 0x5_1000;
    const NEW_CR3: u64 = NEW_PML4 | 0x18;
    const NEW_MARKER: u64 = 0x1234_5678;
    const FLAGS: u64 = 0x63;

    let mut board = board(2);
    put_u64(&mut board, NEW_PML4, NEW_PDPT | FLAGS);
    put_u64(&mut board, NEW_PDPT, NEW_PD | FLAGS);
    put_u64(&mut board, NEW_PD, NEW_PT | FLAGS);
    for index in 0..512 {
        let linear = index << 12;
        let frame = match linear {
            CODE_0 => NEW_CODE,
            IDT_AT => NEW_IDT,
            DATA => NEW_DATA,
            _ => linear,
        };
        put_u64(&mut board, NEW_PT + index * 8, frame | FLAGS);
    }
    put_u64(&mut board, NEW_DATA, NEW_MARKER);
    put(&mut board, IDT_AT + 2 * 16, &[0; 16]);
    put(&mut board, NEW_IDT + 2 * 16, &gate(HANDLER_NMI));
    put(&mut board, CODE_0, &[0x0F, 0x22, 0xD8, 0xCC]);
    put(
        &mut board,
        NEW_CODE,
        &[
            0xCC, 0xCC, 0xCC, // only the old root can fetch the first MOV CR3
            0x0F, 0x22, 0xD8, // MOV CR3, RAX again, with the same raw value
            0x48, 0x8B, 0x03, // MOV RAX, [RBX]
            0xF0, 0xFF, 0x03, // LOCK INC dword [RBX]
            0xF4, 0xEB, 0xFD, // HLT; JMP HLT (resume here after IRETQ)
        ],
    );
    let mut bsp = state(CODE_0, STACK_TOP_0);
    bsp.gpr[Register::Rax as usize] = NEW_CR3;
    bsp.gpr[Register::Rbx as usize] = DATA;
    assert!(host_install_vcpu(&mut board, 0, bsp));
    let region = SharedJitRegion::new(2, 4).unwrap();
    let quantum = quantum_of(&region, 2);
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(false),
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| {
            assert!(
                control.wait_until(WAIT, || control.retired()[0] == 5),
                "{:?}",
                quantum.last_failure()
            );
            let stats = quantum.stats(0);
            assert_eq!(stats.retired_jit, 2);
            assert_eq!(stats.atomic_jit_steps, 1);
            assert_eq!(cell(control, DATA), MARKER);
            assert_eq!(cell(control, NEW_DATA), NEW_MARKER + 1);
            queue_nmi_from_ap(control.bus());
            assert!(control.wait_until(WAIT, || cell(control, COUNTER_NMI) == 1));
            assert!(control.wait_until(WAIT, || control.retired()[0] >= 12));
            control.stop_all(WAIT).unwrap();
            control
                .with_state(0, |state| {
                    assert_eq!(state.cr3, Some(NEW_CR3));
                    assert_eq!(state.gpr[Register::Rax as usize], NEW_MARKER);
                    assert_eq!(state.gpr[Register::Rsp as usize], STACK_TOP_0);
                    assert!(!state.nmi_blocked);
                })
                .unwrap();
            control.resume();
        },
    )
    .unwrap();
    assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
    assert_eq!(report.nmi_delivered, [1, 0]);
    assert_eq!(quantum.last_failure(), None);
}

/// vCPU 1 sends an NMI to vCPU 0 through the ICR; vCPU 0 (native and interpreted steps) takes it at
/// a boundary through its initial root and configured EFER, and the handler of vector 2 runs once.
fn nmi_board() -> LoadedFirmwareBoard {
    let mut board = board(2);
    // The IDT is data. Its NX leaf is legal only when EFER.NXE is set.
    put_u64(
        &mut board,
        PT + (IDT_AT >> 12) * 8,
        IDT_AT | 0x63 | (1 << 63),
    );
    let sender = [
        enable_lapic(),
        write_icr(0, 0x0400), // NMI delivery mode, destination vCPU 0
        SLEEP.to_vec(),
    ]
    .concat();
    install(
        &mut board,
        0,
        &[enable_lapic(), native_loop()].concat(),
        CODE_0,
        STACK_TOP_0,
    );
    install(&mut board, 1, &sender, CODE_1, STACK_TOP_1);
    board
}

#[test]
fn s6_42c_an_nmi_is_delivered_through_the_root_named_for_the_vcpu() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(2, 4).unwrap();
    let quantum = quantum_of(&region, 2);
    let mut board = nmi_board();
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(false),
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| {
            assert!(
                control.wait_until(WAIT, || cell(control, COUNTER_NMI) >= 1),
                "the NMI is not delivered: {:?} {:?}",
                quantum.stats(0),
                quantum.last_failure()
            );
            let at = control.retired()[0];
            assert!(control.wait_until(WAIT, || control.retired()[0] >= at + 3000));
            control.stop_all(WAIT).expect("both vCPUs acknowledge");
            assert_eq!(cell(control, COUNTER_NMI), 1, "the handler ran once");
            let state = control.with_state(0, |state| state.clone()).unwrap();
            assert_eq!(state.gpr[Register::Rsp as usize], STACK_TOP_0);
            assert!(!state.nmi_blocked, "IRETQ unblocked the NMI");
            assert_eq!(state.cr3, Some(PML4));
            control.resume();
        },
    )
    .expect("the board runs");
    assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
    assert_eq!(report.nmi_delivered, vec![1, 0], "{report:?}");
    assert_eq!(quantum.last_failure(), None);
    drop(quantum);
    drop(region);
}

#[test]
fn s6_42d_a_native_stop_for_an_nmi_returns_to_the_worker_boundary() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(2, 4).unwrap();
    let quantum = quantum_of(&region, 2);
    let mut board = board(2);
    put(&mut board, CODE_0, &[0x48, 0x89, 0x03, 0xf4]); // mov [rbx], rax; hlt
    let mut bsp = state(CODE_0, STACK_TOP_0);
    bsp.gpr[Register::Rbx as usize] = DATA;
    assert!(host_install_vcpu(&mut board, 0, bsp));
    let at_poll = AtomicBool::new(false);

    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(false),
        &quantum.nmi_paging(),
        &|call| {
            let first = !at_poll.load(Ordering::SeqCst);
            let result = run_after_stop_is_posted(&quantum, call, &at_poll, WAIT)?;
            if first && at_poll.load(Ordering::SeqCst) {
                assert_eq!(result.retired, 0, "STOP must precede the guest store");
            }
            Ok(result)
        },
        &|_| quantum.thread_exit(),
        |control| {
            assert!(control.wait_until(WAIT, || at_poll.load(Ordering::SeqCst)));
            // Use vCPU 1's ICR to enqueue an NMI for vCPU 0 during the delayed poll.
            queue_nmi_from_ap(control.bus());
            assert!(control.wait_until(WAIT, || {
                quantum.stats(0).retired_jit == 1
                    && cell(control, COUNTER_NMI) == 1
                    && cell(control, DATA) == 0
            }));
        },
    )
    .unwrap();
    assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
    assert_eq!(report.nmi_delivered, [1, 0]);
    assert_eq!(quantum.stats(0).native_stop_exits, 1);
    assert_eq!(quantum.stats(0).retired_jit, 1);
}

#[test]
fn s6_42d_an_nmi_uses_the_host_quantums_efer_for_the_idt_walk() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(2, 4).unwrap();
    let quantum = JitHostQuantum::new(
        vec![
            Some(region.take_part().unwrap()),
            Some(region.take_part().unwrap()),
        ],
        vec![
            PagedHostLoopConfig {
                efer: EFER & !(1 << 11),
                ..config()
            },
            config(),
        ],
        RFLAGS_DEFINED,
    )
    .with_cpl0_set();
    let mut board = nmi_board();
    let (report, handler_ran) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(false),
        &quantum.nmi_paging(),
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| control.wait_until(WAIT, || cell(control, COUNTER_NMI) != 0),
    )
    .expect("the board runs");
    assert!(
        !handler_ran,
        "NX is reserved when the host quantum clears NXE"
    );
    assert_eq!(report.nmi_delivered, vec![0, 0], "{report:?}");
    match &report.finished {
        HostParallelFinish::Error { cpu: 0, message } => {
            // P=1 and RSVD=1 identify the reserved-bit page fault at the IDT gate.
            let gate = IDT_AT + 2 * 16;
            assert!(message.contains("phase: IdtGateRead"), "{message}");
            assert!(
                message.contains(&format!(
                    "vector: 14, error_code: Some(9), linear_address: Some({gate})"
                )),
                "{message}"
            );
        }
        other => panic!("expected an NMI page fault reading the IDT, got {other:?}"),
    }
}

/// Without a root named for the vCPU the NMI is an explicit error of the run (the board's own root,
/// 0x1000, is not read): the run ends `Error` naming the missing root, and the vCPU's handler never
/// ran.
#[test]
fn s6_42c_an_nmi_without_a_root_fails_the_run_explicitly() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(2, 4).unwrap();
    let quantum = quantum_of(&region, 2);
    let mut board = nmi_board();
    queue_nmi_from_ap(board.bus_mut());
    let (report, ()) = run_host_threads_with_nmi_paging(
        &mut board,
        host_config(false),
        &[None, None],
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| {
            assert!(
                control.wait_finished(WAIT),
                "the run does not end on the NMI"
            );
        },
    )
    .expect("the board runs");
    match &report.finished {
        HostParallelFinish::Error { cpu: 0, message } => {
            assert!(message.contains("no page-table root"), "{message}");
        }
        other => panic!("expected the NMI error of vCPU 0, got {other:?} ({report:?})"),
    }
    assert_eq!(report.nmi_delivered, vec![0, 0], "{report:?}");
    assert_eq!(report.threads_exited, 2, "every thread is joined");
    assert_eq!(
        board
            .bus_mut()
            .read_physical(GuestPhysicalAddress::new(COUNTER_NMI), AccessWidth::Qword)
            .unwrap(),
        0,
        "the handler never ran"
    );
    drop(quantum);
    drop(region);
}
