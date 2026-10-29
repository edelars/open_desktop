//! Exact, local Long64 CPL0 LRETQ profile used by the S2-27l continuation.

use crate::long_decode::canonical;
use crate::memory::{AccessContext, GuestMemory, MemoryError, PrivilegeLevel};
use crate::state::{CpuActivity, Gdtr, StiShadow};
use crate::{INTEGER_FLAGS, LongModeState};

const IF: u64 = 1 << 9;
const IOPL: u64 = 3 << 12;
const ADMITTED_FLAGS: u64 = INTEGER_FLAGS | IF | IOPL;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LretqProfile {
    SameCpl0ValidFrameV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LretqReadPhase {
    Fetch,
    Frame,
    CsDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LretqResult {
    Retired,
    Unsupported,
    Fault(crate::memory::ArchitecturalFault),
    ReadAborted {
        phase: LretqReadPhase,
        cause: MemoryError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LretqRunExit {
    StopAddress,
    StopRequested,
    InstructionBudgetExhausted,
    Unsupported,
    Fault(crate::memory::ArchitecturalFault),
    ReadAborted {
        phase: LretqReadPhase,
        cause: MemoryError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LretqRunResult {
    pub exit: LretqRunExit,
    pub instructions_executed: u64,
    pub rflags_defined: u64,
}

/// One-instruction run boundary for this profile. Stop and budget checks precede fetch.
pub fn run_with_lretq_profile(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    config: crate::execute::RunConfig,
    rflags_defined: u64,
    profile: LretqProfile,
    mut stop_requested: impl FnMut() -> bool,
) -> LretqRunResult {
    let finish = |exit, retired| LretqRunResult {
        exit,
        instructions_executed: retired,
        rflags_defined,
    };
    if config.stop_rip == Some(state.rip) {
        return finish(LretqRunExit::StopAddress, 0);
    }
    if stop_requested() {
        return finish(LretqRunExit::StopRequested, 0);
    }
    if config.instruction_budget == 0 {
        return finish(LretqRunExit::InstructionBudgetExhausted, 0);
    }
    if config.instruction_budget != 1 {
        return finish(LretqRunExit::Unsupported, 0);
    }
    match step_with_lretq_profile(state, memory, config.context, rflags_defined, profile) {
        LretqResult::Retired => finish(
            if config.stop_rip == Some(state.rip) {
                LretqRunExit::StopAddress
            } else {
                LretqRunExit::InstructionBudgetExhausted
            },
            1,
        ),
        LretqResult::Unsupported => finish(LretqRunExit::Unsupported, 0),
        LretqResult::Fault(fault) => finish(LretqRunExit::Fault(fault), 0),
        LretqResult::ReadAborted { phase, cause } => {
            finish(LretqRunExit::ReadAborted { phase, cause }, 0)
        }
    }
}

/// Execute only exact bytes `48 CB` with a valid two-qword same-CPL frame.
/// Caller certifies private synchronous RAM and FRED/CET/shadow stack/VMX/SMM off,
/// with no pending debug or NMI event.
pub fn step_with_lretq_profile(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    rflags_defined: u64,
    _profile: LretqProfile,
) -> LretqResult {
    if context.mode != crate::memory::InstructionMode::Long64
        || context.privilege != PrivilegeLevel::Ring0
        || state.activity != Some(CpuActivity::Running)
        || state.sti_shadow != Some(StiShadow::Clear)
        || rflags_defined != INTEGER_FLAGS
        || state.rflags & 2 == 0
        || state.rflags & !ADMITTED_FLAGS != 0
        || !valid_current_cs(state)
    {
        return LretqResult::Unsupported;
    }
    let Some(second) = state.rip.checked_add(1) else {
        return LretqResult::Unsupported;
    };
    if !canonical(state.rip) || !canonical(second) {
        return LretqResult::Unsupported;
    }
    let Some(gdtr) = state.gdtr else {
        return LretqResult::Unsupported;
    };
    let first_byte = match memory.fetch(state.rip, context) {
        Ok(byte) => byte,
        Err(cause) => {
            return LretqResult::ReadAborted {
                phase: LretqReadPhase::Fetch,
                cause,
            };
        }
    };
    if first_byte == 0xf0 {
        let Some(third) = second.checked_add(1) else {
            return LretqResult::Unsupported;
        };
        if !canonical(third) {
            return LretqResult::Unsupported;
        }
        let second_byte = match memory.fetch(second, context) {
            Ok(byte) => byte,
            Err(cause) => {
                return LretqResult::ReadAborted {
                    phase: LretqReadPhase::Fetch,
                    cause,
                };
            }
        };
        if second_byte != 0x48 {
            return LretqResult::Unsupported;
        }
        let third_byte = match memory.fetch(third, context) {
            Ok(byte) => byte,
            Err(cause) => {
                return LretqResult::ReadAborted {
                    phase: LretqReadPhase::Fetch,
                    cause,
                };
            }
        };
        if third_byte == 0xcb {
            return LretqResult::Fault(crate::memory::ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            });
        }
        return LretqResult::Unsupported;
    }
    if first_byte != 0x48 {
        return LretqResult::Unsupported;
    }
    let second_byte = match memory.fetch(second, context) {
        Ok(byte) => byte,
        Err(cause) => {
            return LretqResult::ReadAborted {
                phase: LretqReadPhase::Fetch,
                cause,
            };
        }
    };
    if second_byte != 0xcb {
        return LretqResult::Unsupported;
    }

    let old_rsp = state.gpr[crate::Register::Rsp as usize];
    let Some(frame_end) = old_rsp.checked_add(16) else {
        return LretqResult::Unsupported;
    };
    let Some(frame_last) = frame_end.checked_sub(1) else {
        return LretqResult::Unsupported;
    };
    if !canonical(old_rsp) || !canonical(frame_last) || !canonical(frame_end) {
        return LretqResult::Unsupported;
    }
    let mut frame = [0u8; 16];
    if let Err(cause) = memory.read(old_rsp, context, &mut frame) {
        return LretqResult::ReadAborted {
            phase: LretqReadPhase::Frame,
            cause,
        };
    }
    let target_rip = u64::from_le_bytes(frame[0..8].try_into().unwrap());
    let selector = u64::from_le_bytes(frame[8..16].try_into().unwrap()) as u16;
    if !canonical(target_rip) || !valid_selector(selector) {
        return LretqResult::Unsupported;
    }
    let Some(address) = descriptor_address(gdtr, selector) else {
        return LretqResult::Unsupported;
    };
    let mut raw = [0u8; 8];
    if let Err(cause) = memory.read(address, context.descriptor_table(), &mut raw) {
        return LretqResult::ReadAborted {
            phase: LretqReadPhase::CsDescriptor,
            cause,
        };
    }
    if !valid_cs(&raw) {
        return LretqResult::Unsupported;
    }

    state.rip = target_rip;
    state.cs_selector = Some(selector);
    state.cs_cache = Some(crate::state::CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: raw[5],
        flags: raw[6] >> 4,
    });
    state.gpr[crate::Register::Rsp as usize] = frame_end;
    LretqResult::Retired
}

fn valid_selector(selector: u16) -> bool {
    selector & 0xfffc != 0 && selector & 7 == 0
}

fn valid_current_cs(state: &LongModeState) -> bool {
    let (Some(selector), Some(cache)) = (state.cs_selector, state.cs_cache) else {
        return false;
    };
    selector != 0
        && selector & 7 == 0
        && cache.access & 0x88 == 0x88
        && cache.access & 0x10 != 0
        && cache.access & 0x60 == 0
        && cache.flags & 0x2 != 0
        && cache.flags & 0x4 == 0
}

fn descriptor_address(gdtr: Gdtr, selector: u16) -> Option<u64> {
    let offset = u64::from(selector & !7);
    if offset.checked_add(7)? > u64::from(gdtr.limit) {
        return None;
    }
    let address = gdtr.base.checked_add(offset)?;
    let last = address.checked_add(7)?;
    (canonical(address) && canonical(last)).then_some(address)
}

fn valid_cs(raw: &[u8; 8]) -> bool {
    raw[5] == 0x9b
        && raw[6] >> 4 == 0xa
        && decoded_base(raw) == 0
        && effective_limit(raw) == u32::MAX
}

fn decoded_base(raw: &[u8; 8]) -> u32 {
    u32::from(raw[2])
        | (u32::from(raw[3]) << 8)
        | (u32::from(raw[4]) << 16)
        | (u32::from(raw[7]) << 24)
}

fn effective_limit(raw: &[u8; 8]) -> u32 {
    let encoded = u32::from(raw[0]) | (u32::from(raw[1]) << 8) | (u32::from(raw[6] & 0x0f) << 16);
    if raw[6] & 0x80 != 0 {
        (encoded << 12) | 0xfff
    } else {
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{AccessKind, InstructionMode};
    use crate::state::{CodeSegmentDescriptorCache, LongModeState};

    const CODE: u64 = 0x100;
    const FRAME: u64 = 0x300;
    const GDT: u64 = 0x500;

    struct Memory {
        bytes: Vec<u8>,
        fetches: Vec<u64>,
        reads: Vec<(u64, usize)>,
        fail_read: Option<usize>,
        fail_fetch: Option<u64>,
    }
    impl Memory {
        fn setup() -> Self {
            let mut m = Self {
                bytes: vec![0; 8192],
                fetches: vec![],
                reads: vec![],
                fail_read: None,
                fail_fetch: None,
            };
            m.bytes[CODE as usize..CODE as usize + 2].copy_from_slice(&[0x48, 0xcb]);
            m.bytes[FRAME as usize..FRAME as usize + 8].copy_from_slice(&0x1234u64.to_le_bytes());
            m.bytes[FRAME as usize + 8..FRAME as usize + 16]
                .copy_from_slice(&0xabcd_0000_0000_0008u64.to_le_bytes());
            descriptor(&mut m.bytes[GDT as usize + 8..GDT as usize + 16]);
            descriptor(&mut m.bytes[GDT as usize + 24..GDT as usize + 32]);
            m
        }
        fn range(&self, a: u64, n: usize) -> Result<std::ops::Range<usize>, MemoryError> {
            let start = usize::try_from(a).unwrap();
            let end = start + n;
            if end > self.bytes.len() {
                return Err(MemoryError::Unmapped {
                    address: a,
                    access: AccessKind::Read,
                });
            }
            Ok(start..end)
        }
    }
    fn descriptor(raw: &mut [u8]) {
        raw.fill(0);
        raw[0] = 0xff;
        raw[1] = 0xff;
        raw[5] = 0x9b;
        raw[6] = 0xaf;
    }
    impl GuestMemory for Memory {
        fn fetch(&mut self, a: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.fetches.push(a);
            if self.fail_fetch == Some(a) {
                return Err(MemoryError::Backend("fetch abort".into()));
            }
            Ok(self.bytes[self.range(a, 1)?.start])
        }
        fn read(&mut self, a: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
            let i = self.reads.len();
            self.reads.push((a, out.len()));
            if self.fail_read == Some(i) {
                return Err(MemoryError::Backend("read abort".into()));
            }
            out.copy_from_slice(&self.bytes[self.range(a, out.len())?]);
            Ok(())
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            unreachable!()
        }
        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }
        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }
        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            unreachable!()
        }
    }
    fn state(rsp: u64) -> LongModeState {
        let mut s = LongModeState {
            rip: CODE,
            rflags: 2 | (1 << 9) | (3 << 12),
            gdtr: Some(Gdtr {
                base: GDT,
                limit: 0x1f,
            }),
            cs_selector: Some(8),
            cs_cache: Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9b,
                flags: 0xa,
            }),
            activity: Some(CpuActivity::Running),
            sti_shadow: Some(StiShadow::Clear),
            ..LongModeState::default()
        };
        s.gpr[crate::Register::Rsp as usize] = rsp;
        s
    }
    fn context() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        }
    }

    #[test]
    fn valid_frame_rereads_current_gdt_and_preserves_everything_else() {
        let mut cpu = state(FRAME);
        cpu.cs_selector = Some(0x18);
        cpu.cs_cache = Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 0xa,
        });
        cpu.gpr[crate::Register::Rax as usize] = 0xfeed;
        let before = cpu.clone();
        let mut mem = Memory::setup();
        let flags = cpu.rflags;
        let defined = INTEGER_FLAGS;
        assert_eq!(
            step_with_lretq_profile(
                &mut cpu,
                &mut mem,
                context(),
                defined,
                LretqProfile::SameCpl0ValidFrameV1
            ),
            LretqResult::Retired
        );
        assert_eq!(cpu.rip, 0x1234);
        assert_eq!(cpu.gpr[crate::Register::Rsp as usize], FRAME + 16);
        assert_eq!(cpu.rflags, flags);
        assert_eq!(cpu.cs_selector, Some(8));
        assert_eq!(
            cpu.cs_cache,
            Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9b,
                flags: 0xa
            })
        );
        let mut expected = before;
        expected.rip = 0x1234;
        expected.gpr[crate::Register::Rsp as usize] = FRAME + 16;
        expected.cs_selector = Some(8);
        expected.cs_cache = cpu.cs_cache;
        assert_eq!(cpu, expected);
        assert_eq!(mem.fetches, vec![CODE, CODE + 1]);
        assert_eq!(mem.reads, vec![(FRAME, 16), (GDT + 8, 8)]);
    }

    #[test]
    fn same_current_selector_still_requires_a_valid_descriptor_read_from_gdt() {
        let mut cpu = state(FRAME);
        cpu.cs_selector = Some(8);
        let before = cpu.clone();
        let mut mem = Memory::setup();
        mem.bytes[GDT as usize + 13] = 0x91;
        assert_eq!(
            step_with_lretq_profile(
                &mut cpu,
                &mut mem,
                context(),
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1
            ),
            LretqResult::Unsupported
        );
        assert_eq!(cpu, before);
        assert_eq!(mem.reads, vec![(FRAME, 16), (GDT + 8, 8)]);
    }
    #[test]
    fn every_stack_alignment_is_accepted_without_reading_a_third_qword() {
        for alignment in 0..16 {
            let rsp = FRAME + alignment;
            let mut cpu = state(rsp);
            let mut mem = Memory::setup();
            mem.bytes[rsp as usize..rsp as usize + 8].copy_from_slice(&0x1234u64.to_le_bytes());
            mem.bytes[rsp as usize + 8..rsp as usize + 16].copy_from_slice(&8u64.to_le_bytes());
            assert_eq!(
                step_with_lretq_profile(
                    &mut cpu,
                    &mut mem,
                    context(),
                    INTEGER_FLAGS,
                    LretqProfile::SameCpl0ValidFrameV1
                ),
                LretqResult::Retired
            );
            assert_eq!(mem.reads[0], (rsp, 16));
            assert_eq!(cpu.gpr[crate::Register::Rsp as usize], rsp + 16);
        }
    }
    #[test]
    fn invalid_and_aborted_inputs_leave_cpu_uncommitted() {
        for fail in 0..2 {
            let mut cpu = state(FRAME);
            let before = cpu.clone();
            let mut mem = Memory::setup();
            mem.fail_read = Some(fail);
            let result = step_with_lretq_profile(
                &mut cpu,
                &mut mem,
                context(),
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1,
            );
            assert!(matches!(result, LretqResult::ReadAborted { .. }));
            assert_eq!(cpu, before);
        }
        let mut cpu = state(FRAME);
        let before = cpu.clone();
        let mut mem = Memory::setup();
        mem.bytes[GDT as usize + 13] = 0x91;
        assert_eq!(
            step_with_lretq_profile(
                &mut cpu,
                &mut mem,
                context(),
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1
            ),
            LretqResult::Unsupported
        );
        assert_eq!(cpu, before);
        let mut cpu = state(FRAME);
        let before = cpu.clone();
        let mut mem = Memory::setup();
        mem.fail_fetch = Some(CODE + 1);
        assert!(matches!(
            step_with_lretq_profile(
                &mut cpu,
                &mut mem,
                context(),
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1
            ),
            LretqResult::ReadAborted {
                phase: LretqReadPhase::Fetch,
                ..
            }
        ));
        assert_eq!(cpu, before);
        for (rsp, selector, target) in [
            (FRAME, 0u64, 0x1234u64),
            (FRAME, 0xcu64, 0x1234u64),
            (FRAME, 8u64, 0x0000_8000_0000_0000u64),
            (u64::MAX - 15, 8u64, 0x1234u64),
        ] {
            let mut cpu = state(rsp);
            let before = cpu.clone();
            let mut mem = Memory::setup();
            if rsp == FRAME {
                mem.bytes[FRAME as usize..FRAME as usize + 8]
                    .copy_from_slice(&target.to_le_bytes());
                mem.bytes[FRAME as usize + 8..FRAME as usize + 16]
                    .copy_from_slice(&selector.to_le_bytes());
            }
            assert_eq!(
                step_with_lretq_profile(
                    &mut cpu,
                    &mut mem,
                    context(),
                    INTEGER_FLAGS,
                    LretqProfile::SameCpl0ValidFrameV1
                ),
                LretqResult::Unsupported
            );
            assert_eq!(cpu, before);
        }
        let mut cpu = state(FRAME);
        cpu.rflags |= 1 << 8;
        let before = cpu.clone();
        let mut mem = Memory::setup();
        assert_eq!(
            step_with_lretq_profile(
                &mut cpu,
                &mut mem,
                context(),
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1
            ),
            LretqResult::Unsupported
        );
        assert_eq!(cpu, before);
        assert!(mem.fetches.is_empty());
    }
    #[test]
    fn exact_lock_form_ud_after_full_fetch_and_run_boundary_precedes_fetch() {
        let mut cpu = state(FRAME);
        let mut mem = Memory::setup();
        mem.bytes[CODE as usize..CODE as usize + 3].copy_from_slice(&[0xf0, 0x48, 0xcb]);
        assert!(matches!(
            step_with_lretq_profile(
                &mut cpu,
                &mut mem,
                context(),
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1
            ),
            LretqResult::Fault(crate::memory::ArchitecturalFault { vector: 6, .. })
        ));
        assert_eq!(mem.fetches, vec![CODE, CODE + 1, CODE + 2]);
        let mut cpu = state(FRAME);
        let mut mem = Memory::setup();
        let config = crate::execute::RunConfig {
            context: context(),
            stop_rip: Some(CODE),
            instruction_budget: 1,
        };
        assert_eq!(
            run_with_lretq_profile(
                &mut cpu,
                &mut mem,
                config,
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1,
                || false
            )
            .exit,
            LretqRunExit::StopAddress
        );
        assert!(mem.fetches.is_empty());
        let config = crate::execute::RunConfig {
            context: context(),
            stop_rip: None,
            instruction_budget: 1,
        };
        assert_eq!(
            run_with_lretq_profile(
                &mut cpu,
                &mut mem,
                config,
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1,
                || true
            )
            .exit,
            LretqRunExit::StopRequested
        );
        assert!(mem.fetches.is_empty());
        let config = crate::execute::RunConfig {
            stop_rip: None,
            instruction_budget: 0,
            ..config
        };
        assert_eq!(
            run_with_lretq_profile(
                &mut cpu,
                &mut mem,
                config,
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1,
                || false
            )
            .instructions_executed,
            0
        );
        assert!(mem.fetches.is_empty());
    }

    #[test]
    fn neighboring_far_return_encodings_are_unsupported() {
        for bytes in [
            &[0xcb, 0, 0][..],
            &[0x49, 0xcb, 0][..],
            &[0x66, 0x48, 0xcb][..],
        ] {
            let mut cpu = state(FRAME);
            let before = cpu.clone();
            let mut mem = Memory::setup();
            mem.bytes[CODE as usize..CODE as usize + 3].copy_from_slice(bytes);
            assert_eq!(
                step_with_lretq_profile(
                    &mut cpu,
                    &mut mem,
                    context(),
                    INTEGER_FLAGS,
                    LretqProfile::SameCpl0ValidFrameV1
                ),
                LretqResult::Unsupported
            );
            assert_eq!(cpu, before);
        }
    }

    #[test]
    fn control_state_and_context_admission_fail_before_fetch() {
        let cases = [
            (
                state(FRAME),
                AccessContext {
                    privilege: PrivilegeLevel::Ring3,
                    ..context()
                },
                INTEGER_FLAGS,
            ),
            (
                {
                    let mut s = state(FRAME);
                    s.activity = None;
                    s
                },
                context(),
                INTEGER_FLAGS,
            ),
            (
                {
                    let mut s = state(FRAME);
                    s.sti_shadow = Some(StiShadow::Blocking);
                    s
                },
                context(),
                INTEGER_FLAGS,
            ),
            (
                {
                    let mut s = state(FRAME);
                    s.cs_selector = None;
                    s.cs_cache = None;
                    s
                },
                context(),
                INTEGER_FLAGS,
            ),
            (
                {
                    let mut s = state(FRAME);
                    s.gdtr = None;
                    s
                },
                context(),
                INTEGER_FLAGS,
            ),
            (state(FRAME), context(), INTEGER_FLAGS & !(1 << 6)),
        ];
        for (mut cpu, access, defined) in cases {
            let before = cpu.clone();
            let mut mem = Memory::setup();
            assert_eq!(
                step_with_lretq_profile(
                    &mut cpu,
                    &mut mem,
                    access,
                    defined,
                    LretqProfile::SameCpl0ValidFrameV1
                ),
                LretqResult::Unsupported
            );
            assert_eq!(cpu, before);
            assert!(mem.fetches.is_empty());
            assert!(mem.reads.is_empty());
        }
    }

    #[test]
    fn lgdt_lretq_nop_reads_the_new_gdt_and_executes_target_nop() {
        let mut mem = Memory::setup();
        mem.bytes[CODE as usize..CODE as usize + 7]
            .copy_from_slice(&[0x0f, 0x01, 0x55, 0xf6, 0x48, 0xcb, 0x90]);
        mem.bytes[0x1234] = 0x90;
        let gdtr_operand = 0x276usize;
        mem.bytes[gdtr_operand..gdtr_operand + 2].copy_from_slice(&0x001fu16.to_le_bytes());
        mem.bytes[gdtr_operand + 2..gdtr_operand + 10].copy_from_slice(&GDT.to_le_bytes());
        let mut cpu = state(FRAME);
        cpu.rip = CODE;
        cpu.gdtr = Some(Gdtr {
            base: 0x600,
            limit: 0x17,
        });
        cpu.gpr[crate::Register::Rbp as usize] = 0x280;
        assert_eq!(
            crate::execute::step(&mut cpu, &mut mem, context()),
            Ok(crate::execute::StepOutcome::Advanced)
        );
        assert_eq!(
            cpu.gdtr,
            Some(Gdtr {
                base: GDT,
                limit: 0x001f
            })
        );
        assert_eq!(cpu.rip, CODE + 4);
        assert_eq!(
            step_with_lretq_profile(
                &mut cpu,
                &mut mem,
                context(),
                INTEGER_FLAGS,
                LretqProfile::SameCpl0ValidFrameV1
            ),
            LretqResult::Retired
        );
        assert_eq!(cpu.rip, 0x1234);
        assert_eq!(cpu.cs_selector, Some(8));
        assert_eq!(cpu.gpr[crate::Register::Rsp as usize], FRAME + 16);
        assert_eq!(
            crate::execute::step(&mut cpu, &mut mem, context()),
            Ok(crate::execute::StepOutcome::Advanced)
        );
        assert_eq!(cpu.rip, 0x1235);
        assert_eq!(cpu.activity, Some(CpuActivity::Running));
    }
}
