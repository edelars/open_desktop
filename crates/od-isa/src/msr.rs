//! Caller-selected local MSR probe policies for the reference interpreter.
//!
//! `SysenterProbeV1` models a fault-only safe SYSENTER probe, while the FS/GS
//! profiles model one explicit base read/write each. None enables general MSR
//! support.

/// Immutable MSR access policy explicitly selected by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsrProfile {
    /// Fault RDMSR/WRMSR to IA32_SYSENTER_CS/ESP/EIP (`0x174..=0x176`).
    SysenterProbeV1,
    /// CPL0-only access to IA32_GS_BASE (`0xc000_0101`).
    GsBaseV1,
    /// CPL0-only access to IA32_FS_BASE (`0xc000_0100`).
    FsBaseV1,
    /// CPL0-only access to IA32_KERNEL_GS_BASE (`0xc000_0102`).
    KernelGsBaseV1,
    /// Read-only CPL0 access to a caller-selected synthetic IA32_EFER value.
    EferReadOnlyV1(u64),
}

impl MsrProfile {
    /// Whether this profile handles `index`.
    pub const fn handles_index(self, index: u32) -> bool {
        match self {
            Self::SysenterProbeV1 => matches!(index, 0x174..=0x176),
            Self::GsBaseV1 => index == 0xc000_0101,
            Self::FsBaseV1 => index == 0xc000_0100,
            Self::KernelGsBaseV1 => index == 0xc000_0102,
            Self::EferReadOnlyV1(_) => index == 0xc000_0080,
        }
    }

    /// Whether this exact local EFER value belongs to the bounded S2-25e domain.
    pub const fn accepts_efer_value(value: u64) -> bool {
        matches!(value, 0x500 | 0x501 | 0xd00 | 0xd01)
    }
}

/// S2-26h: the Windows-boot MSR file of the CPL0 profile set (`Cpl0ProfileSet::with_msr_windows`).
///
/// The list is the set of MSRs a Windows x64 kernel/loader is expected to touch during boot
/// (hypothesis recorded in ADR-0013 W-18; no Windows binary was observed): IA32_APIC_BASE (`0x1B`),
/// IA32_MISC_ENABLE (`0x1A0`), IA32_PAT (`0x277`), IA32_SYSENTER_CS/ESP/EIP (`0x174..=0x176`),
/// IA32_EFER (`0xC0000080`), IA32_STAR/LSTAR/CSTAR/FMASK (`0xC0000081..=0xC0000084`) and
/// IA32_FS_BASE/GS_BASE/KERNEL_GS_BASE (`0xC0000100..=0xC0000102`). IA32_TSC (`0x10`) is NOT in
/// the list: the TSC policy is withheld pending the owner decision (S2-57), so `RDMSR`/`WRMSR`
/// of `0x10` raise `#GP(0)` like any unimplemented MSR. MTRR MSRs are not listed because the
/// Windows profile does not declare MTRR (CPUID.1:EDX[12]).
pub const fn windows_msr_is_known(index: u32) -> bool {
    matches!(
        index,
        0x1b | 0x1a0 | 0x277 | 0x174..=0x176 | 0xc000_0080..=0xc000_0084 | 0xc000_0100..=0xc000_0102
    )
}

/// Indexes of the Windows list whose value lives in a cell owned by the Windows member itself or
/// by one of the guest MSR cells of the set (everything except APIC_BASE, EFER, FS_BASE, GS_BASE).
pub const fn windows_msr_is_cell_owned(index: u32) -> bool {
    matches!(
        index,
        0x1a0 | 0x277 | 0x174..=0x176 | 0xc000_0081..=0xc000_0084 | 0xc000_0102
    )
}

/// IA32_PAT at power-up (SDM Vol. 3A 14.12.4): PA0 = WB, PA1 = WT, PA2 = UC-, PA3 = UC, repeated.
pub const IA32_PAT_RESET: u64 = 0x0007_0406_0007_0406;
/// IA32_MISC_ENABLE of the virtual CPU: only bit 0 (fast-string enable) is modelled; the other
/// bits (PMU, BTS/PEBS availability, EIST, MONITOR, XD disable, ...) read as 0. The bit layout is
/// from the SDM Vol. 4 table, which is not in the repository text: model decision, not verified.
pub const IA32_MISC_ENABLE_RESET: u64 = 1;
const IA32_MISC_ENABLE_WRITABLE: u64 = 1;

