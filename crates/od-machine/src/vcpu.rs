//! The architectural state of one virtual processor, apart from the board (S6-10).
//!
//! The board keeps a set of these, one element by default (index 0, the boot processor); a board
//! built with more vCPUs (S6-11) adds application processors that are never run yet. The struct is
//! plain data: it holds what the CPL0 set leaves behind between runs and what the guest can read
//! back, and nothing else. Device handles, memory and the interrupt source stay on the board.

use crate::FIRMWARE_EFER;
use od_isa::LongModeState;
use od_isa::execute::GuestFpState;

/// Index of the boot processor (BSP); with one vCPU, the only one.
pub(crate) const BOOT_VCPU_INDEX: usize = 0;

/// Most vCPUs a board may have (SMP_CONTRACT.md 2: values above 4 are outside version 1).
pub(crate) const MAX_VCPUS: usize = 4;

/// The board's vCPU set: the boot processor on `boot_state`, then `count - 1` application
/// processors on `LongModeState::default()` (never run: INIT/SIPI start-up is S6-13).
pub(crate) fn vcpu_set(boot_state: LongModeState, count: usize) -> Vec<Vcpu> {
    let mut vcpus = vec![Vcpu::new(BOOT_VCPU_INDEX, boot_state)];
    vcpus.extend((1..count).map(|index| Vcpu::new(index, LongModeState::default())));
    vcpus
}

/// CPU state and the guest-visible register cells of one virtual processor.
#[derive(Debug)]
pub(crate) struct Vcpu {
    /// Position of this processor in the board's vCPU set, equal to its APIC ID (S6-11). Nothing
    /// outside tests reads it: the Local APIC of this processor is found by the same index.
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
    use crate::{FirmwareBoardConfig, FirmwareBoardError, PciEcam, firmware_board::assemble_board};

    fn assembled(vcpu_count: usize) -> Result<crate::LoadedFirmwareBoard, FirmwareBoardError> {
        let config = FirmwareBoardConfig {
            vcpu_count,
            ..FirmwareBoardConfig::default()
        };
        assemble_board(&config, PciEcam::new(), None, None)?.into_linux_board(0x1000, 0x1000)
    }

    #[test]
    fn the_vcpu_set_numbers_the_processors_and_gives_the_boot_state_to_the_first() {
        let boot = LongModeState {
            rip: 0x1234,
            ..LongModeState::default()
        };
        let set = vcpu_set(boot.clone(), 3);
        assert_eq!(
            set.iter().map(|vcpu| vcpu.index).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(set[BOOT_VCPU_INDEX].state, boot);
        assert!(
            set[1..]
                .iter()
                .all(|vcpu| vcpu.state == LongModeState::default())
        );
        assert_eq!(vcpu_set(boot, 1).len(), 1);
    }

    #[test]
    fn the_board_takes_its_vcpu_count_from_the_configuration() {
        assert_eq!(FirmwareBoardConfig::default().vcpu_count, 1);
        let one = assembled(1).unwrap();
        assert_eq!(one.vcpu_count(), 1);
        assert!(one.local_apic_of(1).is_none());
        let two = assembled(2).unwrap();
        assert_eq!(two.vcpu_count(), 2);
        assert_eq!(two.local_apic_of(0).unwrap().cpu_index(), 0);
        assert_eq!(two.local_apic_of(1).unwrap().cpu_index(), 1);
        assert!(two.local_apic_of(2).is_none());
        // The board's own handle is the boot processor's; all handles share one APIC state.
        assert_eq!(two.local_apic().cpu_index(), BOOT_VCPU_INDEX);
        two.local_apic_of(1)
            .unwrap()
            .request_fixed_edge(0x41)
            .unwrap();
        assert_eq!(two.local_apic().pending(), None);
        assert_eq!(two.local_apic_of(1).unwrap().pending(), Some(0x41));
        assert_eq!(assembled(4).unwrap().vcpu_count(), 4);
    }

    #[test]
    fn a_vcpu_count_outside_one_to_four_is_a_configuration_error() {
        for count in [0, 5, 64] {
            assert_eq!(
                assembled(count).err(),
                Some(FirmwareBoardError::Config(
                    "vcpu_count must be in 1..=MAX_VCPUS"
                )),
                "count {count}"
            );
        }
    }

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
