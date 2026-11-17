//! The step from [`ProtectedCpu`] (the AP trampoline model, stopped at
//! [`crate::protected_exec::ProtectedRefusal::Mode64TargetEntered`]) to a [`LongModeState`] that
//! the ordinary long-mode interpreter runs (S6-16b, CPU_CONTRACT.md "S6-16b").
//!
//! This is a pure conversion: it reads the [`ProtectedCpu`] and the state the processor kept over
//! INIT and builds a new state; nothing is executed and no memory is touched. It lives in its own
//! module so that `protected_exec.rs` (the trampoline executor, 4 000+ lines) keeps one job.
//!
//! Sources: Intel SDM Vol. 3A (long mode, segment registers in 64-bit mode, Table 12-1 INIT
//! state), Vol. 1 section 3.4.1.1, all from memory: the text is not in the repository.

use crate::protected_exec::{EFER_LMA, ProtectedCpu};
use crate::state::{
    CodeSegmentDescriptorCache, CpuActivity, DataSegmentDescriptorCache, FsDescriptorCache, Gdtr,
    Idtr, LongModeState, StiShadow,
};

/// `ProtectedSegment::flags` bit L (64-bit code segment).
const FLAG_L: u8 = 0x2;
/// `ProtectedSegment::flags` bit D/B (default operand size; must be 0 with L = 1).
const FLAG_D: u8 = 0x4;
/// The effective limit of a Long64 code segment ([`CodeSegmentDescriptorCache::limit`]).
const LONG_CODE_LIMIT: u32 = 0xFFFF_FFFF;

/// Why [`ProtectedCpu::into_long_mode`] built no state. Checked in this order; the processor
/// model is only borrowed, so no refusal changes anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LongEntryRefusal {
    /// `target_entered` is false: the block 8 jump has not retired, so `RIP` is not the 64-bit
    /// target.
    TargetNotEntered,
    /// `EFER.LMA` is clear: the processor is not in IA-32e mode.
    LongModeNotActive { efer: u64 },
    /// `CS.L` is clear: the code segment is a compatibility-mode segment.
    CodeSegmentNotLong { cs_flags: u8 },
    /// `CS.D` is set together with `L`: the reserved combination (`#GP` on a far transfer).
    CodeSegmentDefaultSizeSet { cs_flags: u8 },
}