/// Result of a Windows-list `WRMSR` against the current cell value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsMsrWrite {
    /// The cell takes the value.
    Accept,
    /// Architectural `#GP(0)`: reserved bits set or a non-canonical address.
    GeneralProtection,
    /// The value touches a bit the model does not implement: fail closed, no state change.
    Unsupported,
}

const fn canonical48(value: u64) -> bool {
    let high = value >> 47;
    high == 0 || high == 0x1_ffff
}

/// Whether every PAT entry holds an encodable memory type (SDM Vol. 3A Table 14-10: 0, 1, 4, 5, 6,
/// 7; the five high bits of each byte are reserved).
const fn pat_value_valid(value: u64) -> bool {
    let mut entry = 0;
    while entry < 8 {
        let byte = (value >> (entry * 8)) & 0xff;
        if !matches!(byte, 0 | 1 | 4 | 5 | 6 | 7) {
            return false;
        }
        entry += 1;
    }
    true
}

/// Write rules of the cell-owned Windows MSRs (SDM Vol. 2D WRMSR: reserved bits and non-canonical
/// IA32_LSTAR/SYSENTER_ESP/SYSENTER_EIP/KERNEL_GS_BASE raise `#GP(0)`).
///
/// - SYSENTER_CS: bits 63:32 reserved (the register is a 32-bit value, SDM SYSEXIT page).
/// - SYSENTER_ESP/EIP, LSTAR, KERNEL_GS_BASE: canonical 48-bit address.
/// - STAR, CSTAR: any value (CSTAR is not in the WRMSR canonical list).
/// - FMASK: bits 63:32 reserved (SDM Vol. 3A Figure 6-14).
/// - PAT: every entry an encodable type (Table 14-10).
/// - MISC_ENABLE: only bit 0 may change; any other change is `Unsupported`.
pub const fn windows_msr_write_decision(index: u32, current: u64, value: u64) -> WindowsMsrWrite {
    let ok = match index {
        0x174 | 0xc000_0084 => value >> 32 == 0,
        0x175 | 0x176 | 0xc000_0082 | 0xc000_0102 => canonical48(value),
        0xc000_0081 | 0xc000_0083 => true,
        0x277 => pat_value_valid(value),
        0x1a0 => {
            return if (current ^ value) & !IA32_MISC_ENABLE_WRITABLE == 0 {
                WindowsMsrWrite::Accept
            } else {
                WindowsMsrWrite::Unsupported
            };
        }
        _ => return WindowsMsrWrite::Unsupported,
    };
    if ok {
        WindowsMsrWrite::Accept
    } else {
        WindowsMsrWrite::GeneralProtection
    }
}

/// Whether a caller-supplied cell value is a state the Windows member can hold.
pub const fn windows_msr_cell_value_valid(index: u32, value: u64) -> bool {
    match index {
        0x1a0 => value & !IA32_MISC_ENABLE_WRITABLE == 0,
        _ => matches!(
            windows_msr_write_decision(index, value, value),
            WindowsMsrWrite::Accept
        ),
    }
}

