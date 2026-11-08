//! Narrow CPU-connected paging adapters for 10-byte LIDT/LGDT pseudo-descriptors.

use crate::{
    GuestLinearAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus, ProbeConfigError,
    RamAccessOperation, TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::{
    PagedLgdtLong64V1, PagedLidtLong64V1,
    memory::{
        AccessContext, AccessOrigin, ArchitecturalFault, GuestMemory, MemoryError, PageFaultRecord,
    },
    state::LongModeState,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedLidtError {
    Configuration(ProbeConfigError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedLidtReport {
    pub cpu: od_isa::PagedProbeResult<std::convert::Infallible>,
    pub page_fault: Option<PageFaultRecord>,
    pub memory_error: Option<TrackedAccessError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedLgdtError {
    Configuration(ProbeConfigError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedLgdtReport {
    pub cpu: od_isa::PagedProbeResult<std::convert::Infallible>,
    pub page_fault: Option<PageFaultRecord>,
    pub memory_error: Option<TrackedAccessError>,
}

/// Execute the exact bounded LIDT profile through the uncached tracked walker.
/// The descriptor access is one RAM-only 10-byte transaction contained in one
/// 4 KiB page; committed page-table A updates are reported to the writer.
pub fn step_paged_lidt(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedLidtReport, PagedLidtError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedLidtError::Configuration)?;
    let instruction_rip = state.rip;
    let mut memory = PagedDescriptorMemory {
        bus,
        paging,
        instruction_rip,
        instruction_name: "LIDT",
        segment_load: false,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
    };
    let cpu = od_isa::step_paged_lidt(
        PagedLidtLong64V1,
        state,
        &mut memory,
        AccessContext {
            origin: AccessOrigin::Explicit,
            ..context
        },
        efer,
        rflags_defined,
    );
    Ok(PagedLidtReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

/// Execute the exact bounded LGDT profile through RAM-only tracked paging.
/// The pseudo-descriptor is read as one ten-byte operand contained in one
/// guest page, and committed page-table A updates reach the write callback.
pub fn step_paged_lgdt(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedLgdtReport, PagedLgdtError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedLgdtError::Configuration)?;
    let instruction_rip = state.rip;
    let mut memory = PagedDescriptorMemory {
        bus,
        paging,
        instruction_rip,
        instruction_name: "LGDT",
        segment_load: false,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
    };
    let cpu = od_isa::step_paged_lgdt(
        PagedLgdtLong64V1,
        state,
        &mut memory,
        AccessContext {
            origin: AccessOrigin::Explicit,
            ..context
        },
        efer,
        rflags_defined,
    );
    Ok(PagedLgdtReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedLtrError {
    Configuration(ProbeConfigError),
}

/// Bounded-profile rejections of `LTR AX` that are outside the guest exception
/// model; the CPU state is unchanged and no D bit or busy descriptor image was
/// written. Accessed bits set by the preceding walk stay committed, and when the
/// descriptor aliases a page table they may land in the descriptor bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagedLtrRejection {
    /// The TSS descriptor shares a 4 KiB page with a page table used to reach it.
    TableAlias,
    /// The read and write-permission walks disagreed about the descriptor mapping.
    TranslationChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedLtrReport {
    pub cpu: od_isa::PagedProbeResult<std::convert::Infallible>,
    pub page_fault: Option<PageFaultRecord>,
    pub memory_error: Option<TrackedAccessError>,
    pub rejection: Option<PagedLtrRejection>,
}

/// Execute exact `LTR AX` (`0F 00 D8`) through RAM-only tracked paging.
///
/// The 16-byte GDT TSS descriptor is read, validated (CPL0, selector, GDT
/// limit, available 64-bit TSS, present, canonical base), then its busy bit is
/// set with one descriptor-image write after the leaf D bit. The call needs
/// exclusive ownership of RAM: the caller must exclude DMA, other vCPUs and host
/// writers; this is not system-wide locked-bus atomicity. CR3 and EFER are
/// explicit as in the other paged descriptor steps. On any fault or rejection
/// the CPU state is unchanged; page-table A bits and their callbacks stay
/// committed.
pub fn step_paged_ltr_ax(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedLtrReport, PagedLtrError> {
    use crate::page_walk::{LtrTransactionError as Tx, commit_paged_ltr_descriptor};
    use od_isa::execute::{PagedLtrPreparation, prepare_paged_ltr_ax};
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedLtrError::Configuration)?;
    let instruction_rip = state.rip;
    let context = AccessContext {
        origin: AccessOrigin::Explicit,
        ..context
    };
    let (preparation, mut page_fault, mut memory_error) = {
        let mut memory = PagedDescriptorMemory {
            bus,
            paging,
            instruction_rip,
            instruction_name: "LTR",
            segment_load: false,
            committed_write: &mut committed_write,
            page_fault: None,
            memory_error: None,
        };
        let preparation = prepare_paged_ltr_ax(state, &mut memory, context, efer, rflags_defined);
        (preparation, memory.page_fault, memory.memory_error)
    };
    let mut rejection = None;
    let cpu = match &preparation {
        PagedLtrPreparation::Exit(result) => result.clone(),
        PagedLtrPreparation::Ready { .. } => {
            let (physical, _) = bus.split_for_pio_step();
            let transaction = commit_paged_ltr_descriptor(
                physical,
                &paging,
                state,
                &preparation,
                context,
                &mut committed_write,
            );
            let exit = match transaction {
                Ok(prepared) => {
                    prepared.commit(state);
                    od_isa::PagedProbeExit::Retired
                }
                Err(Tx::Descriptor(fault)) => od_isa::PagedProbeExit::Fault(fault),
                Err(Tx::Access(TrackedAccessError::PageFault { record, .. })) => {
                    let fault = record.fault().clone();
                    page_fault = Some(record);
                    od_isa::PagedProbeExit::Fault(fault)
                }
                Err(Tx::Access(error)) => {
                    let message = format!("paged LTR descriptor access failed: {error:?}");
                    memory_error = Some(error);
                    od_isa::PagedProbeExit::Backend(MemoryError::Backend(message))
                }
                Err(error) => {
                    rejection = Some(match error {
                        Tx::TableAlias => PagedLtrRejection::TableAlias,
                        Tx::TranslationChanged => PagedLtrRejection::TranslationChanged,
                        Tx::Access(_) | Tx::Descriptor(_) => unreachable!(),
                    });
                    od_isa::PagedProbeExit::Unsupported { opcode: 0x0f }
                }
            };
            let instructions_executed = u64::from(matches!(exit, od_isa::PagedProbeExit::Retired));
            od_isa::PagedProbeResult {
                exit,
                instructions_executed,
                rflags_defined,
            }
        }
    };
    Ok(PagedLtrReport {
        cpu,
        page_fault,
        memory_error,
        rejection,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedSegmentMovError {
    Configuration(ProbeConfigError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedSegmentMovReport {
    pub cpu: od_isa::PagedProbeResult<std::convert::Infallible>,
    pub page_fault: Option<PageFaultRecord>,
    pub memory_error: Option<TrackedAccessError>,
}

macro_rules! paged_segment_mov_step {
    ($(#[$doc:meta])* $name:ident, $name_str:literal, $cpu:path) => {
        $(#[$doc])*
        pub fn $name(
            state: &mut LongModeState,
            bus: &mut MachineBus,
            cr3: u64,
            efer: u64,
            context: AccessContext,
            rflags_defined: u64,
            mut committed_write: impl FnMut(GuestPhysicalRange),
        ) -> Result<PagedSegmentMovReport, PagedSegmentMovError> {
            let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
                .map_err(PagedSegmentMovError::Configuration)?;
            let instruction_rip = state.rip;
            let mut memory = PagedDescriptorMemory {
                bus,
                paging,
                instruction_rip,
                instruction_name: $name_str,
                segment_load: true,
                committed_write: &mut committed_write,
                page_fault: None,
                memory_error: None,
            };
            let mut cpu = $cpu(
                state,
                &mut memory,
                AccessContext {
                    origin: AccessOrigin::Explicit,
                    ..context
                },
                efer,
                rflags_defined,
            );
            // A descriptor whose eight bytes cross a 4 KiB page is outside the
            // bounded profile rather than a guest exception or backend failure.
            if matches!(
                memory.memory_error,
                Some(TrackedAccessError::CrossesPage { .. })
            ) {
                cpu.exit = od_isa::PagedProbeExit::Unsupported { opcode: 0x8e };
            }
            Ok(PagedSegmentMovReport {
                cpu,
                page_fault: memory.page_fault,
                memory_error: memory.memory_error,
            })
        }
    };
}

paged_segment_mov_step!(
    /// Execute exact `MOV DS,AX` (`8E D8`) through RAM-only tracked paging.
    ///
    /// The 8-byte GDT descriptor is read through a supervisor
    /// (`DescriptorTable`) walk, validated by the CPU loader, its accessed bit is
    /// set with one byte write when clear, and only then the DS cache, selector
    /// and RIP are committed. A fault at any point leaves the CPU unchanged;
    /// page-table A bits (and D for the accessed-bit write) already committed by
    /// walks stay committed. CR3 and EFER are explicit, like the other paged
    /// descriptor steps. Null and LDT selectors, missing GDTR/cache, and
    /// descriptors crossing a 4 KiB page are `Unsupported`.
    step_paged_mov_ds, "MOV DS", od_isa::execute::step_paged_mov_ds
);
paged_segment_mov_step!(
    /// Execute exact `MOV ES,AX` (`8E C0`); see [`step_paged_mov_ds`].
    step_paged_mov_es, "MOV ES", od_isa::execute::step_paged_mov_es
);
paged_segment_mov_step!(
    /// Execute exact `MOV FS,AX` (`8E E0`); see [`step_paged_mov_ds`]. Also
    /// commits the 32-bit descriptor base into `fs_base`.
    step_paged_mov_fs, "MOV FS", od_isa::execute::step_paged_mov_fs
);
paged_segment_mov_step!(
    /// Execute exact `MOV GS,AX` (`8E E8`); see [`step_paged_mov_ds`]. Also
    /// commits the 32-bit descriptor base into `gs_base`.
    step_paged_mov_gs, "MOV GS", od_isa::execute::step_paged_mov_gs
);

/// Shared RAM-only tracked-memory contract for the bounded LGDT/LIDT adapters.
struct PagedDescriptorMemory<'a> {
    bus: &'a mut MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    instruction_name: &'static str,
    /// Set only by the bounded `MOV Sreg,AX` steps: admits the 8-byte descriptor
    /// read and the one-byte accessed-bit write (both RAM-only).
    segment_load: bool,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    page_fault: Option<PageFaultRecord>,
    memory_error: Option<TrackedAccessError>,
}

impl PagedDescriptorMemory<'_> {
    fn access(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        let report = self.bus.access_tracked_ram(
            &self.paging,
            TrackedAccessRequest {
                instruction_rip: self.instruction_rip,
                address: GuestLinearAddress::new(address),
                context,
                operation,
            },
            &mut *self.committed_write,
        );
        report.outcome.map_err(|error| match error {
            TrackedAccessError::PageFault { record, .. } => {
                self.page_fault = Some(record.clone());
                MemoryError::Architectural(record.fault().clone())
            }
            error => {
                self.memory_error = Some(error.clone());
                MemoryError::Backend(format!(
                    "paged {} RAM access failed: {error:?}",
                    self.instruction_name
                ))
            }
        })
    }
}

impl GuestMemory for PagedDescriptorMemory<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        match self.access(address, context, RamAccessOperation::Fetch)? {
            TrackedAccessOutcome::Fetched(byte) => Ok(byte),
            _ => Err(MemoryError::Backend(format!(
                "paged {} fetch returned a non-fetch result",
                self.instruction_name
            ))),
        }
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        if self.segment_load
            && destination.len() == 8
            && context.origin == AccessOrigin::DescriptorTable
        {
            return match self.access(
                address,
                context,
                RamAccessOperation::Read {
                    width: crate::AccessWidth::Qword,
                },
            )? {
                TrackedAccessOutcome::Read(value) => {
                    destination.copy_from_slice(&value.to_le_bytes());
                    Ok(())
                }
                _ => Err(MemoryError::Backend(format!(
                    "paged {} descriptor read returned a non-read result",
                    self.instruction_name
                ))),
            };
        }
        if destination.len() != 10 {
            return Err(MemoryError::Backend(format!(
                "paged {} only reads a 10-byte pseudo-descriptor or an 8-byte descriptor",
                self.instruction_name
            )));
        }
        let last = address.checked_add(9);
        if !canonical48(address) || last.is_none_or(|end| !canonical48(end)) {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            }));
        }
        if address & 0xfff > 0xff6 {
            let error = TrackedAccessError::CrossesPage {
                address: GuestLinearAddress::new(address),
                width: 10,
            };
            self.memory_error = Some(error.clone());
            return Err(MemoryError::Backend(format!(
                "paged {} RAM access failed: {error:?}",
                self.instruction_name
            )));
        }
        match self.access(address, context, RamAccessOperation::ReadPseudoDescriptor)? {
            TrackedAccessOutcome::ReadPseudoDescriptor(bytes) => {
                destination.copy_from_slice(&bytes);
                Ok(())
            }
            _ => Err(MemoryError::Backend(format!(
                "paged {} read returned a non-descriptor result",
                self.instruction_name
            ))),
        }
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        // The only admitted write is the one-byte descriptor accessed-bit update.
        if self.segment_load && source.len() == 1 && context.origin == AccessOrigin::DescriptorTable
        {
            return match self.access(
                address,
                context,
                RamAccessOperation::Write {
                    width: crate::AccessWidth::Byte,
                    value: u64::from(source[0]),
                },
            )? {
                TrackedAccessOutcome::Written => Ok(()),
                _ => Err(MemoryError::Backend(format!(
                    "paged {} descriptor write returned a non-write result",
                    self.instruction_name
                ))),
            };
        }
        Err(MemoryError::Backend(format!(
            "paged {} does not write guest memory",
            self.instruction_name
        )))
    }
    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!(
            "paged {} does not perform atomic memory operations",
            self.instruction_name
        )))
    }
    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!(
            "paged {} does not perform atomic memory operations",
            self.instruction_name
        )))
    }
    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!(
            "paged {} does not perform atomic memory operations",
            self.instruction_name
        )))
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!(
            "paged {} does not perform fences",
            self.instruction_name
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
    };
    use od_isa::memory::{AccessOrigin, InstructionMode, PrivilegeLevel};
    use od_isa::{
        INTEGER_FLAGS,
        state::{CpuActivity, Gdtr, Idtr, StiShadow},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const CR0: u64 = 0x8001_0033;
    const CR4: u64 = 1 << 5;
    const EFER: u64 = 0xd00;
    const ROOT: u64 = 0x1000;
    const CODE: u64 = 0x0040_0000;
    const RAM_LEN: u64 = 0x20_000;

    fn context() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    fn put(bus: &mut MachineBus, address: u64, value: u64) {
        bus.write_physical(
            GuestPhysicalAddress::new(address),
            AccessWidth::Qword,
            value,
            |_| {},
        )
        .unwrap();
    }

    fn put_bytes(bus: &mut MachineBus, address: u64, bytes: &[u8]) {
        for (offset, byte) in bytes.iter().copied().enumerate() {
            bus.write_physical(
                GuestPhysicalAddress::new(address + offset as u64),
                AccessWidth::Byte,
                u64::from(byte),
                |_| {},
            )
            .unwrap();
        }
    }

    fn read_entry(bus: &mut MachineBus, address: u64) -> u64 {
        bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
            .unwrap()
    }

    fn bus(descriptor_pte: u64) -> MachineBus {
        bus_with_ram_len(descriptor_pte, RAM_LEN)
    }

    fn bus_with_ram_len(descriptor_pte: u64, ram_len: u64) -> MachineBus {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), ram_len).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut bus =
            MachineBus::new(map, RamLimits::new(ram_len), vec![], vec![], vec![]).unwrap();
        put(&mut bus, 0x1000, 0x2007);
        put(&mut bus, 0x2000, 0x3007);
        put(&mut bus, 0x3010, 0x4007);
        put(&mut bus, 0x3020, 0x5007);
        put(&mut bus, 0x4000, 0x7007); // CODE -> GPA 0x7000
        put(&mut bus, 0x5000, descriptor_pte); // descriptor -> configured GPA
        put(&mut bus, 0x7000, 0x0000_0624_5c01_0f); // exact LIDT bytes + sentinel
        put(&mut bus, 0x7008, 0);
        if ram_len >= 0x9018 {
            put(&mut bus, 0x9000, 0x55aa_0000_0000_0000);
            put(&mut bus, 0x9008, 0x0000_7fff_0000_1234);
            put(&mut bus, 0x9010, 0xfeed_face_cafe_beef);
        }
        bus
    }

    struct CountingDevice(Arc<AtomicUsize>);

    impl BusDevice for CountingDevice {
        fn read(&mut self, _: DeviceAccess) -> Result<u64, DeviceError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        }

        fn write(&mut self, _: DeviceAccess, _: u64) -> Result<(), DeviceError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn bus_with_mmio_descriptor(calls: Arc<AtomicUsize>) -> MachineBus {
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x9000).unwrap();
        let mmio = GuestPhysicalRange::new(GuestPhysicalAddress::new(0x9000), 0x1000).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
                MemoryRegionSpec::new(mmio, MemoryRegionKind::Mmio),
            ],
        )
        .unwrap();
        let mut bus = MachineBus::new(
            map,
            RamLimits::new(0x9000),
            vec![Box::new(CountingDevice(calls))],
            vec![MmioBinding {
                region_start: GuestPhysicalAddress::new(0x9000),
                device_index: 0,
            }],
            vec![],
        )
        .unwrap();
        put(&mut bus, 0x1000, 0x2007);
        put(&mut bus, 0x2000, 0x3007);
        put(&mut bus, 0x3010, 0x4007);
        put(&mut bus, 0x3020, 0x5007);
        put(&mut bus, 0x4000, 0x7007);
        put(&mut bus, 0x5000, 0x9007);
        put(&mut bus, 0x7000, 0x0000_0624_5c01_0f);
        bus
    }

    fn state(rsp: u64) -> LongModeState {
        LongModeState {
            rip: CODE,
            cr0: Some(CR0),
            cr3: Some(ROOT),
            cr4: Some(CR4),
            gpr: {
                let mut gpr = [0; 16];
                gpr[od_isa::Register::Rsp as usize] = rsp;
                gpr
            },
            idtr: Some(Idtr {
                base: 0x3000,
                limit: 0x5678,
            }),
            activity: Some(CpuActivity::Running),
            sti_shadow: Some(StiShadow::Clear),
            ..LongModeState::default()
        }
    }

    #[test]
    fn nonidentity_lidt_reads_ten_ram_bytes_and_commits_only_idtr_and_rip() {
        let mut bus = bus(0x9007);
        let mut cpu = state(0x0080_0000);
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |r| callbacks.push(r),
        )
        .unwrap();
        assert_eq!(report.cpu.exit, od_isa::PagedProbeExit::Retired);
        assert_eq!(report.cpu.instructions_executed, 1);
        assert_eq!(report.cpu.rflags_defined, INTEGER_FLAGS);
        assert_eq!(cpu.rip, CODE + 5);
        assert_eq!(
            cpu.idtr,
            Some(Idtr {
                base: 0x0000_7fff_0000_1234,
                limit: 0x55aa
            })
        );
        let mut expected = before;
        expected.rip += 5;
        expected.idtr = cpu.idtr;
        assert_eq!(cpu, expected);
        assert_eq!(report.page_fault, None);
        assert_eq!(report.memory_error, None);
        assert_eq!(
            callbacks.len(),
            6,
            "four A writes for code walk, then the new PDE and PTE for the descriptor"
        );
        assert_eq!(
            callbacks,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );
        assert!(callbacks.iter().all(|range| range.len() == 8));
        for (entry, expected) in [
            (0x1000, 0x2027),
            (0x2000, 0x3027),
            (0x3010, 0x4027),
            (0x4000, 0x7027),
            (0x3020, 0x5027),
            (0x5000, 0x9027),
        ] {
            assert_eq!(read_entry(&mut bus, entry), expected, "PTE at {entry:#x}");
        }
    }

    #[test]
    fn missing_descriptor_page_preserves_cpu_and_reports_exact_pf_after_fetch_a_effects() {
        let mut bus = bus(0);
        let mut cpu = state(0x0080_0000);
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |r| callbacks.push(r),
        )
        .unwrap();
        assert_eq!(cpu, before);
        let record = report.page_fault.unwrap();
        assert_eq!(record.saved_rip(), CODE);
        assert_eq!(record.cr2_value(), 0x0080_0006);
        assert_eq!(record.access(), od_isa::memory::AccessKind::Read);
        assert_eq!(record.fault().error_code, Some(0));
        assert_eq!(
            record,
            od_isa::memory::PageFaultRecord::new(
                CODE,
                od_isa::memory::AccessKind::Read,
                context(),
                0,
                0x0080_0006,
            )
        );
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(
            callbacks.len(),
            5,
            "four code A writes and one eligible descriptor-walk A write"
        );
        assert_eq!(
            callbacks,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x3020].map(|address| GuestPhysicalRange::new(
                GuestPhysicalAddress::new(address),
                8
            )
            .unwrap())
        );
        for (address, expected) in [
            (0x1000, 0x2027),
            (0x2000, 0x3027),
            (0x3010, 0x4027),
            (0x4000, 0x7027),
            (0x3020, 0x5027),
        ] {
            assert_eq!(
                read_entry(&mut bus, address),
                expected,
                "entry at {address:#x}"
            );
        }
        assert_eq!(
            read_entry(&mut bus, 0x5000),
            0,
            "non-present leaf is unchanged"
        );
        assert_eq!(
            report.cpu.exit,
            od_isa::PagedProbeExit::Fault(record.fault().clone())
        );
    }

    #[test]
    fn missing_lgdt_descriptor_page_reports_fault_and_committed_fetch_walks() {
        let mut bus = bus(0);
        put_bytes(&mut bus, 0x7000, &[0x0f, 0x01, 0x55, 0x00]);
        let mut cpu = state(0);
        cpu.gpr[od_isa::Register::Rbp as usize] = 0x0080_0000;
        cpu.gdtr = Some(Gdtr {
            base: 0x2222,
            limit: 0x3333,
        });
        let before = cpu.clone();
        let mut callbacks = Vec::new();

        let report = step_paged_lgdt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();

        let record = report.page_fault.as_ref().unwrap();
        assert_eq!(record.saved_rip(), CODE);
        assert_eq!(record.cr2_value(), 0x0080_0000);
        assert_eq!(record.access(), od_isa::memory::AccessKind::Read);
        assert_eq!(record.fault().error_code, Some(0));
        assert_eq!(
            report.cpu.exit,
            od_isa::PagedProbeExit::Fault(record.fault().clone())
        );
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(report.memory_error, None);
        assert_eq!(cpu, before);
        assert_eq!(
            callbacks,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x3020].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );
    }

    #[test]
    fn mmio_lgdt_descriptor_is_a_backend_error_without_device_dispatch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut bus = bus_with_mmio_descriptor(Arc::clone(&calls));
        put_bytes(&mut bus, 0x7000, &[0x0f, 0x01, 0x55, 0x00]);
        let mut cpu = state(0);
        cpu.gpr[od_isa::Register::Rbp as usize] = 0x0080_0000;
        cpu.gdtr = Some(Gdtr {
            base: 0x2222,
            limit: 0x3333,
        });
        let before = cpu.clone();
        let mut callbacks = Vec::new();

        let report = step_paged_lgdt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();

        assert!(matches!(
            report.cpu.exit,
            od_isa::PagedProbeExit::Backend(_)
        ));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(report.page_fault, None);
        assert!(matches!(
            report.memory_error,
            Some(TrackedAccessError::OperandPhysical { .. })
        ));
        assert_eq!(cpu, before);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            callbacks,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );
    }

    #[test]
    fn tracked_nx_fetch_fault_reports_exact_record_and_committed_a_updates() {
        let mut bus = bus(0x9007);
        put(&mut bus, 0x4000, 0x8000_0000_0000_7007); // NX code leaf.
        let mut cpu = state(0x0080_0000);
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let record = PageFaultRecord::new(
            CODE,
            od_isa::memory::AccessKind::Fetch,
            context(),
            0x11,
            CODE,
        );
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();
        assert_eq!(
            report.cpu.exit,
            od_isa::PagedProbeExit::Fault(record.fault().clone())
        );
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(report.page_fault, Some(record));
        assert_eq!(report.memory_error, None);
        assert_eq!(
            callbacks,
            [0x1000, 0x2000, 0x3010, 0x4000].map(|address| GuestPhysicalRange::new(
                GuestPhysicalAddress::new(address),
                8
            )
            .unwrap())
        );
        for (address, expected) in [
            (0x1000, 0x2027),
            (0x2000, 0x3027),
            (0x3010, 0x4027),
            (0x4000, 0x8000_0000_0000_7027),
        ] {
            assert_eq!(read_entry(&mut bus, address), expected);
        }
    }

    #[test]
    fn positive_and_negative_disp8_load_from_the_signed_rsp_address() {
        for (rsp, disp, descriptor_address) in [
            (0x0080_0000, 0x06, 0x0080_0006),
            (0x0080_0010, 0xf6, 0x0080_0006),
        ] {
            let mut bus = bus(0x9007);
            put_bytes(&mut bus, 0x7004, &[disp]);
            let mut cpu = state(rsp);
            let report = step_paged_lidt(
                &mut cpu,
                &mut bus,
                ROOT,
                EFER,
                context(),
                INTEGER_FLAGS,
                |_| {},
            )
            .unwrap();
            assert_eq!(report.cpu.exit, od_isa::PagedProbeExit::Retired);
            assert_eq!(cpu.idtr.unwrap().base, 0x0000_7fff_0000_1234);
            assert_eq!(descriptor_address, 0x0080_0006);
        }
    }

    #[test]
    fn privilege_fault_occurs_after_fetch_without_operand_walk() {
        for privilege in [
            PrivilegeLevel::Ring1,
            PrivilegeLevel::Ring2,
            PrivilegeLevel::Ring3,
        ] {
            let mut bus = bus(0x9007);
            let mut cpu = state(0x0080_0000);
            let before = cpu.clone();
            let mut callbacks = Vec::new();
            let report = step_paged_lidt(
                &mut cpu,
                &mut bus,
                ROOT,
                EFER,
                AccessContext {
                    privilege,
                    ..context()
                },
                INTEGER_FLAGS,
                |range| callbacks.push(range),
            )
            .unwrap();
            assert_eq!(
                report.cpu.exit,
                od_isa::PagedProbeExit::Fault(ArchitecturalFault {
                    vector: 13,
                    error_code: Some(0),
                    linear_address: None,
                })
            );
            assert_eq!(cpu, before);
            assert_eq!(report.cpu.instructions_executed, 0);
            assert_eq!(report.page_fault, None);
            assert_eq!(report.memory_error, None);
            assert_eq!(callbacks.len(), 4, "only instruction fetch A writes");
            assert!(!callbacks.iter().any(|range| range.start().get() == 0x3020));
        }
    }

    #[test]
    fn lock_ud_and_unsupported_form_precede_operand_access() {
        let mut bus = self::bus(0x9007);
        put_bytes(&mut bus, 0x7000, &[0xf0, 0x0f, 0x01, 0x5c, 0x24, 0x06]);
        let mut cpu = state(0x0080_0000);
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();
        assert_eq!(
            report.cpu.exit,
            od_isa::PagedProbeExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(cpu, before);
        assert_eq!(callbacks.len(), 4);
        assert_eq!(report.page_fault, None);
        assert_eq!(report.memory_error, None);

        let mut bus = self::bus(0x9007);
        put_bytes(&mut bus, 0x7000, &[0x90]); // NOP is outside the exact profile.
        let mut cpu = state(0x0080_0000);
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();
        assert_eq!(
            report.cpu.exit,
            od_isa::PagedProbeExit::Unsupported { opcode: 0x0f }
        );
        assert_eq!(cpu, before);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(callbacks.len(), 4, "unsupported form only fetches code");
        assert_eq!(report.page_fault, None);
        assert_eq!(report.memory_error, None);
    }

    #[test]
    fn source_crossing_page_and_noncanonical_range_are_rejected_before_operand_walk() {
        let mut bus = self::bus(0x9007);
        let mut cpu = state(0x0080_0ff1); // +6 => ...ff7, crossing the guest page.
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();
        assert_eq!(cpu, before);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(report.page_fault, None);
        assert_eq!(
            report.memory_error,
            Some(TrackedAccessError::CrossesPage {
                address: GuestLinearAddress::new(0x0080_0ff7),
                width: 10,
            })
        );
        assert_eq!(
            callbacks.len(),
            4,
            "crossing is rejected before operand walk"
        );

        let mut bus = self::bus(0x9007);
        let mut cpu = state(0x0000_7fff_ffff_fff3); // +6 is canonical; byte 10 is not.
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();
        assert_eq!(
            report.cpu.exit,
            od_isa::PagedProbeExit::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(cpu, before);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(callbacks.len(), 4);
        assert_eq!(report.memory_error, None);
    }

    #[test]
    fn non_ram_descriptor_source_fails_without_device_dispatch() {
        let mut bus = bus(0x30_007); // mapped page-table leaf points outside RAM.
        let mut cpu = state(0x0080_0000);
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();
        assert_eq!(cpu, before);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(report.page_fault, None);
        assert!(matches!(
            report.memory_error,
            Some(TrackedAccessError::OperandPhysical { .. })
        ));
        assert_eq!(callbacks.len(), 6);
    }

    #[test]
    fn registered_mmio_descriptor_target_fails_without_device_callbacks() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut bus = bus_with_mmio_descriptor(Arc::clone(&calls));
        let mut cpu = state(0x0080_0000);
        let before = cpu.clone();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |_| {},
        )
        .unwrap();

        assert_eq!(cpu, before);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(report.page_fault, None);
        assert!(matches!(
            report.memory_error,
            Some(TrackedAccessError::OperandPhysical { .. })
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn descriptor_read_with_ram_start_and_unmapped_tail_preserves_destination_and_idtr() {
        let mut bus = bus_with_ram_len(0x8007, 0x800b);
        let mut cpu = state(0x0080_0000);
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();
        assert_eq!(cpu, before);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(report.page_fault, None);
        assert!(matches!(
            report.memory_error,
            Some(TrackedAccessError::OperandPhysical { .. })
        ));
        assert_eq!(callbacks.len(), 6);

        // Also exercise the adapter's caller-buffer contract directly: the
        // physical range starts in RAM at GPA 0x8006 and its tail exceeds RAM.
        let paging = Long64ProbeConfig::from_cpu_state(&cpu, ROOT, EFER).unwrap();
        let mut notify = |_| {};
        let mut memory = PagedDescriptorMemory {
            bus: &mut bus,
            paging,
            instruction_rip: CODE,
            instruction_name: "LIDT",
            segment_load: false,
            committed_write: &mut notify,
            page_fault: None,
            memory_error: None,
        };
        let mut destination = [0xa5; 10];
        let result = memory.read(0x0080_0006, context(), &mut destination);
        assert!(result.is_err());
        assert_eq!(destination, [0xa5; 10]);
        assert!(matches!(
            memory.memory_error,
            Some(TrackedAccessError::OperandPhysical { .. })
        ));
        drop(memory);

        assert_eq!(cpu.idtr, before.idtr);
        assert_eq!(cpu, before);
        assert_eq!(read_entry(&mut bus, 0x5000), 0x8027);
    }

    #[test]
    fn aliased_descriptor_read_observes_committed_accessed_bit() {
        let mut bus = bus(0x3007); // Descriptor aliases the PD entry at GPA 0x3020.
        let mut cpu = state(0x0080_001a); // RSP + 6 starts at GPA 0x3020.
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();

        assert_eq!(report.cpu.exit, od_isa::PagedProbeExit::Retired);
        assert_eq!(report.cpu.instructions_executed, 1);
        assert_eq!(read_entry(&mut bus, 0x3020), 0x5027);
        assert_eq!(cpu.idtr.unwrap().limit, 0x5027);
        assert_eq!(cpu.idtr.unwrap().base, 0);
        assert!(callbacks.iter().any(|range| range.start().get() == 0x3020));
    }

    #[test]
    fn read_only_descriptor_mapping_succeeds_at_last_ten_byte_page_offset() {
        let mut bus = bus(0x9005);
        let descriptor = [0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00];
        put_bytes(&mut bus, 0x9ff6, &descriptor);
        let mut cpu = state(0x0080_0ff0); // RSP + 6 = page offset 0xff6.
        let before = cpu.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_lidt(
            &mut cpu,
            &mut bus,
            ROOT,
            EFER,
            context(),
            INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();

        assert_eq!(report.cpu.exit, od_isa::PagedProbeExit::Retired);
        assert_eq!(report.cpu.instructions_executed, 1);
        let mut expected = before;
        expected.rip += 5;
        expected.idtr = Some(Idtr {
            limit: 0x1234,
            base: 0x0000_0000_1234_5678,
        });
        assert_eq!(cpu, expected);
        assert_eq!(read_entry(&mut bus, 0x5000), 0x9025);
        assert!(
            !callbacks
                .iter()
                .any(|range| range.start().get() == 0x5000 + 8)
        );
    }
}

fn canonical48(address: u64) -> bool {
    (address as i64) >> 47 == 0 || (address as i64) >> 47 == -1
}
