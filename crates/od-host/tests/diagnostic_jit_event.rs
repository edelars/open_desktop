use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use od_host::{PublishedBlockExecutionError, PublishedBlockExecutor};
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{INTEGER_FLAGS, LongModeState};
use od_jit::dispatch::{
    BlockExecutionReport, ControlledBlockExecution, ControlledBlockExecutor, DispatchControl,
    DispatchExit,
};
use od_machine::{
    AccessWidth, DiagnosticApicQuantumResult, DiagnosticIretqResult, DiagnosticVcpuSession,
    GuestPhysicalAddress, GuestPhysicalRange, LOCAL_APIC_MMIO_BASE, LocalApic, LocalApicHandle,
    MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
};

const CODE_START: u64 = 0x1000;
static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());
const RAM_LEN: u64 = 0x1000;
const CODE: [u8; 23] = [
    0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0, // MOV RAX, 1
    0x75, 0x00, // JNZ to the next instruction
    0x90, // NOP fallback
    0x48, 0xbb, 2, 0, 0, 0, 0, 0, 0, 0, // MOV RBX, 2 suffix
];

fn session(code: &[u8]) -> DiagnosticVcpuSession {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(CODE_START), RAM_LEN).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
    for (offset, byte) in code.iter().enumerate() {
        bus.write_physical(
            GuestPhysicalAddress::new(CODE_START + offset as u64),
            AccessWidth::Byte,
            u64::from(*byte),
            |_| {},
        )
        .unwrap();
    }
    let mut state = LongModeState::default();
    state.rip = CODE_START;
    state.activity = Some(od_isa::CpuActivity::Running);
    state.sti_shadow = Some(od_isa::StiShadow::Clear);
    DiagnosticVcpuSession::new(
        state,
        bus,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        },
        INTEGER_FLAGS,
    )
    .unwrap()
}

struct SignalAfterNativeBlock<'a> {
    inner: PublishedBlockExecutor,
    event: &'a AtomicBool,
    native_blocks: usize,
}

impl ControlledBlockExecutor for SignalAfterNativeBlock<'_> {
    type Error = PublishedBlockExecutionError;

    fn execute_one_block_controlled(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<BlockExecutionReport, Self::Error> {
        self.inner
            .execute_one_block_controlled(block, state, incoming_budget, control)
    }

    fn execute_one_block_controlled_with_event(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<ControlledBlockExecution, Self::Error> {
        let result = self.inner.execute_one_block_controlled_with_event(
            block,
            state,
            incoming_budget,
            control,
        )?;
        if matches!(result, ControlledBlockExecution::Executed(_)) {
            self.native_blocks += 1;
            if self.native_blocks == 1 {
                self.event.store(true, Ordering::Release);
            }
        }
        Ok(result)
    }
}

fn control<'a>(stop: &'a AtomicU8, event: Option<&'a AtomicBool>) -> DispatchControl<'a> {
    DispatchControl {
        stop_rip: None,
        stop_requested: stop,
        event_requested: event,
    }
}

#[test]
fn published_native_block_yields_before_nop_and_resumes_without_repeating_prefix() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let mut session = session(&CODE);
    let stop = AtomicU8::new(0);
    let event = AtomicBool::new(false);
    let mut executor = SignalAfterNativeBlock {
        inner: PublishedBlockExecutor,
        event: &event,
        native_blocks: 0,
    };

    let first = session
        .run_jit_nop(
            CODE_START,
            &CODE,
            16,
            control(&stop, Some(&event)),
            &mut executor,
        )
        .unwrap();
    assert_eq!(first.dispatch.exit, DispatchExit::EventPending);
    assert_eq!(first.dispatch.retired_native, 2);
    assert_eq!(first.dispatch.retired_interpreter, 0);
    assert_eq!(first.dispatch.blocks_executed, 1);
    assert_eq!(first.dispatch.remaining_budget, 14);
    assert_eq!(session.state().rip, CODE_START + 12);
    assert_eq!(session.state().gpr[od_isa::Register::Rax as usize], 1);
    assert_eq!(session.state().gpr[od_isa::Register::Rbx as usize], 0);
    assert_eq!(executor.native_blocks, 1);
    assert!(event.load(Ordering::Acquire));

    let before_pause = session.state().clone();
    session.pause().unwrap();
    assert!(event.load(Ordering::Acquire));
    session.resume().unwrap();
    assert_eq!(session.state(), &before_pause);

    let repeated = session
        .run_jit_nop(
            CODE_START,
            &CODE,
            16,
            control(&stop, Some(&event)),
            &mut executor,
        )
        .unwrap();
    assert_eq!(repeated.dispatch.exit, DispatchExit::EventPending);
    assert_eq!(repeated.dispatch.retired_native, 0);
    assert_eq!(repeated.dispatch.retired_interpreter, 0);
    assert_eq!(repeated.dispatch.blocks_executed, 0);
    assert_eq!(executor.native_blocks, 1);
    assert_eq!(session.state(), &before_pause);
    assert!(event.load(Ordering::Acquire));

    event.store(false, Ordering::Release);
    let resumed = session
        .run_jit_nop(
            CODE_START,
            &CODE,
            14,
            control(&stop, Some(&event)),
            &mut executor,
        )
        .unwrap();
    assert_eq!(resumed.dispatch.exit, DispatchExit::CodeEnd);
    assert_eq!(resumed.dispatch.retired_native, 1);
    assert_eq!(resumed.dispatch.retired_interpreter, 1);
    assert_eq!(resumed.dispatch.blocks_executed, 1);
    assert_eq!(resumed.dispatch.remaining_budget, 12);
    assert_eq!(session.state().rip, CODE_START + CODE.len() as u64);
    assert_eq!(session.state().gpr[od_isa::Register::Rax as usize], 1);
    assert_eq!(session.state().gpr[od_isa::Register::Rbx as usize], 2);
    assert_eq!(executor.native_blocks, 2);
    assert!(!event.load(Ordering::Acquire));
}

