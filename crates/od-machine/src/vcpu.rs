//! The architectural state of one virtual processor, apart from the board (S6-10).
//!
//! The board keeps a set of these, one element by default (index 0, the boot processor); a board
//! built with more vCPUs (S6-11) adds application processors that are never run yet. The struct is
//! plain data: it holds what the CPL0 set leaves behind between runs and what the guest can read
//! back, and nothing else. Device handles, memory and the interrupt source stay on the board.

use crate::FIRMWARE_EFER;
use crate::apic_pair::PendingStartup;
use crate::firmware_board::FIRMWARE_CR0;
use od_isa::cpuid::CpuidProfile;
use od_isa::execute::GuestFpState;
use od_isa::legacy_mode::RealModeState;
use od_isa::protected_exec::ProtectedCpu;
use od_isa::real_mode_exec::{CpuidSource, EFLAGS_IF, RealModeCpu};
use od_isa::{CpuActivity, LongModeState};

/// Index of the boot processor (BSP); with one vCPU, the only one.
pub(crate) const BOOT_VCPU_INDEX: usize = 0;

/// Most vCPUs a board may have (SMP_CONTRACT.md 2: values above 4 are outside version 1).
pub(crate) const MAX_VCPUS: usize = 4;

/// CR0 of a processor after power-on (SDM Vol. 3A Table 12-1): `CD`, `NW` and `ET`.
pub(crate) const POWER_ON_CR0: u32 = 0x6000_0010;

/// EDX of an application processor after INIT with the default profile
/// (`CpuidProfile::DiagnosticV1`), SDM Vol. 3A Table 12-1: the processor signature, that is
/// `CPUID.01H:EAX`. Since S6-18a EDX follows the CPUID profile of the vCPU ([`VcpuCpuid`]): the leaf-1
/// EAX of that profile (`0x600` for every profile of `cpuid.rs`), so this constant is the value for
/// the default profile only. The oracle runs of S6-14d use the host value `0x306A9` as an input
/// instead; no profile of the board has that value.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const AP_INIT_EDX: u32 = CpuidProfile::DiagnosticV1.query(1, 0).eax;

/// The CPUID profile chosen for the legacy models of the application processors of the Linux
/// boards (S6-18a): [`CpuidProfile::OdX8664V1Neutral`], the only profile of `cpuid.rs` on which the
/// kernel's `verify_cpu` returns 0 (S6-14h; the other three give 1). It differs from the profile of the
/// boot processor's long-mode CPL0 run (`X8664V1Stage4AApic`, which declares only the features the
/// engine implements): the trampoline's `verify_cpu` demands the whole target mask, the long-mode
/// run of the AP is S6-21.
pub(crate) const LINUX_AP_CPUID_PROFILE: CpuidProfile = CpuidProfile::OdX8664V1Neutral;

/// What the CPUID of one vCPU reads (S6-18a): the profile of the board, the vCPU's APIC ID (its
/// index) and the number of vCPUs of the board, which `CPUID.01H:EBX` reports when it is above one
/// (`SMP_CONTRACT.md` section 2). The profile also gives EDX after INIT.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VcpuCpuid {
    pub(crate) profile: CpuidProfile,
    pub(crate) apic_id: u8,
    pub(crate) cpu_count: u8,
}

impl VcpuCpuid {
    /// The CPUID configuration of vCPU `index` of a board with `count` vCPUs (`1..=MAX_VCPUS`).
    pub(crate) const fn new(profile: CpuidProfile, index: usize, count: usize) -> Self {
        // `as u8` below would truncate silently; with `MAX_VCPUS = 4` it is unreachable.
        debug_assert!(count <= MAX_VCPUS && index < MAX_VCPUS);
        Self {
            profile,
            apic_id: index as u8,
            cpu_count: count as u8,
        }
    }

    /// The source of the legacy model's `CPUID`. For one vCPU it is the plain profile (the bytes
    /// of the model are as before S6-18a).
    pub(crate) const fn source(self) -> CpuidSource {
        if self.cpu_count <= 1 {
            CpuidSource::Profile(self.profile)
        } else {
            CpuidSource::ProfileOnVcpu {
                profile: self.profile,
                apic_id: self.apic_id,
                cpu_count: self.cpu_count,
            }
        }
    }

    /// EDX after INIT: `CPUID.01H:EAX` of the profile (SDM Vol. 3A Table 12-1).
    pub(crate) const fn init_edx(self) -> u32 {
        self.profile.query(1, 0).eax
    }
}

/// The board's vCPU set: the boot processor on `boot_state`, then `count - 1` application
/// processors on `LongModeState::default()` in `WaitForSipi` (S6-13). Only INIT and SIPI, applied
/// by the processor itself ([`Vcpu::apply_startup`]), take an application processor out of it;
/// the scheduler then executes it in legacy mode ([`crate::ap_legacy_exec`], S6-16c). The CPUID of
/// every vCPU is `DiagnosticV1` with the count and its APIC ID (S6-18a).
pub(crate) fn vcpu_set(boot_state: LongModeState, count: usize) -> Vec<Vcpu> {
    let mut vcpus = vec![Vcpu::new(BOOT_VCPU_INDEX, boot_state)];
    vcpus.extend((1..count).map(Vcpu::new_ap));
    for vcpu in &mut vcpus {
        vcpu.set_cpuid(VcpuCpuid::new(
            CpuidProfile::DiagnosticV1,
            vcpu.index,
            count,
        ));
    }
    vcpus
}

/// The model a started application processor runs on until it enters long mode (S6-16c): the
/// real-mode model of the trampoline, then the protected-mode one after its far jump into a
/// protected code segment. The trampoline's 64-bit jump ends it: the state becomes a
/// [`LongModeState`] and the vCPU's `legacy` is `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyCpu {
    Real(RealModeCpu),
    Protected(ProtectedCpu),
}

