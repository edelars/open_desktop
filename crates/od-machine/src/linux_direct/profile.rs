//! Linux CPL0 instruction profiles and their per-vCPU backing state.

use super::LinuxCpl0SetMembers;
use crate::vcpu::Vcpu;
use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet, GuestFpState, MovCr3Long64V1};
use od_isa::msr::MsrProfile;
use od_isa::{CliProfile, HaltProfile, StiProfile};
use std::cell::{Cell, RefCell};
use std::fmt::Arguments;

/// Backs the ISA profile for one vCPU. The caller commits these cells when execution stops,
/// including on an error after a retired prefix.
pub(super) struct LinuxVcpuCells {
    efer: Cell<u64>,
    star: Cell<u64>,
    lstar: Cell<u64>,
    cstar: Cell<u64>,
    sysenter_cs: Cell<u64>,
    sysenter_esp: Cell<u64>,
    sysenter_eip: Cell<u64>,
    syscall_mask: Cell<u64>,
    kernel_gs_base: Cell<u64>,
    fp: RefCell<GuestFpState>,
}

impl LinuxVcpuCells {
    pub(super) fn from_vcpu(vcpu: &Vcpu) -> Self {
        Self {
            efer: Cell::new(vcpu.guest_efer),
            star: Cell::new(vcpu.guest_star),
            lstar: Cell::new(vcpu.guest_lstar),
            cstar: Cell::new(vcpu.guest_cstar),
            sysenter_cs: Cell::new(vcpu.guest_sysenter_cs),
            sysenter_esp: Cell::new(vcpu.guest_sysenter_esp),
            sysenter_eip: Cell::new(vcpu.guest_sysenter_eip),
            syscall_mask: Cell::new(vcpu.guest_syscall_mask),
            kernel_gs_base: Cell::new(vcpu.guest_kernel_gs_base),
            fp: RefCell::new(vcpu.guest_fp.clone()),
        }
    }

    pub(super) fn commit(self, vcpu: &mut Vcpu) {
        vcpu.guest_efer = self.efer.into_inner();
        vcpu.guest_star = self.star.into_inner();
        vcpu.guest_lstar = self.lstar.into_inner();
        vcpu.guest_cstar = self.cstar.into_inner();
        vcpu.guest_sysenter_cs = self.sysenter_cs.into_inner();
        vcpu.guest_sysenter_esp = self.sysenter_esp.into_inner();
        vcpu.guest_sysenter_eip = self.sysenter_eip.into_inner();
        vcpu.guest_syscall_mask = self.syscall_mask.into_inner();
        vcpu.guest_kernel_gs_base = self.kernel_gs_base.into_inner();
        vcpu.guest_fp = self.fp.into_inner();
    }
}

pub(super) struct LinuxCpl0Profile {
    members: LinuxCpl0SetMembers,
    user_mode: bool,
    msr: MsrProfile,
}

impl LinuxCpl0Profile {
    pub(super) fn new(members: LinuxCpl0SetMembers, user_mode: bool, board_efer: u64) -> Self {
        Self {
            members,
            user_mode,
            // The profile names the MSR member; the current guest EFER comes from the vCPU.
            msr: MsrProfile::EferReadOnlyV1(board_efer),
        }
    }