struct SignalBeforePublication<'a> {
    inner: PublishedBlockExecutor,
    event: &'a AtomicBool,
    calls: usize,
}

impl ControlledBlockExecutor for SignalBeforePublication<'_> {
    type Error = PublishedBlockExecutionError;

    fn execute_one_block_controlled(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut LongModeState,
        budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<BlockExecutionReport, Self::Error> {
        self.inner
            .execute_one_block_controlled(block, state, budget, control)
    }

    fn execute_one_block_controlled_with_event(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut LongModeState,
        budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<ControlledBlockExecution, Self::Error> {
        self.calls += 1;
        self.event.store(true, Ordering::Release);
        self.inner
            .execute_one_block_controlled_with_event(block, state, budget, control)
    }
}

#[test]
fn event_during_publication_yields_without_native_execution() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let mut session = session(&CODE);
    let stop = AtomicU8::new(0);
    let event = AtomicBool::new(false);
    let mut executor = SignalBeforePublication {
        inner: PublishedBlockExecutor,
        event: &event,
        calls: 0,
    };
    let before = session.state().clone();

    let result = session
        .run_jit_nop(
            CODE_START,
            &CODE,
            16,
            control(&stop, Some(&event)),
            &mut executor,
        )
        .unwrap();

    assert_eq!(result.dispatch.exit, DispatchExit::EventPending);
    assert_eq!(result.dispatch.retired_native, 0);
    assert_eq!(result.dispatch.retired_interpreter, 0);
    assert_eq!(result.dispatch.blocks_executed, 0);
    assert_eq!(executor.calls, 1);
    assert_eq!(session.state(), &before);
    assert!(event.load(Ordering::Acquire));
}

fn interrupt_session() -> (DiagnosticVcpuSession, LocalApicHandle) {
    let local = LocalApic::new();
    let apic = local.handle();
    let specs = [
        (0x1000, 0x7000, MemoryRegionKind::Ram),
        (LOCAL_APIC_MMIO_BASE, 0x1000, MemoryRegionKind::Mmio),
    ];
    let map = MachineMemoryMap::new(
        36,
        specs
            .into_iter()
            .map(|(start, len, kind)| {
                MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap(),
                    kind,
                )
            })
            .collect(),
    )
    .unwrap();
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(0x7000),
        vec![Box::new(local)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    let mut write = |addr, width, value| {
        bus.write_physical(GuestPhysicalAddress::new(addr), width, value, |_| {})
            .unwrap()
    };
    write(LOCAL_APIC_MMIO_BASE + 0xf0, AccessWidth::Dword, 0x1ff);
    write(0x3000 + 0x61 * 16, AccessWidth::Qword, 0x8e00_0020_4000);
    write(0x3008 + 0x61 * 16, AccessWidth::Qword, 0);
    write(0x2020, AccessWidth::Qword, 0x0020_9b00_0000_0000);
    write(0x2008, AccessWidth::Qword, 0x00af_9b00_0000_ffff);
    write(0x2010, AccessWidth::Qword, 0x00cf_9300_0000_ffff);
    // Read saved RIP and ISR, write guest EOI, read ISR again, then IRETQ.
    for (addr, bytes) in [
        (CODE_START, CODE.as_slice()),
        (
            0x4000,
            &[
                0x48, 0x8b, 0x04, 0x24, 0x8b, 0x0a, 0x89, 0x37, 0x8b, 0x1a, 0x48, 0xcf,
            ][..],
        ),
    ] {
        for (offset, byte) in bytes.iter().enumerate() {
            write(addr + offset as u64, AccessWidth::Byte, u64::from(*byte));
        }
    }
    let mut state = LongModeState::default();
    state.rip = CODE_START;
    state.rflags = 0x202;
    state.gpr[od_isa::Register::Rsp as usize] = 0x7000;
    state.gpr[od_isa::Register::Rdi as usize] = LOCAL_APIC_MMIO_BASE + 0xb0;
    state.gpr[od_isa::Register::Rdx as usize] = LOCAL_APIC_MMIO_BASE + 0x130;
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
    state.sti_shadow = Some(od_isa::StiShadow::Clear);
    (
        DiagnosticVcpuSession::new(
            state,
            bus,
            AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
            INTEGER_FLAGS,
        )
        .unwrap(),
        apic,
    )
}