impl LegacyCpu {
    /// The `EFLAGS` of the model.
    pub(crate) fn eflags(&self) -> u32 {
        match self {
            Self::Real(cpu) => cpu.mode.eflags,
            Self::Protected(cpu) => cpu.eflags,
        }
    }

    /// The `CR0` of the model.
    pub(crate) fn cr0(&self) -> u32 {
        match self {
            Self::Real(cpu) => cpu.mode.cr0,
            Self::Protected(cpu) => cpu.cr0,
        }
    }
}

/// The real-mode processor INIT leaves behind (SDM Vol. 3A Table 12-1): the INIT state of
/// [`RealModeState::after_init`] with every general register `0` but `EDX` ([`VcpuCpuid::init_edx`],
/// the leaf-1 EAX of the vCPU's profile) and the reset descriptor tables; its `CPUID` reads
/// [`VcpuCpuid::source`]. `old_cr0` keeps only `CD` and `NW`.
pub(crate) fn init_cpu_for(old_cr0: u32, cpuid: VcpuCpuid) -> RealModeCpu {
    RealModeCpu {
        mode: RealModeState::after_init(old_cr0),
        cpuid: cpuid.source(),
        ..RealModeCpu::after_sipi(0, old_cr0, cpuid.init_edx())
    }
}

/// [`init_cpu_for`] for a lone processor with the default profile, APIC ID 0 and one vCPU.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn init_cpu(old_cr0: u32) -> RealModeCpu {
    init_cpu_for(old_cr0, VcpuCpuid::new(CpuidProfile::DiagnosticV1, 0, 1))
}

/// What a vCPU did with the INIT and SIPI it took from its Local APIC.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct StartupApplied {
    /// An INIT was applied: the real-mode reset state was loaded.
    pub(crate) init: bool,
    /// A SIPI took the processor out of `WaitForSipi` and set its start address.
    pub(crate) started: bool,
}

/// What a vCPU did with the NMI waiting in its Local APIC at one boundary (S6-12b).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum NmiBoundary {
    /// No NMI waits.
    NoPending,
    /// An NMI waits and cannot be delivered now: it stays pending (the one latch).
    Held,
    /// The NMI was delivered through vector 2: the vCPU is in the handler with NMI blocked, and
    /// the latch is free for one more NMI.
    Delivered,
}