impl ProtectedCpu {
    /// The [`LongModeState`] of this processor at the 64-bit target: the state a long-mode
    /// interpreter continues from (field table in CPU_CONTRACT.md "S6-16b").
    ///
    /// `preserved` is the state the processor kept over INIT (an application processor:
    /// `LongModeState::default()` or what the board carried). Only the values INIT leaves alone are
    /// taken from it: `syscall` (STAR/LSTAR/FMASK; its `efer` is replaced by [`ProtectedCpu::efer`]),
    /// `kernel_gs_base`, `x87` and `sse`. Everything else is built from `self` or is the
    /// [`LongModeState::default`] value, so a stale register of `preserved` never leaks.
    ///
    /// `EFER` has no field in [`LongModeState`]: the caller keeps `self.efer`.
    pub fn into_long_mode(
        &self,
        preserved: &LongModeState,
    ) -> Result<LongModeState, LongEntryRefusal> {
        if !self.target_entered {
            return Err(LongEntryRefusal::TargetNotEntered);
        }
        if self.efer & EFER_LMA == 0 {
            return Err(LongEntryRefusal::LongModeNotActive { efer: self.efer });
        }
        if self.cs.flags & FLAG_L == 0 {
            return Err(LongEntryRefusal::CodeSegmentNotLong {
                cs_flags: self.cs.flags,
            });
        }
        if self.cs.flags & FLAG_D != 0 {
            return Err(LongEntryRefusal::CodeSegmentDefaultSizeSet {
                cs_flags: self.cs.flags,
            });
        }

        let mut gpr = [0u64; 16];
        for (slot, value) in gpr.iter_mut().zip(self.gpr) {
            *slot = u64::from(value);
        }
        let data = |seg: &crate::legacy_mode::ProtectedSegment| DataSegmentDescriptorCache {
            base: seg.base,
            limit: seg.limit,
            access: seg.access,
            flags: seg.flags,
        };
        let fs_like = |seg: &crate::legacy_mode::ProtectedSegment| FsDescriptorCache {
            limit: seg.limit,
            access: seg.access,
            flags: seg.flags,
        };

        Ok(LongModeState {
            gpr,
            rip: self.rip(),
            rflags: u64::from(self.eflags),
            cr0: Some(u64::from(self.cr0)),
            cr2: Some(0),
            cr3: Some(u64::from(self.cr3)),
            cr4: Some(u64::from(self.cr4)),
            syscall: preserved.syscall.map(|mut s| {
                s.efer = self.efer;
                s
            }),
            fs_base: u64::from(self.fs.base),
            gs_base: u64::from(self.gs.base),
            kernel_gs_base: preserved.kernel_gs_base,
            fs_selector: self.fs.selector,
            gs_selector: self.gs.selector,
            ds_selector: self.ds.selector,
            es_selector: self.es.selector,
            cs_selector: Some(self.cs.selector),
            cs_cache: Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: LONG_CODE_LIMIT,
                access: self.cs.access,
                flags: self.cs.flags,
            }),
            ss_selector: Some(self.ss.selector),
            ss_cache: Some(data(&self.ss)),
            ds_cache: Some(data(&self.ds)),
            es_cache: Some(data(&self.es)),
            fs_cache: Some(fs_like(&self.fs)),
            gs_cache: Some(fs_like(&self.gs)),
            gdtr: Some(Gdtr {
                base: u64::from(self.gdtr.base),
                limit: self.gdtr.limit,
            }),
            idtr: Some(Idtr {
                base: u64::from(self.idtr.base),
                limit: self.idtr.limit,
            }),
            x87: preserved.x87.clone(),
            sse: preserved.sse.clone(),
            sti_shadow: Some(if self.mov_ss_shadow {
                StiShadow::Blocking
            } else {
                StiShadow::Clear
            }),
            activity: Some(CpuActivity::Running),
            ..LongModeState::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::{
        ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, MovCr3Long64V1, RunConfig,
        RunExit, run_with_cpl0_profile_set,
    };
    use crate::legacy_mode::ProtectedSegment;
    use crate::memory::{AccessContext, AccessKind, GuestMemory, MemoryError, PrivilegeLevel};
    use crate::real_mode_exec::TableRegister;
    use crate::state::{SseState, TaskRegisterCache, X87State};
    use crate::syscall::SyscallState;

    const TARGET: u64 = 0xFFFF_FFFF_8139_C090;

    fn seg(selector: u16, base: u32, limit: u32, access: u8, flags: u8) -> ProtectedSegment {
        ProtectedSegment {
            selector,
            base,
            limit,
            access,
            flags,
        }
    }

    /// The state the S6-14n path leaves: 64-bit CS (selector 0x10, `L = 1`), flat data selector
    /// 0x18, LMA set, `RIP` at `secondary_startup_64`. Values are distinct per register so that a
    /// mixed-up field shows in the table test.
    fn at_target() -> ProtectedCpu {
        ProtectedCpu {
            gpr: [
                0x1111_1111,
                0x2222_2222,
                0x0003_0018,
                0x4444_4444,
                0x5555_5555,
                0x6666_6666,
                0x7777_7777,
                0x8888_8888,
            ],
            eip: 0x8139_C090,
            rip_high: 0xFFFF_FFFF,
            target_entered: true,
            eflags: 0x0000_0046,
            cr0: 0x8005_0033,
            cr3: 0x0000_4000,
            cr4: 0x0000_00A0,
            efer: 0x0000_0D01,
            cs: seg(0x10, 0, 0xFFFF_FFFF, 0x9B, 0xA),
            ds: seg(0x18, 0x1000, 0xFFFF_FFFF, 0x93, 0xC),
            es: seg(0x18, 0x2000, 0xFFFF_FFFF, 0x93, 0xC),
            ss: seg(0x18, 0x3000, 0xFFFF_FFFF, 0x93, 0xC),
            fs: seg(0x18, 0x4000, 0x000F_FFFF, 0x93, 0x4),
            gs: seg(0x2B, 0x5000, 0xFFFF_FFFF, 0x93, 0xC),
            gdtr: TableRegister {
                base: 0x0000_6000,
                limit: 0x001F,
            },
            idtr: TableRegister {
                base: 0x0000_7000,
                limit: 0x0FFF,
            },
            mov_ss_shadow: false,
        }
    }

    /// What INIT preserves: distinctive values in every carried field, junk in the others.
    fn preserved() -> LongModeState {
        LongModeState {
            gpr: [0xDEAD; 16],
            rip: 0xBAD,
            cr0: Some(0xBAD),
            cr2: Some(0xBAD),
            cr3: Some(0xBAD),
            cr4: Some(0xBAD),
            syscall: Some(SyscallState {
                efer: 0xBAD0_0000,
                star: 0x0023_0010_0000_0000,
                lstar: 0xFFFF_FFFF_8100_0000,
                fmask: 0x4_7700,
                r11_flags_defined: Some(0x246),
            }),
            fs_base: 0xBAD,
            gs_base: 0xBAD,
            kernel_gs_base: Some(0xFFFF_8880_0000_0000),
            tr: Some(TaskRegisterCache {
                selector: 0x40,
                base: 0x1000,
                limit: 0x67,
                access: 0x8B,
                flags: 0,
            }),
            ldtr_null_selector: Some(0),
            debug_registers: [Some(1); 8],
            x87: Some(X87State::fixture_seed()),
            sse: Some(SseState {
                xmm: [[0xA5; 16]; 16],
                mxcsr: 0x1F80,
            }),
            nmi_blocked: true,
            activity: Some(CpuActivity::WaitForSipi),
            sti_shadow: Some(StiShadow::Blocking),
            ..LongModeState::default()
        }
    }

    /// Expected table (SDM, by memory; written by hand from `at_target`, not from model output).
    #[test]
    fn field_table() {
        let cpu = at_target();
        let before = cpu;
        let state = cpu.into_long_mode(&preserved()).unwrap();
        assert_eq!(cpu, before, "the processor model is only borrowed");

        // GPR 0..7: the 32-bit values zero-extended; R8..R15 zero.
        assert_eq!(
            state.gpr,
            [
                0x1111_1111,
                0x2222_2222,
                0x0003_0018,
                0x4444_4444,
                0x5555_5555,
                0x6666_6666,
                0x7777_7777,
                0x8888_8888,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0
            ]
        );
        assert_eq!(state.rip, 0xFFFF_FFFF_8139_C090);
        assert_eq!(state.rflags, 0x46);
        assert_eq!(
            (state.cr0, state.cr2, state.cr3, state.cr4),
            (Some(0x8005_0033), Some(0), Some(0x4000), Some(0xA0))
        );
        // Segment selectors and hidden caches. CS: base 0, limit 4 GiB - 1, access and flags kept.
        assert_eq!(state.cs_selector, Some(0x10));
        assert_eq!(
            state.cs_cache,
            Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: 0xFFFF_FFFF,
                access: 0x9B,
                flags: 0xA
            })
        );
        assert_eq!(state.ss_selector, Some(0x18));
        assert_eq!(
            state.ss_cache,
            Some(DataSegmentDescriptorCache {
                base: 0x3000,
                limit: 0xFFFF_FFFF,
                access: 0x93,
                flags: 0xC
            })
        );
        assert_eq!((state.ds_selector, state.es_selector), (0x18, 0x18));
        assert_eq!(
            state.ds_cache,
            Some(DataSegmentDescriptorCache {
                base: 0x1000,
                limit: 0xFFFF_FFFF,
                access: 0x93,
                flags: 0xC
            })
        );
        assert_eq!(
            state.es_cache,
            Some(DataSegmentDescriptorCache {
                base: 0x2000,
                limit: 0xFFFF_FFFF,
                access: 0x93,
                flags: 0xC
            })
        );
        // FS/GS: selector and cache without base; the bases are in fs_base/gs_base (u32 -> u64).
        assert_eq!((state.fs_selector, state.gs_selector), (0x18, 0x2B));
        assert_eq!(
            state.fs_cache,
            Some(FsDescriptorCache {
                limit: 0x000F_FFFF,
                access: 0x93,
                flags: 0x4
            })
        );
        assert_eq!(
            state.gs_cache,
            Some(FsDescriptorCache {
                limit: 0xFFFF_FFFF,
                access: 0x93,
                flags: 0xC
            })
        );
        assert_eq!((state.fs_base, state.gs_base), (0x4000, 0x5000));
        assert!(!(state.ds_unusable || state.es_unusable || state.ss_unusable));
        assert!(!(state.fs_unusable || state.gs_unusable));
        assert_eq!(
            state.gdtr,
            Some(Gdtr {
                base: 0x6000,
                limit: 0x1F
            })
        );
        assert_eq!(
            state.idtr,
            Some(Idtr {
                base: 0x7000,
                limit: 0xFFF
            })
        );
        // Not represented: TR, LDTR, debug registers (CPU_CONTRACT.md "S6-16b").
        assert_eq!(state.tr, None);
        assert_eq!(state.ldtr_null_selector, None);
        assert_eq!(state.debug_registers, [None; 8]);
        // Execution state.
        assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
        assert_eq!(state.activity, Some(CpuActivity::Running));
        assert!(!state.nmi_blocked);
    }

    #[test]
    fn what_init_keeps_comes_from_preserved_and_the_efer_of_syscall_from_the_cpu() {
        let preserved = preserved();
        let state = at_target().into_long_mode(&preserved).unwrap();
        assert_eq!(state.kernel_gs_base, Some(0xFFFF_8880_0000_0000));
        assert_eq!(state.x87, Some(X87State::fixture_seed()));
        assert_eq!(
            state.sse,
            Some(SseState {
                xmm: [[0xA5; 16]; 16],
                mxcsr: 0x1F80
            })
        );
        // STAR/LSTAR/FMASK kept; the stale EFER of `preserved` is replaced by the CPU's.
        assert_eq!(
            state.syscall,
            Some(SyscallState {
                efer: 0xD01,
                star: 0x0023_0010_0000_0000,
                lstar: 0xFFFF_FFFF_8100_0000,
                fmask: 0x4_7700,
                r11_flags_defined: Some(0x246),
            })
        );
        // Nothing else of `preserved` leaks.
        let default = LongModeState::default();
        let state_default = at_target().into_long_mode(&default).unwrap();
        assert_eq!(
            LongModeState {
                syscall: None,
                kernel_gs_base: None,
                x87: None,
                sse: None,
                ..state
            },
            state_default
        );
        assert_eq!(state_default.syscall, None);
        assert_eq!(state_default.kernel_gs_base, None);
    }

    #[test]
    fn a_code_segment_base_and_limit_are_normalised_a_data_base_is_kept() {
        let mut cpu = at_target();
        cpu.cs = seg(0x10, 0x1000, 0x0000_FFFF, 0x9B, 0x2);
        let state = cpu.into_long_mode(&LongModeState::default()).unwrap();
        assert_eq!(
            state.cs_cache,
            Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: 0xFFFF_FFFF,
                access: 0x9B,
                flags: 0x2
            })
        );
    }

    #[test]
    fn a_mov_ss_shadow_is_an_interrupt_shadow() {
        let mut cpu = at_target();
        cpu.mov_ss_shadow = true;
        let state = cpu.into_long_mode(&LongModeState::default()).unwrap();
        assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
    }

    #[test]
    fn every_refusal_leaves_the_cpu_unchanged_and_is_checked_in_order() {
        let default = LongModeState::default();
        let check = |cpu: ProtectedCpu, expected: LongEntryRefusal| {
            let before = cpu;
            assert_eq!(cpu.into_long_mode(&default), Err(expected));
            assert_eq!(cpu, before);
        };

        let mut cpu = at_target();
        cpu.target_entered = false;
        check(cpu, LongEntryRefusal::TargetNotEntered);

        let mut cpu = at_target();
        cpu.efer = 0x0000_0101; // LME without LMA
        check(cpu, LongEntryRefusal::LongModeNotActive { efer: 0x101 });

        let mut cpu = at_target();
        cpu.cs.flags = 0x8; // L = 0 (compatibility mode, D = 0)
        check(cpu, LongEntryRefusal::CodeSegmentNotLong { cs_flags: 0x8 });

        let mut cpu = at_target();
        cpu.cs.flags = 0x4; // L = 0, D = 1: the 32-bit compatibility segment, still not long
        check(cpu, LongEntryRefusal::CodeSegmentNotLong { cs_flags: 0x4 });

        let mut cpu = at_target();
        cpu.cs.flags = 0xE; // L = 1 and D = 1
        check(
            cpu,
            LongEntryRefusal::CodeSegmentDefaultSizeSet { cs_flags: 0xE },
        );

        // Order: target_entered, then LMA, then L, then D.
        let mut cpu = at_target();
        cpu.target_entered = false;
        cpu.efer = 0;
        cpu.cs.flags = 0xE;
        check(cpu, LongEntryRefusal::TargetNotEntered);
        cpu.target_entered = true;
        check(cpu, LongEntryRefusal::LongModeNotActive { efer: 0 });
        cpu.efer = EFER_LMA;
        cpu.cs.flags = 0x4;
        check(cpu, LongEntryRefusal::CodeSegmentNotLong { cs_flags: 0x4 });
    }

    /// Identity-mapped code memory at `TARGET`-independent address `CODE`.
    struct Code {
        base: u64,
        bytes: Vec<u8>,
    }

    impl GuestMemory for Code {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            address
                .checked_sub(self.base)
                .and_then(|o| self.bytes.get(o as usize).copied())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            unreachable!("no data read")
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            unreachable!("no data write")
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

    /// The converted state is admitted by the CPL0 profile set of the board interpreter (CR0/CR4
    /// inside the policy, CR3 owned by the set, `Running`, STI shadow `Clear`, no x87/SSE) and
    /// runs from the high target address: `mov eax, 0x1234` (`b8 imm32`) and `mov r9, rax`
    /// (`49 89 c1`), 2 instructions, identity memory.
    #[test]
    fn the_converted_state_runs_through_the_ordinary_interpreter() {
        // The x87/SSE state would be refused by the set's admission (the board keeps it in a
        // guest cell); the other fields of `preserved` are carried.
        let mut kept = preserved();
        kept.x87 = None;
        kept.sse = None;
        let mut state = at_target().into_long_mode(&kept).unwrap();
        let mut memory = Code {
            base: TARGET,
            // Padded with NOPs: the decoder fetches up to 15 bytes ahead.
            bytes: [0xB8, 0x34, 0x12, 0x00, 0x00, 0x49, 0x89, 0xC1]
                .into_iter()
                .chain([0x90; 24])
                .collect(),
        };
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cpu_owned_cr3(MovCr3Long64V1);
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 2,
                stop_rip: None,
                context: AccessContext {
                    privilege: PrivilegeLevel::Ring0,
                    ..AccessContext::default()
                },
            },
            crate::INTEGER_FLAGS,
            &set,
            None,
            || false,
        )
        .expect("the converted state is admitted");
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
        );
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(state.rip, TARGET + 8);
        assert_eq!(state.gpr[0], 0x1234, "mov eax, imm32 zero-extends");
        assert_eq!(state.gpr[9], 0x1234);
        // The segment and control state is untouched by the instructions.
        assert_eq!(state.cs_selector, Some(0x10));
        assert_eq!(state.cr3, Some(0x4000));
    }
}
