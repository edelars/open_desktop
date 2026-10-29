/// Explicit admission for the bounded Long64 HLT instruction slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HaltProfile {
    Long64V1,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::{
        MaskableRequestRunResult, RunConfig, RunExit, RunResult, StepOutcome,
        run_with_halt_profile, run_with_maskable_request, run_with_rep_continuation, step,
        step_with_halt_profile,
    };
    use crate::memory::{AccessContext, GuestMemory, MemoryError, PrivilegeLevel};
    use crate::{CpuActivity, LongModeState, StiShadow};

    const RIP: u64 = 0x1000;

    struct Memory {
        bytes: [u8; 8],
        fetches: usize,
        reads: usize,
        writes: usize,
    }

    impl Memory {
        fn code(bytes: &[u8]) -> Self {
            let mut memory = Self {
                bytes: [0; 8],
                fetches: 0,
                reads: 0,
                writes: 0,
            };
            memory.bytes[..bytes.len()].copy_from_slice(bytes);
            memory
        }
    }

    impl GuestMemory for Memory {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.fetches += 1;
            self.bytes
                .get(address.checked_sub(RIP).unwrap_or(u64::MAX) as usize)
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: crate::memory::AccessKind::Fetch,
                })
        }

        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            self.reads += 1;
            unreachable!("HLT does not read data")
        }

        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            self.writes += 1;
            unreachable!("HLT does not write data")
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
            rip: RIP,
            rflags: 0x2 | (1 << 9) | (1 << 10) | (1 << 0),
            sti_shadow: Some(StiShadow::Clear),
            activity: Some(CpuActivity::Running),
            gpr: [0x1234; 16],
            ..LongModeState::default()
        }
    }

    fn config(budget: u64, stop_rip: Option<u64>, context: AccessContext) -> RunConfig {
        RunConfig {
            instruction_budget: budget,
            stop_rip,
            context,
        }
    }

    fn ring0() -> AccessContext {
        AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        }
    }

    #[test]
    fn exact_hlt_retires_then_halts_before_stop_or_budget_boundary() {
        let mut cpu = state();
        let before = cpu.clone();
        let mut memory = Memory::code(&[0xf4]);
        let result = run_with_halt_profile(
            &mut cpu,
            &mut memory,
            config(1, Some(RIP + 1), ring0()),
            &HaltProfile::Long64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::Halted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(cpu.rip, RIP + 1);
        assert_eq!(cpu.gpr, before.gpr);
        assert_eq!(cpu.rflags, before.rflags);
        assert_eq!(cpu.activity, Some(CpuActivity::Halted));
        assert_eq!(cpu.sti_shadow, Some(StiShadow::Clear));
        assert_eq!(memory.fetches, 1);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }

    #[test]
    fn hlt_consumes_only_a_successful_sti_shadow_transition() {
        let mut cpu = state();
        cpu.sti_shadow = Some(StiShadow::Blocking);
        let mut memory = Memory::code(&[0xf4]);
        assert_eq!(
            step_with_halt_profile(&mut cpu, &mut memory, ring0(), &HaltProfile::Long64V1).unwrap(),
            StepOutcome::Advanced
        );
        assert_eq!(cpu.activity, Some(CpuActivity::Halted));
        assert_eq!(cpu.sti_shadow, Some(StiShadow::Clear));

        let mut faulting = state();
        faulting.sti_shadow = Some(StiShadow::Blocking);
        let before = faulting.clone();
        let mut memory = Memory::code(&[0xf4]);
        assert_eq!(
            step_with_halt_profile(
                &mut faulting,
                &mut memory,
                AccessContext {
                    privilege: PrivilegeLevel::Ring3,
                    ..AccessContext::default()
                },
                &HaltProfile::Long64V1,
            )
            .unwrap(),
            StepOutcome::Fault(crate::memory::ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(faulting, before);
    }

    #[test]
    fn step_halt_profile_rejects_non_hlt_without_data_access_or_retirement() {
        for (bytes, opcode) in [(&[0x90][..], 0x90), (&[0x48, 0x8b, 0x00][..], 0x8b)] {
            let mut cpu = state();
            cpu.sti_shadow = Some(StiShadow::Blocking);
            let before = cpu.clone();
            let mut memory = Memory::code(bytes);
            assert_eq!(
                step_with_halt_profile(&mut cpu, &mut memory, ring0(), &HaltProfile::Long64V1,)
                    .unwrap(),
                StepOutcome::Unsupported { opcode }
            );
            assert_eq!(cpu, before);
            assert_eq!(memory.fetches, bytes.len());
            assert_eq!((memory.reads, memory.writes), (0, 0));
        }
    }

    #[test]
    fn sti_with_if_already_set_can_halt_without_creating_shadow() {
        let mut cpu = state();
        assert_ne!(cpu.rflags & (1 << 9), 0);
        let mut memory = Memory::code(&[0xfb, 0xf4]);
        let result = run_with_halt_profile(
            &mut cpu,
            &mut memory,
            config(2, None, ring0()),
            &HaltProfile::Long64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::Halted);
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(cpu.rip, RIP + 2);
        assert_eq!(cpu.activity, Some(CpuActivity::Halted));
        assert_eq!(cpu.sti_shadow, Some(StiShadow::Clear));
        assert_eq!(memory.fetches, 2);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }

    #[test]
    fn repeated_sti_with_if_already_set_is_outside_the_halt_sequence() {
        let mut cpu = state();
        let mut memory = Memory::code(&[0xfb, 0xfb, 0xf4]);
        let result = run_with_halt_profile(
            &mut cpu,
            &mut memory,
            config(3, None, ring0()),
            &HaltProfile::Long64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xfb });
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(cpu.rip, RIP + 1);
        assert_eq!(cpu.activity, Some(CpuActivity::Running));
        assert_eq!(cpu.sti_shadow, Some(StiShadow::Clear));
        assert_eq!(memory.fetches, 2);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }

    #[test]
    fn hlt_exact_prefix_and_privilege_outcomes_are_atomic() {
        for (bytes, context, expected, fetches) in [
            (&[0xf4][..], ring0(), RunExit::Halted, 1),
            (
                &[0xf4][..],
                AccessContext {
                    privilege: PrivilegeLevel::Ring3,
                    ..AccessContext::default()
                },
                RunExit::Fault(crate::memory::ArchitecturalFault {
                    vector: 13,
                    error_code: Some(0),
                    linear_address: None,
                }),
                1,
            ),
            (
                &[0xf0, 0xf4][..],
                ring0(),
                RunExit::Fault(crate::memory::ArchitecturalFault {
                    vector: 6,
                    error_code: None,
                    linear_address: None,
                }),
                2,
            ),
        ] {
            let mut cpu = state();
            let before = cpu.clone();
            let mut memory = Memory::code(bytes);
            let result = run_with_halt_profile(
                &mut cpu,
                &mut memory,
                config(1, None, context),
                &HaltProfile::Long64V1,
                || false,
            );
            assert_eq!(result.exit, expected);
            assert_eq!(
                result.instructions_executed,
                u64::from(expected == RunExit::Halted)
            );
            assert_eq!(memory.fetches, fetches);
            if expected != RunExit::Halted {
                assert_eq!(cpu, before);
            }
        }

        let mut cpu = state();
        let mut memory = Memory::code(&[0x66, 0xf4]);
        let result = run_with_halt_profile(
            &mut cpu,
            &mut memory,
            config(1, None, ring0()),
            &HaltProfile::Long64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xf4 });
        assert_eq!(memory.fetches, 2);
        assert_eq!(cpu, state());

        let mut legacy = LongModeState {
            cr4: None,
            rip: RIP,
            ..LongModeState::default()
        };
        let mut memory = Memory::code(&[0xf4]);
        let result = run_with_halt_profile(
            &mut legacy,
            &mut memory,
            config(1, None, ring0()),
            &HaltProfile::Long64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xf4 });
        assert_eq!(memory.fetches, 0);
    }

    #[test]
    fn stopped_halted_reentry_step_and_rep_continuation_do_not_fetch() {
        let mut cpu = state();
        let mut memory = Memory::code(&[0xf4, 0x90]);
        let first = run_with_halt_profile(
            &mut cpu,
            &mut memory,
            config(1, None, ring0()),
            &HaltProfile::Long64V1,
            || false,
        );
        assert_eq!(first.exit, RunExit::Halted);
        let reads_before = memory.fetches;
        let resumed = run_with_halt_profile(
            &mut cpu,
            &mut memory,
            config(4, None, ring0()),
            &HaltProfile::Long64V1,
            || false,
        );
        assert_eq!(resumed.exit, RunExit::Halted);
        assert_eq!(resumed.instructions_executed, 0);
        assert_eq!(memory.fetches, reads_before);
        assert_eq!(
            step(&mut cpu, &mut memory, ring0()).unwrap(),
            StepOutcome::Halted
        );
        assert_eq!(
            step_with_halt_profile(&mut cpu, &mut memory, ring0(), &HaltProfile::Long64V1).unwrap(),
            StepOutcome::Halted
        );
        assert_eq!(memory.fetches, reads_before);
        let mut continuation = None;
        let rep = run_with_rep_continuation(
            &mut cpu,
            &mut memory,
            config(4, None, ring0()),
            crate::INTEGER_FLAGS,
            &mut continuation,
            || false,
        )
        .unwrap();
        assert_eq!(rep.exit, RunExit::Halted);
        assert_eq!(memory.fetches, reads_before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }

    #[test]
    fn eligible_request_does_not_wake_but_if_zero_stays_halted() {
        let mut cpu = state();
        cpu.activity = Some(CpuActivity::Halted);
        let mut memory = Memory::code(&[0x90]);
        let eligible = run_with_maskable_request(
            &mut cpu,
            &mut memory,
            config(1, None, ring0()),
            crate::INTEGER_FLAGS,
            Some(0x20),
            || false,
        )
        .unwrap();
        assert_eq!(
            eligible,
            MaskableRequestRunResult::InterruptEligible {
                vector: 0x20,
                rip: RIP,
            }
        );
        assert_eq!(cpu.activity, Some(CpuActivity::Halted));
        assert_eq!(memory.fetches, 0);

        cpu.rflags &= !(1 << 9);
        let masked = run_with_maskable_request(
            &mut cpu,
            &mut memory,
            config(1, None, ring0()),
            crate::INTEGER_FLAGS,
            Some(0x20),
            || false,
        )
        .unwrap();
        assert!(matches!(
            masked,
            MaskableRequestRunResult::Run(RunResult {
                exit: RunExit::Halted,
                instructions_executed: 0,
                ..
            })
        ));
        assert_eq!(cpu.activity, Some(CpuActivity::Halted));
        assert_eq!(memory.fetches, 0);

        cpu.sti_shadow = None;
        assert_eq!(
            run_with_maskable_request(
                &mut cpu,
                &mut memory,
                config(1, None, ring0()),
                crate::INTEGER_FLAGS,
                Some(0x20),
                || false,
            ),
            Err(crate::execute::MaskableRequestRunInputError::InvalidHaltedState)
        );
    }
}