/// CPU state and the guest-visible register cells of one virtual processor. A value: `Clone` is
/// the state taken for a snapshot (S6-24).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Vcpu {
    /// Position of this processor in the board's vCPU set, equal to its APIC ID (S6-11).
    pub(crate) index: usize,
    /// The direct-entry CPU state the interpreter runs on.
    pub(crate) state: LongModeState,
    /// The whole CR2 the guest left at the end of the latest CPL0-set run (S4-38L5c): the
    /// faulting address of the last delivered `#PF`, or any other value the CR2 took. Fed back
    /// as CR2 on entry the way the board's page-table root is for CR3. `None` until the first
    /// CPL0-set run.
    pub(crate) guest_cr2: Option<u64>,
    /// The guest-visible `IA32_EFER` (S4-38L7f, S2-25f): `FIRMWARE_EFER` until a guest `WRMSR` of EFER
    /// changed `SCE` or `NXE` (the only bits the set lets change; LME and LMA never change). The
    /// boot processor's walkers take their EFER from the board's `efer()`: `FIRMWARE_EFER` with the
    /// NXE bit of this field, so a cleared NXE reaches every walker the board builds. The walkers
    /// of the other vCPUs and of the host threads are separate (see the S2-25 open limits in the
    /// CPU contract).
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
    /// The guest `IA32_PAT` of the Windows MSR member (S7-32 slice F): the power-up value
    /// [`od_isa::msr::IA32_PAT_RESET`] until a guest `WRMSR`. Carried between runs like the other
    /// MSR cells; memory types are not modelled.
    pub(crate) guest_pat: u64,
    /// The guest `IA32_MISC_ENABLE` of the Windows MSR member (S7-32 slice F):
    /// [`od_isa::msr::IA32_MISC_ENABLE_RESET`] until a guest `WRMSR` of its writable bit.
    pub(crate) guest_misc_enable: u64,
    /// The guest `IA32_BIOS_SIGN_ID` of the Windows MSR member (S7-32 slice F): 0 at power-up and
    /// whatever the guest writes; EDX (bits 63:32) is the microcode update signature, 0 = no update
    /// loaded (the virtual CPU has none).
    pub(crate) guest_bios_sign_id: u64,
    /// The guest MTRR file of the Windows MSR member (S7-32 slice I): disabled at power-up
    /// (`MtrrState::default()`, SDM Vol. 3A "MTRR Initialization") until the firmware programs it. Carried between
    /// runs like the other MSR cells; memory types have no effect on accesses (no caches).
    pub(crate) guest_mtrr: od_isa::msr::MtrrState,
    /// The guest MCA file of the Windows MSR member (S7-32 slice I): MCG_STATUS and the banks' CTL,
    /// 0 at power-up; no error is ever logged.
    pub(crate) guest_mca: od_isa::msr::McaState,
    /// The guest FP state of the CPL0 set's FP member (S4-38L9d): x87 environment and registers,
    /// MXCSR and XMM. `GuestFpState::initial()` until a run with the member changes it; carried
    /// between runs like the MSR cells.
    pub(crate) guest_fp: GuestFpState,
    /// The legacy-mode processor model (S6-13, S6-16c): the state INIT loaded
    /// ([`RealModeState::after_init`]) and a SIPI then moved to the start address, then the
    /// real-mode and protected-mode models of the trampoline. `None` for the boot processor until
    /// it has received an INIT, and for an application processor that has entered long mode: from
    /// then on `state` is the processor. An application processor has the power-on reset state from
    /// the start. While `Some`, `state` is not the processor's state (only its `activity` is):
    /// the board runs the legacy model in [`crate::ap_legacy_exec`].
    pub(crate) legacy: Option<LegacyCpu>,
    /// The next call of the vCPU's set quantum resumes a partial `REP` (the previous call ended on
    /// `RepIterationBudgetExhausted`): its first boundary is inside the `REP` and no external
    /// interrupt is delivered there (S6-16d, plan S6-16 6(г)). The set quantum writes it at the end
    /// of every call; an INIT clears it.
    pub(crate) skip_first_boundary: bool,
    /// What the CPUID of this processor reads and the EDX it has after INIT (S6-18a): the board's
    /// profile, this vCPU's APIC ID and the vCPU count. `DiagnosticV1`, index, one vCPU until the
    /// board's set says otherwise ([`vcpu_set`], [`Vcpu::set_cpuid`]).
    pub(crate) cpuid: VcpuCpuid,
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
            guest_pat: od_isa::msr::IA32_PAT_RESET,
            guest_misc_enable: od_isa::msr::IA32_MISC_ENABLE_RESET,
            guest_bios_sign_id: 0,
            guest_mtrr: od_isa::msr::MtrrState::default(),
            guest_mca: od_isa::msr::McaState::default(),
            guest_fp: GuestFpState::initial(),
            legacy: None,
            skip_first_boundary: false,
            cpuid: VcpuCpuid::new(CpuidProfile::DiagnosticV1, index, 1),
        }
    }

    /// Replace the CPUID configuration. A legacy model that is still exactly the one INIT or
    /// power-on leaves (its `CPUID` source and EDX from the old configuration, nothing run) takes
    /// the new configuration; a model that has run or started keeps what it has.
    pub(crate) fn set_cpuid(&mut self, cpuid: VcpuCpuid) {
        if let Some(LegacyCpu::Real(cpu)) = self.legacy
            && cpu == init_cpu_for(cpu.mode.cr0, self.cpuid)
        {
            self.legacy = Some(LegacyCpu::Real(init_cpu_for(cpu.mode.cr0, cpuid)));
        }
        self.cpuid = cpuid;
    }

    /// An application processor after power-on: `WaitForSipi` with the real-mode reset state
    /// (SMP_CONTRACT.md 3, first rows). `IA32_EFER` is `0` (Table 12-1: no long mode before the
    /// guest enables it), not the boot processor's `FIRMWARE_EFER`. `guest_cr2` stays `None`, as
    /// for the boot processor at power-on: the board reads `None` as CR2 `0`.
    pub(crate) fn new_ap(index: usize) -> Self {
        let mut vcpu = Self::new(
            index,
            LongModeState {
                activity: Some(CpuActivity::WaitForSipi),
                ..LongModeState::default()
            },
        );
        vcpu.guest_efer = 0;
        vcpu.legacy = Some(LegacyCpu::Real(init_cpu_for(POWER_ON_CR0, vcpu.cpuid)));
        vcpu
    }

    /// The activity of this processor. A state that models no activity (`None`, the boot
    /// processor's before the first run) is a `Running` boot processor.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn activity(&self) -> CpuActivity {
        self.state.activity.unwrap_or(CpuActivity::Running)
    }

    /// The real-mode state of the processor, `None` when it is not in the real-mode model (the
    /// boot processor before its first INIT, a processor in the protected-mode model or in long
    /// mode).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn real_mode(&self) -> Option<RealModeState> {
        match self.legacy {
            Some(LegacyCpu::Real(cpu)) => Some(cpu.mode),
            _ => None,
        }
    }

    /// `EFLAGS.IF` of the processor, from the model it runs on.
    pub(crate) fn interrupts_enabled(&self) -> bool {
        let eflags = match &self.legacy {
            Some(legacy) => legacy.eflags(),
            None => self.state.rflags as u32,
        };
        eflags & EFLAGS_IF != 0
    }

    /// Apply the INIT and SIPI this processor took from its own Local APIC, on its own boundary
    /// (SMP_CONTRACT.md 3 and 4). INIT first, then SIPI.
    /// - INIT: the real-mode processor after INIT replaces `legacy` (whatever model it was on),
    ///   keeping only CD and NW of the old CR0, and the guest `IA32_EFER` and CR2 cells go to `0` (Table 12-1). CR2 becomes
    ///   `Some(0)`, not `None`: `None` would let the board fall back to the stale CR2 of `state`.
    ///   `nmi_blocked` is cleared too.
    ///   An application processor enters `WaitForSipi`. The boot processor keeps its role and is
    ///   not put to wait: it is `Running`.
    /// - SIPI: only a processor in `WaitForSipi` takes it; it becomes `Running` with CS and EIP
    ///   set from the vector. In any other state, and for the boot processor, it is ignored.
    ///
    /// `state` is changed only in `activity` and `nmi_blocked`: the legacy model holds the
    /// registers, and a processor that returns to long mode later takes the registers INIT leaves
    /// alone (`syscall`, `kernel_gs_base`, `x87`, `sse`) from it. The other guest cells (STAR, LSTAR, SYSENTER, FP)
    /// are kept: INIT does not clear them (SMP_CONTRACT.md 4).
    ///
    /// The device refuses an INIT addressed to the boot processor (BSP INIT is deferred to
    /// S6-15/S6-16), so the boot processor's INIT branch is reached only by a direct call.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn apply_startup(&mut self, pending: PendingStartup) -> StartupApplied {
        let mut applied = StartupApplied::default();
        if pending.init {
            // The boot processor models no real-mode CR0 before its first INIT: its CR0 is the
            // board's (`FIRMWARE_CR0`) unless the state carries one.
            let old_cr0 = match &self.legacy {
                Some(legacy) => legacy.cr0(),
                None => self.state.cr0.map_or(FIRMWARE_CR0 as u32, |cr0| cr0 as u32),
            };
            self.legacy = Some(LegacyCpu::Real(init_cpu_for(old_cr0, self.cpuid)));
            self.guest_efer = 0;
            self.guest_cr2 = Some(0);
            self.skip_first_boundary = false;
            // INIT-reset also ends the NMI-blocked state (the LAPIC latch goes with it, owner's
            // decision S6-12b (a)): the processor restarts in real mode with no handler running.
            self.state.nmi_blocked = false;
            self.state.activity = Some(if self.index == BOOT_VCPU_INDEX {
                CpuActivity::Running
            } else {
                CpuActivity::WaitForSipi
            });
            applied.init = true;
        }
        if let Some(vector) = pending.sipi
            && self.index != BOOT_VCPU_INDEX
            && self.state.activity == Some(CpuActivity::WaitForSipi)
        {
            let reset = match self.legacy {
                Some(LegacyCpu::Real(cpu)) => cpu,
                _ => init_cpu_for(POWER_ON_CR0, self.cpuid),
            };
            self.legacy = Some(LegacyCpu::Real(RealModeCpu {
                mode: reset.mode.after_sipi(vector),
                ..reset
            }));
            self.state.activity = Some(CpuActivity::Running);
            applied.started = true;
        }
        applied
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
        // The application processors wait for SIPI (S6-13); nothing else of them differs.
        assert!(set[1..].iter().all(|vcpu| vcpu.state
            == LongModeState {
                activity: Some(CpuActivity::WaitForSipi),
                ..LongModeState::default()
            }));
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
        assert!(!vcpu.skip_first_boundary);
        assert_eq!(vcpu.real_mode(), None);
    }

    // S6-12b: NMI to another vCPU waits in its LAPIC.

    #[test]
    fn an_nmi_to_a_vcpu_waiting_for_sipi_stays_pending_and_the_sender_has_none() {
        let mut board = assembled(3).unwrap();
        // vCPU 0 sends the NMI to everyone else (all-but-self, delivery mode 100).
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 0, (4 << 8) | (3 << 18))
            .unwrap();
        assert!(!board.local_apic_of(0).unwrap().pending_nmi());
        for cpu in [1, 2] {
            assert!(board.local_apic_of(cpu).unwrap().pending_nmi());
        }
        assert_eq!(board.deliver_pending_nmi(0), Ok(NmiBoundary::NoPending));
        // An application processor waits for SIPI: it takes no NMI, the NMI is neither delivered
        // nor lost, and its state is untouched.
        let before = board.vcpu(1).unwrap().state.clone();
        assert_eq!(board.deliver_pending_nmi(1), Ok(NmiBoundary::Held));
        assert_eq!(board.vcpu(1).unwrap().state, before);
        assert!(board.local_apic_of(1).unwrap().pending_nmi());
        // A targeted NMI reaches only its target; the other latch is untouched.
        assert!(board.local_apic_of(2).unwrap().take_nmi());
        assert!(board.local_apic_of(1).unwrap().take_nmi());
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 2, 4 << 8)
            .unwrap();
        assert!(board.local_apic_of(2).unwrap().pending_nmi());
        assert!(!board.local_apic_of(1).unwrap().pending_nmi());
        assert!(!board.local_apic_of(0).unwrap().pending_nmi());
    }

    #[test]
    fn a_started_ap_in_real_mode_is_an_error_for_an_nmi_and_the_nmi_stays_pending() {
        let mut board = assembled(2).unwrap();
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, (6 << 8) | 0x08)
            .unwrap();
        board.apply_pending_startup(1).unwrap();
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Running);
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, 4 << 8)
            .unwrap();
        let before = board.vcpu(1).unwrap().state.clone();
        assert_eq!(
            board.deliver_pending_nmi(1),
            Err(FirmwareBoardError::Config(
                "deliver_pending_nmi: NMI in real mode not modelled"
            ))
        );
        assert_eq!(board.vcpu(1).unwrap().state, before);
        assert!(board.local_apic_of(1).unwrap().pending_nmi());
    }

    #[test]
    fn init_ends_the_nmi_blocked_state_of_an_ap_and_of_the_bsp() {
        for index in [1, BOOT_VCPU_INDEX] {
            let mut vcpu = if index == BOOT_VCPU_INDEX {
                Vcpu::new(index, LongModeState::default())
            } else {
                Vcpu::new_ap(index)
            };
            vcpu.state.nmi_blocked = true;
            // SIPI alone does not touch the blocking; INIT clears it.
            vcpu.apply_startup(PendingStartup {
                init: false,
                sipi: Some(0x08),
            });
            assert!(vcpu.state.nmi_blocked, "vcpu {index}");
            vcpu.apply_startup(PendingStartup {
                init: true,
                sipi: None,
            });
            assert!(!vcpu.state.nmi_blocked, "vcpu {index}");
        }
    }

    // S6-13: start-up states of the application processors.

    use od_isa::legacy_mode::{INIT_EIP, RealSegment};

    /// ICR_LOW of INIT assert, INIT deassert and SIPI with `vector`: physical, no shorthand.
    const INIT_ASSERT: u32 = (5 << 8) | (3 << 14);
    const INIT_DEASSERT: u32 = (5 << 8) | (1 << 15);
    fn sipi(vector: u8) -> u32 {
        (6 << 8) | u32::from(vector)
    }
    const INIT_ONLY: PendingStartup = PendingStartup {
        init: true,
        sipi: None,
    };

    fn sipi_only(vector: u8) -> PendingStartup {
        PendingStartup {
            init: false,
            sipi: Some(vector),
        }
    }

    #[test]
    fn after_power_on_the_bsp_runs_and_every_ap_waits_for_sipi() {
        let board = assembled(4).unwrap();
        assert_eq!(
            board.vcpu(BOOT_VCPU_INDEX).unwrap().activity(),
            CpuActivity::Running
        );
        for index in 1..4 {
            let ap = board.vcpu(index).unwrap();
            assert_eq!(ap.activity(), CpuActivity::WaitForSipi, "vcpu {index}");
            // Power-on reset state of real mode: the INIT state with CD and NW set.
            assert_eq!(
                ap.real_mode(),
                Some(RealModeState::after_init(POWER_ON_CR0))
            );
            assert_eq!(ap.real_mode().unwrap().cr0, 0x6000_0010);
            // No long mode at power-on: EFER 0 (the BSP's cell holds FIRMWARE_EFER), CR2 0.
            assert_eq!(ap.guest_efer, 0, "vcpu {index}");
            assert_eq!(ap.guest_cr2, None, "vcpu {index}");
        }
        assert_eq!(
            board.vcpu(BOOT_VCPU_INDEX).unwrap().guest_efer,
            FIRMWARE_EFER
        );
        // With one vCPU there is only the BSP, and it runs.
        let one = assembled(1).unwrap();
        assert_eq!(one.vcpu_count(), 1);
        assert_eq!(one.vcpu(0).unwrap().activity(), CpuActivity::Running);
        assert!(one.vcpu(1).is_none());
    }

    #[test]
    fn an_ap_executes_nothing_before_sipi() {
        let mut board = assembled(2).unwrap();
        let before = board.vcpu(1).unwrap().state.clone();
        // The board has no per-AP instruction counter and runs only vCPU 0, so "executes
        // nothing" is checked through what execution would change: the whole `LongModeState` and
        // the real-mode state stay the initial ones over every boundary below. (The CPL0 set
        // also refuses the activity: od-isa `cpl0_profile_set.rs`, case "wait for sipi".)
        assert_eq!(
            before,
            LongModeState {
                activity: Some(CpuActivity::WaitForSipi),
                ..LongModeState::default()
            }
        );
        assert_eq!(
            board.apply_pending_startup(1),
            Ok(StartupApplied::default())
        );
        assert_eq!(board.vcpu(1).unwrap().state, before);
        assert_eq!(board.vcpu(1).unwrap().real_mode().unwrap().eip, INIT_EIP);
        // An INIT alone, a deassert and a SIPI to the BSP do not start it either.
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, INIT_ASSERT)
            .unwrap();
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, INIT_DEASSERT)
            .unwrap();
        board.apply_pending_startup(1).unwrap();
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 0, sipi(0x08))
            .unwrap();
        board.apply_pending_startup(0).unwrap();
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::WaitForSipi);
        assert_eq!(board.vcpu(1).unwrap().state, before);
        assert_eq!(
            board.vcpu(1).unwrap().real_mode(),
            Some(RealModeState::after_init(POWER_ON_CR0))
        );
    }

    #[test]
    fn the_target_applies_a_pending_sipi_only_at_its_own_boundary() {
        let mut board = assembled(2).unwrap();
        let sender = board.local_apic_of(0).unwrap();
        sender.write_icr(0, 1, sipi(0x08)).unwrap();
        // Written by vCPU 0, pending in the LAPIC of vCPU 1, nothing changed in vCPU 1 yet.
        assert_eq!(
            board.local_apic_of(1).unwrap().pending_startup(),
            sipi_only(0x08)
        );
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::WaitForSipi);
        assert_eq!(
            board.vcpu(1).unwrap().real_mode(),
            Some(RealModeState::after_init(POWER_ON_CR0))
        );
        // The boundary of vCPU 0 takes nothing of vCPU 1's.
        assert_eq!(
            board.apply_pending_startup(0),
            Ok(StartupApplied::default())
        );
        assert_eq!(
            board.local_apic_of(1).unwrap().pending_startup(),
            sipi_only(0x08)
        );
        // The boundary of vCPU 1 starts it at vector << 12.
        assert_eq!(
            board.apply_pending_startup(1),
            Ok(StartupApplied {
                init: false,
                started: true
            })
        );
        let ap = board.vcpu(1).unwrap();
        assert_eq!(ap.activity(), CpuActivity::Running);
        let mode = ap.real_mode().unwrap();
        assert_eq!(mode.cs.selector, 0x0800);
        assert_eq!(mode.cs.base, 0x8000);
        assert_eq!(mode.eip, 0);
        assert_eq!(mode.fetch_linear_address(), 0x8000);
        assert_eq!(
            board.local_apic_of(1).unwrap().pending_startup(),
            PendingStartup::default()
        );
        assert_eq!(
            board.apply_pending_startup(5),
            Err(FirmwareBoardError::Config(
                "apply_pending_startup: no such vCPU"
            ))
        );
    }

    #[test]
    fn a_second_sipi_to_a_started_ap_is_ignored() {
        let mut board = assembled(2).unwrap();
        let sender = board.local_apic_of(0).unwrap();
        sender.write_icr(0, 1, sipi(0x08)).unwrap();
        board.apply_pending_startup(1).unwrap();
        let started = board.vcpu(1).unwrap().real_mode();
        let state = board.vcpu(1).unwrap().state.clone();
        sender.write_icr(0, 1, sipi(0x20)).unwrap();
        assert_eq!(
            board.apply_pending_startup(1),
            Ok(StartupApplied::default())
        );
        assert_eq!(board.vcpu(1).unwrap().real_mode(), started);
        assert_eq!(board.vcpu(1).unwrap().state, state);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Running);
        // The ignored SIPI is consumed, not left to start the AP after a later INIT.
        assert_eq!(
            board.local_apic_of(1).unwrap().pending_startup(),
            PendingStartup::default()
        );
        // Two SIPIs queued before the first is applied: the first one starts the AP.
        let mut board = assembled(2).unwrap();
        let sender = board.local_apic_of(0).unwrap();
        sender.write_icr(0, 1, sipi(0x08)).unwrap();
        sender.write_icr(0, 1, sipi(0x20)).unwrap();
        assert_eq!(
            board.apply_pending_startup(1),
            Ok(StartupApplied {
                init: false,
                started: true
            })
        );
        assert_eq!(board.vcpu(1).unwrap().real_mode().unwrap().cs.base, 0x8000);
    }

    #[test]
    fn init_to_a_running_ap_returns_it_to_wait_for_sipi_with_the_init_state() {
        let mut board = assembled(2).unwrap();
        let sender = board.local_apic_of(0).unwrap();
        sender.write_icr(0, 1, sipi(0x08)).unwrap();
        board.apply_pending_startup(1).unwrap();
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Running);
        // A request waiting in the AP's LAPIC is local interrupt state that INIT clears.
        board
            .local_apic_of(1)
            .unwrap()
            .request_fixed_edge(0x61)
            .unwrap();
        sender.write_icr(0, 1, INIT_ASSERT).unwrap();
        assert_eq!(
            board.apply_pending_startup(1),
            Ok(StartupApplied {
                init: true,
                started: false
            })
        );
        let ap = board.vcpu(1).unwrap();
        assert_eq!(ap.activity(), CpuActivity::WaitForSipi);
        // CD and NW of the old CR0 are kept, everything else is the state of Table 12-1.
        assert_eq!(
            ap.real_mode(),
            Some(RealModeState::after_init(POWER_ON_CR0))
        );
        assert_eq!(ap.real_mode().unwrap().cs.base, 0xFFFF_0000);
        assert_eq!(ap.real_mode().unwrap().eip, INIT_EIP);
        assert_eq!(board.local_apic_of(1).unwrap().pending(), None);
        assert!(
            board.local_apic_of(1).unwrap().take_startup().unwrap() == PendingStartup::default()
        );
    }

    #[test]
    fn init_resets_the_old_cr0_of_an_ap_keeping_cd_and_nw_and_a_halted_ap_waits_again() {
        let mut ap = Vcpu::new_ap(1);
        ap.legacy = Some(LegacyCpu::Real(RealModeCpu {
            mode: RealModeState {
                cr0: 0xE000_003B,
                ..RealModeState::after_init(0)
            },
            ..init_cpu(0)
        }));
        ap.state.activity = Some(CpuActivity::Halted);
        assert_eq!(
            ap.apply_startup(INIT_ONLY),
            StartupApplied {
                init: true,
                started: false
            }
        );
        assert_eq!(ap.activity(), CpuActivity::WaitForSipi);
        assert_eq!(ap.real_mode(), Some(RealModeState::after_init(0x6000_0000)));
        assert_eq!(ap.real_mode().unwrap().cr0, 0x6000_0010);
        // A processor that shut down is also taken back by INIT (hardware_reset_only is not
        // modelled yet, so the `false` row of SMP_CONTRACT.md 3 applies).
        ap.state.activity = Some(CpuActivity::Shutdown);
        ap.apply_startup(INIT_ONLY);
        assert_eq!(ap.activity(), CpuActivity::WaitForSipi);
    }

    #[test]
    fn a_sipi_after_an_init_in_the_same_boundary_starts_the_ap_and_a_sipi_alone_does_not_start_a_halted_one()
     {
        let mut ap = Vcpu::new_ap(2);
        ap.state.activity = Some(CpuActivity::Running);
        let applied = ap.apply_startup(PendingStartup {
            init: true,
            sipi: Some(0x10),
        });
        assert_eq!(
            applied,
            StartupApplied {
                init: true,
                started: true
            }
        );
        assert_eq!(ap.activity(), CpuActivity::Running);
        let mode = ap.real_mode().unwrap();
        assert_eq!(
            (mode.cs.selector, mode.cs.base, mode.eip),
            (0x1000, 0x10000, 0)
        );
        // The other segments, EFLAGS and CR0 are those of INIT.
        assert_eq!(mode.ds, RealSegment::reset_zero());
        assert_eq!(mode.eflags, 2);
        assert_eq!(mode.cr0, 0x6000_0010);
        // A SIPI alone does not take a halted AP out of HLT.
        let mut halted = Vcpu::new_ap(1);
        halted.state.activity = Some(CpuActivity::Halted);
        let before = halted.real_mode();
        assert_eq!(
            halted.apply_startup(sipi_only(0x08)),
            StartupApplied::default()
        );
        assert_eq!(halted.activity(), CpuActivity::Halted);
        assert_eq!(halted.real_mode(), before);
    }

    #[test]
    fn init_deassert_changes_no_vcpu_state() {
        let mut board = assembled(2).unwrap();
        let sender = board.local_apic_of(0).unwrap();
        sender.write_icr(0, 1, sipi(0x08)).unwrap();
        board.apply_pending_startup(1).unwrap();
        let ap_state = board.vcpu(1).unwrap().state.clone();
        let ap_mode = board.vcpu(1).unwrap().real_mode();
        let bsp_state = board.vcpu(0).unwrap().state.clone();
        for target in [0, 1] {
            sender.write_icr(0, target, INIT_DEASSERT).unwrap();
            assert_eq!(
                board
                    .local_apic_of(usize::from(target))
                    .unwrap()
                    .pending_startup(),
                PendingStartup::default()
            );
            assert_eq!(
                board.apply_pending_startup(usize::from(target)),
                Ok(StartupApplied::default())
            );
        }
        assert_eq!(board.vcpu(1).unwrap().state, ap_state);
        assert_eq!(board.vcpu(1).unwrap().real_mode(), ap_mode);
        assert_eq!(board.vcpu(0).unwrap().state, bsp_state);
        assert_eq!(board.vcpu(0).unwrap().real_mode(), None);
    }

    #[test]
    fn init_to_the_bsp_is_refused_by_the_device_and_a_sipi_to_it_changes_nothing() {
        // BSP INIT is deferred to S6-15/S6-16: the device refuses it before anything changes.
        let mut board = assembled(2).unwrap();
        let sender = board.local_apic_of(1).unwrap();
        let bsp_state = board.vcpu(0).unwrap().state.clone();
        assert_eq!(
            sender.write_icr(1, 0, INIT_ASSERT),
            Err(crate::DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(
            board.local_apic_of(0).unwrap().pending_startup(),
            PendingStartup::default()
        );
        assert_eq!(
            board.apply_pending_startup(0),
            Ok(StartupApplied::default())
        );
        // A SIPI to the BSP is consumed and changes nothing.
        sender.write_icr(1, 0, sipi(0x08)).unwrap();
        assert_eq!(
            board.apply_pending_startup(0),
            Ok(StartupApplied::default())
        );
        let bsp = board.vcpu(0).unwrap();
        assert_eq!(bsp.state, bsp_state);
        assert_eq!(bsp.real_mode(), None);
        assert_eq!(bsp.activity(), CpuActivity::Running);
        // With one vCPU the INIT addressed to itself is refused too.
        let one = assembled(1).unwrap();
        assert_eq!(
            one.local_apic_of(0).unwrap().write_icr(0, 0, INIT_ASSERT),
            Err(crate::DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(
            one.local_apic_of(0).unwrap().pending_startup(),
            PendingStartup::default()
        );
    }

    #[test]
    fn a_direct_init_of_the_bsp_vcpu_takes_cr0_from_the_board_not_from_power_on() {
        // Not reachable through the device (see above); the branch itself must still be right.
        // Expected values are worked out by hand from Table 12-1 footnote 2,
        // CR0 = (old & 0x6000_0000) | 0x10.
        // FIRMWARE_CR0 0x8001_0033 has CD and NW clear: 0x0000_0010 (not the power-on 0x6000_0010).
        let mut bsp = Vcpu::new(BOOT_VCPU_INDEX, LongModeState::default());
        assert_eq!(
            bsp.apply_startup(INIT_ONLY),
            StartupApplied {
                init: true,
                started: false
            }
        );
        assert_eq!(bsp.real_mode().unwrap().cr0, 0x0000_0010);
        assert_eq!(bsp.activity(), CpuActivity::Running);
        // A CR0 carried by the state is used instead: CD set, NW clear -> 0x4000_0010.
        let mut bsp = Vcpu::new(
            BOOT_VCPU_INDEX,
            LongModeState {
                cr0: Some(0x4001_0033),
                ..LongModeState::default()
            },
        );
        bsp.apply_startup(INIT_ONLY);
        assert_eq!(bsp.real_mode().unwrap().cr0, 0x4000_0010);
        // A SIPI at the same boundary is still ignored by the BSP.
        let applied = bsp.apply_startup(PendingStartup {
            init: true,
            sipi: Some(0x08),
        });
        assert!(applied.init && !applied.started);
        assert_eq!(bsp.real_mode().unwrap().eip, INIT_EIP);
        assert_eq!(bsp.activity(), CpuActivity::Running);
    }

    #[test]
    fn init_clears_the_skip_first_boundary_flag() {
        // A partial REP left the flag set; INIT restarts the processor (SDM Vol. 3A 9.1), so the
        // interrupted instruction is gone and the next call must not skip its first boundary.
        let mut ap = Vcpu::new_ap(1);
        ap.skip_first_boundary = true;
        let applied = ap.apply_startup(INIT_ONLY);
        assert!(applied.init);
        assert!(!ap.skip_first_boundary);
        // Without INIT (SIPI only, no effect on a running state) the flag is not touched.
        let mut other = Vcpu::new_ap(2);
        other.skip_first_boundary = true;
        other.apply_startup(sipi_only(0x10));
        assert!(other.skip_first_boundary);
    }

    #[test]
    fn init_resets_efer_and_cr2_of_an_ap_and_keeps_the_other_guest_cells() {
        let mut ap = Vcpu::new_ap(1);
        ap.state.activity = Some(CpuActivity::Running);
        ap.state.cr2 = Some(0x1111);
        ap.guest_efer = 0xD01;
        ap.guest_cr2 = Some(0xFFFF_8000_1234_5678);
        ap.guest_star = 1;
        ap.guest_lstar = 2;
        ap.guest_cstar = 3;
        ap.guest_sysenter_cs = 4;
        ap.guest_sysenter_esp = 5;
        ap.guest_sysenter_eip = 6;
        ap.guest_syscall_mask = 7;
        ap.guest_kernel_gs_base = 8;
        // S7-32 slice F: the Windows MSR cells (PAT, MISC_ENABLE, BIOS_SIGN_ID) are not changed by
        // INIT either (SDM Vol. 3A Table 12-1 names neither; PAT keeps its value across INIT).
        ap.guest_pat = 0x0606_0606_0606_0606;
        ap.guest_misc_enable = 0x1800;
        ap.guest_bios_sign_id = 0x24 << 32;
        // S7-32 slice I: the MTRRs and the MCA MSRs are unchanged by INIT (SDM Vol. 3A, table "IA-32
        // and Intel 64 Processor States Following Power-up, Reset, or INIT": INIT "Unchanged").
        let mut mtrr = od_isa::msr::MtrrState::default();
        assert_eq!(
            mtrr.write(od_isa::msr::IA32_MTRR_DEF_TYPE, 0xc06),
            Some(od_isa::msr::MsrWrite::Accept)
        );
        ap.guest_mtrr = mtrr;
        let mut mca = od_isa::msr::McaState::default();
        assert_eq!(
            mca.write(od_isa::msr::IA32_MC0_CTL, u64::MAX),
            Some(od_isa::msr::MsrWrite::Accept)
        );
        ap.guest_mca = mca;
        let mut fp = GuestFpState::initial();
        fp.sse.mxcsr = 0x1F00;
        ap.guest_fp = fp.clone();
        let state_before = ap.state.clone();
        ap.apply_startup(INIT_ONLY);
        assert_eq!(ap.guest_efer, 0);
        assert_eq!(ap.guest_cr2, Some(0));
        // Only `activity` of the long-mode state changed (including its stale CR2 field).
        assert_eq!(
            ap.state,
            LongModeState {
                activity: Some(CpuActivity::WaitForSipi),
                ..state_before
            }
        );
        // Table 12-1 and SMP_CONTRACT.md 4: STAR, LSTAR, SYSENTER and FP are not cleared by INIT;
        // the remaining MSR cells are not named there and stay as they were.
        assert_eq!(
            (
                ap.guest_star,
                ap.guest_lstar,
                ap.guest_cstar,
                ap.guest_sysenter_cs,
                ap.guest_sysenter_esp,
                ap.guest_sysenter_eip,
                ap.guest_syscall_mask,
                ap.guest_kernel_gs_base
            ),
            (1, 2, 3, 4, 5, 6, 7, 8)
        );
        assert_eq!(
            (ap.guest_pat, ap.guest_misc_enable, ap.guest_bios_sign_id),
            (0x0606_0606_0606_0606, 0x1800, 0x24 << 32)
        );
        assert_eq!((ap.guest_mtrr, ap.guest_mca), (mtrr, mca));
        assert_eq!(ap.guest_mtrr.def_type(), 0xc06);
        assert_eq!(ap.guest_fp, fp);
    }

    #[test]
    fn an_init_or_sipi_to_a_vcpu_the_board_does_not_have_is_refused() {
        let board = assembled(1).unwrap();
        let apic = board.local_apic_of(0).unwrap();
        for command in [INIT_ASSERT, sipi(0x08)] {
            assert_eq!(
                apic.write_icr(0, 1, command),
                Err(crate::DeviceError::UnsupportedConfiguration)
            );
        }
        assert_eq!(apic.pending_startup(), PendingStartup::default());
    }

    /// S6-18a: one vCPU keeps the CPUID bytes it had (the plain profile source, EDX `0x600`); N > 1
    /// numbers the processors and gives each the count.
    #[test]
    fn s6_18a_the_vcpu_set_gives_each_vcpu_its_apic_id_and_the_count() {
        let one = vcpu_set(LongModeState::default(), 1);
        assert_eq!(
            one[0].cpuid,
            VcpuCpuid::new(CpuidProfile::DiagnosticV1, 0, 1)
        );
        assert_eq!(
            one[0].cpuid.source(),
            CpuidSource::Profile(CpuidProfile::DiagnosticV1)
        );
        let four = vcpu_set(LongModeState::default(), 4);
        for (i, vcpu) in four.iter().enumerate() {
            assert_eq!(vcpu.cpuid.apic_id as usize, i);
            assert_eq!(vcpu.cpuid.cpu_count, 4);
            assert_eq!(vcpu.cpuid.init_edx(), 0x600);
            if i > 0 {
                let Some(LegacyCpu::Real(cpu)) = vcpu.legacy else {
                    panic!("AP {i}")
                };
                assert_eq!(cpu.gpr[2], 0x600, "EDX after power-on");
                assert_eq!(
                    cpu.cpuid,
                    CpuidSource::ProfileOnVcpu {
                        profile: CpuidProfile::DiagnosticV1,
                        apic_id: i as u8,
                        cpu_count: 4
                    }
                );
            }
        }
    }

    /// S6-18a: `set_cpuid_profile` re-seeds the pristine legacy models (source and EDX from the
    /// profile), INIT keeps the vCPU's configuration, and a model that was started keeps what it
    /// has. A Linux board (`into_linux_board`) has the AP profile from the start; with one vCPU the
    /// profile is set too and no processor has a legacy model.
    #[test]
    fn s6_18a_the_board_profile_reaches_the_legacy_models_and_survives_init() {
        let mut board = assembled(2).unwrap();
        assert_eq!(board.cpuid_profile(), LINUX_AP_CPUID_PROFILE);
        assert_eq!(LINUX_AP_CPUID_PROFILE, CpuidProfile::OdX8664V1Neutral);
        let ap_cpuid = |board: &crate::LoadedFirmwareBoard| {
            let Some(LegacyCpu::Real(cpu)) = board.vcpu(1).unwrap().legacy else {
                panic!("AP legacy")
            };
            (cpu.cpuid, cpu.gpr[2])
        };
        assert_eq!(
            ap_cpuid(&board),
            (
                CpuidSource::ProfileOnVcpu {
                    profile: CpuidProfile::OdX8664V1Neutral,
                    apic_id: 1,
                    cpu_count: 2
                },
                // EDX = CPUID.01H:EAX of the profile (0x600 for every profile of `cpuid.rs`).
                0x600
            )
        );
        board.set_cpuid_profile(CpuidProfile::X8664V1Stage4AApic);
        assert_eq!(
            ap_cpuid(&board).0,
            CpuidSource::ProfileOnVcpu {
                profile: CpuidProfile::X8664V1Stage4AApic,
                apic_id: 1,
                cpu_count: 2
            }
        );
        // INIT and SIPI on the AP keep the configuration.
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, INIT_ASSERT)
            .unwrap();
        board.apply_pending_startup(1).unwrap();
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, sipi(0x08))
            .unwrap();
        board.apply_pending_startup(1).unwrap();
        let started = ap_cpuid(&board);
        assert_eq!(started.1, 0x600);
        assert_eq!(
            board.vcpu(1).unwrap().cpuid,
            VcpuCpuid::new(CpuidProfile::X8664V1Stage4AApic, 1, 2)
        );
        // A started model keeps its source when the profile changes later.
        board.set_cpuid_profile(CpuidProfile::DiagnosticV1);
        assert_eq!(ap_cpuid(&board), started);
        assert_eq!(board.cpuid_profile(), CpuidProfile::DiagnosticV1);

        let one = assembled(1).unwrap();
        assert_eq!(one.cpuid_profile(), LINUX_AP_CPUID_PROFILE);
        assert!(one.vcpu(0).unwrap().legacy.is_none());
    }

    #[test]
    fn a_board_without_a_recorded_entry_state_refuses_a_restart_and_changes_nothing() {
        // S4-21e: the Linux-profile assembly leaves the entry state to its caller.
        let mut board = assembled(2).unwrap();
        board.state_mut().rip = 0x1234;
        board.local_apic().request_fixed_edge(0x41).unwrap();
        assert_eq!(
            board.restart_vcpus(),
            Err(FirmwareBoardError::Config(
                "restart_vcpus: the board has no recorded entry state"
            ))
        );
        assert_eq!(board.state().rip, 0x1234);
        assert_eq!(board.local_apic().pending(), Some(0x41));
    }
}