/// CPU-owned state for the separate, exact IA32_APIC_BASE profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApicBaseCpu {
    pub state: crate::LongModeState,
    apic_base: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApicBaseEffect {
    pub old: u64,
    pub new: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApicBaseExit {
    Read,
    Write(ApicBaseEffect),
    Stop,
    Budget,
    Fault(crate::ArchitecturalFault),
    Unsupported,
    Memory(crate::MemoryError),
}

impl ApicBaseCpu {
    pub const ENABLED: u64 = 0xfee0_0900;
    pub const DISABLED: u64 = 0xfee0_0100;

    pub fn new(state: crate::LongModeState) -> Self {
        Self {
            state,
            apic_base: Self::ENABLED,
        }
    }

    pub fn apic_base(&self) -> u64 {
        self.apic_base
    }

    /// Execute exactly one bare Long64 RDMSR/WRMSR; callers must apply a
    /// returned write effect before executing another instruction or device operation.
    pub fn step(
        &mut self,
        memory: &mut impl crate::GuestMemory,
        context: crate::AccessContext,
        budget: u64,
        stop: bool,
    ) -> ApicBaseExit {
        use crate::long_decode::{self, DecodeError, Operation, Prefixes};
        if stop {
            return ApicBaseExit::Stop;
        }
        if budget == 0 {
            return ApicBaseExit::Budget;
        }
        if self
            .state
            .activity
            .is_some_and(|activity| activity != crate::CpuActivity::Running)
        {
            return ApicBaseExit::Unsupported;
        }
        if self.state.sti_shadow == Some(crate::StiShadow::Blocking) {
            return ApicBaseExit::Unsupported;
        }
        if context.mode != crate::InstructionMode::Long64 {
            return ApicBaseExit::Unsupported;
        }
        let instruction = match long_decode::decode(memory, self.state.rip, context) {
            Ok(i) => i,
            Err(DecodeError::Memory(crate::MemoryError::Architectural(f))) => {
                return ApicBaseExit::Fault(f);
            }
            Err(DecodeError::Memory(e)) => return ApicBaseExit::Memory(e),
            Err(
                DecodeError::Invalid { .. }
                | DecodeError::InvalidMovCr0Lock { .. }
                | DecodeError::InvalidMovCr3Lock { .. }
                | DecodeError::InvalidLgdtLock { .. }
                | DecodeError::InvalidLidtLock { .. },
            ) => return ApicBaseExit::Unsupported,
            Err(DecodeError::Unsupported { .. }) => return ApicBaseExit::Unsupported,
        };
        if instruction.length != 2
            || instruction.prefixes != Prefixes::default()
            || !matches!(
                instruction.operation,
                Operation::ReadMsr | Operation::WriteMsr
            )
            || self.state.gpr[crate::Register::Rcx as usize] as u32 != 0x1b
        {
            return ApicBaseExit::Unsupported;
        }
        let gp = || {
            ApicBaseExit::Fault(crate::ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            })
        };
        if context.privilege != crate::PrivilegeLevel::Ring0 {
            return gp();
        }
        let result = match instruction.operation {
            Operation::ReadMsr => {
                self.state.gpr[crate::Register::Rax as usize] = u64::from(self.apic_base as u32);
                self.state.gpr[crate::Register::Rdx as usize] = self.apic_base >> 32;
                ApicBaseExit::Read
            }
            Operation::WriteMsr => {
                let value = u64::from(self.state.gpr[crate::Register::Rax as usize] as u32)
                    | (u64::from(self.state.gpr[crate::Register::Rdx as usize] as u32) << 32);
                // Bits 0..7, 9, and physical-address bits above bit 35 are reserved in this profile.
                if value & (0xff | (1 << 9) | (!0u64 << 36)) != 0 {
                    return gp();
                }
                if !matches!(value, Self::ENABLED | Self::DISABLED) {
                    return ApicBaseExit::Unsupported;
                }
                let effect = ApicBaseEffect {
                    old: self.apic_base,
                    new: value,
                };
                self.apic_base = value;
                ApicBaseExit::Write(effect)
            }
            _ => unreachable!(),
        };
        self.state.rip = self.state.rip.wrapping_add(2);
        result
    }
}

#[cfg(test)]
mod apic_base_tests {
    use super::*;
    use crate::{AccessContext, GuestMemory, LongModeState, MemoryError, PrivilegeLevel, Register};

