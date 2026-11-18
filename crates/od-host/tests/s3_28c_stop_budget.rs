//! S3-28c: two cross-cutting checks of the one-vCPU JIT path on real native code.
//!
//! 1. STOP raised while a guest loop runs through several dispatcher entries is
//!    observed at the next dispatcher boundary: the current bounded block
//!    completes, no further block starts, and a resume from the actual RIP
//!    equals the uninterrupted interpreter.
//! 2. Native blocks and interpreter NOP steps decrement one shared budget: for
//!    every budget the dispatcher result equals the instruction-by-instruction
//!    interpreter reference.
//!
//! No SMP, no wall-time claim.

use std::num::NonZeroU64;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use od_host::{PublishedBlockExecutionError, PublishedBlockExecutor, PublishedCode};
use od_isa::LongModeState;
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use od_jit::block::{CodeView, NativeRegisterV1Admission};
use od_jit::dispatch::{
    BlockExecutionReport, CachedBlockBackend, DispatchControl, DispatchExit, SingleEntryBlockCache,
    run_cached_native_register_v1_controlled, run_with_native_register_v1,
};
use od_jit::runtime::RuntimeExitTag;
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};

static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());

struct CodeOnly<'a> {
    base: u64,
    bytes: &'a [u8],
}

impl GuestMemory for CodeOnly<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(self.base)
            .and_then(|offset| usize::try_from(offset).ok())
            .and_then(|offset| self.bytes.get(offset))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }
    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        unreachable!("register-only code has no data read")
    }
    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        unreachable!("register-only code has no data write")
    }
    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("register-only code has no atomic operation")
    }
    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("register-only code has no atomic operation")
    }
    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("register-only code has no atomic operation")
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!("register-only code has no fence")
    }
}

fn interpret(state: &mut LongModeState, base: u64, bytes: &[u8], steps: u64) {
    let mut memory = CodeOnly { base, bytes };
    for _ in 0..steps {
        match od_isa::execute::step(state, &mut memory, AccessContext::default()).unwrap() {
            od_isa::execute::StepOutcome::Advanced | od_isa::execute::StepOutcome::Branched => {}
            other => panic!("unexpected interpreter outcome {other:?}"),
        }
    }
}

fn context() -> ExecutionContextSnapshot {
    ExecutionContextSnapshot::native_register_v1_fixture(ExecutionContextId::from_u64(2).unwrap())
}

fn dependencies(view: CodeView<'_>, seed: u64) -> [CodeDependencySpan; 1] {
    let nz = |value| NonZeroU64::new(value).unwrap();
    [CodeDependencySpan {
        start_rip: view.base_rip(),
        end_rip: view.code_end(),
        identity: CodeIdentity {
            address_space: AddressSpaceId::new(nz(seed)),
            mapping: MappingId::new(nz(seed + 1)),
            mapping_version: MappingVersion::new(nz(1)),
            backing: BackingId::new(nz(seed + 2)),
            backing_version: BackingVersion::new(nz(1)),
            executable: true,
        },
    }]
}

/// Real cached native backend that raises the cooperative STOP flag after the
/// `stop_after`-th completed block execution (a second thread would do the
/// same at an arbitrary instant; this makes the instant deterministic).
struct StopAfterNth {
    inner: PublishedBlockExecutor,
    stop_after: usize,
    executions: usize,
}

impl CachedBlockBackend for StopAfterNth {
    type Error = PublishedBlockExecutionError;
    type Published = PublishedCode;

    fn publish_cached_block(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
    ) -> Result<Self::Published, Self::Error> {
        self.inner.publish_cached_block(block)
    }