#[test]
fn published_jit_event_apic_eoi_iretq_and_resume() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let (mut session, apic) = interrupt_session();
    let stop = AtomicU8::new(0);
    let event = AtomicBool::new(false);
    let mut executor = SignalAfterNativeBlock {
        inner: PublishedBlockExecutor,
        event: &event,
        native_blocks: 0,
    };
    let first = session
        .run_jit_nop(
            CODE_START,
            &CODE,
            16,
            control(&stop, Some(&event)),
            &mut executor,
        )
        .unwrap()
        .dispatch;
    assert_eq!(
        (
            first.exit,
            first.retired_native,
            first.retired_interpreter,
            first.blocks_executed,
            first.remaining_budget
        ),
        (DispatchExit::EventPending, 2, 0, 1, 14)
    );
    let boundary = CODE_START + 12;
    assert_eq!(session.state().rip, boundary);
    assert_eq!(apic.eligible(), None); // Caller does not run an ordinary quantum without an IRQ.
    apic.request_fixed_edge(0x61).unwrap();
    let paused = session.state().clone();
    session.pause().unwrap();
    session.resume().unwrap();
    assert_eq!(session.state(), &paused);
    assert!(event.load(Ordering::Acquire));
    assert_eq!(apic.eligible(), Some(0x61));
    let delivered = session.run_apic_quantum(&apic, || false).unwrap();
    assert!(matches!(
        delivered,
        DiagnosticApicQuantumResult::Delivered {
            vector: 0x61,
            handler: od_machine::DiagnosticRunResult {
                instructions_executed: 1,
                ..
            }
        }
    ));
    assert_eq!(
        session.state().gpr[od_isa::Register::Rax as usize],
        boundary
    );
    assert_eq!(session.state().gpr[od_isa::Register::Rsp as usize], 0x6fd8);
    assert_eq!(apic.pending(), None);
    assert_eq!(session.run(1, || false).unwrap().instructions_executed, 1);
    assert_eq!(session.state().gpr[od_isa::Register::Rcx as usize], 1 << 1);
    assert_eq!(session.run(1, || false).unwrap().instructions_executed, 1);
    assert_eq!(session.run(1, || false).unwrap().instructions_executed, 1);
    assert_eq!(session.state().gpr[od_isa::Register::Rbx as usize], 0);
    assert_eq!(session.state().rip, 0x400a);
    assert_eq!(apic.eligible(), None);
    assert_eq!(
        session.run_same_cpl0_iretq().unwrap(),
        DiagnosticIretqResult::Retired
    );
    assert_eq!(session.state().rip, boundary);
    assert_eq!(session.state().gpr[od_isa::Register::Rsp as usize], 0x7000);
    assert_eq!(session.state().rflags, 0x202);
    event.store(false, Ordering::Release);
    let resumed = session
        .run_jit_nop(
            CODE_START,
            &CODE,
            first.remaining_budget,
            control(&stop, Some(&event)),
            &mut executor,
        )
        .unwrap()
        .dispatch;
    assert_eq!(
        (
            resumed.exit,
            resumed.retired_native,
            resumed.retired_interpreter,
            resumed.blocks_executed,
            resumed.remaining_budget
        ),
        (DispatchExit::CodeEnd, 1, 1, 1, 12)
    );
    assert_eq!(session.state().rip, CODE_START + CODE.len() as u64);
    assert_eq!(
        session.state().gpr[od_isa::Register::Rax as usize],
        boundary
    );
    assert_eq!(session.state().gpr[od_isa::Register::Rbx as usize], 2);
    assert_eq!(executor.native_blocks, 2);
}