    struct Code(Vec<u8>);
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
            self.0
                .get(address as usize)
                .copied()
                .ok_or_else(|| MemoryError::Backend("fetch".into()))
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            unreachable!()
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            unreachable!()
        }
    }
    fn context() -> AccessContext {
        AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        }
    }
    fn cpu() -> ApicBaseCpu {
        let mut state = LongModeState::default();
        state.gpr[Register::Rcx as usize] = 0x1b;
        ApicBaseCpu::new(state)
    }
    fn value(cpu: &mut ApicBaseCpu, value: u64) {
        cpu.state.gpr[Register::Rax as usize] = value as u32 as u64;
        cpu.state.gpr[Register::Rdx as usize] = (value >> 32) as u32 as u64;
    }
    #[test]
    fn blocking_sti_shadow_rejects_before_fetch_and_preserves_cpu() {
        for bytes in [vec![], vec![0x0f, 0x32], vec![0x0f, 0x30]] {
            let mut cpu = cpu();
            cpu.state.sti_shadow = Some(crate::StiShadow::Blocking);
            value(&mut cpu, ApicBaseCpu::DISABLED);
            let before = cpu.clone();
            assert_eq!(
                cpu.step(&mut Code(bytes), context(), 1, false),
                ApicBaseExit::Unsupported
            );
            assert_eq!(cpu, before);
        }
        let mut cpu = cpu();
        cpu.state.sti_shadow = Some(crate::StiShadow::Clear);
        assert_eq!(
            cpu.step(&mut Code(vec![0x0f, 0x32]), context(), 1, false),
            ApicBaseExit::Read
        );
    }

    #[test]
    fn unrelated_invalid_decoder_forms_remain_unsupported() {
        for bytes in [
            &[0xf0, 0x0f, 0x20, 0xc0][..],
            &[0xf0, 0x0f, 0x22, 0xc0][..],
            &[0xf0, 0x0f, 0x22, 0xd8][..],
        ] {
            let mut cpu = cpu();
            let before = cpu.clone();
            assert_eq!(
                cpu.step(&mut Code(bytes.to_vec()), context(), 1, false),
                ApicBaseExit::Unsupported
            );
            assert_eq!(cpu, before);
        }
        let mut cpu = cpu();
        let before = cpu.clone();
        let user_context = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };
        assert!(matches!(
            cpu.step(&mut Code(vec![0x0f, 0x32]), user_context, 1, false),
            ApicBaseExit::Fault(crate::ArchitecturalFault { vector: 13, .. })
        ));
        assert_eq!(cpu, before);
    }
    #[test]
    fn read_write_and_failures_are_atomic() {
        let mut cpu = cpu();
        cpu.state.gpr[Register::Rax as usize] = u64::MAX;
        cpu.state.gpr[Register::Rdx as usize] = u64::MAX;
        assert_eq!(
            cpu.step(&mut Code(vec![0x0f, 0x32]), context(), 1, false),
            ApicBaseExit::Read
        );
        assert_eq!(cpu.state.gpr[Register::Rax as usize], ApicBaseCpu::ENABLED);
        assert_eq!(cpu.state.gpr[Register::Rdx as usize], 0);
        for target in [
            ApicBaseCpu::DISABLED,
            ApicBaseCpu::ENABLED,
            ApicBaseCpu::ENABLED,
        ] {
            cpu.state.rip = 0;
            value(&mut cpu, target);
            let old = cpu.apic_base();
            assert_eq!(
                cpu.step(&mut Code(vec![0x0f, 0x30]), context(), 1, false),
                ApicBaseExit::Write(ApicBaseEffect { old, new: target })
            );
            assert_eq!(cpu.state.rip, 2);
        }
        for (target, fault) in [
            (ApicBaseCpu::ENABLED | 1, true),
            (ApicBaseCpu::ENABLED | (1 << 36), true),
            (ApicBaseCpu::ENABLED ^ 0x1000, false),
            (ApicBaseCpu::ENABLED ^ 0x100, false),
            (ApicBaseCpu::ENABLED | 0x400, false),
        ] {
            cpu.state.rip = 0;
            value(&mut cpu, target);
            let before = cpu.clone();
            let result = cpu.step(&mut Code(vec![0x0f, 0x30]), context(), 1, false);
            assert_eq!(
                matches!(
                    result,
                    ApicBaseExit::Fault(crate::ArchitecturalFault { vector: 13, .. })
                ),
                fault
            );
            if !fault {
                assert_eq!(result, ApicBaseExit::Unsupported);
            }
            assert_eq!(cpu, before);
        }
        let before = cpu.clone();
        assert_eq!(
            cpu.step(&mut Code(vec![]), context(), 0, false),
            ApicBaseExit::Budget
        );
        assert_eq!(
            cpu.step(&mut Code(vec![]), context(), 1, true),
            ApicBaseExit::Stop
        );
        assert!(matches!(
            cpu.step(&mut Code(vec![0x0f]), context(), 1, false),
            ApicBaseExit::Memory(_)
        ));
        assert_eq!(cpu, before);
    }
}
