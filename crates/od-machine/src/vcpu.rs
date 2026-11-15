//! The architectural state of one virtual processor, apart from the board (S6-10).
//!
//! The board keeps a set of these; today the set has exactly one element (index 0). The struct is
//! plain data: it holds what the CPL0 set leaves behind between runs and what the guest can read
//! back, and nothing else. Device handles, memory and the interrupt source stay on the board.

use crate::FIRMWARE_EFER;
use od_isa::LongModeState;
use od_isa::execute::GuestFpState;

/// Index of the only processor of the board (the boot processor).
pub(crate) const BOOT_VCPU_INDEX: usize = 0;

/// CPU state and the guest-visible register cells of one virtual processor.
#[derive(Debug)]
pub(crate) struct Vcpu {
    /// Position of this processor in the board's vCPU set. Nothing outside tests reads it while
    /// the set has one element; the per-processor APIC work (S6-11) is the first reader.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) index: usize,
    /// The direct-entry CPU state the interpreter runs on.
    pub(crate) state: LongModeState,
    /// The whole CR2 the guest left at the end of the latest CPL0-set run (S4-38L5c): the
    /// faulting address of the last delivered `#PF`, or any other value the CR2 took. Fed back
    /// as CR2 on entry the way the board's page-table root is for CR3. `None` until the first
    /// CPL0-set run.
    pub(crate) guest_cr2: Option<u64>,
    /// The guest-visible `IA32_EFER` (S4-38L7f): `FIRMWARE_EFER` until a guest `WRMSR` of EFER
    /// changed `SCE` (the only bit the set lets change). The walkers keep using the constant
    /// `FIRMWARE_EFER` (LME/LMA/NXE never change), so the board's `efer()` stays the walker's EFER.
    pub(crate) guest_efer: u64,
    /// The guest `IA32_STAR` (S4-38L7ao): `0` until a guest `WRMSR`. Only stored.
    pub(crate) guest_star: u64,
    /// The guest `IA32_LSTAR` (S4-38L7ap): `0` until a guest `WRMSR`. Only stored.
    pub(crate) guest_lstar: u64,
    /// The guest `IA32_CSTAR` (S4-38L7aq): `0` until a guest `WRMSR`. Only stored.
    pub(crate) guest_cstar: u64,
    /// The guest `IA32_SYSENTER_CS` (S4-38L7ar): `0` until a guest `WRMSR`. The full 64-bit value
    /// written (the real register holds a 16-bit selector; not modelled as such). Only stored.
    pub(crate) guest_sysenter_cs: u64,
    /// The guest `IA32_SYSENTER_ESP` (S4-38L7as): `0` until a guest `WRMSR`. Only stored.
    pub(crate) guest_sysenter_esp: u64,
    /// The guest `IA32_SYSENTER_EIP` (S4-38L7as): `0` until a guest `WRMSR`. Only stored.
    pub(crate) guest_sysenter_eip: u64,
    /// The guest `IA32_SYSCALL_MASK` (S4-38L7at): `0` until a guest `WRMSR`. The full 64-bit
    /// value written (the flag-clearing behaviour of the real FMASK is not modelled). Only stored.
    pub(crate) guest_syscall_mask: u64,
    /// The guest `IA32_KERNEL_GS_BASE` (S4-38L7au): `0` until a guest `WRMSR` (no canonical
    /// check; `SWAPGS` is not modelled). Only stored.
    pub(crate) guest_kernel_gs_base: u64,
    /// The guest FP state of the CPL0 set's FP member (S4-38L9d): x87 environment and registers,
    /// MXCSR and XMM. `GuestFpState::initial()` until a run with the member changes it; carried
    /// between runs like the MSR cells.
    pub(crate) guest_fp: GuestFpState,
}

impl Vcpu {
    /// A processor with the given entry state and every guest cell at its initial value.
    pub(crate) fn new(index: usize, state: LongModeState) -> Self {
        Self {
            index,
            state,
            guest_cr2: None,
            guest_efer: FIRMWARE_EFER,
            guest_star: 0,
            guest_lstar: 0,
            guest_cstar: 0,
            guest_sysenter_cs: 0,
            guest_sysenter_esp: 0,
            guest_sysenter_eip: 0,
            guest_syscall_mask: 0,
            guest_kernel_gs_base: 0,
            guest_fp: GuestFpState::initial(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_vcpu_has_the_given_index_and_initial_cells() {
        let vcpu = Vcpu::new(BOOT_VCPU_INDEX, LongModeState::default());
        assert_eq!(vcpu.index, 0);
        assert_eq!(vcpu.state, LongModeState::default());
        assert_eq!(vcpu.guest_cr2, None);
        assert_eq!(vcpu.guest_efer, FIRMWARE_EFER);
        assert_eq!(vcpu.guest_star, 0);
        assert_eq!(vcpu.guest_lstar, 0);
        assert_eq!(vcpu.guest_cstar, 0);
        assert_eq!(vcpu.guest_sysenter_cs, 0);
        assert_eq!(vcpu.guest_sysenter_esp, 0);
        assert_eq!(vcpu.guest_sysenter_eip, 0);
        assert_eq!(vcpu.guest_syscall_mask, 0);
        assert_eq!(vcpu.guest_kernel_gs_base, 0);
        assert_eq!(vcpu.guest_fp, GuestFpState::initial());
    }
}