    /// Bind the instruction set to one vCPU and, when enabled, the board's PCI address latch.
    /// The member callback records the selections for the LinuxDirect report.
    pub(super) fn bind<'a>(
        &'a self,
        cpu: &'a LinuxVcpuCells,
        pci_config: &'a Cell<u32>,
        mut record_member: impl FnMut(Arguments<'_>),
    ) -> Cpl0ProfileSet<'a> {
        let members = &self.members;
        record_member(format_args!("cli"));
        let mut set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cli(&CliProfile::NonPviLong64V1);
        if members.cpu_owned_cr3 {
            set = set.with_cpu_owned_cr3(MovCr3Long64V1);
            record_member(format_args!("cpu_owned_cr3"));
        }
        if members.cpuid {
            set = set.with_cpuid(&members.cpuid_profile);
            record_member(format_args!("cpuid:{:?}", members.cpuid_profile));
        }
        if members.rdtsc {
            set = set.with_rdtsc();
            record_member(format_args!("rdtsc"));
        }
        if members.msr {
            set = set.with_msr(&self.msr).with_msr_efer_write(&cpu.efer);
            record_member(format_args!("msr:{:?}", self.msr));
        }
        if members.msr_fs_gs_base {
            set = set.with_msr_fs_gs_base();
            record_member(format_args!("msr:fs_gs_base"));
        }
        if members.msr_star {
            set = set.with_msr_star_write(&cpu.star);
            record_member(format_args!("msr:star_write"));
        }
        if members.msr_lstar {
            set = set.with_msr_lstar_write(&cpu.lstar);
            record_member(format_args!("msr:lstar_write"));
        }
        if members.msr_cstar {
            set = set.with_msr_cstar_write(&cpu.cstar);
            record_member(format_args!("msr:cstar_write"));
        }
        if members.msr_sysenter_cs {
            set = set.with_msr_sysenter_cs_write(&cpu.sysenter_cs);
            record_member(format_args!("msr:sysenter_cs_write"));
        }
        if members.msr_sysenter_stack {
            set = set.with_msr_sysenter_stack_write(&cpu.sysenter_esp, &cpu.sysenter_eip);
            record_member(format_args!("msr:sysenter_stack_write"));
        }
        if members.msr_syscall_mask {
            set = set.with_msr_syscall_mask_write(&cpu.syscall_mask);
            record_member(format_args!("msr:syscall_mask_write"));
        }
        if members.msr_kernel_gs_base {
            set = set.with_msr_kernel_gs_base_write(&cpu.kernel_gs_base);
            record_member(format_args!("msr:kernel_gs_base_write"));
        }
        if members.msr_tdx_keyid_partitioning_read {
            set = set.with_msr_tdx_keyid_partitioning_read();
            record_member(format_args!("msr:tdx_keyid_partitioning_read"));
        }
        if members.msr_apic_base {
            set = set.with_msr_apic_base();
            record_member(format_args!("msr:apic_base"));
        }
        if members.fp_state {
            set = set.with_fp_state(&cpu.fp);
            record_member(format_args!("fp_state"));
        }
        if members.int3 {
            set = set.with_int3();
            record_member(format_args!("int3"));
        }
        if members.halt {
            set = set.with_halt(&HaltProfile::Long64V1);
            record_member(format_args!("halt"));
        }
        if members.pio_u8 {
            set = set.with_pio_u8();
            record_member(format_args!("pio_u8"));
        }
        if members.sti {
            set = set.with_sti(&StiProfile::NonPviLong64V1);
            record_member(format_args!("sti"));
        }
        if members.popf_cpl0 {
            set = set.with_popf_cpl0();
            record_member(format_args!("popf_cpl0"));
        }
        if members.pci_config {
            set = set.with_pci_config(pci_config);
            record_member(format_args!("pci_config"));
        }
        if members.irq {
            record_member(format_args!("irq"));
        }
        if members.clock_owner {
            record_member(format_args!("clock_owner"));
        }
        if self.user_mode {
            set = set.with_user_mode();
            record_member(format_args!("user_mode"));
            // S4-39e: `SYSCALL` from CPL3 consumes the EFER, STAR, LSTAR and SYSCALL_MASK cells; the
            // member joins only a user-mode set that carries all four (the stage4a members do).
            // Without one of them `SYSCALL` keeps stopping the run as `user_mode_unsupported`.
            if members.msr && members.msr_star && members.msr_lstar && members.msr_syscall_mask {
                set = set.with_syscall();
                record_member(format_args!("syscall"));
            }
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware_board::FirmwareBoardError;
    use crate::firmware_board::vcpu_quantum::{
        fixture::{CODE_1, STACK_1, TABLES_1, board, install, long_state, patch, tables},
        run_vcpu_set_quantum_shared,
    };
    use crate::parallel_run::{ParallelConfig, ParallelFinish, QuantumCall, run_threads};
    use crate::vcpu::BOOT_VCPU_INDEX;
    use od_isa::CpuActivity;
    use od_isa::cpuid::CpuidProfile;
    use od_isa::real_mode_exec::EFLAGS_IF;
    use std::time::Duration;

    #[test]
    fn linux_profile_commits_the_ap_msr_and_fp_state_before_a_worker_error() {
        const AP: usize = 1;
        const EFER: u64 = 0xD00;
        const BSP_STAR: u64 = 0x0043_0030_0000_0000;
        const AP_STAR: u64 = 0x0023_0010_0000_0000;
        let mut board = board(2);
        let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        let code = [
            0xB9, 0x81, 0x00, 0x00, 0xC0, // mov ecx, IA32_STAR
            0xB8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0
            0xBA, 0x10, 0x00, 0x23, 0x00, // mov edx, high half of AP_STAR
            0x0F, 0x30, // wrmsr
            0xDB, 0xE3, // fninit
            0xF1, // outside the admitted profile
        ];
        patch(&mut board, CODE_1, &code);
        install(
            &mut board,
            AP,
            long_state(CODE_1, STACK_1, cr3, false),
            EFER,
        );

        let bsp = board.vcpu_mut(BOOT_VCPU_INDEX).unwrap();
        bsp.state.activity = Some(CpuActivity::Halted);
        bsp.state.rflags &= !u64::from(EFLAGS_IF);
        bsp.guest_star = BSP_STAR;
        bsp.guest_fp.x87.control_word = 0x027F;
        let bsp_fp = bsp.guest_fp.clone();
        let ap = board.vcpu_mut(AP).unwrap();
        ap.guest_fp.x87.control_word = 0x007F;
        ap.guest_fp.sse.xmm[15] = [0xEE; 16];

        let members = LinuxCpl0SetMembers {
            cpuid_profile: CpuidProfile::X8664V1Stage4A,
            msr_star: true,
            fp_state: true,
            pci_config: false,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let profile = LinuxCpl0Profile::new(members, false, EFER);
        let quantum = |call: QuantumCall<'_, '_>| {
            assert_eq!(call.cpu, AP);
            let cells = LinuxVcpuCells::from_vcpu(call.vcpu);
            let pci_config = Cell::new(0);
            let set = profile.bind(&cells, &pci_config, |_| {});
            let outcome = run_vcpu_set_quantum_shared(
                call.env,
                call.vcpu,
                call.cpu,
                call.lapic,
                call.source,
                Some(call.gate),
                call.budget,
                call.stop,
                &set,
                call.consecutive_faults,
            );
            cells.commit(call.vcpu);
            outcome
        };
        let config = ParallelConfig {
            quantum: 64,
            budget: 64,
            ..ParallelConfig::default()
        };
        let (report, ()) = run_threads(&mut board, config, &quantum, |control| {
            assert!(control.wait_finished(Duration::from_secs(5)));
        })
        .unwrap();

        assert!(
            matches!(
                report.finished,
                ParallelFinish::Error {
                    cpu: AP,
                    error: FirmwareBoardError::VcpuQuantum { .. },
                }
            ),
            "{report:?}"
        );
        assert_eq!(report.threads_exited, 2);
        assert_eq!(report.cpus[BOOT_VCPU_INDEX].retired, 0);
        assert_eq!(report.cpus[AP].retired, 5);
        let ap = board.vcpu(AP).unwrap();
        assert_eq!(ap.state.rip, CODE_1 + code.len() as u64 - 1);
        assert_eq!(ap.guest_star, AP_STAR);
        assert_eq!(ap.guest_fp.x87.control_word, 0x037F);
        assert_eq!(ap.guest_fp.sse.xmm[15], [0xEE; 16]);
        let bsp = board.vcpu(BOOT_VCPU_INDEX).unwrap();
        assert_eq!(bsp.guest_star, BSP_STAR);
        assert_eq!(bsp.guest_fp, bsp_fp);
    }
}
