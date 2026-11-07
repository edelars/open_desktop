//! Opt-in four-instruction Long64 syscall diagnostic profile.
//! Intel SDM combined revision 093, SYSCALL and SYSRET operation sections.
use crate::{
    CpuActivity, INTEGER_FLAGS, LongModeState, StiShadow,
    memory::{
        AccessContext, AccessOrigin, ArchitecturalFault, GuestMemory, InstructionMode, MemoryError,
        PrivilegeLevel,
    },
    state::{CodeSegmentDescriptorCache, DataSegmentDescriptorCache},
};

/// Integer flags and IF tracked by this profile.
pub const SYSCALL_FLAGS: u64 = INTEGER_FLAGS | (1 << 9);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallState {
    pub efer: u64,
    pub star: u64,
    pub lstar: u64,
    pub fmask: u32,
    /// Defined integer flags saved in R11 by the last SYSCALL.
    pub r11_flags_defined: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallInstruction {
    Syscall,
    KernelStore,
    Sysret,
    UserLoad,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyscallStepExit {
    Retired,
    Unsupported,
    Rejected,
    Fault(ArchitecturalFault),
    Backend(MemoryError),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyscallStepResult {
    pub exit: SyscallStepExit,
    pub rflags_defined: u64,
}

fn canonical(v: u64) -> bool {
    ((v as i64) << 16 >> 16) as u64 == v
}
fn context(state: &LongModeState) -> Option<AccessContext> {
    let s = state.cs_selector?;
    let cache = state.cs_cache?;
    let (privilege, access) = match s & 7 {
        0 => (PrivilegeLevel::Ring0, 0x9b),
        3 => (PrivilegeLevel::Ring3, 0xfb),
        _ => return None,
    };
    if s & !7 == 0
        || cache
            != (CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access,
                flags: 0xa,
            })
    {
        return None;
    }
    Some(AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    })
}
fn set_segments(state: &mut LongModeState, cs: u16, ss: u16, user: bool) {
    state.cs_selector = Some(cs);
    state.ss_selector = Some(ss);
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: if user { 0xfb } else { 0x9b },
        flags: 0xa,
    });
    state.ss_cache = Some(DataSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: if user { 0xf3 } else { 0x93 },
        flags: 0xc,
    });
}
fn fault(vector: u8, error_code: Option<u32>) -> SyscallStepExit {
    SyscallStepExit::Fault(ArchitecturalFault {
        vector,
        error_code,
        linear_address: None,
    })
}
fn memory_exit(error: MemoryError) -> SyscallStepExit {
    match error {
        MemoryError::Architectural(f) => SyscallStepExit::Fault(f),
        other => SyscallStepExit::Backend(other),
    }
}

