//! Exact, local Long64 CPL0 IRETQ profile used by the S2-27k continuation.

use crate::long_decode::canonical;
use crate::memory::{AccessContext, GuestMemory, MemoryError, PrivilegeLevel};
use crate::state::{CpuActivity, DataSegmentDescriptorCache, Gdtr, StiShadow};
use crate::{INTEGER_FLAGS, LongModeState};

const IF: u64 = 1 << 9;
const IOPL: u64 = 3 << 12;
const RESTORABLE_FLAGS: u64 = INTEGER_FLAGS | IF | IOPL;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IretqProfile {
    SameCpl0ValidFrameV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IretqReadPhase {
    Fetch,
    Frame,
    CsDescriptor,
    SsDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IretqResult {
    Retired,
    Unsupported,
    Fault(crate::memory::ArchitecturalFault),
    ReadAborted {
        phase: IretqReadPhase,
        cause: MemoryError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IretqRunExit {
    StopAddress,
    StopRequested,
    InstructionBudgetExhausted,
    Unsupported,
    Fault(crate::memory::ArchitecturalFault),
    ReadAborted {
        phase: IretqReadPhase,
        cause: MemoryError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IretqRunResult {
    pub exit: IretqRunExit,
    pub instructions_executed: u64,
    pub rflags_defined: u64,
}

/// One-instruction run boundary for the local profile. Host STOP and budget
/// exits precede fetch; this API never continues after IRETQ.
pub fn run_with_iretq_profile(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    config: crate::execute::RunConfig,
    rflags_defined: u64,
    profile: IretqProfile,
    mut stop_requested: impl FnMut() -> bool,
) -> IretqRunResult {
    let finish = |exit, retired| IretqRunResult {
        exit,
        instructions_executed: retired,
        rflags_defined,
    };
    if state.cr0.is_some() || state.cr4.is_some() {
        return finish(IretqRunExit::Unsupported, 0);
    }
    if config.stop_rip == Some(state.rip) {
        return finish(IretqRunExit::StopAddress, 0);
    }
    if stop_requested() {
        return finish(IretqRunExit::StopRequested, 0);
    }
    if config.instruction_budget == 0 {
        return finish(IretqRunExit::InstructionBudgetExhausted, 0);
    }
    if config.instruction_budget != 1 {
        return finish(IretqRunExit::Unsupported, 0);
    }
    let mut defined = rflags_defined;
    match step_with_iretq_profile(state, memory, config.context, &mut defined, profile) {
        IretqResult::Retired => finish(
            if config.stop_rip == Some(state.rip) {
                IretqRunExit::StopAddress
            } else {
                IretqRunExit::InstructionBudgetExhausted
            },
            1,
        ),
        IretqResult::Unsupported => finish(IretqRunExit::Unsupported, 0),
        IretqResult::Fault(fault) => finish(IretqRunExit::Fault(fault), 0),
        IretqResult::ReadAborted { phase, cause } => {
            finish(IretqRunExit::ReadAborted { phase, cause }, 0)
        }
    }
}

/// Execute only `48 CF` under the same-CPL0 valid-frame profile.
///
/// The caller certifies private synchronous RAM and disabled FRED, CET,
/// shadow-stack, VMX and SMM state. Callback errors are terminal host/read
/// aborts, never converted into architectural faults by this profile.
pub fn step_with_iretq_profile(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    rflags_defined: &mut u64,
    _profile: IretqProfile,
) -> IretqResult {
    if state.cr0.is_some()
        || state.cr4.is_some()
        || context.mode != crate::memory::InstructionMode::Long64
        || context.privilege != PrivilegeLevel::Ring0
        || state.activity != Some(CpuActivity::Running)
        || state.sti_shadow != Some(StiShadow::Clear)
        || *rflags_defined != INTEGER_FLAGS
        || state.rflags & 2 == 0
        || state.rflags & !RESTORABLE_FLAGS != 0
    {
        return IretqResult::Unsupported;
    }
    let Some(second_address) = state.rip.checked_add(1) else {
        return IretqResult::Unsupported;
    };
    if !canonical(state.rip) || !canonical(second_address) {
        return IretqResult::Unsupported;
    }

    let first = match memory.fetch(state.rip, context) {
        Ok(byte) => byte,
        Err(cause) => {
            return IretqResult::ReadAborted {
                phase: IretqReadPhase::Fetch,
                cause,
            };
        }
    };
    if first == 0xcf || first != 0x48 && first != 0xf0 {
        return IretqResult::Unsupported;
    }
    let second = match memory.fetch(second_address, context) {
        Ok(byte) => byte,
        Err(cause) => {
            return IretqResult::ReadAborted {
                phase: IretqReadPhase::Fetch,
                cause,
            };
        }
    };
    if first == 0x48 {
        if second != 0xcf {
            return IretqResult::Unsupported;
        }
    } else if second != 0x48 {
        return IretqResult::Unsupported;
    } else {
        let Some(third_address) = state.rip.checked_add(2) else {
            return IretqResult::Unsupported;
        };
        if !canonical(third_address) {
            return IretqResult::Unsupported;
        }
        let third = match memory.fetch(third_address, context) {
            Ok(byte) => byte,
            Err(cause) => {
                return IretqResult::ReadAborted {
                    phase: IretqReadPhase::Fetch,
                    cause,
                };
            }
        };
        if third == 0xcf {
            return IretqResult::Fault(crate::memory::ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            });
        }
        return IretqResult::Unsupported;
    }

    let (Some(gdtr), Some(rsp)) = (state.gdtr, Some(state.gpr[crate::Register::Rsp as usize]))
    else {
        return IretqResult::Unsupported;
    };
    let Some(frame_last) = rsp.checked_add(39) else {
        return IretqResult::Unsupported;
    };
    if !canonical(rsp) || !canonical(frame_last) {
        return IretqResult::Unsupported;
    }
    let mut frame = [0u8; 40];
    if let Err(cause) = memory.read(rsp, context, &mut frame) {
        return IretqResult::ReadAborted {
            phase: IretqReadPhase::Frame,
            cause,
        };
    }
    let qword = |n: usize| u64::from_le_bytes(frame[n * 8..n * 8 + 8].try_into().unwrap());
    let saved_rip = qword(0);
    let saved_cs = qword(1) as u16;
    let saved_flags = qword(2);
    let saved_rsp = qword(3);
    let saved_ss = qword(4) as u16;
    if saved_flags & 2 == 0
        || saved_flags & !RESTORABLE_FLAGS != 0
        || !canonical(saved_rip)
        || !canonical(saved_rsp)
        || !valid_selector(saved_cs)
        || !valid_selector(saved_ss)
    {
        return IretqResult::Unsupported;
    }
    let Some(cs_address) = descriptor_address(gdtr, saved_cs) else {
        return IretqResult::Unsupported;
    };
    let Some(ss_address) = descriptor_address(gdtr, saved_ss) else {
        return IretqResult::Unsupported;
    };
    let mut cs_raw = [0; 8];
    if let Err(cause) = memory.read(cs_address, context.descriptor_table(), &mut cs_raw) {
        return IretqResult::ReadAborted {
            phase: IretqReadPhase::CsDescriptor,
            cause,
        };
    }
    let mut ss_raw = [0; 8];
    if let Err(cause) = memory.read(ss_address, context.descriptor_table(), &mut ss_raw) {
        return IretqResult::ReadAborted {
            phase: IretqReadPhase::SsDescriptor,
            cause,
        };
    }
    if !valid_cs(&cs_raw) || !valid_ss(&ss_raw) {
        return IretqResult::Unsupported;
    }

    // Commit only after the complete frame and both descriptors have passed.
    state.rip = saved_rip;
    state.cs_selector = Some(saved_cs);
    state.cs_cache = Some(crate::state::CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: cs_raw[5],
        flags: cs_raw[6] >> 4,
    });
    state.rflags = saved_flags;
    state.gpr[crate::Register::Rsp as usize] = saved_rsp;
    state.ss_selector = Some(saved_ss);
    state.ss_cache = Some(DataSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: ss_raw[5],
        flags: ss_raw[6] >> 4,
    });
    state.activity = Some(CpuActivity::Running);
    state.sti_shadow = Some(StiShadow::Clear);
    IretqResult::Retired
}

fn valid_selector(selector: u16) -> bool {
    selector & 0xfffc != 0 && selector & 7 == 0
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

fn valid_ss(raw: &[u8; 8]) -> bool {
    raw[5] == 0x93
        && raw[6] >> 4 == 0xc
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
    use crate::memory::{AccessContext, AccessKind, InstructionMode};

    const CODE: u64 = 0x100;
    const FRAME: u64 = 0x300;
    const GDT: u64 = 0x500;

    #[test]
    fn cr0_state_rejects_run_and_step_before_stop_or_memory_access() {
        let mut cpu = LongModeState {
            cr4: None,
            cr0: Some(crate::execute::Cr0Profile::BASE),
            ..LongModeState::default()
        };
        let before = cpu.clone();
        let stop_rip = cpu.rip;
        let mut memory = Memory::setup();
        assert_eq!(
            run_with_iretq_profile(
                &mut cpu,
                &mut memory,
                crate::execute::RunConfig {
                    instruction_budget: 0,
                    stop_rip: Some(stop_rip),
                    context: AccessContext::default(),
                },
                INTEGER_FLAGS,
                IretqProfile::SameCpl0ValidFrameV1,
                || false,
            )
            .exit,
            IretqRunExit::Unsupported
        );
        assert_eq!(cpu, before);
        assert!(memory.fetches.is_empty() && memory.reads.is_empty());
        let mut defined = INTEGER_FLAGS;
        assert_eq!(
            step_with_iretq_profile(
                &mut cpu,
                &mut memory,
                AccessContext::default(),
                &mut defined,
                IretqProfile::SameCpl0ValidFrameV1,
            ),
            IretqResult::Unsupported
        );
        assert!(memory.fetches.is_empty() && memory.reads.is_empty());
    }

    struct Memory {
        bytes: Vec<u8>,
        fetches: Vec<u64>,
        reads: Vec<(u64, usize, AccessContext)>,
        writes: usize,
        fail_read: Option<usize>,
        fail_fetch: Option<u64>,
    }

    impl Memory {
        fn setup() -> Self {
            let mut memory = Self {
                bytes: vec![0; 4096],
                fetches: Vec::new(),
                reads: Vec::new(),
                writes: 0,
                fail_read: None,
                fail_fetch: None,
            };
            memory.bytes[CODE as usize..CODE as usize + 2].copy_from_slice(&[0x48, 0xcf]);
            let frame = [0x1234, 0x8, 0x2 | IF | IOPL, 0x711, 0x10];
            for (i, value) in frame.into_iter().enumerate() {
                memory.bytes[FRAME as usize + i * 8..FRAME as usize + (i + 1) * 8]
                    .copy_from_slice(&value.to_le_bytes());
            }
            descriptor(
                &mut memory.bytes[GDT as usize + 8..GDT as usize + 16],
                0x9b,
                0xa,
            );
            descriptor(
                &mut memory.bytes[GDT as usize + 16..GDT as usize + 24],
                0x93,
                0xc,
            );
            memory
        }

        fn range(
            &self,
            address: u64,
            length: usize,
        ) -> Result<std::ops::Range<usize>, MemoryError> {
            let start = usize::try_from(address).unwrap();
            let end = start + length;
            if end > self.bytes.len() {
                return Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                });
            }
            Ok(start..end)
        }
        fn configure_gate(&mut self, vector: u8, target: u64) {
            let at = 0x800 + usize::from(vector) * 16;
            self.bytes[at..at + 2].copy_from_slice(&(target as u16).to_le_bytes());
            self.bytes[at + 2..at + 4].copy_from_slice(&8u16.to_le_bytes());
            self.bytes[at + 4] = 0;
            self.bytes[at + 5] = 0x8e;
            self.bytes[at + 6..at + 8].copy_from_slice(&((target >> 16) as u16).to_le_bytes());
            self.bytes[at + 8..at + 12].copy_from_slice(&((target >> 32) as u32).to_le_bytes());
            self.bytes[at + 12..at + 16].fill(0);
        }
    }

    fn descriptor(raw: &mut [u8], access: u8, flags: u8) {
        raw.fill(0);
        raw[0] = 0xff;
        raw[1] = 0xff;
        raw[5] = access;
        raw[6] = 0xcf;
        raw[6] = (raw[6] & 0x0f) | (flags << 4);
    }

    impl GuestMemory for Memory {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.fetches.push(address);
            if self.fail_fetch == Some(address) {
                return Err(MemoryError::Backend("fetch abort".into()));
            }
            Ok(self.bytes[self.range(address, 1)?.start])
        }
        fn read(
            &mut self,
            address: u64,
            context: AccessContext,
            out: &mut [u8],
        ) -> Result<(), MemoryError> {
            let index = self.reads.len();
            self.reads.push((address, out.len(), context));
            if self.fail_read == Some(index) {
                return Err(MemoryError::Backend("read abort".into()));
            }
            out.copy_from_slice(&self.bytes[self.range(address, out.len())?]);
            Ok(())
        }
        fn write(
            &mut self,
            address: u64,
            _: AccessContext,
            source: &[u8],
        ) -> Result<(), MemoryError> {
            self.writes += 1;
            let range = self.range(address, source.len())?;
            self.bytes[range].copy_from_slice(source);
            Ok(())
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

    fn state() -> LongModeState {
        LongModeState {
            cr4: None,
            rip: CODE,
            rflags: 2 | IF,
            gdtr: Some(Gdtr {
                base: GDT,
                limit: 0x17,
            }),
            activity: Some(CpuActivity::Running),
            sti_shadow: Some(StiShadow::Clear),
            gpr: {
                let mut gpr = [0; 16];
                gpr[crate::Register::Rsp as usize] = FRAME;
                gpr
            },
            ..LongModeState::default()
        }
    }

    fn ring0() -> AccessContext {
        AccessContext {
            privilege: PrivilegeLevel::Ring0,
            mode: InstructionMode::Long64,
            ..AccessContext::default()
        }
    }

    #[test]
    fn valid_frame_restores_cs_ss_and_five_qwords_atomically() {
        let mut cpu = state();
        cpu.gpr[crate::Register::Rbx as usize] = 0xfeed_face_cafe_beef;
        cpu.fs_base = 0x1234;
        cpu.gs_base = 0x5678;
        let mut memory = Memory::setup();
        let mut defined = INTEGER_FLAGS;
        let before = cpu.clone();
        assert_eq!(
            step_with_iretq_profile(
                &mut cpu,
                &mut memory,
                ring0(),
                &mut defined,
                IretqProfile::SameCpl0ValidFrameV1
            ),
            IretqResult::Retired
        );
        assert_eq!(cpu.rip, 0x1234);
        assert_eq!(cpu.cs_selector, Some(8));
        assert_eq!(
            cpu.cs_cache,
            Some(crate::CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9b,
                flags: 0xa
            })
        );
        assert_eq!(cpu.gpr[crate::Register::Rsp as usize], 0x711);
        assert_eq!(cpu.ss_selector, Some(0x10));
        assert_eq!(
            cpu.ss_cache,
            Some(DataSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x93,
                flags: 0xc
            })
        );
        assert_eq!(cpu.rflags, 2 | IF | IOPL);
        assert_eq!(cpu.activity, Some(CpuActivity::Running));
        assert_eq!(cpu.sti_shadow, Some(StiShadow::Clear));
        assert_eq!(defined, INTEGER_FLAGS);
        assert_eq!(memory.reads.len(), 3);
        assert_eq!(
            memory.reads[0].0..memory.reads[0].0 + memory.reads[0].1 as u64,
            FRAME..FRAME + 40
        );
        assert_eq!(memory.reads[1].0, GDT + 8);
        assert_eq!(memory.reads[2].0, GDT + 16);
        assert_eq!(memory.writes, 0);
        let mut expected = before;
        expected.rip = 0x1234;
        expected.cs_selector = Some(8);
        expected.cs_cache = Some(crate::CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 0xa,
        });
        expected.rflags = 2 | IF | IOPL;
        expected.gpr[crate::Register::Rsp as usize] = 0x711;
        expected.ss_selector = Some(0x10);
        expected.ss_cache = Some(DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0xc,
        });
        assert_eq!(cpu, expected);
    }

    #[test]
    fn each_read_abort_is_terminal_and_does_not_commit_cpu_state() {
        for (failed_read, expected_phase) in [
            IretqReadPhase::Frame,
            IretqReadPhase::CsDescriptor,
            IretqReadPhase::SsDescriptor,
        ]
        .into_iter()
        .enumerate()
        {
            let mut cpu = state();
            let before = cpu.clone();
            let mut memory = Memory::setup();
            memory.fail_read = Some(failed_read);
            let mut defined = INTEGER_FLAGS;
            assert!(matches!(
                step_with_iretq_profile(
                    &mut cpu,
                    &mut memory,
                    ring0(),
                    &mut defined,
                    IretqProfile::SameCpl0ValidFrameV1
                ),
                IretqResult::ReadAborted { phase, .. } if phase == expected_phase
            ));
            assert_eq!(cpu, before);
            assert_eq!(memory.writes, 0);
        }
    }

    #[test]
    fn fetch_callback_abort_keeps_phase_and_state() {
        let mut cpu = state();
        let before = cpu.clone();
        let mut memory = Memory::setup();
        memory.fail_fetch = Some(CODE + 1);
        let mut defined = INTEGER_FLAGS;
        assert!(matches!(
            step_with_iretq_profile(
                &mut cpu,
                &mut memory,
                ring0(),
                &mut defined,
                IretqProfile::SameCpl0ValidFrameV1,
            ),
            IretqResult::ReadAborted {
                phase: IretqReadPhase::Fetch,
                ..
            }
        ));
        assert_eq!(cpu, before);
        assert!(memory.reads.is_empty());
    }

    #[test]
    fn malformed_frame_and_descriptors_are_unsupported_without_commit() {
        for mutate in [0usize, 1, 2, 3, 4, 5] {
            let mut cpu = state();
            let before = cpu.clone();
            let mut memory = Memory::setup();
            match mutate {
                0 => memory.bytes[(FRAME + 8) as usize] = 0,
                1 => memory.bytes[(FRAME + 16) as usize] &= !0x02,
                2 => memory.bytes[(FRAME + 24) as usize..(FRAME + 32) as usize]
                    .copy_from_slice(&0x0000_8000_0000_0000u64.to_le_bytes()),
                3 => memory.bytes[(FRAME + 8) as usize..(FRAME + 16) as usize].fill(0),
                4 => memory.bytes[(GDT + 8 + 5) as usize] = 0x9a,
                _ => memory.bytes[(GDT + 16 + 5) as usize] = 0x91,
            }
            let mut defined = INTEGER_FLAGS;
            assert_eq!(
                step_with_iretq_profile(
                    &mut cpu,
                    &mut memory,
                    ring0(),
                    &mut defined,
                    IretqProfile::SameCpl0ValidFrameV1
                ),
                IretqResult::Unsupported
            );
            assert_eq!(cpu, before);
        }
    }

    #[test]
    fn strict_profile_gate_and_full_fetch_lock_ud() {
        let mut cpu = state();
        let before = cpu.clone();
        let mut memory = Memory::setup();
        let mut defined = INTEGER_FLAGS;
        let user = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..ring0()
        };
        assert_eq!(
            step_with_iretq_profile(
                &mut cpu,
                &mut memory,
                user,
                &mut defined,
                IretqProfile::SameCpl0ValidFrameV1
            ),
            IretqResult::Unsupported
        );
        assert_eq!(cpu, before);
        memory.bytes[CODE as usize..CODE as usize + 3].copy_from_slice(&[0xf0, 0x48, 0xcf]);
        cpu.rip = CODE;
        assert!(
            matches!(step_with_iretq_profile(&mut cpu, &mut memory, ring0(), &mut defined, IretqProfile::SameCpl0ValidFrameV1), IretqResult::Fault(fault) if fault.vector == 6)
        );
        assert_eq!(memory.fetches.len(), 3); // The failed CPL gate performs no instruction fetch.
    }

    #[test]
    fn legacy_step_does_not_admit_iretq_without_profile() {
        let mut cpu = state();
        let before = cpu.clone();
        let mut memory = Memory::setup();
        let result = crate::execute::step(&mut cpu, &mut memory, ring0()).unwrap();
        assert_eq!(
            result,
            crate::execute::StepOutcome::Unsupported { opcode: 0xcf }
        );
        assert_eq!(cpu, before);
        assert!(memory.reads.is_empty());
    }

    #[test]
    fn run_boundary_orders_stop_callback_budget_then_fetch() {
        use crate::execute::RunConfig;
        let mut cpu = state();
        let mut memory = Memory::setup();
        let mut polls = 0;
        let stopped_at_entry = run_with_iretq_profile(
            &mut cpu,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(CODE),
                context: ring0(),
            },
            INTEGER_FLAGS,
            IretqProfile::SameCpl0ValidFrameV1,
            || {
                polls += 1;
                false
            },
        );
        assert_eq!(stopped_at_entry.exit, IretqRunExit::StopAddress);
        assert_eq!(polls, 0);
        assert!(memory.fetches.is_empty());

        let stopped = run_with_iretq_profile(
            &mut cpu,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: ring0(),
            },
            INTEGER_FLAGS,
            IretqProfile::SameCpl0ValidFrameV1,
            || {
                polls += 1;
                true
            },
        );
        assert_eq!(stopped.exit, IretqRunExit::StopRequested);
        assert_eq!(polls, 1);
        assert!(memory.fetches.is_empty());

        let budget = run_with_iretq_profile(
            &mut cpu,
            &mut memory,
            RunConfig {
                instruction_budget: 0,
                stop_rip: None,
                context: ring0(),
            },
            INTEGER_FLAGS,
            IretqProfile::SameCpl0ValidFrameV1,
            || {
                polls += 1;
                false
            },
        );
        assert_eq!(budget.exit, IretqRunExit::InstructionBudgetExhausted);
        assert_eq!(polls, 2);
        assert!(memory.fetches.is_empty());

        let completed = run_with_iretq_profile(
            &mut cpu,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(0x1234),
                context: ring0(),
            },
            INTEGER_FLAGS,
            IretqProfile::SameCpl0ValidFrameV1,
            || false,
        );
        assert_eq!(completed.exit, IretqRunExit::StopAddress);
        assert_eq!(completed.instructions_executed, 1);
    }

    #[test]
    fn sti_hlt_delivery_iretq_restores_post_hlt_state_and_all_stack_alignments() {
        use crate::deliver_maskable_interrupt;
        use crate::execute::{
            MaskableRequestRunResult, RunConfig, RunExit, run_with_halt_profile,
            run_with_maskable_request, step,
        };

        for offset in 0..16u64 {
            for iopl in [0u64, 3] {
                let mut memory = Memory::setup();
                memory.bytes[CODE as usize..CODE as usize + 3].copy_from_slice(&[0xfb, 0xf4, 0x90]);
                memory.bytes[(CODE + 0x20) as usize..(CODE + 0x22) as usize]
                    .copy_from_slice(&[0x48, 0xcf]);
                memory.configure_gate(14, CODE + 0x20);
                let old_rsp = 0xf00 + offset;
                let mut cpu = LongModeState {
                    cr4: None,
                    rip: CODE,
                    rflags: 2 | (iopl << 12),
                    gdtr: Some(Gdtr {
                        base: GDT,
                        limit: 0x17,
                    }),
                    idtr: Some(crate::state::Idtr {
                        base: 0x800,
                        limit: 0xef,
                    }),
                    cs_selector: Some(8),
                    cs_cache: Some(crate::CodeSegmentDescriptorCache {
                        base: 0,
                        limit: u32::MAX,
                        access: 0x9b,
                        flags: 0xa,
                    }),
                    ss_selector: Some(0x10),
                    ss_cache: Some(DataSegmentDescriptorCache {
                        base: 0,
                        limit: u32::MAX,
                        access: 0x93,
                        flags: 0xc,
                    }),
                    sti_shadow: Some(StiShadow::Clear),
                    activity: Some(CpuActivity::Running),
                    gpr: {
                        let mut gpr = [0; 16];
                        gpr[crate::Register::Rsp as usize] = old_rsp;
                        gpr
                    },
                    ..LongModeState::default()
                };
                let halted = run_with_halt_profile(
                    &mut cpu,
                    &mut memory,
                    RunConfig {
                        instruction_budget: 2,
                        stop_rip: None,
                        context: ring0(),
                    },
                    &crate::HaltProfile::Long64V1,
                    || false,
                );
                assert_eq!(halted.exit, RunExit::Halted);
                assert_eq!(cpu.rip, CODE + 2);
                let post_hlt = cpu.clone();
                assert!(matches!(
                    deliver_maskable_interrupt(&mut cpu, &mut memory, 14, ring0(), INTEGER_FLAGS),
                    crate::InterruptDeliveryResult::Delivered { .. }
                ));
                let frame_at = cpu.gpr[crate::Register::Rsp as usize] as usize;
                let mut defined = INTEGER_FLAGS;
                let saved_flags = u64::from_le_bytes(
                    memory.bytes[frame_at + 16..frame_at + 24]
                        .try_into()
                        .unwrap(),
                );
                assert_eq!(saved_flags, post_hlt.rflags);
                assert_eq!(
                    step_with_iretq_profile(
                        &mut cpu,
                        &mut memory,
                        ring0(),
                        &mut defined,
                        IretqProfile::SameCpl0ValidFrameV1,
                    ),
                    IretqResult::Retired
                );
                assert_eq!(cpu.rip, CODE + 2);
                assert_eq!(cpu.gpr[crate::Register::Rsp as usize], old_rsp);
                assert_eq!(cpu.cs_selector, post_hlt.cs_selector);
                assert_eq!(cpu.cs_cache, post_hlt.cs_cache);
                assert_eq!(cpu.ss_selector, post_hlt.ss_selector);
                assert_eq!(cpu.ss_cache, post_hlt.ss_cache);
                assert_eq!(cpu.rflags, post_hlt.rflags);
                assert_eq!(cpu.activity, Some(CpuActivity::Running));
                assert_eq!(cpu.sti_shadow, Some(StiShadow::Clear));

                let next = run_with_maskable_request(
                    &mut cpu,
                    &mut memory,
                    RunConfig {
                        instruction_budget: 1,
                        stop_rip: None,
                        context: ring0(),
                    },
                    INTEGER_FLAGS,
                    Some(0x20),
                    || false,
                )
                .unwrap();
                assert!(
                    matches!(next, MaskableRequestRunResult::InterruptEligible { vector: 0x20, rip } if rip == CODE + 2)
                );
                assert_eq!(cpu.rip, CODE + 2);
                assert_eq!(
                    step(&mut cpu, &mut memory, ring0()),
                    Ok(crate::execute::StepOutcome::Advanced)
                );
                assert_eq!(cpu.rip, CODE + 3);
                assert_eq!(cpu.activity, Some(CpuActivity::Running));
            }
        }
    }
}
