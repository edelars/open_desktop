//! The application processor between SIPI and long mode, on the board's RAM (S6-16c).
//!
//! After a SIPI the processor is a [`LegacyCpu`](crate::vcpu::LegacyCpu): the real-mode model of
//! the trampoline ([`RealModeCpu`]), then, after its far jump into a protected code segment, the
//! protected-mode model ([`ProtectedCpu`]) up to the 64-bit jump of the trampoline's last block.
//! [`run_legacy`] executes it step by step in the shared RAM of the board; at the jump
//! (`ProtectedCpu::target_entered`) the state becomes a `LongModeState`
//! ([`ProtectedCpu::into_long_mode`], S6-16b), `legacy` is `None`, and the caller continues with the
//! ordinary long-mode quantum in the same call (`BoardSmpHost::run`).
//!
//! What the models see is RAM only ([`BoardRam`]): an instruction fetch, a data access, a
//! descriptor or page-table access outside the board's RAM (MMIO, ROM, a hole) is a refusal of the
//! model, which is an [`ApLegacyRefusal`] here, never a panic and never a device access. Writes the
//! models make (the `LOCK BTS` operand, the accessed bit of a descriptor) go through
//! `PhysicalMemory::store_bytes`, the tracked path that advances the code generation of the written
//! pages like every other guest store. The accessed and dirty bits of a paging entry are one host
//! compare-exchange of the entry (`PhysicalMemory::compare_replace_u64`, S6-31b).
//!
//! Not here: interrupts and NMI in legacy mode (the board refuses NMI, `deliver_pending_nmi`), the
//! devices of the board (the trampoline models have no port or MMIO access), an instruction budget
//! per device tick. The clock is the scheduler's: the call returns what it retired and moves nothing.

// No production caller yet: the scheduler is wired in by S6-16d/S6-17; tests exercise it now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::smp_scheduler::Executed;
use crate::vcpu::LegacyCpu;
use crate::{
    FirmwareBoardError, GuestPhysicalAddress, LoadedFirmwareBoard, MemoryRegionKind, PhysicalMemory,
};
use od_isa::long_mode_entry::LongEntryRefusal;
use od_isa::protected_exec::{ProtectedCpu, ProtectedEntryRefusal, ProtectedRefusal};
use od_isa::real_mode_exec::{CR0_PE, RealModeMemory, RealModeRefusal};

/// Why an application processor in legacy mode could not go on. The processor keeps its state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApLegacyRefusal {
    /// The real-mode model refused the instruction at `CS:IP` (unsupported form, an access outside
    /// the RAM of the board, a limit, ...).
    Real(RealModeRefusal),
    /// The protected-mode model refused (the same for the compatibility and 64-bit steps).
    Protected(ProtectedRefusal),
    /// The processor left real mode but `ProtectedCpu::from_real_entry` refused the entry.
    ProtectedEntry(ProtectedEntryRefusal),
    /// The 64-bit jump retired but the state is not a 64-bit one `into_long_mode` can convert.
    LongEntry(LongEntryRefusal),
}

/// What [`run_legacy`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LegacyRun {
    /// The legacy steps retired by the call (the unit of the budget).
    pub(crate) executed: Executed,
    /// The state of the vCPU is a `LongModeState` now (`legacy` is `None`): the caller continues
    /// with the long-mode quantum on what is left of the budget.
    pub(crate) entered_long_mode: bool,
}

/// RAM of the board as the memory of the legacy models: a byte is readable or writable only in a
/// region of kind RAM, so MMIO, ROM, the framebuffer and holes are refused.
pub(crate) struct BoardRam<'a> {
    memory: &'a PhysicalMemory,
}

impl<'a> BoardRam<'a> {
    pub(crate) fn new(memory: &'a PhysicalMemory) -> Self {
        Self { memory }
    }

    fn ram_address(&self, linear: u32) -> Option<GuestPhysicalAddress> {
        let address = GuestPhysicalAddress::new(u64::from(linear));
        (self.memory.locate_region_kind(address, 1) == Ok(MemoryRegionKind::Ram)).then_some(address)
    }

    /// The 8 bytes of an 8-aligned page-table entry, all in one RAM region (S6-31b).
    fn entry_address(&self, linear: u32) -> Option<GuestPhysicalAddress> {
        let address = GuestPhysicalAddress::new(u64::from(linear));
        (linear.is_multiple_of(8)
            && self.memory.locate_region_kind(address, 8) == Ok(MemoryRegionKind::Ram))
        .then_some(address)
    }
}

impl RealModeMemory for BoardRam<'_> {
    fn read_u8(&self, linear: u32) -> Option<u8> {
        self.memory.load_u8(self.ram_address(linear)?).ok()
    }

    fn can_write(&self, linear: u32) -> bool {
        self.ram_address(linear).is_some()
    }

    fn write_u8(&mut self, linear: u32, value: u8) -> bool {
        match self.ram_address(linear) {
            Some(address) => self.memory.store_bytes(address, &[value], |_| {}).is_ok(),
            None => false,
        }
    }

    /// S6-31b: one aligned 8-byte load. An entry that is not 8-aligned or leaves the RAM region
    /// has no 8-byte cell; it is read byte by byte (the default of the trait), never a host
    /// load of an unaligned cell.
    fn read_u64(&self, linear: u32) -> Option<u64> {
        match self.entry_address(linear) {
            Some(address) => self.memory.load_u64(address).ok(),
            None => {
                let mut bytes = [0u8; 8];
                for (i, slot) in bytes.iter_mut().enumerate() {
                    *slot = u32::checked_add(linear, i as u32).and_then(|a| self.read_u8(a))?;
                }
                Some(u64::from_le_bytes(bytes))
            }
        }
    }

    /// S6-31b: one host compare-exchange of the entry (`PhysicalMemory::compare_replace_u64`,
    /// S6-31), so another vCPU's concurrent change of the entry is never overwritten. Replaces
    /// the read-then-write of `or_u8` (S6-25). `None` for an entry that is not an 8-aligned cell
    /// of one RAM region or whose write is refused (nothing is written).
    fn compare_replace_entry(
        &mut self,
        linear: u32,
        expected: u64,
        replacement: u64,
    ) -> Option<Result<(), u64>> {
        let address = self.entry_address(linear)?;
        self.memory
            .compare_replace_u64(address, expected, replacement, |_| {})
            .ok()
    }
}

/// Run the legacy model of vCPU `cpu` for at most `budget` steps on the RAM of `board`.
///
/// Before every step, in this order: a real-mode processor whose far jump loaded a protected code
/// segment becomes the protected-mode model; a protected-mode processor whose 64-bit jump retired
/// becomes the long-mode state (neither costs a step, so a call that is handed such a processor
/// retires nothing and hands over at once); `stop()` is polled (true: the call ends); the budget is
/// checked (used up: the call ends). Then one step of the model. A refusal ends the call with
/// [`FirmwareBoardError::ApLegacyRefused`]; the model and the memory are unchanged by the refused
/// step, and what earlier steps of the call retired stays in the vCPU. Those steps are in neither
/// the counters of the scheduler nor the clock: the error replaces the `Executed` of the call, so
/// the scheduler charges nothing for it (a refusal ends the run; the clock and the counters of an
/// errored run are short by the steps of the last call).
///
/// At the hand-over `IA32_EFER` of the model goes to `guest_efer`; the state is
/// [`ProtectedCpu::into_long_mode`] of the model with the vCPU's own `state` as the preserved one,
/// so its CR3 is the trampoline's, not the board's.
pub(crate) fn run_legacy(
    board: &mut LoadedFirmwareBoard,
    cpu: usize,
    budget: u64,
    stop: &mut dyn FnMut() -> bool,
) -> Result<LegacyRun, FirmwareBoardError> {
    let vcpu = board
        .vcpu_mut(cpu)
        .ok_or(FirmwareBoardError::Config("ap legacy: no such vCPU"))?;
    // The vCPU and the RAM are two disjoint parts of the board: take the vCPU out for the call.
    let mut taken = std::mem::replace(vcpu, crate::vcpu::Vcpu::new_ap(cpu));
    let result = {
        let (memory, _) = board.bus_mut().split_for_pio_step();
        run_legacy_on(&mut taken, memory, cpu, budget, stop)
    };
    *board.vcpu_mut(cpu).expect("the vCPU was found above") = taken;
    result
}