    fn execute_cached_block(
        &mut self,
        published: &Self::Published,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<BlockExecutionReport, Self::Error> {
        let report =
            self.inner
                .execute_cached_block(published, block, state, incoming_budget, control)?;
        self.executions += 1;
        if self.executions == self.stop_after {
            control.stop_requested.store(1, Ordering::Release);
        }
        Ok(report)
    }
}

/// ADD RAX,RCX ; JZ back (RAX = RCX = 0 keeps ZF set): a two-instruction block
/// that the dispatcher re-enters forever.
/// Limitation: the native register profile admits only RAX-based forms and
/// `JZ` needs ZF set, so the register state is identical at every block
/// boundary; exactness of the stop point rests on the counters (blocks,
/// retirement, publications) and on the interpreter comparison.
const LOOP: [u8; 5] = [0x48, 0x03, 0xC1, 0x74, 0xFB];
const LOOP_INSNS: u64 = 2;

#[test]
fn s3_28c_stop_in_a_guest_loop_exits_after_the_current_block_and_resumes_exactly() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let base = 0x7D40;
    let view = CodeView::new(base, &LOOP).unwrap();
    let admission = NativeRegisterV1Admission::from_code(view).unwrap();
    let deps = dependencies(view, 71);
    for stop_after in [1_usize, 2, 5] {
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut state = LongModeState::default();
        state.rip = base;
        let initial = state.clone();
        let mut backend = StopAfterNth {
            inner: PublishedBlockExecutor,
            stop_after,
            executions: 0,
        };
        let mut cache =
            SingleEntryBlockCache::new(1, od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let first = run_cached_native_register_v1_controlled(
            view,
            &admission,
            context(),
            &deps,
            &mut state,
            1000,
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        let blocks = stop_after as u64;
        assert_eq!(
            first.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(first.blocks_executed, blocks, "stop_after={stop_after}");
        assert_eq!(backend.executions, stop_after, "no block after STOP");
        assert_eq!(first.retired_native, LOOP_INSNS * blocks);
        assert_eq!(first.remaining_budget, 1000 - LOOP_INSNS * blocks);
        assert_eq!(cache.counters().successful_publications, 1);
        assert_eq!(cache.counters().hits, blocks - 1);
        let mut expected = initial.clone();
        interpret(&mut expected, base, &LOOP, LOOP_INSNS * blocks);
        assert_eq!(state, expected, "state at the STOP boundary");
        assert_eq!(stop.load(Ordering::Acquire), 1, "STOP is not cleared");

        // Resume from the actual RIP with the flag cleared: three more blocks.
        stop.store(0, Ordering::Release);
        backend.stop_after = usize::MAX;
        let resumed = run_cached_native_register_v1_controlled(
            view,
            &admission,
            context(),
            &deps,
            &mut state,
            first.remaining_budget.min(3 * LOOP_INSNS),
            control,
            &mut cache,
            &mut backend,
        )
        .unwrap();
        assert_eq!(resumed.exit, DispatchExit::BudgetExhausted);
        assert_eq!(resumed.retired_native, 3 * LOOP_INSNS);
        assert_eq!(resumed.blocks_executed, 3);
        interpret(&mut expected, base, &LOOP, 3 * LOOP_INSNS);
        assert_eq!(
            state, expected,
            "resume equals the uninterrupted interpreter"
        );
        assert_eq!(
            cache.counters().successful_publications,
            1,
            "block stays resident"
        );
    }
}

/// ADD RAX,1 ; NOP ; ADD RAX,1 ; NOP ; ADD RAX,1 — three native steps and two
/// interpreter NOP steps share one budget.
const MIXED: [u8; 14] = [
    0x48, 0x83, 0xC0, 0x01, 0x90, 0x48, 0x83, 0xC0, 0x01, 0x90, 0x48, 0x83, 0xC0, 0x01,
];

#[test]
fn s3_28c_native_blocks_and_interpreter_steps_share_one_budget() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let base = 0x7E40;
    let view = CodeView::new(base, &MIXED).unwrap();
    let admission = NativeRegisterV1Admission::from_code(view).unwrap();
    let deps = dependencies(view, 81);
    let stop = AtomicU8::new(0);
    let control = DispatchControl {
        stop_rip: None,
        stop_requested: &stop,
        event_requested: None,
    };
    for budget in 0_u64..=7 {
        let steps = budget.min(5);
        let native_steps = [1_u64, 1, 2, 2, 3][..steps as usize]
            .last()
            .copied()
            .unwrap_or(0);
        let mut initial = LongModeState::default();
        initial.rip = base;
        initial.rflags = 0x202;
        let mut expected = initial.clone();
        interpret(&mut expected, base, &MIXED, steps);

        let mut uncached_state = initial.clone();
        let uncached = run_with_native_register_v1(
            view,
            &admission,
            &mut uncached_state,
            budget,
            control,
            &mut PublishedBlockExecutor,
        )
        .unwrap();
        let expected_exit = if budget >= 5 {
            DispatchExit::CodeEnd
        } else {
            DispatchExit::BudgetExhausted
        };
        assert_eq!(uncached.exit, expected_exit, "budget={budget}");
        assert_eq!(uncached_state, expected, "budget={budget}");
        assert_eq!(uncached.remaining_budget, budget - steps, "budget={budget}");
        assert_eq!(uncached.retired_native, native_steps, "budget={budget}");
        assert_eq!(
            uncached.retired_interpreter,
            steps - native_steps,
            "budget={budget}"
        );
        assert_eq!(
            uncached.retired_native + uncached.retired_interpreter + uncached.remaining_budget,
            budget,
            "native + interpreter retirement + remaining == incoming budget"
        );

        let mut cached_state = initial;
        let mut cache =
            SingleEntryBlockCache::new(1, od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES).unwrap();
        let cached = run_cached_native_register_v1_controlled(
            view,
            &admission,
            context(),
            &deps,
            &mut cached_state,
            budget,
            control,
            &mut cache,
            &mut PublishedBlockExecutor,
        )
        .unwrap();
        assert_eq!(cached, uncached, "budget={budget}");
        assert_eq!(cached_state, expected, "budget={budget}");
    }
}
