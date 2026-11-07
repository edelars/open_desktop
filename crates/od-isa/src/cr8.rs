//! Exact opt-in Long64 MOV CR8 profile backed by a machine-owned xAPIC TPR.

use crate::{
    AccessContext, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState, MemoryError,
    PrivilegeLevel, StiShadow,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cr8Long64V1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cr8TprEffect {
    pub old_tpr: u8,
    pub new_tpr: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cr8Exit {
    Read,
    Write(Cr8TprEffect),
    Stop,
    Budget,
    Fault(ArchitecturalFault),
    Unsupported,
    Memory(MemoryError),
}

fn gp() -> Cr8Exit {
    Cr8Exit::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    })
}

fn ud() -> Cr8Exit {
    Cr8Exit::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    })
}

fn fetch(
    memory: &mut impl GuestMemory,
    address: u64,
    context: AccessContext,
) -> Result<u8, Cr8Exit> {
    if !crate::long_decode::canonical(address) {
        return Err(gp());
    }
    memory.fetch(address, context).map_err(|error| match error {
        MemoryError::Architectural(fault) => Cr8Exit::Fault(fault),
        other => Cr8Exit::Memory(other),
    })
}

/// Execute one exact CR8 transfer. `tpr` is sampled from the machine's APIC;
/// a successful write returns a typed effect for the machine to apply before
/// returning to its caller.
pub fn step_cr8(
    _profile: Cr8Long64V1,
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    tpr: u8,
    budget: u64,
    stop: bool,
) -> Cr8Exit {
    if stop {
        return Cr8Exit::Stop;
    }
    if budget == 0 {
        return Cr8Exit::Budget;
    }
    if context.mode != crate::InstructionMode::Long64
        || state.activity != Some(CpuActivity::Running)
        || state.sti_shadow != Some(StiShadow::Clear)
    {
        return Cr8Exit::Unsupported;
    }

    let start = state.rip;
    let first = match fetch(memory, start, context) {
        Ok(byte) => byte,
        Err(exit) => return exit,
    };
    let lock = first == 0xf0;
    let rex = if lock {
        match fetch(memory, start.wrapping_add(1), context) {
            Ok(0x44) => 0x44,
            Ok(_) => return Cr8Exit::Unsupported,
            Err(exit) => return exit,
        }
    } else if first == 0x44 {
        first
    } else {
        return Cr8Exit::Unsupported;
    };
    let opcode0 = match fetch(
        memory,
        start.wrapping_add(if lock { 2 } else { 1 }),
        context,
    ) {
        Ok(byte) => byte,
        Err(exit) => return exit,
    };
    if opcode0 != 0x0f {
        return Cr8Exit::Unsupported;
    }
    let opcode = match fetch(
        memory,
        start.wrapping_add(if lock { 3 } else { 2 }),
        context,
    ) {
        Ok(byte @ (0x20 | 0x22)) => byte,
        Ok(_) => return Cr8Exit::Unsupported,
        Err(exit) => return exit,
    };
    let modrm = match fetch(
        memory,
        start.wrapping_add(if lock { 4 } else { 3 }),
        context,
    ) {
        Ok(byte) => byte,
        Err(exit) => return exit,
    };
    if rex != 0x44 || modrm != 0xc0 {
        return Cr8Exit::Unsupported;
    }
    if lock {
        return ud();
    }
    if context.privilege != PrivilegeLevel::Ring0 {
        return gp();
    }

    let result = if opcode == 0x20 {
        state.gpr[crate::Register::Rax as usize] = u64::from((tpr >> 4) & 0x0f);
        Cr8Exit::Read
    } else {
        let value = state.gpr[crate::Register::Rax as usize];
        if value & !0x0f != 0 {
            return gp();
        }
        Cr8Exit::Write(Cr8TprEffect {
            old_tpr: tpr,
            new_tpr: (value as u8) << 4,
        })
    };
    state.rip = start.wrapping_add(4);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccessKind, AccessOrigin, InstructionMode};

    struct Code {
        bytes: Vec<u8>,
        base: u64,
        fault_at: Option<usize>,
        architectural_fault: bool,
        fetches: usize,
    }

    impl GuestMemory for Code {
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
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.fetches += 1;
            if self.fault_at == Some(address as usize) {
                if self.architectural_fault {
                    return Err(MemoryError::Architectural(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(16),
                        linear_address: Some(address),
                    }));
                }
                return Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                });
            }
            self.bytes
                .get(address.wrapping_sub(self.base) as usize)
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            unreachable!()
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            unreachable!()
        }
    }

    fn state() -> LongModeState {
        let mut state = LongModeState::default();
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        state
    }

    fn context(privilege: PrivilegeLevel) -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        }
    }

    fn run(state: &mut LongModeState, code: &mut Code, context: AccessContext, tpr: u8) -> Cr8Exit {
        step_cr8(Cr8Long64V1, state, code, context, tpr, 1, false)
    }

    #[test]
    fn reads_zero_extended_cr8_from_tpr_priority_class() {
        let mut state = state();
        let mut code = Code {
            bytes: vec![0x44, 0x0f, 0x20, 0xc0],
            base: 0,
            fault_at: None,
            architectural_fault: false,
            fetches: 0,
        };
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0x0f),
            Cr8Exit::Read
        );
        assert_eq!(state.gpr[crate::Register::Rax as usize], 0);
        assert_eq!(state.rip, 4);

        state.rip = 0;
        code.fetches = 0;
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0xff),
            Cr8Exit::Read
        );
        assert_eq!(state.gpr[crate::Register::Rax as usize], 15);
    }

    #[test]
    fn writes_only_cr8_low_nibble_and_faults_atomically_on_reserved_bits() {
        let mut state = state();
        state.gpr[crate::Register::Rax as usize] = 5;
        let mut code = Code {
            bytes: vec![0x44, 0x0f, 0x22, 0xc0],
            base: 0,
            fault_at: None,
            architectural_fault: false,
            fetches: 0,
        };
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0x3f),
            Cr8Exit::Write(Cr8TprEffect {
                old_tpr: 0x3f,
                new_tpr: 0x50,
            })
        );
        assert_eq!(state.rip, 4);
        state.rip = 0;
        state.gpr[crate::Register::Rax as usize] = 15;
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0x3f),
            Cr8Exit::Write(Cr8TprEffect {
                old_tpr: 0x3f,
                new_tpr: 0xf0
            })
        );
        state.rip = 0;
        state.gpr[crate::Register::Rax as usize] = 0x10;
        let before = state.clone();
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0x3f),
            gp()
        );
        assert_eq!(state, before);
    }

    #[test]
    fn exact_encoding_privilege_lock_fetch_stop_and_budget_boundaries() {
        let mut state = state();
        let mut code = Code {
            bytes: vec![0xf0, 0x44, 0x0f, 0x20, 0xc0],
            base: 0,
            fault_at: None,
            architectural_fault: false,
            fetches: 0,
        };
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0),
            ud()
        );
        assert_eq!(state.rip, 0);
        state.rip = 0;
        code.bytes = vec![0x44, 0x0f, 0x20, 0xc8];
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0),
            Cr8Exit::Unsupported
        );
        code.bytes = vec![0x44, 0x0f, 0x20, 0xc0];
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring3), 0),
            gp()
        );
        let before = state.clone();
        code.fetches = 0;
        assert_eq!(
            step_cr8(
                Cr8Long64V1,
                &mut state,
                &mut code,
                context(PrivilegeLevel::Ring0),
                0,
                0,
                false
            ),
            Cr8Exit::Budget
        );
        assert_eq!(code.fetches, 0);
        assert_eq!(
            step_cr8(
                Cr8Long64V1,
                &mut state,
                &mut code,
                context(PrivilegeLevel::Ring0),
                0,
                1,
                true
            ),
            Cr8Exit::Stop
        );
        assert_eq!(code.fetches, 0);
        assert_eq!(state, before);
        code.fault_at = Some(2);
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0),
            Cr8Exit::Memory(MemoryError::Unmapped {
                address: 2,
                access: AccessKind::Fetch
            })
        );
        assert_eq!(state, before);
        code.architectural_fault = true;
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0),
            Cr8Exit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(16),
                linear_address: Some(2),
            })
        );
        assert_eq!(state, before);
    }

    #[test]
    fn activity_and_interrupt_shadow_are_checked_before_fetch() {
        let mut state = state();
        let mut code = Code {
            bytes: vec![0x44, 0x0f, 0x20, 0xc0],
            base: 0,
            fault_at: None,
            architectural_fault: false,
            fetches: 0,
        };
        state.activity = Some(CpuActivity::Halted);
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0),
            Cr8Exit::Unsupported
        );
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Blocking);
        assert_eq!(
            run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0),
            Cr8Exit::Unsupported
        );
        assert_eq!(code.fetches, 0);
    }

    #[test]
    fn noncanonical_instruction_fetch_faults_before_backend_and_commit() {
        const START: u64 = 0x0000_7fff_ffff_fffd;
        for opcode in [0x20, 0x22] {
            let mut state = state();
            state.rip = START;
            state.gpr[crate::Register::Rax as usize] = 5;
            let before = state.clone();
            let mut code = Code {
                bytes: vec![0x44, 0x0f, opcode, 0xc0],
                base: START,
                fault_at: None,
                architectural_fault: false,
                fetches: 0,
            };
            assert_eq!(
                run(&mut state, &mut code, context(PrivilegeLevel::Ring0), 0x3f),
                gp()
            );
            assert_eq!(code.fetches, 3);
            assert_eq!(state, before);
        }
    }
}
