/// Explicit admission for the bounded Long64 non-PVI STI transition and its
/// one-instruction maskable-interrupt inhibition state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StiProfile {
    /// Long64, CR0.PE=1, RFLAGS.VM=0, CR4.PVI=0, without VMX or SMM.
    NonPviLong64V1,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::{RunConfig, RunExit, run_with_sti_profile};
    use crate::memory::{AccessContext, GuestMemory, MemoryError};
    use crate::{LongModeState, StiShadow};

    struct Code(&'static [u8], usize);

    impl GuestMemory for Code {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            let at = address as usize;
            self.1 += 1;
            self.0.get(at).copied().ok_or(MemoryError::Unmapped {
                address,
                access: crate::memory::AccessKind::Fetch,
            })
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            unreachable!()
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

    fn context(cpl: u8) -> AccessContext {
        AccessContext {
            privilege: match cpl {
                0 => crate::memory::PrivilegeLevel::Ring0,
                1 => crate::memory::PrivilegeLevel::Ring1,
                2 => crate::memory::PrivilegeLevel::Ring2,
                _ => crate::memory::PrivilegeLevel::Ring3,
            },
            ..AccessContext::default()
        }
    }

    fn run(
        code: &'static [u8],
        state: &mut LongModeState,
        budget: u64,
        cpl: u8,
    ) -> crate::execute::RunResult {
        run_with_sti_profile(
            state,
            &mut Code(code, 0),
            RunConfig {
                instruction_budget: budget,
                stop_rip: None,
                context: context(cpl),
            },
            &StiProfile::NonPviLong64V1,
            || false,
        )
    }

    fn initial_state(iopl: u8, if_enabled: bool) -> LongModeState {
        LongModeState {
            cr3: None,
            cr4: None,
            rflags: 2 | (u64::from(iopl) << 12) | (u64::from(if_enabled) << 9),
            sti_shadow: Some(StiShadow::Clear),
            ..LongModeState::default()
        }
    }

    #[test]
    fn sti_privilege_matrix_is_atomic_and_exact() {
        let mut cases = 0;
        for if_enabled in [false, true] {
            for cpl in 0..=3 {
                for iopl in 0..=3 {
                    let mut state = initial_state(iopl, if_enabled);
                    let before = state.clone();
                    let mut memory = Code(&[0xfb], 0);
                    assert_eq!(
                        crate::long_decode::decode(&mut memory, 0, context(cpl))
                            .unwrap()
                            .operation,
                        crate::long_decode::Operation::Sti
                    );
                    memory.1 = 0;
                    let result = run_with_sti_profile(
                        &mut state,
                        &mut memory,
                        RunConfig {
                            instruction_budget: 1,
                            stop_rip: None,
                            context: context(cpl),
                        },
                        &StiProfile::NonPviLong64V1,
                        || false,
                    );
                    cases += 1;
                    if cpl <= iopl {
                        assert_eq!(
                            result.exit,
                            RunExit::InstructionBudgetExhausted,
                            "result={result:?}, state={state:?}"
                        );
                        assert_eq!(result.instructions_executed, 1);
                        assert_eq!(state.rip, 1);
                        assert_ne!(state.rflags & (1 << 9), 0);
                        assert_eq!(
                            state.sti_shadow,
                            Some(if if_enabled {
                                StiShadow::Clear
                            } else {
                                StiShadow::Blocking
                            })
                        );
                        let mut expected = before.clone();
                        expected.rflags |= 1 << 9;
                        expected.rip = 1;
                        expected.sti_shadow = Some(if if_enabled {
                            StiShadow::Clear
                        } else {
                            StiShadow::Blocking
                        });
                        assert_eq!(state, expected);
                    } else {
                        assert_eq!(
                            result.exit,
                            RunExit::Fault(crate::memory::ArchitecturalFault {
                                vector: 13,
                                error_code: Some(0),
                                linear_address: None
                            })
                        );
                        assert_eq!(result.instructions_executed, 0);
                        assert_eq!(state, before);
                    }
                    assert_eq!(memory.1, 1);
                }
            }
        }
        assert_eq!(cases, 32);
    }

    #[test]
    fn shadow_consumption_stop_budget_and_cli_transitions() {
        let mut state = initial_state(3, false);
        assert_eq!(
            run(&[0xfb, 0x90], &mut state, 1, 0).exit,
            RunExit::InstructionBudgetExhausted
        );
        assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
        assert_eq!(state.rip, 1);
        assert_eq!(
            run(&[0xfb, 0x90], &mut state, 1, 0).exit,
            RunExit::InstructionBudgetExhausted
        );
        assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
        assert_eq!(state.rip, 2);
        let mut after_nop = Code(&[0x90], 0);
        assert_eq!(
            crate::execute::run_with_maskable_request(
                &mut state,
                &mut after_nop,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: context(0)
                },
                crate::INTEGER_FLAGS,
                Some(0x40),
                || false,
            )
            .unwrap(),
            crate::execute::MaskableRequestRunResult::InterruptEligible {
                vector: 0x40,
                rip: 2
            },
        );
        assert_eq!(after_nop.1, 0);

        let mut faulting_fetch = initial_state(3, false);
        let fetch_result = run(&[0xfb], &mut faulting_fetch, 2, 0);
        assert!(matches!(
            fetch_result.exit,
            RunExit::MemoryError(MemoryError::Unmapped {
                access: crate::memory::AccessKind::Fetch,
                ..
            })
        ));
        assert_eq!(fetch_result.instructions_executed, 1);
        assert_eq!(faulting_fetch.rflags & (1 << 9), 1 << 9);
        assert_eq!(faulting_fetch.sti_shadow, Some(StiShadow::Blocking));

        let mut cpu_fault = initial_state(3, false);
        let cpu_fault_result = run(&[0xfb, 0xf0, 0xfb], &mut cpu_fault, 2, 0);
        assert_eq!(
            cpu_fault_result.exit,
            RunExit::Fault(crate::memory::ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(cpu_fault_result.instructions_executed, 1);
        assert_eq!(cpu_fault.rflags & (1 << 9), 1 << 9);
        assert_eq!(cpu_fault.sti_shadow, Some(StiShadow::Blocking));

        let mut unsupported = initial_state(3, false);
        let unsupported_result = run(&[0xfb, 0xf4], &mut unsupported, 2, 0);
        assert_eq!(
            unsupported_result.exit,
            RunExit::Unsupported { opcode: 0xf4 }
        );
        assert_eq!(unsupported_result.instructions_executed, 1);
        assert_eq!(unsupported.sti_shadow, Some(StiShadow::Blocking));

        let mut double = initial_state(3, false);
        assert_eq!(
            run(&[0xfb, 0xfb], &mut double, 2, 0).instructions_executed,
            2
        );
        assert_eq!(double.sti_shadow, Some(StiShadow::Clear));

        let mut cli = initial_state(3, false);
        assert_eq!(
            run(&[0xfa, 0xfb, 0x90], &mut cli, 3, 0).instructions_executed,
            3
        );
        assert_ne!(cli.rflags & (1 << 9), 0);
        assert_eq!(cli.sti_shadow, Some(StiShadow::Clear));

        let mut cli_only = initial_state(3, true);
        assert_eq!(
            crate::execute::run_with_cli_profile(
                &mut cli_only,
                &mut Code(&[0xfa], 0),
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: context(0),
                },
                &crate::CliProfile::NonPviLong64V1,
                || false,
            )
            .instructions_executed,
            1
        );
        assert_eq!(cli_only.rflags & (1 << 9), 0);
        let mut request_memory = Code(&[0x00, 0x90], 0);
        let request_result = crate::execute::run_with_maskable_request(
            &mut cli_only,
            &mut request_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(0),
            },
            crate::INTEGER_FLAGS,
            Some(0x40),
            || false,
        )
        .unwrap();
        assert!(matches!(
            request_result,
            crate::execute::MaskableRequestRunResult::Run(_)
        ));
        assert_eq!(request_memory.1, 1);
    }

    #[test]
    fn stop_address_and_budget_do_not_consume_active_shadow() {
        for (budget, stop_rip, expected) in [
            (0, None, RunExit::InstructionBudgetExhausted),
            (2, Some(0), RunExit::StopAddress),
        ] {
            let mut state = initial_state(3, true);
            state.sti_shadow = Some(StiShadow::Blocking);
            let mut memory = Code(&[0x90], 0);
            let result = run_with_sti_profile(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: context(0),
                },
                &StiProfile::NonPviLong64V1,
                || false,
            );
            assert_eq!(result.exit, expected);
            assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
            assert_eq!(memory.1, 0);
        }
        let mut invalid = initial_state(3, false);
        invalid.sti_shadow = Some(StiShadow::Blocking);
        let before = invalid.clone();
        let mut memory = Code(&[0x90], 0);
        let result = run_with_sti_profile(
            &mut invalid,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(0),
            },
            &StiProfile::NonPviLong64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xfb });
        assert_eq!(invalid, before);
        assert_eq!(memory.1, 0);
    }

    #[test]
    fn active_shadow_rejects_maskable_request_before_fetch() {
        let mut state = initial_state(3, true);
        state.sti_shadow = Some(StiShadow::Blocking);
        let before = state.clone();
        let mut memory = Code(&[0x90], 0);
        assert_eq!(
            crate::execute::run_with_maskable_request(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: context(0)
                },
                crate::INTEGER_FLAGS,
                Some(0x40),
                || false,
            ),
            Err(crate::execute::MaskableRequestRunInputError::InterruptInhibitionActive),
        );
        assert_eq!(state, before);
        assert_eq!(memory.1, 0);
    }

    #[test]
    fn sequence_profile_rejects_extra_and_prefixed_instructions() {
        let mut state = initial_state(3, false);
        let mut extra = Code(&[0xfb, 0xfb, 0xfb], 0);
        let result = run_with_sti_profile(
            &mut state,
            &mut extra,
            RunConfig {
                instruction_budget: 3,
                stop_rip: None,
                context: context(0),
            },
            &StiProfile::NonPviLong64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xfb });
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(state.sti_shadow, Some(StiShadow::Clear));

        let mut locked = initial_state(3, false);
        let mut lock_memory = Code(&[0xf0, 0xfb], 0);
        let result = run_with_sti_profile(
            &mut locked,
            &mut lock_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(0),
            },
            &StiProfile::NonPviLong64V1,
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(crate::memory::ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(lock_memory.1, 2);
        assert_eq!(locked.sti_shadow, Some(StiShadow::Clear));

        let mut legacy = initial_state(3, false);
        let before = legacy.clone();
        let mut legacy_memory = Code(&[0xfb], 0);
        let result = crate::execute::run(
            &mut legacy,
            &mut legacy_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(0),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xfb });
        assert_eq!(legacy, before);

        let mut active_legacy = initial_state(3, true);
        active_legacy.sti_shadow = Some(StiShadow::Blocking);
        let before = active_legacy.clone();
        let mut active_legacy_memory = Code(&[0x90], 0);
        let result = crate::execute::run(
            &mut active_legacy,
            &mut active_legacy_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(0),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xfb });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(active_legacy, before);
        assert_eq!(active_legacy_memory.1, 0);
    }
}