/// [`run_legacy`] on what it needs: the vCPU and the RAM (the vCPU threads of the parallel mode,
/// S6-34, call it with the vCPU they own and the shared reference to the RAM).
pub(crate) fn run_legacy_on(
    vcpu: &mut crate::vcpu::Vcpu,
    memory: &PhysicalMemory,
    cpu: usize,
    budget: u64,
    stop: &mut dyn FnMut() -> bool,
) -> Result<LegacyRun, FirmwareBoardError> {
    const NO_LEGACY: FirmwareBoardError =
        FirmwareBoardError::Config("ap legacy: the vCPU is not in a legacy mode");
    let mut model = vcpu.legacy.take().ok_or(NO_LEGACY)?;
    let preserved = vcpu.state.clone();
    let mut retired = 0u64;
    let (mut handed_over, mut refusal) = (None, None);
    {
        let mut ram = BoardRam::new(memory);
        loop {
            // The transitions cost no step.
            if let LegacyCpu::Real(real) = &model
                && real.cs_protected.is_some()
                && real.mode.cr0 & CR0_PE != 0
            {
                match ProtectedCpu::from_real_entry(real) {
                    Ok(protected) => model = LegacyCpu::Protected(protected),
                    Err(error) => {
                        refusal = Some(ApLegacyRefusal::ProtectedEntry(error));
                        break;
                    }
                }
            }
            if let LegacyCpu::Protected(protected) = &model
                && protected.target_entered
            {
                match protected.into_long_mode(&preserved) {
                    Ok(state) => handed_over = Some((state, protected.efer)),
                    Err(error) => refusal = Some(ApLegacyRefusal::LongEntry(error)),
                }
                break;
            }
            if stop() || retired >= budget {
                break;
            }
            let stepped = match &mut model {
                LegacyCpu::Real(real) => real
                    .step(&mut ram)
                    .map(|_| ())
                    .map_err(ApLegacyRefusal::Real),
                LegacyCpu::Protected(protected) => protected
                    .step(&mut ram)
                    .map(|_| ())
                    .map_err(ApLegacyRefusal::Protected),
            };
            match stepped {
                Ok(()) => retired += 1,
                Err(error) => {
                    refusal = Some(error);
                    break;
                }
            }
        }
    }
    let executed = Executed {
        retired,
        ..Executed::default()
    };
    if let Some(refusal) = refusal {
        vcpu.legacy = Some(model);
        return Err(FirmwareBoardError::ApLegacyRefused { cpu, refusal });
    }
    if let Some((state, efer)) = handed_over {
        vcpu.state = state;
        vcpu.guest_efer = efer;
        vcpu.legacy = None;
        return Ok(LegacyRun {
            executed,
            entered_long_mode: true,
        });
    }
    vcpu.legacy = Some(model);
    Ok(LegacyRun {
        executed,
        entered_long_mode: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware_board::vcpu_quantum::fixture;
    use crate::smp_scheduler::{
        BoardSmpHost, Finished, QuantumPolicy, SmpError, SmpHost, SmpScheduler, StopRequest,
    };
    use crate::vcpu::{
        AP_INIT_EDX, LINUX_AP_CPUID_PROFILE, POWER_ON_CR0, VcpuCpuid, init_cpu, init_cpu_for,
    };
    use crate::{
        FirmwareBoardConfig, LOCAL_APIC_MMIO_BASE, PciEcam, firmware_board::assemble_board,
    };
    use od_isa::legacy_mode::RealModeState;
    use od_isa::protected_exec::ProtectedRefusal;
    use od_isa::real_mode_exec::{ImageMemoryMut, RealModeCpu};
    use od_isa::{CpuActivity, LongModeState, Register, cpuid::CpuidProfile};

    // ---- the test trampoline -------------------------------------------------------------------
    //
    // Only forms of S6-14a. SIPI vector 0x08: the AP starts at 0800:0000, physical 0x8000.
    //
    //   real mode, 0x8000 (14 steps)           protected mode, 0x8200 (14 steps)
    //   cli                                    mov ss,edx            (EDX = 0x18)
    //   mov ax,cs; mov ds,ax; mov es,ax;       mov ds,edx; mov es,edx
    //     mov ss,ax                            mov eax,0x6b0; mov cr4,eax
    //   lock bts ds:LOCK,0; jae; (pause; jmp)  mov eax,PML4; mov cr3,eax
    //   mov esp,0x7000                         mov ecx,EFER_MSR; mov eax,ds:EFER_LO;
    //   lidt ds:IDTR; lgdt ds:GDTR               mov edx,ds:EFER_HI; wrmsr
    //   mov dx,0x18                            mov eax,0x80050033; mov cr0,eax   (PG: IA-32e)
    //   mov eax,0x50033; mov cr0,eax           jmp 0x10:0x8300   (compatibility -> 64-bit CS)
    //   jmp 0x08:0x8200
    //   64-bit, 0x8300 (1 step): jmp qword [rip+..]  to the cell JUMP_CELL = LONG_CODE
    //   long mode, 0x8500: mov eax,MARK; mov ebx,MARKER; mov [rbx],eax; hlt
    const SIPI_VECTOR: u8 = 0x08;
    const REAL_AT: u64 = 0x8000;
    const LOCK: u64 = 0x8100;
    const IDTR_AT: u64 = 0x8140;
    const GDTR_AT: u64 = 0x8130;
    const PROTECTED_AT: u64 = 0x8200;
    const COMPAT_AT: u64 = 0x8300;
    const CELLS_AT: u64 = 0x8400;
    const JUMP_CELL: u64 = 0x8408;
    const LONG_AT: u64 = 0x8500;
    const GDT_AT: u64 = 0x9100;
    const MARKER: u64 = 0xA000;
    const MARK: u32 = 0x4D41_524B;
    const BSP_PML4: u64 = 0x1000;
    const PML4: u64 = 0x4000;
    const PDPT: u64 = 0x5000;
    const PD: u64 = 0x6000;
    /// Steps of the trampoline up to and including the 64-bit jump: 14 in real mode, 14 in
    /// protected mode (the far jumps included), 1 in the 64-bit code segment.
    const LEGACY_STEPS: u64 = 29;
    /// Steps of the long-mode program: two `MOV r32, imm32`, the store and the `HLT`.
    const LONG_STEPS: u64 = 4;
    const RAM_IMAGE: usize = 0xB000;
    const FLAT_CODE: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x9B, 0xCF, 0];
    const CODE64: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x9B, 0xAF, 0];
    /// Data with the accessed bit clear: the load into `SS` sets it (a write through the tracked path).
    const DATA_A_CLEAR: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x92, 0xCF, 0];

    fn le32(value: u32) -> [u8; 4] {
        value.to_le_bytes()
    }

    fn real_block() -> Vec<u8> {
        let mut code = vec![0xFA, 0x8C, 0xC8, 0x8E, 0xD8, 0x8E, 0xC0, 0x8E, 0xD0];
        // lock bts DWORD PTR ds:LOCK, 0; jae +4; pause; jmp back to the lock bts.
        code.extend([
            0x66,
            0xF0,
            0x0F,
            0xBA,
            0x2E,
            (LOCK - REAL_AT) as u8,
            0x01,
            0x00,
        ]);
        code.extend([0x73, 0x04, 0xF3, 0x90, 0xEB, 0xF2]);
        code.extend([0x66, 0xBC, 0x00, 0x70, 0x00, 0x00]); // mov esp, 0x7000
        code.extend([0x66, 0x0F, 0x01, 0x1E, (IDTR_AT - REAL_AT) as u8, 0x01]); // lidt
        code.extend([0x66, 0x0F, 0x01, 0x16, (GDTR_AT - REAL_AT) as u8, 0x01]); // lgdt
        code.extend([0xBA, 0x18, 0x00]); // mov dx, 0x18
        code.extend([0x66, 0xB8, 0x33, 0x00, 0x05, 0x00]); // mov eax, 0x50033
        code.extend([0x0F, 0x22, 0xC0]); // mov cr0, eax
        code.extend([0x66, 0xEA]);
        code.extend(le32(PROTECTED_AT as u32));
        code.extend([0x08, 0x00]); // jmp 0x08:PROTECTED_AT
        code
    }

    fn protected_block(first_read: u32) -> Vec<u8> {
        let mut code = vec![0x8E, 0xD2, 0x8E, 0xDA, 0x8E, 0xC2]; // mov ss/ds/es, edx
        code.extend([0xB8]);
        code.extend(le32(0x6B0));
        code.extend([0x0F, 0x22, 0xE0]); // mov cr4, eax
        code.extend([0xB8]);
        code.extend(le32(PML4 as u32));
        code.extend([0x0F, 0x22, 0xD8]); // mov cr3, eax
        code.extend([0xB9, 0x80, 0x00, 0x00, 0xC0]); // mov ecx, EFER
        code.extend([0xA1]);
        code.extend(le32(first_read)); // mov eax, ds:EFER_LO
        code.extend([0x8B, 0x15]);
        code.extend(le32((CELLS_AT + 4) as u32)); // mov edx, ds:EFER_HI
        code.extend([0x0F, 0x30]); // wrmsr
        code.extend([0xB8]);
        code.extend(le32(0x8005_0033));
        code.extend([0x0F, 0x22, 0xC0]); // mov cr0, eax
        code.extend([0xEA]);
        code.extend(le32(COMPAT_AT as u32));
        code.extend([0x10, 0x00]); // jmp 0x10:COMPAT_AT
        code
    }

    fn long_block() -> Vec<u8> {
        let mut code = vec![0xB8];
        code.extend(le32(MARK));
        code.extend([0xBB]);
        code.extend(le32(MARKER as u32));
        code.extend([0x89, 0x83, 0, 0, 0, 0]); // mov [rbx+0], eax
        code.push(0xF4);
        code
    }

    /// Everything the test trampoline needs in RAM, as (address, bytes).
    fn trampoline(first_read: u32) -> Vec<(u64, Vec<u8>)> {
        let mut gdt = Vec::new();
        for descriptor in [[0u8; 8], FLAT_CODE, CODE64, DATA_A_CLEAR] {
            gdt.extend(descriptor);
        }
        let mut gdtr = vec![0x1F, 0x00];
        gdtr.extend(le32(GDT_AT as u32));
        let mut idtr = vec![0x00, 0x00];
        idtr.extend(le32(0));
        let jmp_rip = {
            let mut code = vec![0xFF, 0x25];
            code.extend((((JUMP_CELL as i64) - (COMPAT_AT as i64 + 6)) as i32).to_le_bytes());
            code
        };
        let mut cells = Vec::new();
        cells.extend(le32(0x901)); // EFER low: SCE | LME | NXE
        cells.extend(le32(0)); // EFER high
        cells.extend(LONG_AT.to_le_bytes()); // JUMP_CELL
        let qword = |value: u64| value.to_le_bytes().to_vec();
        vec![
            (REAL_AT, real_block()),
            (GDTR_AT, gdtr),
            (IDTR_AT, idtr),
            (PROTECTED_AT, protected_block(first_read)),
            (COMPAT_AT, jmp_rip),
            (CELLS_AT, cells),
            (LONG_AT, long_block()),
            (GDT_AT, gdt),
            // PML4[0] -> PDPT, PDPT[0] -> PD, PD[0] = a 2 MiB page at 0 (present, writable, PS).
            (PML4, qword(PDPT | 3)),
            (PDPT, qword(PD | 3)),
            (PD, qword(0x83)),
        ]
    }

    fn board_with(vcpus: usize, patches: &[(u64, Vec<u8>)]) -> LoadedFirmwareBoard {
        let config = FirmwareBoardConfig {
            vcpu_count: vcpus,
            ..FirmwareBoardConfig::default()
        };
        let mut board = assemble_board(&config, PciEcam::new(), None, None)
            .unwrap()
            .into_linux_board(BSP_PML4, 0x1000)
            .unwrap();
        board.enable_clock_owner().unwrap();
        for (at, bytes) in patches {
            board
                .bus_mut()
                .patch_physical_ram(GuestPhysicalAddress::new(*at), bytes)
                .unwrap();
        }
        board
    }

    fn board() -> LoadedFirmwareBoard {
        board_with(2, &trampoline((CELLS_AT) as u32))
    }

    fn ram(board: &mut LoadedFirmwareBoard, at: u64, len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        let (memory, _) = board.bus_mut().split_for_pio_step();
        memory
            .load_bytes(GuestPhysicalAddress::new(at), &mut bytes)
            .unwrap();
        bytes
    }

    fn generation(board: &mut LoadedFirmwareBoard, at: u64) -> u64 {
        board
            .bus_mut()
            .code_page_generation(GuestPhysicalAddress::new(at))
            .unwrap()
    }

    const INIT_ASSERT: u32 = (5 << 8) | (3 << 14);
    const NMI: u32 = 4 << 8;
    fn sipi(vector: u8) -> u32 {
        (6 << 8) | u32::from(vector)
    }

    /// The long-mode quantum of a vCPU of this board: the board's own (S6-16d,
    /// `run_vcpu_set_quantum`): the vCPU's own CR3 (the trampoline's) and its own `guest_efer`
    /// (0xD01, not `FIRMWARE_EFER` 0xD00), the Local APIC of the vCPU, its own boundary hook.
    fn long_quantum(
        board: &mut LoadedFirmwareBoard,
        cpu: usize,
        budget: u64,
        stop: &mut dyn FnMut() -> bool,
    ) -> Result<Executed, FirmwareBoardError> {
        fixture::with_set(|set| board.run_vcpu_set_quantum(cpu, budget, stop, set))
    }

    /// What the boot processor does: its first call sends INIT and SIPI to the AP (when `send`),
    /// the second halts it. One instruction each.
    fn bsp_or_long(
        board: &mut LoadedFirmwareBoard,
        cpu: usize,
        budget: u64,
        stop: &mut dyn FnMut() -> bool,
        call: &mut u32,
        send: bool,
    ) -> Result<Executed, FirmwareBoardError> {
        if cpu != 0 {
            return long_quantum(board, cpu, budget, stop);
        }
        *call += 1;
        if *call == 1 && send {
            let apic = board.local_apic_of(0).unwrap();
            apic.write_icr(0, 1, INIT_ASSERT).unwrap();
            apic.write_icr(0, 1, sipi(SIPI_VECTOR)).unwrap();
        } else {
            board.vcpu_mut(0).unwrap().state.activity = Some(CpuActivity::Halted);
        }
        Ok(Executed {
            retired: 1,
            ..Executed::default()
        })
    }

    /// The final state of one whole run, to compare runs with each other.
    #[derive(Debug, PartialEq)]
    struct Outcome {
        finished: Finished,
        ap: LongModeState,
        ap_efer: u64,
        ap_retired: u64,
        bsp_retired: u64,
        ram: Vec<u8>,
        now_ns: u64,
    }

    fn run_whole(policy: QuantumPolicy, budget: u64) -> Outcome {
        let mut board = board();
        let mut call = 0;
        let (finished, counters) = {
            let mut host = BoardSmpHost::new(
                &mut board,
                |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                    bsp_or_long(board, cpu, budget, stop, &mut call, true)
                },
            )
            .unwrap();
            let mut scheduler = SmpScheduler::new(2, policy, StopRequest::new()).unwrap();
            let finished = scheduler.run(&mut host, budget).unwrap();
            (finished, scheduler.counters().to_vec())
        };
        let now_ns = board.clock().now_ns();
        let ap = board.vcpu(1).unwrap();
        assert!(ap.legacy.is_none());
        let (state, efer) = (ap.state.clone(), ap.guest_efer);
        Outcome {
            finished,
            ap: state,
            ap_efer: efer,
            ap_retired: counters[1].retired,
            bsp_retired: counters[0].retired,
            ram: ram(&mut board, 0, RAM_IMAGE),
            now_ns,
        }
    }

    // ---- the whole path ------------------------------------------------------------------------

    #[test]
    fn an_ap_started_by_sipi_runs_the_trampoline_to_long_mode_and_the_program_stores_its_marker() {
        let outcome = run_whole(QuantumPolicy::default(), 1_000);
        assert_eq!(outcome.finished, Finished::AllIdle);
        // The long-mode program ran on the state of the hand-over and halted.
        assert_eq!(&outcome.ram[MARKER as usize..][..4], &le32(MARK));
        assert_eq!(outcome.ap.activity, Some(CpuActivity::Halted));
        assert_eq!(outcome.ap.cs_selector, Some(0x10));
        assert_eq!(outcome.ap.cr3, Some(PML4));
        assert_eq!(outcome.ap.cr0, Some(0x8005_0033));
        assert_eq!(outcome.ap_efer, 0xD01);
        assert_eq!(
            outcome.ap.rip,
            LONG_AT + 0x10 + 1,
            "the HLT retired: RIP after it"
        );
        // The count is the length of the path worked out by hand (see the table above); the
        // boot processor retired two (send, halt).
        assert_eq!(outcome.ap_retired, LEGACY_STEPS + LONG_STEPS);
        assert_eq!(outcome.bsp_retired, 2);
        // One nanosecond per retired instruction, nothing else moved the clock.
        assert_eq!(outcome.now_ns, 2 + LEGACY_STEPS + LONG_STEPS);
    }

    /// The trampoline's bytes run by the models alone on an image (no board): the RAM image and the
    /// protected-mode model at the 64-bit jump, after 14 + 15 steps (the last of them is the
    /// refusal `Mode64TargetEntered`, which retires nothing).
    fn isolated_run() -> (ProtectedCpu, Vec<u8>) {
        let mut image = vec![0u8; RAM_IMAGE];
        for (at, bytes) in trampoline(CELLS_AT as u32) {
            image[at as usize..at as usize + bytes.len()].copy_from_slice(&bytes);
        }
        let mut real = RealModeCpu {
            mode: init_cpu(POWER_ON_CR0).mode.after_sipi(SIPI_VECTOR),
            ..init_cpu(POWER_ON_CR0)
        };
        let mut memory = ImageMemoryMut {
            base: 0,
            bytes: &mut image,
        };
        let run = real.run(&mut memory, 100);
        assert_eq!((run.steps, run.stop.is_some()), (14, true), "{run:?}");
        let mut protected = ProtectedCpu::from_real_entry(&real).unwrap();
        let run = protected.run(&mut memory, 100);
        assert_eq!(run.steps, LEGACY_STEPS - 14, "{run:?}");
        assert!(matches!(
            run.stop,
            Some(ProtectedRefusal::Mode64TargetEntered { rip: LONG_AT })
        ));
        (protected, image)
    }

    #[test]
    fn the_state_at_the_hand_over_is_that_of_the_same_bytes_run_in_isolation() {
        // The budget is the boot processor's two instructions and the AP's legacy path exactly:
        // the AP stops on the hand-over, before the first long-mode instruction.
        let mut board = board();
        // What INIT keeps (`kernel_gs_base`, the FP and SSE state, `syscall`) is taken from the
        // state the vCPU has BEFORE the run, not read back from the vCPU after the hand-over.
        board.vcpu_mut(1).unwrap().state.kernel_gs_base = Some(0x5A5A_0000_1234);
        let preserved = board.vcpu(1).unwrap().state.clone();
        let mut call = 0;
        {
            let mut host = BoardSmpHost::new(
                &mut board,
                |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                    bsp_or_long(board, cpu, budget, stop, &mut call, true)
                },
            )
            .unwrap();
            let mut scheduler =
                SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
            assert_eq!(
                scheduler.run(&mut host, 2 + LEGACY_STEPS).unwrap(),
                Finished::BudgetExhausted
            );
            assert_eq!(scheduler.counters()[1].retired, LEGACY_STEPS);
        }
        let (protected, image) = isolated_run();
        let expected = protected.into_long_mode(&preserved).unwrap();
        let ap = board.vcpu(1).unwrap();
        assert!(ap.legacy.is_none());
        assert_eq!(ap.state, expected);
        // The kept register survived the whole path; the registers the trampoline wrote did not
        // come from `preserved`.
        assert_eq!(ap.state.kernel_gs_base, preserved.kernel_gs_base);
        assert_eq!(ap.state.x87, preserved.x87);
        assert_eq!(ap.state.sse, preserved.sse);
        assert_ne!(ap.state.cr0, preserved.cr0);
        assert_eq!(ap.guest_efer, protected.efer);
        assert_eq!(ap.state.rip, LONG_AT);
        assert_eq!(ap.state.activity, Some(CpuActivity::Running));
        // The RAM is the isolated one too: the accessed bit in the GDT, the lock cell, the
        // accessed and dirty bits of the paging entries the walks of the model used.
        assert_eq!(ram(&mut board, 0, RAM_IMAGE), image);
    }

    #[test]
    fn the_models_writes_advance_the_page_generations_like_a_guest_store() {
        let mut board = board();
        let before = [
            generation(&mut board, LOCK),
            generation(&mut board, GDT_AT),
            generation(&mut board, PD),
            generation(&mut board, MARKER),
        ];
        let mut call = 0;
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                bsp_or_long(board, cpu, budget, stop, &mut call, true)
            },
        )
        .unwrap();
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
        scheduler.run(&mut host, 2 + LEGACY_STEPS).unwrap();
        let after = [
            generation(&mut board, LOCK),
            generation(&mut board, GDT_AT),
            generation(&mut board, PD),
            generation(&mut board, MARKER),
        ];
        // LOCK BTS (the lock cell), the accessed bit of the data descriptor (the GDT page), the
        // accessed bit of the paging entries (the PD page): written; the marker page: not yet.
        assert!(after[0] > before[0], "lock cell page");
        assert!(after[1] > before[1], "GDT page");
        assert!(after[2] > before[2], "page-table page");
        assert_eq!(after[3], before[3], "the marker is stored in long mode");
        assert_eq!(ram(&mut board, LOCK, 4), le32(1));
    }

    #[test]
    fn the_run_is_the_same_under_every_quantum_and_the_same_seed_gives_the_same_run() {
        let whole = run_whole(QuantumPolicy::Fixed(1024), 1_000);
        for quantum in [1, 2, 7, 29, 30] {
            assert_eq!(
                run_whole(QuantumPolicy::Fixed(quantum), 1_000),
                whole,
                "quantum {quantum}"
            );
        }
        let seeded = |seed| QuantumPolicy::Seeded {
            seed,
            min: 1,
            max: 9,
        };
        for seed in [0, 1, 0xDEAD_BEEF, u64::MAX] {
            let first = run_whole(seeded(seed), 1_000);
            assert_eq!(run_whole(seeded(seed), 1_000), first, "seed {seed:#x}");
            // The AP does not depend on how its turns are cut: the same final state as above.
            assert_eq!(first, whole, "seed {seed:#x}");
        }
    }

    // ---- S6-16d: after the hand-over the AP is a vCPU like any other --------------------------------

    /// The boot processor of the test (a long-mode processor of the harness, SMP plan 4.4: it is
    /// not what is proved here) sends INIT and SIPI to the AP with the ICR, waits for the flag the
    /// AP raises in long mode, and sends the first Fixed IPI; its handler sends the next ones.
    fn bsp_program() -> Vec<u8> {
        let mut code = fixture::enable_lapic();
        code.extend(fixture::mov_eax(1 << 24)); // ICR_HIGH: destination APIC ID 1
        code.extend(fixture::imm(&[0x89, 0x83], 0x310));
        code.extend(fixture::mov_eax(INIT_ASSERT));
        code.extend(fixture::store_eax(0x300));
        code.extend(fixture::mov_eax(sipi(SIPI_VECTOR)));
        code.extend(fixture::store_eax(0x300));
        code.extend(fixture::imm(&[0xB9], fixture::FLAG as u32)); // mov ecx, FLAG
        code.extend([0x8B, 0x01, 0x85, 0xC0, 0x74, 0xFA]); // wait: mov eax,[rcx]; test; jz wait
        code.extend(fixture::send_ipi_from_ebx(fixture::TO_AP, 1));
        code.extend(fixture::HLT_LOOP);
        code
    }

    #[test]
    fn two_vcpus_exchange_fixed_ipis_after_the_ap_was_started_by_sipi_from_guest_code() {
        const IDT_AP: u64 = 0xE000;
        const PD_L: u64 = 0xC000;
        const PT_L: u64 = 0xD000;
        let mut board = board();
        board.bus_mut().enable_apic_mmio_windows();
        // The boot processor: tables, GDT, IDT, handlers and program of its own.
        fixture::system(&mut board, true);
        let cr3_0 = fixture::tables(&mut board, fixture::TABLES_0, 0x60_0000, 0);
        fixture::patch(&mut board, fixture::CODE_0, &bsp_program());
        fixture::install(
            &mut board,
            0,
            fixture::long_state(fixture::CODE_0, fixture::STACK_0, cr3_0, true),
            0xD01,
        );
        // The AP: the trampoline of the tests above with three changes. Its IDTR points at an IDT
        // whose gates select the 64-bit code segment of the trampoline's GDT (0x10); its tables get
        // the page of the Local APIC window; the long-mode code is the AP program, which ends in
        // `sti; hlt` (the trampoline leaves IF clear).
        let mut idtr = vec![0xFF, 0x0F];
        idtr.extend(le32(IDT_AP as u32));
        fixture::patch(&mut board, IDTR_AT, &idtr);
        fixture::idt(&mut board, IDT_AP, 0x10);
        fixture::patch(&mut board, LONG_AT, &fixture::ap_program(true));
        fixture::patch(&mut board, PDPT + 3 * 8, &(PD_L | 3).to_le_bytes());
        fixture::patch(&mut board, PD_L + 0x1F7 * 8, &(PT_L | 3).to_le_bytes());
        fixture::patch(
            &mut board,
            PT_L,
            &(LOCAL_APIC_MMIO_BASE | 0x13).to_le_bytes(),
        );

        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                long_quantum(board, cpu, budget, stop)
            },
        )
        .unwrap();
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::Fixed(32), StopRequest::new()).unwrap();
        assert_eq!(
            scheduler.run(&mut host, 100_000).unwrap(),
            Finished::AllIdle
        );
        let counters = scheduler.counters().to_vec();
        // The AP was started by the guest's own ICR writes, once, and ran the trampoline.
        assert_eq!((counters[1].init_applied, counters[1].started), (1, 1));
        assert!(counters[1].retired > LEGACY_STEPS);
        let ap = board.vcpu(1).unwrap();
        assert!(ap.legacy.is_none());
        assert_eq!(ap.guest_efer, 0xD01);
        assert_eq!(
            ap.state.cr3,
            Some(PML4),
            "its own tables, not the board's root"
        );
        assert_eq!(ap.activity(), CpuActivity::Halted);
        assert_eq!(board.vcpu(0).unwrap().activity(), CpuActivity::Halted);
        // ROUNDS Fixed IPIs each way, each handler ended with its EOI.
        assert_eq!(
            fixture::dword(&mut board, fixture::COUNTER_1),
            fixture::ROUNDS
        );
        assert_eq!(
            fixture::dword(&mut board, fixture::COUNTER_0),
            fixture::ROUNDS
        );
        for cpu in 0..2 {
            for vector in [fixture::TO_AP, fixture::TO_BSP] {
                assert_eq!(
                    board.local_apic_of(cpu).unwrap().vector_state(vector),
                    (false, false),
                    "cpu {cpu} vector {vector:#x}"
                );
            }
        }
    }

    // ---- before SIPI, INIT, NMI, refusals -------------------------------------------------------

    #[test]
    fn an_ap_retires_nothing_before_sipi() {
        let mut board = board();
        let mut call = 0;
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                bsp_or_long(board, cpu, budget, stop, &mut call, false)
            },
        )
        .unwrap();
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
        assert_eq!(scheduler.run(&mut host, 1_000).unwrap(), Finished::AllIdle);
        assert_eq!(scheduler.counters()[1].retired, 0);
        assert_eq!(scheduler.counters()[1].turns, 0);
        let ap = host.board().vcpu(1).unwrap();
        assert_eq!(ap.activity(), CpuActivity::WaitForSipi);
        // S6-18a: the AP's CPUID is the Linux AP profile with its APIC ID and the vCPU count; only
        // the `cpuid` source of the model differs from the lone-processor `init_cpu`.
        assert_eq!(
            ap.cpuid,
            VcpuCpuid::new(LINUX_AP_CPUID_PROFILE, 1, 2),
            "the Linux board's AP"
        );
        assert_eq!(
            ap.legacy,
            Some(LegacyCpu::Real(init_cpu_for(POWER_ON_CR0, ap.cpuid)))
        );
        assert_eq!(ram(&mut board, LOCK, 4), le32(0));
    }

    #[test]
    fn a_started_ap_is_a_real_mode_processor_with_if_clear_until_it_runs() {
        let mut board = board();
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, sipi(SIPI_VECTOR))
            .unwrap();
        board.apply_pending_startup(1).unwrap();
        let ap = board.vcpu(1).unwrap();
        assert_eq!(ap.activity(), CpuActivity::Running);
        assert!(!ap.interrupts_enabled());
        let Some(LegacyCpu::Real(cpu)) = ap.legacy else {
            panic!("{:?}", ap.legacy)
        };
        assert_eq!(cpu.mode.fetch_linear_address(), REAL_AT as u32);
        // EDX holds the signature INIT preserves, the other registers are zero.
        assert_eq!(cpu.gpr, [0, 0, AP_INIT_EDX, 0, 0, 0, 0, 0]);
        assert_eq!(
            cpu.mode,
            RealModeState::after_init(POWER_ON_CR0).after_sipi(SIPI_VECTOR)
        );
    }

    #[test]
    fn init_in_the_middle_of_the_trampoline_returns_the_ap_to_wait_for_sipi_with_the_reset_state() {
        // 8 steps: still real mode, ESP loaded, the lock cell taken; 20 steps: protected mode, the
        // paging registers half written.
        for ap_steps in [8, 20] {
            let mut board = board();
            let mut call = 0;
            let mut host = BoardSmpHost::new(
                &mut board,
                |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                    bsp_or_long(board, cpu, budget, stop, &mut call, true)
                },
            )
            .unwrap();
            let mut scheduler =
                SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
            assert_eq!(
                scheduler.run(&mut host, 2 + ap_steps).unwrap(),
                Finished::BudgetExhausted
            );
            assert_eq!(scheduler.counters()[1].retired, ap_steps);
            let mid = host.board().vcpu(1).unwrap().legacy.unwrap();
            assert_eq!(
                matches!(mid, LegacyCpu::Protected(_)),
                ap_steps == 20,
                "{mid:?}"
            );
            // Not the reset state: something was executed.
            assert_ne!(mid, LegacyCpu::Real(init_cpu(POWER_ON_CR0)));
            // INIT from the boot processor, applied on the AP's own boundary.
            host.apic(0).write_icr(0, 1, INIT_ASSERT).unwrap();
            assert_eq!(
                scheduler.run(&mut host, 1_000).unwrap(),
                Finished::AllIdle,
                "ap_steps {ap_steps}"
            );
            let ap = host.board().vcpu(1).unwrap();
            assert_eq!(ap.activity(), CpuActivity::WaitForSipi);
            assert_eq!(ap.cpuid, VcpuCpuid::new(LINUX_AP_CPUID_PROFILE, 1, 2));
            // The INIT state: CD and NW of the old CR0 kept (the trampoline's `mov cr0` of the
            // 20-step case had cleared them: the old CR0 is the one it wrote), the registers zero
            // but EDX.
            let old_cr0 = if ap_steps == 20 {
                0x0005_0033
            } else {
                POWER_ON_CR0
            };
            assert_eq!(
                ap.legacy,
                Some(LegacyCpu::Real(init_cpu_for(old_cr0, ap.cpuid)))
            );
            assert_eq!(
                ap.real_mode().unwrap().cr0,
                if ap_steps == 20 { 0x10 } else { 0x6000_0010 }
            );
            assert_eq!(ap.guest_efer, 0);
            assert_eq!(scheduler.counters()[1].init_applied, 2);
            // Memory is not reset by INIT: the lock the first steps took stays taken.
            assert_eq!(ram(&mut board, LOCK, 4), le32(1));
        }
    }

    #[test]
    fn an_nmi_to_an_ap_in_a_legacy_mode_is_an_error_and_stays_pending() {
        for ap_steps in [8, 20] {
            let mut board = board();
            let mut call = 0;
            let mut host = BoardSmpHost::new(
                &mut board,
                |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                    bsp_or_long(board, cpu, budget, stop, &mut call, true)
                },
            )
            .unwrap();
            let mut scheduler =
                SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
            scheduler.run(&mut host, 2 + ap_steps).unwrap();
            host.apic(0).write_icr(0, 1, NMI).unwrap();
            assert_eq!(
                scheduler.run(&mut host, 1_000).unwrap_err(),
                SmpError::Host(FirmwareBoardError::Config(
                    "deliver_pending_nmi: NMI in real mode not modelled"
                ))
            );
            assert!(host.apic(1).pending_nmi());
        }
    }

    #[test]
    fn an_nmi_to_an_ap_that_entered_long_mode_is_delivered_on_its_own_cr3() {
        // After the hand-over the AP is a long-mode vCPU on its own CR3 (`PML4` of the trampoline,
        // not the board's `page_table_root` 0x1000): the NMI walk is built on the vCPU's CR3
        // (S6-16f), so the vector-2 gate of the AP's own IDT is read and the NMI is delivered.
        const IDT_AP: u64 = 0xE000;
        const HANDLER: u64 = 0xF000;
        let mut board = board();
        let mut idtr = vec![0xFF, 0x0F];
        idtr.extend(le32(IDT_AP as u32));
        fixture::patch(&mut board, IDTR_AT, &idtr);
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(HANDLER as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&0x10u16.to_le_bytes()); // 64-bit code of the trampoline's GDT
        gate[5] = 0x8E; // present, DPL 0, 64-bit interrupt gate
        fixture::patch(&mut board, IDT_AP + 2 * 16, &gate);
        fixture::patch(&mut board, HANDLER, &[0xF4]); // the handler of vector 2: HLT
        let mut call = 0;
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                bsp_or_long(board, cpu, budget, stop, &mut call, true)
            },
        )
        .unwrap();
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
        scheduler.run(&mut host, 2 + LEGACY_STEPS).unwrap();
        assert!(host.board().vcpu(1).unwrap().legacy.is_none());
        assert_eq!(host.board().vcpu(1).unwrap().state.cr3, Some(PML4));
        host.apic(0).write_icr(0, 1, NMI).unwrap();
        scheduler.run(&mut host, 1_000).unwrap();
        // The latch is taken, the handler of vector 2 ran (its HLT retired), NMI stays blocked until
        // an IRETQ that this handler does not execute.
        assert!(!host.apic(1).pending_nmi());
        let ap = host.board().vcpu(1).unwrap();
        assert!(ap.state.nmi_blocked);
        assert_eq!(ap.state.rip, HANDLER + 1);
        assert_eq!(ap.activity(), CpuActivity::Halted);
    }

    #[test]
    fn a_trampoline_access_to_mmio_is_a_typed_error_and_touches_no_device() {
        // `mov eax, ds:LAPIC+0x20` in place of the first read of the protected block.
        let mut board = board_with(2, &trampoline(LOCAL_APIC_MMIO_BASE as u32 + 0x20));
        let mut call = 0;
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                bsp_or_long(board, cpu, budget, stop, &mut call, true)
            },
        )
        .unwrap();
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
        let error = scheduler.run(&mut host, 1_000).unwrap_err();
        let SmpError::Host(FirmwareBoardError::ApLegacyRefused { cpu: 1, refusal }) = error else {
            panic!("{error:?}")
        };
        assert!(
            matches!(
                refusal,
                ApLegacyRefusal::Protected(ProtectedRefusal::DataUnreadable {
                    linear,
                    ..
                }) if linear == LOCAL_APIC_MMIO_BASE as u32 + 0x20
            ),
            "{refusal:?}"
        );
        // The AP keeps the state before the refused instruction: protected mode, at the read.
        let ap = host.board().vcpu(1).unwrap();
        let Some(LegacyCpu::Protected(cpu)) = ap.legacy else {
            panic!("{:?}", ap.legacy)
        };
        assert_eq!(cpu.eip, PROTECTED_AT as u32 + 0x1B);
        assert_eq!(ap.activity(), CpuActivity::Running);
        assert_eq!(ram(&mut board, MARKER, 4), le32(0));
    }

    #[test]
    fn an_unsupported_instruction_is_a_typed_error_with_the_state_kept() {
        let mut board = board_with(2, &[(REAL_AT, vec![0x90])]);
        let mut call = 0;
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                bsp_or_long(board, cpu, budget, stop, &mut call, true)
            },
        )
        .unwrap();
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
        let error = scheduler.run(&mut host, 1_000).unwrap_err();
        let SmpError::Host(FirmwareBoardError::ApLegacyRefused { cpu: 1, refusal }) = error else {
            panic!("{error:?}")
        };
        assert!(matches!(
            refusal,
            ApLegacyRefusal::Real(RealModeRefusal::Unsupported { linear: 0x8000, .. })
        ));
        let cpuid = host.board().vcpu(1).unwrap().cpuid;
        assert_eq!(cpuid, VcpuCpuid::new(LINUX_AP_CPUID_PROFILE, 1, 2));
        assert_eq!(
            host.board().vcpu(1).unwrap().legacy,
            Some(LegacyCpu::Real(RealModeCpu {
                mode: RealModeState::after_init(POWER_ON_CR0).after_sipi(SIPI_VECTOR),
                ..init_cpu_for(POWER_ON_CR0, cpuid)
            }))
        );
    }

    /// S6-18a: an AP that executes `CPUID` leaf 1 in its real-mode model reads its own APIC ID and
    /// the vCPU count in EBX (oracle by hand from the SDM fields: `0x100 | N << 16 | id << 24`),
    /// the feature mask of the Linux AP profile in EDX (`OdX8664V1Neutral`, the contract's
    /// `0x0781A379`) and `0x600` in EAX; the other leaves of the target are read the same.
    #[test]
    fn s6_18a_an_ap_reads_its_apic_id_and_the_vcpu_count_through_cpuid() {
        // 66 b8 01 00 00 00: mov eax,1; 0f a2: cpuid.
        let code = vec![0x66, 0xb8, 0x01, 0, 0, 0, 0x0f, 0xa2];
        for count in [2usize, 4] {
            let mut board = board_with(count, &[(REAL_AT, code.clone())]);
            for ap in 1..count {
                board
                    .local_apic_of(0)
                    .unwrap()
                    .write_icr(0, ap as u8, sipi(SIPI_VECTOR))
                    .unwrap();
                board.apply_pending_startup(ap).unwrap();
                run_legacy(&mut board, ap, 2, &mut || false).unwrap();
                let Some(LegacyCpu::Real(cpu)) = board.vcpu(ap).unwrap().legacy else {
                    panic!("AP {ap} left real mode")
                };
                assert_eq!(cpu.gpr[0], 0x600, "EAX, count {count} AP {ap}");
                assert_eq!(
                    cpu.gpr[3],
                    0x100 | (count as u32) << 16 | (ap as u32) << 24,
                    "EBX, count {count} AP {ap}"
                );
                assert_eq!(cpu.gpr[1], 0, "ECX");
                assert_eq!(cpu.gpr[2], 0x0781_A379, "EDX, count {count} AP {ap}");
            }
        }
    }

    #[test]
    fn s6_21_long_mode_cpuid_uses_one_profile_and_each_vcpus_topology() {
        let profile = CpuidProfile::X8664V1Stage4AApic;
        let mut patches = trampoline(CELLS_AT as u32);
        patches.iter_mut().find(|(at, _)| *at == LONG_AT).unwrap().1 = vec![0x0f, 0xa2];
        for count in [1usize, 2, 4] {
            let mut board = board_with(count, &patches);
            fixture::patch(&mut board, BSP_PML4, &(PDPT | 3).to_le_bytes());
            fixture::install(
                &mut board,
                0,
                fixture::long_state(LONG_AT, 0x7000, BSP_PML4, false),
                0xD01,
            );
            let ap = count - 1;
            if ap > 0 {
                let bsp_lapic = board.local_apic_of(0).unwrap();
                bsp_lapic.write_icr(0, ap as u8, INIT_ASSERT).unwrap();
                bsp_lapic.write_icr(0, ap as u8, sipi(SIPI_VECTOR)).unwrap();
                board.apply_pending_startup(ap).unwrap();
                assert!(
                    run_legacy(&mut board, ap, 100, &mut || false)
                        .unwrap()
                        .entered_long_mode
                );
                assert_eq!(board.vcpu(ap).unwrap().state.rip, LONG_AT);
                assert_eq!(
                    board.vcpu(ap).unwrap().cpuid.profile,
                    LINUX_AP_CPUID_PROFILE
                );
            }

            fixture::with_set(|set| {
                let set = set.with_cpuid(&profile);
                for cpu in std::iter::once(0).chain((ap > 0).then_some(ap)) {
                    for leaf in
                        (0..=1)
                            .chain(0x8000_0000..=0x8000_0008)
                            .chain([0xb, 0x1f, 0x8000_0026])
                    {
                        let state = &mut board.vcpu_mut(cpu).unwrap().state;
                        let flags = state.rflags;
                        state.rip = LONG_AT;
                        state.gpr[Register::Rax as usize] = 0xffff_ffff_0000_0000 | u64::from(leaf);
                        state.gpr[Register::Rbx as usize] = u64::MAX;
                        state.gpr[Register::Rcx as usize] = 0xffff_ffff_0000_0000;
                        state.gpr[Register::Rdx as usize] = u64::MAX;

                        let retired = if cpu == 0 {
                            let report = board
                                .run_linux_boot_cpl0_set(1, &set, false, false, false)
                                .unwrap();
                            assert_eq!(report.run.exit, crate::PagedProbeExit::BudgetExhausted);
                            report.run.retired
                        } else {
                            board
                                .run_vcpu_set_quantum(cpu, 1, &mut || false, &set)
                                .unwrap()
                                .retired
                        };
                        assert_eq!(retired, 1);

                        let mut expected = profile.query(leaf, 0);
                        if leaf == 1 && count > 1 {
                            expected.ebx = 0x100 | (count as u32) << 16 | (cpu as u32) << 24;
                        }
                        let state = &board.vcpu(cpu).unwrap().state;
                        assert_eq!(state.rip, LONG_AT + 2);
                        assert_eq!(state.rflags, flags);
                        assert_eq!(
                            [Register::Rax, Register::Rbx, Register::Rcx, Register::Rdx]
                                .map(|reg| state.gpr[reg as usize]),
                            [expected.eax, expected.ebx, expected.ecx, expected.edx].map(u64::from),
                            "count {count}, vCPU {cpu}, leaf {leaf:#x}",
                        );
                    }
                }
            });
        }
    }

    #[test]
    fn stop_is_read_before_every_step() {
        let mut board = board_with(2, &trampoline(CELLS_AT as u32));
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, sipi(SIPI_VECTOR))
            .unwrap();
        board.apply_pending_startup(1).unwrap();
        let mut polls = 0;
        let run = run_legacy(&mut board, 1, 100, &mut || {
            polls += 1;
            polls > 3
        })
        .unwrap();
        // Three steps ran, the fourth poll said stop: no more.
        assert_eq!(run.executed.retired, 3);
        assert!(!run.entered_long_mode);
        // A call with a budget of nothing retires nothing and does not stop the processor.
        let run = run_legacy(&mut board, 1, 0, &mut || false).unwrap();
        assert_eq!(run.executed.retired, 0);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Running);
        // A vCPU that is not in a legacy mode is not run by this module.
        assert_eq!(
            run_legacy(&mut board, 0, 10, &mut || false).unwrap_err(),
            FirmwareBoardError::Config("ap legacy: the vCPU is not in a legacy mode")
        );
        assert_eq!(
            run_legacy(&mut board, 7, 10, &mut || false).unwrap_err(),
            FirmwareBoardError::Config("ap legacy: no such vCPU")
        );
    }

    #[test]
    fn a_call_handed_a_processor_that_has_already_jumped_hands_over_and_goes_on_in_the_same_call() {
        // The scheduler takes a call without a step for lack of progress, so the call that finds
        // the 64-bit jump already retired (a budget that ended exactly on it) must go on in long
        // mode itself, whatever the cut of the quanta.
        let (protected, _) = isolated_run();
        let mut board = board();
        board.vcpu_mut(1).unwrap().legacy = Some(LegacyCpu::Protected(protected));
        board.vcpu_mut(1).unwrap().state.activity = Some(CpuActivity::Running);
        let mut call = 0;
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                bsp_or_long(board, cpu, budget, stop, &mut call, true)
            },
        )
        .unwrap();
        let executed = host.run(1, 100, &mut || false).unwrap();
        assert_eq!(executed.retired, LONG_STEPS);
        let ap = host.board().vcpu(1).unwrap();
        assert!(ap.legacy.is_none());
        assert_eq!(ap.activity(), CpuActivity::Halted);
        assert_eq!(ram(&mut board, MARKER, 4), le32(MARK));
        // The same for every cut of the budget: with a budget of one only the first long-mode
        // instruction runs in the call that hands over.
        let mut board = self::board();
        board.vcpu_mut(1).unwrap().legacy = Some(LegacyCpu::Protected(protected));
        board.vcpu_mut(1).unwrap().state.activity = Some(CpuActivity::Running);
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                long_quantum(board, cpu, budget, stop)
            },
        )
        .unwrap();
        let executed = host.run(1, 1, &mut || false).unwrap();
        assert_eq!(executed.retired, 1);
        assert!(host.board().vcpu(1).unwrap().legacy.is_none());
        assert_eq!(host.board().vcpu(1).unwrap().state.rip, LONG_AT + 5);
    }

    #[test]
    fn the_board_ram_adapter_refuses_everything_that_is_not_ram() {
        let mut board = board();
        let generation_before = generation(&mut board, MARKER);
        let (memory, _) = board.bus_mut().split_for_pio_step();
        let mut ram = BoardRam::new(memory);
        for linear in [
            LOCAL_APIC_MMIO_BASE as u32,
            0xFEC0_0000,
            0xFFFF_FFF0,
            0xF800_0000,
        ] {
            assert_eq!(ram.read_u8(linear), None, "{linear:#x}");
            assert!(!ram.can_write(linear), "{linear:#x}");
            assert!(!ram.write_u8(linear, 1), "{linear:#x}");
        }
        assert_eq!(ram.read_u8(MARKER as u32), Some(0));
        assert!(ram.can_write(MARKER as u32));
        assert!(ram.write_u8(MARKER as u32, 0x5A));
        assert_eq!(ram.read_u8(MARKER as u32), Some(0x5A));
        assert!(generation(&mut board, MARKER) > generation_before);
    }

    // ---- S6-31b: the accessed/dirty update of the AP walk is one host compare-exchange ----------

    mod entry_cas {
        use super::*;
        use crate::{GuestPhysicalRange, MachineMemoryMap, MemoryRegionSpec, RamLimits};
        use od_isa::paging::{self, AccessKind, PagingParams};
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        const TPML4: u64 = 0x1000;
        const TPDPT: u64 = 0x2000;
        const TPD: u64 = 0x3000;
        const TPT: u64 = 0x4000;
        /// The leaf under test: PT[2], linear 0x2000.
        const LEAF: u64 = TPT + 16;
        const IMAGE: usize = 0x6000;
        const ROM_AT: u64 = 0x2_0000;
        const PARAMS: PagingParams = PagingParams {
            cr3: TPML4 as u32,
            wp: true,
            nxe: true,
        };

        /// RAM below 0x2_0000 and a ROM page above it.
        fn table_memory() -> PhysicalMemory {
            let range = |start: u64, len: u64| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap()
            };
            let regions = vec![
                MemoryRegionSpec::new(range(0, ROM_AT), MemoryRegionKind::Ram),
                MemoryRegionSpec::new(range(ROM_AT, 0x1000), MemoryRegionKind::Rom),
            ];
            PhysicalMemory::new(
                &MachineMemoryMap::new(36, regions).unwrap(),
                RamLimits::new(1 << 20),
            )
            .unwrap()
        }

        /// PML4[0] -> PDPT -> PD; PD[0] -> PT (PT[i] = page i, i < 5); PD[1] = 2 MiB page at
        /// 2 MiB. Every entry gets `ad` (0 or 0x60) on top of P and RW.
        fn tables(ad: u64) -> Vec<u8> {
            let mut image = vec![0u8; IMAGE];
            let mut put = |at: u64, value: u64| {
                image[at as usize..at as usize + 8].copy_from_slice(&(value | ad).to_le_bytes());
            };
            put(TPML4, TPDPT | 3);
            put(TPDPT, TPD | 3);
            put(TPD, TPT | 3);
            put(TPD + 8, 0x20_0000 | 0x80 | 3);
            for page in 0..5u64 {
                put(TPT + 8 * page, page << 12 | 3);
            }
            image
        }

        fn load_image(memory: &PhysicalMemory, image: &[u8]) {
            memory
                .store_bytes(GuestPhysicalAddress::new(0), image, |_| {})
                .unwrap();
        }

        fn read_image(memory: &PhysicalMemory) -> Vec<u8> {
            let mut bytes = vec![0u8; IMAGE];
            memory
                .load_bytes(GuestPhysicalAddress::new(0), &mut bytes)
                .unwrap();
            bytes
        }

        fn at(address: u64) -> GuestPhysicalAddress {
            GuestPhysicalAddress::new(address)
        }

        #[test]
        fn the_adapter_reads_an_entry_in_one_load_and_replaces_it_in_one_compare_exchange() {
            let memory = table_memory();
            load_image(&memory, &tables(0));
            let mut ram = BoardRam::new(&memory);
            assert_eq!(ram.read_u64(TPML4 as u32), Some(TPDPT | 3));
            // Not aligned: read byte by byte (the default of the trait), never a host load.
            let image = tables(0);
            let from = TPML4 as usize + 4;
            assert_eq!(
                ram.read_u64(TPML4 as u32 + 4),
                Some(u64::from_le_bytes(
                    image[from..from + 8].try_into().unwrap()
                ))
            );
            assert_eq!(
                ram.read_u64(ROM_AT as u32 - 4),
                None,
                "crosses into ROM: not RAM"
            );
            let generation = memory.code_page_generation(at(LEAF)).unwrap();
            // A mismatch writes nothing and moves no generation.
            assert_eq!(
                ram.compare_replace_entry(LEAF as u32, 1, 2),
                Some(Err(0x2003))
            );
            assert_eq!(memory.code_page_generation(at(LEAF)), Ok(generation));
            // A match writes the replacement and moves the generation once.
            assert_eq!(
                ram.compare_replace_entry(LEAF as u32, 0x2003, 0x2023),
                Some(Ok(()))
            );
            assert_eq!(memory.load_u64(at(LEAF)), Ok(0x2023));
            assert_eq!(memory.code_page_generation(at(LEAF)), Ok(generation + 1));
            // ROM, a hole, a misaligned entry: None, nothing written, no split-lock count.
            let refusals = memory.split_lock_refusals();
            for linear in [ROM_AT as u32, 0x30_0000, LEAF as u32 + 4, LEAF as u32 + 1] {
                assert_eq!(ram.compare_replace_entry(linear, 0, 1), None, "{linear:#x}");
            }
            assert_eq!(memory.split_lock_refusals(), refusals);
            assert_eq!(memory.load_u64(at(LEAF)), Ok(0x2023));
        }

        /// S6-31b (a): the walk and the commit over the board's RAM leave the entries exactly as
        /// the former byte OR did, for two accessed/dirty states and every leaf kind.
        #[test]
        fn the_walk_and_commit_on_board_ram_equal_the_former_byte_or() {
            let leaves = [
                (0x2000u32, AccessKind::Read),
                (0x2000, AccessKind::Write),
                (0x2000, AccessKind::Fetch),
                (0x20_0123, AccessKind::Read),
                (0x20_0123, AccessKind::Write),
                (0x20_0123, AccessKind::Fetch),
            ];
            for ad in [0u64, 0x60] {
                for (linear, kind) in leaves {
                    let memory = table_memory();
                    load_image(&memory, &tables(ad));
                    let mut ram = BoardRam::new(&memory);
                    let walked = paging::translate(&ram, PARAMS, linear, kind).unwrap();
                    let mut expected = tables(ad);
                    for touch in &walked.touches {
                        expected[touch.at as usize] |= touch.mask;
                    }
                    assert_eq!(paging::commit_touches(&mut ram, &walked.touches), 0);
                    assert_eq!(
                        read_image(&memory),
                        expected,
                        "{linear:#x} {kind:?} ad {ad:#x}"
                    );
                    assert_eq!(walked.touches.is_empty(), ad == 0x60);
                }
            }
        }

        /// S6-31b (b), (c) on the board's RAM: another writer changes the leaf between the walk
        /// and the commit.
        #[test]
        fn a_change_between_the_walk_and_the_commit_is_discarded_or_absorbed_on_board_ram() {
            let soft = 0x155u64 << 52 | 0b110 << 9;
            // (value written by the other vCPU, value after the commit, touches discarded). The
            // walk read 0x2023 (A preset) and wants D.
            for (raced, after, discarded) in [
                (0x3003u64, 0x3003u64, 1usize),
                (0x2001, 0x2001, 1),
                (0, 0, 1),
                (0x2003 | soft, 0x2003 | soft | 0x40, 0),
                (0x2003 | 0x20, 0x2003 | 0x60, 0),
                (0x2003 | 0x60 | soft, 0x2003 | 0x60 | soft, 0),
            ] {
                let memory = table_memory();
                load_image(&memory, &tables(0x20));
                let mut ram = BoardRam::new(&memory);
                // Only the leaf lacks D: the one touch of the walk.
                let walked = paging::translate(&ram, PARAMS, 0x2000, AccessKind::Write).unwrap();
                assert_eq!(walked.touches.len(), 1, "A is preset, D is not");
                assert_eq!(walked.touches[0].at, LEAF as u32);
                memory.store_u64(at(LEAF), raced, |_| {}).unwrap();
                assert_eq!(
                    paging::commit_touches(&mut ram, &walked.touches),
                    discarded,
                    "{raced:#x}"
                );
                assert_eq!(memory.load_u64(at(LEAF)), Ok(after), "{raced:#x}");
            }
        }

        /// S6-31b (d): an AP walk with its commit against another thread that rewrites the low
        /// byte (P, RW, US, A, D, PS) and the ignored bits of the same PTE, including zeroing it.
        /// The other thread is the only writer of the bits other than A and D and checks, on
        /// every load, that nothing else changed (a stale value written back would revert its
        /// last change; a zeroed entry must stay zero). Mutation check, made once: replacing the
        /// host compare-exchange in `BoardRam::compare_replace_entry` by a load followed by a
        /// store makes this test fail.
        #[test]
        fn an_ap_walk_against_a_concurrent_writer_of_the_same_entry_loses_no_change() {
            const ROUNDS: u64 = 400_000;
            const SOFT_BITS: [u64; 3] = [0x4, 0x80, 0x200];
            let mut memory = table_memory();
            memory.set_execution_mode(crate::ExecutionMode::Parallel);
            let mut image = tables(0x60);
            image[LEAF as usize..LEAF as usize + 8].copy_from_slice(&0x2003u64.to_le_bytes());
            load_image(&memory, &image);
            let stop = AtomicBool::new(false);
            let (committed, discarded, corrupted) =
                (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0));
            let last_written = AtomicU64::new(0x2003);
            std::thread::scope(|scope| {
                // The other vCPU: flips bits through compare-exchange, clears A and D so the AP
                // has more to do, now and then zeroes the entry and holds it zero.
                scope.spawn(|| {
                    let mut state = 0x9E37_79B9_7F4A_7C15u64;
                    let mut next = move || {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state
                    };
                    let mut mine = 0x2003u64;
                    let entry = at(LEAF);
                    while !stop.load(Ordering::Relaxed) {
                        let observed = memory.load_u64(entry).unwrap();
                        // A zeroed entry must stay exactly zero: the unmasked check also catches
                        // a zero turned into 0x20 / 0x60 (A/D set on a discarded walk).
                        if observed & !0x60 != mine & !0x60 || (mine == 0 && observed != 0) {
                            corrupted.fetch_add(1, Ordering::Relaxed);
                        }
                        let random = next();
                        let new = if mine == 0 {
                            // Held zero for a while, then restored.
                            if random % 64 != 0 {
                                continue;
                            }
                            0x2003 | SOFT_BITS[(random >> 8) as usize % 3]
                        } else if random % 16 == 0 {
                            0
                        } else {
                            (mine ^ SOFT_BITS[(random >> 8) as usize % 3]) & !0x60 | 0x3
                        };
                        // CAS over what was observed (A/D of the AP may differ from `mine`).
                        if memory
                            .compare_replace_u64(entry, observed, new, |_| {})
                            .unwrap()
                            .is_ok()
                        {
                            mine = new;
                            last_written.store(new, Ordering::Relaxed);
                        }
                    }
                });
                // The AP: walk (write), then commit, ROUNDS times.
                scope.spawn(|| {
                    let mut ram = BoardRam::new(&memory);
                    for _ in 0..ROUNDS {
                        if let Ok(walked) =
                            paging::translate(&ram, PARAMS, 0x2000, AccessKind::Write)
                        {
                            let lost = paging::commit_touches(&mut ram, &walked.touches);
                            discarded.fetch_add(lost as u64, Ordering::Relaxed);
                            committed
                                .fetch_add((walked.touches.len() - lost) as u64, Ordering::Relaxed);
                        }
                    }
                    stop.store(true, Ordering::Relaxed);
                });
            });
            assert_eq!(corrupted.load(Ordering::Relaxed), 0, "a change was lost");
            let last = last_written.load(Ordering::Relaxed);
            let final_value = memory.load_u64(at(LEAF)).unwrap();
            assert_eq!(final_value & !0x60, last & !0x60);
            if last == 0 {
                assert_eq!(final_value, 0, "a zeroed entry stayed zero");
            }
            assert!(committed.load(Ordering::Relaxed) > 0, "the AP set bits");
            assert!(
                discarded.load(Ordering::Relaxed) > 0,
                "the race must be exercised on this host"
            );
        }
    }
}