/// Execute one exact instruction, with privilege derived from the committed CS.
/// Only the four forms of the bounded profile are admitted; no GDT or stack access.
pub fn step(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    defined: u64,
    expected: SyscallInstruction,
) -> SyscallStepResult {
    let result = |exit, rflags_defined| SyscallStepResult {
        exit,
        rflags_defined,
    };
    let Some(msr) = state.syscall else {
        return result(SyscallStepExit::Rejected, defined);
    };
    let Some(ctx) = context(state) else {
        return result(SyscallStepExit::Rejected, defined);
    };
    let kernel_raw = (msr.star >> 32) as u16;
    let kernel = kernel_raw & !3;
    let user_base = (msr.star >> 48) as u16;
    if state.cr0 != Some(0x8001_0033)
        || state.cr4 != Some(0x20)
        || !state.cr3.is_some_and(|x| x < (1u64 << 36))
        || !matches!(msr.efer, 0xd00 | 0xd01)
        || !canonical(msr.lstar)
        || kernel < 8
        || kernel & 7 != 0
        || kernel_raw > 0xfff7
        || user_base & 7 != 0
        || user_base < 8
        || user_base > 0xffe8
        || msr.fmask as u64 & !(SYSCALL_FLAGS & !2) != 0
        || msr.star as u32 != 0
        || state.activity != Some(CpuActivity::Running)
        || state.sti_shadow != Some(StiShadow::Clear)
        || defined & !SYSCALL_FLAGS != 0
        || !canonical(state.rip)
        || state.rflags & (0x100 | 0x10000) != 0
    // Single-step and instruction-breakpoint handling are outside this profile
    {
        return result(SyscallStepExit::Rejected, defined);
    }
    let encoding: &[u8] = match expected {
        SyscallInstruction::Syscall => &[0x0f, 0x05],
        SyscallInstruction::KernelStore => &[0x4c, 0x89, 0x17],
        SyscallInstruction::Sysret => &[0x48, 0x0f, 0x07],
        SyscallInstruction::UserLoad => &[0x48, 0x8b, 0x03],
    };
    if state.rip & 0xfff > 0x1000 - encoding.len() as u64 {
        return result(SyscallStepExit::Unsupported, defined);
    }
    for (offset, expected_byte) in encoding.iter().enumerate() {
        let byte = match memory.fetch(state.rip.wrapping_add(offset as u64), ctx) {
            Ok(b) => b,
            Err(e) => return result(memory_exit(e), defined),
        };
        if byte != *expected_byte {
            return result(SyscallStepExit::Unsupported, defined);
        }
    }
    if expected == SyscallInstruction::Syscall {
        if msr.efer & 1 == 0 {
            return result(fault(6, None), defined);
        }
        if ctx.privilege != PrivilegeLevel::Ring3
            || state.rflags & (0x10000 | 0x4000 | 0x20000 | 0x3000) != 0
        {
            return result(SyscallStepExit::Rejected, defined);
        }
        state.gpr[1] = state.rip.wrapping_add(2);
        state.gpr[11] = state.rflags;
        state.syscall.as_mut().unwrap().r11_flags_defined = Some(defined);
        state.rflags &= !(msr.fmask as u64);
        set_segments(state, kernel, kernel_raw + 8, false);
        state.rip = msr.lstar;
        return result(
            SyscallStepExit::Retired,
            (defined | (msr.fmask as u64 & SYSCALL_FLAGS)) & SYSCALL_FLAGS,
        );
    }
    if expected == SyscallInstruction::Sysret {
        if msr.efer & 1 == 0 {
            return result(fault(6, None), defined);
        }
        if ctx.privilege != PrivilegeLevel::Ring0 || !canonical(state.gpr[1]) {
            return result(fault(13, Some(0)), defined);
        }
        let Some(saved_defined) = msr.r11_flags_defined else {
            return result(SyscallStepExit::Rejected, defined);
        };
        if saved_defined & !SYSCALL_FLAGS != 0 || state.gpr[11] & 0x100 != 0 {
            return result(SyscallStepExit::Rejected, defined);
        }
        state.rip = state.gpr[1];
        state.rflags = (state.gpr[11] & 0x3c_7fd7) | 2;
        set_segments(state, user_base + 16 | 3, user_base + 8 | 3, true);
        return result(SyscallStepExit::Retired, (saved_defined & 0x3c_7fd7) | 2);
    }
    let (addr, is_store) = if expected == SyscallInstruction::KernelStore
        && ctx.privilege == PrivilegeLevel::Ring0
    {
        (state.gpr[7], true)
    } else if expected == SyscallInstruction::UserLoad && ctx.privilege == PrivilegeLevel::Ring3 {
        (state.gpr[3], false)
    } else {
        return result(SyscallStepExit::Unsupported, defined);
    };
    if !canonical(addr) || addr & 0xfff > 0xff8 {
        return result(SyscallStepExit::Rejected, defined);
    }
    if is_store {
        if let Err(e) = memory.write(addr, ctx, &state.gpr[10].to_le_bytes()) {
            return result(memory_exit(e), defined);
        }
    } else {
        let mut value = [0; 8];
        if let Err(e) = memory.read(addr, ctx, &mut value) {
            return result(memory_exit(e), defined);
        }
        state.gpr[0] = u64::from_le_bytes(value);
    }
    state.rip = state.rip.wrapping_add(3);
    result(SyscallStepExit::Retired, defined)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FetchMemory {
        bytes: Vec<u8>,
        calls: Vec<u64>,
        fail_at: Option<usize>,
    }
    impl GuestMemory for FetchMemory {
        fn fetch(&mut self, addr: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.calls.push(addr);
            let n = (addr - 0x1000) as usize;
            if self.fail_at == Some(n) {
                return Err(MemoryError::Backend("fetch failure".into()));
            }
            Ok(self.bytes[n])
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            panic!("operand read")
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            panic!("operand write")
        }
        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("atomic")
        }
        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("atomic")
        }
        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("atomic")
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            panic!("fence")
        }
    }
    fn cpu() -> LongModeState {
        LongModeState {
            rip: 0x1000,
            rflags: 0x202,
            cr0: Some(0x8001_0033),
            cr3: Some(0x1000),
            cr4: Some(0x20),
            activity: Some(CpuActivity::Running),
            sti_shadow: Some(StiShadow::Clear),
            cs_selector: Some(0x23),
            cs_cache: Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0xfb,
                flags: 0xa,
            }),
            syscall: Some(SyscallState {
                efer: 0xd01,
                star: (0x10u64 << 48) | (8u64 << 32),
                lstar: 0x2000,
                fmask: 0x200,
                r11_flags_defined: None,
            }),
            ..Default::default()
        }
    }
    #[test]
    fn predecode_stops_at_every_mismatch_or_fetch_error() {
        for (instruction, encoding) in [
            (SyscallInstruction::Syscall, &[0x0f, 0x05][..]),
            (SyscallInstruction::KernelStore, &[0x4c, 0x89, 0x17][..]),
            (SyscallInstruction::Sysret, &[0x48, 0x0f, 0x07][..]),
            (SyscallInstruction::UserLoad, &[0x48, 0x8b, 0x03][..]),
        ] {
            for offset in 0..encoding.len() {
                for failure in [false, true] {
                    let mut bytes = encoding.to_vec();
                    if !failure {
                        bytes[offset] ^= 1;
                    }
                    let mut memory = FetchMemory {
                        bytes,
                        calls: Vec::new(),
                        fail_at: failure.then_some(offset),
                    };
                    let mut state = cpu();
                    let before = state.clone();
                    let outcome = step(&mut state, &mut memory, SYSCALL_FLAGS, instruction);
                    if failure {
                        assert!(matches!(outcome.exit, SyscallStepExit::Backend(_)));
                    } else {
                        assert_eq!(outcome.exit, SyscallStepExit::Unsupported);
                    }
                    assert_eq!(state, before);
                    assert_eq!(memory.calls.len(), offset + 1);
                }
            }
        }
    }
    #[test]
    fn syscall_sysret_flags_and_selector_admission() {
        let mut state = cpu();
        state.rflags = 0x8a02;
        state.syscall.as_mut().unwrap().star |= 3u64 << 32;
        let mut memory = FetchMemory {
            bytes: vec![0x0f, 0x05],
            calls: vec![],
            fail_at: None,
        };
        let defined = SYSCALL_FLAGS & !(1 << 9);
        let result = step(
            &mut state,
            &mut memory,
            defined,
            SyscallInstruction::Syscall,
        );
        assert_eq!(result.exit, SyscallStepExit::Retired);
        assert_eq!(state.cs_selector, Some(8));
        assert_eq!(state.ss_selector, Some(0x13));
        assert_eq!(state.gpr[11], 0x8a02);
        assert_eq!(state.rflags & 0x200, 0);
        assert_ne!(result.rflags_defined & 0x200, 0);
        state.rip = 0x1000;
        memory.bytes = vec![0x48, 0x0f, 0x07];
        let result = step(
            &mut state,
            &mut memory,
            result.rflags_defined,
            SyscallInstruction::Sysret,
        );
        assert_eq!(result.exit, SyscallStepExit::Retired);
        assert_eq!(state.rflags & 0x8a00, 0x0a00);
        assert_ne!(result.rflags_defined & 0x800, 0);
        assert_eq!(result.rflags_defined & 0x200, 0);
    }

    #[test]
    fn sysret_rejects_invalid_saved_definedness_and_sce_disabled() {
        let mut state = cpu();
        set_segments(&mut state, 8, 0x10, false);
        state.gpr[1] = 0x1000;
        state.syscall.as_mut().unwrap().r11_flags_defined = Some(1 << 18);
        let mut memory = FetchMemory {
            bytes: vec![0x48, 0x0f, 0x07],
            calls: vec![],
            fail_at: None,
        };
        let before = state.clone();
        assert_eq!(
            step(
                &mut state,
                &mut memory,
                SYSCALL_FLAGS,
                SyscallInstruction::Sysret
            )
            .exit,
            SyscallStepExit::Rejected
        );
        assert_eq!(state, before);
        state.syscall.as_mut().unwrap().efer = 0xd00;
        let before = state.clone();
        assert!(
            matches!(step(&mut state, &mut memory, SYSCALL_FLAGS, SyscallInstruction::Sysret).exit, SyscallStepExit::Fault(f) if f.vector == 6)
        );
        assert_eq!(state, before);
    }
    #[test]
    fn resume_flag_is_rejected_before_fetch_without_partial_commit() {
        for instruction in [
            SyscallInstruction::Syscall,
            SyscallInstruction::KernelStore,
            SyscallInstruction::Sysret,
            SyscallInstruction::UserLoad,
        ] {
            let mut state = cpu();
            if matches!(
                instruction,
                SyscallInstruction::KernelStore | SyscallInstruction::Sysret
            ) {
                set_segments(&mut state, 8, 0x10, false);
            }
            state.rflags |= 0x10000;
            let before = state.clone();
            let mut memory = FetchMemory {
                bytes: vec![],
                calls: vec![],
                fail_at: None,
            };
            let result = step(&mut state, &mut memory, SYSCALL_FLAGS, instruction);
            assert_eq!(result.exit, SyscallStepExit::Rejected);
            assert_eq!(result.rflags_defined, SYSCALL_FLAGS);
            assert_eq!(state, before);
            assert!(memory.calls.is_empty());
        }
    }
    #[test]
    fn trap_flag_is_rejected_without_partial_commit() {
        for instruction in [
            SyscallInstruction::Syscall,
            SyscallInstruction::KernelStore,
            SyscallInstruction::Sysret,
            SyscallInstruction::UserLoad,
        ] {
            let mut state = cpu();
            if matches!(
                instruction,
                SyscallInstruction::KernelStore | SyscallInstruction::Sysret
            ) {
                set_segments(&mut state, 8, 0x10, false);
            }
            state.rflags |= 0x100;
            let before = state.clone();
            let mut memory = FetchMemory {
                bytes: vec![],
                calls: vec![],
                fail_at: None,
            };
            let result = step(&mut state, &mut memory, SYSCALL_FLAGS, instruction);
            assert_eq!(result.exit, SyscallStepExit::Rejected);
            assert_eq!(result.rflags_defined, SYSCALL_FLAGS);
            assert_eq!(state, before);
            assert!(memory.calls.is_empty());
        }

        let mut state = cpu();
        set_segments(&mut state, 8, 0x10, false);
        state.gpr[1] = 0x1000;
        state.gpr[11] = 0x302;
        state.syscall.as_mut().unwrap().r11_flags_defined = Some(SYSCALL_FLAGS);
        let before = state.clone();
        let mut memory = FetchMemory {
            bytes: vec![0x48, 0x0f, 0x07],
            calls: vec![],
            fail_at: None,
        };
        let result = step(
            &mut state,
            &mut memory,
            SYSCALL_FLAGS,
            SyscallInstruction::Sysret,
        );
        assert_eq!(result.exit, SyscallStepExit::Rejected);
        assert_eq!(result.rflags_defined, SYSCALL_FLAGS);
        assert_eq!(state, before);
        assert_eq!(memory.calls, [0x1000, 0x1001, 0x1002]);
    }
}
